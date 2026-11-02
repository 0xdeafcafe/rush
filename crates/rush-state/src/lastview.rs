//! Where the view was when rush last closed: the place, its page, the
//! agent session open and how far up its conversation was scrolled, and
//! when. The next rush opens that session again when it's recent (Settings
//! › General's "Reopen the last agent if closed within").
//!
//! One file under rush's folder, lastview.json, like health.json: what
//! this machine's views last had, not a setting. With several views open,
//! the last to close wins. While a view runs it also writes what it shows
//! every so often, marked open, so a view that crashed or whose terminal
//! closed still leaves a place behind; such a write never replaces what
//! another view left as it closed.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

/// What a view had open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LastView {
    /// The place showing: "Agents", "Projects", "Harnesses", "Settings".
    pub place: String,
    /// Agents' page (0 its list).
    pub page: usize,
    /// The agent session open over the list, by its key; "" for none.
    pub session: String,
    /// The session's view showing ("conversation", "overview", ...).
    pub view: String,
    /// The conversation scrolled up: the turn at the top and the rows into
    /// it; None at its end.
    pub turn: Option<u32>,
    pub off: usize,
    /// When it was written, in ms since the epoch.
    pub at: i64,
    /// The view that wrote it.
    pub pid: u32,
    /// Written while that view still ran, not as it closed.
    pub open: bool,
}

/// rush's folder's lastview.json.
pub fn path() -> PathBuf {
    crate::dir().join("lastview.json")
}

/// What the last view left, if anything readable.
pub fn load() -> Option<LastView> {
    let b = std::fs::read(path()).ok()?;
    serde_json::from_slice(&b).ok()
}

/// Keeps v, as a view closing: it replaces whatever is there.
pub fn save_closed(v: &LastView) -> io::Result<()> {
    let mut v = v.clone();
    v.open = false;
    crate::write_json(&path(), &v)
}

/// Keeps v as what a running view, started at started (ms), shows, unless
/// another view has closed since it started and left its own: the last to
/// close wins.
pub fn save_running(v: &LastView, started: i64) -> io::Result<()> {
    if let Some(had) = load()
        && !had.open
        && had.pid != v.pid
        && had.at >= started
    {
        return Ok(());
    }
    let mut v = v.clone();
    v.open = true;
    crate::write_json(&path(), &v)
}

/// Whether v is a place a view left, not one still showing in a view that
/// runs: written as it closed, or by a view that has gone since (crashed,
/// or its terminal closed).
pub fn left(v: &LastView) -> bool {
    if !v.open || v.pid == std::process::id() {
        return true;
    }
    !alive(v.pid)
}

fn alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: signal 0 only asks whether the process is there.
    let r = unsafe { libc::kill(pid as i32, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(dir: &std::path::Path) {
        crate::set_thread_dir(Some(dir.to_path_buf()));
    }

    #[test]
    fn a_closed_view_wins_over_one_still_running() {
        let d = tempfile::tempdir().unwrap();
        at(d.path());
        let mut a = LastView {
            place: "Agents".into(),
            session: "claude:x:a".into(),
            at: 1_000,
            pid: std::process::id(),
            ..Default::default()
        };
        save_closed(&a).unwrap();
        // Another view, still running, writes what it shows after.
        let b = LastView {
            session: "claude:x:b".into(),
            at: 2_000,
            pid: 999_999_999,
            ..Default::default()
        };
        save_running(&b, 500).unwrap();
        assert_eq!(load().unwrap().session, "claude:x:a");
        // It then closes: the last to close wins.
        save_closed(&b).unwrap();
        assert_eq!(load().unwrap().session, "claude:x:b");
        assert!(!load().unwrap().open);
        // A view started after that close replaces it as it runs, so
        // where it was is kept if it crashes.
        a.at = 3_000;
        a.pid = 999_999_998;
        save_running(&a, 2_500).unwrap();
        assert_eq!(load().unwrap().session, "claude:x:a");
        assert!(load().unwrap().open);
        crate::set_thread_dir(None);
    }

    #[test]
    fn a_running_view_is_not_left() {
        let me = LastView {
            open: true,
            pid: std::process::id(),
            ..Default::default()
        };
        assert!(left(&me), "this process's own write");
        let gone = LastView {
            open: true,
            pid: 999_999_999,
            ..Default::default()
        };
        assert!(left(&gone), "a view that has gone");
        let closed = LastView {
            open: false,
            pid: 1,
            ..Default::default()
        };
        assert!(left(&closed));
        let running = LastView {
            open: true,
            pid: 1, // launchd: always there
            ..Default::default()
        };
        assert!(!left(&running));
    }
}
