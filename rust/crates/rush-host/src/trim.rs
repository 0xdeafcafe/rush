//! A running session's tmp folder only grows: every build, test run and
//! clone it makes as scratch stays until the session is cleaned, which
//! rush won't do while it runs. When the setting is on and the disk runs
//! low, the host clears out its own session's old tmp: what's been
//! untouched for TRIM_AGE and is open in no process.
//!
//! Ported from internal/host/trim.go.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// How often the host looks whether to trim.
pub const TRIM_EVERY: Duration = Duration::from_secs(10 * 60);
const TRIM_AGE: Duration = Duration::from_secs(3 * 3600);
/// The disk is low with less than this free, or less than a LOW_SHARE'th
/// of it.
const LOW_DISK: u64 = 20 << 30;
const LOW_SHARE: u64 = 10;

/// Whether the disk dir is on is running out.
pub fn disk_low(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: statfs is plain data, filled in by the call.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a NUL-terminated path, st a valid statfs.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    let bsize = st.f_bsize as u64;
    let (free, total) = (st.f_bavail as u64 * bsize, st.f_blocks as u64 * bsize);
    free < LOW_DISK || free * LOW_SHARE < total
}

/// The entries of dir that a process has open, or works in: their names,
/// as they are in dir. None when that can't be told.
pub fn held_under(dir: &Path) -> Option<HashSet<String>> {
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc::getuid() };
    let out = std::process::Command::new("lsof")
        .args(["-n", "-Fn", "-u", &uid.to_string()])
        .stderr(std::process::Stdio::null())
        .output();
    let out = match out {
        Ok(o) if o.status.success() || !o.stdout.is_empty() => o.stdout,
        _ => return None,
    };
    // lsof names files by their real path.
    let real = std::fs::canonicalize(dir).ok()?;
    let prefix = format!("n{}/", real.to_string_lossy());
    let mut held = HashSet::new();
    for line in String::from_utf8_lossy(&out).lines() {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        let name = rest.split('/').next().unwrap_or(rest);
        held.insert(name.to_string());
    }
    Some(held)
}

/// Removes the entries of dir that are older than TRIM_AGE and not held.
/// Claude Code's own folders (claude-<uid>: its task output and diffs)
/// stay, as the session reads them back.
pub fn trim_tmp(dir: &Path, now: SystemTime, held: &HashSet<String>) -> usize {
    if !crate::tmp::is_temp_dir(dir) {
        return 0; // only ever a session's own tmp folder
    }
    let Ok(ents) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for e in ents.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if held.contains(&name) || name.starts_with("claude-") {
            continue;
        }
        let Ok(m) = e.metadata() else { continue };
        let Ok(mt) = m.modified() else { continue };
        if now.duration_since(mt).unwrap_or_default() < TRIM_AGE {
            continue;
        }
        let p = e.path();
        let gone = if m.is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        if gone.is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_mtime(p: &Path, t: SystemTime) {
        let f = std::fs::File::open(p).unwrap();
        f.set_modified(t).unwrap();
    }

    // trim_test.go:10
    #[test]
    fn trim_keeps_whats_in_use() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("tmp");
        let old = SystemTime::now() - Duration::from_secs(4 * 3600);
        for n in ["old-build", "held-build", "claude-502", "fresh-build"] {
            let p = dir.join(n);
            std::fs::create_dir_all(p.join("deep")).unwrap();
            if n != "fresh-build" {
                set_mtime(&p, old);
            }
        }
        let held: HashSet<String> = ["held-build".to_string()].into();
        assert_eq!(trim_tmp(&dir, SystemTime::now(), &held), 1);
        for (n, want) in [
            ("old-build", false),
            ("held-build", true),
            ("claude-502", true),
            ("fresh-build", true),
        ] {
            assert_eq!(dir.join(n).exists(), want, "{n}");
        }
        assert_eq!(
            trim_tmp(base.path(), SystemTime::now(), &HashSet::new()),
            0,
            "trimmed a folder that isn't a session's tmp"
        );
    }

    // trim_test.go:37
    #[test]
    fn held_under_sees_open_files() {
        let dir = tempfile::tempdir().unwrap(); // on macOS, under /var, a link to /private/var
        let p = dir.path().join("busy").join("deep").join("f");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let _f = std::fs::File::create(&p).unwrap();
        let Some(held) = held_under(dir.path()) else {
            return; // lsof isn't here
        };
        assert!(
            held.contains("busy"),
            "an open file's folder wasn't held: {held:?}"
        );
    }
}
