//! ghpr asks GitHub, through the gh command, about a repository's pull
//! requests that concern you: those you opened, and those waiting on your
//! review. It runs gh, so never on the UI.
//!
//! Ported from internal/ghpr/ghpr.go. Go keeps its answers in package
//! variables; here they live in a [`Gh`], one per runner: [`Gh::shared`]
//! is the process's, on the real gh, and tests make their own on a fake.

mod act;
mod branches;
mod run;

pub use act::Comment;
pub use branches::{BRANCH_EVERY, ByBranch, Status, parse_branches};
pub use run::{Error, MergeError, RunError, Runner, SystemGh, gh_err, merge_err};

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use rush_core::time::{Ms, now};
use serde::Deserialize;

use crate::rfc3339::{de_time, null_default};

/// fields are what gh is asked for of each pull request (ghpr.go:19).
const FIELDS: &str = "number,title,url,state,isDraft,author,headRefName,baseRefName,reviewDecision,reviewRequests,latestReviews,statusCheckRollup,updatedAt,body,additions,deletions,changedFiles,comments";

/// Check is one check on a pull request's last commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub workflow: String,
    /// passed, failed, pending or skipped
    pub state: String,
    pub url: String,
}

/// Review is someone's last word on a pull request: APPROVED,
/// CHANGES_REQUESTED, COMMENTED or DISMISSED.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Review {
    pub who: String,
    pub state: String,
    pub body: String,
}

/// PR is a pull request as GitHub says it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PR {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// OPEN, MERGED or CLOSED
    pub state: String,
    pub draft: bool,
    pub author: String,
    pub head: String,
    pub base: String,
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED or ""
    pub decision: String,
    /// who is asked to review and hasn't: logins, or teams' names
    pub waiting: Vec<String>,
    pub reviews: Vec<Review>,
    pub checks: Vec<Check>,
    pub updated: Ms,
    pub body: String,
    pub mine: bool,
    pub asked_you: bool,
    /// added and removed are its lines, files the files it changes, and
    /// comments the comments on its conversation.
    pub added: u64,
    pub removed: u64,
    pub files: u64,
    pub comments: usize,
}

impl PR {
    /// Counts are a pull request's checks by how they went: passed, failed,
    /// pending.
    pub fn counts(&self) -> (usize, usize, usize) {
        let (mut passed, mut failed, mut pending) = (0, 0, 0);
        for c in &self.checks {
            match c.state.as_str() {
                "passed" | "skipped" => passed += 1,
                "failed" => failed += 1,
                _ => pending += 1,
            }
        }
        (passed, failed, pending)
    }
}

/// Repo is what GitHub said of one repository's pull requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Repo {
    pub root: String,
    pub prs: Vec<PR>,
    /// why it couldn't say: no gh, not on GitHub, signed out
    pub err: String,
    pub at: Ms,
    /// next is when look asks GitHub again: BRANCH_EVERY after an answer,
    /// longer after each failure in a row (see retry_in).
    pub next: Ms,
    fails: u32,
}

/// retry_in is how long after an answer GitHub is asked again:
/// BRANCH_EVERY, doubled for each failure in a row up to a quarter of an
/// hour, and never sooner than ten minutes once GitHub has said it's being
/// asked too much.
pub fn retry_in(fails: u32, err: &str) -> Duration {
    let mut d = BRANCH_EVERY * (1 << fails.min(4));
    d = d.min(Duration::from_secs(15 * 60));
    if err.to_lowercase().contains("rate limit") {
        d = d.max(Duration::from_secs(10 * 60));
    }
    d
}

/// Gate lets at most a few ask GitHub at once (Go's buffered channel).
struct Gate {
    free: Mutex<usize>,
    freed: Condvar,
}

impl Gate {
    fn new(n: usize) -> Self {
        Gate {
            free: Mutex::new(n),
            freed: Condvar::new(),
        }
    }

    fn enter(&self) -> GatePass<'_> {
        let mut free = self.free.lock().unwrap();
        while *free == 0 {
            free = self.freed.wait(free).unwrap();
        }
        *free -= 1;
        GatePass(self)
    }
}

struct GatePass<'a>(&'a Gate);

impl Drop for GatePass<'_> {
    fn drop(&mut self) {
        *self.0.free.lock().unwrap() += 1;
        self.0.freed.notify_one();
    }
}

type OnChange = Arc<dyn Fn(&str) + Send + Sync>;

pub(crate) struct Inner {
    run: Box<dyn Runner>,
    /// What GitHub last said of each repository's pull requests by branch.
    branches: Mutex<HashMap<String, branches::BranchRepo>>,
    /// At most three ask GitHub at once.
    gate: Gate,
    /// lists is what list last said of each repository, by its name on
    /// GitHub (its folder when GitHub can't say), shared by everything in
    /// this process that shows it: a repository's every worktree asks once.
    lists: Mutex<HashMap<String, Repo>>,
    names: Mutex<HashMap<String, String>>,
    me: Mutex<String>,
    changed: Mutex<Option<OnChange>>,
}

/// Gh asks GitHub through one runner and keeps what it said.
#[derive(Clone)]
pub struct Gh {
    pub(crate) inner: Arc<Inner>,
}

impl Gh {
    /// A Gh asking through run, knowing nothing yet.
    pub fn new(run: impl Runner + 'static) -> Self {
        Gh {
            inner: Arc::new(Inner {
                run: Box::new(run),
                branches: Mutex::new(HashMap::new()),
                gate: Gate::new(3),
                lists: Mutex::new(HashMap::new()),
                names: Mutex::new(HashMap::new()),
                me: Mutex::new(String::new()),
                changed: Mutex::new(None),
            }),
        }
    }

    /// The process's Gh, on the real gh.
    pub fn shared() -> &'static Gh {
        static SHARED: OnceLock<Gh> = OnceLock::new();
        SHARED.get_or_init(|| Gh::new(SystemGh::default()))
    }

    /// on_change is told a repository's folder each time GitHub's answer
    /// about its pull requests by branch comes in, so a screen redraws then
    /// rather than asking again and again.
    pub fn on_change(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        *self.inner.changed.lock().unwrap() = Some(Arc::new(f));
    }

    pub(crate) fn gh(&self, dir: &str, args: &[&str]) -> Result<Vec<u8>, RunError> {
        self.inner.run.run(dir, args)
    }

    /// list asks gh, in the repository at root, for the open pull requests
    /// you opened and those asking for your review, yours first, then the
    /// most lately touched.
    pub fn list(&self, root: &str) -> Repo {
        let mut r = Repo {
            root: root.to_string(),
            at: now(),
            ..Default::default()
        };
        let mut mine = match self.list_search(root, "author:@me") {
            Ok(p) => p,
            Err(e) => {
                r.err = e.to_string();
                return r;
            }
        };
        let mut asked = match self.list_search(root, "review-requested:@me") {
            Ok(p) => p,
            Err(e) => {
                r.err = e.to_string();
                return r;
            }
        };
        // Asked of you by name, it's waiting on you.
        let me = self.login();
        if !me.is_empty() {
            for p in mine.iter_mut().chain(asked.iter_mut()) {
                for q in &mut p.waiting {
                    if *q == me {
                        *q = "you".to_string();
                    }
                }
            }
        }
        let mut by_num = HashMap::new();
        for mut p in mine {
            p.mine = true;
            by_num.insert(p.number, r.prs.len());
            r.prs.push(p);
        }
        for mut p in asked {
            if let Some(&j) = by_num.get(&p.number) {
                r.prs[j].asked_you = true;
                continue;
            }
            p.asked_you = true;
            r.prs.push(p);
        }
        r.prs
            .sort_by(|a, b| b.mine.cmp(&a.mine).then(b.updated.cmp(&a.updated)));
        r
    }

    /// look is list through a shared answer: one not due again (next) is
    /// used as it is, and GitHub is asked through the same gate as
    /// by_branch, so the Projects page and the branches never ask more than
    /// a few at once. One that fails keeps the pull requests the last answer
    /// had, and is asked again later each time it fails in a row.
    pub fn look(&self, root: &str) -> Repo {
        let mut key = self.repo_name(root);
        if key.is_empty() {
            key = root.to_string();
        }
        let old = self.inner.lists.lock().unwrap().get(&key).cloned();
        if let Some(old) = &old
            && now() < old.next
        {
            let mut old = old.clone();
            old.root = root.to_string();
            return old;
        }
        let old = old.unwrap_or_default();
        let mut r = {
            let _pass = self.inner.gate.enter();
            self.list(root)
        };
        if !r.err.is_empty() {
            r.fails = old.fails + 1;
            if !old.prs.is_empty() {
                r.prs = old.prs; // GitHub failing once doesn't lose what it said before
            }
        }
        r.next = r.at + retry_in(r.fails, &r.err).as_millis() as Ms;
        self.inner.lists.lock().unwrap().insert(key, r.clone());
        r
    }

    /// login is who gh is signed in as, asked once.
    fn login(&self) -> String {
        let mut me = self.inner.me.lock().unwrap();
        if me.is_empty()
            && let Ok(out) = self.gh("", &["api", "user", "--jq", ".login"])
        {
            *me = String::from_utf8_lossy(&out).trim().to_string();
        }
        me.clone()
    }

    fn list_search(&self, root: &str, search: &str) -> Result<Vec<PR>, Error> {
        let name = self.repo_name(root);
        let mut args = vec![
            "pr", "list", "--state", "open", "--limit", "30", "--search", search, "--json", FIELDS,
        ];
        if !name.is_empty() {
            args.extend(["--repo", name.as_str()]);
        }
        let out = self.gh(root, &args).map_err(gh_err)?;
        parse(&out)
    }

    /// repo_name is the name GitHub has for root's repository now, asked
    /// once: a remote still on a renamed repository's old name reaches it,
    /// but a search by that name finds nothing. "" when GitHub can't say.
    fn repo_name(&self, root: &str) -> String {
        if let Some(n) = self.inner.names.lock().unwrap().get(root) {
            return n.clone();
        }
        let Ok(out) = self.gh(
            root,
            &[
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "--jq",
                ".nameWithOwner",
            ],
        ) else {
            return String::new(); // asked again next time: GitHub may answer then
        };
        let name = String::from_utf8_lossy(&out).trim().to_string();
        self.inner
            .names
            .lock()
            .unwrap()
            .insert(root.to_string(), name.clone());
        name
    }

    /// forget drops every repository's answer, so the next read asks
    /// GitHub.
    pub fn forget(&self) {
        self.inner.branches.lock().unwrap().clear();
        self.inner.lists.lock().unwrap().clear();
        self.inner.names.lock().unwrap().clear();
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct Who {
    #[serde(deserialize_with = "null_default")]
    login: String,
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(deserialize_with = "null_default")]
    slug: String,
}

impl Who {
    pub(crate) fn words(&self) -> String {
        if !self.login.is_empty() {
            self.login.clone()
        } else if !self.slug.is_empty() {
            self.slug.clone()
        } else {
            self.name.clone()
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawRev {
    #[serde(deserialize_with = "null_default")]
    author: Who,
    #[serde(deserialize_with = "null_default")]
    state: String,
    #[serde(deserialize_with = "null_default")]
    body: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct RawChk {
    #[serde(rename = "__typename", deserialize_with = "null_default")]
    typ: String,
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(deserialize_with = "null_default")]
    context: String,
    #[serde(rename = "workflowName", deserialize_with = "null_default")]
    workflow: String,
    #[serde(deserialize_with = "null_default")]
    status: String,
    #[serde(deserialize_with = "null_default")]
    conclusion: String,
    #[serde(deserialize_with = "null_default")]
    state: String,
    #[serde(rename = "detailsUrl", deserialize_with = "null_default")]
    details_url: String,
    #[serde(rename = "targetUrl", deserialize_with = "null_default")]
    target_url: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawPR {
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
    author: Who,
    #[serde(deserialize_with = "null_default")]
    head_ref_name: String,
    #[serde(deserialize_with = "null_default")]
    base_ref_name: String,
    #[serde(rename = "reviewDecision", deserialize_with = "null_default")]
    decision: String,
    #[serde(rename = "reviewRequests", deserialize_with = "null_default")]
    requests: Vec<Who>,
    #[serde(rename = "latestReviews", deserialize_with = "null_default")]
    reviews: Vec<RawRev>,
    #[serde(rename = "statusCheckRollup", deserialize_with = "null_default")]
    rollup: Vec<RawChk>,
    #[serde(deserialize_with = "de_time")]
    updated_at: Ms,
    #[serde(deserialize_with = "null_default")]
    body: String,
    #[serde(deserialize_with = "null_default")]
    additions: u64,
    #[serde(deserialize_with = "null_default")]
    deletions: u64,
    #[serde(rename = "changedFiles", deserialize_with = "null_default")]
    files: u64,
    #[serde(deserialize_with = "null_default")]
    comments: Vec<serde::de::IgnoredAny>,
}

/// parse reads gh pr list's JSON.
pub fn parse(b: &[u8]) -> Result<Vec<PR>, Error> {
    let raw: Vec<RawPR> = null_or(b)?;
    Ok(raw
        .into_iter()
        .map(|r| PR {
            number: r.number,
            title: r.title,
            url: r.url,
            state: r.state,
            draft: r.is_draft,
            author: r.author.words(),
            head: r.head_ref_name,
            base: r.base_ref_name,
            decision: r.decision,
            updated: r.updated_at,
            body: r.body,
            added: r.additions,
            removed: r.deletions,
            files: r.files,
            comments: r.comments.len(),
            waiting: r.requests.iter().map(Who::words).collect(),
            reviews: r
                .reviews
                .iter()
                .map(|v| Review {
                    who: v.author.words(),
                    state: v.state.clone(),
                    body: v.body.clone(),
                })
                .collect(),
            checks: r.rollup.iter().map(check).collect(),
            mine: false,
            asked_you: false,
        })
        .collect())
}

/// A JSON list where null is an empty one, as Go reads it.
pub(crate) fn null_or<T: serde::de::DeserializeOwned + Default>(b: &[u8]) -> Result<T, Error> {
    Ok(serde_json::from_slice::<Option<T>>(b)?.unwrap_or_default())
}

/// check is a CheckRun or a commit status as one Check.
pub(crate) fn check(c: &RawChk) -> Check {
    if c.typ == "StatusContext" {
        let st = match c.state.as_str() {
            "SUCCESS" => "passed",
            "FAILURE" | "ERROR" => "failed",
            _ => "pending",
        };
        return Check {
            name: c.context.clone(),
            workflow: String::new(),
            state: st.into(),
            url: c.target_url.clone(),
        };
    }
    let st = if c.status == "COMPLETED" {
        match c.conclusion.as_str() {
            "SUCCESS" => "passed",
            "NEUTRAL" | "SKIPPED" => "skipped",
            _ => "failed",
        }
    } else {
        "pending"
    };
    Check {
        name: c.name.clone(),
        workflow: c.workflow.clone(),
        state: st.into(),
        url: c.details_url.clone(),
    }
}

#[cfg(test)]
mod tests;
