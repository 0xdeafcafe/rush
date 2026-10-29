//! How much disk folders take, and what's yours at the top of /tmp. Go:
//! internal/fleet/temp.go:53 (DiskUsage), diskusage.go:12 (statUsage)
//! and scratch.go (Scratch :23, StaleScratch :36, FindScratch :54,
//! ClearScratch :68, scratchEntries :91, scratchWalk :126). They walk the
//! disk: never on the UI thread.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::SystemTime;

use rush_core::time::{Ms, now};

/// How long something in /tmp goes untouched before it's taken as left
/// behind.
pub const SCRATCH_IDLE: Ms = 24 * 3600 * 1000;

/// Whether path is root or inside it.
pub fn within(root: &str, path: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root || path.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

/// How much disk the paths take, counted as du does (allocated blocks),
/// without following links.
pub fn disk_usage(paths: &[&str]) -> i64 {
    let mut n = 0;
    for p in paths {
        let Ok(st) = std::fs::symlink_metadata(p) else {
            continue;
        };
        n += st.blocks() as i64 * 512;
        if st.is_dir() {
            n += dir_usage(Path::new(p));
        }
    }
    n
}

fn dir_usage(dir: &Path) -> i64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let Ok(st) = e.metadata() else { continue };
        n += st.blocks() as i64 * 512;
        if st.is_dir() {
            n += dir_usage(&e.path());
        }
    }
    n
}

/// What's yours at the top of /tmp: Stale is what nothing has touched,
/// anywhere inside, for SCRATCH_IDLE.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scratch {
    pub items: usize,
    pub stale_items: usize,
    pub size: i64,
    pub stale: i64,
    /// When it was looked at; 0 before.
    pub checked: Ms,
}

/// One thing of yours at the top of /tmp.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScratchItem {
    pub path: String,
    pub size: i64,
}

/// Walks your entries at the top of dir (/tmp), skipping the agents' own
/// scratch roots by name (cleaned with their sessions).
pub fn find_scratch(dir: &str, own: &[String]) -> Scratch {
    let mut s = Scratch {
        checked: now(),
        ..Scratch::default()
    };
    for p in scratch_entries(dir, own) {
        let (size, newest) = scratch_walk(&p);
        s.items += 1;
        s.size += size;
        if now() - newest >= SCRATCH_IDLE {
            s.stale_items += 1;
            s.stale += size;
        }
    }
    s
}

/// What clear_scratch would remove now, biggest first.
pub fn stale_scratch(dir: &str, own: &[String]) -> Vec<ScratchItem> {
    let mut out: Vec<ScratchItem> = scratch_entries(dir, own)
        .into_iter()
        .filter_map(|p| {
            let (size, newest) = scratch_walk(&p);
            (now() - newest >= SCRATCH_IDLE).then_some(ScratchItem { path: p, size })
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.size));
    out
}

/// Removes what's stale, looking again at each first: anything touched
/// since stays. What it freed, how many, and the first error.
pub fn clear_scratch(dir: &str, own: &[String]) -> (i64, usize, Option<String>) {
    let (mut freed, mut n, mut err) = (0, 0, None);
    for p in scratch_entries(dir, own) {
        let (size, newest) = scratch_walk(&p);
        if now() - newest < SCRATCH_IDLE {
            continue;
        }
        let r = if Path::new(&p).is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        if let Err(e) = r {
            err.get_or_insert(e.to_string());
            continue;
        }
        freed += size;
        n += 1;
    }
    (freed, n, err)
}

/// Your plain files and folders at the top of dir, but for the agents'
/// own scratch roots and tmux's.
fn scratch_entries(dir: &str, own: &[String]) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if own.contains(&name) || name.starts_with("tmux-") {
            continue;
        }
        let Ok(st) = e.metadata() else { continue };
        let ft = st.file_type();
        if !(ft.is_dir() || ft.is_file()) || st.uid() != uid {
            continue;
        }
        out.push(format!("{}/{name}", dir.trim_end_matches('/')));
    }
    out.sort();
    out
}

/// How much disk p takes, and when anything in it last changed (ms).
fn scratch_walk(p: &str) -> (i64, Ms) {
    let (mut size, mut newest) = (0i64, 0 as Ms);
    let mut stack = vec![std::path::PathBuf::from(p)];
    while let Some(d) = stack.pop() {
        let Ok(st) = std::fs::symlink_metadata(&d) else {
            continue;
        };
        size += st.blocks() as i64 * 512;
        if let Ok(t) = st.modified() {
            newest = newest.max(ms_of(t));
        }
        if st.is_dir()
            && let Ok(rd) = std::fs::read_dir(&d)
        {
            stack.extend(rd.flatten().map(|e| e.path()));
        }
    }
    (size, newest)
}

/// A file time in ms since the epoch.
pub fn ms_of(t: SystemTime) -> Ms {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as Ms)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_a_folder() {
        assert!(within("/a/b", "/a/b"));
        assert!(within("/a/b/", "/a/b/c"));
        assert!(!within("/a/b", "/a/bc"));
    }

    #[test]
    fn scratch_sized_and_aged() {
        let d = std::env::temp_dir().join(format!("rush-scratch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("fresh")).unwrap();
        std::fs::write(d.join("fresh/x"), vec![0u8; 8192]).unwrap();
        std::fs::create_dir_all(d.join("tmux-501")).unwrap();
        std::fs::create_dir_all(d.join("claude")).unwrap();
        let dir = d.to_string_lossy().into_owned();
        let s = find_scratch(&dir, &["claude".into()]);
        assert_eq!(s.items, 1);
        assert_eq!(s.stale_items, 0);
        assert!(s.size >= 8192);
        assert!(stale_scratch(&dir, &[]).is_empty());
        assert!(disk_usage(&[dir.as_str()]) >= 8192);
        let _ = std::fs::remove_dir_all(&d);
    }
}
