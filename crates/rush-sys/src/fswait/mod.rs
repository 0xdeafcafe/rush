//! Waits for files to grow without polling them: kqueue on macOS (and
//! FSEvents for a folder of many watched paths), inotify on Linux, a
//! quick stat loop where the kernel won't say.
//!
//! Ported from internal/fswait/fswait.go:1-40, fswait_darwin.go:1-64 and
//! fswait_other.go; the [`Watcher`] from watch_darwin.go and
//! watch_other.go. Go's `stop <-chan struct{}` is a [`Stop`] here: on macOS
//! it sits in the same kqueue as the files (on Linux, in the same poll as
//! the inotify descriptor), so a quiet wait wakes for nothing, not even to
//! check whether it should stop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[cfg(target_os = "macos")]
mod darwin;
#[cfg(target_os = "macos")]
mod deep;
#[cfg(target_os = "macos")]
mod tree;
#[cfg(target_os = "macos")]
pub use darwin::{Watcher, grown};
#[cfg(target_os = "linux")]
mod linux;
pub use deep::{Deep, Tell};
#[cfg(target_os = "linux")]
pub use linux::{Watcher, grown};

/// Elsewhere there's no stream: [`Deep::new`] is None and callers keep
/// their timer.
#[cfg(not(target_os = "macos"))]
mod deep {
    use std::path::PathBuf;

    /// What the stream calls with the folders (as given) that changed.
    pub type Tell = Box<dyn Fn(Vec<PathBuf>) + Send + Sync>;

    /// A stream that never starts here.
    pub struct Deep(());

    impl Deep {
        /// None: there's no stream here.
        pub fn new(_roots: &[PathBuf], _skip: &[PathBuf], _tell: Tell) -> Option<Deep> {
            None
        }
    }
}

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

    /// Waits up to d for close, asleep until then; whether it closed.
    pub fn wait(&self, d: Duration) -> bool {
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
pub use follow::{Grows, follow, follow_grows};

/// Tests wait here until a grown call watches their file, so what they do
/// next lands after the watch starts.
#[cfg(test)]
pub(crate) mod watched {
    use std::path::{Path, PathBuf};
    use std::sync::{Condvar, Mutex};

    static W: (Mutex<Vec<PathBuf>>, Condvar) = (Mutex::new(Vec::new()), Condvar::new());

    pub(crate) fn told(files: &[super::File]) {
        W.0.lock()
            .unwrap()
            .extend(files.iter().map(|f| f.path.clone()));
        W.1.notify_all();
    }

    /// Blocks until a grown call has watched p.
    pub(crate) fn wait(p: &Path) {
        let g = W.0.lock().unwrap();
        let _g = W.1.wait_while(g, |w| !w.iter().any(|x| x == p)).unwrap();
    }
}

#[cfg(test)]
mod tests;
