//! One rule for every clean-up. Port of internal/fleet/verdict.go, and
//! host/workspace.go's IsOwnWorktree (:36).
//!
//! A worktree can go when its work is on its target branch (an ancestor of
//! origin/<target> or <target>, or its pull request merged, squashed or
//! not), or every commit is pushed, with nothing uncommitted, and no
//! session works in it. Projects, Clean up, Delete, the automatic tidy-up
//! and git-clean all ask verdict (or landed_branches, for branches alone);
//! only worktrees rush made for a session (own) ever go without being
//! ticked by hand.

use std::collections::HashMap;
use std::path::Path;

use rush_core::agent::PrStatus;
use serde_json::json;

use crate::worktree::{Worktree, first_non_empty, git, remove, worktree_event};

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

impl VerdictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            VerdictKind::KeepInUse => "keep-in-use",
            VerdictKind::KeepPrOpen => "keep-pr-open",
            VerdictKind::CanGo => "can-go",
            VerdictKind::WouldLose => "would-lose",
        }
    }
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

/// Who made a worktree, by where it is and its branch.
/// A session's own: <main>/.claude/worktrees/<id> on rush/<id>.
pub const MADE_BY_RUSH: &str = "rush";
/// A subagent's: .claude/worktrees/agent-*.
pub const MADE_BY_SUBAGENT: &str = "subagent";
/// The harness's other ones in .claude/worktrees (verdict.go:51).
pub const MADE_BY_CLAUDE: &str = "harness";
/// Anywhere else: .worktrees/, ~/worktrees, by hand.
pub const MADE_BY_OTHER: &str = "other";

fn base(p: &str) -> &str {
    let p = p.trim_end_matches('/');
    p.rsplit('/').next().unwrap_or(p)
}

fn dir(p: &str) -> &str {
    let p = p.trim_end_matches('/');
    match p.rfind('/') {
        Some(0) => "/",
        Some(i) => &p[..i],
        None => ".",
    }
}

/// Whether a worktree at path on branch is one rush made for a session:
/// <main>/.claude/worktrees/<id> on rush/<id>.
pub fn is_own_worktree(path: &str, branch: &str) -> bool {
    let id = base(path);
    branch == format!("rush/{id}")
        && base(dir(path)) == "worktrees"
        && base(dir(dir(path))) == ".claude"
}

/// Who made the worktree at path on branch.
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

impl Worktree {
    /// What clean-up may do with it, by the one rule. It reads only what
    /// check found and find_worktrees said: no git.
    pub fn verdict(&self) -> Verdict {
        let v = |kind, reason: String| Verdict { kind, reason };
        if !self.in_use.is_empty() {
            return v(
                VerdictKind::KeepInUse,
                format!("{} is working in it", self.in_use),
            );
        }
        if self.checked == 0 {
            return v(VerdictKind::WouldLose, "not looked at with git yet".into());
        }
        if !self.err.is_empty() {
            return v(VerdictKind::WouldLose, self.err.clone());
        }
        if !self.safe() {
            return v(VerdictKind::WouldLose, self.losses());
        }
        if self.merged_pr() {
            let n = self.pr.as_ref().map_or(0, |p| p.number);
            return v(
                VerdictKind::CanGo,
                format!("its pull request #{n} was merged"),
            );
        }
        if self.landed {
            return v(
                VerdictKind::CanGo,
                format!("its work is on {}", self.target),
            );
        }
        if let Some(p) = &self.pr
            && p.open()
        {
            return v(
                VerdictKind::KeepPrOpen,
                format!("its pull request #{} is {}", p.number, p.state),
            );
        }
        if self.no_remote || self.off_remote > 0 {
            return v(
                VerdictKind::CanGo,
                format!(
                    "every commit is in {}, not pushed",
                    first_non_empty(&[&self.target, "main"])
                ),
            );
        }
        v(VerdictKind::CanGo, "every commit is pushed".into())
    }

    /// Its pull request was merged with everything it has: its HEAD is in
    /// the pull request's head.
    pub(crate) fn merged_pr(&self) -> bool {
        self.pr.as_ref().is_some_and(|p| p.merged()) && self.in_pr
    }

    /// Its work has landed: its pull request merged, or its branch on its
    /// target with work of its own since it began (base).
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

    /// Looks with git at whether its work has landed (finished), the cheap
    /// way first. Never on the UI's thread.
    pub fn landing(&mut self) -> (bool, String) {
        let merged = self
            .pr
            .as_ref()
            .is_some_and(|p| p.merged() && !p.head_oid.is_empty());
        if !merged {
            self.landed();
            if !self.landed || !self.own_work {
                return (false, String::new());
            }
        }
        self.check_git();
        self.finished()
    }

    /// Looks with git at whether HEAD is on its target, here or on origin,
    /// and whether it's in its merged pull request's head.
    pub(crate) fn landed(&mut self) {
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
            self.in_pr = git(
                &self.path,
                &["merge-base", "--is-ancestor", "HEAD", &p.head_oid],
            )
            .is_ok();
        }
    }
}

/// Whether HEAD at path holds a commit made in that worktree since base:
/// one its HEAD's reflog says was committed, cherry-picked, reverted or
/// picked by a rebase there. A reflog git can't read says no.
fn own_work(path: &str, base: &str) -> bool {
    let Ok(log) = git(path, &["reflog", "show", "--format=%H %gs", "HEAD"]) else {
        return false;
    };
    let made: Vec<&str> = log
        .split('\n')
        .filter_map(|l| {
            let (sha, how) = l.split_once(' ').unwrap_or((l, ""));
            made_here(how).then_some(sha)
        })
        .collect();
    if made.is_empty() {
        return false;
    }
    let Ok(since) = git(path, &["rev-list", &format!("{base}..HEAD")]) else {
        return false;
    };
    since.split_whitespace().any(|s| made.contains(&s))
}

/// A reflog subject for a commit made by hand in the worktree.
pub(crate) fn made_here(how: &str) -> bool {
    if ["commit", "cherry-pick", "revert"]
        .iter()
        .any(|p| how.starts_with(p))
    {
        return true;
    }
    if !how.starts_with("rebase") {
        return false;
    }
    [
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

/// repo's local branches, checked out nowhere, whose work is on target by
/// the same rule as verdict: in origin/<target> or <target>, or in the
/// head of their merged pull request (prs, by branch). Deleting them loses
/// nothing. It runs git: never on the UI's thread.
pub fn landed_branches(repo: &str, target: &str, prs: &HashMap<String, PrStatus>) -> Vec<String> {
    let Ok(out) = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)\t%(worktreepath)",
            "refs/heads",
        ],
    ) else {
        return Vec::new();
    };
    let mut merged: Vec<String> = Vec::new();
    for r in [
        format!("refs/remotes/origin/{target}"),
        format!("refs/heads/{target}"),
    ] {
        if let Ok(m) = git(
            repo,
            &[
                "for-each-ref",
                "--merged",
                &r,
                "--format=%(refname:short)",
                "refs/heads",
            ],
        ) {
            merged.extend(m.split('\n').map(str::to_string));
        }
    }
    let mut landed = Vec::new();
    for l in out.split('\n') {
        let (name, wt) = l.split_once('\t').unwrap_or((l, ""));
        if name.is_empty() || name == target || !wt.is_empty() {
            continue;
        }
        if !merged.iter().any(|m| m == name) {
            let Some(p) = prs.get(name) else { continue };
            if !p.merged() || p.head_oid.is_empty() {
                continue;
            }
            if git(
                repo,
                &[
                    "merge-base",
                    "--is-ancestor",
                    &format!("refs/heads/{name}"),
                    &p.head_oid,
                ],
            )
            .is_err()
            {
                continue;
            }
        }
        landed.push(name.to_string());
    }
    landed.sort();
    landed
}

/// What's done when a worktree's work has landed: the worktree removed,
/// if rush made it and the rule says it can go, its branch deleted (here
/// only), and the main checkout fast-forwarded to its target, if it's
/// clean and on it. It says what it did. Never on the UI's thread.
pub fn settle(w: &Worktree) -> Result<Vec<String>, String> {
    if !w.own {
        return Ok(Vec::new()); // not rush's: only ever removed by hand
    }
    let name = base(&w.path).to_string();
    let v = w.verdict();
    if !v.can_go() {
        return Err(format!("{name} stays: {}", v.reason));
    }
    let mut did = Vec::new();
    if Path::new(&w.path).exists() {
        if let Err(e) = remove(w, false, false) {
            worktree_event(
                "worktree-settled",
                w,
                json!({"why": "its work landed"}),
                Some(&e),
            );
            return Err(e);
        }
        did.push(format!("removed {name}"));
    }
    if !w.branch.is_empty()
        && w.branch != w.target
        && git(&w.repo, &["branch", "-D", &w.branch]).is_ok()
    {
        // -D: a squash-merged branch isn't merged as git sees it, but the
        // verdict has said its work is on the target.
        did.push(format!("deleted branch {}", w.branch));
    }
    if let Ok(true) = fast_forward(&w.repo, &w.target) {
        did.push(format!(
            "brought {} up to date with {}",
            base(&w.repo),
            w.target
        ));
    }
    worktree_event(
        "worktree-settled",
        w,
        json!({"why": "its work landed", "did": did}),
        None,
    );
    Ok(did)
}

/// Brings the main checkout at repo up to origin's target with git merge
/// --ff-only, and nothing else: only when it's on target with nothing
/// uncommitted. It fetches target first. Ok(true) is that it moved.
pub fn fast_forward(repo: &str, target: &str) -> Result<bool, String> {
    if target.is_empty() {
        return Ok(false);
    }
    match git(repo, &["symbolic-ref", "--short", "-q", "HEAD"]) {
        Ok(b) if b == target => {}
        _ => return Ok(false),
    }
    match git(repo, &["status", "--porcelain", "--untracked-files=no"]) {
        Ok(s) if s.is_empty() => {}
        _ => return Ok(false),
    }
    let remotes = git(repo, &["remote"]).unwrap_or_default();
    if !remotes.split_whitespace().any(|r| r == "origin") {
        return Ok(false);
    }
    git(repo, &["fetch", "--quiet", "origin", target])?;
    let before = git(repo, &["rev-parse", "HEAD"]).unwrap_or_default();
    git(
        repo,
        &[
            "merge",
            "--ff-only",
            "--quiet",
            &format!("refs/remotes/origin/{target}"),
        ],
    )?;
    let after = git(repo, &["rev-parse", "HEAD"]).unwrap_or_default();
    if after != before {
        rush_sys::events::log(
            "main-fast-forwarded",
            json!({"repo": repo, "target": target, "from": before, "to": after}),
        );
    }
    Ok(after != before)
}
