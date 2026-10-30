//! Waits for files to grow without polling them: kqueue on macOS, a quick
//! stat loop elsewhere.
//!
//! Ported from internal/fswait/fswait.go:1-40, fswait_darwin.go:1-64 and
//! fswait_other.go; the [`Watcher`] from watch_darwin.go and
//! watch_other.go. Go's `stop <-chan struct{}` is a [`Stop`] here: on macOS
//! it sits in the same kqueue as the files, so a quiet wait wakes for
//! nothing, not even to check whether it should stop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[cfg(target_os = "macos")]
mod darwin;
#[cfg(target_os = "macos")]
pub use darwin::{Watcher, grown};

#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
pub use other::{Watcher, grown};

/// A file and the size it had when last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: PathBuf,
    pub size: u64,
}

/// Tells waits to give up: Go's closed `stop` channel. Once closed it
/// stays closed.
pub struct Stop {
    closed: AtomicBool,
    lock: Mutex<()>,
    cv: Condvar,
    /// A pipe whose write end goes on close: its read end then reads as
    /// ended, which kqueue reports.
    r: std::io::PipeReader,
    w: Mutex<Option<std::io::PipeWriter>>,
}

impl Stop {
    pub fn new() -> std::io::Result<Stop> {
        let (r, w) = std::io::pipe()?;
        Ok(Stop {
            closed: AtomicBool::new(false),
            lock: Mutex::new(()),
            cv: Condvar::new(),
            r,
            w: Mutex::new(Some(w)),
        })
    }

    /// Stops every wait on it, now and later.
    pub fn close(&self) {
        let _g = self.lock.lock().unwrap();
        self.closed.store(true, Ordering::SeqCst);
        self.w.lock().unwrap().take();
        self.cv.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Waits up to d for close; whether it closed.
    fn wait(&self, d: Duration) -> bool {
        let g = self.lock.lock().unwrap();
        let (_g, _) = self
            .cv
            .wait_timeout_while(g, d, |_| !self.is_closed())
            .unwrap();
        self.is_closed()
    }
}

/// Whether any file's size differs from what was read.
fn changed(files: &[File]) -> bool {
    files
        .iter()
        .any(|f| std::fs::metadata(&f.path).is_ok_and(|m| m.len() != f.size))
}

/// The fallback: a stat of each file every interval.
fn poll(stop: &Stop, files: &[File], every: Duration) -> bool {
    loop {
        if changed(files) {
            return true;
        }
        if stop.wait(every) {
            return false;
        }
    }
}

mod follow;
pub use follow::follow;

#[cfg(test)]
mod tests;
