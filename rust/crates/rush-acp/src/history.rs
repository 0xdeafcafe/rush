//! Saved conversations read back without starting an agent: the message
//! logs Kimi's older CLI and Vibe keep (one JSON message a line), the
//! content parts every provider's transcripts share, and the turn
//! boundaries message-only logs leave out. Port of
//! internal/adapters/acp/history.go:1-243.
//!
//! Go reads a whole log at once. Here a log is read a line at a time by a
//! [`Lines`] (this module's for message logs, kimi_wire's for Kimi Code's
//! wire), handed on a slice at a time ([`read_each`]), from its end
//! ([`read_tail`]), or as it grows ([`Follower`]).

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use base64::Engine;
use rush_core::adapter::{self, Follow, Slice, WaitOn};
use rush_core::event::{self, Event, PartKind};
use rush_core::time::Ms;
use rush_core::tool;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::kimi_wire::kimi_call;

/// Why a saved conversation couldn't be read.
#[derive(Debug)]
pub struct HistoryError(pub String);

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HistoryError {}

/// The text and inline images provider transcripts share: a string, or a
/// list of blocks. Remote image URLs stay visible as text; reading history
/// never fetches them.
pub fn content_parts(raw: Option<&RawValue>) -> Vec<event::Part> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    if let Ok(t) = serde_json::from_str::<Option<String>>(raw.get()) {
        return match t {
            Some(t) if !t.is_empty() => vec![text_part(PartKind::Text, t)],
            _ => Vec::new(),
        };
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Url {
        url: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Inline {
        #[serde(rename = "mimeType")]
        mime_type: String,
        data: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Block {
        text: String,
        image_url: Url,
        #[serde(rename = "inlineData")]
        inline_data: Option<Inline>,
    }
    let Ok(blocks) = serde_json::from_str::<Vec<Block>>(raw.get()) else {
        return Vec::new();
    };
    let b64 = &base64::engine::general_purpose::STANDARD;
    let mut parts = Vec::new();
    for b in blocks {
        if !b.text.is_empty() {
            parts.push(text_part(PartKind::Text, b.text));
        }
        if let Some(d) = b.inline_data
            && let Ok(data) = b64.decode(d.data.as_bytes())
        {
            parts.push(image_part(d.mime_type, data, String::new()));
        }
        let url = b.image_url.url;
        if url.is_empty() {
            continue;
        }
        match url.split_once(',') {
            Some((header, data)) if header.starts_with("data:") && header.ends_with(";base64") => {
                if let Ok(decoded) = b64.decode(data.as_bytes()) {
                    let mt = header["data:".len()..header.len() - ";base64".len()].to_string();
                    parts.push(image_part(mt, decoded, String::new()));
                }
            }
            _ => parts.push(text_part(PartKind::Text, format!("[image: {url}]"))),
        }
    }
    parts
}

pub(crate) fn text_part(kind: PartKind, text: String) -> event::Part {
    event::Part {
        kind,
        text,
        ..event::Part::default()
    }
}

fn image_part(media_type: String, data: Vec<u8>, path: String) -> event::Part {
    event::Part {
        kind: PartKind::Image,
        image: Some(Box::new(event::ImageData {
            media_type,
            data,
            path,
        })),
        ..event::Part::default()
    }
}

/// The boundaries message-only snapshots leave out. A regular user message
/// starts a new turn; tool results and messages marked injected stay in the
/// current one. Fed events in order; finish closes the last, so viewing a
/// saved conversation never leaves a working spinner.
#[derive(Default, Clone)]
pub struct Turns {
    open: bool,
}

impl Turns {
    pub fn push(&mut self, ev: Event, out: &mut Vec<Event>) {
        match &ev {
            Event::Message(m) => {
                let prompt = m.role == "user"
                    && !m.injected
                    && m.parent.is_empty()
                    && !m.parts.is_empty()
                    && m.parts
                        .iter()
                        .all(|p| p.kind == PartKind::Text || p.kind == PartKind::Image);
                if prompt && self.open {
                    out.push(turn_done());
                    self.open = false;
                }
                if !m.parts.is_empty() {
                    self.open = true;
                }
            }
            Event::TurnEnd(_) => self.open = false,
            _ => {}
        }
        out.push(ev);
    }

    pub fn finish(&mut self, out: &mut Vec<Event>) {
        if self.open {
            out.push(turn_done());
            self.open = false;
        }
    }
}

fn turn_done() -> Event {
    Event::TurnEnd(event::TurnEnd {
        reason: "done".into(),
        turns: 1,
        ..event::TurnEnd::default()
    })
}

/// Go's EndHistoryTurns: events with the boundaries supplied.
pub fn end_history_turns(events: Vec<Event>) -> Vec<Event> {
    let mut t = Turns::default();
    let mut out = Vec::with_capacity(events.len() + 1);
    for ev in events {
        t.push(ev, &mut out);
    }
    t.finish(&mut out);
    out
}

/// One transcript's lines read as events, keeping what reading on needs.
pub(crate) trait Lines: Send {
    /// Reads line b (without its newline). partial: it's the last, with no
    /// newline yet, so still being written.
    fn line(&mut self, b: &[u8], partial: bool, out: &mut Vec<Event>) -> Result<(), HistoryError>;
    /// The transcript has ended.
    fn finish(&mut self, out: &mut Vec<Event>);
}

/// Reads r a line at a time through lines, handing events to each once at
/// least most bytes have been read; each returns false to stop.
pub(crate) fn read_each(
    r: impl Read,
    lines: &mut dyn Lines,
    most: usize,
    each: &mut dyn FnMut(Vec<Event>) -> bool,
) -> Result<(), HistoryError> {
    let mut r = BufReader::with_capacity(64 << 10, r);
    let (mut out, mut taken, mut buf) = (Vec::new(), 0usize, Vec::new());
    loop {
        buf.clear();
        let n = r
            .read_until(b'\n', &mut buf)
            .map_err(|e| HistoryError(e.to_string()))?;
        if n == 0 {
            break;
        }
        taken += n;
        let partial = buf.last() != Some(&b'\n');
        let mut b = buf.as_slice();
        b = b.strip_suffix(b"\n").unwrap_or(b);
        b = b.strip_suffix(b"\r").unwrap_or(b);
        if let Err(e) = lines.line(b, partial, &mut out) {
            if !out.is_empty() {
                each(std::mem::take(&mut out));
            }
            return Err(e);
        }
        if taken >= most.max(1) {
            taken = 0;
            if !out.is_empty() && !each(std::mem::take(&mut out)) {
                return Ok(());
            }
        }
    }
    lines.finish(&mut out);
    if !out.is_empty() {
        each(out);
    }
    Ok(())
}

/// Every event lines reads from r, at once.
pub(crate) fn read_all(r: impl Read, lines: &mut dyn Lines) -> Result<Vec<Event>, HistoryError> {
    let mut all = Vec::new();
    read_each(r, lines, usize::MAX, &mut |evs| {
        all.extend(evs);
        true
    })?;
    Ok(all)
}

/// The end of the transcript at path, from the first whole line of its last
/// most bytes (all of it when most is 0 or more than it has); the bool is
/// whether that left some out.
pub(crate) fn read_tail(
    path: &Path,
    most: i64,
    lines: &mut dyn Lines,
) -> Result<(Vec<Event>, bool), HistoryError> {
    let io = |e: std::io::Error| HistoryError(e.to_string());
    let mut f =
        File::open(path).map_err(|e| HistoryError(format!("open {}: {e}", path.display())))?;
    let size = f.metadata().map_err(io)?.len() as i64;
    if most <= 0 || size <= most {
        return Ok((read_all(f, lines)?, false));
    }
    f.seek(SeekFrom::Start((size - most - 1) as u64))
        .map_err(io)?;
    let mut r = BufReader::new(f.take(most as u64 + 1));
    // Skip a first line cut short, without keeping its body.
    loop {
        let buf = r.fill_buf().map_err(io)?;
        if buf.is_empty() {
            break;
        }
        match memchr::memchr(b'\n', buf) {
            Some(i) => {
                r.consume(i + 1);
                break;
            }
            None => {
                let n = buf.len();
                r.consume(n);
            }
        }
    }
    Ok((read_all(r, lines)?, true))
}

/// A transcript followed as it grows, read through lines: no turn is closed
/// at its end, since one may be under way.
pub(crate) struct Follower {
    path: PathBuf,
    off: u64,
    lines: Box<dyn Lines>,
    fresh: fn(&Path) -> Box<dyn Lines>,
}

impl Follower {
    pub fn new(path: &str, fresh: fn(&Path) -> Box<dyn Lines>) -> Follower {
        let path = PathBuf::from(path);
        Follower {
            lines: fresh(&path),
            path,
            off: 0,
            fresh,
        }
    }
}

impl Follow for Follower {
    fn next(&mut self, most: usize) -> adapter::Result<Slice> {
        let mut f = File::open(&self.path)?;
        let size = f.metadata()?.len();
        let mut out = Slice::default();
        if size < self.off {
            // Rewritten: start over.
            self.off = 0;
            self.lines = (self.fresh)(&self.path);
            out.reset = true;
        }
        if size == self.off {
            return Ok(out);
        }
        f.seek(SeekFrom::Start(self.off))?;
        let want = (size - self.off).min(most.max(1) as u64);
        let mut buf = Vec::with_capacity(want as usize);
        (&mut f).take(want).read_to_end(&mut buf)?;
        let full = match memchr::memrchr(b'\n', &buf) {
            Some(i) => i + 1,
            // One line longer than most: a slice of its own, once whole.
            None if (buf.len() as u64) < size - self.off => {
                let mut r = BufReader::new(f);
                r.read_until(b'\n', &mut buf)?;
                if buf.last() != Some(&b'\n') {
                    return Ok(out);
                }
                buf.len()
            }
            None => return Ok(out), // a line still being written waits
        };
        for l in buf[..full].split_inclusive(|&c| c == b'\n') {
            let mut b = l.strip_suffix(b"\n").unwrap_or(l);
            b = b.strip_suffix(b"\r").unwrap_or(b);
            // A line that can't be read is passed over: what follows still
            // reads.
            let _ = self.lines.line(b, false, &mut out.events);
        }
        self.off += full as u64;
        out.more = self.off < size && want as usize == most.max(1);
        Ok(out)
    }

    fn wait_on(&self) -> WaitOn {
        WaitOn {
            paths: vec![self.path.clone()],
            ..WaitOn::default()
        }
    }
}

/// A message log's lines: Kimi's older context.jsonl and Vibe's
/// messages.jsonl. Checkpoint and system records aren't conversation
/// messages.
pub(crate) struct MessageLines {
    transcript: PathBuf,
    before: Ms,
    n: usize,
    turns: Turns,
}

impl MessageLines {
    pub fn new(transcript: &Path, before: Ms) -> MessageLines {
        MessageLines {
            transcript: transcript.to_path_buf(),
            before,
            n: 0,
            turns: Turns::default(),
        }
    }

    pub fn boxed(transcript: &Path) -> Box<dyn Lines> {
        Box::new(MessageLines::new(transcript, 0))
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Source {
    path: String,
    data: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Image {
    path: String,
    data: String,
    mime_type: String,
    source: Source,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Function {
    name: String,
    arguments: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CallLine {
    id: String,
    function: Function,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MessageLine<'a> {
    #[serde(borrow)]
    timestamp: Option<&'a RawValue>,
    role: String,
    message_id: String,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    #[serde(borrow)]
    reasoning_content: Option<&'a RawValue>,
    tool_call_id: String,
    injected: bool,
    images: Option<Vec<Image>>,
    tool_calls: Option<Vec<CallLine>>,
}

impl Lines for MessageLines {
    fn line(&mut self, b: &[u8], _: bool, out: &mut Vec<Event>) -> Result<(), HistoryError> {
        self.n += 1;
        let m: MessageLine = serde_json::from_slice(b)
            .map_err(|e| HistoryError(format!("transcript line {}: {e}", self.n)))?;
        if self.before != 0
            && let Some(ts) = m.timestamp
            && let Ok(stamp) = serde_json::from_str::<String>(ts.get())
        {
            let at = rush_core::time::parse_rfc3339(&stamp);
            if at != 0 && at >= self.before {
                return Ok(());
            }
        }
        if m.role != "user" && m.role != "assistant" && m.role != "tool" {
            return Ok(());
        }
        let mut msg = event::Message {
            role: m.role.clone(),
            id: m.message_id,
            injected: m.injected,
            ..event::Message::default()
        };
        for mut p in content_parts(m.reasoning_content) {
            p.kind = PartKind::Thinking;
            msg.parts.push(p);
        }
        let parts = content_parts(m.content);
        if m.role == "tool" {
            msg.role = "user".into();
            let text: String = parts.iter().map(|p| p.text.as_str()).collect();
            msg.parts.push(event::Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(tool::Output {
                    call_id: m.tool_call_id,
                    text,
                    ..tool::Output::default()
                })),
                ..event::Part::default()
            });
        } else {
            msg.parts.extend(parts);
        }
        let b64 = &base64::engine::general_purpose::STANDARD;
        for image in m.images.unwrap_or_default() {
            let mut path = image.source.path;
            if path.is_empty() {
                path = image.path;
            }
            let mut data = image.source.data;
            if data.is_empty() {
                data = image.data;
            }
            if !path.is_empty() && !Path::new(&path).is_absolute() {
                let dir = self.transcript.parent().unwrap_or(Path::new(""));
                path = dir.join(path).to_string_lossy().into_owned();
            }
            let data = if data.is_empty() {
                Vec::new()
            } else {
                b64.decode(data.as_bytes()).unwrap_or_default()
            };
            if !path.is_empty() || !data.is_empty() {
                msg.parts.push(image_part(image.mime_type, data, path));
            }
        }
        for c in m.tool_calls.unwrap_or_default() {
            let mut call = kimi_call(&c.id, &c.function.name, &c.function.arguments);
            if call.kind == tool::Kind::Subagent {
                call.input.child = call.id.clone(); // Vibe's child session, linked by the call
            }
            msg.parts.push(event::Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..event::Part::default()
            });
        }
        if !msg.parts.is_empty() {
            self.turns.push(Event::Message(msg), out);
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut Vec<Event>) {
        self.turns.finish(out);
    }
}

fn open(path: &str) -> Result<File, HistoryError> {
    File::open(path).map_err(|e| HistoryError(format!("open {path}: {e}")))
}

/// Kimi's context and Vibe's message log, read without starting an agent.
pub fn read_messages(path: &str) -> Result<Vec<Event>, HistoryError> {
    read_messages_before(path, 0)
}

/// Those written before `before` (0: all of them).
pub fn read_messages_before(path: &str, before: Ms) -> Result<Vec<Event>, HistoryError> {
    read_all(open(path)?, &mut MessageLines::new(Path::new(path), before))
}

/// A message log's end, from within its last most bytes; the bool is
/// whether that left some out.
pub fn read_messages_tail(path: &str, most: i64) -> Result<(Vec<Event>, bool), HistoryError> {
    read_tail(
        Path::new(path),
        most.max(1),
        &mut MessageLines::new(Path::new(path), 0),
    )
}

/// A message log read a slice at a time.
pub(crate) fn messages_each(
    path: &str,
    before: Ms,
    most: usize,
    each: &mut dyn FnMut(Vec<Event>) -> bool,
) -> Result<(), HistoryError> {
    read_each(
        open(path)?,
        &mut MessageLines::new(Path::new(path), before),
        most,
        each,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn messages(evs: &[Event]) -> Vec<&event::Message> {
        evs.iter()
            .filter_map(|e| match e {
                Event::Message(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    // history_test.go: TestVibeHistoryImages
    #[test]
    fn vibe_history_images() {
        let d = tempfile::tempdir().unwrap();
        let path = write(
            d.path(),
            "messages.jsonl",
            r#"{"role":"user","content":"question","images":[{"source":{"kind":"file","path":"attachments/a.png"},"mime_type":"image/png"}]}"#,
        );
        let evs = read_messages(&path).unwrap();
        let m = messages(&evs)[0];
        let want = d.path().join("attachments/a.png");
        assert_eq!(m.parts.len(), 2);
        assert_eq!(
            m.parts[1].image.as_ref().unwrap().path,
            want.to_string_lossy()
        );
    }

    // history_test.go: TestMessageHistoryTailAndCutoff
    #[test]
    fn tail_and_cutoff() {
        let d = tempfile::tempdir().unwrap();
        let path = write(
            d.path(),
            "messages.jsonl",
            "{\"timestamp\":\"2026-10-01T00:00:00Z\",\"role\":\"user\",\"content\":\"old\"}\n{\"timestamp\":\"2026-10-01T00:01:00Z\",\"role\":\"assistant\",\"content\":\"recent\"}\n",
        );
        let (evs, cut) = read_messages_tail(&path, 85).unwrap();
        assert!(cut);
        let ms = messages(&evs);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].parts[0].text, "recent");
        let before = rush_core::time::parse_rfc3339("2026-10-01T00:01:00Z");
        let evs = read_messages_before(&path, before).unwrap();
        for m in messages(&evs) {
            assert_eq!(m.parts[0].text, "old", "included the cutoff message");
        }
    }

    // history_turns_test.go: TestSavedMessageTurnsRenderSeparatelyAndFinish,
    // without convo: the boundaries it needs.
    #[test]
    fn saved_turns_end_and_injected_stay() {
        let d = tempfile::tempdir().unwrap();
        let path = write(
            d.path(),
            "messages.jsonl",
            r#"{"role":"user","content":"first request"}
{"role":"assistant","content":"working","tool_calls":[{"id":"call","function":{"name":"read","arguments":"{}"}}]}
{"role":"tool","tool_call_id":"call","content":"contents"}
{"role":"user","injected":true,"content":"injected context"}
{"role":"assistant","content":"first answer"}
{"role":"user","content":"second request"}
{"role":"assistant","content":"second answer"}
"#,
        );
        let evs = read_messages(&path).unwrap();
        let shape: Vec<String> = evs
            .iter()
            .map(|e| match e {
                Event::TurnEnd(t) => format!("end:{}", t.reason),
                Event::Message(m) => {
                    format!("{}{}", m.role, if m.injected { "(injected)" } else { "" })
                }
                _ => "?".into(),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "user",
                "assistant",
                "user",
                "user(injected)",
                "assistant",
                "end:done",
                "user",
                "assistant",
                "end:done"
            ]
        );
    }

    #[test]
    fn slices_carry_the_turns_across() {
        let d = tempfile::tempdir().unwrap();
        let line =
            "{\"role\":\"user\",\"content\":\"q\"}\n{\"role\":\"assistant\",\"content\":\"a\"}\n";
        let path = write(d.path(), "m.jsonl", &line.repeat(3));
        let mut slices = Vec::new();
        messages_each(&path, 0, line.len(), &mut |evs| {
            slices.push(evs.len());
            true
        })
        .unwrap();
        // Each slice two lines; the boundary before a prompt goes with it,
        // and the last turn is closed at the end.
        assert_eq!(slices, [2, 3, 3, 1]);
    }

    #[test]
    fn content_parts_read_strings_blocks_and_images() {
        let raw = |s: &str| RawValue::from_string(s.to_string()).unwrap();
        assert!(content_parts(Some(&raw(r#""""#))).is_empty());
        assert!(content_parts(Some(&raw("null"))).is_empty());
        let parts = content_parts(Some(&raw(
            r#"[{"text":"a"},{"inlineData":{"mimeType":"image/png","data":"aGk="}},{"image_url":{"url":"https://x/y.png"}}]"#,
        )));
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].image.as_ref().unwrap().data, b"hi");
        assert_eq!(parts[2].text, "[image: https://x/y.png]");
    }

    #[test]
    fn a_malformed_line_fails_the_read() {
        let d = tempfile::tempdir().unwrap();
        let path = write(
            d.path(),
            "m.jsonl",
            "{\"role\":\"user\",\"content\":\"a\"}\nnope\n",
        );
        let err = read_messages(&path).unwrap_err();
        assert!(err.0.starts_with("transcript line 2: "), "{err}");
    }

    #[test]
    fn follows_a_log_as_it_grows() {
        let d = tempfile::tempdir().unwrap();
        let a = "{\"role\":\"user\",\"content\":\"a\"}\n";
        let path = write(d.path(), "m.jsonl", &a.repeat(2));
        let mut f = Follower::new(&path, MessageLines::boxed);
        let sl = f.next(a.len()).unwrap();
        assert_eq!((sl.events.len(), sl.more), (1, true));
        let sl = f.next(1 << 20).unwrap();
        // The second prompt ends the first turn; the last stays open.
        assert_eq!((sl.events.len(), sl.more), (2, false));
        std::fs::write(&path, a).unwrap();
        let sl = f.next(1 << 20).unwrap();
        assert!(sl.reset);
        assert_eq!(sl.events.len(), 1);
    }
}
