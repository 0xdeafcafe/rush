//! Where a session works, in words: its folder, or its repository, branch
//! and worktree. After Go's internal/ui/location.go (sessionLocation :14,
//! locationWithin :26, locationBranch :34, locationSummary :48,
//! locationFolder :65). The list's own one-line summary is
//! list::location_summary (milestone 1); these are for the screens that
//! say more.

use rush_core::agent::Agent;
use rush_core::text::width;

use crate::style::*;

/// Where an agent is: its cwd, repository, main checkout and branch. A
/// live cwd (a hosted session's own word) wins at once, without
/// borrowing git's view of a checkout it has left (sessionLocation).
pub struct Location<'a> {
    pub cwd: &'a str,
    pub repo: &'a str,
    pub root: &'a str,
    pub branch: &'a str,
}

pub fn session_location<'a>(
    a: &'a Agent,
    root: &'a str,
    live_cwd: Option<&'a str>,
) -> Location<'a> {
    let cwd = live_cwd.filter(|c| !c.is_empty()).unwrap_or(&a.job.cwd);
    let mut l = Location {
        cwd,
        repo: &a.repo,
        root,
        branch: &a.branch,
    };
    if !l.repo.is_empty() && !within(l.repo, cwd) {
        l.repo = "";
        l.root = "";
        l.branch = "";
    }
    l
}

/// Whether path is root or inside it (locationWithin).
pub fn within(root: &str, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let root = root.trim_end_matches('/');
    let path = path.trim_end_matches('/');
    path == root || path.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

fn base(p: &str) -> &str {
    let p = p.trim_end_matches('/');
    p.rsplit('/').next().unwrap_or(p)
}

/// The branch, and the worktree it's in when that isn't the main
/// checkout (locationBranch).
pub fn branch_words(repo: &str, root: &str, branch: &str) -> String {
    if !repo.is_empty() && !root.is_empty() && repo != root {
        let tree = base(repo);
        if tree == branch {
            return format!("{branch} (worktree)");
        }
        if branch.is_empty() {
            return format!("worktree {tree}");
        }
        return format!("{branch} · worktree {tree}");
    }
    branch.to_string()
}

/// Where an agent works, in a line: tmp/x, a folder, or its project,
/// branch and folder in it (locationSummary).
pub fn summary(a: &Agent, root: &str) -> String {
    let l = session_location(a, root, None);
    if l.repo.is_empty() {
        if crate::list::is_temp(l.cwd) {
            return format!("tmp/{}", base(l.cwd));
        }
        return tildify(l.cwd);
    }
    let mut s = base(if l.root.is_empty() { l.repo } else { l.root }).to_string();
    let b = branch_words(l.repo, l.root, l.branch);
    if !b.is_empty() {
        s.push_str(" · ");
        s.push_str(&b);
    }
    if let Some(rel) = l.cwd.strip_prefix(l.repo).and_then(|r| r.strip_prefix('/'))
        && !rel.is_empty()
    {
        s.push_str(" · ./");
        s.push_str(rel);
    }
    s
}

/// The folder in at most room cells: a repository by its project's name
/// and where in it, else the folder itself (locationFolder).
pub fn folder(a: &Agent, root: &str, live_cwd: Option<&str>, room: usize) -> String {
    let l = session_location(a, root, live_cwd);
    if l.repo.is_empty() {
        return crate::header::short_path(&tildify(l.cwd), room);
    }
    let name = base(if root.is_empty() { l.repo } else { root });
    match l.cwd.strip_prefix(l.repo).and_then(|r| r.strip_prefix('/')) {
        Some(rel) if !rel.is_empty() => format!(
            "{name}/{}",
            crate::header::short_path(rel, room.saturating_sub(width(name) + 1))
        ),
        _ => rush_draw::truncate(name, room, "…").into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_and_worktree_words() {
        let cases = [
            ("/r/rush", "/r/rush", "main", "main"),
            ("/r/wt/e57", "/r/rush", "e57", "e57 (worktree)"),
            ("/r/wt/e57", "/r/rush", "", "worktree e57"),
            (
                "/r/wt/e57",
                "/r/rush",
                "rush/e57",
                "rush/e57 · worktree e57",
            ),
        ];
        for (repo, root, branch, want) in cases {
            assert_eq!(branch_words(repo, root, branch), want);
        }
        assert!(within("/a/b", "/a/b/c"));
        assert!(within("/a/b", "/a/b"));
        assert!(!within("/a/b", "/a/bc"));
        assert!(!within("/a/b", ""));
    }

    #[test]
    fn summary_says_where() {
        let mut a = Agent::default();
        a.job.cwd = "/r/wt/e57/sub".into();
        a.repo = "/r/wt/e57".into();
        a.branch = "rush/e57".into();
        assert_eq!(
            summary(&a, "/r/rush"),
            "rush · rush/e57 · worktree e57 · ./sub"
        );
        // A cwd outside the repository is just the folder.
        a.job.cwd = "/elsewhere".into();
        assert_eq!(summary(&a, "/r/rush"), "/elsewhere");
    }
}
