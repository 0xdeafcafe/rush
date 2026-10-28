//! Claude's message JSON (a transcript line's `message`, or a headless
//! stream event's) into rush_core's Message. Port of
//! internal/adapters/claude/headless/event.go DecodeMessage.

use rush_core::message::Message;

/// Decodes one `message` object; `None` when it isn't one.
pub fn decode_message(_raw: &str) -> Option<Message> {
    None // TODO(port)
}
