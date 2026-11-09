//! Which thread is the UI's, so what waits (the disk, another process, a
//! socket) can say it must never run there (Rust-only; DESIGN.md
//! "Threads"). The UI loop marks its thread for each turn it hands the
//! model ([`ui_thread`]); rush's own helpers for files and programs call
//! [`check`], which in a debug build (tests included) panics when it runs
//! on a marked thread, so a blocking call put back on the UI thread fails
//! CI rather than stutter in use. A release build costs a thread-local
//! read where it's called, nothing else.

use std::cell::Cell;
use std::io;
use std::path::Path;

thread_local! {
    static ON_UI: Cell<bool> = const { Cell::new(false) };
}

/// The UI thread marked until it drops, then as it was.
#[must_use]
pub struct Mark {
    was: bool,
}

impl Drop for Mark {
    fn drop(&mut self) {
        ON_UI.with(|c| c.set(self.was));
    }
}

/// Marks this thread as the UI's while the guard lives: what the loop
/// does around each key, message, tick and frame it hands the model.
pub fn ui_thread() -> Mark {
    Mark {
        was: ON_UI.with(|c| c.replace(true)),
    }
}

/// Runs f with the mark lifted: work that stands in for a worker's where
/// there's no loop to hand it to (tests, `rush --render`), run at once.
pub fn off<R>(f: impl FnOnce() -> R) -> R {
    let was = ON_UI.with(|c| c.replace(false));
    let r = f();
    ON_UI.with(|c| c.set(was));
    r
}

/// Whether this thread is the UI's, now.
pub fn on_ui() -> bool {
    ON_UI.with(Cell::get)
}

/// Says what must not run on the UI thread: a debug build panics there.
#[track_caller]
#[inline]
pub fn check(what: &str) {
    if cfg!(debug_assertions) && on_ui() {
        panic!("{what} on the UI thread: hand it to Model::later or a worker");
    }
}

/// The disk, checked: std::fs's calls, each saying it isn't on the UI
/// thread first.
pub mod fs {
    use super::*;

    #[track_caller]
    pub fn read(p: impl AsRef<Path>) -> io::Result<Vec<u8>> {
        check("a file read");
        std::fs::read(p)
    }

    #[track_caller]
    pub fn read_to_string(p: impl AsRef<Path>) -> io::Result<String> {
        check("a file read");
        std::fs::read_to_string(p)
    }

    #[track_caller]
    pub fn metadata(p: impl AsRef<Path>) -> io::Result<std::fs::Metadata> {
        check("a file's metadata");
        std::fs::metadata(p)
    }

    #[track_caller]
    pub fn open(p: impl AsRef<Path>) -> io::Result<std::fs::File> {
        check("a file opened");
        std::fs::File::open(p)
    }

    #[track_caller]
    pub fn read_link(p: impl AsRef<Path>) -> io::Result<std::path::PathBuf> {
        check("a link read");
        std::fs::read_link(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_only_while_held() {
        assert!(!on_ui());
        {
            let _m = ui_thread();
            assert!(on_ui());
            off(|| assert!(!on_ui()));
            assert!(on_ui());
            {
                let _again = ui_thread();
            }
            assert!(on_ui(), "a nested mark restores, not clears");
        }
        assert!(!on_ui());
        check("anything"); // off the UI thread: fine
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "on the UI thread")]
    fn a_read_on_the_ui_thread_panics() {
        let _m = ui_thread();
        let _ = fs::read("/nonexistent");
    }

    #[test]
    fn other_threads_are_not_the_ui() {
        let _m = ui_thread();
        std::thread::spawn(|| assert!(!on_ui())).join().unwrap();
    }
}
