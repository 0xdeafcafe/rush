//! Sessions' kept folders ($RUSH_KEEP): what a session made that it or you
//! may want to look at again (screenshots, outputs, logs, reports), apart
//! from its scratch ($TMPDIR), which goes whenever it's in nothing's way.
//! One folder per project, one per session in it (keep/<project>/<id>),
//! in a tree of their own so nothing that trims or cleans tmp reaches it.
//! A kept folder stays when its session is done, and goes KEEP_FOR after
//! nothing in it was last touched; with the disk nearly full (under
//! KEEP_LOW free) the oldest go first, until it isn't.
//!
//! No Go original: new in the Rust port.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::paths::dir;
use crate::tmp::{project_of, slug};

/// How long a kept folder nothing has touched stays.
pub const KEEP_FOR: std::time::Duration = std::time::Duration::from_secs(30 * 86_400);
/// With less than this free, kept folders go oldest first until there's
/// this much: only once scratch has gone, as the clean-up empties it first.
pub const KEEP_LOW: u64 = 5 << 30;

/// Holds sessions' kept folders.
pub fn keep_root() -> PathBuf {
    rush_state::dir().join("keep")
}

/// Session id's kept folder, when it has one: its session's folder links
/// to it (keep).
pub fn keep_dir(id: &str) -> Option<PathBuf> {
    std::fs::read_link(dir(id).join("keep")).ok()
}

/// Session id's kept folder, made for work in cwd's project when it has
/// none. Once made it stays where it is, wherever the session goes: its
/// conversation names the files in it by path. One removed is made again
/// where it was.
pub fn place_keep(id: &str, cwd: &str) -> PathBuf {
    if let Some(p) = keep_dir(id)
        && p.parent().and_then(Path::parent) == Some(keep_root().as_path())
    {
        let _ = mkdir(&p);
        return p;
    }
    let proj = project_of(cwd);
    let group = if proj.is_empty() {
        "elsewhere".to_string()
    } else {
        slug(&proj)
    };
    let want = keep_root().join(group).join(id);
    let _ = mkdir(&want);
    let link = dir(id).join("keep");
    let _ = std::fs::remove_file(&link);
    let _ = std::os::unix::fs::symlink(&want, &link);
    want
}

fn mkdir(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(p)
}

/// One session's kept folder, and when it or anything at its top was last
/// changed.
#[derive(Clone, Debug)]
pub struct Kept {
    pub id: String,
    pub path: PathBuf,
    pub touched: SystemTime,
}

/// Every session's kept folder.
pub fn kept() -> Vec<Kept> {
    let mut out = Vec::new();
    let Ok(groups) = std::fs::read_dir(keep_root()) else {
        return out;
    };
    for g in groups.flatten() {
        if !g.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(ents) = std::fs::read_dir(g.path()) else {
            continue;
        };
        for e in ents.flatten() {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = e.path();
            let Some(touched) = touched(&path) else {
                continue; // one that can't be read can't be said idle
            };
            out.push(Kept {
                id: e.file_name().to_string_lossy().into_owned(),
                path,
                touched,
            });
        }
    }
    out
}

/// The newest change to p or anything at its top, or None when one can't
/// be read.
fn touched(p: &Path) -> Option<SystemTime> {
    let mut t = std::fs::metadata(p).ok()?.modified().ok()?;
    for e in std::fs::read_dir(p).ok()? {
        let m = e.ok()?.metadata().ok()?.modified().ok()?;
        t = t.max(m);
    }
    Some(t)
}

/// What expire_kept took: how many folders, and their bytes as size said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Expired {
    pub removed: usize,
    pub bytes: i64,
}

/// Removes kept folders of sessions not running and open in nothing:
/// those untouched for KEEP_FOR, then, while free() is under KEEP_LOW, the
/// rest oldest first. running says whether a session's host runs it; size
/// measures a folder before it goes.
pub fn expire_kept(
    now: SystemTime,
    running: &dyn Fn(&str) -> bool,
    free: &dyn Fn() -> u64,
    size: &dyn Fn(&Path) -> i64,
) -> Expired {
    let mut out = Expired::default();
    let root = keep_root();
    if std::fs::metadata(&root).is_err() {
        return out;
    }
    let Some(held) = crate::trim::held_under_tree(&root) else {
        return out; // without knowing what's open, nothing is safe to remove
    };
    let mut all: Vec<Kept> = kept()
        .into_iter()
        .filter(|k| !running(&k.id) && !held.contains(&k.path))
        .collect();
    all.sort_by_key(|k| k.touched);
    let mut low = free() < KEEP_LOW;
    for k in all {
        let old = now.duration_since(k.touched).unwrap_or_default() >= KEEP_FOR;
        if !old && !low {
            continue;
        }
        let why = if old {
            "a session's kept files, untouched for 30 days"
        } else {
            "a session's kept files, the oldest, with under 5G free"
        };
        if rush_core::dryrun::would("expire-kept", &k.path.to_string_lossy(), why) {
            continue;
        }
        let bytes = size(&k.path);
        if std::fs::remove_dir_all(&k.path).is_ok() {
            out.removed += 1;
            out.bytes += bytes;
            let link = dir(&k.id).join("keep");
            if std::fs::read_link(&link).is_ok_and(|t| t == k.path) {
                let _ = std::fs::remove_file(&link);
            }
            if !old {
                low = free() < KEEP_LOW;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;
    use std::time::Duration;

    fn age(p: &Path, t: SystemTime) {
        std::fs::File::open(p).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn place_keep_stays_put() {
        let h = Home::new();
        crate::tmp::ignore_system_temp(true);
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (
            a.path().to_string_lossy().into_owned(),
            b.path().to_string_lossy().into_owned(),
        );
        std::fs::create_dir_all(dir("s1")).unwrap();
        let got = place_keep("s1", &a);
        assert_eq!(got, keep_root().join(slug(&a)).join("s1"));
        assert!(got.is_dir());
        assert_eq!(keep_dir("s1"), Some(got.clone()));
        assert_eq!(place_keep("s1", &b), got, "stays where it was made");
        std::fs::remove_dir_all(&got).unwrap();
        assert_eq!(place_keep("s1", &b), got, "made again where it was");
        assert!(got.is_dir());
        // Not a project's: work in it is scratch.
        assert!(crate::tmp::is_temp(&got.join("x").to_string_lossy()));
        // Nowhere to say whose: kept with the rest elsewhere.
        std::fs::create_dir_all(dir("s2")).unwrap();
        assert_eq!(
            place_keep("s2", ""),
            keep_root().join("elsewhere").join("s2")
        );
        drop(h);
        crate::tmp::ignore_system_temp(false);
    }

    #[test]
    fn expiry_keeps_whats_recent_running_or_open() {
        let _h = Home::new();
        let now = SystemTime::now();
        let make = |id: &str, ago: Duration| {
            std::fs::create_dir_all(dir(id)).unwrap();
            let p = place_keep(id, "");
            let f = p.join("shot.png");
            std::fs::write(&f, "x").unwrap();
            age(&f, now - ago);
            age(&p, now - ago);
            p
        };
        let day = Duration::from_secs(86_400);
        let old = make("old", 31 * day);
        let fresh = make("fresh", day);
        let run = make("run", 40 * day);
        let plenty = || u64::MAX;
        let r = expire_kept(now, &|id| id == "run", &plenty, &|_| 1);
        assert_eq!(
            r,
            Expired {
                removed: 1,
                bytes: 1
            }
        );
        assert!(!old.exists() && fresh.exists() && run.exists());
        assert_eq!(keep_dir("old"), None, "its link goes with it");
        // A disk nearly full takes the oldest that isn't running, and
        // stops once there's room.
        let older = make("older", 10 * day);
        let left = std::cell::Cell::new(0u64);
        let free = || left.get();
        let size = |_: &Path| {
            left.set(KEEP_LOW);
            7
        };
        let r = expire_kept(now, &|id| id == "run", &free, &size);
        assert_eq!(
            r,
            Expired {
                removed: 1,
                bytes: 7
            }
        );
        assert!(!older.exists() && fresh.exists() && run.exists());
    }

    #[test]
    fn nothing_kept_nothing_done() {
        let _h = Home::new();
        let r = expire_kept(SystemTime::now(), &|_| false, &|| 0, &|_| 1);
        assert_eq!(r, Expired::default());
    }
}
