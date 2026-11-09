//! Emeritus: an honourable handover (docs/emeritus.md). A session near the
//! end of its context writes a baton, rush starts a fresh session on it,
//! and the old one retires for good, its transcript kept.
//!
//! This is the part that needs no running host: the words the agents are
//! given, the baton's facts, the successor's config, the handover's
//! journal and its recovery, the terminal check every way into a session
//! makes, and moving what was the retiring session's to its successor.
//! The host's side (the nudge, the retire op, the transaction itself) is
//! server/emeritus.rs. Rust-only: Go rush has no Emeritus to port.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rush_core::event::Exchange;
use rush_state::Time;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client::{Client, await_line, read_config, read_info, read_info_file};
use crate::events::log_event;
use crate::lines::Line;
use crate::op::Op;
use crate::paths::{dir, root};
use crate::subtool::{report, report_path};
use crate::wire::{Config, Emeritus, Info, queue_exchanges};

/// The rush tool a session calls to retire after its turn.
pub const RETIRE_TOOL: &str = "retire";

/// How long a successor has to answer its first request before the
/// handover is rolled back.
pub const START_WAIT: Duration = Duration::from_secs(5 * 60);

/// How much a session's context must grow past the one it was last
/// nudged at, as a share of the threshold, before it's nudged again: a
/// nudge it turned down (its task was nearly done) isn't asked again
/// turn after turn.
pub const RENUDGE_SHARE: i64 = 4;

/// The sections a baton has, in order: the agent writes them, rush adds
/// the state after (docs/emeritus.md, "The baton").
pub const SECTIONS: &[&str] = &[
    "Destination",
    "Core ideas",
    "Methods",
    "Journey",
    "Limits",
    "Tests that pin text",
    "Key files",
    "Next action",
];

/// What every nudge begins with, and the notes after: rush-convo's, so
/// the view draws them as rush's own.
pub use rush_convo::emeritus::{
    CALLED_OFF, NUDGE_START, ROLLED_BACK_START, TAKE_OVER_START, WAITS_START,
};

/// The baton's template, as the nudge and the retire tool give it.
pub fn template() -> String {
    "## Destination\n\
The task's whole goal, in a form someone can check: the whole goal, not just the last thing you were asked.\n\
## Core ideas\n\
The 3 to 5 things that matter most.\n\
## Methods\n\
How the work is done: the commands, the loop, how it's checked.\n\
## Journey\n\
What failed or surprised you, and why.\n\
## Limits\n\
✅ good enough / ❌ never acceptable. Durable rules only. Scope limits the user gave you for one turn \
(\"phase 1 only\", \"don't start X yet\") are not limits for your successor: leave them out.\n\
## Tests that pin text\n\
The tests that assert literal strings or snapshots of what you changed (prompts, rows, help text), \
by file and test name, or \"none\". Both a compacted agent and the original broke one of these in the lab.\n\
## Key files\n\
A short map: the files and functions that matter, a line each, with what's where.\n\
## Next action\n\
One line."
        .into()
}

/// The message that nudges a session past the threshold to hand over.
pub fn nudge(context: i64, threshold: i64) -> String {
    format!(
        "{NUDGE_START}{}k tokens, past the {}k where each call's re-read of it costs more than a fresh start. \
Hand the task over: write a baton and call rush's `{RETIRE_TOOL}` tool with it, then end your turn. Once your turn \
ends rush starts your successor in this folder, on your model, mode and account, and you retire for good; your \
transcript stays. If the successor can't start, you carry on and are told why.\n\n\
If the task will be done within a turn or two, say so in a line and carry on instead: you won't be asked again \
until your context has grown a good deal more.\n\n\
The baton is judgement only, in Markdown with these sections (rush adds the branch, HEAD, git status, diffstat, \
your transcript's path and your cost):\n\n{}",
        context / 1000,
        threshold / 1000,
        template()
    )
}

/// What the retiring session is told when its handover didn't happen.
pub fn rolled_back_note(why: &str) -> String {
    format!(
        "{ROLLED_BACK_START}{why}. You aren't retired: carry on with the task \
yourself. You may call `{RETIRE_TOOL}` again later."
    )
}

/// What a session that asked to retire is told when it can't yet.
pub fn waits_for_tasks(n: usize) -> String {
    format!(
        "{WAITS_START}{n} still running. Wait for them (or stop \
them), then call `{RETIRE_TOOL}` again. You aren't retired."
    )
}

/// Why a baton won't do: the sections it lacks. Empty when it will.
pub fn baton_lacks(baton: &str) -> Vec<&'static str> {
    let lower = baton.to_lowercase();
    SECTIONS
        .iter()
        .copied()
        .filter(|s| !lower.contains(&s.to_lowercase()))
        .collect()
}

/// Where a session's baton is kept.
pub fn baton_path(id: &str) -> PathBuf {
    dir(id).join("baton.md")
}

/// Where a session's handover is journalled.
pub fn journal_path(id: &str) -> PathBuf {
    dir(id).join("handover.json")
}

/// Runs git in cwd, its output trimmed; None when it fails.
fn git(cwd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// At most n lines of s, saying how many more there were.
fn lines_of(s: &str, n: usize) -> String {
    let all: Vec<&str> = s.lines().collect();
    if all.len() <= n {
        return s.to_string();
    }
    format!("{}\n… and {} more", all[..n].join("\n"), all.len() - n)
}

/// The state rush appends to a baton: the work's branch, HEAD, what git
/// says has changed, the transcript and the cost. What the agent might
/// misremember, rush reads.
pub fn facts(cwd: &str, transcript: &str, cost: f64, context: i64) -> String {
    let mut out = String::from("## State (recorded by rush)\n\n");
    out += &format!("- Folder: {cwd}\n");
    match git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Some(branch) => {
            out += &format!("- Branch: {branch}\n");
            if let Some(head) = git(cwd, &["log", "-1", "--format=%H %s"]) {
                out += &format!("- HEAD: {head}\n");
            }
        }
        None => out += "- Not a git repository\n",
    }
    if !transcript.is_empty() {
        out += &format!("- Transcript: {transcript}\n");
    }
    out += &format!("- Cost so far: ${cost:.2}, context {context} tokens\n");
    if let Some(st) = git(cwd, &["status", "--short"]) {
        let st = if st.is_empty() {
            "(clean)".into()
        } else {
            lines_of(&st, 60)
        };
        out += &format!("\n`git status --short`:\n```\n{st}\n```\n");
    }
    if let Some(d) = git(cwd, &["diff", "--stat", "HEAD"])
        && !d.is_empty()
    {
        out += &format!(
            "\n`git diff --stat HEAD`:\n```\n{}\n```\n",
            lines_of(&d, 60)
        );
    }
    out
}

/// The baton as kept: the agent's judgement, then rush's facts.
pub fn baton_file(baton: &str, facts: &str) -> String {
    format!("{}\n\n{}", baton.trim(), facts.trim_end())
}

/// A message for the retiring session that its successor answers: who
/// sent it, and what.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Forwarded {
    pub id: String,
    pub from: String,
    pub text: String,
}

/// The messages from other agents waiting in info's queue: the
/// successor's to answer. What the user typed is never among them.
pub fn from_agents(info: &Info) -> Vec<Forwarded> {
    queue_exchanges(info)
        .iter()
        .zip(&info.queue)
        .filter_map(|(e, text)| {
            let e = e.as_ref()?;
            let from = if e.sender.name.is_empty() {
                e.sender.session_id.clone()
            } else {
                format!("{} (session {})", e.sender.name, e.sender.session_id)
            };
            Some(Forwarded {
                id: e.id.clone(),
                from,
                text: text.clone(),
            })
        })
        .collect()
}

/// Whether info's queue holds something the user typed (not another
/// agent's message, nor rush's).
pub fn user_queued(info: &Info) -> bool {
    queue_exchanges(info).iter().any(Option::is_none)
}

/// The successor's first message: who it takes over from, the baton,
/// and what came for its predecessor that it now answers.
pub fn successor_prompt(a: &Config, baton: &str, transcript: &str, fwd: &[Forwarded]) -> String {
    let name = if a.name.is_empty() {
        String::new()
    } else {
        format!("\"{}\" ", a.name)
    };
    let mut out = format!(
        "{TAKE_OVER_START}{name}(rush session {}), an agent that handed it over near the end of its \
context and has retired. Its baton follows: its own judgement of what matters, then the state rush recorded as \
it handed over. Check that state before you rely on it, and work from the baton as if it were your own notes.",
        a.id
    );
    if !transcript.is_empty() {
        out += &format!(
            " Its transcript is at {transcript}: search it for a detail the baton leaves out, but never read it whole."
        );
    }
    out += "\n\n";
    out += baton.trim();
    if !fwd.is_empty() {
        out += "\n\n## Messages for you\n\nThese came for your predecessor as it handed over; they're yours to answer.\n";
        for f in fwd {
            out += &format!("\nFrom {}:\n{}\n", f.from, f.text.trim());
        }
    }
    out
}

/// A fresh session that carries on from a: the same folder, agent,
/// model, effort, permission mode, account, trust and parent, tagged
/// with its predecessor and how long the chain is, starting on prompt.
pub fn successor_config(a: &Config, info: &Info, prompt: String) -> Config {
    let or = |x: &str, y: &str| {
        if x.is_empty() {
            y.to_string()
        } else {
            x.to_string()
        }
    };
    let mut b = a.clone();
    b.cause.clear();
    b.id.clear();
    b.session_id.clear();
    b.resume = false;
    b.fork = false;
    b.from.clear();
    b.branches.clear();
    b.carry = None;
    b.prompt_exchange = None;
    b.images.clear();
    b.name_first = false;
    b.own_worktree = false;
    b.emeritus = None;
    b.prompt = prompt;
    b.cwd = or(&info.cwd, &a.cwd);
    b.model = or(&info.model, &a.model);
    b.effort = or(&info.effort, &a.effort);
    b.permission_mode = or(&info.permission_mode, &a.permission_mode);
    if info.workspace.is_some() {
        b.workspace = info.workspace.clone();
    }
    b.meta.insert("predecessor".into(), a.id.clone());
    b.meta
        .insert("lineage".into(), (lineage(a) + 1).to_string());
    b
}

/// How many handovers led to the session: 0 for one started afresh.
pub fn lineage(c: &Config) -> i64 {
    c.meta
        .get("lineage")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0)
}

/// Why a session of config cfg may not be started, woken or sent
/// anything: it's retired. None when it may.
pub fn refusal(cfg: &Config) -> Option<String> {
    let e = cfg.emeritus.as_ref().filter(|e| e.retired())?;
    Some(if !e.successor.is_empty() {
        format!(
            "session {} is retired: it handed over to session {}, which carries on its work. Write to that one; nothing sent here is passed on.",
            cfg.id, e.successor
        )
    } else {
        let why = if e.why.is_empty() {
            "its start as a successor was rolled back"
        } else {
            &e.why
        };
        format!("session {} is retired: {why}", cfg.id)
    })
}

/// refusal, for session id as its config on disk says.
pub fn refused(id: &str) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    read_config(id).ok().and_then(|c| refusal(&c))
}

/// The session that took over from id, when it retired by handing over.
pub fn successor(id: &str) -> Option<String> {
    read_config(id)
        .ok()?
        .emeritus
        .filter(|e| e.retired() && !e.successor.is_empty())
        .map(|e| e.successor)
}

/// The session at the end of id's chain of successors: id itself when
/// it never handed over.
pub fn live(id: &str) -> String {
    let mut cur = id.to_string();
    for _ in 0..64 {
        match successor(&cur) {
            Some(next) if next != cur => cur = next,
            _ => break,
        }
    }
    cur
}

/// How a successor's start stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    /// Its first request came back: its context then, 0 when its agent
    /// doesn't say.
    Started(i64),
    /// It can't take over, and why.
    Failed(String),
    /// Not yet.
    Waiting,
}

/// How a successor's start stands by its info, its host alive or not. It
/// has started once its first request came back (its context is
/// measured), or its first turn ended clean; a limit, an error being
/// retried, an error, or its host gone are a failed start.
pub fn start_of(info: &Info, alive: bool) -> Start {
    if info.limit.is_some() {
        return Start::Failed("hit a usage limit".into());
    }
    if let Some(r) = &info.retry {
        return Start::Failed(format!("hit a provider error: {}", r.reason));
    }
    if !info.error.is_empty() {
        return Start::Failed(format!("couldn't start: {}", info.error));
    }
    if info.context_tokens > 0 {
        return Start::Started(info.context_tokens);
    }
    if info.state == "stopped" || !alive {
        return Start::Failed("stopped before it answered".into());
    }
    if info.turn > 0 && info.state == "idle" {
        return Start::Started(0);
    }
    Start::Waiting
}

/// Waits up to wait for session id's start to settle.
pub fn wait_started(id: &str, wait: Duration) -> Start {
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[dir(id)]);
    let deadline = Instant::now() + wait;
    loop {
        let info = match read_info(id) {
            Ok(i) => i,
            Err(e) => return Start::Failed(format!("left no info: {e}")),
        };
        let alive = info.host_pid > 0 && crate::alive(info.host_pid);
        let s = start_of(&info, alive);
        if s != Start::Waiting {
            return s;
        }
        let now = Instant::now();
        if now >= deadline {
            return Start::Failed(format!(
                "didn't answer within {} minutes",
                wait.as_secs() / 60
            ));
        }
        w.wait(None, Some((deadline - now).min(Duration::from_secs(5))));
    }
}

/// Where a handover has got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// The retiring session takes no input; no successor yet.
    #[default]
    Frozen,
    /// The successor is being started.
    Starting,
    /// Done: the predecessor is retired, the successor owns the task.
    Committed,
    /// Undone: the predecessor carries on, the successor is retired.
    RolledBack,
}

/// A handover, journalled beside the retiring session so that one cut
/// short (its host killed) is settled at its next start: never two
/// owners of the task, never none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Journal {
    pub phase: Phase,
    pub from: String,
    /// The successor, once its id is chosen; "" before.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    pub at: Time,
    #[serde(rename = "fromContext")]
    pub from_context: i64,
    #[serde(rename = "fromCostUsd")]
    pub from_cost_usd: f64,
    /// The threshold that nudged it; 0 when it retired unasked.
    #[serde(skip_serializing_if = "is_zero")]
    pub threshold: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Session id's handover journal, when it has one.
pub fn read_journal(id: &str) -> Option<Journal> {
    let b = std::fs::read(journal_path(id)).ok()?;
    serde_json::from_slice(&b).ok()
}

/// Writes session id's handover journal whole.
pub fn write_journal(id: &str, j: &Journal) -> std::io::Result<()> {
    let b = serde_json::to_vec_pretty(j).map_err(std::io::Error::other)?;
    crate::json::write_atomic(&journal_path(id), &b)
}

/// Writes cfg as its config.json, whole.
pub fn write_config(cfg: &Config) -> std::io::Result<()> {
    crate::json::write_atomic(&dir(&cfg.id).join("config.json"), &cfg.to_json_indent())
}

/// Marks session id retired in its config on disk, handed over to
/// successor or, with none, retired for why. Its host, if it runs, is
/// told first and stops (emeritus_void); one that doesn't answer is left
/// to the config, which every start reads.
pub fn void(id: &str, why: &str) {
    if let Ok(c) = Client::dial(id) {
        let o = Op {
            op: "emeritus_void".into(),
            text: why.into(),
            ..Op::default()
        };
        let pid = read_info(id).map(|i| i.host_pid).unwrap_or(0);
        if c.send_op(o).is_ok() && pid > 0 {
            crate::sys::wait_exit(pid, Duration::from_secs(10));
        }
    }
    if let Ok(mut cfg) = read_config(id)
        && !cfg.retired()
    {
        let e = cfg.emeritus.get_or_insert_with(Emeritus::default);
        e.retired_at = Time::now();
        e.why = why.into();
        let _ = write_config(&cfg);
    }
    log_event(id, "emeritus-voided", json!({"why": why}));
}

/// Moves to successor to what was from's: the sessions it started, as
/// its subagents (spawnedBy) and beside it (startedFrom), the answers
/// owed to it (their report files) and its reminders. The ids of the
/// sessions moved. Done again it moves nothing more.
pub fn move_children(from: &str, to: &str) -> Vec<String> {
    let mut moved = Vec::new();
    let ids: Vec<String> = std::fs::read_dir(root())
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    for id in ids {
        if id == from || id == to {
            continue;
        }
        if let Ok(cfg) = read_config(&id) {
            for key in ["spawnedBy", "startedFrom"] {
                if cfg.meta.get(key).is_some_and(|p| p == from) {
                    adopt(&id, key, from, to);
                    moved.push(id.clone());
                }
            }
        }
        if std::fs::read_to_string(report_path(&id)).is_ok_and(|p| p == from) {
            let _ = report(&id, to);
        }
    }
    let now = Time::now();
    for r in rush_remind::list(from) {
        if rush_remind::add(to, &r.text, r.due, now).is_ok() {
            let _ = rush_remind::cancel(from, &r.id);
        }
    }
    moved.dedup();
    moved
}

/// Makes session id's key (spawnedBy or startedFrom) name to rather
/// than from: told to its host when one runs (it keeps its config in
/// memory), else written to its files.
fn adopt(id: &str, key: &str, from: &str, to: &str) {
    if let Ok(c) = Client::dial(id)
        && await_line(&c, Duration::from_secs(5), |l| matches!(l, Line::Info(_))).is_ok()
    {
        let o = Op {
            op: "emeritus_adopt".into(),
            id: to.into(),
            text: key.into(),
            was: from.into(),
            ..Op::default()
        };
        let took = c.send_op(o).is_ok()
            && await_line(
                &c,
                Duration::from_secs(5),
                |l| matches!(l, Line::Info(i) if i.meta.get(key).is_some_and(|p| p == to)),
            )
            .is_ok();
        if took {
            return;
        }
    }
    if let Ok(mut cfg) = read_config(id)
        && cfg.meta.get(key).is_some_and(|p| p == from)
    {
        cfg.meta.insert(key.into(), to.into());
        let _ = write_config(&cfg);
    }
    if let Ok(mut info) = read_info_file(id)
        && info.meta.get(key).is_some_and(|p| p == from)
    {
        info.meta.insert(key.into(), to.into());
        let _ = crate::json::write_atomic(&dir(id).join("info.json"), &info.to_json());
    }
}

/// What finishing a handover does once the retiring session's config
/// says it's retired: what was its moves to its successor, and the
/// handover is logged for the Efficiency place.
pub fn finish(a: &Config, j: &Journal, to_context: i64) {
    let moved = move_children(&j.from, &j.to);
    // The answer it owed its parent is its successor's to give.
    if let Ok(p) = std::fs::read_to_string(report_path(&j.from))
        && !p.is_empty()
    {
        let _ = report(&j.to, &p);
        let _ = report(&j.from, "");
    }
    let to_lineage = read_config(&j.to).map(|c| lineage(&c)).unwrap_or(0);
    let h = rush_efficiency::events::Handover {
        from: j.from.clone(),
        from_session: a.session_id.clone(),
        to: j.to.clone(),
        agent: a.kind.clone(),
        model: a.model.clone(),
        from_context: j.from_context,
        from_cost_usd: j.from_cost_usd,
        to_context,
        threshold: j.threshold,
        lineage: to_lineage,
        subagent: a.meta.get("spawnedBy").is_some_and(|p| !p.is_empty()),
    };
    if let Err(e) = rush_efficiency::events::add_handover(&a.account.dir, h) {
        log_event(&j.from, "emeritus-unlogged", json!({"err": e.to_string()}));
    }
    log_event(
        &j.from,
        "emeritus-committed",
        json!({"to": j.to, "fromContext": j.from_context, "toContext": to_context, "moved": moved}),
    );
}

/// Settles a handover its host was cut short in, before the session
/// starts again: committed if its successor had started (or its config
/// already says it's retired), rolled back otherwise, its successor
/// retired. What it did, when there was one to settle.
pub fn recover(id: &str) -> Option<Phase> {
    let mut j = read_journal(id)?;
    if matches!(j.phase, Phase::Committed | Phase::RolledBack) {
        return None;
    }
    let mut cfg = read_config(id).ok()?;
    if cfg.retired() {
        j.phase = Phase::Committed;
        let _ = write_journal(id, &j);
        let ctx = read_info(&j.to).map(|i| i.context_tokens).unwrap_or(0);
        finish(&cfg, &j, ctx);
        return Some(j.phase);
    }
    let started = if j.to.is_empty() {
        Start::Failed("wasn't started before its predecessor's host went".into())
    } else {
        match read_info(&j.to) {
            Ok(i) => start_of(&i, i.host_pid > 0 && crate::alive(i.host_pid)),
            Err(_) => Start::Failed("wasn't started before its predecessor's host went".into()),
        }
    };
    match started {
        Start::Started(ctx) => {
            let e = cfg.emeritus.get_or_insert_with(Emeritus::default);
            e.retired_at = Time::now();
            e.successor = j.to.clone();
            if write_config(&cfg).is_err() {
                return roll_back_offline(j, "its predecessor's config couldn't be written");
            }
            j.phase = Phase::Committed;
            let _ = write_journal(id, &j);
            finish(&cfg, &j, ctx);
            Some(j.phase)
        }
        Start::Failed(why) => roll_back_offline(j, &why),
        // Not settled while its predecessor waited to start: it's not
        // waited for now.
        Start::Waiting => roll_back_offline(j, "hadn't answered when its predecessor's host went"),
    }
}

fn roll_back_offline(mut j: Journal, why: &str) -> Option<Phase> {
    if !j.to.is_empty() && dir(&j.to).join("config.json").exists() {
        void(
            &j.to,
            &format!(
                "its start as {}'s successor was rolled back: it {why}",
                j.from
            ),
        );
    }
    j.phase = Phase::RolledBack;
    j.why = why.into();
    let _ = write_journal(&j.from, &j);
    log_event(
        &j.from,
        "emeritus-rolled-back",
        json!({"to": j.to, "why": why}),
    );
    Some(j.phase)
}

/// Writes a retired session's info as stopped, naming its successor:
/// what the list shows of a session a start was refused for.
pub fn mark_stopped(id: &str, cfg: &Config) {
    let Ok(mut info) = read_info_file(id) else {
        return;
    };
    info.state = "stopped".into();
    info.emeritus = "retired".into();
    info.successor = cfg
        .emeritus
        .as_ref()
        .map(|e| e.successor.clone())
        .unwrap_or_default();
    let _ = crate::json::write_atomic(&dir(id).join("info.json"), &info.to_json());
}

/// Asks session id's host to retire it after its turn, handing over on
/// baton: the retire tool's call. What the agent is told.
pub fn ask_retire(id: &str, baton: &str) -> Result<String, String> {
    let lacks = baton_lacks(baton);
    if !lacks.is_empty() {
        return Err(format!(
            "The baton lacks {}. Write it with these sections:\n\n{}",
            lacks.join(", "),
            template()
        ));
    }
    let c = Client::dial(id).map_err(|e| e.to_string())?;
    await_line(&c, Duration::from_secs(10), |l| matches!(l, Line::Info(_)))
        .map_err(|e| e.to_string())?;
    c.send_op(Op {
        op: "retire".into(),
        text: baton.into(),
        ..Op::default()
    })
    .map_err(|e| e.to_string())?;
    let mut failed = None;
    await_line(&c, Duration::from_secs(10), |l| match l {
        Line::Error(s) => {
            failed = Some(s.clone());
            true
        }
        Line::Info(i) => i.emeritus == "retiring",
        _ => false,
    })
    .map_err(|e| e.to_string())?;
    if let Some(f) = failed {
        return Err(f);
    }
    Ok(format!(
        "Retiring after this turn. End your turn now: rush then starts your successor on your baton and you retire \
for good. If it can't start, you carry on and are told why. The baton is kept at {}.",
        baton_path(id).display()
    ))
}

/// An exchange carried to the successor instead: to it, by name.
pub fn readdressed(mut e: Exchange, to: &str) -> Exchange {
    e.receiver = crate::subtool::peer(to);
    e
}

#[cfg(test)]
mod tests;
