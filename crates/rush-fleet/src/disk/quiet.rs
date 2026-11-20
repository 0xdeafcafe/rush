//! Keeps folders that only hold what's made again out of Spotlight and
//! Time Machine, so a build's churn doesn't set mds and backupd working
//! (Settings › Disk's "Keep scratch out of Spotlight and Time Machine",
//! and, opted in, "…your projects too"). Rust-only: Go had no such
//! thing. FSEvents itself can't be turned off for a folder; what reads
//! its news can.
//!
//! Spotlight skips a folder with a `.metadata_never_index` file in it;
//! inside a git checkout that file is put in the repo's info/exclude, so
//! it never shows as untracked. Time Machine skips one `tmutil
//! addexclusion` named (the sticky kind: an xattr on the folder, which
//! needs no admin). Both are checked before anything's done, so a pass
//! over folders already quiet writes nothing and runs nothing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::find::{GitWorktree, parse_worktrees, repo_of};
use crate::worktree::git;

/// The file Spotlight takes as "never index this folder".
pub const NEVER_INDEX: &str = ".metadata_never_index";

/// The xattr `tmutil addexclusion` sets on a folder it excludes.
pub const TM_XATTR: &str = "com.apple.metadata:com_apple_backup_excludeItem";

/// Which folders to keep quiet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Quiet {
    /// rush's scratch roots, each repo's .claude/worktrees, and every
    /// checkout's target.
    pub scratch: bool,
    /// Every checkout of the repos rush's sessions work in.
    pub projects: bool,
}

impl Quiet {
    pub fn any(self) -> bool {
        self.scratch || self.projects
    }
}

/// One repo as git lists it: its main checkout and its worktrees.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repo {
    pub main: PathBuf,
    pub worktrees: Vec<GitWorktree>,
}

/// The repos the folders are in, each found once, with git.
pub fn repos_of(dirs: &[PathBuf]) -> Vec<Repo> {
    let mains: BTreeSet<PathBuf> = dirs.iter().filter_map(|d| repo_of(d)).collect();
    mains
        .into_iter()
        .map(|main| {
            let worktrees = git(
                &main.to_string_lossy(),
                &["worktree", "list", "--porcelain"],
            )
            .map(|o| parse_worktrees(&o))
            .unwrap_or_default()
            .into_iter()
            .filter(|w| !w.bare && !w.prunable)
            .collect();
            Repo { main, worktrees }
        })
        .collect()
}

/// The folders q keeps quiet, of those there: each with the repo it's
/// in, when it's in one.
pub fn folders(q: Quiet, repos: &[Repo], scratch: &[PathBuf]) -> Vec<(PathBuf, Option<PathBuf>)> {
    let mut out: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let mut add = |p: PathBuf, repo: Option<&PathBuf>| {
        if p.is_dir() && !out.iter().any(|(o, _)| *o == p) {
            out.push((p, repo.cloned()));
        }
    };
    if q.scratch {
        for s in scratch {
            add(s.clone(), None);
        }
    }
    for r in repos {
        let mut checkouts: Vec<&Path> = vec![&r.main];
        checkouts.extend(
            r.worktrees
                .iter()
                .filter(|w| !w.main)
                .map(|w| w.path.as_path()),
        );
        if q.projects {
            for c in &checkouts {
                add(c.to_path_buf(), Some(&r.main));
            }
        }
        if q.scratch {
            add(r.main.join(".claude").join("worktrees"), Some(&r.main));
            for c in &checkouts {
                add(c.join("target"), Some(&r.main));
            }
        }
    }
    out
}

/// What a pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quieted {
    /// Folders given a Spotlight marker.
    pub spotlight: usize,
    /// Folders newly excluded from Time Machine.
    pub backups: usize,
    /// What went wrong, if anything did.
    pub errs: Vec<String>,
}

/// Keeps each folder out of Spotlight and Time Machine, doing only what
/// isn't done yet. Off the UI: it may run git and tmutil.
pub fn quiet(folders: &[(PathBuf, Option<PathBuf>)]) -> Quieted {
    quiet_with(folders, tmutil_exclude)
}

/// quiet with exclude standing for tmutil addexclusion.
pub fn quiet_with(
    folders: &[(PathBuf, Option<PathBuf>)],
    exclude: impl FnOnce(&[&Path]) -> Result<(), String>,
) -> Quieted {
    let mut did = Quieted::default();
    let mut excluded_in: BTreeSet<PathBuf> = BTreeSet::new();
    let mut to_exclude: Vec<&Path> = Vec::new();
    for (p, repo) in folders {
        let marker = p.join(NEVER_INDEX);
        if !marker.exists() {
            if let Some(r) = repo
                && excluded_in.insert(r.clone())
                && let Err(e) = ignore_in_git(r)
            {
                did.errs.push(e);
                continue; // not marked, rather than shown as untracked
            }
            match std::fs::write(&marker, b"") {
                Ok(()) => did.spotlight += 1,
                Err(e) => did.errs.push(format!("{}: {e}", marker.display())),
            }
        }
        if !backups_skip(p) {
            to_exclude.push(p);
        }
    }
    if !to_exclude.is_empty() {
        match exclude(&to_exclude) {
            Ok(()) => did.backups += to_exclude.len(),
            Err(e) => did.errs.push(e),
        }
    }
    did
}

/// Has Time Machine skip each of paths, in one tmutil; nothing off a
/// Mac, or in a test, whose folders are throwaway.
fn tmutil_exclude(paths: &[&Path]) -> Result<(), String> {
    if !cfg!(target_os = "macos") || rush_core::sandbox::active() {
        return Ok(());
    }
    let o = std::process::Command::new("tmutil")
        .arg("addexclusion")
        .args(paths)
        .output()
        .map_err(|e| format!("tmutil: {e}"))?;
    if o.status.success() {
        return Ok(());
    }
    Err(format!(
        "tmutil addexclusion: {}",
        String::from_utf8_lossy(&o.stderr).trim()
    ))
}

/// Whether Time Machine already skips p (or there's none to ask, off a
/// Mac).
pub fn backups_skip(p: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let (Ok(path), Ok(name)) = (
            CString::new(p.as_os_str().as_bytes()),
            CString::new(TM_XATTR),
        ) else {
            return true;
        };
        // SAFETY: both are NUL-terminated; a null buffer of 0 asks only
        // the attribute's size.
        let n =
            unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
        n >= 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = p;
        true
    }
}

/// Has git ignore the Spotlight marker in every checkout of the repo
/// whose main checkout is main: a line in its info/exclude, which its
/// worktrees share.
pub fn ignore_in_git(main: &Path) -> Result<(), String> {
    let common = git(
        &main.to_string_lossy(),
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map_err(|e| format!("{}: {e}", main.display()))?;
    let exclude = Path::new(common.trim()).join("info").join("exclude");
    add_line(&exclude, NEVER_INDEX)
}

/// Adds line to the file at p unless it has it already, making both.
pub fn add_line(p: &Path, line: &str) -> Result<(), String> {
    let had = std::fs::read_to_string(p).unwrap_or_default();
    if had.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let mut text = had;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    std::fs::write(p, text).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &Path, main: bool) -> GitWorktree {
        GitWorktree {
            path: path.to_path_buf(),
            main,
            ..GitWorktree::default()
        }
    }

    // Scratch is rush's tmp, a repo's .claude/worktrees and the targets;
    // projects are the checkouts. Only folders that are there.
    #[test]
    fn which_folders() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("repo");
        let linked = main.join(".claude/worktrees/abc");
        let tmp = d.path().join("tmp");
        for p in [&main.join("target"), &linked.join("target"), &tmp] {
            std::fs::create_dir_all(p).unwrap();
        }
        let repos = vec![Repo {
            main: main.clone(),
            worktrees: vec![wt(&main, true), wt(&linked, false)],
        }];
        let scratch = vec![tmp.clone(), d.path().join("gone")];
        let paths = |q| -> Vec<PathBuf> {
            folders(q, &repos, &scratch)
                .into_iter()
                .map(|(p, _)| p)
                .collect()
        };
        assert!(paths(Quiet::default()).is_empty());
        assert_eq!(
            paths(Quiet {
                scratch: true,
                projects: false
            }),
            vec![
                tmp.clone(),
                main.join(".claude/worktrees"),
                main.join("target"),
                linked.join("target")
            ]
        );
        assert_eq!(
            paths(Quiet {
                scratch: false,
                projects: true
            }),
            vec![main.clone(), linked.clone()]
        );
        let both = folders(
            Quiet {
                scratch: true,
                projects: true,
            },
            &repos,
            &scratch,
        );
        assert_eq!(both.len(), 6);
        assert_eq!(both[0].1, None, "tmp is in no repo");
        assert!(both[1..].iter().all(|(_, r)| r.as_ref() == Some(&main)));
    }

    // A pass marks what isn't marked and has git ignore the marker; a
    // second pass does nothing.
    #[test]
    fn marks_once_and_git_ignores_it() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("repo");
        std::fs::create_dir_all(main.join("target")).unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&main)
            .status()
            .unwrap();
        assert!(init.success());
        let tmp = d.path().join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let marked: Vec<(PathBuf, Option<PathBuf>)> =
            vec![(tmp.clone(), None), (main.clone(), Some(main.clone()))];
        let mut asked = Vec::new();
        let did = quiet_with(&marked, |ps| {
            asked = ps.iter().map(|p| p.to_path_buf()).collect();
            Ok(())
        });
        // Off a Mac there's no Time Machine to ask.
        let mac = cfg!(target_os = "macos");
        assert_eq!(did.spotlight, 2, "{:?}", did.errs);
        assert_eq!(did.backups, if mac { 2 } else { 0 });
        if mac {
            assert_eq!(asked, vec![tmp.clone(), main.clone()]);
        }
        assert!(tmp.join(NEVER_INDEX).exists() && main.join(NEVER_INDEX).exists());
        let again = quiet_with(&marked, |_| Ok(()));
        assert_eq!(again.spotlight, 0, "marked once");
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=all"])
            .current_dir(&main)
            .output()
            .unwrap();
        let s = String::from_utf8_lossy(&status.stdout);
        assert!(!s.contains(NEVER_INDEX), "the marker shows in git: {s}");
        ignore_in_git(&main).unwrap();
        let exclude = std::fs::read_to_string(main.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.lines().filter(|l| *l == NEVER_INDEX).count(), 1);
    }

    #[test]
    fn add_line_once() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("info/exclude");
        add_line(&p, "x").unwrap();
        std::fs::write(&p, "# a\nx").unwrap();
        add_line(&p, "x").unwrap();
        add_line(&p, "y").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "# a\nx\ny\n");
    }
}
