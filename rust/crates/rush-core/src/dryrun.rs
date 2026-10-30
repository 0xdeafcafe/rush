//! The dry run: with RUSH_DRYRUN=1, what rush does by itself (ending
//! processes, removing worktrees and branches, emptying temp work,
//! compressing transcripts, switching logins, restarting or reviving
//! hosts, sending to sessions, asking the turn model) is written down
//! instead of done, one line per action with its target and why, in
//! dryrun.log in rush's folder. Rust-only: Go has none. DESIGN.md
//! "Dry run".
//!
//! Each action asks [`would`] once its own checks have passed, just
//! before it would act: true means it's a dry run, the line is written,
//! and the action skips itself. The same action on the same target is
//! written once an hour, so one asked about on every reading doesn't
//! fill the file.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::time::{Ms, format_rfc3339, now};

/// The variable that makes a run dry: set to anything but "" or "0".
pub const ENV: &str = "RUSH_DRYRUN";
/// Where the log goes instead of rush's folder, when set.
pub const LOG_ENV: &str = "RUSH_DRYRUN_LOG";

/// How long the same action on the same target isn't written again.
const QUIET: Ms = 3_600_000;

thread_local! {
    /// This thread's dry run, over RUSH_DRYRUN: for tests, which share
    /// one process's environment. The log too.
    static THREAD: std::cell::RefCell<Option<(bool, Option<PathBuf>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Sets this thread's dry run and its log, over the environment; None
/// goes back to it. For tests.
pub fn set_thread(v: Option<(bool, Option<PathBuf>)>) {
    THREAD.with(|t| *t.borrow_mut() = v);
}

fn env_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var(ENV)
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0")
    })
}

/// Whether this run is dry.
pub fn on() -> bool {
    if let Some(on) = THREAD.with(|t| t.borrow().as_ref().map(|(on, _)| *on)) {
        return on;
    }
    env_on()
}

static FOLDER: OnceLock<PathBuf> = OnceLock::new();

/// Says rush's folder, where the log goes (the program, once it knows).
pub fn set_folder(dir: PathBuf) {
    let _ = FOLDER.set(dir);
}

/// Where the lines go: RUSH_DRYRUN_LOG, else dryrun.log in rush's folder
/// (as the program said, else RUSH_HOME, else ~/.config/rush).
pub fn log_path() -> PathBuf {
    if let Some(Some(p)) = THREAD.with(|t| t.borrow().as_ref().map(|(_, p)| p.clone())) {
        return p;
    }
    if let Some(p) = std::env::var_os(LOG_ENV).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(d) = FOLDER.get() {
        return d.join("dryrun.log");
    }
    if let Some(d) = std::env::var_os("RUSH_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("dryrun.log");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("rush").join("dryrun.log")
}

/// When each action on each target was last written.
static SAID: Mutex<Option<HashMap<(String, String), Ms>>> = Mutex::new(None);

/// In a dry run: writes that action would be done to target, and why,
/// and says true, for the caller to skip it. Otherwise false: do it.
pub fn would(action: &str, target: &str, why: &str) -> bool {
    if !on() {
        return false;
    }
    let t = now();
    {
        let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
        let said = said.get_or_insert_with(HashMap::new);
        let k = (action.to_string(), target.to_string());
        if said.get(&k).is_some_and(|&at| t - at < QUIET) {
            return true;
        }
        said.insert(k, t);
    }
    let line = format!(
        "{}\t{}\t{action}\t{}\t{}\n",
        format_rfc3339(t),
        std::process::id(),
        one_line(target),
        one_line(why)
    );
    let p = log_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        let _ = f.write_all(line.as_bytes());
    }
    true
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\t', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_once_an_hour_and_only_when_dry() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("dryrun.log");
        set_thread(Some((false, Some(p.clone()))));
        assert!(!would("remove-worktree", "/r/w", "landed"));
        assert!(!p.exists());
        set_thread(Some((true, Some(p.clone()))));
        assert!(would("remove-worktree", "/r/w", "its work\tlanded"));
        assert!(would("remove-worktree", "/r/w", "again"));
        assert!(would("remove-worktree", "/r/x", "landed"));
        let s = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2, "{s}");
        let f: Vec<&str> = lines[0].split('\t').collect();
        assert_eq!(&f[2..], ["remove-worktree", "/r/w", "its work landed"]);
        set_thread(None);
    }
}
