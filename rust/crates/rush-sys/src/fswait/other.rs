//! Without kqueue: a quick stat loop, and a watcher that can't tell.
//!
//! Ported from internal/fswait/fswait_other.go and watch_other.go.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{File, Stop, poll};

/// Blocks until one of files differs in size from what was read, or stop
/// closes; it reports which.
pub fn grown(stop: &Stop, files: &[File]) -> bool {
    poll(stop, files, Duration::from_millis(25))
}

/// A watcher, elsewhere, can't tell: every ask is a change, of everything.
#[derive(Default)]
pub struct Watcher;

impl Watcher {
    pub fn new() -> Watcher {
        Watcher
    }
    pub fn watch<P: AsRef<Path>>(&self, _: &[P]) {}
    pub fn watching(&self, _: impl AsRef<Path>) -> bool {
        false
    }
    pub fn changed(&self) -> bool {
        true
    }
    pub fn changes(&self) -> (HashSet<PathBuf>, bool) {
        (HashSet::new(), true)
    }
    pub fn close(&self) {}
}
