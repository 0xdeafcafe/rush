//! A Pi session file read back as the events a live session sends: the
//! branch of its tree it's on now. Port of internal/adapters/pi/history.go:
//! 1-159 (History, readSession, branch, replay, run).
//!
//! Go reads the whole file into memory and then replays the branch. Here
//! the file is read twice: once keeping only each entry's id, parent and
//! place (each line decoded whole, as Go does, so one it can't read is
//! left out the same way), to find the branch; then the branch's lines
//! are read again
//! and replayed a slice at a time, so a long session never sits whole in
//! memory. Following a session as it grows is this port's own: new
//! entries that carry on the branch are replayed as they come, and one
//! that starts another branch (you went back in /tree) reads the file
//! again, as a reset.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::time::Duration;

use rush_core::adapter::{self, Follow, Session, Slice, WaitOn};
use rush_core::event::{Compacted, Event, Init, TurnEnd};
use rush_core::time::Ms;
use rush_core::usage::TokenUsage;

use crate::discover::{Entry, session_file};
use crate::messages::reason;

/// A session with no file to read.
#[derive(Debug)]
pub(crate) struct NoFile(pub String);

impl std::fmt::Display for NoFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pi: session {} has no file", self.0)
    }
}

impl std::error::Error for NoFile {}

/// The file at s.transcript, or else the one of session s.id in
/// s.profile.
pub(crate) fn path_of(s: &Session) -> adapter::Result<String> {
    let mut path = s.transcript.clone();
    if path.is_empty() && !s.id.is_empty() {
        path = session_file(&s.profile, &s.id);
    }
    if path.is_empty() {
        return Err(Box::new(NoFile(s.id.clone())));
    }
    Ok(path)
}

/// An entry's place in the file, and what the branch needs of it.
#[derive(Clone, Debug, Default)]
struct Place {
    off: u64,
    len: usize,
    id: String,
    parent: String,
    typ: String,
    model: String,
}

/// The file as the first read found it: its header, the entries of the
/// branch it's on, and where the read stopped.
#[derive(Default)]
struct Scan {
    init: Init,
    branch: Vec<Place>,
    end: u64,
}

/// Reads path's entries' places, up to the first written at or after
/// before (0: all). whole is whether a last line without its newline is
/// left for later (following) or read as it is (Go reads it).
fn scan(path: &str, before: Ms, whole: bool) -> std::io::Result<Scan> {
    let mut r = BufReader::with_capacity(64 << 10, File::open(path)?);
    let mut out = Scan::default();
    let mut all: Vec<Place> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let (mut off, mut line) = (0u64, Vec::new());
    loop {
        line.clear();
        let n = r.read_until(b'\n', &mut line)?;
        if n == 0 || (whole && line.last() != Some(&b'\n')) {
            break;
        }
        let at = off;
        off += n as u64;
        out.end = off;
        let b = line.trim_ascii();
        if b.is_empty() {
            continue;
        }
        // Decoded whole, as Go does, so a line Go can't read (a field of
        // the wrong type in its message) is left off the branch here too;
        // only its place is kept.
        let Ok(e) = serde_json::from_slice::<Entry>(b) else {
            continue; // a line cut short while pi writes it
        };
        if e.typ == "session" {
            (out.init.session_id, out.init.cwd) = (e.id, e.cwd);
            continue;
        }
        if before != 0 {
            let t = e.time();
            if t != 0 && t >= before {
                break;
            }
        }
        if !e.id.is_empty() {
            by_id.insert(e.id.clone(), all.len());
        }
        all.push(Place {
            off: at,
            len: n,
            id: e.id,
            parent: e.parent_id,
            typ: e.typ,
            model: e.model_id,
        });
    }
    out.branch = branch(all, &by_id);
    // The first model the branch names, as Go's replay sets it.
    if let Some(p) = out
        .branch
        .iter()
        .find(|p| p.typ == "model_change" && !p.model.is_empty())
    {
        out.init.model = p.model.clone();
    }
    Ok(out)
}

/// The entries from the root to the last one written, the tip of the
/// branch the session is on: those it left by going back in its tree
/// aren't in the conversation any more.
fn branch(all: Vec<Place>, by_id: &HashMap<String, usize>) -> Vec<Place> {
    if all.is_empty() {
        return Vec::new();
    }
    let mut rev = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut i = all.len() - 1;
    loop {
        let e = all[i].clone();
        if !seen.insert(e.id.clone()) {
            break;
        }
        let parent = by_id.get(&e.parent).copied();
        let root = e.parent.is_empty();
        rev.push(e);
        match parent {
            Some(j) if !root => i = j,
            _ => break,
        }
    }
    rev.reverse();
    rev
}

/// Reads the line at p and decodes it.
fn entry_at(f: &File, p: &Place) -> Option<Entry> {
    let mut b = vec![0u8; p.len];
    f.read_exact_at(&mut b, p.off).ok()?;
    serde_json::from_slice(b.trim_ascii()).ok()
}

/// What a replay keeps of the run being read: from a message of yours to
/// the model's last word.
#[derive(Default)]
struct Run {
    open: bool,
    start: Ms,
    tokens: TokenUsage,
    cost: f64, // the session's, all told
    last: String,
}

impl Run {
    /// An entry as events: compactions and messages; the run's end where
    /// the model stopped to wait on the user.
    fn entry(&mut self, e: &Entry, out: &mut Vec<Event>) {
        match e.typ.as_str() {
            "compaction" => out.push(Event::Compacted(Compacted {
                before: e.tokens_before,
                ..Compacted::default()
            })),
            "message" if e.message.is_some() => self.message(e, out),
            _ => {}
        }
    }

    fn message(&mut self, e: &Entry, out: &mut Vec<Event>) {
        let Some(m) = &e.message else { return };
        let id = if m.role == "assistant" {
            m.id()
        } else {
            e.id.clone()
        };
        if !self.open && (m.role == "user" || m.role == "assistant") {
            (self.open, self.start, self.tokens) = (true, e.time(), TokenUsage::default());
            self.last.clear();
        }
        out.extend(m.events(&id));
        if m.role != "assistant" {
            return;
        }
        if let Some(u) = &m.usage {
            self.tokens.add(u.usage());
            self.cost += u.cost.total;
        }
        let t = m.text();
        if !t.is_empty() {
            self.last = t;
        }
        if m.stop_reason == "toolUse" || m.stop_reason.is_empty() {
            return;
        }
        let mut end = TurnEnd {
            reason: reason(&m.stop_reason),
            text: self.last.clone(),
            cost: self.cost,
            tokens: self.tokens,
            turns: 1,
            ..TurnEnd::default()
        };
        if m.stop_reason == "error" {
            end.err = m.error_message.clone();
        }
        let t = e.time();
        if self.start != 0 && t > self.start {
            end.duration = Duration::from_millis((t - self.start) as u64);
        }
        self.open = false;
        out.push(Event::TurnEnd(end));
    }
}

/// HistoryReader::history: the branch the session at path is on, written
/// before `before` (0: all), handed to each a slice at a time, each slice
/// replayed from at most `most` bytes of the file.
pub(crate) fn read(
    path: &str,
    before: Ms,
    most: usize,
    each: &mut dyn FnMut(Vec<Event>) -> bool,
) -> adapter::Result<()> {
    let sc = scan(path, before, false)?;
    let f = File::open(path)?;
    let mut out = vec![Event::Init(Box::new(sc.init))];
    let (mut run, mut taken) = (Run::default(), 0usize);
    for p in &sc.branch {
        if let Some(e) = entry_at(&f, p) {
            run.entry(&e, &mut out);
        }
        taken += p.len;
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

/// HistoryFollower::follow_history: the session at path, from its start,
/// then what's added as it grows.
pub(crate) fn follow(path: String) -> Box<dyn Follow> {
    Box::new(Follower {
        path: PathBuf::from(path),
        fresh: true,
        ..Follower::default()
    })
}

/// A session file followed.
#[derive(Default)]
struct Follower {
    path: PathBuf,
    /// Not read yet, or to be read again from the start.
    fresh: bool,
    /// Whether it has been read before: a read again is a reset.
    read: bool,
    /// The branch's entries still to replay, and the Init to send first.
    pending: VecDeque<Place>,
    init: Init,
    send_init: bool,
    /// Where the next new line starts, and the branch's tip.
    off: u64,
    tip: String,
    run: Run,
}

impl Follower {
    /// Reads the file's branch again, to replay from its start.
    fn rescan(&mut self, out: &mut Slice) -> std::io::Result<()> {
        let sc = scan(&self.path.to_string_lossy(), 0, true)?;
        self.tip = sc.branch.last().map(|p| p.id.clone()).unwrap_or_default();
        self.pending = sc.branch.into();
        self.off = sc.end;
        self.init = sc.init;
        self.send_init = true;
        self.run = Run::default();
        out.events.clear();
        out.reset = self.read;
        self.read = true;
        self.fresh = false;
        Ok(())
    }

    /// Replays pending entries until most bytes of them are read; whether
    /// any are left.
    fn drain(&mut self, f: &File, most: usize, out: &mut Slice) -> bool {
        if self.send_init {
            out.events.push(Event::Init(Box::new(self.init.clone())));
            self.send_init = false;
        }
        let mut taken = 0usize;
        while let Some(p) = self.pending.front() {
            if taken >= most.max(1) {
                return true;
            }
            if let Some(e) = entry_at(f, p) {
                self.run.entry(&e, &mut out.events);
            }
            taken += p.len;
            self.pending.pop_front();
        }
        false
    }

    /// What's been written past off: whole lines from at most most bytes,
    /// or one longer line once it's whole.
    fn fresh_lines(&self, f: &mut File, size: u64, most: usize) -> std::io::Result<Vec<u8>> {
        f.seek(SeekFrom::Start(self.off))?;
        let want = (size - self.off).min(most.max(1) as u64);
        let mut buf = Vec::with_capacity(want as usize);
        (&mut *f).take(want).read_to_end(&mut buf)?;
        match buf.iter().rposition(|&c| c == b'\n') {
            Some(i) => buf.truncate(i + 1),
            None if (buf.len() as u64) < size - self.off => {
                let mut r = BufReader::new(f);
                r.read_until(b'\n', &mut buf)?;
                if buf.last() != Some(&b'\n') {
                    buf.clear(); // still being written
                }
            }
            None => buf.clear(), // a line still being written waits
        }
        Ok(buf)
    }
}

impl Follow for Follower {
    fn next(&mut self, most: usize) -> adapter::Result<Slice> {
        let mut out = Slice::default();
        let mut f = File::open(&self.path)?;
        let size = f.metadata()?.len();
        if self.fresh || size < self.off {
            self.rescan(&mut out)?;
        }
        if self.drain(&f, most, &mut out) {
            out.more = true;
            return Ok(out);
        }
        if size <= self.off {
            return Ok(out);
        }
        let limited = size - self.off > most.max(1) as u64;
        let buf = self.fresh_lines(&mut f, size, most)?;
        let mut used = 0u64;
        for l in buf.split_inclusive(|&c| c == b'\n') {
            used += l.len() as u64;
            let b = l.trim_ascii();
            let Ok(e) = serde_json::from_slice::<Entry>(b) else {
                continue;
            };
            if e.typ == "session" {
                (self.init.session_id, self.init.cwd) = (e.id, e.cwd);
                out.events.push(Event::Init(Box::new(self.init.clone())));
                continue;
            }
            let carries_on =
                (self.tip.is_empty() && e.parent_id.is_empty()) || e.parent_id == self.tip;
            if !carries_on {
                // Another branch: read it all again, as a reset.
                self.rescan(&mut out)?;
                out.more = self.drain(&f, most, &mut out) || self.off < size;
                return Ok(out);
            }
            if !e.id.is_empty() {
                self.tip = e.id.clone();
            }
            if e.typ == "model_change" && self.init.model.is_empty() {
                self.init.model = e.model_id.clone();
                out.events.push(Event::Init(Box::new(self.init.clone())));
            }
            self.run.entry(&e, &mut out.events);
        }
        self.off += used;
        out.more = self.off < size && limited;
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
pub(crate) fn read_all(r: &str, before: Ms) -> Vec<Event> {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.jsonl");
    std::fs::write(&p, r).unwrap();
    let mut out = Vec::new();
    read(&p.to_string_lossy(), before, adapter::SLICE, &mut |evs| {
        out.extend(evs);
        true
    })
    .unwrap();
    out
}
