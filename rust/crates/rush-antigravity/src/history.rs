//! The history rush keeps of an Antigravity conversation, and reading it
//! back. Port of internal/adapters/antigravity/history.go:1-143.
//!
//! Rush keeps its own normalized transcript; agy's native conversations
//! are resumed by ID and are never rewritten or mistaken for Gemini CLI
//! sessions. Each line is `{"at":time,"event":<rush's event wire>}`, as Go
//! writes it, under <rush's folder>/antigravity-history/<id>.jsonl.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rush_core::adapter::{self, Follow, Slice, WaitOn};
use rush_core::event::{self, Event};
use rush_core::time::{Ms, format_rfc3339, now};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::says;

/// Where conversation id's history is under rush's folder dir; None for an
/// id that isn't a plain name.
pub(crate) fn history_path(dir: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
        return None;
    }
    Some(dir.join("antigravity-history").join(format!("{id}.jsonl")))
}

#[derive(Serialize)]
struct SavedOut<'a> {
    at: String,
    event: &'a RawValue,
}

#[derive(Deserialize)]
struct SavedIn<'a> {
    #[serde(default)]
    at: Option<String>,
    #[serde(borrow)]
    event: &'a RawValue,
}

/// One saved line for ev, with its newline.
pub(crate) fn saved_line(ev: &Event) -> adapter::Result<Vec<u8>> {
    let raw = event::marshal(ev)?;
    let raw = RawValue::from_string(raw)?;
    let mut b = serde_json::to_vec(&SavedOut {
        at: format_rfc3339(now()),
        event: &raw,
    })?;
    b.push(b'\n');
    Ok(b)
}

/// Appends a line to an open history.
pub(crate) fn append(f: &mut File, ev: &Event) -> adapter::Result<()> {
    f.write_all(&saved_line(ev)?)?;
    Ok(())
}

/// Opens (making it) the history at path, to append to.
pub(crate) fn open(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(d) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(d)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

/// What one line comes to.
enum Took {
    Event(Event),
    Skip,
    /// Written at or after the time asked for: the read ends.
    Past,
}

fn take(b: &[u8], before: Ms) -> Took {
    let Ok(s) = serde_json::from_slice::<SavedIn>(b.trim_ascii()) else {
        return Took::Skip;
    };
    if before != 0 {
        let at = s.at.as_deref().map_or(0, rush_core::wire::time::from_str);
        if at >= before {
            return Took::Past;
        }
    }
    match event::unmarshal(s.event.get().as_bytes()) {
        Ok(ev) => Took::Event(ev),
        Err(_) => Took::Skip,
    }
}

/// The history at path from byte offset on (the first whole line after
/// it, when it isn't 0), before `before` (0: all), handed to each a slice
/// at a time, each from at most most bytes.
pub(crate) fn read(
    path: &Path,
    offset: u64,
    before: Ms,
    most: usize,
    each: &mut dyn FnMut(Vec<Event>) -> bool,
) -> adapter::Result<()> {
    let mut f = File::open(path)?;
    let mut r;
    if offset > 0 {
        f.seek(SeekFrom::Start(offset - 1))?;
        r = BufReader::with_capacity(64 << 10, f);
        let mut skip = Vec::new();
        r.read_until(b'\n', &mut skip)?;
    } else {
        r = BufReader::with_capacity(64 << 10, f);
    }
    let (mut out, mut taken, mut line) = (Vec::new(), 0usize, Vec::new());
    loop {
        line.clear();
        let n = (&mut r).take(8 << 20).read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        if n >= 8 << 20 && line.last() != Some(&b'\n') {
            return Err(says("bufio.Scanner: token too long"));
        }
        taken += n;
        match take(&line, before) {
            Took::Event(ev) => out.push(ev),
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

/// Follows a history as it's appended to.
pub(crate) struct Follower {
    pub path: PathBuf,
    pub off: u64,
}

impl Follow for Follower {
    fn next(&mut self, most: usize) -> adapter::Result<Slice> {
        let mut f = File::open(&self.path)?;
        let size = f.metadata()?.len();
        let mut out = Slice::default();
        if size < self.off {
            self.off = 0; // rewritten: start over
            out.reset = true;
        }
        if size == self.off {
            return Ok(out);
        }
        f.seek(SeekFrom::Start(self.off))?;
        let want = (size - self.off).min(most.max(1) as u64);
        let mut buf = Vec::with_capacity(want as usize);
        (&mut f).take(want).read_to_end(&mut buf)?;
        let full = match buf.iter().rposition(|&c| c == b'\n') {
            Some(i) => i + 1,
            // One line longer than most: a slice of its own, once whole.
            None if (buf.len() as u64) < size - self.off => {
                BufReader::new(f).read_until(b'\n', &mut buf)?;
                if buf.last() != Some(&b'\n') {
                    return Ok(out);
                }
                buf.len()
            }
            None => return Ok(out), // a line still being written waits
        };
        for l in buf[..full].split_inclusive(|&c| c == b'\n') {
            if let Took::Event(ev) = take(l, 0) {
                out.events.push(ev);
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
