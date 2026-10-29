//! The host's half of Claude Code as rush's Native agent: its stream-json
//! lines and its transcripts' messages read as its own events, and those
//! as rush's. Port of internal/adapters/claude/native.go:16-45 (Line,
//! Message, Neutral) and rewind.go (RewindFiles, Recap).

use std::any::Any;

use rush_core::adapter::{self, FileRewind, NativeStream, Profile, Rewinder};
use rush_core::event::Event;
use rush_core::time::now;
use serde_json::value::RawValue;

use crate::adapter::{Claude, account};
use crate::headless::{self, Event as HEvent};
use crate::neutral::Neutral;

impl NativeStream for Claude {
    fn line(&self, line: &[u8]) -> adapter::Result<Box<dyn Any + Send>> {
        Ok(Box::new(headless::decode(line)?))
    }

    fn message(
        &self,
        typ: &str,
        message: &RawValue,
        result: Option<&RawValue>,
    ) -> adapter::Result<Box<dyn Any + Send>> {
        let m = headless::event::decode_message_parts(typ, Some(message), result)
            .ok_or_else(|| adapter::Error::from("not a message"))?;
        Ok(Box::new(HEvent::Message(m)))
    }

    fn neutral(&self) -> Box<dyn adapter::Neutral> {
        Box::new(LiveNeutral(Neutral::new()))
    }
}

/// Reads one session's headless events as rush's.
struct LiveNeutral(Neutral);

impl adapter::Neutral for LiveNeutral {
    fn event(&mut self, ev: &dyn Any) -> Option<Vec<Event>> {
        let h = ev.downcast_ref::<HEvent>()?;
        Some(self.0.event(h.clone(), now()))
    }
}

fn rewind_opts(p: &Profile, cwd: &str, sid: &str, model: &str) -> headless::Options {
    headless::Options {
        account: account(p),
        dir: cwd.into(),
        resume: sid.into(),
        model: model.into(),
        ..headless::Options::default()
    }
}

impl Rewinder for Claude {
    /// Claude Code's own rewind of a conversation's files.
    fn rewind_files(
        &self,
        p: &Profile,
        cwd: &str,
        sid: &str,
        model: &str,
        msg_id: &str,
        dry_run: bool,
    ) -> adapter::Result<FileRewind> {
        headless::rewind_files(&rewind_opts(p, cwd, sid, model), msg_id, dry_run)
    }

    /// Asks a fork of the conversation what it learned since from.
    fn recap(
        &self,
        p: &Profile,
        cwd: &str,
        sid: &str,
        model: &str,
        from: &str,
    ) -> adapter::Result<String> {
        headless::recap(&rewind_opts(p, cwd, sid, model), from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::event::tests::{ASK, TOOL_RESULT, TOOL_USE};
    use rush_core::adapter::Adapter as _;

    /// What a host an older rush started sends, as written, reads as a
    /// live session's events, a tool's result by its call.
    #[test]
    fn lines_read_as_rushs_events() {
        let c = Claude::new();
        let ns = c.native_stream().unwrap();
        let mut n = ns.neutral();
        let mut kinds = Vec::new();
        for l in [TOOL_USE, ASK, TOOL_RESULT] {
            let ev = ns.line(l.as_bytes()).unwrap();
            for e in n.event(&*ev).unwrap() {
                kinds.push(match e {
                    Event::Message(m) if m.role == "assistant" => "call".to_string(),
                    Event::Message(m) => {
                        let o = m.parts[0].output.as_ref().unwrap();
                        format!("result:{:?}", o.exit)
                    }
                    Event::Approval(_) => "approval".into(),
                    e => format!("{e:?}"),
                });
            }
        }
        assert_eq!(kinds, ["call", "approval", "result:Some(0)"]);
        assert!(n.event(&5u8).is_none(), "not one of Claude's events");
        let m: Box<RawValue> =
            RawValue::from_string(r#"{"role":"user","content":"hi"}"#.into()).unwrap();
        let ev = ns.message("user", &m, None).unwrap();
        let Some(HEvent::Message(m)) = ev.downcast_ref::<HEvent>() else {
            panic!()
        };
        assert_eq!(m.blocks[0].text, "hi");
        assert!(ns.line(b"nope").is_err());
    }
}
