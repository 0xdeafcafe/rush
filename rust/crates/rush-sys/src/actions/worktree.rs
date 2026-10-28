//! Git worktrees: where a checkout's top is, its main checkout, and making
//! a worktree where Claude Code puts its own.
//!
//! Ported from internal/actions/worktree.go:1-125.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The top of the git checkout dir is in, or "" if it isn't in one. In a
/// linked worktree it's the worktree's own top: see [`main_root`] for the
/// checkout it was made from.
pub fn repo_root(dir: &str) -> String {
    git_out(dir, &["rev-parse", "--show-toplevel"]).unwrap_or_default()
}

/// The main checkout of the repository dir is in, through any linked
/// worktree, or "" if it isn't in one: where rush makes worktrees, so one
/// made from inside another goes beside it, not inside it.
pub fn main_root(dir: &str) -> String {
    let common = match git_out(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        Some(c) if !c.is_empty() => c,
        _ => return String::new(),
    };
    let p = Path::new(&common);
    if p.file_name().is_none_or(|n| n != ".git") {
        return repo_root(dir); // a bare repository's, or one with its .git elsewhere
    }
    p.parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A linked worktree: where it is, its branch, and where it came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Made {
    /// The worktree's top.
    pub path: String,
    /// The folder matching the one asked from, inside it.
    pub dir: String,
    /// The main checkout it belongs to.
    pub repo: String,
    pub branch: String,
    /// The commit it started from, in full.
    pub base: String,
    /// The branch the main checkout had checked out when it was made:
    /// where its work is meant to land.
    pub target: String,
}

/// Makes a git worktree for the checkout dir is in, where Claude Code puts
/// its own (.claude/worktrees/<name>), on a new branch named after it, and
/// returns the folder that matches dir inside it.
pub fn new_worktree(dir: &str, name: &str) -> Result<String, String> {
    new_worktree_on(dir, name, &format!("worktree-{name}"), "")
}

/// [`new_worktree`] on a branch of your choosing, made from base (the main
/// checkout's HEAD when it's "").
pub fn new_worktree_on(dir: &str, name: &str, branch: &str, base: &str) -> Result<String, String> {
    make_worktree(dir, name, branch, base).map(|m| m.dir)
}

/// Makes a worktree named name on a new branch, always in the main
/// checkout's .claude/worktrees, whichever worktree dir is in, and from
/// base: the main checkout's HEAD when it's "", never the HEAD of the
/// worktree dir happens to be in.
pub fn make_worktree(dir: &str, name: &str, branch: &str, base: &str) -> Result<Made, String> {
    let main = main_root(dir);
    if main.is_empty() {
        return Err(format!("{dir} isn't in a git repository"));
    }
    let path = Path::new(&main)
        .join(".claude")
        .join("worktrees")
        .join(name);
    let mut m = Made {
        repo: main.clone(),
        branch: branch.to_string(),
        path: path.to_string_lossy().into_owned(),
        ..Made::default()
    };
    if path.exists() {
        return Err(format!("a worktree named {name} already exists"));
    }
    m.target = git_out(&main, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    let base = if base.is_empty() { "HEAD" } else { base };
    let sha = match git_out(
        &main,
        &["rev-parse", "--verify", "-q", &format!("{base}^{{commit}}")],
    ) {
        Some(s) if !s.is_empty() => s,
        _ => {
            let repo = Path::new(&main)
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            return Err(format!(
                "{repo} has no commit {base} to start a worktree from"
            ));
        }
    };
    m.base = sha.clone();
    let out = Command::new("git")
        .args(["-C", &main, "worktree", "add", "-b", branch, &m.path, &sha])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git worktree add: {e}"))?;
    if !out.status.success() {
        let mut all = out.stdout;
        all.extend_from_slice(&out.stderr);
        return Err(format!(
            "git worktree add: {}",
            String::from_utf8_lossy(&all).trim()
        ));
    }
    m.dir = m.path.clone();
    // The same folder inside it as dir is in its own checkout.
    let top = repo_root(dir);
    if !top.is_empty() {
        let real = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
        if let Ok(rel) = real.strip_prefix(&top)
            && !rel.as_os_str().is_empty()
        {
            let inside = path.join(rel);
            if inside.is_dir() {
                m.dir = inside.to_string_lossy().into_owned();
            }
        }
    }
    Ok(m)
}

/// The linked worktree dir is in, as git says it: None when dir is in a
/// main checkout, or in none. Its base is where its branch left the main
/// checkout's: the commit they last had in common.
pub fn worktree_at(dir: &str) -> Option<Made> {
    let (top, main) = (repo_root(dir), main_root(dir));
    if top.is_empty() || main.is_empty() || clean(&top) == clean(&main) {
        return None;
    }
    let mut m = Made {
        path: top.clone(),
        dir: dir.to_string(),
        repo: main.clone(),
        ..Made::default()
    };
    m.branch = git_out(&top, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    m.target = git_out(&main, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    if !m.target.is_empty() {
        m.base = git_out(&top, &["merge-base", "HEAD", &m.target]).unwrap_or_default();
    }
    Some(m)
}

fn clean(p: &str) -> PathBuf {
    Path::new(p).components().collect()
}

/// Runs git in dir and gives its output, trimmed; None when it fails.
fn git_out(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    out.status.success().then_some(s)
}
