//! kanban connects rush to kanban-code (github.com/langwatch/kanban-code),
//! the board that shows coding agents as cards. kanban-code knows what the
//! work is: cards, their issues, pull requests, checks and review threads.
//! rush runs the agents. This plugin joins the two:
//!
//! - Claude can read the board, and the card it's working on, with the
//!   issue, the PR, failing checks and unresolved review threads.
//! - Claude (or you, through it) can start an agent on a card: in rush, in
//!   the card's worktree or a new one, tagged with the card. Once the
//!   agent's conversation has begun, the plugin has kanban-code link the
//!   card to it, so the card follows the agent across the board.
//! - When a card's PR gets a failing check or a new review thread, the
//!   agent working on it hears about it: the plugin queues it a message,
//!   sent when its turn ends.
//! - rush's agent list can show the board: a section per column, and each
//!   card's agent under the card's name (`sidebar.set`).
//!
//! It reads ~/.kanban-code/links.json, and changes cards only through the
//! kanban CLI, which asks the running app to: the app owns the file.
//!
//! ```sh
//! cargo build --release -p kanban
//! mkdir -p ~/.config/rush/plugins/kanban
//! cp target/release/kanban plugins/examples/kanban/plugin.json ~/.config/rush/plugins/kanban/
//! rush plugin approve kanban
//! ```

mod cards;
mod sidebar;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rush_plugin_sdk::proto::{ExecResult, Initialize, Session, Started, ToolCall};
use rush_plugin_sdk::{CallError, Conn, Error, params, tool};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use cards::{Card, card_of, describe, find_card, read_cards, slug};

/// What the plugin keeps across restarts, in its data folder.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Memory {
    /// The card each conversation has been linked to, by Claude Code
    /// session id.
    #[serde(deserialize_with = "rush_plugin_sdk::proto::nullable")]
    linked: BTreeMap<String, String>,
    /// Failed attempts at linking, by Claude Code session id.
    #[serde(deserialize_with = "rush_plugin_sdk::proto::nullable")]
    tries: BTreeMap<String, u32>,
    /// Each card's PRs as last looked at, so only what's new is passed on.
    #[serde(deserialize_with = "rush_plugin_sdk::proto::nullable")]
    seen: BTreeMap<String, BTreeMap<i64, PrSeen>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct PrSeen {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    failing: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    threads: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

#[derive(Default)]
struct State {
    data: PathBuf,
    /// rush's sessions, by rush id.
    sessions: HashMap<String, Session>,
    /// Until rush has sent its sessions, a card can't tell it already has
    /// an agent.
    watching: bool,
    mem: Memory,
    /// Relinks under way, by Claude Code session id.
    linking: HashSet<String>,
    /// When each was last tried.
    tried: HashMap<String, Instant>,
}

struct App {
    state: Mutex<State>,
    /// Woken when watching starts.
    watched: Condvar,
}

impl App {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn main() {
    let app = Arc::new(App {
        state: Mutex::new(State::default()),
        watched: Condvar::new(),
    });
    rush_plugin_sdk::serve(move |conn, method, p| handle(conn, &app, method, p))
}

fn handle(conn: &Conn, app: &Arc<App>, method: &str, p: Value) -> Result<Value, Error> {
    match method {
        "initialize" => {
            let init: Initialize = serde_json::from_value(p).unwrap_or_default();
            {
                let mut st = app.lock();
                st.mem = load(Path::new(&init.data_dir));
                st.data = init.data_dir.into();
            }
            let (c, a) = (conn.clone(), app.clone());
            std::thread::spawn(move || follow(&c, &a));
            let c = conn.clone();
            std::thread::spawn(move || sidebar::show_board(&c));
            Ok(json!({}))
        }
        "tools.list" => Ok(json!({"tools": tools()})),
        "tools.call" => {
            let tc: ToolCall = params(p)?;
            let me = Session {
                id: tc.session,
                session_id: tc.session_id,
                cwd: tc.cwd,
                meta: tc.meta,
                ..Default::default()
            };
            Ok(tool::result(call(conn, app, &me, &tc.name, tc.arguments)))
        }
        "session.changed" => {
            if let Ok(s) = serde_json::from_value::<Session>(p) {
                app.lock().sessions.insert(s.id.clone(), s.clone());
                // Linking calls rush, which answers on the thread this
                // notification holds up: it goes on a thread of its own.
                let (c, a) = (conn.clone(), app.clone());
                std::thread::spawn(move || link(&c, &a, &s));
            }
            Ok(Value::Null)
        }
        "session.gone" => {
            if let Some(id) = p.get("id").and_then(Value::as_str) {
                app.lock().sessions.remove(id);
            }
            Ok(Value::Null)
        }
        _ => Err(Error::no_method(method)),
    }
}

/// Watches rush's sessions, and every minute looks at the board for news
/// for the agents working on cards.
fn follow(conn: &Conn, app: &App) {
    // rush takes the watch once initialize has been answered.
    loop {
        match conn.call::<_, Vec<Session>>("sessions.watch", &()) {
            Ok(list) => {
                {
                    let mut st = app.lock();
                    for s in &list {
                        st.sessions.insert(s.id.clone(), s.clone());
                    }
                    st.watching = true;
                }
                app.watched.notify_all();
                for s in &list {
                    link(conn, app, s);
                }
                break;
            }
            Err(_) => {
                if conn.wait_timeout(Duration::from_secs(1)) {
                    return;
                }
            }
        }
    }
    loop {
        review(conn, app);
        if conn.wait_timeout(Duration::from_secs(60)) {
            return;
        }
        let all: Vec<Session> = app.lock().sessions.values().cloned().collect();
        for s in &all {
            link(conn, app, s); // retries what failed, kanban-code not running say
        }
    }
}

/// Has kanban-code link a card to the conversation of the agent the plugin
/// started for it, once there is one. Only the app may change a card, so
/// the kanban CLI asks it to; if it isn't running, this tries again later.
fn link(conn: &Conn, app: &App, s: &Session) {
    let Some(card) = s.meta.get("card").filter(|c| !c.is_empty()) else {
        return;
    };
    if s.started_by != "kanban" || s.session_id.is_empty() {
        return;
    }
    let sid = &s.session_id;
    {
        let mut st = app.lock();
        let recent = st
            .tried
            .get(sid)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        if st.mem.linked.get(sid) == Some(card)
            || st.linking.contains(sid)
            || st.mem.tries.get(sid).copied().unwrap_or(0) >= 20
            || recent
        {
            return;
        }
        st.linking.insert(sid.clone());
        st.tried.insert(sid.clone(), Instant::now());
    }
    let out = kanban(conn, &["relink", card, sid, "--json"]);
    let mut st = app.lock();
    st.linking.remove(sid);
    match out {
        Err(e) => {
            let n = st.mem.tries.entry(sid.clone()).or_default();
            *n += 1;
            conn.log(format!("linking card {card} to {sid}: {e} (try {n})"));
        }
        Ok(out) => {
            st.mem.linked.insert(sid.clone(), card.clone());
            st.mem.tries.remove(sid);
            conn.log(format!("linked card {card} to {sid}: {}", out.trim()));
        }
    }
    save(&st);
}

/// Passes on what's new on the PRs of cards an agent in rush is working
/// on: a check that started failing, review threads that appeared. The
/// first look at a card only notes how it is.
fn review(conn: &Conn, app: &App) {
    let Ok(cards) = read_cards() else {
        return;
    };
    let live: Vec<Session> = app
        .lock()
        .sessions
        .values()
        .filter(|s| s.live())
        .cloned()
        .collect();
    for s in &live {
        let Some(c) = card_of(&cards, s).filter(|c| !c.pr_links.is_empty()) else {
            continue;
        };
        let news = {
            let mut st = app.lock();
            let was = st.mem.seen.get(&c.id);
            let (now, news) = pr_news(&c.pr_links, was);
            st.mem.seen.insert(c.id.clone(), now);
            save(&st);
            news
        };
        if news.is_empty() {
            continue;
        }
        let text = format!(
            "News on your kanban card, {}:\n- {}\nLook into it (gh pr checks, gh pr view --comments), fix what's yours, push, and say what you did.",
            c.title(),
            news.join("\n- ")
        );
        if let Err(e) = conn.call::<_, Value>("sessions.queue", &json!({"id": s.id, "text": text}))
        {
            conn.log(format!("telling {} about card {}: {e}", s.id, c.id));
        }
    }
}

/// Compares a card's PRs with how they were last seen (None: not yet, so
/// there's nothing to compare with, and no news): checks that started
/// failing, and review threads that appeared.
fn pr_news(
    prs: &[cards::Pr],
    was: Option<&BTreeMap<i64, PrSeen>>,
) -> (BTreeMap<i64, PrSeen>, Vec<String>) {
    let mut now = BTreeMap::new();
    let mut news = Vec::new();
    for p in prs {
        let seen = PrSeen {
            failing: p.failing(),
            threads: p.unresolved_threads,
        };
        now.insert(p.number, seen.clone());
        let Some(was) = was else {
            continue;
        };
        let old = was.get(&p.number).cloned().unwrap_or_default();
        let newly: Vec<&str> = seen
            .failing
            .iter()
            .filter(|f| !old.failing.contains(f))
            .map(String::as_str)
            .collect();
        if !newly.is_empty() {
            news.push(format!(
                "PR #{}: checks failing: {}.",
                p.number,
                newly.join(", ")
            ));
        }
        if seen.threads > old.threads {
            news.push(format!(
                "PR #{}: {} unresolved review thread(s), {} new; the first is {}",
                p.number,
                seen.threads,
                seen.threads - old.threads,
                p.first_unresolved_thread_url
            ));
        }
    }
    (now, news)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Args {
    card: String,
    column: String,
    prompt: String,
    #[serde(rename = "permissionMode")]
    mode: String,
    text: String,
}

fn call(conn: &Conn, app: &App, me: &Session, name: &str, args: Value) -> Result<String, String> {
    let a: Args = serde_json::from_value(args).unwrap_or_default();
    let cards = read_cards()?;
    match name {
        "board" => Ok(cards::board(&cards, &a.column)),
        "my_card" => card_of(&cards, me)
            .map(describe)
            .ok_or_else(|| "this session isn't working on a card on the board".to_string()),
        "start_card" => {
            let c = find_card(&cards, &a.card)?;
            if let Some(s) = working(app, &cards, c) {
                return Err(format!(
                    "{} is already being worked on by {} ({}); send it a message with card_message",
                    c.title(),
                    s.name,
                    s.id
                ));
            }
            start(conn, c, &a.prompt, &a.mode).map_err(|e| e.to_string())
        }
        "card_message" => {
            let c = find_card(&cards, &a.card)?;
            let Some(s) = working(app, &cards, c) else {
                return Err(format!(
                    "no agent in rush is working on {}; start one with start_card",
                    c.title()
                ));
            };
            conn.call::<_, Value>("sessions.queue", &json!({"id": s.id, "text": a.text}))
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "Queued for {}; it goes when its turn ends.",
                s.name
            ))
        }
        _ => Err(format!("no tool named {name}")),
    }
}

/// The live rush session working on a card, if there is one.
fn working(app: &App, cards: &[Card], c: &Card) -> Option<Session> {
    // Just started, rush may not have said yet which agents are running:
    // wait a moment rather than start a second one on the card.
    let st = app.lock();
    let (st, _) = app
        .watched
        .wait_timeout_while(st, Duration::from_secs(3), |st| !st.watching)
        .unwrap_or_else(|e| e.into_inner());
    st.sessions
        .values()
        .find(|s| s.live() && card_of(cards, s).is_some_and(|o| o.id == c.id))
        .cloned()
}

/// Starts an agent on a card: in its worktree if it has one, else in a new
/// worktree of its project, on a branch named after it.
fn start(conn: &Conn, c: &Card, prompt: &str, mode: &str) -> Result<String, String> {
    let prompt = match prompt.trim() {
        "" => task(c),
        p => p.to_string(),
    };
    let mut req = json!({"prompt": prompt, "name": c.title(), "meta": {"card": c.id}});
    if !mode.is_empty() {
        req["permissionMode"] = json!(mode);
    }
    match &c.worktree_link {
        Some(wt) if is_dir(&wt.path) => req["cwd"] = json!(wt.path),
        _ if !c.project_path.is_empty() && is_dir(&c.project_path) => {
            let s = slug(&c.title());
            let tail = c.id.get(c.id.len().saturating_sub(6)..).unwrap_or(&c.id);
            req["cwd"] = json!(c.project_path);
            req["worktree"] = json!({"name": s, "branch": format!("{s}-{tail}")});
        }
        _ => {
            return Err(format!(
                "{} has no project folder or worktree to work in",
                c.title()
            ));
        }
    }
    let out: Started = conn
        .call("sessions.start", &req)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Started an agent on {}: {}, in {}. kanban-code links the card to it once its conversation has begun.",
        c.title(),
        out.id,
        out.cwd
    ))
}

/// What an agent starting on a card is asked to do.
fn task(c: &Card) -> String {
    let mut b = if !c.prompt_body.is_empty() {
        c.prompt_body.clone()
    } else if let Some(is) = &c.issue_link {
        let mut b = format!("Resolve issue #{}: {}\n{}", is.number, is.title, is.url);
        let body = is.body.trim();
        if !body.is_empty() {
            b += &format!("\n\n{body}");
        }
        b
    } else {
        c.title()
    };
    b += &format!(
        "\n\nThis is kanban card {}; mcp__rush-kanban__my_card shows it, with its PR and review state once there is one.",
        c.id
    );
    b
}

/// Runs the kanban CLI, which rush runs for the plugin, outside its
/// sandbox.
fn kanban(conn: &Conn, args: &[&str]) -> Result<String, CallError> {
    let out: ExecResult = conn.call("exec", &json!({"name": "kanban", "args": args}))?;
    if out.code != 0 {
        let msg = if out.stderr.is_empty() {
            out.stdout
        } else {
            out.stderr
        };
        return Err(CallError::Other(msg.trim().to_string()));
    }
    Ok(out.stdout)
}

fn is_dir(p: &str) -> bool {
    Path::new(p).is_dir()
}

/// What it kept, in data; nothing, if there's nothing there to read.
fn load(data: &Path) -> Memory {
    std::fs::read(data.join("state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Keeps the memory, whole, in place of what was kept.
fn save(st: &State) {
    if st.data.as_os_str().is_empty() {
        return;
    }
    let Ok(b) = serde_json::to_vec_pretty(&st.mem) else {
        return;
    };
    let tmp = st.data.join("state.json.tmp");
    if write_private(&tmp, &b).is_ok() {
        let _ = std::fs::rename(&tmp, st.data.join("state.json"));
    }
}

fn write_private(path: &Path, b: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(b)
}

fn tools() -> Value {
    json!([
        tool::tool(
            "board",
            "The kanban-code board: every card by column (In Progress, Waiting, In Review, Backlog, Done), with its id, branch and PR status.",
            tool::schema(
                &[(
                    "column",
                    "Only this column: in_progress, requires_attention, in_review, backlog or done."
                )],
                &[]
            ),
        ),
        tool::tool(
            "my_card",
            "The kanban-code card you're working on: its task or issue, its worktree, and its PRs with their status, failing checks and unresolved review threads.",
            tool::schema(&[], &[]),
        ),
        tool::tool(
            "start_card",
            "Start an agent in rush on a kanban-code card: in the card's worktree, or a new worktree of its project. It's asked to do the card's task or issue, unless you give a prompt. The card follows it across the board.",
            tool::schema(
                &[
                    ("card", "The card's id, id prefix, or name."),
                    (
                        "prompt",
                        "What to ask the agent, instead of the card's own task."
                    ),
                    (
                        "permissionMode",
                        "default, acceptEdits or plan. Your default if left out."
                    ),
                ],
                &["card"]
            ),
        ),
        tool::tool(
            "card_message",
            "Send a message to the agent in rush working on a kanban-code card. It goes when the agent's turn ends.",
            tool::schema(
                &[
                    ("card", "The card's id, id prefix, or name."),
                    ("text", "The message.")
                ],
                &["card", "text"]
            ),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINKS: &str = r#"{"links": [
  {"id": "card_bbb222", "column": "backlog", "projectPath": "/src/app", "issueLink": {"number": 12, "title": "Dark mode", "body": "Please."}}
]}"#;

    #[test]
    fn asks_for_the_cards_task() {
        let cards = cards::parse_cards(LINKS.as_bytes()).unwrap();
        let t = task(&cards[0]);
        assert!(
            t.starts_with("Resolve issue #12: Dark mode")
                && t.contains("Please.")
                && t.contains("card_bbb222"),
            "task = {t:?}"
        );
    }

    #[test]
    fn passes_on_only_whats_new() {
        let mut p: Vec<cards::Pr> = serde_json::from_str(
            r#"[{"number": 7, "unresolvedThreads": 1, "firstUnresolvedThreadURL": "u",
                "checkRuns": [{"name": "lint", "conclusion": "failure"}]}]"#,
        )
        .unwrap();
        let (seen, news) = pr_news(&p, None);
        assert!(
            news.is_empty(),
            "the first look should only note how it is: {news:?}"
        );
        let (_, news) = pr_news(&p, Some(&seen));
        assert!(news.is_empty(), "nothing changed, yet: {news:?}");
        p[0].unresolved_threads = 3;
        let mut test = p[0].check_runs[0].clone();
        test.name = "test".into();
        p[0].check_runs.push(test);
        let (_, news) = pr_news(&p, Some(&seen));
        assert_eq!(news.len(), 2, "{news:?}");
        assert!(news[0].contains("checks failing: test."), "{news:?}");
        assert!(
            news[1].contains("3 unresolved review thread(s), 2 new"),
            "{news:?}"
        );
    }

    #[test]
    fn keeps_its_memory() {
        let dir = std::env::temp_dir().join(format!("kanban-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = State {
            data: dir.clone(),
            ..Default::default()
        };
        st.mem.linked.insert("s-1".into(), "card_a".into());
        st.mem.seen.insert(
            "card_a".into(),
            [(
                7,
                PrSeen {
                    failing: vec!["lint".into()],
                    threads: 2,
                },
            )]
            .into(),
        );
        save(&st);
        let m = load(&dir);
        assert_eq!(m.linked["s-1"], "card_a");
        assert_eq!(m.seen["card_a"][&7].threads, 2);
        assert!(load(&dir.join("nowhere")).linked.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn manifest_is_valid() {
        let m = rush_plugin_sdk::Manifest::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(m.sidebar && m.exec.contains_key("kanban"));
        assert_eq!(tools().as_array().unwrap().len(), 4);
    }
}
