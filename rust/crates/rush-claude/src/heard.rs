//! Whether Claude Code took in a message sent to it: it writes each to its
//! transcript as it queues it (with no content when it has images) and
//! again as the turn's user message. Port of
//! internal/adapters/claude/heard.go.

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::json::{Str, ms};

/// How much of a transcript's end is read.
const HEARD_TAIL: u64 = 4 << 20;

/// The longest line read; a longer one leaves it unknown.
const MAX_LINE: u64 = 32 << 20;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(borrow)]
    operation: Str<'a>,
    #[serde(borrow)]
    timestamp: Str<'a>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

/// Whether the transcript at path took in a message of text sent at
/// since; None when the transcript or the text can't say.
pub(crate) fn heard_in(path: &Path, text: &str, since: Ms) -> Option<bool> {
    let snip = heard_snippet(text)?;
    let mut f = std::fs::File::open(path).ok()?;
    if f.metadata().is_ok_and(|m| m.len() > HEARD_TAIL) {
        let _ = f.seek(SeekFrom::End(-(HEARD_TAIL as i64)));
    }
    let mut r = BufReader::with_capacity(64 << 10, f);
    let from = since - 2000; // its clock and ours, a write apart
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match std::io::Read::take(&mut r, MAX_LINE + 1).read_until(b'\n', &mut buf) {
            Ok(0) => return Some(false),
            Ok(_) => {}
            Err(_) => return None,
        }
        if buf.last() != Some(&b'\n') && buf.len() as u64 > MAX_LINE {
            return None;
        }
        let l = buf.strip_suffix(b"\n").unwrap_or(&buf);
        let Ok(line) = serde_json::from_slice::<Line>(l) else {
            continue;
        };
        let at = ms(&line.timestamp);
        if at == 0 || at < from {
            continue;
        }
        let queued_images = &*line.typ == "queue-operation"
            && &*line.operation == "enqueue"
            && line.content.is_none();
        if queued_images || memchr::memmem::find(l, snip.as_bytes()).is_some() {
            return Some(true);
        }
    }
}

/// The end of text's last line as the transcript writes it, JSON-escaped:
/// the part of a message least like anything else.
fn heard_snippet(text: &str) -> Option<String> {
    let text = strip_markers(text);
    let mut text = text.trim();
    if let Some(i) = text.rfind('\n') {
        text = text[i + 1..].trim();
    }
    let n = text.chars().count();
    if n > 32 {
        let skip = text.char_indices().nth(n - 32).map_or(0, |(i, _)| i);
        text = &text[skip..];
    }
    if text.is_empty()
        || text
            .chars()
            .any(|r| r < '\u{20}' || r == '\u{2028}' || r == '\u{2029}')
    {
        return None;
    }
    Some(text.replace('\\', r"\\").replace('"', r#"\""#))
}

/// text without its images' places: [Image #2].
fn strip_markers(text: &str) -> String {
    const OPEN: &str = "[Image #";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        let digits = &rest[i + OPEN.len()..];
        let len = digits.bytes().take_while(u8::is_ascii_digit).count();
        if len > 0 && digits[len..].starts_with(']') {
            out.push_str(&rest[..i]);
            rest = &digits[len + 1..];
        } else {
            out.push_str(&rest[..i + 1]);
            rest = &rest[i + 1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::time::parse_rfc3339;

    // As a resumed Claude Code wrote it after a restart, a second on from
    // the first message, which never reached its queue.
    const DROPPED: &str = r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-23T10:11:57.620Z","content":"<task-notification>\n<task-id>af7034493327da98a</task-id>"}
{"type":"queue-operation","operation":"dequeue","timestamp":"2026-10-23T10:11:57.631Z"}
{"type":"user","timestamp":"2026-10-23T10:11:57.539Z","message":{"role":"user","content":[{"type":"tool_result","content":"[Tool call interrupted: the session ended]"}]}}
{"type":"assistant","timestamp":"2026-10-23T10:11:57.539Z","message":{"content":[{"type":"text","text":"No response requested."}]}}
"#;

    // And later: the next message, with an image, did.
    const TAKEN: &str = r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-23T10:12:03.974Z"}
{"type":"queue-operation","operation":"dequeue","timestamp":"2026-10-23T10:12:03.974Z"}
{"type":"user","timestamp":"2026-10-23T10:12:03.987Z","message":{"role":"user","content":[{"type":"text","text":"[Image #1]"},{"type":"image"},{"type":"text","text":" maybe related"}]}}
{"type":"queue-operation","operation":"enqueue","timestamp":"2026-10-23T10:20:00.000Z","content":"say \"hi\" to the \\ team"}
"#;

    #[test]
    fn heard() {
        let d = tempfile::tempdir().unwrap();
        let dropped = d.path().join("a.jsonl");
        let p = d.path().join("b.jsonl");
        std::fs::write(&dropped, DROPPED).unwrap();
        std::fs::write(&p, format!("{DROPPED}{TAKEN}")).unwrap();
        for (path, text, since, want) in [
            (
                &dropped,
                "[Image #1] sometimes sub agent just doen't show",
                "2026-10-23T10:11:57.115Z",
                Some(false),
            ),
            (
                &p,
                "[Image #1] maybe related",
                "2026-10-23T10:12:03.970Z",
                Some(true),
            ),
            (
                &p,
                "line one\nsay \"hi\" to the \\ team",
                "2026-10-23T10:19:59Z",
                Some(true),
            ),
            // Only what came since.
            (
                &p,
                "[Image #1] maybe related",
                "2026-10-23T10:30:00Z",
                Some(false),
            ),
            // Nothing to look for.
            (&p, "[Image #1]", "2026-10-23T10:11:57Z", None),
        ] {
            assert_eq!(heard_in(path, text, parse_rfc3339(since)), want, "{text:?}");
        }
        assert_eq!(
            heard_in(&d.path().join("none.jsonl"), "hi", rush_core::time::now()),
            None,
            "a transcript that isn't there can't say"
        );
    }

    #[test]
    fn snippets() {
        assert_eq!(heard_snippet("a [Image #3] b").as_deref(), Some("a  b"));
        assert_eq!(
            heard_snippet(&"x".repeat(40)).as_deref(),
            Some(&*"x".repeat(32))
        );
        assert_eq!(heard_snippet("tab\there"), None);
        assert_eq!(strip_markers("[Image #] [Image #12]x"), "[Image #] x");
    }
}
