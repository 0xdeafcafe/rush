//! The lines a host sends its clients, a line of JSON each, and reading
//! them back. Claude Code's own lines go as it wrote them, to a client
//! that doesn't read events; the host's own are agtop_* lines (it kept the
//! name it had as agtop: sessions' logs hold them), written with sorted
//! keys as Go writes a map, and rush's own events go as agtop_ev lines.
//!
//! Ported from internal/host/client.go:309-420 (InfoEvent … Decode),
//! host.go:396-406 (the line types), host.go:1314-1317 (the info line),
//! agent.go:32-35 and :503-509 (typeEvent, wireCommand), agent.go:654-675
//! (eventLine, isEventLine) and decoder.go (Decoder, nativeLine).

use std::any::Any;

use rush_core::adapter::{Neutral, global};
use rush_core::event::{self, Command, Event, Exchange};
use rush_core::usage;
use rush_core::wire::RawJson;
use rush_state::Time;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::wire::Info;

pub const TYPE_INFO: &str = "agtop_info";
pub const TYPE_ANSWERED: &str = "agtop_answered";
pub const TYPE_COMMANDS: &str = "agtop_commands";
pub const TYPE_CONTEXT: &str = "agtop_context";
pub const TYPE_REPLY: &str = "agtop_reply";
pub const TYPE_TIME: &str = "agtop_time";
pub const TYPE_ERROR: &str = "agtop_error";
/// A line carrying one of rush's own events: every agent's but Claude
/// Code's, whose own lines go to clients that don't read events.
pub const TYPE_EVENT: &str = "agtop_ev";

/// What one line from a host says.
pub enum Line {
    /// The session's info, sent on connect and whenever it changes.
    Info(Box<Info>),
    /// A permission request was settled, here or by another client.
    Answered(String),
    /// A command the host could not carry out.
    Error(String),
    /// A message a client sent, echoed so every client shows it.
    Sent(Sent),
    /// When the output that follows it happened.
    Stamp(Time),
    /// The session's slash commands.
    Commands(Vec<Command>),
    /// The answer to a control request a client passed through with ask:
    /// id is the client's own.
    Reply {
        id: String,
        body: Option<RawJson>,
        error: String,
    },
    /// What fills the context window, as last counted.
    Context(Box<usage::Context>),
    /// One of rush's own events.
    Event(Event),
    /// A line the native agent wrote, as its own event (NativeStream).
    Native(Box<dyn Any + Send>),
}

impl std::fmt::Debug for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Line::Info(i) => write!(f, "Info({} {})", i.id, i.state),
            Line::Answered(id) => write!(f, "Answered({id})"),
            Line::Error(e) => write!(f, "Error({e})"),
            Line::Sent(s) => write!(f, "Sent({:?})", s.text),
            Line::Stamp(t) => write!(f, "Stamp({t})"),
            Line::Commands(c) => write!(f, "Commands({})", c.len()),
            Line::Reply { id, .. } => write!(f, "Reply({id})"),
            Line::Context(_) => write!(f, "Context"),
            Line::Event(e) => write!(f, "Event({:?})", e.name()),
            Line::Native(_) => write!(f, "Native"),
        }
    }
}

/// A message a client sent, echoed so every client shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sent {
    /// None for your own input; another agent's carries where it came from.
    pub exchange: Option<Box<Exchange>>,
    pub text: String,
    /// The paths of attached images.
    pub images: Vec<String>,
}

/// What reading a line fails with.
#[derive(Debug)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

/// Whether l is a line as Claude Code wrote it, rather than the host, told
/// by how it starts: most are, deltas above all (hello.go:58).
pub fn is_claude_line(l: &[u8]) -> bool {
    l.starts_with(br#"{"type":""#) && !l.starts_with(br#"{"type":"agtop_"#)
}

/// A line the native agent wrote, as its own event.
pub fn native_line(line: &[u8]) -> Result<Box<dyn Any + Send>, DecodeError> {
    let Some(n) = global().native_agent() else {
        return Err(DecodeError("host: no agent reads this line".into()));
    };
    let Some(s) = n.native_stream() else {
        return Err(DecodeError("host: no agent reads this line".into()));
    };
    s.line(line).map_err(|e| DecodeError(e.to_string()))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Head<'a> {
    #[serde(rename = "type")]
    typ: String,
    #[serde(borrow)]
    info: Option<&'a RawValue>,
    request_id: String,
    error: String,
    agtop_sent: bool,
    agtop_exchange: Option<Box<Exchange>>,
    #[serde(deserialize_with = "rush_core::wire::nullvec::deserialize")]
    agtop_images: Vec<String>,
    #[serde(borrow)]
    message: Option<&'a RawValue>,
    #[serde(deserialize_with = "rush_core::wire::nullvec::deserialize")]
    commands: Vec<WireCommand>,
    #[serde(borrow)]
    context: Option<&'a RawValue>,
    id: String,
    reply: Option<RawJson>,
    t: i64,
    #[serde(borrow)]
    ev: Option<&'a RawValue>,
}

/// Reads one line from a host: its own lines, Claude Code's, or rush's own
/// events from another agent's session.
pub fn decode(line: &[u8]) -> Result<Line, DecodeError> {
    // Claude Code's lines start with their type; the host's own (and its
    // echo of what you sent) are written with sorted keys and don't. So
    // most lines, deltas above all, are only taken apart once.
    if is_claude_line(line) {
        return native_line(line).map(Line::Native);
    }
    let h: Head = serde_json::from_slice(line).map_err(|e| DecodeError(e.to_string()))?;
    let err = |e: serde_json::Error| DecodeError(e.to_string());
    match h.typ.as_str() {
        TYPE_INFO => {
            let mut i: Info = match h.info {
                Some(r) => serde_json::from_str(r.get()).map_err(err)?,
                None => Info::default(),
            };
            i.migrate();
            return Ok(Line::Info(Box::new(i)));
        }
        TYPE_ANSWERED => return Ok(Line::Answered(h.request_id)),
        TYPE_ERROR => return Ok(Line::Error(h.error)),
        TYPE_COMMANDS => {
            return Ok(Line::Commands(
                h.commands.into_iter().map(Command::from).collect(),
            ));
        }
        TYPE_REPLY => {
            let body = h.reply.filter(|r| r.get() != "null");
            return Ok(Line::Reply {
                id: h.id,
                body,
                error: h.error,
            });
        }
        TYPE_CONTEXT => {
            let Some(c) = h.context.filter(|c| c.get() != "null") else {
                return Err(DecodeError("context line without a count".into()));
            };
            let c: usage::Context = serde_json::from_str(c.get()).map_err(err)?;
            return Ok(Line::Context(Box::new(c)));
        }
        TYPE_TIME => return Ok(Line::Stamp(Time::from_ms(h.t))),
        TYPE_EVENT => {
            let ev = h.ev.map_or("null", RawValue::get);
            return event::unmarshal(ev.as_bytes())
                .map(Line::Event)
                .map_err(|e| DecodeError(e.to_string()));
        }
        _ => {}
    }
    if h.agtop_sent {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct M {
            content: String,
        }
        let m: M = h
            .message
            .and_then(|m| serde_json::from_str(m.get()).ok())
            .unwrap_or_default();
        return Ok(Line::Sent(Sent {
            exchange: h.agtop_exchange,
            text: m.content,
            images: h.agtop_images,
        }));
    }
    native_line(line).map(Line::Native)
}

/// Reads one host's lines, in order, as decode does, except that the
/// native agent's own lines come out as rush's own events: a host from
/// before rush's own events, or a client that didn't say hello, has Claude
/// Code's lines, and this reads them as any agent's. One Decoder follows
/// one connection (decoder.go).
#[derive(Default)]
pub struct Decoder {
    n: Option<Box<dyn Neutral>>,
}

impl Decoder {
    /// What line says: none for the traffic only the host needs, and more
    /// than one where Claude Code says two things at once.
    pub fn decode(&mut self, line: &[u8]) -> Result<Vec<Line>, DecodeError> {
        let l = decode(line)?;
        let Line::Native(ev) = l else {
            return Ok(vec![l]);
        };
        match neutral(&mut self.n).and_then(|n| n.event(&*ev)) {
            Some(evs) => Ok(evs.into_iter().map(Line::Event).collect()),
            None => Ok(vec![Line::Native(ev)]),
        }
    }
}

/// *n, started on first use: the native agent's reader of one session's
/// own events as rush's. With no native agent it reads none.
pub(crate) fn neutral(n: &mut Option<Box<dyn Neutral>>) -> Option<&mut Box<dyn Neutral>> {
    if n.is_none() {
        *n = global()
            .native_agent()
            .and_then(|a| a.native_stream().map(|s| s.neutral()));
    }
    n.as_mut()
}

// ---- The host's own lines ----

/// The line clients get for info: it wraps info.json's bytes, marshalled
/// once (host.go:1314).
pub fn info_line(info_json: &[u8]) -> Vec<u8> {
    let mut l = Vec::with_capacity(info_json.len() + 32);
    l.extend_from_slice(br#"{"info":"#);
    l.extend_from_slice(info_json);
    l.extend_from_slice(br#","type":"agtop_info"}"#);
    l
}

/// A permission request settled.
pub fn answered_line(id: &str) -> Vec<u8> {
    #[derive(Serialize)]
    struct L<'a> {
        request_id: &'a str,
        #[serde(rename = "type")]
        typ: &'a str,
    }
    crate::json::line(&L {
        request_id: id,
        typ: TYPE_ANSWERED,
    })
}

/// An op the host could not carry out.
pub fn error_line(err: &str) -> Vec<u8> {
    #[derive(Serialize)]
    struct L<'a> {
        error: &'a str,
        #[serde(rename = "type")]
        typ: &'a str,
    }
    crate::json::line(&L {
        error: err,
        typ: TYPE_ERROR,
    })
}

/// A time mark, Unix milliseconds.
pub fn time_line(ms: i64) -> Vec<u8> {
    format!(r#"{{"t":{ms},"type":"agtop_time"}}"#).into_bytes()
}

/// The host's echo of a message sent: the start of a turn.
pub fn echo_line(text: &str, images: &[String], exchange: Option<&Exchange>) -> Vec<u8> {
    #[derive(Serialize)]
    struct Msg<'a> {
        content: &'a str,
        role: &'a str,
    }
    #[derive(Serialize)]
    struct L<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        agtop_exchange: Option<&'a Exchange>,
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        agtop_images: &'a [String],
        agtop_sent: bool,
        message: Msg<'a>,
        #[serde(rename = "type")]
        typ: &'a str,
    }
    crate::json::line(&L {
        agtop_exchange: exchange,
        agtop_images: images,
        agtop_sent: true,
        message: Msg {
            content: text,
            role: "user",
        },
        typ: "user",
    })
}

/// A slash command as the commands line has always had it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WireCommand {
    pub name: String,
    pub description: String,
    #[serde(rename = "argumentHint")]
    pub argument_hint: String,
    #[serde(with = "rush_core::wire::nullvec")]
    pub aliases: Vec<String>,
}

impl From<WireCommand> for Command {
    fn from(c: WireCommand) -> Command {
        Command {
            name: c.name,
            description: c.description,
            argument_hint: c.argument_hint,
            aliases: c.aliases,
        }
    }
}

/// The session's slash commands.
pub fn commands_line(list: &[Command]) -> Vec<u8> {
    #[derive(Serialize)]
    struct L<'a> {
        commands: Vec<WireCommand>,
        #[serde(rename = "type")]
        typ: &'a str,
    }
    let commands = list
        .iter()
        .map(|c| WireCommand {
            name: c.name.clone(),
            description: c.description.clone(),
            argument_hint: c.argument_hint.clone(),
            aliases: c.aliases.clone(),
        })
        .collect();
    crate::json::line(&L {
        commands,
        typ: TYPE_COMMANDS,
    })
}

/// A control request's reply, tagged with the client's id for it.
pub fn reply_line(id: &str, body: Option<&RawJson>, err: &str) -> Vec<u8> {
    #[derive(Serialize)]
    struct L<'a> {
        error: &'a str,
        id: &'a str,
        reply: Option<&'a RawJson>,
        #[serde(rename = "type")]
        typ: &'a str,
    }
    crate::json::line(&L {
        error: err,
        id,
        reply: body,
        typ: TYPE_REPLY,
    })
}

/// What fills the context window.
pub fn context_line(c: &usage::Context) -> Vec<u8> {
    #[derive(Serialize)]
    struct L<'a> {
        context: &'a usage::Context,
        #[serde(rename = "type")]
        typ: &'a str,
    }
    crate::json::line(&L {
        context: c,
        typ: TYPE_CONTEXT,
    })
}

/// ev as a line to clients.
pub fn event_line(ev: &Event) -> Result<Vec<u8>, event::CodecError> {
    let b = event::marshal(ev)?;
    let mut l = Vec::with_capacity(b.len() + 24);
    l.extend_from_slice(br#"{"type":"agtop_ev","ev":"#);
    l.extend_from_slice(b.as_bytes());
    l.push(b'}');
    Ok(l)
}

/// Whether l is a line event_line wrote about one of kinds.
pub fn is_event_line(l: &[u8], kinds: &[&str]) -> bool {
    let Some(rest) = l.strip_prefix(br#"{"type":"agtop_ev","ev":{"t":""#.as_slice()) else {
        return false;
    };
    kinds
        .iter()
        .any(|k| rest.starts_with(k.as_bytes()) && rest.get(k.len()) == Some(&b'"'))
}

/// The host's echo of a message sent: the start of a turn.
pub fn is_echo(line: &[u8]) -> bool {
    line.starts_with(br#"{"agtop_"#) && memfind(line, br#""agtop_sent":true"#)
}

pub(crate) fn is_stream_event(l: &[u8]) -> bool {
    l.starts_with(br#"{"type":"stream_event""#)
        || is_event_line(l, &["delta", "part_start", "message_start"])
}

pub(crate) fn is_whole_message(l: &[u8]) -> bool {
    l.starts_with(br#"{"type":"assistant""#)
        || l.starts_with(br#"{"type":"user""#)
        || is_event_line(l, &["message"])
}

pub(crate) fn memfind(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
