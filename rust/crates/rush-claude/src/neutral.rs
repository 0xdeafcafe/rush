//! Claude Code's messages as rush's own events: what its transcripts keep
//! read as a live session sends them, so the conversation reads one way
//! whichever agent ran it. Port of internal/adapters/claude/headless/
//! neutral.go:149-188 (message, tokens) and native.go (Message, Neutral).

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rush_core::adapter::MessageDecoder;
use rush_core::event::{self, Event, ImageData, Part, PartKind};
use rush_core::tool::{Call, Kind};
use rush_core::usage::TokenUsage;
use serde_json::value::RawValue;

use crate::decode::decode_message;
use crate::message::{BlockKind, Image, Message, Role, Usage};
use crate::tools;

/// Reads one session's messages, in order, as rush's: a tool result is read
/// knowing what its call was.
#[derive(Debug, Default)]
pub struct Neutral {
    /// What each call still waiting for its result does, by its id.
    pub(crate) calls: HashMap<String, Kind>,
}

impl Neutral {
    pub fn new() -> Neutral {
        Neutral::default()
    }

    /// One message as rush's. Claude Code's own user messages of text are
    /// never what you said (the host tells of that itself, and transcripts
    /// apart from these), so they're injected.
    pub(crate) fn message(&mut self, m: Message) -> event::Message {
        let mut out = event::Message {
            role: match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            }
            .into(),
            id: m.id,
            model: m.model,
            parent: m.parent_tool_use_id,
            parts: Vec::with_capacity(m.blocks.len()),
            tokens: m.usage.as_ref().map(tokens),
            injected: m.role == Role::User,
        };
        for b in m.blocks {
            match b.kind {
                BlockKind::Text => out.parts.push(Part {
                    kind: PartKind::Text,
                    text: b.text,
                    ..Part::default()
                }),
                BlockKind::Thinking => out.parts.push(Part {
                    kind: PartKind::Thinking,
                    text: b.text,
                    ..Part::default()
                }),
                BlockKind::ToolUse => {
                    let c = tools::call(&b.id, &b.name, b.input.as_deref());
                    self.calls.insert(b.id, c.kind);
                    out.parts.push(Part {
                        kind: PartKind::ToolCall,
                        call: Some(Box::new(c)),
                        ..Part::default()
                    });
                }
                BlockKind::ToolResult => {
                    // A call answers once: what it was isn't needed again.
                    let c = Call {
                        kind: self.calls.remove(&b.tool_use_id).unwrap_or_default(),
                        id: b.tool_use_id,
                        ..Call::default()
                    };
                    let o = tools::output(&c, &b.text, b.is_error, m.tool_result.as_deref());
                    out.parts.push(Part {
                        kind: PartKind::ToolResult,
                        output: Some(Box::new(o)),
                        ..Part::default()
                    });
                    // The images it gave back follow it.
                    for img in b.images {
                        out.parts.push(Part {
                            kind: PartKind::Image,
                            image: Some(Box::new(image(img))),
                            ..Part::default()
                        });
                    }
                }
            }
        }
        out
    }
}

impl MessageDecoder for Neutral {
    fn message(&mut self, typ: &str, message: &RawValue, result: Option<&RawValue>) -> Vec<Event> {
        let Some(mut m) = decode_message(message.get()) else {
            return Vec::new();
        };
        m.role = if typ == "user" {
            Role::User
        } else {
            Role::Assistant
        };
        m.tool_result = result.map(ToOwned::to_owned);
        vec![Event::Message(self.message(m))]
    }
}

/// What reads Claude Code's transcripts' messages: a fresh Neutral, for
/// rush_convo::Tail.
pub fn messages() -> Box<dyn MessageDecoder> {
    Box::new(Neutral::new())
}

/// An image a message carried, its base64 read.
fn image(i: Image) -> ImageData {
    ImageData {
        media_type: i.media_type,
        data: STANDARD.decode(i.data.as_bytes()).unwrap_or_default(),
        path: String::new(),
    }
}

/// A request's usage as rush counts it.
fn tokens(u: &Usage) -> TokenUsage {
    TokenUsage {
        input: u.input,
        output: u.output,
        cache_read: u.cache_read,
        cache_write_5m: u.cache_write_5m,
        cache_write_1h: u.cache_write_1h,
        reasoning: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(s: &str) -> Box<RawValue> {
        RawValue::from_string(s.to_owned()).unwrap()
    }

    fn one(n: &mut Neutral, typ: &str, msg: &str, result: Option<&str>) -> event::Message {
        let r = result.map(raw);
        let mut evs = MessageDecoder::message(n, typ, &raw(msg), r.as_deref());
        assert_eq!(evs.len(), 1);
        match evs.remove(0) {
            Event::Message(m) => m,
            e => panic!("not a message: {e:?}"),
        }
    }

    #[test]
    fn calls_and_their_results() {
        let mut n = Neutral::new();
        let m = one(
            &mut n,
            "assistant",
            r#"{"id":"msg_1","model":"claude-opus-5-5","content":[{"type":"text","text":"On it."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go vet ./..."}}],"usage":{"input_tokens":3,"output_tokens":4,"cache_creation_input_tokens":7}}"#,
            None,
        );
        assert_eq!((m.role.as_str(), m.id.as_str()), ("assistant", "msg_1"));
        assert!(!m.injected);
        assert_eq!(m.parts[0].text, "On it.");
        let c = m.parts[1].call.as_ref().unwrap();
        assert_eq!(
            (c.kind, c.input.command.as_str()),
            (Kind::Shell, "go vet ./...")
        );
        assert_eq!(
            c.raw.as_ref().unwrap().get(),
            r#"{"command":"go vet ./..."}"#
        );
        let t = m.tokens.unwrap();
        assert_eq!((t.input, t.output, t.cache_write_1h), (3, 4, 7));

        let m = one(
            &mut n,
            "user",
            r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"Exit code 1\nvet: bad","is_error":true},{"type":"text","text":"note"}]}"#,
            Some(r#"{"stdout":"","stderr":"vet: bad"}"#),
        );
        assert!(m.injected);
        let o = m.parts[0].output.as_ref().unwrap();
        // Read as the shell call it answers.
        assert_eq!(
            (o.call_id.as_str(), o.exit, o.stderr.as_str()),
            ("t1", Some(1), "vet: bad")
        );
        assert_eq!(m.parts[1].kind, PartKind::Text);
        assert!(n.calls.is_empty());
    }

    #[test]
    fn images_follow_their_result() {
        let mut n = Neutral::new();
        let m = one(
            &mut n,
            "user",
            r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content":[{"type":"text","text":"shot"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"cG5nIQ=="}}]}]}"#,
            None,
        );
        assert_eq!(m.parts.len(), 2);
        assert_eq!(m.parts[0].output.as_ref().unwrap().text, "shot");
        let img = m.parts[1].image.as_ref().unwrap();
        assert_eq!(
            (img.media_type.as_str(), &img.data[..]),
            ("image/png", &b"png!"[..])
        );
    }

    #[test]
    fn not_a_message() {
        let mut n = Neutral::new();
        assert!(MessageDecoder::message(&mut n, "user", &raw("5"), None).is_empty());
    }
}
