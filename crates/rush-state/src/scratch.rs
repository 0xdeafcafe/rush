//! Where sessions' scratch folders ($TMPDIR) are: one folder per project,
//! one per session in it (<root>/<project>/<id>). New sessions' are under
//! the system's /tmp (`rush/`), which the system empties at reboot and of
//! what's gone unused for days, so scratch never piles up in rush's
//! folder. Sessions started before that keep theirs in rush's folder
//! (`tmp/`) until they're done, and what's there is cleaned as it was:
//! everything that cleans or measures scratch asks [`scratch_roots`] for
//! both.
//!
//! No Go original: Go kept every session's scratch in rush's folder
//! (internal/host/tmp.go).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    /// This thread's stand-in for the system's /tmp: for tests.
    static THREAD_SYSTEM_TMP: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Sets this thread's stand-in for the system's /tmp; None goes back to
/// the real one (or, in a test, one in its own folder). For tests.
pub fn set_thread_system_tmp(dir: Option<PathBuf>) {
    THREAD_SYSTEM_TMP.with(|d| *d.borrow_mut() = dir);
}

/// The system's temp folder as its real path (/private/tmp on macOS, so
/// a path read back from a process compares equal), or /tmp when that
/// can't be read. A test's is in its own folder: a thread's rush folder
/// (`systmp` beside what it holds), else the throwaway home's.
pub fn system_tmp() -> PathBuf {
    if let Some(d) = THREAD_SYSTEM_TMP.with(|d| d.borrow().clone()) {
        return d;
    }
    if let (Some(d), _) = crate::dir::thread_dirs() {
        return d.join("systmp");
    }
    if let Some(r) = rush_core::sandbox::root() {
        return r.join("systmp");
    }
    std::fs::canonicalize("/tmp").unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Where new sessions' scratch folders go: rush/ in the system's /tmp.
/// Only once [`make_scratch_root`] says it's rush's own.
pub fn scratch_root() -> PathBuf {
    system_tmp().join("rush")
}

/// Where sessions started before scratch moved to /tmp keep theirs:
/// tmp/ in rush's folder. Cleaned by the same rules until it's empty.
pub fn old_scratch_root() -> PathBuf {
    crate::dir().join("tmp")
}

/// Every folder sessions' scratch folders are two below, the new one
/// first: what cleaning, trimming and measuring scratch look in.
pub fn scratch_roots() -> Vec<PathBuf> {
    vec![scratch_root(), old_scratch_root()]
}

/// The scratch root p is, or is in, by its text (p made clean first).
pub fn scratch_root_of(p: &Path) -> Option<PathBuf> {
    let p = clean(p);
    scratch_roots().into_iter().find(|r| p.starts_with(r))
}

/// Whether p is a real folder (not a link) that uid owns and only uid may
/// reach: what rush's root in a shared /tmp must be before a session's
/// scratch goes in it.
pub fn owned_by(p: &Path, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(p)
        .is_ok_and(|m| m.file_type().is_dir() && m.uid() == uid && m.mode() & 0o077 == 0)
}

/// The scratch root in /tmp, made owner-only (0700) when it isn't there,
/// or None when it can't be rush's: /tmp is everyone's, so a folder (or a
/// link) someone else put there first is never used. One of the user's
/// own that others can reach is closed to them first.
pub fn make_scratch_root() -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let r = scratch_root();
    // SAFETY: geteuid can't fail.
    let uid = unsafe { libc::geteuid() };
    if std::fs::symlink_metadata(&r).is_err() {
        if let Some(parent) = r.parent()
            && !parent.exists()
        {
            let _ = std::fs::create_dir_all(parent); // only a test's stand-in is ever missing
        }
        // Not recursive: the last part is made here or was another's.
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&r);
    }
    let m = std::fs::symlink_metadata(&r).ok()?;
    if !m.file_type().is_dir() || m.uid() != uid {
        return None;
    }
    if m.mode() & 0o077 != 0 {
        std::fs::set_permissions(&r, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    owned_by(&r, uid).then_some(r)
}

/// p made lexically clean, as Go's filepath.Clean.
fn clean(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !p.is_absolute() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Sys(tempfile::TempDir);

    impl Sys {
        fn new() -> Sys {
            let d = tempfile::tempdir().unwrap();
            set_thread_system_tmp(Some(d.path().to_path_buf()));
            crate::set_thread_dir(Some(d.path().join("home")));
            Sys(d)
        }
    }

    impl Drop for Sys {
        fn drop(&mut self) {
            set_thread_system_tmp(None);
            crate::set_thread_dir(None);
        }
    }

    fn mode(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    // The new root first, in the system's tmp; the old in rush's folder.
    #[test]
    fn both_roots_new_first() {
        let s = Sys::new();
        let t = s.0.path();
        assert_eq!(
            scratch_roots(),
            vec![t.join("rush"), t.join("home").join("tmp")]
        );
        for (p, want) in [
            (t.join("rush/app-1/s1/"), Some(t.join("rush"))),
            (t.join("rush"), Some(t.join("rush"))),
            (t.join("home/tmp/app-1/s1"), Some(t.join("home/tmp"))),
            (t.join("rush/../home/tmp/a"), Some(t.join("home/tmp"))),
            (t.join("rushx/a"), None),
            (t.join("home/keep/a"), None),
        ] {
            assert_eq!(scratch_root_of(&p), want, "{p:?}");
        }
    }

    // Made owner-only when missing; one of ours others can reach is
    // closed; a link or a file there is never used.
    #[test]
    fn the_root_in_tmp_is_ours_or_unused() {
        type Setup = fn(&Path);
        let cases: [(&str, Setup, bool); 5] = [
            ("missing", |_| {}, true),
            (
                "ours, 0700",
                |r| {
                    std::fs::create_dir(r).unwrap();
                    std::fs::set_permissions(r, std::fs::Permissions::from_mode(0o700)).unwrap();
                },
                true,
            ),
            (
                "ours, open to all",
                |r| {
                    std::fs::create_dir(r).unwrap();
                    std::fs::set_permissions(r, std::fs::Permissions::from_mode(0o777)).unwrap();
                },
                true,
            ),
            (
                "a link",
                |r| {
                    let to = r.with_file_name("elsewhere");
                    std::fs::create_dir(&to).unwrap();
                    std::os::unix::fs::symlink(&to, r).unwrap();
                },
                false,
            ),
            ("a file", |r| std::fs::write(r, "x").unwrap(), false),
        ];
        for (what, setup, ok) in cases {
            let _s = Sys::new();
            let r = scratch_root();
            setup(&r);
            let got = make_scratch_root();
            assert_eq!(got.is_some(), ok, "{what}");
            if ok {
                assert_eq!(got.as_deref(), Some(r.as_path()), "{what}");
                assert_eq!(mode(&r), 0o700, "{what}");
            }
        }
    }

    // Someone else's folder never passes, however it's set.
    #[test]
    fn owned_by_only_the_owner() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("r");
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        // SAFETY: geteuid can't fail.
        let me = unsafe { libc::geteuid() };
        for (uid, want) in [(me, true), (me.wrapping_add(1), false)] {
            assert_eq!(owned_by(&p, uid), want, "{uid}");
        }
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(!owned_by(&p, me), "open to the group");
        assert!(!owned_by(&d.path().join("none"), me), "missing");
    }

    // A test's stand-in for /tmp is in its own folder; the real one is
    // the system's real path.
    #[test]
    fn a_tests_tmp_is_its_own() {
        let st = system_tmp();
        assert!(
            st.starts_with(rush_core::sandbox::root().unwrap()),
            "{st:?}"
        );
        set_thread_system_tmp(Some("/x".into()));
        assert_eq!(scratch_root(), Path::new("/x/rush"));
        set_thread_system_tmp(None);
    }
}
