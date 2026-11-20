//! Where sessions' scratch folders ($TMPDIR) are: one folder per project,
//! one per session in it (<root>/<project>/<id>). New sessions' are under
//! the system's /tmp (`rush/`), which the system empties at reboot and of
//! what's gone unused for days, so scratch never piles up in rush's
//! folder. Sessions started before that keep theirs in rush's folder
//! (`tmp/`) until they're done, and what's there is cleaned as it was:
//! everything that cleans or measures scratch asks [`scratch_roots`] for
//! both.
//!
//! config.json's `scratchRoot` names a folder of the user's to have them
//! in instead (a volume of its own, say, with Spotlight and fseventsd's
//! logging off): rush keeps a folder of its own in it (`rush/`,
//! owner-only, as in /tmp), so what else is there is never taken for
//! scratch. While that can't be used (the folder missing, a volume not
//! mounted, not writable) new sessions' go where they would without it
//! ([`new_scratch_root`] says why), and it's still one of the roots.
//!
//! No Go original: Go kept every session's scratch in rush's folder
//! (internal/host/tmp.go).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

/// Every folder sessions' scratch folders are two below, the one
/// scratchRoot names first (when set, mounted or not), then /tmp's, then
/// rush's folder's: what cleaning, trimming and measuring scratch look in.
pub fn scratch_roots() -> Vec<PathBuf> {
    let mut v = Vec::with_capacity(3);
    v.extend(configured_scratch_root());
    for r in [scratch_root(), old_scratch_root()] {
        if !v.contains(&r) {
            v.push(r);
        }
    }
    v
}

/// rush's root in the folder config.json's scratchRoot names
/// (RUSH_SCRATCH_ROOT over it): `<it>/rush`. None when it's unset or
/// can't be one (see [`configured`]). By its text: it may not be there.
pub fn configured_scratch_root() -> Option<PathBuf> {
    configured().and_then(Result::ok)
}

/// Where new sessions' scratch folders go, and when scratchRoot is set
/// but can't be used now, what it asked for and why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRoot {
    pub root: PathBuf,
    pub fell_back: Option<FellBack>,
}

/// scratchRoot set and not used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FellBack {
    /// As config.json has it.
    pub wanted: String,
    pub why: String,
}

/// Where a new session's scratch folder goes: rush's folder in the one
/// scratchRoot names, made owner-only when it can be; else rush's root in
/// /tmp when it's rush's own; else rush's folder's. Never fails.
pub fn new_scratch_root() -> NewRoot {
    let fell_back = match configured() {
        None => None,
        Some(r) => match r.and_then(|r| make_configured_root(&r)) {
            Ok(root) => {
                return NewRoot {
                    root,
                    fell_back: None,
                };
            }
            Err(why) => Some(FellBack {
                wanted: setting().1,
                why,
            }),
        },
    };
    NewRoot {
        root: make_scratch_root().unwrap_or_else(old_scratch_root),
        fell_back,
    }
}

/// Makes r, rush's root in the folder scratchRoot names, when it isn't
/// there: owner-only, and only in a folder that's there (a volume not
/// mounted leaves nothing behind at its mount point). Err says why it
/// can't be used.
pub fn make_configured_root(r: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let Some(base) = r.parent() else {
        return Err("it has no parent folder".into());
    };
    match std::fs::metadata(base) {
        Err(_) => {
            return Err(format!(
                "{} isn't there (is its volume mounted?)",
                base.display()
            ));
        }
        Ok(m) if !m.is_dir() => return Err(format!("{} isn't a folder", base.display())),
        Ok(_) => {}
    }
    if std::fs::symlink_metadata(r).is_err()
        && let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(r)
    {
        return Err(format!("rush can't make {}: {e}", r.display()));
    }
    // SAFETY: geteuid can't fail.
    let uid = unsafe { libc::geteuid() };
    let m = std::fs::symlink_metadata(r).map_err(|e| format!("{}: {e}", r.display()))?;
    if !m.file_type().is_dir() || m.uid() != uid {
        return Err(format!("{} isn't a folder of rush's own", r.display()));
    }
    if m.mode() & 0o077 != 0 {
        std::fs::set_permissions(r, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("rush can't close {} to others: {e}", r.display()))?;
    }
    // Writable: a read-only volume has the folder and takes no files.
    let probe = r.join(format!(".rush-probe-{}", std::process::id()));
    match std::fs::DirBuilder::new().mode(0o700).create(&probe) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&probe);
            Ok(r.to_path_buf())
        }
        Err(e) => Err(format!("rush can't write in {}: {e}", r.display())),
    }
}

/// scratchRoot as rush's root in it, or why it can't be one; None when
/// unset. Read again only when config.json changes.
fn configured() -> Option<Result<PathBuf, String>> {
    let (key, value) = setting();
    let mut last = CONFIGURED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, r)) = last.as_ref()
        && *k == key
    {
        return r.clone();
    }
    let r = resolve_setting(&value, &crate::dir::home_dir(), &crate::dir());
    *last = Some((key, r.clone()));
    r
}

/// What config.json and the environment said, and what that made.
type Configured = (SettingKey, Option<Result<PathBuf, String>>);

static CONFIGURED: Mutex<Option<Configured>> = Mutex::new(None);

/// What a reading of scratchRoot depends on: config.json as it is on disk
/// (its path, change time, size and inode), the variable and the home.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SettingKey {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    size: u64,
    ino: u64,
    env: Option<String>,
    home: PathBuf,
}

/// config.json's file as it is and scratchRoot as it says (the variable
/// over it); its file is read only when it changed since last time.
fn setting() -> (SettingKey, String) {
    use std::os::unix::fs::MetadataExt;
    static READ: Mutex<Option<(SettingKey, String)>> = Mutex::new(None);
    let path = crate::dir().join("config.json");
    let fi = std::fs::metadata(&path).ok();
    let env = std::env::var(crate::env_name("scratchRoot"))
        .ok()
        .filter(|v| !v.is_empty());
    let key = SettingKey {
        path,
        modified: fi.as_ref().and_then(|f| f.modified().ok()),
        size: fi.as_ref().map_or(0, |f| f.len()),
        ino: fi.as_ref().map_or(0, |f| f.ino()),
        env: env.clone(),
        home: crate::dir::home_dir(),
    };
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, v)) = read.as_ref()
        && *k == key
    {
        return (key, v.clone());
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Only {
        #[serde(rename = "scratchRoot")]
        scratch_root: String,
    }
    let v = env.unwrap_or_else(|| {
        std::fs::read(&key.path)
            .ok()
            .and_then(|b| crate::json::from_slice::<Only>(&b).ok())
            .unwrap_or_default()
            .scratch_root
    });
    *read = Some((key.clone(), v.clone()));
    (key, v)
}

/// scratchRoot's value v as rush's root in it, by its text: ~ is home;
/// it must be absolute, and neither rush's folder, nor home, nor above
/// or inside either of them as rush's root (`~/.config` would make rush's
/// own folder scratch, and scratch is what's cleaned). None when unset.
pub fn resolve_setting(v: &str, home: &Path, rush_dir: &Path) -> Option<Result<PathBuf, String>> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    let expanded = if v == "~" || v.starts_with("~/") {
        if home.as_os_str().is_empty() {
            return Some(Err("~ is no folder: HOME isn't set".into()));
        }
        home.join(v[1..].trim_start_matches('/'))
    } else {
        PathBuf::from(v)
    };
    if !expanded.is_absolute() {
        return Some(Err(format!("{v} isn't a whole path: start it with / or ~")));
    }
    let base = clean(&expanded);
    let r = base.join("rush");
    let (rd, h) = (clean(rush_dir), clean(home));
    if rd.starts_with(&r) || r.starts_with(&rd) {
        return Some(Err(format!(
            "{} would be in rush's own folder, {}",
            r.display(),
            rd.display()
        )));
    }
    if !h.as_os_str().is_empty() && h.starts_with(&r) {
        return Some(Err(format!("{} would hold your home", r.display())));
    }
    Some(Ok(r))
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

    /// Writes config.json in the test's rush folder with scratchRoot v.
    fn set_root(t: &Path, v: &str) {
        let home = t.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let b = serde_json::json!({"trimRunningTmp": true, "scratchRoot": v}).to_string();
        std::fs::write(home.join("config.json"), b).unwrap();
    }

    // scratchRoot by its text: ~ is home, rush's root is rush/ in it, and
    // nothing that would make rush's folder or home scratch.
    #[test]
    fn the_setting_read_as_a_root() {
        let (home, rd) = (Path::new("/Users/me"), Path::new("/Users/me/.config/rush"));
        for (v, want) in [
            ("", None),
            ("  ", None),
            ("/Volumes/Builds", Some(Ok("/Volumes/Builds/rush"))),
            ("/Volumes/Builds/", Some(Ok("/Volumes/Builds/rush"))),
            ("/Volumes/Builds/./x/..", Some(Ok("/Volumes/Builds/rush"))),
            ("~/scratch", Some(Ok("/Users/me/scratch/rush"))),
            ("~", Some(Ok("/Users/me/rush"))),
            ("/", Some(Ok("/rush"))),
            ("scratch", Some(Err(()))),
            ("~me/scratch", Some(Err(()))),
            ("~/.config", Some(Err(()))),
            ("~/.config/rush", Some(Err(()))),
            ("~/.config/rush/tmp", Some(Err(()))),
        ] {
            let got = resolve_setting(v, home, rd);
            let got = got.map(|r| r.map(|p| p.to_string_lossy().into_owned()).map_err(|_| ()));
            let want = want.map(|r| r.map(str::to_string));
            assert_eq!(got, want, "{v:?}");
        }
        assert!(resolve_setting("~/x", Path::new(""), rd).unwrap().is_err());
    }

    // Set and there: new sessions' scratch goes in rush/ in it, owner-only,
    // and it's the first of the roots; the rest still are.
    #[test]
    fn the_configured_root_is_used_and_first() {
        let s = Sys::new();
        let t = s.0.path();
        let vol = t.join("vol");
        std::fs::create_dir(&vol).unwrap();
        set_root(t, &vol.to_string_lossy());
        let got = new_scratch_root();
        assert_eq!(
            got,
            NewRoot {
                root: vol.join("rush"),
                fell_back: None
            }
        );
        assert_eq!(mode(&vol.join("rush")), 0o700);
        assert_eq!(
            scratch_roots(),
            vec![vol.join("rush"), t.join("rush"), t.join("home").join("tmp")]
        );
        assert_eq!(
            scratch_root_of(&vol.join("rush/app-1/s1")),
            Some(vol.join("rush"))
        );
        assert_eq!(
            scratch_root_of(&vol.join("game/build")),
            None,
            "beside rush's"
        );
        assert_eq!(scratch_root_of(&vol), None, "the folder named itself");
        // Changed: read again. Unset: as before.
        set_root(t, "");
        assert_eq!(
            scratch_roots(),
            vec![t.join("rush"), t.join("home").join("tmp")]
        );
        assert_eq!(new_scratch_root().root, t.join("rush"));
    }

    // Set and not usable: new sessions' go where they would without it,
    // saying why, and nothing is made where it pointed. It's still a root,
    // so what's under it is still known as scratch once it's back.
    #[test]
    fn an_unusable_root_falls_back() {
        type Setup = fn(&Path);
        let cases: [(&str, Setup, &str); 5] = [
            ("not mounted", |_| {}, "isn't there"),
            (
                "a file",
                |v| std::fs::write(v, "x").unwrap(),
                "isn't a folder",
            ),
            (
                "rush/ a link",
                |v| {
                    std::fs::create_dir(v).unwrap();
                    let to = v.with_file_name("elsewhere");
                    std::fs::create_dir(&to).unwrap();
                    std::os::unix::fs::symlink(&to, v.join("rush")).unwrap();
                },
                "rush's own",
            ),
            (
                "read-only",
                |v| {
                    std::fs::create_dir(v).unwrap();
                    std::fs::set_permissions(v, std::fs::Permissions::from_mode(0o500)).unwrap();
                },
                "can't make",
            ),
            (
                "rush/ read-only",
                |v| {
                    std::fs::create_dir_all(v.join("rush")).unwrap();
                    std::fs::set_permissions(
                        v.join("rush"),
                        std::fs::Permissions::from_mode(0o500),
                    )
                    .unwrap();
                },
                "can't write",
            ),
        ];
        // SAFETY: geteuid can't fail.
        if unsafe { libc::geteuid() } == 0 {
            return; // root writes anywhere
        }
        for (what, setup, why) in cases {
            let s = Sys::new();
            let t = s.0.path();
            let vol = t.join("vol");
            setup(&vol);
            set_root(t, &vol.to_string_lossy());
            let got = new_scratch_root();
            assert_eq!(got.root, t.join("rush"), "{what}");
            let fb = got.fell_back.expect(what);
            assert_eq!(fb.wanted, vol.to_string_lossy(), "{what}");
            assert!(fb.why.contains(why), "{what}: {}", fb.why);
            assert_eq!(scratch_roots()[0], vol.join("rush"), "{what}");
            if what == "not mounted" {
                assert!(!vol.exists(), "made a folder where a volume mounts");
            }
            for p in [&vol, &vol.join("rush")] {
                if std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) {
                    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
                }
            }
        }
    }

    // A setting rush won't use is said, and isn't a root.
    #[test]
    fn a_bad_setting_falls_back_and_isnt_a_root() {
        let s = Sys::new();
        let t = s.0.path();
        set_root(t, "relative/path");
        let got = new_scratch_root();
        assert_eq!(got.root, t.join("rush"));
        assert!(got.fell_back.unwrap().why.contains("whole path"));
        assert_eq!(
            scratch_roots(),
            vec![t.join("rush"), t.join("home").join("tmp")]
        );
        set_root(t, &t.join("home").to_string_lossy()); // rush's folder's parent
        assert!(new_scratch_root().fell_back.is_some());
        assert_eq!(scratch_roots().len(), 2);
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
