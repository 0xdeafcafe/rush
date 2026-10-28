//! Pull requests by branch: a session works on one branch, and its pull
//! request is the one GitHub has for it, whichever agent made it. One gh pr
//! list per repository covers every session in it: asked in the
//! background, kept here for the process, and asked again at most every
//! BRANCH_EVERY while something reads it, or at once on refresh.
//!
//! Ported from internal/ghpr/branches.go.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rush_core::time::Ms;
use serde::Deserialize;

use super::{Error, Gh, RawChk, check, gh_err, null_or};
use crate::rfc3339::{de_time, null_default};

/// Status is a pull request as a session needs it, found by its branch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// its head branch
    pub branch: String,
    /// open, draft, merged or closed
    pub state: String,
    /// checks is how its checks went, all told: pass, fail or pending; ""
    /// when it has none. failing names the first that failed.
    pub checks: String,
    pub failing: String,
    /// review is approved or changes (requested); "" while neither.
    pub review: String,
    /// the commit GitHub has at its head
    pub head_oid: String,
    /// when it was merged; 0 when it wasn't
    pub merged_at: Ms,
}

impl Status {
    /// merged is a pull request merged, squashed or not.
    pub fn merged(&self) -> bool {
        self.state == "merged"
    }

    /// open is a pull request still open, draft or not.
    pub fn open(&self) -> bool {
        self.state == "open" || self.state == "draft"
    }
}

/// BRANCH_EVERY is how often, at most, a repository's pull requests are
/// asked again while they're read.
pub const BRANCH_EVERY: Duration = Duration::from_secs(60);

/// branch_fields are what gh is asked for of each pull request by branch.
const BRANCH_FIELDS: &str = "number,title,state,headRefName,headRefOid,isDraft,statusCheckRollup,reviewDecision,mergedAt,url";

/// Pull requests by head branch, shared with every reader.
pub type ByBranch = Arc<HashMap<String, Status>>;

/// BranchRepo is what GitHub last said of one repository's pull requests.
#[derive(Default)]
pub(crate) struct BranchRepo {
    /// when it was last asked; None before
    at: Option<Instant>,
    by: Option<ByBranch>,
    /// why it couldn't say, last time: no gh, not GitHub, signed out
    err: String,
    asking: bool,
}

impl Gh {
    /// by_branch is root's pull requests by head branch, as GitHub last
    /// said them; it never waits on GitHub. ok is false until GitHub has
    /// answered once, and while gh can't (not installed, not on GitHub,
    /// signed out). An answer older than BRANCH_EVERY is asked for again in
    /// the background.
    pub fn by_branch(&self, root: &str) -> (Option<ByBranch>, bool) {
        if root.is_empty() {
            return (None, false);
        }
        let mut repos = self.inner.branches.lock().unwrap();
        let r = repos.entry(root.to_string()).or_default();
        let mut every = BRANCH_EVERY;
        if !r.err.is_empty() {
            every *= 10; // no gh, or not on GitHub: that changes seldom
        }
        if !r.asking && r.at.is_none_or(|at| at.elapsed() >= every) {
            self.ask(root, r);
        }
        (r.by.clone(), r.by.is_some() && r.err.is_empty())
    }

    /// for_branch is the pull request GitHub has for branch in root, as
    /// last said.
    pub fn for_branch(&self, root: &str, branch: &str) -> Option<Status> {
        if branch.is_empty() {
            return None;
        }
        let (by, _) = self.by_branch(root);
        by?.get(branch).cloned()
    }

    /// refresh asks GitHub again about root's pull requests now, in the
    /// background, however recently it was: on demand, as when a session's
    /// pull request is opened or merged from rush.
    pub fn refresh(&self, root: &str) {
        if root.is_empty() {
            return;
        }
        let mut repos = self.inner.branches.lock().unwrap();
        let r = repos.entry(root.to_string()).or_default();
        if !r.asking {
            self.ask(root, r);
        }
    }

    /// look_branches asks GitHub about root's pull requests and waits for
    /// the answer, keeping it for by_branch too; one fresher than
    /// BRANCH_EVERY is used as it is. For what runs off the UI and must know
    /// now: git-clean.
    pub fn look_branches(&self, root: &str) -> Result<ByBranch, Error> {
        {
            let repos = self.inner.branches.lock().unwrap();
            if let Some(r) = repos.get(root)
                && let Some(by) = &r.by
                && r.err.is_empty()
                && r.at.is_some_and(|at| at.elapsed() < BRANCH_EVERY)
            {
                return Ok(by.clone());
            }
        }
        let got = self.list_branches(root).map(Arc::new);
        self.keep(root, got.clone());
        got
    }

    /// ask asks GitHub in the background. Called with branches held.
    fn ask(&self, root: &str, r: &mut BranchRepo) {
        r.asking = true;
        let gh = self.clone();
        let root = root.to_string();
        std::thread::spawn(move || {
            let got = {
                let _pass = gh.inner.gate.enter();
                gh.list_branches(&root).map(Arc::new)
            };
            gh.keep(&root, got);
        });
    }

    /// keep takes in GitHub's answer for root. One that failed keeps what it
    /// said before, and is asked again after BRANCH_EVERY like any other.
    fn keep(&self, root: &str, got: Result<ByBranch, Error>) {
        {
            let mut repos = self.inner.branches.lock().unwrap();
            let r = repos.entry(root.to_string()).or_default();
            r.asking = false;
            r.at = Some(Instant::now());
            r.err.clear();
            match got {
                Ok(by) => r.by = Some(by),
                Err(e) => r.err = e.to_string(),
            }
        }
        let changed = self.inner.changed.lock().unwrap().clone();
        if let Some(f) = changed {
            f(root);
        }
    }

    /// list_branches asks gh for root's last hundred pull requests, of any
    /// state, by head branch.
    fn list_branches(&self, root: &str) -> Result<HashMap<String, Status>, Error> {
        let out = self
            .gh(
                root,
                &[
                    "pr",
                    "list",
                    "--state",
                    "all",
                    "--limit",
                    "100",
                    "--json",
                    BRANCH_FIELDS,
                ],
            )
            .map_err(gh_err)?;
        parse_branches(&out)
    }
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawBranchPR {
    #[serde(deserialize_with = "null_default")]
    number: u64,
    #[serde(deserialize_with = "null_default")]
    title: String,
    #[serde(deserialize_with = "null_default")]
    url: String,
    #[serde(deserialize_with = "null_default")]
    state: String,
    #[serde(deserialize_with = "null_default")]
    is_draft: bool,
    #[serde(deserialize_with = "null_default")]
    head_ref_name: String,
    #[serde(deserialize_with = "null_default")]
    head_ref_oid: String,
    #[serde(rename = "reviewDecision", deserialize_with = "null_default")]
    decision: String,
    #[serde(rename = "statusCheckRollup", deserialize_with = "null_default")]
    rollup: Vec<RawChk>,
    #[serde(deserialize_with = "de_time")]
    merged_at: Ms,
}

/// parse_branches reads gh pr list's JSON as pull requests by head branch.
/// A branch with several keeps the one that matters most: open, then
/// merged, then closed; the newest of those.
pub fn parse_branches(b: &[u8]) -> Result<HashMap<String, Status>, Error> {
    let raw: Vec<RawBranchPR> = null_or(b)?;
    let mut by: HashMap<String, Status> = HashMap::with_capacity(raw.len());
    for r in raw {
        let mut s = Status {
            number: r.number,
            title: r.title,
            url: r.url,
            branch: r.head_ref_name,
            head_oid: r.head_ref_oid,
            merged_at: r.merged_at,
            ..Default::default()
        };
        s.state = match r.state.as_str() {
            "MERGED" => "merged",
            "CLOSED" => "closed",
            _ if r.is_draft => "draft",
            _ => "open",
        }
        .into();
        s.review = match r.decision.as_str() {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "changes",
            _ => "",
        }
        .into();
        for c in r.rollup.iter().map(check) {
            if c.state == "failed" {
                if s.checks != "fail" {
                    s.checks = "fail".into();
                    s.failing = c.name;
                }
            } else if c.state == "pending" && s.checks != "fail" {
                s.checks = "pending".into();
            } else if s.checks.is_empty() {
                s.checks = "pass".into();
            }
        }
        if let Some(was) = by.get(&s.branch)
            && !matters(&s, was)
        {
            continue;
        }
        by.insert(s.branch.clone(), s);
    }
    Ok(by)
}

/// matters is whether a matters more than b for their branch: open beats
/// merged beats closed, then the newer.
fn matters(a: &Status, b: &Status) -> bool {
    let rank = |s: &Status| {
        if s.open() {
            2
        } else if s.merged() {
            1
        } else {
            0
        }
    };
    if rank(a) != rank(b) {
        return rank(a) > rank(b);
    }
    a.number > b.number
}
