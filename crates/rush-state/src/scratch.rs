//! Where sessions' scratch folders ($TMPDIR) are: one folder per project,
//! one per session in it (<root>/<project>/<id>). New sessions' are under
//! the system's /tmp (`rush/`), which the system empties at reboot and of
//! what's gone unused for days, so scratch never piles up in rush's
//! folder. Sessions started before that keep theirs in rush's folder
//! (`tmp/`) until they're done, and what's there is cleaned as it was:
//! everything that cleans or measures scratch asks [`scratch_roots`] for
//! both.
//!
//! On macOS, once you've made it on Settings › Disk, scratch has an APFS
//! volume of its own (`rush-scratch`, sharing the startup disk's space),
//! with no FSEvents log, no Spotlight and no Time Machine: builds there
//! no longer flood fseventsd. New sessions' scratch goes in `rush/` on it
//! while it's mounted, owners are honoured and that folder is the user's
//! alone ([`ready_volume_root`]); else in /tmp as before. It's a scratch
//! root like the others.
//!
//! No Go original: Go kept every session's scratch in rush's folder
//! (internal/host/tmp.go).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    /// This thread's stand-in for the system's /tmp: for tests.
    static THREAD_SYSTEM_TMP: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// This thread's stand-in for the scratch volume: where it's mounted
    /// and what statfs would say of it. For tests.
    static THREAD_VOLUME: RefCell<Option<(PathBuf, Option<MountFacts>)>> = const { RefCell::new(None) };
}

/// The scratch volume's name, as diskutil makes it. No space in it: a
/// TMPDIR with one breaks every script that leaves "$TMPDIR" unquoted.
pub const VOLUME_NAME: &str = "rush-scratch";

/// Where macOS mounts the scratch volume.
pub const VOLUME_MOUNT: &str = "/Volumes/rush-scratch";

/// What statfs says of the file system a path is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountFacts {
    /// Where it's mounted (f_mntonname).
    pub on: PathBuf,
    /// Its device (f_mntfromname): /dev/disk3s7.
    pub from: String,
    /// Its kind (f_fstypename): apfs.
    pub fs: String,
    /// Whether owners and modes count (no MNT_IGNORE_OWNERSHIP).
    pub owners: bool,
}

/// Sets this thread's stand-in for the scratch volume: its mount point
/// and what statfs says there (None: nothing mounted). None goes back to
/// none in a test, the real one otherwise. For tests.
pub fn set_thread_volume(v: Option<(PathBuf, Option<MountFacts>)>) {
    THREAD_VOLUME.with(|d| *d.borrow_mut() = v);
}

/// Where the scratch volume is mounted when it is: /Volumes/rush-scratch
/// on macOS, nowhere elsewhere. A test has none unless it sets one, so no
/// test ever looks at, or cleans, the real one.
pub fn volume_mount() -> Option<PathBuf> {
    if let Some((m, _)) = THREAD_VOLUME.with(|d| d.borrow().clone()) {
        return Some(m);
    }
    if crate::dir::thread_dirs().0.is_some() || rush_core::sandbox::root().is_some() {
        return None;
    }
    cfg!(target_os = "macos").then(|| PathBuf::from(VOLUME_MOUNT))
}

/// The user's folder on the scratch volume, where sessions' scratch
/// folders go: `rush/` at its top, made by the admin script, owner-only.
pub fn volume_root() -> Option<PathBuf> {
    volume_mount().map(|m| m.join("rush"))
}

/// What statfs says of the file system p is on; None when it can't be
/// read, or off macOS. A test's stand-in says what it was given.
pub fn mount_facts(p: &Path) -> Option<MountFacts> {
    if let Some((_, f)) = THREAD_VOLUME.with(|d| d.borrow().clone()) {
        return f;
    }
    statfs_facts(p)
}

#[cfg(target_os = "macos")]
fn statfs_facts(p: &Path) -> Option<MountFacts> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    /// sys/mount.h: owners and modes on the volume are ignored.
    const MNT_IGNORE_OWNERSHIP: u32 = 0x0020_0000;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: statfs is plain data, filled in by the call.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a NUL-terminated path, st a valid statfs.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let text = |b: &[libc::c_char]| {
        // SAFETY: statfs NUL-terminates its names within their arrays.
        unsafe { CStr::from_ptr(b.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    Some(MountFacts {
        on: PathBuf::from(text(&st.f_mntonname)),
        from: text(&st.f_mntfromname),
        fs: text(&st.f_fstypename),
        owners: st.f_flags & MNT_IGNORE_OWNERSHIP == 0,
    })
}

#[cfg(not(target_os = "macos"))]
fn statfs_facts(_: &Path) -> Option<MountFacts> {
    None
}

/// Whether the volume at mount may hold sessions' scratch: an APFS
/// volume mounted right there (not an empty folder left in /Volumes),
/// owners honoured (else anyone may read what's on it), and its root, the
/// user's folder, a real folder uid owns and only uid may reach. Pure
/// but for root's mode: what [`ready_volume_root`] asks.
pub fn volume_ready(mount: &Path, facts: Option<&MountFacts>, root: &Path, uid: u32) -> bool {
    facts.is_some_and(|f| f.on == mount && f.fs == "apfs" && f.owners) && owned_by(root, uid)
}

/// The user's folder on the scratch volume when new sessions' scratch
/// may go there ([`volume_ready`]); None when there's no volume, it isn't
/// mounted, or it isn't safe to use.
pub fn ready_volume_root() -> Option<PathBuf> {
    let (mount, root) = (volume_mount()?, volume_root()?);
    // SAFETY: geteuid can't fail.
    let uid = unsafe { libc::geteuid() };
    volume_ready(&mount, mount_facts(&mount).as_ref(), &root, uid).then_some(root)
}

/// Where new sessions' scratch goes, of the roots that may hold it: the
/// volume when it's ready, else rush's root in /tmp when that's its own,
/// else rush's own folder.
pub fn pick_scratch_root(volume: Option<PathBuf>, tmp: Option<PathBuf>, old: PathBuf) -> PathBuf {
    volume.or(tmp).unwrap_or(old)
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

/// Every folder sessions' scratch folders are two below, the newest
/// first: what cleaning, trimming and measuring scratch look in. The
/// scratch volume's on macOS, whether or not it's there now (its path
/// is fixed, so a session's folder on it reads as scratch while it's
/// away); then rush's root in /tmp; then the old one in rush's folder.
pub fn scratch_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = volume_root().into_iter().collect();
    out.extend([scratch_root(), old_scratch_root()]);
    out
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
            set_thread_volume(None);
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

    // No test sees the real volume; a stand-in leads the roots.
    #[test]
    fn the_volume_is_a_root_only_where_set() {
        let s = Sys::new();
        let t = s.0.path();
        assert_eq!(volume_mount(), None);
        assert_eq!(scratch_roots().len(), 2);
        set_thread_volume(Some((t.join("vol"), None)));
        assert_eq!(
            scratch_roots(),
            vec![
                t.join("vol/rush"),
                t.join("rush"),
                t.join("home").join("tmp")
            ]
        );
        assert_eq!(
            scratch_root_of(&t.join("vol/rush/app-1/s1")),
            Some(t.join("vol/rush"))
        );
        set_thread_volume(None);
    }

    // The volume holds scratch only mounted where it should be, APFS,
    // owners honoured, and its folder the user's alone.
    #[test]
    fn the_volume_is_ready_only_when_safe() {
        let d = tempfile::tempdir().unwrap();
        let mount = d.path().join("vol");
        let root = mount.join("rush");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        // SAFETY: geteuid can't fail.
        let me = unsafe { libc::geteuid() };
        let good = MountFacts {
            on: mount.clone(),
            from: "/dev/disk3s7".into(),
            fs: "apfs".into(),
            owners: true,
        };
        let with = |f: fn(&mut MountFacts)| {
            let mut g = good.clone();
            f(&mut g);
            Some(g)
        };
        let cases: [(&str, Option<MountFacts>, u32, bool); 6] = [
            ("ready", Some(good.clone()), me, true),
            ("not mounted", None, me, false),
            (
                "a folder on another volume",
                with(|f| f.on = "/".into()),
                me,
                false,
            ),
            ("not apfs", with(|f| f.fs = "msdos".into()), me, false),
            ("owners ignored", with(|f| f.owners = false), me, false),
            (
                "another user's folder",
                Some(good.clone()),
                me.wrapping_add(1),
                false,
            ),
        ];
        for (what, facts, uid, want) in cases {
            assert_eq!(
                volume_ready(&mount, facts.as_ref(), &root, uid),
                want,
                "{what}"
            );
        }
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            !volume_ready(&mount, Some(&good), &root, me),
            "open to others"
        );
    }

    // New sessions' scratch: the volume when ready, else /tmp's when
    // it's rush's, else rush's folder; through the stand-ins too.
    #[test]
    fn new_scratch_goes_on_the_volume_when_ready() {
        let (v, t, o) = (
            PathBuf::from("/v"),
            PathBuf::from("/t"),
            PathBuf::from("/o"),
        );
        for (vol, tmp, want) in [
            (Some(v.clone()), Some(t.clone()), &v),
            (None, Some(t.clone()), &t),
            (Some(v.clone()), None, &v),
            (None, None, &o),
        ] {
            assert_eq!(&pick_scratch_root(vol, tmp, o.clone()), want);
        }
        let s = Sys::new();
        let mount = s.0.path().join("vol");
        std::fs::create_dir_all(mount.join("rush")).unwrap();
        std::fs::set_permissions(mount.join("rush"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let facts = MountFacts {
            on: mount.clone(),
            fs: "apfs".into(),
            owners: true,
            ..MountFacts::default()
        };
        set_thread_volume(Some((mount.clone(), None)));
        assert_eq!(ready_volume_root(), None, "not mounted");
        set_thread_volume(Some((mount.clone(), Some(facts))));
        assert_eq!(ready_volume_root(), Some(mount.join("rush")));
        set_thread_volume(None);
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
