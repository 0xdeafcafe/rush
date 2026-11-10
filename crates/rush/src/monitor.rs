//! `rush session read`, `wait`, `needs` and `search`, and the needsYou and
//! lastReplyAt that `list --json` and `info --json` add: what an agent
//! watching other agents' sessions needs, without reading their
//! transcripts by hand. Rust-only: Go's rush had none of these.
//!
//! A session is found as the view's list has it (rush-fleet's loader), so
//! these take any session the list shows: a rush session by its short id,
//! or any agent's by its session id, `list --others`' included. A
//! conversation is read as the view reads it (rush_ui::search's
//! conversation_of, rush-convo's Tail and Session); a search is the
//! command bar's (rush-convo's Library). What needs you is the list's own
//! reading (rush_ui::turnwants::needs_you_of, with the cache of what
//! finished turns want). wait follows each rush session's events.jsonl,
//! the host's record of its turns' ends and states, through kqueue
//! (rush_sys::fswait): nothing polls.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rush_convo::search::{Reader, library};
use rush_convo::{Item, Session as Convo, Tail};
use rush_core::adapter::Registry;
use rush_core::agent::Agent;
use rush_core::time::{Ms, format_rfc3339, format_rfc3339_local, now, parse_rfc3339};
use rush_host::Info;
use rush_ui::screens::session_core::turnwants as reading;
use rush_ui::turnwants::{NeedsYou, needs_you_of};
use serde::Serialize;
use serde_json::{Value, json};

use crate::flags::{Flags, has_flag};
use crate::session::{Env, write_json};

/// How much of a transcript's end read reads: its last turns, whole.
const READ_TAIL: u64 = 8 << 20;
/// How much of a session's events.jsonl is read back for its last turn's
/// end.
const EVENTS_TAIL: u64 = 256 << 10;

/// The agents the view's list has now: the loader's quick reading.
pub fn load_agents(reg: &Registry) -> Vec<Agent> {
    rush_fleet::Loader::from_state(reg.clone())
        .load_quick(now())
        .agents
}

/// The id a session goes by here: a rush session's short id, else its
/// agent's session id.
pub fn id_of(a: &Agent) -> String {
    if a.rush && !a.job.id.is_empty() {
        a.job.id.clone()
    } else {
        a.job.session_id.clone()
    }
}

/// The agent id names: a rush session's short id, a session id, a row's
/// key, or the start of a session id (8 characters at least) that only
/// one session has.
pub fn find<'a>(agents: &'a [Agent], id: &str) -> Result<&'a Agent, String> {
    let exact = agents
        .iter()
        .find(|a| (a.rush && a.job.id == id) || a.job.session_id == id || a.key == id);
    if let Some(a) = exact {
        return Ok(a);
    }
    if id.len() >= 8 {
        let mut it = agents.iter().filter(|a| a.job.session_id.starts_with(id));
        if let (Some(a), None) = (it.next(), it.next()) {
            return Ok(a);
        }
    }
    Err(format!("session {id} not found"))
}

/// The ids among args, and the flags around them, wherever they are.
fn ids_and_flags(fs: &mut Flags, args: &[String]) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let mut rest = args.to_vec();
    loop {
        fs.rest.clear();
        fs.parse(&rest)?;
        let Some(first) = fs.rest.first().cloned() else {
            break;
        };
        ids.push(first);
        rest = fs.rest[1..].to_vec();
    }
    Ok(ids)
}

/// A time as the commands print it: this machine's, to the second.
fn clock(ms: Ms) -> String {
    if ms == 0 {
        return "-".into();
    }
    let s = format_rfc3339_local(ms);
    let s = s.replacen('T', " ", 1);
    s.chars().take(19).collect()
}

/// A time as JSON has it, null when not known.
fn at_json(ms: Ms) -> Value {
    if ms == 0 {
        Value::Null
    } else {
        json!(format_rfc3339(ms))
    }
}

// ---- read ----

/// One thing said or done in a conversation, as read prints it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Entry {
    pub turn: u32,
    /// you, agent or tool.
    pub who: String,
    #[serde(serialize_with = "ser_at")]
    pub at: Ms,
    /// What was said, whole; a tool call on one line.
    pub text: String,
    /// The tool, for a call.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

fn ser_at<S: serde::Serializer>(at: &Ms, s: S) -> Result<S::Ok, S::Error> {
    at_json(*at).serialize(s)
}

/// A's conversation, read as the view reads it; an error when there's
/// nothing to read.
pub fn conversation(reg: &Registry, a: &Agent) -> Result<Convo, String> {
    let src = rush_ui::search::conversation_of(reg, a)
        .ok_or_else(|| format!("session {} has no conversation rush can read", id_of(a)))?;
    match &src.read {
        Reader::Transcript(decode) => {
            let mut t = Tail::from_end(&src.path, READ_TAIL, decode.clone());
            let mut s = Convo::new();
            t.read(&mut s)
                .map_err(|e| format!("read {}: {e}", src.path))?;
            Ok(s)
        }
        Reader::Other(f) => {
            f().ok_or_else(|| format!("session {}: its agent couldn't read it back", id_of(a)))
        }
    }
}

/// A conversation's turns as entries: your messages and the agent's words
/// whole, each tool call a line; doing says a call in words.
pub fn entries(s: &Convo, doing: &dyn Fn(&rush_core::tool::Call) -> String) -> Vec<Entry> {
    let mut out = Vec::new();
    for t in &s.turns {
        if !t.prompt.is_empty() {
            out.push(Entry {
                turn: t.n,
                who: if t.from.is_empty() {
                    "you".into()
                } else {
                    t.from.clone()
                },
                at: t.start,
                text: t.prompt.clone(),
                ..Entry::default()
            });
        }
        // Words carry no time of their own: the last call's, else the
        // turn's start; the final words the turn's end.
        let mut last = t.start;
        for it in &t.items {
            match it {
                Item::Text { text, answer } => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    let at = if *answer && t.end != 0 { t.end } else { last };
                    out.push(Entry {
                        turn: t.n,
                        who: "agent".into(),
                        at,
                        text: text.clone(),
                        ..Entry::default()
                    });
                }
                Item::Step(id) => {
                    let st = s.step(*id);
                    let at = if st.start != 0 { st.start } else { last };
                    last = at.max(st.end);
                    let mut line = doing(st.call());
                    if line.is_empty() {
                        line = st.tool.to_string();
                    }
                    out.push(Entry {
                        turn: t.n,
                        who: "tool".into(),
                        at,
                        text: line.lines().next().unwrap_or("").to_string(),
                        tool: st.tool.to_string(),
                        failed: st.status == rush_convo::Status::Failed,
                    });
                }
                _ => {}
            }
        }
    }
    out
}

/// The last n entries, or with turns the entries of the last n turns.
pub fn last_of(mut es: Vec<Entry>, n: usize, turns: bool) -> Vec<Entry> {
    if turns {
        let mut seen: Vec<u32> = es.iter().map(|e| e.turn).collect();
        seen.dedup();
        let from = seen.len().saturating_sub(n);
        let Some(&first) = seen.get(from) else {
            return Vec::new();
        };
        es.retain(|e| e.turn >= first);
        return es;
    }
    let from = es.len().saturating_sub(n);
    es.split_off(from)
}

/// Entries as read prints them.
pub fn print_entries(w: &mut dyn Write, es: &[Entry]) {
    let mut turn = 0;
    for e in es {
        if e.who == "tool" {
            let failed = if e.failed { " · failed" } else { "" };
            let _ = writeln!(w, "  → {} {}{failed}", clock(e.at), e.text);
            continue;
        }
        if e.turn != turn {
            turn = e.turn;
            let _ = writeln!(w, "── turn {turn}");
        }
        let _ = writeln!(w, "{} · {}", e.who, clock(e.at));
        let _ = writeln!(w, "{}", e.text.trim_end());
    }
}

/// rush session read.
pub fn session_read(
    env: &Env,
    args: &[String],
    stdout: &mut dyn Write,
) -> (bool, Result<(), String>) {
    let mut fs = Flags::session("read", &["last", "turns"], &["json"], &[]);
    let id = match fs.id_and_flags(args) {
        Ok(id) => id,
        Err(e) => return (has_flag(args, "--json"), Err(e)),
    };
    let as_json = fs.b("json");
    (as_json, read(env, &id, &fs, as_json, stdout))
}

fn count(fs: &Flags, k: &str) -> Result<Option<usize>, String> {
    let s = fs.s(k);
    if s.is_empty() {
        return Ok(None);
    }
    match s.parse::<usize>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(format!("--{k} wants a number above 0, not {s:?}")),
    }
}

fn read(env: &Env, id: &str, fs: &Flags, as_json: bool, w: &mut dyn Write) -> Result<(), String> {
    let (last, turns) = (count(fs, "last")?, count(fs, "turns")?);
    if last.is_some() && turns.is_some() {
        return Err("--last and --turns don't go together".into());
    }
    let agents = (env.load)(&env.reg);
    let a = find(&agents, id)?;
    let s = conversation(&env.reg, a)?;
    let kind = a.kind.as_str().to_string();
    let es = entries(&s, &|c| env.reg.doing(&kind, c));
    let es = match (last, turns) {
        (Some(n), _) => last_of(es, n, false),
        (_, Some(n)) => last_of(es, n, true),
        _ => last_of(es, 1, true),
    };
    if as_json {
        write_json(
            w,
            &json!({ "session": id_of(a), "name": a.display_name, "agent": kind, "entries": es }),
        );
        return Ok(());
    }
    let _ = writeln!(w, "{}  {}", id_of(a), a.display_name);
    print_entries(w, &es);
    Ok(())
}

// ---- needs ----

/// What a needs of you now, as the view's list reads it: its live state,
/// and the reading of its finished turn, the cache's or, with ask, a
/// model's read now as the leading view would.
pub fn needs_of(reg: &Registry, a: &Agent, ask: bool) -> Option<NeedsYou> {
    let now = now();
    let t = if ask {
        reading::read_now(reg, a, now)
    } else {
        reading::cached(a).unwrap_or_default()
    };
    needs_you_of(now, a, &t)
}

/// rush session needs.
pub fn session_needs(
    env: &Env,
    args: &[String],
    stdout: &mut dyn Write,
) -> (bool, Result<(), String>) {
    let mut fs = Flags::session("needs", &[], &["json"], &[]);
    let ids = match ids_and_flags(&mut fs, args) {
        Ok(ids) => ids,
        Err(e) => return (has_flag(args, "--json"), Err(e)),
    };
    let as_json = fs.b("json");
    if ids.is_empty() {
        return (as_json, Err("usage: rush session needs <id>...".into()));
    }
    let agents = (env.load)(&env.reg);
    let mut found = Vec::new();
    for id in &ids {
        match find(&agents, id) {
            Ok(a) => found.push(a),
            Err(e) => return (as_json, Err(e)),
        }
    }
    let mut out = Vec::new();
    for a in found {
        let n = needs_of(&env.reg, a, true);
        if as_json {
            out.push(json!({ "session": id_of(a), "name": a.display_name, "needsYou": n }));
            continue;
        }
        match n {
            None => {
                let _ = writeln!(stdout, "{}  none  {}", id_of(a), a.display_name);
            }
            Some(n) => {
                let _ = writeln!(
                    stdout,
                    "{}  {}: {}  {}",
                    id_of(a),
                    n.kind,
                    n.reason,
                    a.display_name
                );
                for l in n.text.lines().filter(|l| !l.trim().is_empty()) {
                    let _ = writeln!(stdout, "  │ {l}");
                }
            }
        }
    }
    if as_json {
        write_json(stdout, &out);
    }
    (as_json, Ok(()))
}

// ---- search ----

/// rush session search.
pub fn session_search(
    env: &Env,
    args: &[String],
    stdout: &mut dyn Write,
) -> (bool, Result<(), String>) {
    let mut fs = Flags::session("search", &["since", "limit"], &["json"], &[]);
    let words = match ids_and_flags(&mut fs, args) {
        Ok(w) => w,
        Err(e) => return (has_flag(args, "--json"), Err(e)),
    };
    let as_json = fs.b("json");
    (as_json, search(env, &words, &fs, as_json, stdout))
}

fn search(
    env: &Env,
    words: &[String],
    fs: &Flags,
    as_json: bool,
    w: &mut dyn Write,
) -> Result<(), String> {
    let Some((q, ids)) = words.split_first() else {
        return Err("usage: rush session search <text> [<id>...]".into());
    };
    if q.trim().is_empty() {
        return Err("nothing to search for".into());
    }
    let since = rush_events::parse_since(&fs.s("since"), now())?;
    let limit = count(fs, "limit")?.unwrap_or(3);
    let agents = (env.load)(&env.reg);
    let mut pick: Vec<&Agent> = Vec::new();
    for id in ids {
        pick.push(find(&agents, id)?);
    }
    if ids.is_empty() {
        pick = agents.iter().collect();
    }
    pick.retain(|a| since == 0 || a.job.updated_at >= since);
    pick.sort_by_key(|a| std::cmp::Reverse(a.job.updated_at));
    let mut out = Vec::new();
    for a in pick {
        let Some(src) = rush_ui::search::conversation_of(&env.reg, a) else {
            continue;
        };
        let Ok(mut f) = library().search(&src.path, q, limit, &src.read) else {
            continue;
        };
        if since != 0 {
            f.hits.retain(|h| h.at == 0 || h.at >= since);
        }
        if f.hits.is_empty() {
            continue;
        }
        out.push((a, f));
    }
    if as_json {
        let v: Vec<Value> = out
            .iter()
            .map(|(a, f)| {
                json!({
                    "session": id_of(a), "name": a.display_name, "agent": a.kind.as_str(),
                    "total": f.total,
                    "hits": f.hits.iter().map(|h| json!({
                        "turn": h.turn, "at": at_json(h.at), "who": h.who,
                        "snippet": h.snippet, "ref": h.r#ref,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        write_json(w, &v);
        return Ok(());
    }
    for (a, f) in &out {
        let more = if f.total > f.hits.len() {
            format!(" · {} matches", f.total)
        } else {
            String::new()
        };
        let _ = writeln!(w, "{}  {}{more}", id_of(a), a.display_name);
        for h in &f.hits {
            let _ = writeln!(
                w,
                "  turn {} · {} · {}: {}",
                h.turn,
                clock(h.at),
                h.who,
                h.snippet
            );
        }
    }
    Ok(())
}

// ---- list and info's fields ----

/// A session as list --json and info --json print it: as before, with
/// what needs you of it (the cache's reading only: neither asks a model)
/// and when its last turn ended.
#[derive(Serialize)]
pub struct Listed<'a> {
    #[serde(flatten)]
    pub view: &'a rush_remote::Session,
    #[serde(rename = "needsYou")]
    pub needs_you: Option<NeedsYou>,
    #[serde(rename = "lastReplyAt")]
    pub last_reply_at: Value,
}

/// v as list and info print it, with agents the list's reading.
pub fn listed<'a>(reg: &Registry, agents: &[Agent], v: &'a rush_remote::Session) -> Listed<'a> {
    let needs_you = agents
        .iter()
        .find(|a| a.rush && a.job.id == v.info.id)
        .and_then(|a| needs_of(reg, a, false));
    Listed {
        view: v,
        needs_you,
        last_reply_at: at_json(last_reply_at(&v.info)),
    }
}

/// When info's session last ended a turn: its idle_since while it rests,
/// else the last turn-end its events.jsonl has; 0 before its first.
pub fn last_reply_at(info: &Info) -> Ms {
    if !info.idle_since.is_zero() {
        return info.idle_since.to_ms();
    }
    last_turn_end(&rush_host::paths::events_path(&info.id)).map_or(0, |e| e.at)
}

/// A turn's end, as a session's events.jsonl says it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnEnd {
    /// The turn's number; 0 when the line is older than numbers.
    pub turn: i64,
    pub at: Ms,
}

/// What one line of events.jsonl says: a turn's end, a state, a host gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    TurnEnd(TurnEnd),
    State(String),
    HostStopped,
}

/// A line of a session's events.jsonl, when it's one wait reads.
pub fn host_event(line: &[u8]) -> Option<HostEvent> {
    // Most lines are other kinds: only these are parsed.
    let s = std::str::from_utf8(line).ok()?;
    if !s.contains("\"turn-end\"") && !s.contains("\"state\"") && !s.contains("\"host-stopped\"") {
        return None;
    }
    let v: Value = serde_json::from_str(s).ok()?;
    match v.get("kind")?.as_str()? {
        "turn-end" => Some(HostEvent::TurnEnd(TurnEnd {
            turn: v.get("turn").and_then(Value::as_i64).unwrap_or(0),
            at: parse_rfc3339(v.get("t").and_then(Value::as_str).unwrap_or("")),
        })),
        "state" => Some(HostEvent::State(
            v.pointer("/info/state")?.as_str()?.to_string(),
        )),
        "host-stopped" => Some(HostEvent::HostStopped),
        _ => None,
    }
}

/// The last turn-end in the end of the events file at path.
pub fn last_turn_end(path: &std::path::Path) -> Option<TurnEnd> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(EVENTS_TAIL);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).ok()?;
    b.split(|c| *c == b'\n')
        .rev()
        .find_map(|l| match host_event(l) {
            Some(HostEvent::TurnEnd(e)) => Some(e),
            _ => None,
        })
}

// ---- wait ----

/// What a wait counts as after what the caller has seen: a turn's number
/// or a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Since {
    Turn(i64),
    At(Ms),
}

impl Since {
    /// --since read: a turn's number, a time or a while back; "" now.
    pub fn parse(s: &str, now: Ms) -> Result<Since, String> {
        if s.is_empty() {
            return Ok(Since::At(now));
        }
        if let Ok(n) = s.parse::<i64>() {
            return Ok(Since::Turn(n));
        }
        rush_events::parse_since(s, now).map(Since::At)
    }

    /// Whether e came after it.
    pub fn after(self, e: &TurnEnd) -> bool {
        match self {
            Since::Turn(n) => e.turn > n,
            Since::At(t) => e.at > t,
        }
    }
}

/// What wait reports of a session.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Woke {
    pub session: String,
    pub name: String,
    /// turn or needs-you.
    pub event: String,
    pub turn: i64,
    #[serde(serialize_with = "ser_at")]
    pub at: Ms,
    #[serde(rename = "needsYou")]
    pub needs_you: Option<NeedsYou>,
    #[serde(rename = "lastReply")]
    pub last_reply: String,
}

/// One session wait follows: its events file and how far it's read.
struct Followed {
    id: String,
    path: PathBuf,
    off: u64,
    gone: bool,
    last: TurnEnd,
}

/// rush session wait: its exit code, as rush watch's: 0 something
/// happened, 1 a usage error, 2 timed out, 3 every session gone.
pub fn session_wait(
    env: &Env,
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut fs = Flags::session("wait", &["until", "since", "timeout"], &["json"], &[]);
    let as_json = has_flag(args, "--json");
    let ids = match ids_and_flags(&mut fs, args) {
        Ok(ids) if !ids.is_empty() => ids,
        Ok(_) => {
            return fail_with(
                as_json,
                stdout,
                stderr,
                "usage: rush session wait <id>...",
                1,
            );
        }
        Err(e) => return fail_with(as_json, stdout, stderr, &e, 1),
    };
    let needs = match fs.s("until").as_str() {
        "" | "turn" => false,
        "needs-you" => true,
        u => {
            let e = format!("--until is turn or needs-you, not {u:?}");
            return fail_with(as_json, stdout, stderr, &e, 1);
        }
    };
    let start = now();
    let since = match Since::parse(&fs.s("since"), start) {
        Ok(s) => s,
        Err(e) => return fail_with(as_json, stdout, stderr, &e, 1),
    };
    let timeout = match fs.s("timeout").as_str() {
        "" => None,
        t => match rush_events::parse_duration(t) {
            Ok(ns) if ns > 0 => Some(Duration::from_nanos(ns as u64)),
            _ => {
                return fail_with(
                    as_json,
                    stdout,
                    stderr,
                    &format!("--timeout wants a duration, not {t:?}"),
                    1,
                );
            }
        },
    };
    let w = Wait {
        env,
        needs,
        since,
        timeout,
    };
    match w.run(&ids) {
        Ok(woke) => {
            for x in &woke {
                if as_json {
                    write_json(stdout, x);
                } else {
                    print_woke(stdout, x);
                }
            }
            0
        }
        Err((2, _)) => {
            let e = format!("nothing happened in {}", fs.s("timeout"));
            fail_with(as_json, stdout, stderr, &e, 2)
        }
        Err((code, e)) => fail_with(as_json, stdout, stderr, &e, code),
    }
}

/// An error said as the session commands say one; its exit code.
fn fail_with(
    as_json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    e: &str,
    code: i32,
) -> i32 {
    if as_json {
        write_json(stdout, &json!({ "error": e }));
    } else {
        let _ = writeln!(stderr, "rush: {e}");
    }
    code
}

fn print_woke(w: &mut dyn Write, x: &Woke) {
    let turn = if x.turn > 0 {
        format!("turn {} ", x.turn)
    } else {
        String::new()
    };
    let what = if x.event == "needs-you" {
        "needs you"
    } else {
        "ended its turn"
    };
    let _ = writeln!(
        w,
        "{}  {turn}{what} · {} · {}",
        x.session,
        clock(x.at),
        x.name
    );
    match &x.needs_you {
        Some(n) => {
            let _ = writeln!(w, "  needs you: {} · {}", n.kind, n.reason);
        }
        None => {
            let _ = writeln!(w, "  needs you: nothing");
        }
    }
    for l in x.last_reply.trim_end().lines() {
        let _ = writeln!(w, "  │ {l}");
    }
    if x.at != 0 {
        let _ = writeln!(w, "  next: --since {}", format_rfc3339(x.at));
    }
}

/// A wait's settings.
pub struct Wait<'a> {
    pub env: &'a Env,
    pub needs: bool,
    pub since: Since,
    pub timeout: Option<Duration>,
}

impl Wait<'_> {
    /// What happened to the sessions ids name: at once, what already did
    /// after since; else the first that does. Err with its exit code: 1
    /// for an id that names no rush session, 2 timed out, 3 all gone.
    pub fn run(&self, ids: &[String]) -> Result<Vec<Woke>, (i32, String)> {
        let agents = (self.env.load)(&self.env.reg);
        let mut fol = Vec::new();
        for id in ids {
            let host = match find(&agents, id) {
                Ok(a) if a.rush => a.job.id.clone(),
                Ok(_) => {
                    return Err((
                        1,
                        format!("session {id} isn't a rush session: wait follows rush sessions"),
                    ));
                }
                // One the list hasn't caught up with is still waited on.
                Err(_) if rush_host::paths::dir(id).is_dir() => id.clone(),
                Err(e) => return Err((3, e)),
            };
            let path = rush_host::paths::events_path(&host);
            let off = std::fs::metadata(&path).map_or(0, |m| m.len());
            let last = last_turn_end(&path).unwrap_or_default();
            fol.push(Followed {
                gone: gone(&host),
                id: host,
                path,
                off,
                last,
            });
        }
        // Already happened, after what the caller saw: a turn's end, or
        // with --until needs-you a change since then that needs you.
        let mut woke = Vec::new();
        for f in &fol {
            let ended = self.since.after(&f.last);
            let moved = match self.since {
                Since::At(t) => agents
                    .iter()
                    .any(|a| a.rush && a.job.id == f.id && a.job.updated_at > t),
                Since::Turn(_) => false,
            };
            if (ended || self.needs && moved)
                && let Some(x) = self.check(&agents, f, ended)
            {
                woke.push(x);
            }
        }
        if !woke.is_empty() {
            return Ok(woke);
        }
        let stop = Arc::new(rush_sys::fswait::Stop::new().map_err(|e| (1, e.to_string()))?);
        if let Some(t) = self.timeout {
            let s = stop.clone();
            std::thread::Builder::new()
                .name("rush-wait-timeout".into())
                .spawn(move || {
                    // Not a poll: woken only to give up.
                    if !s.wait(t) {
                        s.close();
                    }
                })
                .map_err(|e| (1, e.to_string()))?;
        }
        loop {
            if fol.iter().all(|f| f.gone) {
                let ids: Vec<&str> = fol.iter().map(|f| f.id.as_str()).collect();
                return Err((3, format!("session {} is gone", ids.join(", "))));
            }
            let files: Vec<rush_sys::fswait::File> = fol
                .iter()
                .filter(|f| !f.gone)
                .map(|f| rush_sys::fswait::File {
                    path: f.path.clone(),
                    size: f.off,
                })
                .collect();
            if !rush_sys::fswait::grown(&stop, &files) || stop.is_closed() {
                return Err((2, "timed out".into()));
            }
            let mut woke = Vec::new();
            for f in fol.iter_mut().filter(|f| !f.gone) {
                let (evs, off) = read_from(&f.path, f.off);
                f.off = off;
                let (mut ended, mut blocked) = (false, false);
                for ev in evs {
                    match ev {
                        HostEvent::TurnEnd(e) => {
                            f.last = e;
                            ended = true;
                        }
                        HostEvent::State(s) if s == "blocked" => blocked = true,
                        HostEvent::HostStopped => f.gone = gone(&f.id),
                        HostEvent::State(_) => {}
                    }
                }
                if ended || blocked && self.needs {
                    let agents = (self.env.load)(&self.env.reg);
                    if let Some(x) = self.check(&agents, f, ended) {
                        woke.push(x);
                    }
                }
            }
            if !woke.is_empty() {
                stop.close();
                return Ok(woke);
            }
        }
    }

    /// What to report of f now, if anything: a turn's end (ended), or
    /// with --until needs-you, what it needs of you.
    fn check(&self, agents: &[Agent], f: &Followed, ended: bool) -> Option<Woke> {
        let a = agents.iter().find(|a| a.rush && a.job.id == f.id);
        let needs_you = a.and_then(|a| needs_of(&self.env.reg, a, self.needs || ended));
        if self.needs && needs_you.is_none() || !self.needs && !ended {
            return None;
        }
        let last_reply = a.map_or(String::new(), |a| {
            self.env
                .reg
                .read_preview(a.kind.as_str(), &a.job.transcript_path, 128 << 10)
                .text
        });
        let (turn, at) = if ended {
            (f.last.turn, f.last.at)
        } else {
            (f.last.turn, a.map_or(0, |a| a.job.updated_at))
        };
        Some(Woke {
            session: f.id.clone(),
            name: a.map_or(String::new(), |a| a.display_name.clone()),
            event: if self.needs { "needs-you" } else { "turn" }.into(),
            turn,
            at,
            needs_you,
            last_reply,
        })
    }
}

/// Whether session id is gone for good: no folder, or stopped and not
/// asleep (a message wakes a sleeping one).
fn gone(id: &str) -> bool {
    match rush_host::client::read_info(id) {
        Ok(i) => i.state == "stopped" && !i.sleeping,
        Err(_) => true,
    }
}

/// The events in path from byte off on, and where its whole lines end. A
/// file that shrank (rotated) is read from its start.
fn read_from(path: &std::path::Path, off: u64) -> (Vec<HostEvent>, u64) {
    let Ok(mut f) = std::fs::File::open(path) else {
        return (Vec::new(), off);
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let off = if len < off { 0 } else { off };
    if f.seek(SeekFrom::Start(off)).is_err() {
        return (Vec::new(), off);
    }
    let mut b = Vec::new();
    if f.read_to_end(&mut b).is_err() {
        return (Vec::new(), off);
    }
    // Only whole lines: the rest is read once it's written.
    let whole = b.iter().rposition(|c| *c == b'\n').map_or(0, |i| i + 1);
    let evs = b[..whole]
        .split(|c| *c == b'\n')
        .filter_map(host_event)
        .collect();
    (evs, off + whole as u64)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use rush_core::adapter::{self, Adapter, Features, HistoryReader, Kind, Level, Profile};
    use rush_core::agent::State;
    use rush_core::event::{Event, Message, Part, PartKind, TurnEnd as End};
    use rush_core::tool;
    use rush_ui::turnwants::{TurnWants, WANTS_YOU};

    use super::*;
    use crate::session::session_cmd;

    thread_local! {
        /// What this thread's Env lists.
        static AGENTS: RefCell<Vec<Agent>> = const { RefCell::new(Vec::new()) };
    }

    /// A rush folder for this test, as the session tests' Home.
    struct Home(tempfile::TempDir);

    impl Home {
        fn new() -> Home {
            let d = tempfile::Builder::new()
                .prefix("rm-")
                .tempdir_in("/tmp")
                .unwrap();
            let p = std::fs::canonicalize(d.path()).unwrap();
            rush_state::set_thread_dir(Some(p.clone()));
            rush_state::set_thread_cache_dir(Some(p.join("cache")));
            Home(d)
        }
        fn path(&self) -> PathBuf {
            std::fs::canonicalize(self.0.path()).unwrap()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            rush_state::set_thread_dir(None);
            rush_state::set_thread_cache_dir(None);
        }
    }

    fn text(role: &str, t: &str) -> Event {
        Event::Message(Message {
            role: role.into(),
            parts: vec![Part {
                kind: PartKind::Text,
                text: t.into(),
                ..Part::default()
            }],
            ..Message::default()
        })
    }

    fn call(id: &str, command: &str) -> Event {
        Event::Message(Message {
            role: "assistant".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(tool::Call {
                    id: id.into(),
                    name: "Bash".into(),
                    kind: tool::Kind::Shell,
                    input: tool::Input {
                        command: command.into(),
                        ..tool::Input::default()
                    },
                    ..tool::Call::default()
                })),
                ..Part::default()
            }],
            ..Message::default()
        })
    }

    /// An agent whose history is two turns, read back through its
    /// adapter as any agent but Claude Code's is.
    struct Hist;

    impl Adapter for Hist {
        fn kind(&self) -> Kind {
            Kind::from("hist")
        }
        fn name(&self) -> String {
            "Hist".into()
        }
        fn features(&self) -> Features {
            Features::default()
        }
        fn level(&self) -> Level {
            Level::default()
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        rush_core::has!(history_reader: HistoryReader);
    }

    impl HistoryReader for Hist {
        fn history(
            &self,
            _: &adapter::Session,
            _: Ms,
            _: usize,
            each: &mut dyn FnMut(Vec<Event>) -> bool,
        ) -> adapter::Result<()> {
            each(vec![
                text("user", "look at the flaky parser test"),
                text("assistant", "It fails on empty input."),
                Event::TurnEnd(End {
                    reason: "done".into(),
                    ..End::default()
                }),
                text("user", "fix it then"),
                call("c1", "cargo test -p parser"),
                text("assistant", "Fixed: the parser now takes empty input."),
                Event::TurnEnd(End {
                    reason: "done".into(),
                    ..End::default()
                }),
            ]);
            Ok(())
        }
    }

    fn env() -> Env {
        let reg = Registry::new();
        reg.register(Hist);
        Env {
            reg,
            spawn: |_, _| Err("no hosts here".into()),
            alive: |_| false,
            load: |_| AGENTS.with(|a| a.borrow().clone()),
        }
    }

    fn run(env: &Env, args: &[&str]) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = session_cmd(env, &args, &mut &b""[..], &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn set_agents(v: Vec<Agent>) {
        AGENTS.with(|a| *a.borrow_mut() = v);
    }

    /// A session of the Hist agent, its transcript a file holding its
    /// words (a search passes over a file without them unread).
    fn hist_agent(home: &Home) -> Agent {
        let path = home.path().join("hist.jsonl");
        std::fs::write(
            &path,
            "look at the flaky parser test\nIt fails on empty input.\nfix it then\n\
             cargo test -p parser\nFixed: the parser now takes empty input.\n",
        )
        .unwrap();
        let mut a = Agent {
            kind: Kind::from("hist"),
            key: "hist:default:s-1234567890".into(),
            display_name: "parser fix".into(),
            ..Agent::default()
        };
        a.job.session_id = "s-1234567890".into();
        a.job.transcript_path = path.to_string_lossy().into_owned();
        a.job.updated_at = now();
        a
    }

    // read: the last turn unless told, your words and the agent's whole, a
    // line a call; any agent's session, found by its session id or the
    // start of one.
    #[test]
    fn reads_a_conversation() {
        let home = Home::new();
        let env = env();
        set_agents(vec![hist_agent(&home)]);
        let (code, out, err) = run(&env, &["read", "s-1234567890"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(
            out.starts_with("s-1234567890  parser fix\n── turn 2\nyou · -\nfix it then\n"),
            "{out}"
        );
        assert!(out.contains("  → - "), "{out}");
        assert!(
            out.ends_with("agent · -\nFixed: the parser now takes empty input.\n"),
            "{out}"
        );
        assert!(!out.contains("flaky"), "{out}");

        let (code, out, _) = run(&env, &["read", "s-12345678", "--turns", "2", "--json"]);
        assert_eq!(code, 0);
        let v: Value = serde_json::from_str(&out).unwrap();
        let es = v["entries"].as_array().unwrap();
        let who: Vec<&str> = es.iter().map(|e| e["who"].as_str().unwrap()).collect();
        assert_eq!(who, ["you", "agent", "you", "tool", "agent"]);
        assert_eq!(es[0]["text"], "look at the flaky parser test");
        assert_eq!(es[3]["tool"], "Bash");
        assert_eq!(
            (es[0]["turn"].as_u64(), es[4]["turn"].as_u64()),
            (Some(1), Some(2))
        );

        let (_, out, _) = run(&env, &["read", "s-1234567890", "--last", "1", "--json"]);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["entries"].as_array().unwrap().len(), 1);

        let cases: &[(&[&str], &str)] = &[
            (&["read", "nosuch"], "session nosuch not found"),
            (&["read"], "usage: rush session read <id>"),
            (
                &["read", "s-1234567890", "--last", "1", "--turns", "1"],
                "--last and --turns don't go together",
            ),
            (
                &["read", "s-1234567890", "--last", "x"],
                "--last wants a number above 0, not \"x\"",
            ),
        ];
        for (args, want) in cases {
            let (code, _, err) = run(&env, args);
            assert_eq!((code, err), (1, format!("rush: {want}\n")), "{args:?}");
        }
    }

    // search: each session's matches, with its turn, who and the words
    // around them; --since leaves out sessions quiet since.
    #[test]
    fn searches_conversations() {
        let home = Home::new();
        let env = env();
        let mut old = hist_agent(&home);
        old.job.session_id = "s-old".into();
        old.key = "hist:default:s-old".into();
        old.job.updated_at = now() - 5 * 3_600_000;
        set_agents(vec![hist_agent(&home), old]);
        let (code, out, err) = run(&env, &["search", "empty input", "--json"]);
        assert_eq!((code, err.as_str()), (0, ""));
        let v: Vec<Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(v.len(), 2, "{out}");
        assert_eq!(v[0]["session"], "s-1234567890");
        let hit = &v[0]["hits"][0];
        assert!(
            hit["snippet"].as_str().unwrap().contains("empty input"),
            "{out}"
        );
        assert!(hit["turn"].as_u64().unwrap() >= 1);

        let (_, out, _) = run(&env, &["search", "empty input", "--since", "2h", "--json"]);
        let v: Vec<Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(v.len(), 1, "{out}");

        let (_, out, _) = run(&env, &["search", "empty input", "s-old"]);
        assert!(out.starts_with("s-old  parser fix\n  turn "), "{out}");
        let (_, out, _) = run(&env, &["search", "nowhere-said"]);
        assert_eq!(out, "");
        let (code, _, err) = run(&env, &["search"]);
        assert_eq!(
            (code, err.as_str()),
            (1, "rush: usage: rush session search <text> [<id>...]\n")
        );
    }

    fn no_model(_: &Registry, _: &str, _: &str) -> Option<TurnWants> {
        None
    }

    /// A rush session's row, as the list has it.
    fn rush_row(id: &str, state: State, needs: &str) -> Agent {
        let mut a = Agent {
            kind: Kind::from("hist"),
            key: format!("default:a:{id}"),
            display_name: format!("session {id}"),
            rush: true,
            pid: 4242,
            ..Agent::default()
        };
        a.job.id = id.into();
        a.job.state = state;
        a.job.needs = needs.into();
        a.job.updated_at = now() - 1000;
        a
    }

    // needs: what the list's Needs you section reads of each: a card or a
    // permission waiting, a decisions card, a finished turn's reading from
    // the cache, or nothing.
    #[test]
    fn says_what_needs_you() {
        let home = Home::new();
        reading::set_wanter(no_model);
        let env = env();
        let approval = rush_row("a1", State::Blocked, "Bash rm -rf build");
        let question = rush_row("a2", State::Blocked, "asks: which theme to keep?");
        let mut decision = rush_row("a3", State::Blocked, "2 decisions");
        decision.decides = true;
        decision.decisions_key = "Scope\x1fFormat\x1f".into();
        let mut command = rush_row("a4", State::Done, "");
        command.job.transcript_path = "/nonexistent/t.jsonl".into();
        let mut seen = rush_row("a5", State::Done, "");
        seen.seen = true;
        // The leading view's reading of a4's finished turn.
        let t = TurnWants {
            at: command.job.updated_at,
            wants: WANTS_YOU.into(),
            line: "merge PR #41".into(),
            quote: "run gh pr merge 41 when you're happy".into(),
            ui: vec![rush_convo::render::NeedsItem {
                kind: "run".into(),
                text: "gh pr merge 41".into(),
            }],
        };
        let cache = rush_state::cache_path("turns.json");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(
            &cache,
            serde_json::to_vec(&HashMap::from([(command.key.clone(), t)])).unwrap(),
        )
        .unwrap();
        set_agents(vec![approval, question, decision, command, seen]);
        let (code, out, err) = run(&env, &["needs", "a1", "a2", "a3", "a4", "a5"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(
            out,
            "a1  approval: Bash rm -rf build  session a1\n  │ Bash rm -rf build\n\
             a2  question: asks: which theme to keep?  session a2\n  │ asks: which theme to keep?\n\
             a3  decision: 2 decisions  session a3\n  │ Scope; Format\n\
             a4  command: merge PR #41  session a4\n  │ run gh pr merge 41 when you're happy\n\
             a5  none  session a5\n"
        );
        let (_, out, _) = run(&env, &["needs", "--json", "a4", "a5"]);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v[0]["needsYou"]["kind"], "command");
        assert_eq!(v[0]["needsYou"]["reason"], "merge PR #41");
        assert_eq!(v[1]["needsYou"], Value::Null);
        let (code, out, _) = run(&env, &["needs", "nosuch", "--json"]);
        assert_eq!(
            (code, out.as_str()),
            (1, "{\"error\":\"session nosuch not found\"}\n")
        );
        drop(home);
    }

    /// A rush session's folder: its info, and its events.jsonl holding
    /// lines.
    fn host_dir(id: &str, state: &str, sleeping: bool, lines: &[String]) -> PathBuf {
        let d = rush_host::paths::dir(id);
        std::fs::create_dir_all(&d).unwrap();
        let info = Info {
            id: id.into(),
            state: state.into(),
            sleeping,
            ..Info::default()
        };
        std::fs::write(d.join("info.json"), info.to_json()).unwrap();
        let p = d.join("events.jsonl");
        let mut b = String::new();
        for l in lines {
            b.push_str(l);
            b.push('\n');
        }
        std::fs::write(&p, b).unwrap();
        p
    }

    fn turn_end(turn: i64, at: Ms) -> String {
        json!({"kind": "turn-end", "reason": "done", "session": "w", "turn": turn,
            "t": format_rfc3339(at)})
        .to_string()
    }

    // wait: what already happened after --since is reported at once; else
    // the next turn's end as the host writes it; a timeout ends 2, a
    // session gone 3, one rush doesn't host 1.
    #[test]
    fn waits_on_turns() {
        let home = Home::new();
        reading::set_wanter(no_model);
        let env = env();
        let t0 = now() - 60_000;
        let ev = host_dir(
            "w1",
            "idle",
            true,
            &[
                json!({"kind": "state", "info": {"state": "working"}}).to_string(),
                turn_end(3, t0),
            ],
        );
        host_dir("w2", "stopped", false, &[]);
        let other = hist_agent(&home);
        set_agents(vec![
            rush_row("w1", State::Done, ""),
            rush_row("w2", State::Stopped, ""),
            other,
        ]);

        let (code, out, err) = run(&env, &["wait", "w1", "--since", "2"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(out.starts_with("w1  turn 3 ended its turn · "), "{out}");
        assert!(out.contains("  needs you: nothing\n"), "{out}");
        assert!(
            out.ends_with(&format!("  next: --since {}\n", format_rfc3339(t0))),
            "{out}"
        );
        let since = format_rfc3339(t0 - 1000);
        let (code, out, _) = run(&env, &["wait", "w1", "w2", "--since", &since, "--json"]);
        assert_eq!(code, 0);
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(
            (v["session"].as_str(), v["turn"].as_i64()),
            (Some("w1"), Some(3))
        );
        assert_eq!(v["event"], "turn");

        // Seen already: nothing more by the timeout.
        let (code, _, err) = run(&env, &["wait", "w1", "--since", "3", "--timeout", "1ms"]);
        assert_eq!((code, err.as_str()), (2, "rush: nothing happened in 1ms\n"));

        // The next turn's end, written while it waits (or before it
        // looks: either way it's after turn 3).
        let writer = std::thread::spawn(move || {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new().append(true).open(ev).unwrap();
            writeln!(f, "{}", turn_end(4, now())).unwrap();
        });
        let (code, out, _) = run(&env, &["wait", "w1", "--since", "3", "--timeout", "1m"]);
        writer.join().unwrap();
        assert_eq!(code, 0, "{out}");
        assert!(out.starts_with("w1  turn 4 ended its turn"), "{out}");

        let cases: &[(&[&str], i32, &str)] = &[
            (&["wait", "w2"], 3, "session w2 is gone"),
            (&["wait", "nosuch"], 3, "session nosuch not found"),
            (
                &["wait", "s-1234567890"],
                1,
                "session s-1234567890 isn't a rush session: wait follows rush sessions",
            ),
            (
                &["wait", "w1", "--until", "x"],
                1,
                "--until is turn or needs-you, not \"x\"",
            ),
            (&["wait"], 1, "usage: rush session wait <id>..."),
        ];
        for (args, code, want) in cases {
            let (c, _, err) = run(&env, args);
            assert_eq!((c, err), (*code, format!("rush: {want}\n")), "{args:?}");
        }
    }

    // wait --until needs-you: a permission asked for after --since is
    // reported at once with what it needs.
    #[test]
    fn waits_until_needed() {
        let _home = Home::new();
        reading::set_wanter(no_model);
        let env = env();
        host_dir("n1", "blocked", false, &[]);
        let row = rush_row("n1", State::Blocked, "Bash git push");
        let since = format_rfc3339(row.job.updated_at - 1000);
        set_agents(vec![row]);
        let (code, out, err) = run(
            &env,
            &["wait", "n1", "--until", "needs-you", "--since", &since],
        );
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(out.starts_with("n1  needs you · "), "{out}");
        assert!(
            out.contains("  needs you: approval · Bash git push\n"),
            "{out}"
        );
    }

    // The host's lines wait reads, and the rest passed over.
    #[test]
    fn reads_host_events() {
        let cases: &[(&str, Option<HostEvent>)] = &[
            (
                r#"{"kind":"turn-end","t":"2026-11-10T00:00:01Z","turn":7}"#,
                Some(HostEvent::TurnEnd(TurnEnd {
                    turn: 7,
                    at: parse_rfc3339("2026-11-10T00:00:01Z"),
                })),
            ),
            (
                r#"{"kind":"turn-end","t":"2026-11-10T00:00:01Z"}"#,
                Some(HostEvent::TurnEnd(TurnEnd {
                    turn: 0,
                    at: parse_rfc3339("2026-11-10T00:00:01Z"),
                })),
            ),
            (
                r#"{"kind":"state","info":{"state":"blocked"}}"#,
                Some(HostEvent::State("blocked".into())),
            ),
            (r#"{"kind":"host-stopped"}"#, Some(HostEvent::HostStopped)),
            (r#"{"kind":"delivered","text":"state"}"#, None),
            ("not json \"state\"", None),
        ];
        for (line, want) in cases {
            assert_eq!(&host_event(line.as_bytes()), want, "{line}");
        }
        assert_eq!(Since::parse("12", 0), Ok(Since::Turn(12)));
        assert_eq!(Since::parse("", 5), Ok(Since::At(5)));
        assert!(Since::parse("whenever", 0).is_err());
    }
}
