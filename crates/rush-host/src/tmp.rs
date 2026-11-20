//! Sessions' scratch folders: one folder per project, one per session in
//! it (<root>/<project>/<id>), so what's left behind says whose it is and
//! what it was working on. New sessions' are in the system's /tmp
//! (rush_state::scratch_root, owner-only), cleared at reboot and of what's
//! unused for days; those started before keep theirs in rush's folder
//! (rush_state::old_scratch_root) until they're done.
//!
//! Ported from internal/host/tmp.go; the root in /tmp is Rust-only.

use std::cell::Cell;
use std::path::{Component, Path, PathBuf};

use crate::paths::{dir, root};

/// Holds new sessions' scratch folders: rush's in the folder config.json's
/// scratchRoot names when it can be used, else rush's root in /tmp when
/// it's rush's own (rush_state::make_scratch_root), else the one in rush's
/// folder sessions had before (rush_state::new_scratch_root).
pub fn temp_root() -> PathBuf {
    rush_state::new_scratch_root().root
}

/// Whether p is a folder directly in a scratch root: a project's, or two
/// below, by the parent given.
fn in_a_root(p: Option<&Path>) -> bool {
    p.is_some_and(|p| rush_state::scratch_roots().iter().any(|r| r == p))
}

/// Where a session's agent and everything it runs keep their scratch
/// files. The session's own folder links to it (tmp); one from before the
/// per-project layout has the folder itself there.
pub fn temp_dir(id: &str) -> PathBuf {
    let p = dir(id).join("tmp");
    match std::fs::read_link(&p) {
        Ok(r) => r,
        Err(_) => p,
    }
}

/// p made lexically clean, as Go's filepath.Clean.
pub(crate) fn clean(p: &Path) -> PathBuf {
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
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Whether p is a session's scratch folder as rush lays them out: the only
/// folders its tidy-ups empty. Under the scratch root that's
/// tmp/<project>/<id> of a session rush knows, not anything two below it
/// (a sibling a script left, a deleted session's). A trailing "/", as
/// TMPDIR has, is the same folder.
pub fn is_temp_dir(p: &Path) -> bool {
    let p = clean(p);
    let roots = rush_state::scratch_roots();
    if roots.contains(&p) || system_tmp_itself(&p) {
        return false; // named tmp, but it holds every project's, or everyone's
    }
    p.file_name().is_some_and(|n| n == "tmp")
        || in_a_root(p.parent().and_then(Path::parent))
            && p.file_name().is_some_and(|n| root().join(n).is_dir())
}

/// Whether clean p is the system's temp folder itself, by any name.
fn system_tmp_itself(p: &Path) -> bool {
    p == Path::new("/tmp") || p == Path::new("/private/tmp") || p == rush_state::system_tmp()
}

thread_local! {
    /// Tests' own folders are in the system's temp folder: they say it
    /// isn't one.
    static NO_SYSTEM_TEMP: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn ignore_system_temp(on: bool) {
    NO_SYSTEM_TEMP.with(|c| c.set(on));
}

/// Whether p is in the system's temp folders.
fn system_temp(p: &str) -> bool {
    if NO_SYSTEM_TEMP.with(Cell::get) {
        return false;
    }
    p.contains("/var/folders/") || p.starts_with("/tmp/") || p.starts_with("/private/tmp/")
}

/// Whether clean path p is root or under it.
fn inside(root: &str, p: &str) -> bool {
    p.starts_with(root) && (p.len() == root.len() || p.as_bytes()[root.len()] == b'/')
}

/// Whether p is in a temp folder, a session's (its scratch or what it
/// keeps) or the system's: somewhere work is scratch, never a project.
pub fn is_temp(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    let p = clean(Path::new(p));
    let p = p.to_string_lossy();
    let kr = crate::keep::keep_root();
    if rush_state::scratch_roots()
        .iter()
        .any(|r| inside(&r.to_string_lossy(), &p))
        || inside(&kr.to_string_lossy(), &p)
        || system_temp(&p)
    {
        return true;
    }
    let r = root();
    let r = r.to_string_lossy();
    if !inside(&r, &p) || *p == *r {
        return false;
    }
    let rest = &p[r.len() + 1..];
    let rest = rest.split_once('/').map_or("", |(_, rest)| rest);
    rest == "tmp" || rest.starts_with("tmp/")
}

/// Session id's scratch folder for work in cwd: under cwd's project,
/// moved there with all it holds from the old layout, when nothing has it
/// open. Once under a project it stays there, wherever the session goes:
/// its conversation names the files in it by path. A cwd that's itself
/// scratch keeps it where it is. One under either root stays where it
/// is, so a session started before scratch moved to /tmp keeps its own;
/// one the system cleared (a reboot) is made again, where new ones go.
pub fn place_temp(id: &str, cwd: &str) -> PathBuf {
    let was = temp_dir(id);
    if in_a_root(was.parent().and_then(Path::parent)) && std::fs::metadata(&was).is_ok() {
        return was;
    }
    let proj = project_of(cwd);
    if proj.is_empty() || within(&was, Path::new(cwd)) {
        let _ = make(&was);
        return was;
    }
    let want = project_temp(&proj).join(id);
    if was == want {
        if make(&want).is_ok() {
            return want;
        }
        return link_to(id, &old_place(&proj, id).unwrap_or(want));
    }
    if std::fs::metadata(&was).is_ok() {
        // Something running out of it would lose its files mid-way.
        match crate::trim::held_under(&was) {
            Some(held) if held.is_empty() => {}
            _ => return was,
        }
        if std::fs::metadata(&want).is_ok()
            || want.parent().is_none_or(|p| make(p).is_err())
            || std::fs::rename(&was, &want).is_err()
        {
            return was;
        }
    }
    if make(&want).is_err() {
        return match old_place(&proj, id) {
            Some(p) => link_to(id, &p),
            None => was,
        };
    }
    link_to(id, &want)
}

/// Why session scratch folder tmp isn't in the folder config.json's
/// scratchRoot names when that's set and can't be used now: for the
/// session's events. None when it's there, unset or usable (a session
/// started before it was set keeps its folder where it is, unsaid).
pub fn fell_back(tmp: &Path) -> Option<rush_state::FellBack> {
    let r = rush_state::configured_scratch_root();
    if r.as_deref().is_some_and(|r| within(r, tmp)) {
        return None;
    }
    rush_state::new_scratch_root().fell_back
}

/// Makes session id's scratch folder again when it's gone from under a
/// running session: the system empties /tmp's folders nothing has used
/// for days, and its agent's TMPDIR still names it. Only a folder in a
/// scratch root, and one in /tmp only while rush's root there is its own.
pub fn remake_temp(id: &str) {
    let p = temp_dir(id);
    if in_a_root(p.parent().and_then(Path::parent)) && std::fs::metadata(&p).is_err() {
        let _ = make(&p);
    }
}

/// The session's folder in the old root, made, for when the one in /tmp
/// can't be: someone else's rush folder there, or /tmp unwritable.
fn old_place(proj: &str, id: &str) -> Option<PathBuf> {
    let p = rush_state::old_scratch_root().join(slug(proj)).join(id);
    mkdir(&p).is_ok().then_some(p)
}

/// Links the session's folder (tmp) to its scratch folder p, and says p.
fn link_to(id: &str, p: &Path) -> PathBuf {
    let link = dir(id).join("tmp");
    let _ = std::fs::remove_file(&link); // the old link; an old folder there was just moved
    let _ = std::os::unix::fs::symlink(p, &link);
    p.to_path_buf()
}

/// Makes folder p, owner-only; one in /tmp only while rush's root there
/// is its own (rush_state::make_scratch_root), as anyone may make
/// folders in /tmp; one in scratchRoot's only while it can be used.
fn make(p: &Path) -> std::io::Result<()> {
    if within(&rush_state::scratch_root(), p) && rush_state::make_scratch_root().is_none() {
        return Err(std::io::Error::other("rush's folder in /tmp isn't its own"));
    }
    // Nor one in scratchRoot's while it can't be used: a volume not
    // mounted would get a folder at its mount point.
    if let Some(r) = rush_state::configured_scratch_root()
        && within(&r, p)
    {
        rush_state::scratch::make_configured_root(&r).map_err(std::io::Error::other)?;
    }
    mkdir(p)
}

fn mkdir(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(p)
}

/// Holds the scratch folders of the sessions working in project.
pub fn project_temp(project: &str) -> PathBuf {
    temp_root().join(slug(project))
}

/// The project cwd is in: its repository's main checkout, so a worktree's
/// scratch sits with the rest, or cwd when it isn't one.
pub fn project_of(cwd: &str) -> String {
    if cwd.is_empty() || is_temp(cwd) {
        return String::new();
    }
    let out = std::process::Command::new("git")
        .args([
            "-C",
            cwd,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ])
        .stderr(std::process::Stdio::null())
        .output();
    if let Ok(o) = out
        && o.status.success()
    {
        let common = String::from_utf8_lossy(&o.stdout).trim().to_string();
        let p = Path::new(&common);
        if p.file_name().is_some_and(|n| n == ".git")
            && let Some(parent) = p.parent()
        {
            return parent.to_string_lossy().into_owned();
        }
    }
    cwd.to_string()
}

/// Names a project's folder by its name and a hash of its whole path:
/// /Users/me/src/app is app-1a2b3c4d. Short, as a test's unix socket under
/// a session's TMPDIR must fit in 104 bytes.
pub fn slug(p: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in p.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let base = go_base(p);
    let mut name: String = base
        .chars()
        .map(|r| {
            if r.is_ascii_alphanumeric() || matches!(r, '-' | '_' | '.') {
                r
            } else {
                '-'
            }
        })
        .collect();
    if name.len() > 24 {
        // Cut at 24 bytes, as Go's name[:24] (only ASCII is left in it).
        name.truncate(24);
    }
    format!("{name}-{h:08x}")
}

/// filepath.Base as Go has it: "/" for the root, "." for "".
fn go_base(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/".into();
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// Whether p is root or under it, lexically.
pub(crate) fn within(root: &Path, p: &Path) -> bool {
    let (r, p) = (clean(root), clean(p));
    p.starts_with(&r)
}

/// A session's own permission rules (Config.allow) with "$TMPDIR" made its
/// scratch folder tmp, clean and without its "/": `Write(/$TMPDIR/**)`
/// is `Write(//…/tmp/app-1a2b3c4d/s1/**)`, Claude Code's absolute path.
/// Rust-only.
pub fn allow_rules(rules: &[String], tmp: &str) -> Vec<String> {
    let t = clean(Path::new(tmp)).to_string_lossy().into_owned();
    rules
        .iter()
        .filter(|r| !r.contains("$TMPDIR") || (t.starts_with('/') && t != "/"))
        .map(|r| r.replace("$TMPDIR", &t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;
    use std::time::SystemTime;

    #[test]
    fn allow_rules_name_the_scratch() {
        let rules = ["Bash(du:*)".to_string(), "Write(/$TMPDIR/**)".to_string()];
        for (tmp, want) in [
            (
                "/r/tmp/p-1/s1",
                vec!["Bash(du:*)", "Write(//r/tmp/p-1/s1/**)"],
            ),
            (
                "/r/tmp/p-1/s1/",
                vec!["Bash(du:*)", "Write(//r/tmp/p-1/s1/**)"],
            ),
            ("", vec!["Bash(du:*)"]),
            ("/", vec!["Bash(du:*)"]),
        ] {
            assert_eq!(allow_rules(&rules, tmp), want, "{tmp:?}");
        }
    }

    #[test]
    fn slugs_as_go_names_them() {
        let want = include_str!("../tests/fixtures/slugs.tsv");
        for l in want.lines() {
            let (p, s) = l.split_once('\t').unwrap();
            assert_eq!(slug(p), s, "{p}");
        }
    }

    // tmp_test.go:12: a session's scratch sits under its project, and goes
    // there, contents and all, from the old layout. Once there it stays
    // when the session moves to another project.
    #[test]
    fn place_temp_follows_project() {
        let _h = Home::new();
        ignore_system_temp(true);
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (
            a.path().to_string_lossy().into_owned(),
            b.path().to_string_lossy().into_owned(),
        );
        let old = root().join("s1").join("tmp");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("left"), "x").unwrap();
        // Nothing holds it open: lsof is asked, as Go does.
        let got = place_temp("s1", &a);
        let want = temp_root().join(slug(&a)).join("s1");
        assert_eq!(got, want);
        assert_eq!(temp_dir("s1"), want);
        assert!(
            got.join("left").exists(),
            "the old folder's files didn't move with it"
        );
        assert_eq!(place_temp("s1", &b), got, "moved with the session");
        assert!(got.join("left").exists() && temp_dir("s1") == got);
        // One whose folder has gone gets a new one where it works now.
        std::fs::remove_dir_all(&got).unwrap();
        assert_eq!(place_temp("s1", &b), temp_root().join(slug(&b)).join("s1"));
        let got = temp_dir("s1");
        assert!(is_temp(&got.join("x").to_string_lossy()));
        assert!(is_temp(&old.join("x").to_string_lossy()));
        assert!(!is_temp(&a));
        assert!(is_temp_dir(&got));
        // Working in its own scratch doesn't move it.
        assert_eq!(place_temp("s1", &got.join("x").to_string_lossy()), got);
        ignore_system_temp(false);
    }

    // Beyond Go: a session's scratch is the same folder as its TMPDIR,
    // "/" and all, in either root; what a script left beside it, or a
    // deleted session's folder, isn't one, nor are the folders above, nor
    // the system's tmp itself.
    #[test]
    fn is_temp_dir_table() {
        let _h = Home::new();
        std::fs::create_dir_all(dir("s1")).unwrap();
        let env = |p: &str| rush_core::adapter::tmpdir_env(p);
        let old = dir("s1").join("tmp").to_string_lossy().into_owned();
        for root in rush_state::scratch_roots() {
            let proj = root.join(slug("/src/app"));
            let s = |x: &str| proj.join(x).to_string_lossy().into_owned();
            for (p, want) in [
                (s("s1"), true),
                (env(&s("s1")), true),
                (format!("{}//", s("s1")), true),
                (s("s1browsefix-dd"), false),
                (s("gone"), false),
                (proj.to_string_lossy().into_owned(), false),
                (root.to_string_lossy().into_owned(), false),
                (env(&root.to_string_lossy()), false),
                (old.clone(), true),
                (env(&old), true),
                ("/tmp".into(), false),
                ("/private/tmp/".into(), false),
                (
                    rush_state::system_tmp().to_string_lossy().into_owned(),
                    false,
                ),
            ] {
                assert_eq!(is_temp_dir(Path::new(&p)), want, "{p:?}");
            }
        }
    }

    // Rust-only: a new session's scratch is in rush's root in /tmp,
    // owner-only, given to the agent ending in "/"; one started before
    // keeps its folder in rush's own root; one the system cleared (a
    // reboot) is made again.
    #[test]
    fn new_scratch_goes_in_tmp_old_stays() {
        use std::os::unix::fs::PermissionsExt;
        let _h = Home::new();
        ignore_system_temp(true);
        let a = tempfile::tempdir().unwrap();
        let a = a.path().to_string_lossy().into_owned();
        let new = rush_state::scratch_root();
        let old = rush_state::old_scratch_root();
        for id in ["s1", "s2"] {
            std::fs::create_dir_all(dir(id)).unwrap();
        }
        // s1 is new.
        let got = place_temp("s1", &a);
        assert_eq!(got, new.join(slug(&a)).join("s1"));
        assert_eq!(temp_dir("s1"), got);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&new), 0o700, "the root in /tmp");
        assert_eq!(mode(&got), 0o700, "the session's folder");
        let env = rush_core::adapter::tmpdir_env(&got.to_string_lossy());
        assert_eq!(env, format!("{}/", got.to_string_lossy()));
        assert!(is_temp_dir(Path::new(&env)));
        // s2 started before: its folder in rush's root stays its own.
        let before = old.join(slug(&a)).join("s2");
        std::fs::create_dir_all(&before).unwrap();
        std::os::unix::fs::symlink(&before, dir("s2").join("tmp")).unwrap();
        assert_eq!(place_temp("s2", &a), before, "an old session's moved");
        assert!(is_temp_dir(&before) && is_temp(&before.join("x").to_string_lossy()));
        // After a reboot cleared /tmp, s1 resumes into a new one there.
        std::fs::remove_dir_all(&new).unwrap();
        assert_eq!(place_temp("s1", &a), got);
        assert!(got.is_dir() && mode(&new) == 0o700);
        // Cleared under a running session: made again where its TMPDIR says.
        std::fs::remove_dir_all(&got).unwrap();
        remake_temp("s1");
        assert!(got.is_dir() && temp_dir("s1") == got);
        // s2's old one cleaned away: it moves to /tmp.
        std::fs::remove_dir_all(&before).unwrap();
        assert_eq!(place_temp("s2", &a), new.join(slug(&a)).join("s2"));
        ignore_system_temp(false);
    }

    // Rust-only: when rush's root in /tmp isn't its own (a link someone
    // left, a file), a session's scratch goes in rush's folder instead.
    #[test]
    fn scratch_stays_home_when_tmp_isnt_ours() {
        type Plant = fn(&Path);
        let plants: [(&str, Plant); 2] = [
            ("a link", |r| {
                let to = r.with_file_name("theirs");
                std::fs::create_dir_all(&to).unwrap();
                std::os::unix::fs::symlink(&to, r).unwrap();
            }),
            ("a file", |r| {
                std::fs::create_dir_all(r.parent().unwrap()).unwrap();
                std::fs::write(r, "x").unwrap();
            }),
        ];
        for (what, plant) in plants {
            let _h = Home::new();
            ignore_system_temp(true);
            let a = tempfile::tempdir().unwrap();
            let a = a.path().to_string_lossy().into_owned();
            std::fs::create_dir_all(dir("s1")).unwrap();
            plant(&rush_state::scratch_root());
            let want = rush_state::old_scratch_root().join(slug(&a)).join("s1");
            assert_eq!(temp_root(), rush_state::old_scratch_root(), "{what}");
            assert_eq!(place_temp("s1", &a), want, "{what}");
            assert!(want.is_dir() && temp_dir("s1") == want, "{what}");
            ignore_system_temp(false);
        }
    }

    /// Sets config.json's scratchRoot in the test's rush folder.
    fn set_scratch_root(v: &Path) {
        let b = serde_json::json!({"scratchRoot": v}).to_string();
        std::fs::write(rush_state::dir().join("config.json"), b).unwrap();
    }

    // Rust-only: with scratchRoot set, a new session's scratch is in rush/
    // in it; one started before keeps its own, in either other root. Each
    // is a session's scratch to the tidy-ups; what else is on that volume,
    // and the folders above, never are, nor are trimmed.
    #[test]
    fn scratch_goes_where_the_setting_says() {
        let _h = Home::new();
        ignore_system_temp(true);
        let a = tempfile::tempdir().unwrap();
        let a = a.path().to_string_lossy().into_owned();
        // Beside rush's folder, never in it: refused there.
        let v = tempfile::Builder::new()
            .prefix("rv-")
            .tempdir_in("/tmp")
            .unwrap();
        let vol = std::fs::canonicalize(v.path()).unwrap().join("Builds");
        std::fs::create_dir(&vol).unwrap();
        set_scratch_root(&vol);
        let mine = vol.join("rush");
        for id in ["s1", "s2", "s3"] {
            std::fs::create_dir_all(dir(id)).unwrap();
        }
        let got = place_temp("s1", &a);
        assert_eq!(got, mine.join(slug(&a)).join("s1"));
        assert_eq!(temp_dir("s1"), got);
        assert_eq!(fell_back(&got), None);
        // Started before: in /tmp's root and in rush's folder's.
        let in_tmp = rush_state::scratch_root().join(slug(&a)).join("s2");
        let in_old = rush_state::old_scratch_root().join(slug(&a)).join("s3");
        for (id, p) in [("s2", &in_tmp), ("s3", &in_old)] {
            std::fs::create_dir_all(p).unwrap();
            std::os::unix::fs::symlink(p, dir(id).join("tmp")).unwrap();
            assert_eq!(place_temp(id, &a), *p, "{id} moved");
        }
        for p in [&got, &in_tmp, &in_old] {
            assert!(is_temp_dir(p), "{p:?}");
            assert!(is_temp(&p.join("x").to_string_lossy()), "{p:?}");
        }
        // What else is on the volume isn't rush's.
        let theirs = vol.join("game").join("s1");
        let old = SystemTime::now() - std::time::Duration::from_secs(4 * 3600);
        for p in [&theirs, &vol.join(slug(&a)).join("s1")] {
            std::fs::create_dir_all(p.join("build")).unwrap();
            std::fs::File::open(p.join("build"))
                .unwrap()
                .set_modified(old)
                .unwrap();
            assert!(!is_temp_dir(p), "{p:?}");
            assert!(!is_temp(&p.to_string_lossy()), "{p:?}");
            let none = std::collections::HashSet::new();
            assert_eq!(crate::trim::trim_tmp(p, SystemTime::now(), &none), 0);
            assert!(p.join("build").exists(), "{p:?} trimmed");
        }
        for p in [&vol, &mine, &mine.join(slug(&a))] {
            assert!(!is_temp_dir(p), "{p:?}");
        }
        ignore_system_temp(false);
    }

    // Rust-only: scratchRoot set but its volume not mounted: a new
    // session's scratch goes where it would without it, which is said; a
    // running one's under it isn't made again at the mount point.
    #[test]
    fn an_unmounted_scratch_root_falls_back() {
        let _h = Home::new();
        ignore_system_temp(true);
        let a = tempfile::tempdir().unwrap();
        let a = a.path().to_string_lossy().into_owned();
        // Beside rush's folder, never in it: refused there.
        let v = tempfile::Builder::new()
            .prefix("rv-")
            .tempdir_in("/tmp")
            .unwrap();
        let vol = std::fs::canonicalize(v.path()).unwrap().join("Builds");
        set_scratch_root(&vol);
        std::fs::create_dir_all(dir("s1")).unwrap();
        let got = place_temp("s1", &a);
        assert_eq!(got, rush_state::scratch_root().join(slug(&a)).join("s1"));
        let f = fell_back(&got).expect("the fallback wasn't said");
        assert_eq!(f.wanted, vol.to_string_lossy());
        assert!(f.why.contains("mounted"), "{}", f.why);
        // s2 was in it before the volume went.
        std::fs::create_dir_all(dir("s2")).unwrap();
        let gone = vol.join("rush").join(slug(&a)).join("s2");
        std::os::unix::fs::symlink(&gone, dir("s2").join("tmp")).unwrap();
        remake_temp("s2");
        assert!(!vol.exists(), "made a folder where the volume mounts");
        // Back: new sessions go in it again, nothing said.
        std::fs::create_dir(&vol).unwrap();
        std::fs::create_dir_all(dir("s3")).unwrap();
        let got = place_temp("s3", &a);
        assert_eq!(got, vol.join("rush").join(slug(&a)).join("s3"));
        assert_eq!(fell_back(&got), None);
        ignore_system_temp(false);
    }

    // tmp_test.go:53, for each scratch root.
    #[test]
    fn is_temp_table() {
        let _h = Home::new();
        ignore_system_temp(true);
        let r = root();
        let j = |p: &Path, s: &[&str]| {
            let mut p = p.to_path_buf();
            for x in s {
                p = p.join(x);
            }
            p.to_string_lossy().into_owned()
        };
        for root in rush_state::scratch_roots() {
            let tr = root.to_string_lossy().into_owned();
            for (p, want) in [
                (String::new(), false),
                (tr.clone(), true),
                (j(&root, &["a", "b"]), true),
                (format!("{}/", j(&root, &["a", "b"])), true), // as TMPDIR has it
                (format!("{}/", j(&r, &["s1", "tmp"])), true),
                (format!("{tr}x"), false),
                (j(&crate::keep::keep_root(), &["a", "b", "c"]), true),
                (j(&r, &["s1", "tmp"]), true),
                (j(&r, &["s1", "tmp", "x"]), true),
                (j(&r, &["s1", "tmpx"]), false),
                (j(&r, &["s1"]), false),
                (r.to_string_lossy().into_owned(), false),
                (j(&r, &["tmp"]), false),
                ("/elsewhere/s1/tmp".into(), false),
                ("rel/tmp".into(), false),
            ] {
                assert_eq!(is_temp(&p), want, "{p:?}");
            }
        }
        ignore_system_temp(false);
    }
}
