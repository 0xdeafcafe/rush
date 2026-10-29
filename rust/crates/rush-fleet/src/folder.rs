//! A repository agents work in, as git sees it: the main checkout's state
//! and that of each linked worktree an agent is in, checked whole for the
//! Projects place (Go: internal/fleet/folder.go, Folder :16, GitState :45,
//! CheckFolder :65, shortRemote :137, gitState :161, base :202,
//! FolderWants :245). It runs git: never on the UI thread.

use std::collections::{BTreeMap, HashMap};
use std::process::Command;
use std::thread;
use std::time::Duration;

use rush_core::time::{Ms, now};

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
    pub trees: HashMap<String, GitState>,
    /// Checked whole, for the Projects place: where it's pushed, its last
    /// commits, and every linked worktree in the order git lists them.
    pub whole: bool,
    /// origin, as host/owner/repo where it looks like one.
    pub remote: String,
    pub recent: Vec<Commit>,
    pub linked: Vec<String>,
    /// When git was asked.
    pub checked: Ms,
}

/// One commit on a checkout's branch.
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// Commits not on the upstream yet.
    pub ahead: usize,
    /// Commits on the upstream not here yet.
    pub behind: usize,
    /// Files with uncommitted changes, untracked ones included.
    pub changed: usize,
    /// HEAD's short hash.
    pub commit: String,
    /// The upstream the branch tracks, as git names it ("origin/main").
    pub target: String,
    /// The branch a linked worktree's branch was made from, and how far
    /// it's gone from it: commits of its own, and the base's since.
    pub base: String,
    pub base_ahead: usize,
    pub base_behind: usize,
    pub err: String,
}

/// git -C dir args, its output with the last newline gone; with
/// GIT_OPTIONAL_LOCKS=0, so a look never blocks an agent's own git.
pub fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    git_within(Duration::from_secs(20), dir, args)
}

/// git with a time limit of its own; ZERO is none, for what mustn't be
/// stopped halfway (worktree.go:105 gitWithin).
pub fn git_within(limit: Duration, dir: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0");
    let limit = if limit.is_zero() {
        Duration::from_secs(24 * 3600)
    } else {
        limit
    };
    match rush_sys::cmd::output_within(&mut cmd, limit) {
        Ok(o) if o.ok() => {
            let s = String::from_utf8_lossy(&o.out);
            Ok(s.trim_end_matches('\n').to_string())
        }
        Ok(o) => Err(o.err_text()),
        Err(e) => Err(e.to_string()),
    }
}

/// Asks git about root and the worktrees under it that agents work in;
/// whole, about every worktree it has, and its remote and last commits
/// too. Several gits at once: a big checkout's status takes a second.
pub fn check_folder(root: &str, trees: &[String], whole: bool) -> Folder {
    let mut f = Folder {
        root: root.to_string(),
        whole,
        checked: now(),
        ..Folder::default()
    };
    let root_s = root.to_string();
    let main = thread::spawn(move || git_state(&root_s));
    if let Ok(list) = git(root, &["worktree", "list", "--porcelain"]) {
        for l in list.lines() {
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
    let trees: Vec<String> = if whole {
        if let Ok(u) = git(root, &["remote", "get-url", "origin"]) {
            f.remote = short_remote(&u);
        }
        if let Ok(log) = git(root, &["log", "-3", "--format=%ct%x09%s"]) {
            for l in log.lines() {
                let Some((at, subject)) = l.split_once('\t') else {
                    continue;
                };
                f.recent.push(Commit {
                    subject: subject.to_string(),
                    at: at.parse::<i64>().unwrap_or(0) * 1000,
                });
            }
        }
        f.linked.clone()
    } else {
        trees.to_vec()
    };
    f.git = main.join().unwrap_or_default();
    // Each checkout's status is asked at once, six at a time.
    let fallback = f.git.branch.clone();
    let mut states = Vec::with_capacity(trees.len());
    for chunk in trees.chunks(6) {
        let hs: Vec<_> = chunk
            .iter()
            .map(|t| {
                let (t, fb) = (t.clone(), fallback.clone());
                thread::spawn(move || {
                    let mut s = git_state(&t);
                    s.find_base(&t, &fb);
                    s
                })
            })
            .collect();
        states.extend(hs.into_iter().map(|h| h.join().unwrap_or_default()));
    }
    for (t, s) in trees.into_iter().zip(states) {
        f.trees.insert(t, s);
    }
    f
}

/// A remote URL the way you'd say it: host/owner/repo, from either an
/// https or an ssh URL.
pub fn short_remote(u: &str) -> String {
    let mut u = u.trim().trim_end_matches(".git").to_string();
    let scheme = u.contains("://");
    if let Some((_, rest)) = u.split_once("://") {
        u = rest.to_string();
    }
    if let Some(i) = u.find('@') {
        u = u[i + 1..].to_string();
    }
    let Some((host, path)) = u.split_once(':') else {
        return u;
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
    match git(
        dir,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ],
    ) {
        Ok(out) => parse_status(&out),
        Err(_) => GitState {
            err: "git status failed".into(),
            ..GitState::default()
        },
    }
}

/// git status --porcelain=v2 --branch, read.
pub fn parse_status(out: &str) -> GitState {
    let mut s = GitState::default();
    for l in out.split('\n') {
        if l.is_empty() {
            continue;
        }
        if let Some(b) = l.strip_prefix("# branch.head ") {
            s.branch = if b == "(detached)" { "detached" } else { b }.to_string();
        } else if let Some(oid) = l.strip_prefix("# branch.oid ") {
            if oid.len() >= 7 && oid != "(initial)" {
                s.commit = oid[..7].to_string();
            }
        } else if let Some(u) = l.strip_prefix("# branch.upstream ") {
            s.upstream = true;
            s.target = u.to_string();
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
    /// The branch tree's was made from, as its reflog says ("Created from
    /// main"), else the main checkout's branch, and the commits between.
    fn find_base(&mut self, tree: &str, fallback: &str) {
        if self.branch.is_empty() || self.branch == "detached" {
            return;
        }
        let mut b = fallback.to_string();
        let r = format!("refs/heads/{}", self.branch);
        if let Ok(log) = git(tree, &["reflog", "show", "--format=%gs", &r])
            && let Some(from) = log
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
        let range = format!("{b}...HEAD");
        self.base = b;
        if let Ok(n) = git(tree, &["rev-list", "--left-right", "--count", &range])
            && let Some((behind, ahead)) = n.split_once('\t')
        {
            self.base_behind = behind.trim().parse().unwrap_or(0);
            self.base_ahead = ahead.trim().parse().unwrap_or(0);
        }
    }
}

/// Whether s looks like a commit's hash rather than a branch.
pub fn is_hash(s: &str) -> bool {
    s.len() >= 7
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// What check_folder should be asked for: each repository (root) agents
/// work in, with the linked worktrees (repo) they're in. Pairs are each
/// agent's (main checkout, checkout).
pub fn folder_wants<'a>(
    pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (root, repo) in pairs {
        if root.is_empty() {
            continue;
        }
        let trees = out.entry(root.to_string()).or_default();
        if repo != root && !repo.is_empty() && !trees.iter().any(|t| t == repo) {
            trees.push(repo.to_string());
        }
    }
    for t in out.values_mut() {
        t.sort();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_said_short() {
        for (u, want) in [
            (
                "git@github.com:0xdeafcafe/rush.git",
                "github.com/0xdeafcafe/rush",
            ),
            (
                "https://github.com/0xdeafcafe/rush.git",
                "github.com/0xdeafcafe/rush",
            ),
            ("ssh://git@host:2222/o/r.git", "host/o/r"),
            ("https://x.com/o/r", "x.com/o/r"),
            ("/local/path", "/local/path"),
        ] {
            assert_eq!(short_remote(u), want, "{u}");
        }
    }

    #[test]
    fn status_read() {
        let s = parse_status(
            "# branch.oid 0123456789abcdef\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -3\n1 .M N... x\n? new\n",
        );
        assert_eq!(s.branch, "main");
        assert_eq!(s.commit, "0123456");
        assert!(s.upstream);
        assert_eq!(s.target, "origin/main");
        assert_eq!((s.ahead, s.behind, s.changed), (2, 3, 2));
        let d = parse_status("# branch.oid (initial)\n# branch.head (detached)\n");
        assert_eq!(d.branch, "detached");
        assert_eq!(d.commit, "");
    }

    #[test]
    fn wants_by_repository() {
        let w = folder_wants([
            ("/r", "/r/.claude/worktrees/b"),
            ("/r", "/r"),
            ("/r", "/r/.claude/worktrees/a"),
            ("", "/x"),
        ]);
        assert_eq!(
            w.get("/r").unwrap(),
            &vec![
                "/r/.claude/worktrees/a".to_string(),
                "/r/.claude/worktrees/b".to_string()
            ]
        );
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn hashes() {
        assert!(is_hash("0123abc"));
        assert!(!is_hash("main"));
        assert!(!is_hash("01ab"));
    }
}
