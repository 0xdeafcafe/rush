//! A repository agents work in, as git sees it: the main checkout's state
//! and each linked worktree an agent is in. Port of
//! internal/fleet/folder.go.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Condvar, Mutex};

use rush_core::agent::Agent;
use rush_core::time::{Ms, now};

use crate::worktree::git;

/// A repository agents work in, as git sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folder {
    pub root: String,
    pub git: GitState,
    /// Every linked worktree the repository has, whether an agent works
    /// in it or not.
    pub worktrees: usize,
    /// The linked worktrees agents work in, by path; checked whole, every
    /// linked worktree.
    pub trees: BTreeMap<String, GitState>,
    /// Checked whole, for the Overview's Projects page: where it's pushed,
    /// its last commits, and every linked worktree in git's order.
    pub whole: bool,
    /// origin, as host/owner/repo where it looks like one.
    pub remote: String,
    pub recent: Vec<Commit>,
    pub linked: Vec<String>,
    /// When git was asked.
    pub checked: Ms,
}

/// One commit on a checkout's branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    pub subject: String,
    pub at: Ms,
}

/// One checkout's branch and how far it is from committed and pushed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitState {
    pub branch: String,
    /// The branch tracks a remote one.
    pub upstream: bool,
    pub ahead: i64,
    pub behind: i64,
    /// Files with uncommitted changes, untracked ones included.
    pub changed: i64,
    /// HEAD's short hash.
    pub commit: String,
    /// The upstream the branch tracks, as git names it ("origin/main").
    pub target: String,
    /// The branch a linked worktree's branch was made from, and how far
    /// it's gone from it.
    pub base: String,
    pub base_ahead: i64,
    pub base_behind: i64,
    pub err: String,
}

/// A gate letting at most n through at once.
struct Gate(Mutex<usize>, Condvar);

impl Gate {
    fn enter(&self) {
        let mut n = self.0.lock().unwrap_or_else(|e| e.into_inner());
        while *n == 0 {
            n = self.1.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n -= 1;
    }
    fn leave(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.1.notify_one();
    }
}

/// Asks git about root and the worktrees under it that agents work in;
/// whole, about every worktree it has, and its remote and last commits
/// too. It can take seconds on a big checkout: never on the UI's thread.
pub fn check_folder(root: &str, trees: &[String], whole: bool) -> Folder {
    let mut f = Folder {
        root: root.to_string(),
        whole,
        checked: now(),
        ..Folder::default()
    };
    let mut trees = trees.to_vec();
    std::thread::scope(|s| {
        let root_git = s.spawn(|| git_state(root));
        if let Ok(list) = git(root, &["worktree", "list", "--porcelain"]) {
            for l in list.split('\n') {
                if let Some(p) = l.strip_prefix("worktree ")
                    && p != root
                {
                    f.worktrees += 1;
                    if whole {
                        f.linked.push(p.to_string());
                    }
                }
            }
        }
        if whole {
            trees = f.linked.clone();
            if let Ok(u) = git(root, &["remote", "get-url", "origin"]) {
                f.remote = short_remote(&u);
            }
            if let Ok(log) = git(root, &["log", "-3", "--format=%ct%x09%s"]) {
                for l in log.split('\n') {
                    let Some((at, subject)) = l.split_once('\t') else {
                        continue;
                    };
                    f.recent.push(Commit {
                        subject: subject.to_string(),
                        at: at.parse::<i64>().unwrap_or(0) * 1000,
                    });
                }
            }
        }
        // Each checkout's status takes a second or more on a big
        // repository: they're asked at once, six at a time.
        let gate = Arc::new(Gate(Mutex::new(6), Condvar::new()));
        let handles: Vec<_> = trees
            .iter()
            .map(|t| {
                let gate = gate.clone();
                s.spawn(move || {
                    gate.enter();
                    let st = git_state(t);
                    gate.leave();
                    st
                })
            })
            .collect();
        let states: Vec<GitState> = handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect();
        f.git = root_git.join().unwrap_or_default();
        // The main checkout's branch stands in for an unknown base.
        let fallback = f.git.branch.clone();
        let based: Vec<_> = trees
            .iter()
            .zip(states)
            .map(|(t, mut st)| {
                let fallback = fallback.clone();
                let gate = gate.clone();
                s.spawn(move || {
                    gate.enter();
                    st.find_base(t, &fallback);
                    gate.leave();
                    (t.clone(), st)
                })
            })
            .collect();
        for h in based {
            if let Ok((t, st)) = h.join() {
                f.trees.insert(t, st);
            }
        }
    });
    f
}

/// A remote URL the way you'd say it: host/owner/repo, from either an
/// https or an ssh URL.
pub fn short_remote(u: &str) -> String {
    let mut u = u.trim();
    u = u.strip_suffix(".git").unwrap_or(u);
    let scheme = u.contains("://");
    if let Some((_, rest)) = u.split_once("://") {
        u = rest;
    }
    if let Some(i) = u.find('@') {
        u = &u[i + 1..];
    }
    let Some((host, path)) = u.split_once(':') else {
        return u.to_string();
    };
    if scheme {
        // host:port/path
        return match path.split_once('/') {
            Some((_, p)) => format!("{host}/{p}"),
            None => host.to_string(),
        };
    }
    format!("{host}/{path}") // scp-like host:path
}

/// A checkout's branch, upstream distance and changes, from one git
/// status.
pub fn git_state(dir: &str) -> GitState {
    let Ok(out) = git(
        dir,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ],
    ) else {
        return GitState {
            err: "git status failed".into(),
            ..GitState::default()
        };
    };
    let mut s = GitState::default();
    for l in out.split('\n') {
        if l.is_empty() {
            continue;
        }
        if let Some(b) = l.strip_prefix("# branch.head ") {
            s.branch = if b == "(detached)" { "detached" } else { b }.into();
        } else if let Some(oid) = l.strip_prefix("# branch.oid ") {
            if oid.len() >= 7 && oid != "(initial)" {
                s.commit = oid[..7].into();
            }
        } else if let Some(u) = l.strip_prefix("# branch.upstream ") {
            s.upstream = true;
            s.target = u.into();
        } else if let Some(ab) = l.strip_prefix("# branch.ab ") {
            for f in ab.split_whitespace() {
                let n = f[1..].parse().unwrap_or(0);
                if f.starts_with('+') {
                    s.ahead = n;
                } else {
                    s.behind = n;
                }
            }
        } else if !l.starts_with('#') {
            s.changed += 1;
        }
    }
    s
}

impl GitState {
    /// Finds the branch tree's was made from, as its reflog says
    /// ("Created from main"), else the main checkout's branch, and counts
    /// the commits between them.
    fn find_base(&mut self, tree: &str, fallback: &str) {
        if self.branch.is_empty() || self.branch == "detached" {
            return;
        }
        let mut b = fallback.to_string();
        if let Ok(log) = git(
            tree,
            &[
                "reflog",
                "show",
                "--format=%gs",
                &format!("refs/heads/{}", self.branch),
            ],
        ) && let Some(from) = log
            .split('\n')
            .next_back()
            .and_then(|l| l.strip_prefix("branch: Created from "))
            && from != "HEAD"
            && !is_hash(from)
        {
            b = from.to_string();
        }
        if b.is_empty() || b == self.branch {
            return;
        }
        if let Ok(n) = git(
            tree,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("{b}...HEAD"),
            ],
        ) && let Some((behind, ahead)) = n.split_once('\t')
        {
            self.base_behind = behind.parse().unwrap_or(0);
            self.base_ahead = ahead.parse().unwrap_or(0);
        }
        self.base = b;
    }
}

/// Whether s looks like a commit's hash rather than a branch.
fn is_hash(s: &str) -> bool {
    s.len() >= 7
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// What check_folder should be asked for the agents given: each repository
/// they work in, with the linked worktrees they're in.
pub fn folder_wants(agents: &[Agent]) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for a in agents {
        if a.root.is_empty() {
            continue;
        }
        let trees = out.entry(a.root.clone()).or_default();
        if a.repo != a.root && !a.repo.is_empty() && !trees.contains(&a.repo) {
            trees.push(a.repo.clone());
        }
    }
    for t in out.values_mut() {
        t.sort();
    }
    out
}
