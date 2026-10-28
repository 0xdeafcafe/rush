//! rush's folder and its cache folder.
//! Ported from internal/state/state.go:24-65 (Dir, pick, Key) and
//! :873-886 (cacheDir, CachePath), profiles.go:221-232 (ExpandHome).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

thread_local! {
    /// This thread's rush folder and cache folder, over RUSH_HOME and
    /// RUSH_CACHE: for tests, which run side by side in one process where
    /// the environment is shared.
    static THREAD_DIRS: RefCell<(Option<PathBuf>, Option<PathBuf>)> = const { RefCell::new((None, None)) };
}

/// Sets this thread's rush folder, over RUSH_HOME; None goes back to it.
/// For tests: the environment is the whole process's.
pub fn set_thread_dir(dir: Option<PathBuf>) {
    THREAD_DIRS.with(|d| d.borrow_mut().0 = dir);
}

/// Sets this thread's cache folder, over RUSH_CACHE; None goes back to it.
pub fn set_thread_cache_dir(dir: Option<PathBuf>) {
    THREAD_DIRS.with(|d| d.borrow_mut().1 = dir);
}

/// Dir's last answer and the home it was for: it's asked for on every
/// reading, and which folder is in use changes only when it's copied over,
/// between runs.
static LAST_DIR: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// rush's folder.
pub fn dir() -> PathBuf {
    if let Some(d) = THREAD_DIRS.with(|d| d.borrow().0.clone()) {
        return d;
    }
    if let Some(d) = std::env::var_os("RUSH_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = home_dir();
    let mut last = LAST_DIR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((h, d)) = last.as_ref()
        && *h == home
    {
        return d.clone(); // the same home as last time
    }
    let d = pick(
        &home.join(".config").join("rush"),
        &home.join(".config").join("agtop"),
        "config.json",
    );
    *last = Some((home, d.clone()));
    d
}

/// rush's folder, or the one it had as agtop while that's the one in use
/// (rush's has no mark in it yet): nothing is moved, so what runs from
/// there keeps working. Copy agtop's to rush's to switch.
pub fn pick(rush: &Path, agtop: &Path, mark: &str) -> PathBuf {
    if std::fs::metadata(join(rush, mark)).is_err() && std::fs::metadata(join(agtop, mark)).is_ok()
    {
        return agtop.to_path_buf();
    }
    rush.to_path_buf()
}

fn join(d: &Path, name: &str) -> PathBuf {
    if name.is_empty() {
        d.to_path_buf()
    } else {
        d.join(name)
    }
}

/// Key identifies a job across accounts.
pub fn key(account: &str, id: &str) -> String {
    format!("{account}/{id}")
}

/// rush's cache folder: what can be worked out again, but is kept so a
/// restart needn't.
pub fn cache_dir() -> PathBuf {
    if let Some(d) = THREAD_DIRS.with(|d| d.borrow().1.clone()) {
        return d;
    }
    if let Some(d) = std::env::var_os("RUSH_CACHE").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    match user_cache_dir() {
        Some(d) => pick(&d.join("rush"), &d.join("agtop"), ""),
        None => dir(),
    }
}

/// os.UserCacheDir: ~/Library/Caches on macOS, else $XDG_CACHE_HOME or
/// ~/.cache.
fn user_cache_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let h = home_dir();
        return (!h.as_os_str().is_empty()).then(|| h.join("Library").join("Caches"));
    }
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME").filter(|x| !x.is_empty()) {
        return Some(PathBuf::from(x));
    }
    let h = home_dir();
    (!h.as_os_str().is_empty()).then(|| h.join(".cache"))
}

/// A file in rush's cache folder.
pub fn cache_path(name: &str) -> PathBuf {
    cache_dir().join(name)
}

/// Turns a leading ~ into the home folder.
pub fn expand_home(p: &str) -> String {
    if p != "~" && !p.starts_with("~/") {
        return p.to_string();
    }
    let home = home_dir();
    if home.as_os_str().is_empty() {
        return p.to_string();
    }
    let rest = p.trim_start_matches('~').trim_start_matches('/');
    if rest.is_empty() {
        return home.to_string_lossy().into_owned();
    }
    home.join(rest).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // agtop's folder stays in use until rush's has a config in it.
    #[test]
    fn pick_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let (rush, agtop) = (tmp.path().join("rush"), tmp.path().join("agtop"));
        assert_eq!(pick(&rush, &agtop, "config.json"), rush, "neither");
        std::fs::create_dir_all(&agtop).unwrap();
        std::fs::write(agtop.join("config.json"), "{}").unwrap();
        std::fs::create_dir_all(rush.join("sessions")).unwrap(); // made by something else
        assert_eq!(pick(&rush, &agtop, "config.json"), agtop, "agtop's");
        std::fs::write(rush.join("config.json"), "{}").unwrap();
        assert_eq!(pick(&rush, &agtop, "config.json"), rush, "copied");
        assert!(
            std::fs::symlink_metadata(&agtop).is_ok(),
            "agtop's was touched"
        );
    }
}
