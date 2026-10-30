//! A thread's rollout read back as rush's events: a slice at a time, its
//! end only, up to a time, and as it grows. Port of
//! internal/adapters/codex/history.go:1-253 (History, HistoryTail,
//! HistoryTailBefore, rolloutCutoff, FollowHistory, wholeLines,
//! openRollout, readRollout).
//!
//! Go's FollowHistory gives the whole history again each time, the turn
//! under way read as if it had ended; a Follow here gives only what's new
//! since the last slice, each event once, as Replay::take settles them.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use rush_core::adapter::{self, Follow, Session, Slice, WaitOn};
use rush_core::event::Event;
use rush_core::time::Ms;

use crate::discover::is_rollout;
use crate::json::ms;
use crate::rollout::{Replay, feed, read_lines, read_rollout};

/// A thread with no rollout to read.
#[derive(Debug)]
struct NoRollout(String);

impl std::fmt::Display for NoRollout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "codex: session {} has no rollout", self.0)
    }
}

impl std::error::Error for NoRollout {}

/// Where s's rollout is: its transcript, else the one of thread s.id in
/// s.profile, found by the name Codex gives it.
pub(crate) fn rollout_path(s: &Session) -> adapter::Result<PathBuf> {
    if !s.transcript.is_empty() {
        return Ok(PathBuf::from(&s.transcript));
    }
    if !s.id.is_empty() {
        // Known by its thread alone: its rollout is named after it,
        // sessions/YYYY/MM/DD/rollout-*-<id>.jsonl; the last, in name order.
        let want = format!("-{}.jsonl", s.id);
        let mut found: Vec<PathBuf> = Vec::new();
        let root = Path::new(&s.profile.dir).join("sessions");
        for y in sorted(&root) {
            for m in sorted(&y) {
                for d in sorted(&m) {
                    for f in sorted(&d) {
                        let name = f.file_name().map(|n| n.to_string_lossy().into_owned());
                        if name.is_some_and(|n| is_rollout(&n) && n.ends_with(&want)) {
                            found.push(f);
                        }
                    }
                }
            }
        }
        if let Some(p) = found.pop() {
            return Ok(p);
        }
    }
    Err(Box::new(NoRollout(s.id.clone())))
}

fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// HistoryReader::history: s's rollout as the events a live thread sends,
/// those written before `before` (0: all), handed to each a slice at a
/// time, each slice from at most `most` bytes.
pub(crate) fn read(
    s: &Session,
    before: Ms,
    most: usize,
    each: &mut dyn FnMut(Vec<Event>) -> bool,
) -> adapter::Result<()> {
    let f = File::open(rollout_path(s)?)?;
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut rp = Replay::default();
    let (mut taken, mut stopped) = (0usize, false);
    read_lines(&mut r, |b, n| {
        if !feed(&mut rp, b, before) {
            return false;
        }
        taken += n;
        if taken >= most.max(1) {
            taken = 0;
            let evs = rp.take();
            if !evs.is_empty() && !each(evs) {
                stopped = true;
                return false;
            }
        }
        true
    })?;
    if stopped {
        return Ok(());
    }
    rp.flush();
    let evs = rp.take();
    if !evs.is_empty() {
        each(evs);
    }
    Ok(())
}

/// History read from the rollout's first line, the thread's meta, then the
/// first whole line of its last most bytes on, of those written before
/// `before` (0: all); the bool is whether that left some out.
pub(crate) fn tail_before(
    s: &Session,
    most: i64,
    before: Ms,
) -> adapter::Result<(Vec<Event>, bool)> {
    let f = File::open(rollout_path(s)?)?;
    let size = f.metadata()?.len() as i64;
    let mut end = size;
    if before != 0 {
        match rollout_cutoff(&f, size, before) {
            Some(e) => end = e,
            None => return Ok((read_rollout(section(&f, 0, size), before)?, false)),
        }
    }
    let most = most.max(1);
    if end <= most {
        return Ok((read_rollout(section(&f, 0, end), before)?, false));
    }
    // The first line, and the first turn_context, from before the tail.
    let mut head = Vec::new();
    let mut n = 0;
    let mut hr = BufReader::with_capacity(64 << 10, section(&f, 0, end - most));
    read_lines(&mut hr, |b, _| {
        n += 1;
        let context = n > 1
            && memchr::memmem::find(&b[..b.len().min(160)], br#""type":"turn_context""#).is_some();
        if n == 1 || context {
            head.extend_from_slice(b);
            head.push(b'\n');
        }
        !context && n < 64
    })?;
    // The tail, from the first whole line: the line before it ends with the
    // byte before the tail's start.
    let mut tail = BufReader::new(section(&f, end - most - 1, most + 1));
    let mut skip = Vec::new();
    tail.read_until(b'\n', &mut skip)?;
    let evs = read_rollout(head.as_slice().chain(tail), before)?;
    Ok((evs, true))
}

/// Bytes from..from+n of f.
fn section(f: &File, from: i64, n: i64) -> impl Read + '_ {
    let mut r = f;
    let ok = r.seek(SeekFrom::Start(from.max(0) as u64)).is_ok();
    r.take(if ok { n.max(0) as u64 } else { 0 })
}

/// Where the first line written at or after before starts, found by a
/// binary search over Codex's chronological envelope timestamps. Only
/// short envelope prefixes are read, never the messages or tool outputs.
/// None for an envelope it doesn't know, for the whole reader to read.
pub(crate) fn rollout_cutoff(f: &File, size: i64, before: Ms) -> Option<i64> {
    let mut buf = vec![0u8; 64 << 10];
    let mut next = |at: i64| -> i64 {
        if at == 0 {
            return 0;
        }
        let mut at = at - 1;
        while at < size {
            let want = (buf.len() as i64).min(size - at) as usize;
            let n = f.read_at(&mut buf[..want], at as u64).unwrap_or(0);
            if n == 0 {
                return size;
            }
            if let Some(i) = memchr::memchr(b'\n', &buf[..n]) {
                return at + i as i64 + 1;
            }
            at += n as i64;
        }
        size
    };
    let mut prefix = [0u8; 512];
    let (mut lo, mut hi) = (0i64, size);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let at = next(mid);
        if at >= size {
            hi = mid;
            continue;
        }
        let want = 512.min(size - at) as usize;
        let n = f.read_at(&mut prefix[..want], at as u64).unwrap_or(0);
        let p = &prefix[..n];
        let key = memchr::memmem::find(p, br#""timestamp""#)?;
        if memchr::memmem::find(p, br#""payload""#).is_some_and(|x| x < key) {
            return None;
        }
        let v = p[key + 11..].trim_ascii_start();
        let v = v.strip_prefix(b":")?.trim_ascii_start();
        let v = v.strip_prefix(b"\"")?;
        let j = memchr::memchr(b'"', v)?;
        let stamp = ms(std::str::from_utf8(&v[..j]).ok()?);
        if stamp == 0 {
            return None;
        }
        if stamp < before {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Some(next(lo))
}

/// HistoryFollower::follow_history: s's rollout from its start, then what
/// it gains.
pub(crate) fn follow(s: &Session) -> adapter::Result<Box<dyn Follow>> {
    Ok(Box::new(Follower {
        path: rollout_path(s)?,
        off: 0,
        replay: Replay::default(),
    }))
}

/// A rollout followed: how far it's been read, and what reading on needs.
struct Follower {
    path: PathBuf,
    off: u64,
    replay: Replay,
}

impl Follow for Follower {
    fn next(&mut self, most: usize) -> adapter::Result<Slice> {
        let mut f = File::open(&self.path)?;
        let size = f.metadata()?.len();
        let mut out = Slice::default();
        if size < self.off {
            // Rewritten: start over.
            (self.off, self.replay) = (0, Replay::default());
            out.reset = true;
        }
        if size == self.off {
            return Ok(out);
        }
        let most = most.max(1) as u64;
        f.seek(SeekFrom::Start(self.off))?;
        let want = (size - self.off).min(most);
        let mut buf = Vec::with_capacity(want as usize);
        (&mut f).take(want).read_to_end(&mut buf)?;
        let full = match memchr::memrchr(b'\n', &buf) {
            Some(i) => i + 1,
            // One line longer than most: a slice of its own, once whole.
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
        let rp = &mut self.replay;
        read_lines(&mut io::Cursor::new(&buf[..full]), |b, _| {
            rp.line(b);
            true
        })?;
        out.events = rp.take();
        self.off += full as u64;
        out.more = self.off < size && want == most;
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
    use crate::rollout::tests::{
        Rollout, describe, describe_all, history, item_turn, item_turn_want, meta, t0, user_msg,
    };
    use rush_core::adapter::Profile;
    use serde_json::json;

    fn at(path: &Path) -> Session {
        Session {
            transcript: path.to_string_lossy().into_owned(),
            ..Session::default()
        }
    }

    fn whole(s: &Session) -> Vec<Event> {
        let mut out = Vec::new();
        read(s, 0, adapter::SLICE, &mut |evs| {
            out.extend(evs);
            true
        })
        .unwrap();
        out
    }

    // rollout_test.go:TestHistoryFindsTheRolloutByThread
    #[test]
    fn finds_the_rollout_by_thread() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions/2026/09/27");
        std::fs::create_dir_all(&day).unwrap();
        let id = "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000";
        let line = format!(
            r#"{{"timestamp":"2026-09-27T10:00:00.000Z","type":"session_meta","payload":{{"id":"{id}","cwd":"/w","cli_version":"0.155.1"}}}}"#
        );
        std::fs::write(
            day.join(format!("rollout-2026-09-27T10-00-00-{id}.jsonl")),
            line + "\n",
        )
        .unwrap();
        let p = Profile {
            dir: dir.path().to_string_lossy().into_owned(),
            ..Profile::default()
        };
        let s = Session {
            id: id.into(),
            profile: p.clone(),
            ..Session::default()
        };
        assert!(!whole(&s).is_empty());
        let s = Session {
            id: "nope".into(),
            profile: p,
            ..Session::default()
        };
        assert!(read(&s, 0, 1 << 20, &mut |_| true).is_err());
    }

    #[test]
    fn reads_in_slices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let r = item_turn(Rollout::new(t0()));
        r.write(&path);
        // A line's worth a slice gives the whole, a turn's settled events at
        // a time.
        let mut slices = 0;
        let mut got = Vec::new();
        read(&at(&path), 0, 1, &mut |evs| {
            slices += 1;
            got.extend(evs);
            true
        })
        .unwrap();
        assert!(slices > 5, "{slices} slices");
        assert_eq!(describe_all(&got), item_turn_want());
        // Stopped after the first.
        let mut n = 0;
        read(&at(&path), 0, 1, &mut |_| {
            n += 1;
            false
        })
        .unwrap();
        assert_eq!(n, 1);
    }

    fn two_turns(with_context: bool) -> Rollout {
        let turn = |r: Rollout, said: &str, answer: &str| {
            let r = if with_context {
                r.add(
                    "turn_context",
                    json!({"model": "gpt-5", "approval_policy": "never"}),
                )
            } else {
                r
            };
            let r = r
                .event("task_started", json!({}))
                .response("message", user_msg(&[said]));
            let r = if with_context {
                r.response("function_call", json!({"id": "f", "call_id": format!("c-{said}"), "name": "exec_command", "arguments": r#"{"cmd":"ls"}"#}))
                    .response("function_call_output", json!({"id": "fo", "call_id": format!("c-{said}"), "output": "a.go"}))
            } else {
                r
            };
            r.response("message", json!({"id": "a", "role": "assistant", "content": [{"type": "output_text", "text": answer}]}))
                .event("task_complete", json!({}))
        };
        let r = Rollout::new(t0()).add("session_meta", meta("thread-9", "cli".into()));
        let r = if with_context {
            r
        } else {
            r.add(
                "turn_context",
                json!({"model": "gpt-5", "approval_policy": "never"}),
            )
        };
        turn(turn(r, "first", "one"), "second", "two")
    }

    // rollout_test.go:TestHistoryTail: a rollout read from part way through
    // a turn keeps the thread's meta and reads its last turn as the whole
    // rollout does.
    #[test]
    fn tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let r = two_turns(true);
        r.write(&path);
        let whole = history(&r, 0);
        let all = r.text();
        let most = (all.len() - all.find(r#""c-first""#).unwrap()) as i64;
        let (evs, cut) = tail_before(&at(&path), most, 0).unwrap();
        assert!(cut);
        let got = describe_all(&evs);
        assert!(
            got[0].starts_with("init thread-9") && !got.join("\n").contains("text:first"),
            "{got:?}"
        );
        assert_eq!(got[got.len() - 5..], whole[whole.len() - 5..]);
    }

    // rollout_test.go:TestHistoryTailModel: a tail that starts after the
    // rollout's only turn_context still says the model.
    #[test]
    fn tail_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let r = two_turns(false);
        r.write(&path);
        let all = r.text();
        let most = (all.len() - all.find(r#""second""#).unwrap()) as i64;
        let (evs, cut) = tail_before(&at(&path), most, 0).unwrap();
        assert!(cut);
        assert!(
            describe(&evs[0]).starts_with("init thread-9 gpt-5"),
            "{:?}",
            describe_all(&evs)
        );
    }

    // history_cutoff_test.go:TestRolloutCutoffSeeksExactBoundary
    #[test]
    fn cutoff_seeks_exact_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let mut content = String::new();
        let mut offsets = Vec::new();
        for i in 0..100i64 {
            offsets.push(content.len() as i64);
            content += &format!(
                "{{\"timestamp\":{:?},\"type\":\"response_item\",\"payload\":{{\"padding\":{:?}}}}}\n",
                rush_core::time::format_rfc3339(t0() + i * 1000),
                "x".repeat(i as usize * 111)
            );
        }
        std::fs::write(&path, &content).unwrap();
        let f = File::open(&path).unwrap();
        for i in 0..=100i64 {
            let want = if i < 100 {
                offsets[i as usize]
            } else {
                content.len() as i64
            };
            let got = rollout_cutoff(&f, content.len() as i64, t0() + i * 1000);
            assert_eq!(got, Some(want), "cut {i}");
        }
    }

    #[test]
    fn tail_before_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let r = two_turns(true);
        r.write(&path);
        // Before the second turn's task_started (line 8), read whole.
        let before = r.at(8);
        let (evs, _) = tail_before(&at(&path), 1 << 20, before).unwrap();
        assert_eq!(describe_all(&evs), history(&r, before));
    }

    // rollout_test.go:TestFollowHistory: following a rollout as it's
    // written reads only what's new, and in all gives what a whole read
    // would.
    #[test]
    fn follows_as_it_grows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let r = item_turn(Rollout::new(t0()));
        let whole = r.text();
        let last = r.lines.last().unwrap().len();
        let half = whole.len() - last / 2; // the last line still being written
        std::fs::write(&path, &whole[..half]).unwrap();
        let mut f = follow(&at(&path)).unwrap();
        assert_eq!(f.wait_on().paths, vec![path.clone()]);
        let mut got = Vec::new();
        loop {
            let sl = f.next(512).unwrap();
            assert!(!sl.reset);
            got.extend(sl.events);
            if !sl.more {
                break;
            }
        }
        // The turn hasn't ended: what's settled of it so far.
        let want = item_turn_want();
        assert_eq!(describe_all(&got), want[..got.len()].to_vec());
        assert!(got.len() >= 10, "{}", got.len());
        std::fs::write(&path, &whole).unwrap();
        let sl = f.next(1 << 20).unwrap();
        got.extend(sl.events);
        assert_eq!(describe_all(&got), want);
        assert!(f.next(1 << 20).unwrap().events.is_empty());
        // Rewritten shorter: it starts over.
        std::fs::write(&path, &whole[..whole.len() / 2]).unwrap();
        let sl = f.next(1 << 20).unwrap();
        assert!(sl.reset && !sl.events.is_empty());
        assert!(describe(&sl.events[0]).starts_with("init thread-1"));
    }
}
