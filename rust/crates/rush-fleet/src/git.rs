//! The checkout a folder is in, and its branch, read from .git directly
//! (fleet.go:1312-1363 gitFor, gitAt).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rush_core::time::Ms;

/// A folder's checkout and branch, and when they were read.
pub(crate) struct GitInfo {
    repo: String,
    branch: String,
    at: Ms,
}

/// The checkout dir is in and its branch, kept 30s.
pub(crate) fn git_for(
    cache: &mut HashMap<String, GitInfo>,
    dir: &str,
    now: Ms,
) -> (String, String) {
    if dir.is_empty() {
        return Default::default();
    }
    if let Some(g) = cache.get(dir)
        && now - g.at < 30_000
    {
        return (g.repo.clone(), g.branch.clone());
    }
    let (repo, branch) = git_at(dir);
    cache.insert(
        dir.to_string(),
        GitInfo {
            repo: repo.clone(),
            branch: branch.clone(),
            at: now,
        },
    );
    (repo, branch)
}

/// The checkout dir is in, and its branch, read directly.
pub fn git_at(dir: &str) -> (String, String) {
    let mut d = Path::new(dir);
    loop {
        let s = d.as_os_str();
        if s == "/" || s == "." || s.is_empty() {
            return Default::default();
        }
        let p = d.join(".git");
        if let Ok(m) = std::fs::metadata(&p) {
            let mut git_dir = p.clone();
            if !m.is_dir() {
                let b = std::fs::read_to_string(&p).unwrap_or_default();
                let s = b.trim().strip_prefix("gitdir:").unwrap_or(b.trim()).trim();
                git_dir = if s.starts_with('/') {
                    PathBuf::from(s)
                } else {
                    d.join(s)
                };
            }
            let mut branch = String::new();
            if let Ok(h) = std::fs::read_to_string(git_dir.join("HEAD")) {
                let h = h.trim();
                branch = h.strip_prefix("ref: refs/heads/").unwrap_or(h).to_string();
                // A detached HEAD's commit, shortened; a HEAD file not
                // ASCII is no commit, and cutting it could split a
                // character.
                if branch.len() == 40 && branch.is_ascii() {
                    branch.truncate(8);
                }
            }
            return (d.to_string_lossy().into_owned(), branch);
        }
        match d.parent() {
            Some(p) => d = p,
            None => return Default::default(),
        }
    }
}
