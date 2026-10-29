//! Linked git worktrees agents work in, whether each can go without
//! losing anything, and removing one. Go: internal/fleet/worktree.go
//! (Worktree :27, Safe :68, Losses :79, FindWorktrees :131, agentSays
//! :198, Check :216, look :240, changes :265, unkept :285, removeWorktree
//! :349, mainCheckout :413, linkedWorktree :447) and verdict.go (the one
//! rule :17, madeBy :55, Verdict :70, landed :125, ownWork :148,
//! madeHere :175). It runs git: never on the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use rush_core::time::{Ms, now};
use rush_gh::ghpr::Status;

use crate::disk::disk_usage;
use crate::folder::{git, git_within};

/// A worktree's pull request, as GitHub last said.
pub type PR = Status;

/// Who made a worktree, by where it is and its branch.
pub const MADE_BY_RUSH: &str = "rush";
pub const MADE_BY_SUBAGENT: &str = "subagent";
pub const MADE_BY_CLAUDE: &str = "claude";
pub const MADE_BY_OTHER: &str = "other";

/// A linked git worktree an agent works in, and whether it can go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: String,
    /// The main checkout it belongs to.
    pub repo: String,
    pub branch: String,
    /// Keys of the agents working in it.
    pub agents: Vec<String>,
    /// Under a repo's .claude/worktrees: made for an agent.
    pub claude: bool,
    pub size: i64,
    /// Files with uncommitted changes, untracked ones included.
    pub changed: usize,
    /// Tracked files gone from disk, nothing staged: nothing lost.
    pub deleted: usize,
    /// Commits on no remote and not in main.
    pub unpushed: usize,
    /// Commits on no remote, main or not.
    pub off_remote: usize,
    pub no_remote: bool,
    pub locked: bool,
    pub err: String,
    /// When git last looked; 0 before it has.
    pub checked: Ms,
    /// Who made it (MADE_BY_*), and whether that's rush, for a session:
    /// only those ever go without being ticked by hand.
    pub made: String,
    pub own: bool,
    /// A session working in it now; "" when none is.
    pub in_use: String,
    pub pr: Option<PR>,
    /// The branch its work is meant to land on, and the commit its
    /// session's worktree started from, when known.
    pub target: String,
    pub base: String,
    /// HEAD's commit, and whether that's on target, here or on origin.
    pub head: String,
    pub landed: bool,
    in_pr: bool,
    own_work: bool,
}

/// What clean-up may do with a worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerdictKind {
    /// A session works in it now.
    KeepInUse,
    /// Its pull request is still open.
    KeepPrOpen,
    /// Removing it loses nothing.
    CanGo,
    /// Removing it would lose work: the reason says what.
    WouldLose,
}

/// What clean-up may do with a worktree, and why, in a few words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub kind: VerdictKind,
    pub reason: String,
}

impl Verdict {
    pub fn can_go(&self) -> bool {
        self.kind == VerdictKind::CanGo
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn first_non_empty<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

fn base(p: &str) -> &str {
    Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
}

/// Who made the worktree at path on branch (verdict.go:55 madeBy).
pub fn made_by(path: &str, branch: &str) -> &'static str {
    if is_own_worktree(path, branch) {
        MADE_BY_RUSH
    } else if !path.contains("/.claude/worktrees/") {
        MADE_BY_OTHER
    } else if base(path).starts_with("agent-") {
        MADE_BY_SUBAGENT
    } else {
        MADE_BY_CLAUDE
    }
}

/// A worktree rush made for a session: <main>/.claude/worktrees/<id> on
/// rush/<id> (host/workspace.go:35).
pub fn is_own_worktree(path: &str, branch: &str) -> bool {
    let p = Path::new(path);
    let id = base(path);
    let parent = p.parent();
    branch == format!("rush/{id}")
        && parent
            .and_then(|d| d.file_name())
            .is_some_and(|n| n == "worktrees")
        && parent
            .and_then(|d| d.parent())
            .and_then(|d| d.file_name())
            .is_some_and(|n| n == ".claude")
}

impl Worktree {
    /// Every change committed, and on a remote or in main, or in its
    /// merged pull request: removing it loses nothing but ignored files.
    pub fn safe(&self) -> bool {
        self.err.is_empty()
            && self.checked != 0
            && self.changed == 0
            && (self.unpushed == 0 || self.merged_pr())
            && !self.locked
    }

    /// Safe with every commit on a remote: what the automatic tidy-up
    /// asks for.
    pub fn pushed(&self) -> bool {
        self.safe() && self.off_remote == 0 && !self.no_remote
    }

    /// What removing it would throw away, for a confirmation.
    pub fn losses(&self) -> String {
        let mut out = Vec::new();
        if self.changed > 0 {
            out.push(plural(self.changed, "file") + " with uncommitted changes");
        }
        if self.unpushed > 0 && !self.merged_pr() {
            out.push(plural(self.unpushed, "commit") + " not pushed or in main");
        }
        if self.locked {
            out.push("a lock someone put on it".into());
        }
        out.join(" · ")
    }

    fn merged_pr(&self) -> bool {
        self.pr.as_ref().is_some_and(|p| p.merged()) && self.in_pr
    }

    /// What clean-up may do with it, by the one rule: only what check
    /// found, no git.
    pub fn verdict(&self) -> Verdict {
        let v = |kind, reason: String| Verdict { kind, reason };
        use VerdictKind::*;
        if !self.in_use.is_empty() {
            return v(KeepInUse, format!("{} is working in it", self.in_use));
        }
        if self.checked == 0 {
            return v(WouldLose, "not looked at with git yet".into());
        }
        if !self.err.is_empty() {
            return v(WouldLose, self.err.clone());
        }
        if !self.safe() {
            return v(WouldLose, self.losses());
        }
        if self.merged_pr() {
            let n = self.pr.as_ref().map_or(0, |p| p.number);
            return v(CanGo, format!("its pull request #{n} was merged"));
        }
        if self.landed {
            return v(CanGo, format!("its work is on {}", self.target));
        }
        if let Some(p) = self.pr.as_ref().filter(|p| p.open()) {
            return v(
                KeepPrOpen,
                format!("its pull request #{} is {}", p.number, p.state),
            );
        }
        if self.no_remote || self.off_remote > 0 {
            return v(
                CanGo,
                format!(
                    "every commit is in {}, not pushed",
                    first_non_empty(&self.target, "main")
                ),
            );
        }
        v(CanGo, "every commit is pushed".into())
    }

    /// Looks with git, then measures the disk it takes (Check).
    pub fn check(&mut self) {
        if self.look().2 {
            self.size = disk_usage(&[self.path.as_str()]);
        }
    }

    /// What removing it would lose, looked at afresh: its uncommitted
    /// files as git status names them, and its commits nowhere else, as
    /// "sha subject" (Doomed).
    pub fn doomed(&mut self) -> (Vec<String>, Vec<String>) {
        let (f, c, _) = self.look();
        (f, c)
    }

    fn look(&mut self) -> (Vec<String>, Vec<String>, bool) {
        self.checked = now();
        self.err.clear();
        let status = match git(
            &self.path,
            &["status", "--porcelain", "--untracked-files=normal"],
        ) {
            Ok(s) => s,
            Err(_) => {
                self.err = "git status failed".into();
                return (Vec::new(), Vec::new(), false);
            }
        };
        let (files, deleted) = changes(&status);
        self.deleted = deleted;
        self.changed = files.len();
        self.no_remote = git(&self.path, &["remote"])
            .unwrap_or_default()
            .trim()
            .is_empty();
        let (commits, off) = match unkept(&self.path, &self.repo, self.no_remote) {
            Ok(v) => v,
            Err(_) => {
                self.err = "couldn't compare with the remote".into();
                return (files, Vec::new(), false);
            }
        };
        self.unpushed = commits.len();
        self.off_remote = off;
        self.find_landed();
        (files, commits, true)
    }

    /// Whether HEAD is on its target, here or on origin, and in its merged
    /// pull request's head (verdict.go:125 landed).
    fn find_landed(&mut self) {
        self.landed = false;
        self.in_pr = false;
        self.own_work = false;
        self.head = git(&self.path, &["rev-parse", "HEAD"]).unwrap_or_default();
        if !self.target.is_empty() && self.branch != self.target {
            for r in [
                format!("refs/remotes/origin/{}", self.target),
                format!("refs/heads/{}", self.target),
            ] {
                if git(&self.path, &["merge-base", "--is-ancestor", "HEAD", &r]).is_ok() {
                    self.landed = true;
                    break;
                }
            }
        }
        if self.landed && !self.base.is_empty() && !self.head.is_empty() && self.head != self.base {
            self.own_work = own_work(&self.path, &self.base);
        }
        if let Some(p) = &self.pr
            && p.merged()
            && !p.head_oid.is_empty()
        {
            let oid = p.head_oid.clone();
            self.in_pr = git(&self.path, &["merge-base", "--is-ancestor", "HEAD", &oid]).is_ok();
        }
    }

    /// Whether its work has landed: its pull request merged, or its
    /// branch on its target with work of its own since it began.
    pub fn finished(&self) -> (bool, String) {
        if self.checked == 0 || !self.err.is_empty() {
            return (false, String::new());
        }
        if self.merged_pr() {
            let n = self.pr.as_ref().map_or(0, |p| p.number);
            return (true, format!("pull request #{n} merged"));
        }
        if self.landed && self.own_work {
            return (true, format!("its branch is on {}", self.target));
        }
        (false, String::new())
    }
}

/// HEAD at path holds a commit made in that worktree since base.
fn own_work(path: &str, base: &str) -> bool {
    let Ok(log) = git(path, &["reflog", "show", "--format=%H %gs", "HEAD"]) else {
        return false;
    };
    let made: HashSet<&str> = log
        .lines()
        .filter_map(|l| {
            let (sha, how) = l.split_once(' ').unwrap_or((l, ""));
            made_here(how).then_some(sha)
        })
        .collect();
    if made.is_empty() {
        return false;
    }
    let range = format!("{base}..HEAD");
    let Ok(since) = git(path, &["rev-list", &range]) else {
        return false;
    };
    since.split_whitespace().any(|s| made.contains(s))
}

/// A reflog subject for a commit made by hand in the worktree.
pub fn made_here(how: &str) -> bool {
    if ["commit", "cherry-pick", "revert"]
        .iter()
        .any(|p| how.starts_with(p))
    {
        return true;
    }
    how.starts_with("rebase")
        && [
            "(pick)",
            "(continue)",
            "(squash)",
            "(fixup)",
            "(reword)",
            "(edit)",
        ]
        .iter()
        .any(|p| how.contains(p))
}

/// The lines of git status --porcelain removing the worktree would lose,
/// and how many are tracked files gone from disk (" D"), which lose
/// nothing.
pub fn changes(status: &str) -> (Vec<String>, usize) {
    let (mut lost, mut deleted) = (Vec::new(), 0);
    for l in status.split('\n') {
        if l.is_empty() {
            continue;
        }
        if l.starts_with(" D ") {
            deleted += 1;
        } else {
            lost.push(l.to_string());
        }
    }
    (lost, deleted)
}

/// The worktree's commits nowhere else, as "sha subject", and how many
/// are on no remote, in main or not.
fn unkept(path: &str, repo: &str, no_remote: bool) -> Result<(Vec<String>, usize), String> {
    let mut args = vec!["rev-list", "HEAD"];
    if !no_remote {
        args.extend(["--not", "--remotes"]);
    }
    let out = git(path, &args)?;
    if out.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let shas: Vec<&str> = out.split_whitespace().collect();
    let short = |s: &str| s[..s.len().min(7)].to_string();
    let all = || (shas.iter().map(|s| short(s)).collect(), shas.len());
    let Ok(head) = git(repo, &["rev-parse", "HEAD"]) else {
        return Ok(all());
    };
    let Ok(cherry) = git(path, &["cherry", "-v", &head, "HEAD"]) else {
        return Ok(all());
    };
    let mut unique: HashMap<&str, &str> = HashMap::new();
    for l in cherry.lines() {
        if let Some(rest) = l.strip_prefix("+ ") {
            let (sha, subject) = rest.split_once(' ').unwrap_or((rest, ""));
            unique.insert(sha, subject);
        }
    }
    let lost = shas
        .iter()
        .filter_map(|s| unique.get(s).map(|subj| format!("{} {subj}", short(s))))
        .collect();
    Ok((lost, shas.len()))
}

/// One agent's say in which worktrees there are: its key, name, the
/// folders it worked in, and whether it's in use now.
#[derive(Clone, Debug, Default)]
pub struct WorkerOf {
    pub key: String,
    pub name: String,
    pub dirs: Vec<String>,
    pub in_use: bool,
}

/// The linked worktrees of every repository the agents work in, each with
/// the agents working in it. Nothing is checked yet. pr finds a branch's
/// pull request as GitHub last said, never waiting.
pub fn find_worktrees(agents: &[WorkerOf], pr: &dyn Fn(&str, &str) -> Option<PR>) -> Vec<Worktree> {
    let mut repos: Vec<String> = Vec::new();
    let mut seen = HashMap::new();
    for a in agents {
        for d in &a.dirs {
            let m = main_checkout_seen(d, &mut seen);
            if !m.is_empty() && !repos.contains(&m) {
                repos.push(m);
            }
        }
    }
    let mut out = Vec::new();
    for repo in &repos {
        let Ok(list) = git(repo, &["worktree", "list", "--porcelain"]) else {
            continue;
        };
        let target = crate::git_at(repo).1;
        out.extend(parse_worktree_list(&list, repo, &target, pr));
    }
    for w in &mut out {
        for a in agents {
            if a.dirs.iter().any(|d| crate::within(&w.path, d)) {
                w.agents.push(a.key.clone());
                if w.in_use.is_empty() && a.in_use {
                    w.in_use = first_non_empty(&a.name, "a session").to_string();
                }
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// git worktree list --porcelain, read: the linked ones, not repo itself
/// nor one whose folder has gone.
pub fn parse_worktree_list(
    list: &str,
    repo: &str,
    target: &str,
    pr: &dyn Fn(&str, &str) -> Option<PR>,
) -> Vec<Worktree> {
    let mut out = Vec::new();
    let mut w: Option<Worktree> = None;
    let flush = |w: &mut Option<Worktree>, out: &mut Vec<Worktree>| {
        if let Some(t) = w.take()
            && t.path != repo
        {
            out.push(t);
        }
    };
    for l in list.split('\n') {
        if let Some(p) = l.strip_prefix("worktree ") {
            flush(&mut w, &mut out);
            w = Some(Worktree {
                path: p.to_string(),
                repo: repo.to_string(),
                claude: p.contains("/.claude/worktrees/"),
                target: target.to_string(),
                made: made_by(p, "").to_string(),
                ..Worktree::default()
            });
            continue;
        }
        let Some(t) = w.as_mut() else { continue };
        if let Some(b) = l.strip_prefix("branch ") {
            t.branch = b.trim_start_matches("refs/heads/").to_string();
            t.made = made_by(&t.path, &t.branch).to_string();
            t.own = t.made == MADE_BY_RUSH;
            t.pr = pr(repo, &t.branch);
        } else if l == "locked" || l.starts_with("locked ") {
            t.locked = true;
        } else if l == "prunable" || l.starts_with("prunable ") {
            w = None; // its folder is already gone
        }
    }
    flush(&mut w, &mut out);
    out
}

/// The main checkout of the repository dir is in, by looking for .git
/// upwards: stats only. "" when it isn't in one.
pub fn main_checkout(dir: &str) -> String {
    main_checkout_seen(dir, &mut HashMap::new())
}

fn main_checkout_seen(dir: &str, seen: &mut HashMap<String, String>) -> String {
    let mut walked = Vec::new();
    let mut main = String::new();
    let mut d = Path::new(dir);
    loop {
        let s = d.to_string_lossy();
        if s == "/" || s == "." || s.is_empty() {
            break;
        }
        if let Some(m) = seen.get(s.as_ref()) {
            main = m.clone();
            break;
        }
        walked.push(s.to_string());
        let p = d.join(".git");
        if let Ok(st) = std::fs::metadata(&p) {
            if st.is_dir() {
                main = s.to_string();
                break;
            }
            let b = std::fs::read_to_string(&p).unwrap_or_default();
            let g = b.trim().trim_start_matches("gitdir:").trim();
            let g = if g.starts_with('/') {
                g.to_string()
            } else {
                d.join(g).to_string_lossy().into_owned()
            };
            if let Some(i) = g.find("/.git/worktrees/") {
                main = g[..i].to_string();
            }
            break;
        }
        match d.parent() {
            Some(p) => d = p,
            None => break,
        }
    }
    if !main.is_empty() {
        for w in walked {
            seen.insert(w, main.clone());
        }
    }
    main
}

/// Refuses anything that isn't a linked worktree of its repo: only a
/// linked worktree is ever removed, forced or not.
pub fn linked_worktree(w: &Worktree) -> Result<(), String> {
    let path = w.path.trim_end_matches('/');
    let repo = w.repo.trim_end_matches('/');
    if path == repo
        || format!("{repo}/").starts_with(&format!("{path}/"))
        || path.is_empty()
        || repo.is_empty()
    {
        return Err(format!(
            "{} is the repository itself, not a worktree; it's never removed",
            w.path
        ));
    }
    let dotgit = Path::new(path).join(".git");
    match std::fs::symlink_metadata(&dotgit) {
        Ok(m) if !m.is_dir() => {}
        _ => {
            return Err(format!(
                "{path} isn't a linked worktree; it's never removed"
            ));
        }
    }
    let b = std::fs::read_to_string(&dotgit).unwrap_or_default();
    let g = b.trim().trim_start_matches("gitdir:").trim();
    if !g.starts_with(&format!("{repo}/.git/worktrees/")) {
        return Err(format!(
            "{path} doesn't belong to {repo} as a worktree; it's never removed"
        ));
    }
    Ok(())
}

/// Removes a worktree through git; its branch stays. Unless force, git
/// refuses one with uncommitted changes, and this one with unpushed
/// commits.
pub fn remove_worktree(w: &Worktree, force: bool) -> Result<(), String> {
    linked_worktree(w)?;
    let mut deleted = false;
    if !force {
        let mut c = w.clone();
        let _ = c.look();
        deleted = c.deleted > 0;
        if !c.safe() {
            let why = if c.losses().is_empty() {
                c.err.clone()
            } else {
                c.losses()
            };
            return Err(format!("{} isn't safe to remove: {why}", base(&w.path)));
        }
    }
    let mut args = vec!["worktree", "remove"];
    if deleted {
        args.push("--force");
    }
    if force {
        args.extend(["--force", "--force"]);
    }
    args.push(&w.path);
    git_within(Duration::ZERO, &w.repo, &args)
        .map(|_| ())
        .map_err(|e| format!("git: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(w: Worktree) -> Worktree {
        Worktree { checked: 1, ..w }
    }

    #[test]
    fn verdicts_by_the_one_rule() {
        let cases = [
            (
                Worktree {
                    in_use: "fix".into(),
                    ..Worktree::default()
                },
                VerdictKind::KeepInUse,
                "fix is working in it",
            ),
            (
                Worktree::default(),
                VerdictKind::WouldLose,
                "not looked at with git yet",
            ),
            (
                checked(Worktree {
                    changed: 2,
                    ..Worktree::default()
                }),
                VerdictKind::WouldLose,
                "2 files with uncommitted changes",
            ),
            (
                checked(Worktree {
                    landed: true,
                    target: "main".into(),
                    ..Worktree::default()
                }),
                VerdictKind::CanGo,
                "its work is on main",
            ),
            (
                checked(Worktree {
                    pr: Some(PR {
                        number: 7,
                        state: "open".into(),
                        ..PR::default()
                    }),
                    ..Worktree::default()
                }),
                VerdictKind::KeepPrOpen,
                "its pull request #7 is open",
            ),
            (
                checked(Worktree {
                    off_remote: 2,
                    ..Worktree::default()
                }),
                VerdictKind::CanGo,
                "every commit is in main, not pushed",
            ),
            (
                checked(Worktree::default()),
                VerdictKind::CanGo,
                "every commit is pushed",
            ),
            (
                checked(Worktree {
                    unpushed: 1,
                    locked: true,
                    ..Worktree::default()
                }),
                VerdictKind::WouldLose,
                "1 commit not pushed or in main · a lock someone put on it",
            ),
        ];
        for (w, kind, why) in cases {
            let v = w.verdict();
            assert_eq!((v.kind, v.reason.as_str()), (kind, why));
        }
    }

    #[test]
    fn who_made_it() {
        for (path, branch, want) in [
            ("/r/.claude/worktrees/ab12", "rush/ab12", MADE_BY_RUSH),
            ("/r/.claude/worktrees/agent-1", "x", MADE_BY_SUBAGENT),
            ("/r/.claude/worktrees/fix", "fix", MADE_BY_CLAUDE),
            ("/r/.worktrees/spike", "spike", MADE_BY_OTHER),
        ] {
            assert_eq!(made_by(path, branch), want, "{path}");
        }
    }

    #[test]
    fn porcelain_lists_linked_only() {
        let list = "worktree /r\nHEAD x\nbranch refs/heads/main\n\nworktree /r/.claude/worktrees/a\nbranch refs/heads/rush/a\nlocked\n\nworktree /r/gone\nbranch refs/heads/g\nprunable gitdir file points to non-existent location\n";
        let got = parse_worktree_list(list, "/r", "main", &|_, b| {
            (b == "rush/a").then(|| PR {
                number: 3,
                ..PR::default()
            })
        });
        assert_eq!(got.len(), 1);
        let w = &got[0];
        assert_eq!(w.branch, "rush/a");
        assert!(w.own && w.locked && w.claude);
        assert_eq!(w.target, "main");
        assert_eq!(w.pr.as_ref().unwrap().number, 3);
    }

    #[test]
    fn status_lines_lost() {
        let (lost, del) = changes(" D gone.go\n M kept.go\n?? new.go\n");
        assert_eq!(lost, vec![" M kept.go", "?? new.go"]);
        assert_eq!(del, 1);
    }

    #[test]
    fn reflog_made_here() {
        assert!(made_here("commit: fix"));
        assert!(made_here("rebase (pick): x"));
        assert!(!made_here("rebase (start): checkout main"));
        assert!(!made_here("reset: moving to HEAD~1"));
    }

    #[test]
    fn never_the_repository_itself() {
        let w = Worktree {
            path: "/r".into(),
            repo: "/r".into(),
            ..Worktree::default()
        };
        assert!(
            linked_worktree(&w)
                .unwrap_err()
                .contains("repository itself")
        );
        let w = Worktree {
            path: "/".into(),
            repo: "/r".into(),
            ..Worktree::default()
        };
        assert!(linked_worktree(&w).is_err());
    }
}
