//! Frames of the list and a session over fixture agents, as
//! `rush --render` draws them; nothing read from ~/.claude.
//! UPDATE_SNAPSHOTS=1 writes the snapshots again.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::collections::HashMap;
use std::path::PathBuf;

use rush_core::agent::{Agent, State, SubagentStats};
use rush_core::time::Ms;
use rush_draw::{Key, KeyMods};
use rush_ui::{Fleet, Model};

const NOW: Ms = 1_793_190_896_789;
const MIN: Ms = 60_000;
const HOUR: Ms = 60 * MIN;

#[allow(clippy::too_many_arguments)]
fn agent(
    id: &str,
    name: &str,
    state: State,
    pid: i32,
    cpu: f32,
    mem_mb: u64,
    cost: f64,
    ago: Ms,
) -> Agent {
    let mut a = Agent {
        key: format!("default/{id}"),
        display_name: name.into(),
        repo: "/src/checkout".into(),
        branch: "main".into(),
        pid,
        cpu,
        mem: mem_mb << 20,
        interactive: true,
        ..Agent::default()
    };
    a.job.id = id.into();
    a.job.state = state;
    a.job.cwd = "/src/checkout".into();
    a.job.created_at = NOW - ago - 20 * MIN;
    a.job.updated_at = NOW - ago;
    a.spend.cost = cost;
    a.spend.first = NOW - ago - 20 * MIN;
    a.spend.last = NOW - ago;
    a.spend.ready = true;
    a
}

fn fixture() -> Vec<Agent> {
    let mut ask = agent(
        "a1",
        "fix the flaky tax rounding test",
        State::Blocked,
        101,
        1.0,
        332,
        0.19,
        2 * MIN,
    );
    ask.job.needs = "asks: Round half up, or half to even?".into();
    let mut pay = agent(
        "b2",
        "add Apple Pay to checkout",
        State::Working,
        102,
        31.0,
        874,
        2.62,
        MIN,
    );
    pay.subs = SubagentStats {
        spawned: 1,
        direct: 1,
        nested: 0,
        ..SubagentStats::default()
    };
    pay.spend.context = 140_000;
    pay.job.detail = "subagent: draft the rollout notes".into();
    pay.branch = "feat/apple-pay".into();
    let tokens = agent(
        "c3",
        "move the design tokens to CSS variables",
        State::Working,
        103,
        79.0,
        316,
        3.10,
        MIN / 2,
    );
    let idle = agent(
        "d4",
        "set up caching for CI",
        State::Idle,
        104,
        0.0,
        120,
        1.10,
        10 * MIN,
    );
    let mut past = agent(
        "e5",
        "why does the ledger import time out on big files?",
        State::Stopped,
        0,
        0.0,
        0,
        1.71,
        3 * HOUR,
    );
    past.past = true;
    let old = agent(
        "f6",
        "rename Invoice to Bill everywhere",
        State::Done,
        0,
        0.0,
        0,
        0.42,
        72 * HOUR,
    );
    vec![ask, pay, tokens, idle, past, old]
}

fn model() -> Model {
    // As the binary does: the list names each agent through the registry.
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| rush_core::adapter::global().register(rush_claude::Claude::new()));
    // Times are drawn in the local zone: the snapshots are UTC's, whatever
    // the machine's. Before any test formats a time (every test starts here).
    static ZONE: std::sync::Once = std::sync::Once::new();
    // SAFETY: once, ahead of every other use of the environment in this binary.
    ZONE.call_once(|| unsafe { std::env::set_var("TZ", "UTC") });
    // The battery and disk as the snapshots show them, whatever the
    // machine's.
    rush_sys::sysinfo::pin(
        rush_sys::sysinfo::Battery {
            present: true,
            percent: 80,
            charging: false,
        },
        rush_sys::sysinfo::Disk {
            free: 37 << 30,
            total: 500 << 30,
        },
    );
    let mut m = rush_ui::new_model("0.1.0");
    m.clock = Some(NOW);
    m.launch_dir = "/src/checkout".into();
    let agents = fixture();
    let roots = agents.iter().map(|a| a.repo.clone()).collect();
    m.set_fleet(Fleet {
        agents,
        roots,
        previews: HashMap::new(),
        priced: true,
        accounts: Vec::new(),
    });
    m
}

fn snapshot(name: &str, got: &str) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !p.exists() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap();
    if got != want {
        eprintln!("=====SNAPSHOT {name}=====\n{got}=====END {name}=====");
    }
    assert_eq!(got, want, "{name} differs; UPDATE_SNAPSHOTS=1 to accept");
}

fn plain(m: &mut Model, w: u16, h: u16, keys: &[(Key, KeyMods)]) -> String {
    rush_ui::frame(m, w, h, keys, false)
}

#[test]
fn list_at_120() {
    let mut m = model();
    let f = plain(&mut m, 120, 40, &[]);
    snapshot("list_120x40", &f);
    let rows: Vec<&str> = f.lines().collect();
    assert!(rows[1].contains("rush   ● 1 needs you   ✻ 2 working"));
    assert!(!rows[1].contains("finished"));
    assert!(rows[6].starts_with("  AGENTS▾"));
    assert!(rows[6].ends_with("RUNNING   CPU    RAM TOKENS    COST   TIME"));
    // What needs you floats to the top, picked.
    assert_eq!(rows[7].trim_end(), "┃ Needs you  1 · $0.19");
    assert!(rows[8].starts_with("▌ └─ ? fix the flaky tax rounding test ✻ Claude Co…"));
    assert!(rows[8].contains("asks: Round half up,…"));
    assert!(f.contains("┃ Working  2 · $5.72"));
    assert!(f.contains(" ▸ Earlier  1 · $0.42"));
}

#[test]
fn narrow_lists_stack_and_drop_columns() {
    let mut m = model();
    let f = plain(&mut m, 70, 30, &[]);
    snapshot("list_70x30", &f);
    let head = f.lines().nth(6).unwrap();
    assert!(!head.contains("CPU") && !head.contains("TOKENS") && !head.contains("LATEST"));
    assert!(head.contains("RAM") && head.contains("COST") && head.contains("TIME"));
    // Stacked: the question hangs under its name.
    assert!(f.contains("╰ asks: Round half up"));
    let f = plain(&mut m, 50, 24, &[]);
    snapshot("list_50x24", &f);
    assert!(f.lines().next().unwrap().contains("RUSH"));
}

#[test]
fn keys_move_and_fold() {
    let mut m = model();
    let down = (Key::Down, KeyMods::NONE);
    plain(&mut m, 120, 40, &[down]);
    assert_eq!(m.sel, "§Working");
    plain(&mut m, 120, 40, &[down, down]);
    assert_eq!(m.sel, "default/c3");
    // Enter on a heading folds it.
    let up = (Key::Up, KeyMods::NONE);
    let enter = (Key::Enter, KeyMods::NONE);
    let f = plain(&mut m, 120, 40, &[up, up, enter]);
    assert!(f.contains("▸ Working  2 · $5.72"), "{f}");
    // ctrl+n goes to what needs you.
    plain(&mut m, 120, 40, &[(Key::Char('n'), KeyMods::CTRL)]);
    assert_eq!(m.sel, "default/a1");
}

#[test]
fn opens_a_session_read_only() {
    let dir = std::env::temp_dir().join(format!("rush-ui-render-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"user","timestamp":"2026-10-28T12:00:00Z","cwd":"/src/checkout","message":{"role":"user","content":"fix the test"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:02Z","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go test ./..."}}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
            r#"{"type":"user","timestamp":"2026-10-28T12:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]},"toolUseResult":{"stdout":"ok","stderr":""}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Fixed it."}]}}"#,
            r#"{"type":"system","subtype":"turn_duration","timestamp":"2026-10-28T12:00:13Z"}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let mut m = model();
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].job.transcript_path = path.to_string_lossy().into_owned();
    m.sel = m.agents[i].key.clone();
    let f = plain(&mut m, 100, 30, &[(Key::Enter, KeyMods::NONE)]);
    assert!(m.session.is_some());
    // The name, its @handle, then its state (rushmode.go:2924).
    assert!(
        f.contains("set up caching for CI   @set-up-caching-for-ci  ·  idle"),
        "{f}"
    );
    assert!(f.contains("esc back to the list"), "{f}");
    assert!(f.contains("fix the test"), "{f}");
    assert!(f.contains("Fixed it."), "{f}");
    // ↑ picks a row; esc drops it, esc again goes back to the list.
    plain(&mut m, 100, 30, &[(Key::Up, KeyMods::NONE)]);
    assert!(!m.session.as_ref().unwrap().sel.is_empty());
    let esc = (Key::Esc, KeyMods::NONE);
    plain(&mut m, 100, 30, &[esc]);
    assert!(m.session.as_ref().unwrap().sel.is_empty());
    let f = plain(&mut m, 100, 30, &[esc]);
    assert!(m.session.is_none());
    assert!(f.contains("AGENTS▾"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An agent whose adapter isn't Claude Code's reads its history back
/// through its HistoryFollower: the view opens it all the same.
struct Hist;

impl rush_core::adapter::Adapter for Hist {
    fn kind(&self) -> rush_core::adapter::Kind {
        rush_core::adapter::Kind::from("histfake")
    }
    fn name(&self) -> String {
        "Hist".into()
    }
    fn features(&self) -> rush_core::adapter::Features {
        Default::default()
    }
    fn level(&self) -> rush_core::adapter::Level {
        Default::default()
    }
    fn profiles(&self) -> Vec<rush_core::adapter::Profile> {
        Vec::new()
    }
    rush_core::has!(history_follower: HistoryFollower);
}

struct Once(Option<Vec<rush_core::event::Event>>);

impl rush_core::adapter::Follow for Once {
    fn next(&mut self, _: usize) -> rush_core::adapter::Result<rush_core::adapter::Slice> {
        Ok(rush_core::adapter::Slice {
            events: self.0.take().unwrap_or_default(),
            ..Default::default()
        })
    }
    fn wait_on(&self) -> rush_core::adapter::WaitOn {
        Default::default()
    }
}

impl rush_core::adapter::HistoryFollower for Hist {
    fn follow_history(
        &self,
        s: &rush_core::adapter::Session,
    ) -> rush_core::adapter::Result<Box<dyn rush_core::adapter::Follow>> {
        use rush_core::event::{Event, Message, Part, PartKind};
        let msg = |role: &str, text: &str| {
            Event::Message(Message {
                role: role.into(),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text: text.into(),
                    ..Part::default()
                }],
                ..Message::default()
            })
        };
        Ok(Box::new(Once(Some(vec![
            msg("user", &format!("hello from {}", s.id)),
            msg("assistant", "Read back through my adapter."),
        ]))))
    }
}

#[test]
fn opens_any_adapters_history() {
    let mut m = model();
    rush_core::adapter::global().register(Hist);
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].kind = "histfake".into();
    m.agents[i].job.session_id = "s-42".into();
    m.sel = m.agents[i].key.clone();
    let f = plain(&mut m, 100, 30, &[(Key::Enter, KeyMods::NONE)]);
    assert!(m.session.is_some(), "{f}");
    assert!(f.contains("hello from s-42"), "{f}");
    assert!(f.contains("Read back through my adapter."), "{f}");
}

#[test]
fn columns_drop_by_width() {
    let m = model();
    // RUNNING CPU RAM TOKENS COST TIME
    let cases: &[(usize, [usize; 6])] = &[
        (120, [10, 6, 7, 7, 8, 7]),
        (96, [10, 6, 7, 7, 8, 7]),
        (95, [0, 0, 7, 7, 8, 7]),
        (76, [0, 0, 7, 7, 8, 7]),
        (75, [0, 0, 7, 0, 8, 7]),
        (63, [0, 0, 0, 0, 8, 7]),
        (43, [0, 0, 0, 0, 0, 7]),
    ];
    for &(w, want) in cases {
        assert_eq!(m.col_widths(w), want, "at {w}");
    }
    let mut m = model();
    m.cfg.hide_columns = vec!["cpu".into(), "cost".into()];
    assert_eq!(m.col_widths(120), [10, 0, 7, 7, 0, 7]);
}

#[test]
fn rows_say_what_each_agent_is_doing() {
    let mut m = model();
    plain(&mut m, 140, 40, &[]);
    let row = |m: &Model, id: &str| {
        let i = m.agents.iter().position(|a| a.job.id == id).unwrap();
        m.agent_line(i, 136, 140, false, 40, false, None).plain()
    };
    // marker, name, badge … RUNNING CPU RAM TOKENS COST TIME
    let cases: &[(&str, &str, &str)] = &[
        (
            "a1",
            " ? fix the flaky tax rounding test ✻",
            "1%   332M          $0.19   2m ● ",
        ),
        (
            "b2",
            " ✳ add Apple Pay to checkout ✻",
            "↳1   31%   874M   140k   $2.62    21m ",
        ),
        (
            "c3",
            " ✳ move the design tokens to CSS variables ✻",
            "79%   316M          $3.10    20m ",
        ),
        (
            "d4",
            " ◦ set up caching for CI ✻",
            "0%   120M          $1.10    10m ",
        ),
        (
            "e5",
            " · why does the ledger import time out on big files? ✻",
            "$1.71     3h ",
        ),
    ];
    for &(id, start, end) in cases {
        let r = row(&m, id);
        assert!(r.starts_with(start), "{id}: {r:?}");
        assert!(r.ends_with(end), "{id}: {r:?}");
    }
    let (s, _, _) = m.row_summary(m.agents.iter().position(|a| a.job.id == "a1").unwrap());
    assert_eq!(s, "asks: Round half up, or half to even?");
}

/// A transcript of n turns, each a question and its answer, in a temp
/// folder; the agent d4 reads it.
fn long_session(m: &mut Model, name: &str, lines: &[String]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rush-ui-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].job.transcript_path = path.to_string_lossy().into_owned();
    m.sel = m.agents[i].key.clone();
    dir
}

fn turns(n: usize) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..n {
        out.push(format!(
            r#"{{"type":"user","timestamp":"2026-10-28T12:{:02}:00Z","cwd":"/src/checkout","message":{{"role":"user","content":"question {i}"}}}}"#,
            i % 60
        ));
        out.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-10-28T12:{:02}:05Z","message":{{"id":"m{i}","role":"assistant","content":[{{"type":"text","text":"answer {i}"}}]}}}}"#,
            i % 60
        ));
    }
    out
}

/// Home goes to the conversation's real top, and the view stays full;
/// end follows it again.
#[test]
fn home_and_end_in_a_long_session() {
    let mut m = model();
    let dir = long_session(&mut m, "home", &turns(80));
    let key = |k: Key| (k, KeyMods::NONE);
    let f = plain(&mut m, 100, 30, &[key(Key::Enter)]);
    assert!(f.contains("answer 79"), "{f}");
    let f = plain(&mut m, 100, 30, &[key(Key::Home)]);
    assert!(f.contains("question 0"), "{f}");
    assert!(!f.contains("answer 79"), "{f}");
    // As many rows of the conversation as before: none scrolled off the top.
    let body = |f: &str| {
        f.lines()
            .filter(|l| l.contains("question") || l.contains("answer"))
            .count()
    };
    let top = body(&f);
    // The Session's box and keys take the rows under it.
    assert!(top >= 3, "{f}");
    // Again: it's already there.
    let again = plain(&mut m, 100, 30, &[key(Key::Home)]);
    assert_eq!(body(&again), top, "{again}");
    // A page down and back up again lands where it was.
    plain(&mut m, 100, 30, &[key(Key::PageDown)]);
    let back = plain(&mut m, 100, 30, &[key(Key::PageUp)]);
    assert!(back.contains("question 0"), "{back}");
    let f = plain(&mut m, 100, 30, &[key(Key::End)]);
    assert!(f.contains("answer 79"), "{f}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A transcript that ended mid-turn, of an agent that has stopped, doesn't
/// spin: its turn is ended (Go's followTail).
#[test]
fn a_stopped_session_ends_its_live_turn() {
    let mut m = model();
    let lines = vec![
        r#"{"type":"user","timestamp":"2026-10-28T12:00:00Z","cwd":"/src/checkout","message":{"role":"user","content":"run it"}}"#.to_string(),
        r#"{"type":"assistant","timestamp":"2026-10-28T12:00:02Z","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"sleep 100"}}]}}"#.to_string(),
    ];
    let dir = long_session(&mut m, "stopped", &lines);
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].pid = 0;
    m.agents[i].job.state = State::Stopped;
    plain(&mut m, 100, 30, &[(Key::Enter, KeyMods::NONE)]);
    let v = m.session.as_ref().unwrap();
    assert!(v.sess.live().is_none(), "the turn still runs");
    assert_eq!(v.sess.info.cost, m.agents[i].spend.cost);
    assert!(!m.spinning());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// rush's hosts as --render's setup gives them: the session's shell had
/// rush host one agent, h1, whose conversation is at transcript.
struct HostedOne {
    transcript: String,
}

impl rush_ui::screens::session_core::host::Hosts for HostedOne {
    // Its own conversation read from its transcript, as with no host up.
    fn hosts(&self, _: &Agent) -> bool {
        false
    }
    fn dial(
        &self,
        _: &Agent,
    ) -> Result<std::sync::Arc<dyn rush_ui::screens::session_core::host::Link>, String> {
        Err("no host here".into())
    }
    fn reply(
        &self,
        _: &Agent,
        _: &str,
    ) -> Result<(), rush_ui::screens::session_core::host::SendErr> {
        Ok(())
    }
    fn resume(&self, _: &Agent, _: &str, _: &[String]) -> Result<(), String> {
        Err("no".into())
    }
    fn adopt(
        &self,
        _: &Agent,
        _: &str,
    ) -> Result<rush_ui::screens::session_core::host::Started, String> {
        Err("no".into())
    }
    fn spawned(&self, parent: &str) -> Vec<rush_ui::screens::session_core::host::Spawned> {
        assert_eq!(parent, "d4");
        vec![rush_ui::screens::session_core::host::Spawned {
            id: "h1".into(),
            step: "t1".into(),
            kind: "claude".into(),
            name: "Claude Code".into(),
            prompt: "fix the rounding".into(),
            transcript: self.transcript.clone(),
            born: NOW - 10 * MIN,
            ..Default::default()
        }]
    }
}

/// A render of a session that had rush host an agent finds it as the
/// ticks would and counts its edits in Changes, as the live UI does
/// (render.go:39 refreshSpawns).
#[test]
fn a_rendered_session_counts_its_hosted_agents_changes() {
    let dir = std::env::temp_dir().join(format!("rush-ui-render-sp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"user","timestamp":"2026-10-28T12:00:00Z","cwd":"/src/checkout","message":{"role":"user","content":"get it fixed"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:02Z","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"rush agent start claude 'fix the rounding'"}}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
            r#"{"type":"user","timestamp":"2026-10-28T12:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"started h1","is_error":false}]},"toolUseResult":{"stdout":"started h1","stderr":""}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Started it."}]}}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let kid = dir.join("h1.jsonl");
    std::fs::write(
        &kid,
        [
            r#"{"type":"user","timestamp":"2026-10-28T12:00:03Z","cwd":"/src/checkout","message":{"role":"user","content":"fix the rounding"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:04Z","message":{"id":"k1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/src/checkout/tax/round.go","old_string":"a\n","new_string":"b\nc\n"}}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
            r#"{"type":"user","timestamp":"2026-10-28T12:00:05Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"e1","content":"ok","is_error":false}]}}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let mut m = model();
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].job.transcript_path = path.to_string_lossy().into_owned();
    // A stopped rush session: read from its files, as openSavedHost does.
    m.agents[i].rush = true;
    m.agents[i].pid = 0;
    rush_ui::screens::session_core::host::set_hosts(
        &mut m,
        std::sync::Arc::new(HostedOne {
            transcript: kid.to_string_lossy().into_owned(),
        }),
    );
    rush_ui::render_select(&mut m, 120, 30, "d4", Some("changes"));
    let f = plain(&mut m, 120, 30, &[]);
    assert!(f.contains("round.go"), "{f}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A wide session keeps the conversation at its reading width and runs
/// the column beside it from the top: the decisions not answered, the
/// tool at work, what changed. Narrower, there's no column (the 120x40
/// snapshot). A message of yours after a block, picking from it or not,
/// leaves it no longer open, in the column as in the conversation.
#[test]
fn a_wide_session_has_the_column_beside_it() {
    let dir = std::env::temp_dir().join(format!("rush-ui-render-side-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"user","timestamp":"2026-10-28T12:00:00Z","cwd":"/src/checkout","message":{"role":"user","content":"fix the rounding"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:02Z","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/src/checkout/tax/round.go","old_string":"a\n","new_string":"b\nc\n"}}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
            r#"{"type":"user","timestamp":"2026-10-28T12:00:05Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"e1","content":"ok","is_error":false}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:08Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Rounded.\n\nDecisions:\n1. **Rounding mode:** banks differ. (a) half up: as the old code; (b) half even: as the spec.\nMy picks: 1b."}]}}"#,
            r#"{"type":"system","subtype":"turn_duration","timestamp":"2026-10-28T12:00:09Z"}"#,
            r#"{"type":"user","timestamp":"2026-10-28T12:00:20Z","message":{"role":"user","content":"run the tests meanwhile"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:22Z","message":{"id":"m3","role":"assistant","content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test","description":"Run the tests"}}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let mut m = model();
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].job.transcript_path = path.to_string_lossy().into_owned();
    m.agents[i].job.state = State::Working;
    m.sel = m.agents[i].key.clone();
    let f = plain(&mut m, 260, 60, &[(Key::Enter, KeyMods::NONE)]);
    snapshot("snap_session_side_260x60", &f);
    let rows: Vec<&str> = f.lines().collect();
    let at = |what: &str| {
        rows.iter()
            .position(|r| r.contains(what))
            .unwrap_or_else(|| panic!("no {what:?} in\n{f}"))
    };
    let (dec, run, chg) = (
        at("Decisions  none open"),
        at(" Running  1 step"),
        at("▌ Changes"),
    );
    assert!(dec < run && run < chg, "{f}");
    assert!(!f.contains("not answered"), "{f}");
    assert!(rows[run + 1].contains("Run the tests  cargo"), "{f}");
    assert!(rows[chg + 1].contains("tax/round.go"), "{f}");
    // The column starts on the conversation's second row, not at its foot.
    let head = rows
        .iter()
        .position(|r| r.contains("Conversation"))
        .unwrap();
    assert!(dec <= head + 6, "{f}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A block nothing of yours has come after is listed in the column, each
/// decision a row with the agent's suggestion, as the conversation says
/// it's not answered.
#[test]
fn a_wide_session_lists_the_decisions_still_open() {
    let dir = std::env::temp_dir().join(format!("rush-ui-render-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"user","timestamp":"2026-10-28T12:00:00Z","cwd":"/src/checkout","message":{"role":"user","content":"fix the rounding"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:08Z","message":{"id":"m2","model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"Rounded.\n\nDecisions:\n1. **Rounding mode:** banks differ. (a) half up: as the old code; (b) half even: as the spec.\nMy picks: 1b."}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
            r#"{"type":"system","subtype":"turn_duration","timestamp":"2026-10-28T12:00:09Z"}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let mut m = model();
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].job.transcript_path = path.to_string_lossy().into_owned();
    m.sel = m.agents[i].key.clone();
    let f = plain(&mut m, 260, 60, &[(Key::Enter, KeyMods::NONE)]);
    snapshot("snap_session_side_open_260x60", &f);
    let rows: Vec<&str> = f.lines().collect();
    let dec = rows
        .iter()
        .position(|r| r.contains("Decisions  not answered"))
        .unwrap_or_else(|| panic!("no open decisions in\n{f}"));
    assert!(
        rows[dec + 1].contains("1. Rounding mode") && rows[dec + 1].contains("☆ half even"),
        "{f}"
    );
    assert!(
        f.contains("Decisions · not answered · d opens the card"),
        "{f}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- fixed-fixture snapshots for the crate split ----
//
// Every view below is drawn from the fixture fleet at three sizes, on a
// clock that stands still, in the throwaway home. They are the exact-match
// check each step of splitting rush-ui into crates must pass unchanged;
// UPDATE_SNAPSHOTS=1 writes them again.

const SIZES: [(u16, u16); 3] = [(80, 24), (120, 40), (200, 50)];

/// Draws one view at every size, each from a fresh model, as
/// `name_WxH`.
fn views(name: &str, setup: impl Fn(&mut Model), keys: &[(Key, KeyMods)]) {
    let mut bad = Vec::new();
    for (w, h) in SIZES {
        let mut m = model();
        setup(&mut m);
        let f = plain(&mut m, w, h, keys);
        let n = format!("{name}_{w}x{h}");
        if std::panic::catch_unwind(|| snapshot(&n, &f)).is_err() {
            bad.push(n);
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn snapshot_list() {
    views("snap_list", |_| {}, &[]);
}

/// The session the snapshots below open: a test run and its answer.
const SNAP_SESSION: [&str; 5] = [
    r#"{"type":"user","timestamp":"2026-10-28T12:00:00Z","cwd":"/src/checkout","message":{"role":"user","content":"fix the test"}}"#,
    r#"{"type":"assistant","timestamp":"2026-10-28T12:00:02Z","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
    r#"{"type":"user","timestamp":"2026-10-28T12:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]},"toolUseResult":{"stdout":"ok","stderr":""}}"#,
    r#"{"type":"assistant","timestamp":"2026-10-28T12:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Fixed it."}]}}"#,
    r#"{"type":"system","subtype":"turn_duration","timestamp":"2026-10-28T12:00:13Z"}"#,
];

#[test]
fn snapshot_session() {
    // The transcript's folder is never drawn; the name is fixed so the
    // frame can't depend on the process.
    let dir = std::env::temp_dir().join(format!("rush-ui-snap-session-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(&path, SNAP_SESSION.join("\n") + "\n").unwrap();
    let p = path.to_string_lossy().into_owned();
    views(
        "snap_session",
        |m| {
            let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
            m.agents[i].job.transcript_path = p.clone();
            m.sel = m.agents[i].key.clone();
        },
        &[(Key::Enter, KeyMods::NONE)],
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The Conversation tab and the views' dropdown under it, at 80x24: ⌥s
/// opens it over the conversation's first rows, Normal marked; ↓ and
/// enter show Chat, which the tab then names.
#[test]
fn snapshot_session_views_dropdown() {
    let dir = std::env::temp_dir().join(format!("rush-ui-snap-dropdown-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(&path, SNAP_SESSION.join("\n") + "\n").unwrap();
    let p = path.to_string_lossy().into_owned();
    let open = |m: &mut Model| {
        // The view is kept in the config, for every session: in memory
        // only, so no other test's Session shows it.
        rush_ui::accounts::use_store(m, rush_state::Store::default());
        rush_ui::accounts::accounts(m).read_only = true;
        let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
        m.agents[i].job.transcript_path = p.clone();
        m.sel = m.agents[i].key.clone();
    };
    let enter = (Key::Enter, KeyMods::NONE);
    let alt_s = (Key::Char('s'), KeyMods::ALT);
    let mut m = model();
    open(&mut m);
    let f = plain(&mut m, 80, 24, &[enter, alt_s]);
    let _ = std::panic::catch_unwind(|| snapshot("snap_session_views_menu_80x24", &f));
    assert!(f.contains("Conversation ▾"), "{f}");
    assert!(f.contains("● Normal"), "{f}");
    let mut m = model();
    open(&mut m);
    let f = plain(
        &mut m,
        80,
        24,
        &[enter, alt_s, (Key::Down, KeyMods::NONE), enter],
    );
    snapshot("snap_session_view_chat_80x24", &f);
    assert!(f.contains("Conversation · Chat ▾"), "{f}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn snapshot_projects() {
    // "." goes to the next place.
    views(
        "snap_projects",
        rush_ui::screens::projects_machine::freeze,
        &[(Key::Char('.'), KeyMods::NONE)],
    );
}

#[test]
fn snapshot_settings_general() {
    views(
        "snap_settings_general",
        |m| {
            rush_ui::screens::settings::set_settings_page(
                m,
                rush_ui::screens::settings::PAGE_GENERAL,
            )
        },
        &[],
    );
}

/// Every place's pages on a row of their own under the places, drawn
/// alike: the header alone, at 120×40, one per place with pages.
#[test]
fn snapshot_page_tabs() {
    use rush_ui::screens::settings::{PAGE_GENERAL, PAGE_PROVIDERS, set_settings_page};
    let header = |name: &str, setup: &dyn Fn(&mut Model), keys: &[(Key, KeyMods)]| {
        let mut m = model();
        setup(&mut m);
        let f = plain(&mut m, 120, 40, keys);
        let head: String = f.lines().take(6).map(|l| format!("{l}\n")).collect();
        snapshot(&format!("page_tabs_{name}_120x40"), &head);
    };
    header("agents", &|_| {}, &[]);
    header("wall", &|_| {}, &[(Key::Char(']'), KeyMods::NONE)]);
    header("harnesses", &|m| set_settings_page(m, PAGE_PROVIDERS), &[]);
    header("settings", &|m| set_settings_page(m, PAGE_GENERAL), &[]);
}

#[test]
fn snapshot_command_bar() {
    views("snap_ctrl_k", |_| {}, &[(Key::Char('k'), KeyMods::CTRL)]);
}

#[test]
fn snapshot_key_help() {
    views("snap_key_help", |_| {}, &[(Key::Char('?'), KeyMods::NONE)]);
}
