//! The git worktree a session works in: one session, one worktree, one
//! branch, and maybe one pull request, found by the branch. The server's
//! part (making a session's own, noting one it went into) is server.rs's.
//!
//! Ported from internal/host/workspace.go:30-60.

use std::path::Path;

use rush_sys::actions::Made;

use crate::wire::Workspace;

/// The branch rush makes a session's own worktree on, named for its id,
/// as the worktree's folder is.
pub fn own_branch(id: &str) -> String {
    format!("rush/{id}")
}

/// Whether a worktree at path on branch is one rush made for a session:
/// <main>/.claude/worktrees/<id> on rush/<id>.
pub fn is_own_worktree(path: &str, branch: &str) -> bool {
    let p = Path::new(path);
    let name = |p: Option<&Path>| {
        p.and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let id = name(Some(p));
    branch == own_branch(&id)
        && name(p.parent()) == "worktrees"
        && name(p.parent().and_then(Path::parent)) == ".claude"
}

/// A worktree as a Workspace.
pub fn workspace_of(m: &Made, own: bool) -> Workspace {
    Workspace {
        path: m.path.clone(),
        repo: m.repo.clone(),
        branch: m.branch.clone(),
        base: m.base.clone(),
        target: m.target.clone(),
        own,
    }
}

/// The line rush's prompt tells an agent in its own worktree, so it
/// doesn't make another.
pub fn workspace_note(w: Option<&Workspace>) -> String {
    match w {
        Some(w) if w.own => format!(
            "You are already in your own git worktree {} on branch {}; commit there; don't create another worktree.",
            w.path, w.branch
        ),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_its_own_worktrees() {
        assert!(is_own_worktree("/r/.claude/worktrees/abc", "rush/abc"));
        assert!(!is_own_worktree("/r/.claude/worktrees/abc", "rush/abd"));
        assert!(!is_own_worktree("/r/worktrees/abc", "rush/abc"));
        let w = Workspace { path: "/w".into(), branch: "rush/x".into(), own: true, ..Workspace::default() };
        assert!(workspace_note(Some(&w)).contains("/w on branch rush/x"));
        assert_eq!(workspace_note(None), "");
    }
}
