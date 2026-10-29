//! rush's log of what it does and why, to debug by: a session restarted, a
//! message queued, a check-in sent, a command a hook rewrote, an account
//! moved off at a usage limit. Each is a line of JSON in rush's
//! events.jsonl, and a session's are in its own folder's too.
//!
//! Each line has a short id of its own, ev, and the ev of the line that led
//! to it, cause, where one did. A process rush starts learns its cause from
//! RUSH_CAUSE, as it learns its session from RUSH_SESSION, and its lines
//! carry it.
//!
//! Ported from Go `internal/events` (events.go, query.go) and the read side
//! of `rush events` / `rush why` (`cmd/rush/events.go`). Same file format as
//! Go: Go and Rust rush read and write the same log.

mod cli;
mod flags;
mod json;
mod log;
mod query;
mod since;

pub use cli::{EVENTS_USAGE, NARROW_EVENTS, events_cmd, why_cmd};
pub use log::{
    Fields, Logger, cause, dir, env, flush, id, log, path, session, set_cause, set_process,
};
pub use query::{
    BUDGET, Filter, Line, Node, age, chain, flat, format, query, query_why as why, tree,
};
pub use since::{parse_duration, parse_since};

/// What reading the log back can fail with; its text is Go's.
#[derive(Debug)]
pub enum Error {
    /// A file couldn't be opened, stat'd or read: "open PATH: no such file
    /// or directory", as Go's os errors say it.
    Io(String),
    /// No line in the log is this ev.
    NoLine(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(s) => f.write_str(s),
            Error::NoLine(ev) => write!(f, "no line in rush's log is {ev}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
pub(crate) mod testdir {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A folder of a test's own, removed when it's dropped.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "rush-events-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
