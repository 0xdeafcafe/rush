//! What the broker needs of rush's session hosts (Go's internal/host): the
//! sessions there are, one's info, a connection to its host to send it
//! messages or follow what it says, starting one, its own events, and the
//! views beside its conversation.
//!
//! The host module is ported on its own; [`Hosts`] is where it plugs in.
//! [`FileHosts`] does what needs only the files and sockets Go's hosts
//! keep: listing and reading sessions, dialling a host and its ops, and
//! a session's events. Starting a session and its views need the host
//! module itself, and say so until it's there.
//!
//! Ported from internal/host/client.go:179-320 (ReadInfo, List, Lister),
//! :323-418 (the lines a host sends, Decode), :420-480 and :483-541 (Dial
//! and the ops plugind uses), events.go:20-31 (LogEvent), host.go:359-382
//! (where sessions live) and :2435 (alive).

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rush_core::event::Event;
use rush_core::tool::Call;
use rush_events::Fields;
use rush_state::Time;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

/// Proto is the host protocol this client speaks, said in its hello: the
/// host then sends rush's own events (agtop_ev) rather than the agent's
/// lines.
pub const PROTO: i64 = 11;

/// A session's info, as its host publishes it in info.json and on its
/// socket: what plugind reads of it. Unknown fields are left alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Info {
    #[serde(deserialize_with = "or_default")]
    pub id: String,
    #[serde(rename = "sessionId", deserialize_with = "or_default")]
    pub session_id: String,
    #[serde(deserialize_with = "or_default")]
    pub cwd: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub name: String,
    #[serde(rename = "hostPid", deserialize_with = "or_default")]
    pub host_pid: i64,
    /// starting, working, blocked, idle, stopped
    #[serde(deserialize_with = "or_default")]
    pub state: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub detail: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub needs: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub model: String,
    #[serde(
        rename = "permissionMode",
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub permission_mode: String,
    #[serde(
        rename = "costUsd",
        skip_serializing_if = "is_zero_f",
        deserialize_with = "or_default"
    )]
    pub cost_usd: f64,
    /// Messages sent while the agent was busy.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "or_default")]
    pub queue: Vec<String>,
    #[serde(rename = "startedAt", deserialize_with = "or_default")]
    pub started_at: Time,
    #[serde(rename = "updatedAt", deserialize_with = "or_default")]
    pub updated_at: Time,
    #[serde(skip_serializing_if = "is_false", deserialize_with = "or_default")]
    pub sleeping: bool,
    /// The machine a remote session is really on.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub remote: String,
    /// The agent it runs: empty is Claude Code.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub kind: String,
    /// The plugin that started it, if one did.
    #[serde(
        rename = "startedBy",
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub started_by: String,
    #[serde(
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "or_default"
    )]
    pub meta: BTreeMap<String, String>,
    #[serde(
        rename = "contextTokens",
        skip_serializing_if = "is_zero",
        deserialize_with = "or_default"
    )]
    pub context_tokens: i64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn is_zero_f(n: &f64) -> bool {
    *n == 0.0
}

/// null, as Go's decoder takes it: the field left as it was.
fn or_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

impl Info {
    /// A session whose host has gone is stopped, unless it went to sleep
    /// (ReadInfo's rule).
    fn settle(&mut self) {
        self.kind = rush_core::adapter::migrated(&self.kind).0;
        if !alive(self.host_pid) && !self.sleeping {
            self.state = "stopped".into();
        }
    }
}

/// Whether process pid is there (signal 0), even if it isn't ours.
pub fn alive(pid: i64) -> bool {
    if pid <= 0 || pid > i32::MAX as i64 {
        return false;
    }
    // SAFETY: signal 0 only asks whether the process exists.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// One thing a host said, as plugind follows it.
#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    /// The session's info: on connect, and whenever it changes.
    Info(Box<Info>),
    /// An op the host couldn't carry out.
    Error(String),
    /// A message a client sent, echoed so every client shows it.
    Sent(String),
    /// What the session's agent said or did.
    Event(Box<Event>),
}

/// Reads one line from a host (Go's Decode): None for the lines plugind
/// has no use for.
pub fn decode_line(line: &[u8]) -> Option<Vec<HostEvent>> {
    // The agent's own lines start with their type; the host's own are
    // written with sorted keys and don't. With the hello said, an agent's
    // lines come as rush's own events instead.
    if line.starts_with(br#"{"type":""#) && !line.starts_with(br#"{"type":"agtop_"#) {
        return None;
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Head {
        #[serde(rename = "type", deserialize_with = "or_default")]
        kind: String,
        #[serde(deserialize_with = "or_default")]
        info: Info,
        #[serde(deserialize_with = "or_default")]
        error: String,
        #[serde(rename = "agtop_sent", deserialize_with = "or_default")]
        sent: bool,
        message: Option<Box<RawValue>>,
        ev: Option<Box<RawValue>>,
    }
    let head: Head = serde_json::from_slice(line).ok()?;
    match head.kind.as_str() {
        "agtop_info" => {
            let mut i = head.info;
            i.kind = rush_core::adapter::migrated(&i.kind).0;
            return Some(vec![HostEvent::Info(Box::new(i))]);
        }
        "agtop_error" => return Some(vec![HostEvent::Error(head.error)]),
        "agtop_ev" => {
            let ev = rush_core::event::unmarshal(head.ev.as_ref()?.get().as_bytes()).ok()?;
            return Some(vec![HostEvent::Event(Box::new(ev))]);
        }
        _ => {}
    }
    if head.sent {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct M {
            #[serde(deserialize_with = "or_default")]
            content: String,
        }
        let m: M = head
            .message
            .and_then(|m| serde_json::from_str(m.get()).ok())
            .unwrap_or_default();
        return Some(vec![HostEvent::Sent(m.content)]);
    }
    // The host's other lines (its time stamps, commands, context, …) say
    // nothing plugind follows; the agent's own are its adapter's to read.
    head.kind.starts_with("agtop_").then(Vec::new)
}

/// An op for a session's host, as Go's host.op writes it (its fields in
/// Go's order, the empty ones left out).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Op {
    pub op: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cause: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "is_false")]
    pub now: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub index: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub was: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub proto: i64,
}

impl Op {
    fn named(op: &str) -> Op {
        Op {
            op: op.into(),
            ..Default::default()
        }
    }
    /// Sends text: queued behind the turn under way, or now, stopping it.
    pub fn send(text: &str, now: bool) -> Op {
        Op {
            text: text.into(),
            now,
            ..Op::named("send")
        }
    }
    pub fn interrupt() -> Op {
        Op::named("interrupt")
    }
    /// Ends the session and its host; the conversation is kept.
    pub fn stop() -> Op {
        Op::named("stop")
    }
    /// Sends queued message i (was, as the client saw it) now.
    pub fn queue_send(i: i64, was: &str) -> Op {
        Op {
            index: i,
            was: was.into(),
            ..Op::named("queue_send")
        }
    }
    /// Drops queued message i.
    pub fn queue_remove(i: i64, was: &str) -> Op {
        Op {
            index: i,
            was: was.into(),
            ..Op::named("queue_remove")
        }
    }
    fn hello() -> Op {
        Op {
            proto: PROTO,
            ..Op::named("hello")
        }
    }
}

/// A connection to one session's host.
pub trait HostClient: Send + Sync {
    /// Has the host carry out o.
    fn op(&self, o: Op) -> Result<(), String>;
    /// Ends the connection; its lines then end.
    fn close(&self);
}

/// A host dialled: the connection, and what it says, a line at a time
/// (replay first), until it closes.
pub struct Dialed {
    pub client: Arc<dyn HostClient>,
    pub lines: mpsc::Receiver<Vec<HostEvent>>,
}

/// How a plugin's session is started (the part of Go's host.Config
/// plugind fills in).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpawnConfig {
    /// Where every session runs: the account in use.
    pub account: rush_state::Folder,
    pub cwd: String,
    pub prompt: String,
    pub name: String,
    pub model: String,
    pub effort: String,
    pub permission_mode: String,
    pub limit_mode: String,
    pub lean: bool,
    pub started_by: String,
    pub meta: BTreeMap<String, String>,
}

/// A view beside a session's conversation (Go's host.View), as JSON: the
/// broker names it and says who drew it; the host checks the rest.
pub type View = serde_json::Map<String, serde_json::Value>;

/// Lists sessions again and again, reading only what changed since its
/// last look.
pub type Lister = Box<dyn FnMut() -> Vec<Info> + Send>;

/// rush's session hosts, as the broker reaches them.
pub trait Hosts: Send + Sync {
    /// Every session, newest first.
    fn list(&self) -> Vec<Info> {
        (self.lister())()
    }
    /// A lister of its own, for a watch.
    fn lister(&self) -> Lister;
    /// A session's last published info; a session whose host has gone is
    /// stopped, unless it went to sleep.
    fn read_info(&self, id: &str) -> Result<Info, String>;
    /// Connects to a running session's host.
    fn dial(&self, id: &str) -> Result<Dialed, String>;
    /// Starts a session, and says its id.
    fn spawn(&self, cfg: &SpawnConfig) -> Result<String, String>;
    /// Records an event in a session's own events (and rush's).
    fn log_event(&self, id: &str, kind: &str, fields: Fields);
    /// The views a session has.
    fn views(&self, id: &str) -> Vec<View>;
    /// Keeps view v of session id; the same name replaces it.
    fn save_view(&self, id: &str, v: View) -> Result<(), String>;
    /// Takes view name of session id away; none is no error.
    fn drop_view(&self, id: &str, name: &str) -> Result<(), String>;
    /// A tool call in a few words, as agent kind says it.
    fn doing(&self, kind: &str, c: &Call) -> String {
        let _ = kind;
        rush_convo::tool::doing(&convo_call(c))
    }
}

/// c as rush-convo words calls (Go's tool.Doing): the fields its words
/// use.
fn convo_call(c: &Call) -> rush_convo::tool::Call {
    use rush_convo::tool::Kind as K;
    use rush_core::tool::Kind;
    let kind = match c.kind {
        Kind::Other => K::Other,
        Kind::Shell => K::Shell,
        Kind::Read => K::Read,
        Kind::Edit => K::Edit,
        Kind::Write => K::Write,
        Kind::Delete => K::Delete,
        Kind::Move => K::Move,
        Kind::Search => K::Search,
        Kind::Glob => K::Glob,
        Kind::Fetch => K::Fetch,
        Kind::WebSearch => K::WebSearch,
        Kind::Subagent => K::Subagent,
        Kind::Todo => K::Todo,
        Kind::Question => K::Question,
        Kind::PlanMode => K::PlanMode,
        Kind::Mcp => K::Mcp,
        Kind::Notebook => K::Notebook,
        Kind::Think => K::Think,
    };
    let i = &c.input;
    rush_convo::tool::Call {
        id: c.id.clone(),
        name: c.name.clone(),
        kind,
        title: c.title.clone(),
        input: rush_convo::tool::Input {
            command: i.command.clone(),
            cwd: i.cwd.clone(),
            path: i.path.clone(),
            to: i.to.clone(),
            pattern: i.pattern.clone(),
            query: i.query.clone(),
            url: i.url.clone(),
            description: i.description.clone(),
            prompt: i.prompt.clone(),
            agent: i.agent.clone(),
            server: i.server.clone(),
            tool: i.tool.clone(),
            ..Default::default()
        },
    }
}

const NO_HOST_MODULE: &str = "rush's host module isn't in this build yet";

/// The hosts Go rush keeps, by their files under rush's folder.
#[derive(Clone, Default)]
pub struct FileHosts {
    /// Where a session's events go besides its own file: rush's log,
    /// unless a test gives it one.
    events: Option<Arc<rush_events::Logger>>,
}

impl FileHosts {
    /// Sessions' events logged to l rather than rush's log.
    pub fn logging_to(l: Arc<rush_events::Logger>) -> FileHosts {
        FileHosts { events: Some(l) }
    }
}

/// Holds one folder per session.
pub fn root() -> PathBuf {
    rush_state::dir().join("sessions")
}

fn dir(id: &str) -> PathBuf {
    root().join(id)
}

/// Where a session's host listens.
pub fn sock_path(id: &str) -> PathBuf {
    dir(id).join("host.sock")
}

/// Where a session's events are kept.
pub fn events_path(id: &str) -> PathBuf {
    dir(id).join("events.jsonl")
}

fn read_info_file(id: &str) -> Result<Info, String> {
    let p = dir(id).join("info.json");
    let b = std::fs::read(&p).map_err(|e| format!("open {}: {e}", p.display()))?;
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}

/// A session's info file as last read.
struct Listed {
    modified: SystemTime,
    size: u64,
    info: Option<Info>,
}

impl Hosts for FileHosts {
    fn lister(&self) -> Lister {
        let mut seen: HashMap<String, Listed> = HashMap::new();
        Box::new(move || {
            let mut out = Vec::new();
            let mut now = HashMap::new();
            for e in std::fs::read_dir(root()).into_iter().flatten().flatten() {
                let id = e.file_name().to_string_lossy().into_owned();
                let Ok(md) = std::fs::metadata(dir(&id).join("info.json")) else {
                    continue;
                };
                let modified = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let l = match seen.remove(&id) {
                    Some(l) if l.modified == modified && l.size == md.len() => l,
                    _ => Listed {
                        modified,
                        size: md.len(),
                        info: read_info_file(&id).ok(),
                    },
                };
                if let Some(i) = &l.info {
                    let mut i = i.clone();
                    i.settle();
                    out.push(i);
                }
                now.insert(id, l);
            }
            seen = now;
            out.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
            out
        })
    }

    fn read_info(&self, id: &str) -> Result<Info, String> {
        let mut i = read_info_file(id)?;
        i.settle();
        Ok(i)
    }

    fn dial(&self, id: &str) -> Result<Dialed, String> {
        dial_sock(&sock_path(id))
    }

    fn spawn(&self, _cfg: &SpawnConfig) -> Result<String, String> {
        Err(NO_HOST_MODULE.into())
    }

    fn log_event(&self, id: &str, kind: &str, fields: Fields) {
        if id.is_empty() {
            return;
        }
        match &self.events {
            Some(l) => l.session(id, &events_path(id), kind, fields),
            None => rush_events::session(id, &events_path(id), kind, fields),
        }
    }

    fn views(&self, _id: &str) -> Vec<View> {
        Vec::new()
    }

    fn save_view(&self, _id: &str, _v: View) -> Result<(), String> {
        Err(NO_HOST_MODULE.into())
    }

    fn drop_view(&self, _id: &str, _name: &str) -> Result<(), String> {
        Err(NO_HOST_MODULE.into())
    }
}

/// A host's socket, written a line an op.
struct SockClient {
    w: Mutex<UnixStream>,
    closer: UnixStream,
}

impl HostClient for SockClient {
    fn op(&self, mut o: Op) -> Result<(), String> {
        if o.cause.is_empty() {
            o.cause = rush_events::cause();
        }
        let mut b = rush_state::json::to_vec(&o).map_err(|e| e.to_string())?;
        b.push(b'\n');
        let mut w = self.w.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(&b).map_err(|e| e.to_string())
    }

    fn close(&self) {
        let _ = self.closer.shutdown(std::net::Shutdown::Both);
    }
}

/// Connects to the host listening at path, saying hello, with a thread
/// reading what it says until it closes.
pub fn dial_sock(path: &std::path::Path) -> Result<Dialed, String> {
    let s = UnixStream::connect(path).map_err(|e| e.to_string())?;
    let r = s.try_clone().map_err(|e| e.to_string())?;
    let closer = s.try_clone().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::sync_channel(1024);
    std::thread::Builder::new()
        .name("host-lines".into())
        .spawn(move || {
            let mut r = BufReader::new(r);
            let mut line = Vec::new();
            loop {
                line.clear();
                match r.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                if let Some(evs) = decode_line(line.trim_ascii())
                    && !evs.is_empty()
                    && tx.send(evs).is_err()
                {
                    return;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let c = SockClient {
        w: Mutex::new(s),
        closer,
    };
    if let Err(e) = c.op(Op::hello()) {
        c.close();
        return Err(e);
    }
    Ok(Dialed {
        client: Arc::new(c),
        lines: rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_what_plugind_follows() {
        let info =
            br#"{"type":"agtop_info","info":{"id":"a","state":"idle","queue":null,"hostPid":0}}"#;
        let Some(v) = decode_line(info) else {
            panic!("info not read")
        };
        let HostEvent::Info(i) = &v[0] else {
            panic!("{v:?}")
        };
        assert_eq!(
            (i.id.as_str(), i.state.as_str(), i.kind.as_str()),
            ("a", "idle", "claude")
        );
        assert_eq!(
            decode_line(br#"{"error":"no","type":"agtop_error"}"#),
            Some(vec![HostEvent::Error("no".into())])
        );
        assert_eq!(
            decode_line(
                br#"{"agtop_sent":true,"message":{"content":"hi","role":"user"},"type":"user"}"#
            ),
            Some(vec![HostEvent::Sent("hi".into())])
        );
        // Claude Code's own lines are read by its adapter, not here.
        assert_eq!(decode_line(br#"{"type":"assistant","message":{}}"#), None);
        assert_eq!(decode_line(br#"{"t":5,"type":"agtop_time"}"#), Some(vec![]));
        let ev = br#"{"ev":{"t":"turn_end","e":{"Reason":"done","Text":"ok"}},"type":"agtop_ev"}"#;
        match decode_line(ev).as_deref() {
            Some([HostEvent::Event(e)]) => match &**e {
                Event::TurnEnd(t) => assert_eq!(t.text, "ok"),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ops_as_go_writes_them() {
        let b = rush_state::json::to_vec(&Op {
            cause: "c".into(),
            ..Op::queue_send(2, "x")
        })
        .unwrap();
        assert_eq!(
            String::from_utf8(b).unwrap(),
            r#"{"op":"queue_send","cause":"c","index":2,"was":"x"}"#
        );
        let b = rush_state::json::to_vec(&Op::send("hi", true)).unwrap();
        assert_eq!(
            String::from_utf8(b).unwrap(),
            r#"{"op":"send","text":"hi","now":true}"#
        );
    }
}
