//! A Claude Code transcript read back as rush's own events, a slice at a
//! time: its messages, from before a time when one is given. A subagent's
//! lines and Claude Code's own notes are left out, as the conversation
//! leaves them out. Port of internal/adapters/claude/history.go:31-71
//! (History), sliced; following it as it grows is this port's own
//! (Go's Claude adapter has no FollowHistory: its pane tails the
//! transcript itself).

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;

use rush_core::adapter::{self, Follow, MessageDecoder, Session, Slice, WaitOn};
use rush_core::event::Event;
use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::json::{Str, ms, nd};

/// What a history read needs of a transcript line.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(rename = "isMeta", deserialize_with = "nd")]
    is_meta: bool,
    #[serde(rename = "isSidechain", deserialize_with = "nd")]
    is_sidechain: bool,
    #[serde(borrow)]
    timestamp: Str<'a>,
    #[serde(borrow)]
    message: Option<&'a RawValue>,
    #[serde(rename = "toolUseResult", borrow)]
    tool_use_result: Option<&'a RawValue>,
}

/// What one line comes to.
enum Took {
    Events(Vec<Event>),
    Skip,
    /// Written at or after the time asked for: the read ends.
    Past,
}

fn take(dec: &mut dyn MessageDecoder, b: &[u8], before: Ms) -> Took {
    let b = b.strip_suffix(b"\n").unwrap_or(b);
    if b.is_empty() {
        return Took::Skip;
    }
    let Ok(l) = serde_json::from_slice::<Line>(b) else {
        return Took::Skip;
    };
    if l.is_meta || l.is_sidechain {
        return Took::Skip;
    }
    if before != 0 {
        let at = ms(&l.timestamp);
        if at != 0 && at >= before {
            return Took::Past;
        }
    }
    if &*l.typ != "user" && &*l.typ != "assistant" {
        return Took::Skip;
    }
    let Some(m) = l.message else {
        return Took::Skip;
    };
    Took::Events(dec.message(&l.typ, m, l.tool_use_result))
}

/// A session with no transcript to read.
#[derive(Debug)]
struct NoTranscript;

impl std::fmt::Display for NoTranscript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("claude: this session has no transcript")
    }
}

impl std::error::Error for NoTranscript {}

/// HistoryReader::history: s's messages written before `before`, handed
/// to each a slice at a time, each slice from at most `most` bytes.
pub(crate) fn read(
    s: &Session,
    before: Ms,
    most: usize,
    each: &mut dyn FnMut(Vec<Event>) -> bool,
) -> adapter::Result<()> {
    if s.transcript.is_empty() {
        return Err(Box::new(NoTranscript));
    }
    let mut r = BufReader::with_capacity(64 << 10, File::open(&s.transcript)?);
    let mut dec = crate::messages();
    let (mut out, mut taken, mut line) = (Vec::new(), 0usize, Vec::new());
    loop {
        line.clear();
        let n = r.read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        taken += n;
        match take(dec.as_mut(), &line, before) {
            Took::Events(evs) => out.extend(evs),
            Took::Skip => {}
            Took::Past => break,
        }
        if taken >= most.max(1) {
            taken = 0;
            if !out.is_empty() && !each(std::mem::take(&mut out)) {
                return Ok(());
            }
        }
    }
    if !out.is_empty() {
        each(out);
    }
    Ok(())
}

/// HistoryFollower::follow_history: s's transcript from its start, then
/// what's added as it grows.
pub(crate) fn follow(s: &Session) -> adapter::Result<Box<dyn Follow>> {
    if s.transcript.is_empty() {
        return Err(Box::new(NoTranscript));
    }
    Ok(Box::new(Follower {
        path: PathBuf::from(&s.transcript),
        off: 0,
        seen: None,
        dec: crate::messages(),
    }))
}

/// A transcript followed: how far it's been read, and the decoder that
/// knows the calls still waiting for their results.
struct Follower {
    path: PathBuf,
    off: u64,
    /// The file read so far (adapter::file_id), to tell one renamed over it.
    seen: Option<(u64, u64)>,
    dec: Box<dyn MessageDecoder>,
}

impl Follow for Follower {
    fn next(&mut self, most: usize) -> adapter::Result<Slice> {
        let mut f = File::open(&self.path)?;
        let meta = f.metadata()?;
        let size = meta.len();
        let mut out = Slice::default();
        if adapter::rewritten(&mut self.seen, self.off, &meta) {
            // Rewritten, or another file renamed over it: start over.
            self.off = 0;
            self.dec = crate::messages();
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
            // One line longer than most: it's a slice of its own, once
            // it's whole.
            None if (buf.len() as u64) < size - self.off => {
                let mut r = BufReader::new(f);
                r.read_until(b'\n', &mut buf)?;
                if buf.last() != Some(&b'\n') {
                    return Ok(out); // still being written
                }
                buf.len()
            }
            None => return Ok(out), // a line still being written waits
        };
        for l in buf[..full].split_inclusive(|&c| c == b'\n') {
            if let Took::Events(evs) = take(self.dec.as_mut(), l, 0) {
                out.events.extend(evs);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::tests::TempDir;
    use rush_core::adapter::{HistoryFollower, HistoryReader, read_history};
    use rush_core::event::{Event, PartKind};

    fn user(text: &str, at: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"{at}","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        ) + "\n"
    }

    fn texts(evs: &[Event]) -> Vec<String> {
        evs.iter()
            .filter_map(|e| match e {
                Event::Message(m) => m
                    .parts
                    .iter()
                    .find(|p| p.kind == PartKind::Text)
                    .map(|p| p.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reads_in_slices_up_to_a_time() {
        let d = TempDir::new("history");
        let p = d.0.join("t.jsonl");
        let mut body = String::new();
        for i in 0..6 {
            body += &user(&format!("m{i}"), &format!("2026-09-29T10:00:0{i}Z"));
        }
        // Claude Code's own notes and a subagent's lines are left out.
        body += r#"{"type":"user","isMeta":true,"timestamp":"2026-09-29T10:00:07Z","message":{"role":"user","content":"meta"}}"#;
        body += "\n";
        std::fs::write(&p, &body).unwrap();
        let s = Session {
            transcript: p.to_string_lossy().into_owned(),
            ..Session::default()
        };
        let c = crate::Claude::new();
        let all = read_history(&c, &s, 0).unwrap();
        assert_eq!(texts(&all), ["m0", "m1", "m2", "m3", "m4", "m5"]);
        // Two lines' worth a slice, stopped after the second.
        let one = user("m0", "2026-09-29T10:00:00Z").len();
        let mut slices = Vec::new();
        c.history(&s, 0, 2 * one, &mut |evs| {
            slices.push(texts(&evs));
            slices.len() < 2
        })
        .unwrap();
        assert_eq!(slices, [vec!["m0", "m1"], vec!["m2", "m3"]]);
        // Only those written before 10:00:03.
        let before = rush_core::time::parse_rfc3339("2026-09-29T10:00:03Z");
        let mut got = Vec::new();
        c.history(&s, before, 1 << 20, &mut |evs| {
            got.extend(texts(&evs));
            true
        })
        .unwrap();
        assert_eq!(got, ["m0", "m1", "m2"]);
    }

    #[test]
    fn follows_as_it_grows() {
        let d = TempDir::new("follow");
        let p = d.0.join("t.jsonl");
        let a = user("a", "2026-09-29T10:00:00Z");
        std::fs::write(&p, a.repeat(3)).unwrap();
        let s = Session {
            transcript: p.to_string_lossy().into_owned(),
            ..Session::default()
        };
        let mut f = crate::Claude::new().follow_history(&s).unwrap();
        assert_eq!(f.wait_on().paths, std::slice::from_ref(&p));
        // A slice of two lines, then the rest.
        let sl = f.next(2 * a.len()).unwrap();
        assert_eq!(
            (texts(&sl.events).len(), sl.more, sl.reset),
            (2, true, false)
        );
        let sl = f.next(2 * a.len()).unwrap();
        assert_eq!((texts(&sl.events).len(), sl.more), (1, false));
        // A line still being written waits for its end.
        {
            use std::io::Write;
            let mut w = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            let b = user("b", "2026-09-29T10:00:01Z");
            w.write_all(&b.as_bytes()[..10]).unwrap();
            assert!(f.next(1 << 20).unwrap().events.is_empty());
            w.write_all(&b.as_bytes()[10..]).unwrap();
        }
        assert_eq!(texts(&f.next(1 << 20).unwrap().events), ["b"]);
        // Rewritten shorter: it starts over.
        std::fs::write(&p, &a).unwrap();
        let sl = f.next(1 << 20).unwrap();
        assert!(sl.reset);
        assert_eq!(texts(&sl.events), ["a"]);
        // Another file renamed over it, longer than what was read: it
        // starts over too.
        let new = d.0.join("t.new");
        std::fs::write(&new, a.repeat(2)).unwrap();
        std::fs::rename(&new, &p).unwrap();
        let sl = f.next(1 << 20).unwrap();
        assert!(sl.reset, "a file renamed over it wasn't a rewrite");
        assert_eq!(texts(&sl.events), ["a", "a"]);
    }
}
