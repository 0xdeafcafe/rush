//! The little of a session's host the menu bar needs: connecting to it,
//! reading what its replay says it waits on, and answering it. Port of
//! internal/host/client.go:433-560 (Dial, hello, Allow, Deny,
//! ContinueAtReset), client.go:352-410 and decoder.go (Decode: only the
//! lines that can ask or settle), answer.go (AnswerInput), and
//! internal/menubar/feed.go:380-450 (waitingOn, waits, skipped).
//!
//! The host itself (wave B) isn't ported yet; this speaks its protocol as
//! Go's hosts do today, so the Rust feed answers Go-hosted sessions.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant};

use rush_core::event::{self, Event, Question};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The host protocol this client speaks (host.go:307 Proto).
pub const PROTO: i64 = 11;

/// How long reading what a host waits on may take.
const WAIT_ON: Duration = Duration::from_secs(2);

/// A session's folder under rush's (host.go:359 Root, dir).
pub fn session_dir(id: &str) -> PathBuf {
    rush_state::dir().join("sessions").join(id)
}

/// Where a session's host listens.
pub fn sock_path(id: &str) -> PathBuf {
    session_dir(id).join("host.sock")
}

/// Where a session's own events are kept (host/events.go EventsPath).
pub fn events_path(id: &str) -> PathBuf {
    session_dir(id).join("events.jsonl")
}

/// What a host says of a usage limit that stopped its session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Limit {
    pub window: String,
    #[serde(rename = "continue")]
    pub carry_on: bool,
    /// Set while carrying on at the reset is still yours to decide.
    pub ask: bool,
}

/// The part of a host's Info the menu bar reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Info {
    pub id: String,
    pub state: String,
    pub needs: String,
    pub limit: Option<Limit>,
}

/// One line from a host, as far as the menu bar cares.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// The session's info: the replay's end.
    Info(Info),
    /// A request settled, here or by another client.
    Answered(String),
    /// One of rush's own events.
    Event(Event),
    /// Anything else.
    Other,
}

/// A connection to one session's host.
pub struct Client {
    w: Mutex<UnixStream>,
    /// Every line from the host, replay first; it ends when the
    /// connection does.
    pub lines: Receiver<Vec<u8>>,
}

#[derive(Serialize, Default)]
struct Op<'a> {
    op: &'a str,
    #[serde(skip_serializing_if = "String::is_empty")]
    cause: String,
    #[serde(skip_serializing_if = "str::is_empty")]
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    now: bool,
    #[serde(skip_serializing_if = "is_zero")]
    proto: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Client {
    /// Connects to a running session, saying hello.
    pub fn dial(id: &str) -> io::Result<Client> {
        Client::connect(&sock_path(id))
    }

    /// Connects to the host listening at sock, saying hello.
    pub fn connect(sock: &std::path::Path) -> io::Result<Client> {
        rush_core::sandbox::guard("a host's socket", sock);
        let s = UnixStream::connect(sock)?;
        let r = s.try_clone()?;
        let (tx, lines) = sync_channel(1024);
        std::thread::Builder::new()
            .name("menubar-host".into())
            .spawn(move || {
                let mut r = BufReader::new(r);
                loop {
                    let mut l = Vec::new();
                    match r.read_until(b'\n', &mut l) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    if l.last() == Some(&b'\n') {
                        l.pop();
                    }
                    // Gone once the client is: nobody reads on.
                    if tx.send(l).is_err() {
                        return;
                    }
                }
            })?;
        let c = Client {
            w: Mutex::new(s),
            lines,
        };
        c.op(Op {
            op: "hello",
            proto: PROTO,
            ..Op::default()
        })?;
        Ok(c)
    }

    fn op(&self, mut o: Op<'_>) -> io::Result<()> {
        o.cause = rush_events::cause();
        let mut b = serde_json::to_vec(&o).map_err(io::Error::other)?;
        b.push(b'\n');
        self.w
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&b)
    }

    /// Lets a pending tool call run; input None keeps the requested input.
    pub fn allow(&self, id: &str, input: Option<&RawValue>) -> io::Result<()> {
        self.op(Op {
            op: "allow",
            id,
            input,
            ..Op::default()
        })
    }

    pub fn deny(&self, id: &str) -> io::Result<()> {
        self.op(Op {
            op: "deny",
            id,
            ..Op::default()
        })
    }

    /// Says whether a session stopped by a usage limit carries on when the
    /// limit resets.
    pub fn continue_at_reset(&self, yes: bool) -> io::Result<()> {
        self.op(Op {
            op: "limit",
            now: yes,
            ..Op::default()
        })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let s = self.w.lock().unwrap_or_else(|e| e.into_inner());
        let _ = s.shutdown(std::net::Shutdown::Both);
    }
}

/// A replay line of Claude Code's that can't be a permission request or
/// its cancelling: most of the replay, and its biggest lines (whole
/// messages, tool results), so they aren't taken apart for nothing.
pub fn skipped(l: &[u8]) -> bool {
    l.starts_with(br#"{"type":""#)
        && !l.starts_with(br#"{"type":"agtop_"#)
        && !l.starts_with(br#"{"type":"control_"#)
}

/// Reads one host line: the host's own, or one of the native agent's
/// (Claude Code's control requests, read as rush's events).
pub fn decode(line: &[u8]) -> Vec<Said> {
    if line.starts_with(br#"{"type":""#) && !line.starts_with(br#"{"type":"agtop_"#) {
        return native(line).into_iter().map(Said::Event).collect();
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Head<'a> {
        #[serde(rename = "type", deserialize_with = "nullable")]
        typ: String,
        info: Option<Info>,
        #[serde(deserialize_with = "nullable")]
        request_id: String,
        #[serde(borrow)]
        ev: Option<&'a RawValue>,
    }
    let Ok(h) = serde_json::from_slice::<Head>(line) else {
        return vec![];
    };
    match h.typ.as_str() {
        "agtop_info" => vec![Said::Info(h.info.unwrap_or_default())],
        "agtop_answered" => vec![Said::Answered(h.request_id)],
        "agtop_ev" => {
            h.ev.and_then(|e| event::unmarshal(e.get().as_bytes()).ok())
                .map(Said::Event)
                .into_iter()
                .collect()
        }
        _ => vec![Said::Other],
    }
}

/// A line of the native agent's as rush's events: through its adapter's
/// NativeStream when the registry has one, else the two control lines of
/// Claude Code's that ask and take back (headless/event.go:296,
/// 570-610 decodeControl; neutral.go:56-73), which is all a wait is read
/// from.
fn native(line: &[u8]) -> Vec<Event> {
    let reg = rush_core::adapter::global();
    if let Some(a) = reg.native_agent()
        && let Some(ns) = a.native_stream()
    {
        let Ok(v) = ns.line(line) else {
            return vec![];
        };
        return ns.neutral().event(&*v).unwrap_or_default();
    }
    claude_control(line)
}

fn claude_control(line: &[u8]) -> Vec<Event> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Request<'a> {
        #[serde(deserialize_with = "nullable")]
        subtype: String,
        #[serde(deserialize_with = "nullable")]
        tool_name: String,
        #[serde(borrow)]
        input: Option<&'a RawValue>,
        #[serde(deserialize_with = "nullable")]
        description: String,
        #[serde(deserialize_with = "nullable")]
        decision_reason: String,
        #[serde(deserialize_with = "nullable")]
        tool_use_id: String,
        #[serde(deserialize_with = "nullable")]
        blocked_path: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Envelope<'a> {
        #[serde(rename = "type", deserialize_with = "nullable")]
        typ: String,
        #[serde(deserialize_with = "nullable")]
        request_id: String,
        #[serde(borrow)]
        request: Option<Request<'a>>,
    }
    let Ok(e) = serde_json::from_slice::<Envelope>(line) else {
        return vec![];
    };
    match e.typ.as_str() {
        "control_cancel_request" => vec![Event::ApprovalCancelled(event::ApprovalCancelled {
            id: e.request_id,
        })],
        "control_request" => {
            let Some(r) = e.request.filter(|r| r.subtype == "can_use_tool") else {
                return vec![];
            };
            if r.tool_name == "AskUserQuestion" {
                return vec![Event::Question(question(
                    &e.request_id,
                    &r.tool_use_id,
                    r.input,
                ))];
            }
            let reason = if r.decision_reason.is_empty() {
                r.description
            } else {
                r.decision_reason
            };
            vec![Event::Approval(Box::new(event::Approval {
                id: e.request_id,
                reason,
                path: r.blocked_path,
                call: rush_core::tool::Call {
                    id: r.tool_use_id,
                    name: r.tool_name,
                    ..Default::default()
                },
                ..Default::default()
            }))]
        }
        _ => vec![],
    }
}

/// An AskUserQuestion request as the question it asks (neutral.go:204).
fn question(id: &str, call: &str, input: Option<&RawValue>) -> Question {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Opt {
        #[serde(deserialize_with = "nullable")]
        label: String,
        #[serde(deserialize_with = "nullable")]
        description: String,
        #[serde(deserialize_with = "nullable")]
        preview: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Q {
        #[serde(deserialize_with = "nullable")]
        question: String,
        #[serde(deserialize_with = "nullable")]
        header: String,
        #[serde(rename = "multiSelect")]
        multi: bool,
        options: Option<Vec<Opt>>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct In {
        #[serde(deserialize_with = "nullable")]
        title: String,
        questions: Option<Vec<Q>>,
    }
    let i: In = input
        .and_then(|r| serde_json::from_str(r.get()).ok())
        .unwrap_or_default();
    Question {
        id: id.to_string(),
        call_id: call.to_string(),
        title: i.title,
        asks: i
            .questions
            .unwrap_or_default()
            .into_iter()
            .map(|q| event::Ask {
                header: q.header,
                text: q.question,
                multi: q.multi,
                options: q
                    .options
                    .unwrap_or_default()
                    .into_iter()
                    .map(|o| event::Choice {
                        label: o.label,
                        description: o.description,
                        preview: o.preview,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
            .collect(),
    }
}

/// A string Claude Code may write as null, read as empty (Go's decoder
/// leaves a string empty for a null).
fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    rush_core::wire::string::deserialize(d)
}

/// What a session waits on you for: leave for a call, or the answers to a
/// question.
#[derive(Clone, Debug, PartialEq)]
pub struct Asking {
    pub id: String,
    /// Why it asks, when it says.
    pub reason: String,
    /// Set when it's a question.
    pub q: Option<Question>,
}

/// What a replay asked and hasn't yet settled, in order.
#[derive(Default)]
pub struct Waits {
    open: HashMap<String, Asking>,
    order: Vec<String>,
}

impl Waits {
    pub fn take(&mut self, s: Said) {
        match s {
            Said::Event(Event::Approval(a)) => {
                self.order.push(a.id.clone());
                self.open.insert(
                    a.id.clone(),
                    Asking {
                        id: a.id,
                        reason: a.reason,
                        q: None,
                    },
                );
            }
            Said::Event(Event::Question(q)) => {
                self.order.push(q.id.clone());
                self.open.insert(
                    q.id.clone(),
                    Asking {
                        id: q.id.clone(),
                        reason: String::new(),
                        q: Some(q),
                    },
                );
            }
            Said::Event(Event::ApprovalCancelled(c)) => {
                self.open.remove(&c.id);
            }
            Said::Answered(id) => {
                self.open.remove(&id);
            }
            _ => {}
        }
    }

    /// The latest still waiting.
    pub fn last(&self) -> Option<Asking> {
        self.order
            .iter()
            .rev()
            .find_map(|id| self.open.get(id))
            .cloned()
    }
}

/// What a session waits on, as its host's replay on c says, and its info;
/// nothing when the host doesn't say within two seconds.
pub fn read_wait(c: &Client) -> (Option<Asking>, Info) {
    let until = Instant::now() + WAIT_ON;
    let mut w = Waits::default();
    loop {
        let left = until.saturating_duration_since(Instant::now());
        let l = match c.lines.recv_timeout(left) {
            Ok(l) => l,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                return (None, Info::default());
            }
        };
        if skipped(&l) {
            continue;
        }
        for s in decode(&l) {
            if let Said::Info(info) = s {
                return (w.last(), info);
            }
            w.take(s);
        }
    }
}

/// The input that answers q through Client::allow: the questions as
/// Claude Code's AskUserQuestion takes them, with the answers keyed by
/// question text and the preview of each chosen option beside it.
/// Questions left unanswered go without one (host/answer.go:16).
pub fn answer_input(q: &Question, answers: &[(String, String)]) -> Box<RawValue> {
    use serde_json::{Map, Value, json};
    let answer = |text: &str| {
        answers
            .iter()
            .find(|(k, _)| k == text)
            .map(|(_, v)| v.as_str())
    };
    let mut notes = Map::new();
    let mut questions = Vec::new();
    for a in &q.asks {
        let mut opts = Vec::new();
        for o in &a.options {
            let mut opt = Map::new();
            opt.insert("label".into(), json!(o.label));
            opt.insert("description".into(), json!(o.description));
            if !o.preview.is_empty() {
                opt.insert("preview".into(), json!(o.preview));
                if answer(&a.text) == Some(o.label.as_str()) {
                    notes.insert(a.text.clone(), json!({ "preview": o.preview }));
                }
            }
            opts.push(Value::Object(opt));
        }
        questions.push(json!({
            "question": a.text,
            "header": a.header,
            "multiSelect": a.multi,
            "options": opts,
        }));
    }
    let mut m = Map::new();
    if !q.title.is_empty() {
        m.insert("title".into(), json!(q.title));
    }
    m.insert("questions".into(), Value::Array(questions));
    let ans: Map<String, Value> = answers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    m.insert("answers".into(), Value::Object(ans));
    if !notes.is_empty() {
        m.insert("annotations".into(), Value::Object(notes));
    }
    RawValue::from_string(Value::Object(m).to_string()).expect("JSON from a Value")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reading what a host waits on only takes apart the lines that can
    /// say so (skip_test.go TestSkipped).
    #[test]
    fn skips() {
        for (line, want) in [
            (r#"{"type":"assistant","message":{}}"#, true),
            (r#"{"type":"user","message":{}}"#, true),
            (r#"{"type":"stream_event","event":{}}"#, true),
            (
                r#"{"type":"control_request","request_id":"r1","request":{}}"#,
                false,
            ),
            (
                r#"{"type":"control_cancel_request","request_id":"r1"}"#,
                false,
            ),
            (r#"{"request_id":"r1","type":"agtop_answered"}"#, false),
            (r#"{"info":{"state":"blocked"},"type":"agtop_info"}"#, false),
            (
                r#"{"agtop_sent":true,"message":{"content":"hi"},"type":"user"}"#,
                false,
            ),
        ] {
            assert_eq!(skipped(line.as_bytes()), want, "{line}");
        }
    }

    #[test]
    fn decodes_what_asks_and_settles() {
        let ask = r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo hi"},"description":"Say hi","decision_reason":"This command requires approval","tool_use_id":"toolu_1"}}"#;
        let Said::Event(Event::Approval(a)) = &decode(ask.as_bytes())[0] else {
            panic!("not an approval");
        };
        assert_eq!(a.id, "req-1");
        assert_eq!(a.reason, "This command requires approval");
        let cancel = r#"{"type":"control_cancel_request","request_id":"req-1"}"#;
        assert_eq!(
            decode(cancel.as_bytes()),
            vec![Said::Event(Event::ApprovalCancelled(
                event::ApprovalCancelled { id: "req-1".into() }
            ))]
        );
        assert_eq!(
            decode(br#"{"type":"agtop_answered","request_id":"r"}"#),
            vec![Said::Answered("r".into())]
        );
        let info =
            br#"{"type":"agtop_info","info":{"id":"h2","state":"blocked","limit":{"ask":true}}}"#;
        let Said::Info(i) = &decode(info)[0] else {
            panic!("not info");
        };
        assert!(i.limit.as_ref().is_some_and(|l| l.ask));
        // Another agent's events come as rush's own.
        let ev = event::marshal(&Event::Question(Question {
            id: "q1".into(),
            ..Question::default()
        }))
        .unwrap();
        let line = format!(r#"{{"type":"agtop_ev","ev":{ev}}}"#);
        let Said::Event(Event::Question(q)) = &decode(line.as_bytes())[0] else {
            panic!("not a question");
        };
        assert_eq!(q.id, "q1");
    }

    /// Fields Claude Code writes as null read as empty, as Go's decoder
    /// reads them, rather than losing the request.
    #[test]
    fn nulls_read_as_empty() {
        let ask = br#"{"type":"control_request","request_id":"req-n","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{},"description":null,"decision_reason":null,"blocked_path":null,"tool_use_id":"t"}}"#;
        let Said::Event(Event::Approval(a)) = &decode(ask)[0] else {
            panic!("not an approval");
        };
        assert_eq!((a.id.as_str(), a.reason.as_str()), ("req-n", ""));
        let q = br#"{"type":"control_request","request_id":"req-q","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"title":null,"questions":[{"question":"Which?","header":null,"options":[{"label":"A","description":null}]}]},"tool_use_id":"t"}}"#;
        let Said::Event(Event::Question(q)) = &decode(q)[0] else {
            panic!("not a question");
        };
        assert_eq!(q.asks[0].options[0].label, "A");
    }

    #[test]
    fn the_last_still_open_is_waited_on() {
        let mut w = Waits::default();
        let ap = |id: &str| {
            Said::Event(Event::Approval(Box::new(event::Approval {
                id: id.into(),
                ..Default::default()
            })))
        };
        w.take(ap("a"));
        w.take(ap("b"));
        assert_eq!(w.last().unwrap().id, "b");
        w.take(Said::Answered("b".into()));
        assert_eq!(w.last().unwrap().id, "a");
        w.take(Said::Event(Event::ApprovalCancelled(
            event::ApprovalCancelled { id: "a".into() },
        )));
        assert!(w.last().is_none());
    }

    #[test]
    fn answers_keep_the_chosen_preview() {
        let q = Question {
            asks: vec![event::Ask {
                text: "Which?".into(),
                header: "Pick".into(),
                options: vec![
                    event::Choice {
                        label: "A".into(),
                        description: "a".into(),
                        ..Default::default()
                    },
                    event::Choice {
                        label: "B".into(),
                        description: "b".into(),
                        preview: "file.db".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let got = answer_input(&q, &[("Which?".into(), "B".into())]);
        let v: serde_json::Value = serde_json::from_str(got.get()).unwrap();
        assert_eq!(v["answers"]["Which?"], "B");
        assert_eq!(v["annotations"]["Which?"]["preview"], "file.db");
        assert_eq!(v["questions"][0]["options"][1]["preview"], "file.db");
        assert!(v["questions"][0]["options"][0].get("preview").is_none());
        assert!(v.get("title").is_none());
        let got = answer_input(&q, &[("Which?".into(), "A".into())]);
        let v: serde_json::Value = serde_json::from_str(got.get()).unwrap();
        assert!(v.get("annotations").is_none());
    }
}
