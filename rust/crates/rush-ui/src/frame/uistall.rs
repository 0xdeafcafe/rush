//! The UI thread, watched. Nothing that waits (the disk, the network,
//! another process) belongs on it: every key waits behind it. Each
//! message handled and each frame drawn is timed; one that held the
//! thread longer than STALL_AFTER is counted and noted in stalls.log, so
//! whatever still does is found by name rather than by feel. The Network
//! sheet shows the count. After Go's internal/ui/uistall.go (stallAfter
//! :26, uiBusy :52, watchUI :98, appendStall :160, stallReport :182).
//! Unlike Go, no watchdog thread ticks to catch the stack mid-stall: idle
//! costs nothing, so a stall is measured when it ends and logged with
//! what it was.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

pub const STALL_AFTER: Duration = Duration::from_millis(40);

/// One time the UI thread was held up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stall {
    /// The message's kind, or "frame".
    pub what: String,
    pub took: Duration,
    pub at: Option<SystemTime>,
}

#[derive(Default)]
struct Stalls {
    on: bool,
    n: usize,
    longest: Stall,
    last: Stall,
    log: Option<PathBuf>,
}

static STALLS: Mutex<Stalls> = Mutex::new(Stalls {
    on: false,
    n: 0,
    longest: Stall {
        what: String::new(),
        took: Duration::ZERO,
        at: None,
    },
    last: Stall {
        what: String::new(),
        took: Duration::ZERO,
        at: None,
    },
    log: None,
});

fn lock() -> std::sync::MutexGuard<'static, Stalls> {
    STALLS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts watching, stalls kept in log; safe to call more than once.
pub fn watch_ui(log: PathBuf) {
    let mut s = lock();
    s.on = true;
    s.log = Some(log);
}

/// The UI thread busy with what until the guard drops (uiBusy).
pub struct Busy {
    what: &'static str,
    began: Instant,
}

pub fn busy(what: &'static str) -> Busy {
    Busy {
        what,
        began: Instant::now(),
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        note(self.what, self.began.elapsed());
    }
}

/// Counts a stretch the UI thread was held for took, when it's a stall.
pub fn note(what: &str, took: Duration) {
    if took < STALL_AFTER {
        return;
    }
    let mut s = lock();
    if !s.on {
        return;
    }
    let st = Stall {
        what: what.to_string(),
        took,
        at: Some(SystemTime::now()),
    };
    s.n += 1;
    if took > s.longest.took {
        s.longest = st.clone();
    }
    s.last = st;
    if let Some(p) = s.log.clone() {
        let line = format!("{}  {what} took {}ms\n\n", stamp(), took.as_millis());
        // Written off the UI thread.
        let _ = std::thread::Builder::new()
            .name("rush-stall".into())
            .spawn(move || append(&p, &line));
    }
}

fn stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("{secs}")
}

/// Adds to stalls.log, starting it afresh past a megabyte (appendStall).
fn append(p: &PathBuf, s: &str) {
    let big = std::fs::metadata(p).is_ok_and(|m| m.len() > 1 << 20);
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true);
    if big {
        o.truncate(true);
    } else {
        o.append(true);
    }
    if let Ok(mut f) = o.open(p) {
        let _ = f.write_all(s.as_bytes());
    }
}

/// What the watch has seen, for the Network sheet (stallReport).
pub fn report() -> (usize, Stall, Stall, Option<PathBuf>) {
    let s = lock();
    (s.n, s.longest.clone(), s.last.clone(), s.log.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_stalls_once_watched() {
        let dir = std::env::temp_dir().join(format!("rush-stall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let before = report().0;
        note("quick", Duration::from_millis(1));
        watch_ui(dir.join("stalls.log"));
        note("quick", Duration::from_millis(1));
        note("frame", Duration::from_millis(80));
        let (n, longest, last, _) = report();
        assert_eq!(n, before + 1);
        assert_eq!(last.what, "frame");
        assert!(longest.took >= Duration::from_millis(80));
    }
}
