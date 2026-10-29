//! Sessions' scratch folders: one folder per project, one per session in
//! it (tmp/<project>/<id>), so what's left behind says whose it is and what
//! it was working on.
//!
//! Ported from internal/host/tmp.go.

use std::cell::Cell;
use std::path::{Component, Path, PathBuf};

use crate::paths::{dir, root};

/// Holds sessions' scratch folders.
pub fn temp_root() -> PathBuf {
    rush_state::dir().join("tmp")
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
/// folders its tidy-ups empty.
pub fn is_temp_dir(p: &Path) -> bool {
    let p = clean(p);
    p.file_name().is_some_and(|n| n == "tmp")
        || p.parent().and_then(Path::parent) == Some(temp_root().as_path())
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

/// Whether p is in a temp folder, a session's or the system's: somewhere
/// work is scratch, never a project.
pub fn is_temp(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    let p = clean(Path::new(p));
    let p = p.to_string_lossy();
    let tr = temp_root();
    if inside(&tr.to_string_lossy(), &p) || system_temp(&p) {
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
/// scratch keeps it where it is.
pub fn place_temp(id: &str, cwd: &str) -> PathBuf {
    let was = temp_dir(id);
    if was.parent().and_then(Path::parent) == Some(temp_root().as_path())
        && std::fs::metadata(&was).is_ok()
    {
        return was;
    }
    let proj = project_of(cwd);
    if proj.is_empty() || within(&was, Path::new(cwd)) {
        let _ = mkdir(&was);
        return was;
    }
    let want = project_temp(&proj).join(id);
    if was == want {
        let _ = mkdir(&want);
        return want;
    }
    if std::fs::metadata(&was).is_ok() {
        // Something running out of it would lose its files mid-way.
        match crate::trim::held_under(&was) {
            Some(held) if held.is_empty() => {}
            _ => return was,
        }
        if std::fs::metadata(&want).is_ok()
            || want.parent().is_none_or(|p| mkdir(p).is_err())
            || std::fs::rename(&was, &want).is_err()
        {
            return was;
        }
    }
    if mkdir(&want).is_err() {
        return was;
    }
    let link = dir(id).join("tmp");
    let _ = std::fs::remove_file(&link); // the old link; an old folder there was just moved
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

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

    // tmp_test.go:53
    #[test]
    fn is_temp_table() {
        let _h = Home::new();
        ignore_system_temp(true);
        let tr = temp_root().to_string_lossy().into_owned();
        let r = root();
        let j = |p: &Path, s: &[&str]| {
            let mut p = p.to_path_buf();
            for x in s {
                p = p.join(x);
            }
            p.to_string_lossy().into_owned()
        };
        for (p, want) in [
            (String::new(), false),
            (tr.clone(), true),
            (j(&temp_root(), &["a", "b"]), true),
            (format!("{tr}x"), false),
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
        ignore_system_temp(false);
    }
}
