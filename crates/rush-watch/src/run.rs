//! Waiting on a GitHub Actions run for an agent (a branch's, a commit's,
//! or one run), and saying once, briefly, what happened: rush watch run.
//! It asks through the same shared answers as rush watch pr (gh::get: one
//! asker at a time per question, an ETag so an unchanged answer costs
//! none of the rate limit), backs off as it does, and holds off while
//! GitHub says the limit is spent.
//! New in the Rust port: Go's rush watched pull requests only.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use rush_state::Time;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::gh::{self, Error, nd};
use crate::go::{duration, quote, round, round_to, short};
use crate::limit::{self, Patience};
use crate::pr::{
    Check, EXIT_ERROR, EXIT_FAILED, EXIT_PASSED, EXIT_TIMED_OUT, LogFn, LogsFn, NowFn, SayFn,
    SleepFn, StoppedFn, Unresolved, Until, interval,
};

/// What rush watch run was given to watch, before asking anyone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    /// Nothing: this checkout's branch and commit.
    Current,
    Branch(String),
    Sha(String),
    Run(i64),
}

static RUN_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"github\.com/([^/\t\n\x0c\r ]+/[^/\t\n\x0c\r ]+)/actions/runs/([0-9]+)").unwrap()
});

/// What what names, and the repository when it says one ("" when it
/// doesn't): a run id, a commit's sha (7 to 40 hex digits), a branch,
/// owner/name@<any of those>, a run's URL, or "" for this checkout.
pub fn parse_arg(what: &str) -> Result<(String, Arg), String> {
    let what = what.trim();
    if let Some(m) = RUN_URL.captures(what) {
        let id = m[2]
            .parse()
            .map_err(|_| format!("{} isn't a run", quote(what)))?;
        return Ok((m[1].to_string(), Arg::Run(id)));
    }
    if what.is_empty() {
        return Ok((String::new(), Arg::Current));
    }
    if let Some((repo, r)) = what.split_once('@')
        && repo.matches('/').count() == 1
        && !repo.starts_with('/')
        && !repo.ends_with('/')
    {
        if r.is_empty() {
            return Err(format!(
                "{} names no branch, commit or run after the @",
                quote(what)
            ));
        }
        return Ok((repo.to_string(), ref_arg(r)));
    }
    Ok((String::new(), ref_arg(what)))
}

/// A ref as a run id (digits), a sha (hex with a letter in it) or a branch.
fn ref_arg(r: &str) -> Arg {
    if r.bytes().all(|b| b.is_ascii_digit())
        && let Ok(id) = r.parse::<i64>()
    {
        return Arg::Run(id);
    }
    if (7..=40).contains(&r.len())
        && r.bytes().all(|b| b.is_ascii_hexdigit())
        && r.bytes().any(|b| b.is_ascii_alphabetic())
    {
        return Arg::Sha(r.to_ascii_lowercase());
    }
    Arg::Branch(r.to_string())
}

/// Which runs a watch asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// The newest run of each workflow (or of the one asked for) on a
    /// commit; branch is the branch it heads, "" when not known.
    Sha { sha: String, branch: String },
    /// One run.
    Run(i64),
}

/// A watch's runs, resolved: the repository, which runs, and the workflow
/// they're kept to ("" for every one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// owner/name
    pub repo: String,
    pub what: What,
    /// A workflow's file name (ci.yml) or id; "" for all.
    pub workflow: String,
}

impl Target {
    /// What its lines call it: "main @ abc1234", "abc1234", "run 123".
    pub fn label(&self) -> String {
        match &self.what {
            What::Sha { sha, branch } if !branch.is_empty() => {
                format!("{branch} @ {}", short(sha))
            }
            What::Sha { sha, .. } => short(sha).to_string(),
            What::Run(id) => format!("run {id}"),
        }
    }

    /// The question asked of GitHub for its runs.
    pub fn runs_path(&self) -> String {
        match &self.what {
            What::Run(id) => format!("repos/{}/actions/runs/{id}", self.repo),
            What::Sha { sha, .. } if self.workflow.is_empty() => {
                format!(
                    "repos/{}/actions/runs?head_sha={sha}&per_page=100",
                    self.repo
                )
            }
            What::Sha { sha, .. } => format!(
                "repos/{}/actions/workflows/{}/runs?head_sha={sha}&per_page=100",
                self.repo, self.workflow
            ),
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(b: &[u8]) -> Result<T, Error> {
    rush_state::json::from_slice(b).map_err(|e| Error::Other(e.to_string()))
}

/// Runs git in dir: its output, trimmed.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if !dir.as_os_str().is_empty() {
        cmd.current_dir(dir);
    }
    let out = cmd.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The runs what names (see [`parse_arg`]), in dir's repository unless it
/// names one, kept to workflow when one is given. A branch is asked of
/// GitHub for its head commit, once; "" is this checkout's commit.
pub fn resolve_run(dir: &Path, what: &str, workflow: &str) -> Result<Target, String> {
    resolve_run_with(dir, what, workflow, &Patience::none()).map_err(|e| e.to_string())
}

/// [`resolve_run`], waiting out GitHub's rate limit as patience says.
pub fn resolve_run_with(
    dir: &Path,
    what: &str,
    workflow: &str,
    p: &Patience,
) -> Result<Target, Unresolved> {
    let (mut repo, arg) = parse_arg(what).map_err(Unresolved::Failed)?;
    if repo.is_empty() {
        let out = p
            .call(|| {
                gh::run_checked(
                    Some(dir),
                    &[
                        "repo",
                        "view",
                        "--json",
                        "nameWithOwner",
                        "--jq",
                        ".nameWithOwner",
                    ],
                )
            })
            .map_err(Unresolved::of)?;
        repo = match &out.err {
            Some(e @ rush_gh::ghpr::RunError::Exit { .. }) => {
                return Err(Unresolved::Failed(
                    gh::gh_error(0, &[], &out.stderr, Some(e)).to_string(),
                ));
            }
            Some(e) => return Err(Unresolved::Failed(e.to_string())),
            None => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        };
    }
    // A wait that ran out of time, from the lookups that return no error.
    let held: std::cell::RefCell<Option<Unresolved>> = std::cell::RefCell::new(None);
    let what = match arg {
        Arg::Run(id) => What::Run(id),
        Arg::Sha(sha) => What::Sha {
            sha: full_sha(
                &sha,
                |s| {
                    let rev = format!("{s}^{{commit}}");
                    git(dir, &["rev-parse", "--verify", "-q", &rev])
                },
                |s| match commit_sha(&repo, s, p) {
                    Ok(sha) => sha,
                    Err(e) => {
                        *held.borrow_mut() = Some(e);
                        None
                    }
                },
            )
            .map_err(|e| held.borrow_mut().take().unwrap_or(Unresolved::Failed(e)))?,
            branch: String::new(),
        },
        Arg::Current => {
            let sha = git(dir, &["rev-parse", "HEAD"])
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    Unresolved::Failed("no commit here: give a branch, a sha or a run id".into())
                })?;
            let branch = git(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
            What::Sha { sha, branch }
        }
        Arg::Branch(b) => What::Sha {
            sha: head_of(&repo, &b, p)?,
            branch: b,
        },
    };
    Ok(Target {
        repo,
        what,
        workflow: workflow.to_string(),
    })
}

/// sha as the full 40 characters GitHub's head_sha filter needs (it matches
/// nothing shorter): as given when already full, else what local (git
/// rev-parse in the watch's dir) says, else what remote (GitHub's commits
/// endpoint) says. Neither knowing it is an error at once, not a wait.
pub fn full_sha(
    sha: &str,
    local: impl Fn(&str) -> Option<String>,
    remote: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let is_full = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if is_full(sha) {
        return Ok(sha.to_string());
    }
    if let Some(s) = local(sha).map(|s| s.trim().to_ascii_lowercase())
        && is_full(&s)
    {
        return Ok(s);
    }
    remote(sha)
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| is_full(s))
        .ok_or_else(|| format!("no commit {} here or on GitHub: was it pushed?", quote(sha)))
}

/// The full sha GitHub gives a commit (any unambiguous prefix of one).
fn commit_sha(repo: &str, rev: &str, p: &Patience) -> Result<Option<String>, Unresolved> {
    #[derive(Deserialize, Default)]
    struct Commit {
        #[serde(default, deserialize_with = "nd")]
        sha: String,
    }
    let got = p.call(|| {
        gh::get(
            &format!("repos/{repo}/commits/{rev}"),
            Duration::from_secs(10),
        )
    });
    match got {
        Ok((b, _)) => Ok(decode::<Commit>(&b).ok().map(|c| c.sha)),
        Err(limit::Waited::Err(Error::Limited)) => {
            Err(Unresolved::Failed(Error::Limited.to_string()))
        }
        Err(limit::Waited::Err(_)) => Ok(None),
        Err(w) => Err(Unresolved::of(w)),
    }
}

/// The commit branch (or a tag) heads on GitHub.
fn head_of(repo: &str, branch: &str, p: &Patience) -> Result<String, Unresolved> {
    #[derive(Deserialize, Default)]
    struct Commit {
        #[serde(default, deserialize_with = "nd")]
        sha: String,
    }
    #[derive(Deserialize)]
    struct Branch {
        #[serde(default, deserialize_with = "nd")]
        commit: Commit,
    }
    let fresh = Duration::from_secs(10);
    let by_branch = p.call(|| {
        gh::get(&format!("repos/{repo}/branches/{branch}"), fresh)
            .and_then(|(b, _)| decode::<Branch>(&b))
            .map(|b| b.commit.sha)
    });
    match by_branch {
        Ok(sha) if !sha.is_empty() => return Ok(sha),
        Err(limit::Waited::Err(Error::Limited)) => {
            return Err(Unresolved::Failed(Error::Limited.to_string()));
        }
        Err(w @ (limit::Waited::TimedOut(_) | limit::Waited::Stopped)) => {
            return Err(Unresolved::of(w));
        }
        _ => {}
    }
    let by_commit = p.call(|| {
        gh::get(&format!("repos/{repo}/commits/{branch}"), fresh)
            .and_then(|(b, _)| decode::<Commit>(&b))
    });
    match by_commit {
        Ok(c) if !c.sha.is_empty() => Ok(c.sha),
        Ok(_) => Err(Unresolved::Failed(format!(
            "no branch or tag {} in {repo}",
            quote(branch)
        ))),
        Err(limit::Waited::Err(e)) => Err(Unresolved::Failed(format!(
            "no branch or tag {} in {repo}: {e}",
            quote(branch)
        ))),
        Err(w) => Err(Unresolved::of(w)),
    }
}

/// One workflow run and its jobs, as GitHub last said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowRun {
    pub id: i64,
    /// The workflow's name: CI.
    pub name: String,
    pub url: String,
    /// queued, in_progress, completed…
    pub status: String,
    /// success, failure, cancelled…; "" while it runs.
    pub conclusion: String,
    pub head: String,
    pub branch: String,
    /// Its workflow's id, and when it started (zero before it has): what
    /// how long it should take is worked out from (eta.rs).
    pub workflow: i64,
    pub started: Time,
    /// Its jobs, as checks: passed, failed, cancelled, skipped or pending.
    pub jobs: Vec<Check>,
}

/// The runs a watch waits on, as GitHub last said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Runs {
    pub runs: Vec<WorkflowRun>,
}

impl Runs {
    /// Every run's jobs, with the run they're in.
    pub fn jobs(&self) -> impl Iterator<Item = (&WorkflowRun, &Check)> {
        self.runs
            .iter()
            .flat_map(|r| r.jobs.iter().map(move |j| (r, j)))
    }

    /// Its jobs by how they went: passed, failed, cancelled, pending,
    /// skipped.
    pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
        let (mut passed, mut failed, mut cancelled, mut pending, mut skipped) = (0, 0, 0, 0, 0);
        for (_, j) in self.jobs() {
            match j.state.as_str() {
                "passed" => passed += 1,
                "failed" => failed += 1,
                "cancelled" => cancelled += 1,
                "skipped" => skipped += 1,
                _ => pending += 1,
            }
        }
        (passed, failed, cancelled, pending, skipped)
    }

    /// Every run has finished, and every job in it.
    pub fn done(&self) -> bool {
        !self.runs.is_empty()
            && self.runs.iter().all(|r| r.status == "completed")
            && self.counts().3 == 0
    }

    /// Its workflows' names, once each.
    fn names(&self) -> String {
        let mut seen = HashSet::new();
        self.runs
            .iter()
            .filter(|r| seen.insert(r.name.as_str()))
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A job's (or a run's) status and conclusion as a check's state.
pub fn job_state(status: &str, conclusion: &str) -> &'static str {
    if status != "completed" {
        return "pending";
    }
    match conclusion {
        "success" => "passed",
        "neutral" | "skipped" => "skipped",
        "cancelled" => "cancelled",
        _ => "failed", // failure, timed_out, startup_failure, action_required, stale
    }
}

#[derive(Deserialize)]
struct RunJson {
    #[serde(default, deserialize_with = "nd")]
    id: i64,
    #[serde(default, deserialize_with = "nd")]
    name: String,
    #[serde(default, deserialize_with = "nd")]
    html_url: String,
    #[serde(default, deserialize_with = "nd")]
    status: String,
    #[serde(default, deserialize_with = "nd")]
    conclusion: String,
    #[serde(default, deserialize_with = "nd")]
    head_sha: String,
    #[serde(default, deserialize_with = "nd")]
    head_branch: String,
    #[serde(default, deserialize_with = "nd")]
    workflow_id: i64,
    #[serde(default, deserialize_with = "crate::gh::time_or_zero")]
    run_started_at: Time,
}

/// How old the jobs of a run still going may be: any watcher's answer
/// within it is used as it is. Rust-only.
pub const JOBS_FRESH: Duration = Duration::from_secs(45);

/// The runs t names as GitHub says now, or as another watcher heard within
/// fresh: the newest run of each workflow on its commit, or the one run,
/// each with its jobs. With it, the rate limit left, -1 when unknown.
pub fn fetch(t: &Target, fresh: Duration) -> (Result<Runs, Error>, i64) {
    let remaining = Cell::new(-1i64);
    let get = |path: String, fresh: Duration| -> Result<Vec<u8>, Error> {
        let (b, r) = gh::get(&path, fresh)?;
        if r >= 0 {
            remaining.set(r);
        }
        Ok(b)
    };
    #[derive(Deserialize)]
    struct List {
        #[serde(default, deserialize_with = "nd")]
        workflow_runs: Vec<RunJson>,
    }
    let found: Vec<RunJson> = match &t.what {
        What::Run(_) => match get(t.runs_path(), fresh).and_then(|b| decode::<RunJson>(&b)) {
            Ok(r) => vec![r],
            Err(e) => return (Err(e), remaining.get()),
        },
        What::Sha { .. } => match get(t.runs_path(), fresh).and_then(|b| decode::<List>(&b)) {
            Ok(l) => {
                // Newest first, as GitHub lists them: the first of each
                // workflow is its newest.
                let mut seen = HashSet::new();
                l.workflow_runs
                    .into_iter()
                    .filter(|r| seen.insert(r.workflow_id))
                    .collect()
            }
            Err(e) => return (Err(e), remaining.get()),
        },
    };
    #[derive(Deserialize)]
    struct Job {
        #[serde(default, deserialize_with = "nd")]
        id: i64,
        #[serde(default, deserialize_with = "nd")]
        name: String,
        #[serde(default, deserialize_with = "nd")]
        status: String,
        #[serde(default, deserialize_with = "nd")]
        conclusion: String,
        #[serde(default, deserialize_with = "nd")]
        html_url: String,
        #[serde(default, deserialize_with = "crate::gh::time_or_zero")]
        started_at: Time,
        #[serde(default, deserialize_with = "crate::gh::time_or_zero")]
        completed_at: Time,
    }
    #[derive(Deserialize)]
    struct Jobs {
        #[serde(default, deserialize_with = "nd")]
        jobs: Vec<Job>,
    }
    let many = found.len() > 1;
    let mut out = Runs::default();
    for r in found {
        // A run still going has its jobs asked for less often than the
        // run is: the run says when it ends, and its jobs are half of
        // what a watch spends. One that ended is asked as fresh as the
        // run was, so its last word is never an old one.
        let jobs_fresh = if r.status == "completed" {
            fresh
        } else {
            fresh.max(JOBS_FRESH)
        };
        let jobs: Jobs = match get(
            format!("repos/{}/actions/runs/{}/jobs?per_page=100", t.repo, r.id),
            jobs_fresh,
        )
        .and_then(|b| decode(&b))
        {
            Ok(j) => j,
            Err(e) => return (Err(e), remaining.get()),
        };
        let mut w = WorkflowRun {
            id: r.id,
            name: r.name,
            url: r.html_url,
            status: r.status,
            conclusion: r.conclusion,
            head: r.head_sha,
            branch: r.head_branch,
            workflow: r.workflow_id,
            started: r.run_started_at,
            jobs: Vec::new(),
        };
        for j in jobs.jobs {
            w.jobs.push(Check {
                id: j.id,
                name: if many {
                    format!("{} / {}", w.name, j.name)
                } else {
                    j.name
                },
                state: job_state(&j.status, &j.conclusion).into(),
                url: j.html_url,
                actions: true,
                note: String::new(),
                started: j.started_at,
                ended: j.completed_at,
            });
        }
        if w.jobs.is_empty() && w.status == "completed" {
            // It ended before any job ran (a broken workflow file): the
            // run stands for its jobs.
            w.jobs.push(Check {
                id: 0,
                name: w.name.clone(),
                state: job_state(&w.status, &w.conclusion).into(),
                url: w.url.clone(),
                actions: false,
                note: format!("the run ended {} before any job ran", w.conclusion),
                ..Default::default()
            });
        }
        out.runs.push(w);
    }
    (Ok(out), remaining.get())
}

/// Asks GitHub for the runs: how fresh an answer another watcher had may be.
pub type FetchRunsFn<'a> = Box<dyn Fn(Duration) -> (Result<Runs, Error>, i64) + 'a>;
/// When GitHub's spent rate limit resets, if it is spent.
pub type LimitFn<'a> = Box<dyn Fn() -> Option<Time> + 'a>;
/// Where a branch is on GitHub now (repo, branch, the commit watched): the
/// commit it heads, and whether that has the one watched in its history
/// (None when GitHub couldn't say); None when it can't say at all.
pub type HeadFn<'a> = Box<dyn Fn(&str, &str, &str) -> Option<(String, Option<bool>)> + 'a>;

/// When runs started this way should be done (eta::expected_end), or None.
pub type ExpectFn<'a> = Box<dyn Fn(&[crate::eta::Started]) -> Option<Time> + 'a>;

/// What a run watch waits for. Say is told each line to print; fetch,
/// sleep, logs, now and limited are GitHub and the clock, which tests set;
/// log records what it did.
#[derive(Default)]
pub struct RunOptions<'a> {
    pub target: Option<Target>,
    pub until: Until,
    /// Zero: never.
    pub timeout: Duration,
    /// The background task the watch runs as, if it is one: it keeps a
    /// Live for it while it runs, for rush to draw.
    pub task: String,
    /// When its runs should be done; unset, nothing is expected.
    pub expect: Option<ExpectFn<'a>>,
    pub say: Option<SayFn<'a>>,
    pub fetch: Option<FetchRunsFn<'a>>,
    pub sleep: Option<SleepFn<'a>>,
    pub logs: Option<LogsFn<'a>>,
    pub now: Option<NowFn<'a>>,
    pub log: Option<LogFn<'a>>,
    pub stopped: Option<StoppedFn<'a>>,
    pub limited: Option<LimitFn<'a>>,
    pub head: Option<HeadFn<'a>>,
}

impl RunOptions<'_> {
    fn target(&self) -> Target {
        self.target.clone().unwrap_or_else(|| Target {
            repo: String::new(),
            what: What::Run(0),
            workflow: String::new(),
        })
    }

    fn repo(&self) -> String {
        self.target().repo
    }

    fn label(&self) -> String {
        if self.target.is_none() {
            return String::new();
        }
        self.target().label()
    }

    fn head(&self, repo: &str, branch: &str, sha: &str) -> Option<(String, Option<bool>)> {
        match &self.head {
            Some(f) => f(repo, branch, sha),
            None => {
                let new = head_of(repo, branch, &Patience::none()).ok()?;
                let has = (new != sha).then(|| has_commit(repo, sha, &new)).flatten();
                Some((new, has))
            }
        }
    }

    fn say(&self, s: &str) {
        if let Some(f) = &self.say {
            f(s)
        }
    }

    fn expect(&self, rs: &Runs) -> Time {
        let Some(f) = &self.expect else {
            return Time::default();
        };
        let started: Vec<crate::eta::Started> = rs
            .runs
            .iter()
            .map(|r| crate::eta::Started {
                workflow: r.workflow,
                at: r.started,
                done: r.status == "completed",
            })
            .collect();
        f(&started).unwrap_or_default()
    }

    fn fetch(&self, fresh: Duration) -> (Result<Runs, Error>, i64) {
        match &self.fetch {
            Some(f) => f(fresh),
            None => fetch(&self.target(), fresh),
        }
    }

    fn sleep(&self, d: Duration) -> bool {
        match &self.sleep {
            Some(f) => f(d),
            None => crate::stop::sleep(d),
        }
    }

    fn logs(&self, job: i64) -> Result<Vec<u8>, Error> {
        let repo = self.repo();
        match &self.logs {
            Some(f) => f(&repo, job),
            None => gh::raw(&format!("repos/{repo}/actions/jobs/{job}/logs")),
        }
    }

    fn now(&self) -> Time {
        match &self.now {
            Some(f) => f(),
            None => Time::now(),
        }
    }

    fn log(&self, kind: &str, fields: Map<String, Value>) {
        match &self.log {
            Some(f) => f(kind, fields),
            None => rush_events::log(kind, fields),
        }
    }

    fn stopped(&self) -> bool {
        match &self.stopped {
            Some(f) => f(),
            None => crate::stop::stopped(),
        }
    }

    fn limited(&self) -> Option<Time> {
        match &self.limited {
            Some(f) => f(),
            None => gh::limited_until(),
        }
    }
}

/// How long a watch waits for a run to show up on a freshly pushed commit
/// before deciding none will.
pub const NO_RUNS_AFTER: Duration = Duration::from_secs(2 * 60);

fn since(now: Time, start: Time) -> Duration {
    Duration::from_nanos(now.sub_nanos(start).clamp(0, u64::MAX as i128) as u64)
}

fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// What a run watch keeps for its last line in the log.
struct State {
    remaining: i64,
    last: Option<Runs>,
    why: String,
    /// Each failed job's first cause, read once, for its Live.
    notes: HashMap<i64, String>,
}

/// Waits on o's runs as o.until has it, and says once, briefly, what
/// happened; it gives the exit code to end with: 0 all passed, 1 a job
/// failed, 2 timed out, 3 cancelled, gone, or GitHub couldn't be asked.
pub fn watch_run(o: &RunOptions) -> i32 {
    let start = o.now();
    let (repo, label) = (o.repo(), o.label());
    let workflow = o
        .target
        .as_ref()
        .map(|t| t.workflow.clone())
        .unwrap_or_default();
    o.log(
        "watch-run",
        fields(json!({"repo": repo, "what": label, "workflow": workflow,
            "until": o.until.as_str(), "timeout": duration(o.timeout)})),
    );
    let mut st = State {
        remaining: -1,
        last: None,
        why: String::new(),
        notes: HashMap::new(),
    };
    let code = watching(o, start, &mut st);
    if !o.task.is_empty() {
        // Kept a while, for rush to draw how it went.
        let now = o.now();
        let looked = Looked {
            runs: st.last.clone(),
            asked: now,
            notes: st.notes.clone(),
            ..Default::default()
        };
        let mut l = live(o, start, &looked);
        l.ended = now;
        l.outcome = crate::live::outcome_of(code, &st.why);
        crate::live::save_ended(&l);
    }
    let mut f = fields(json!({"repo": repo, "what": label, "code": code,
        "after": duration(round_to(since(o.now(), start), Duration::from_secs(1))),
        "remaining": st.remaining}));
    if !st.why.is_empty() {
        f.insert("why".into(), st.why.clone().into());
    }
    if let Some(last) = &st.last {
        let (passed, failed, cancelled, pending, skipped) = last.counts();
        let ids: Vec<i64> = last.runs.iter().map(|r| r.id).collect();
        f.insert("runs".into(), ids.into());
        f.insert("passed".into(), passed.into());
        f.insert("failed".into(), failed.into());
        f.insert("cancelled".into(), cancelled.into());
        f.insert("pending".into(), pending.into());
        f.insert("skipped".into(), skipped.into());
        let names: Vec<&str> = last
            .jobs()
            .filter(|(_, j)| j.state == "failed")
            .map(|(_, j)| j.name.as_str())
            .collect();
        if !names.is_empty() {
            f.insert("failedJobs".into(), names.into());
        }
    }
    o.log("watch-run-end", f);
    crate::live::drop_live(&o.task);
    code
}

/// The Live a run watch keeps while it runs: its runs as last seen (None
/// before GitHub has said), when it last asked and will ask next, and
/// when the runs should be done.
pub fn live(o: &RunOptions, start: Time, at: &Looked) -> crate::live::Live {
    let mut l = crate::live::Live {
        task: o.task.clone(),
        pid: std::process::id() as i64,
        repo: o.repo(),
        what: o.label(),
        until: o.until,
        timeout: o.timeout,
        start,
        asked: at.asked,
        next: at.next,
        limited: at.limited,
        expect: at.expect,
        ..Default::default()
    };
    let Some(rs) = &at.runs else {
        return l;
    };
    let names = rs.names();
    if !names.is_empty() {
        l.title = names;
    }
    let (passed, failed, cancelled, pending, skipped) = rs.counts();
    (l.passed, l.failed, l.pending, l.skipped) = (passed, failed + cancelled, pending, skipped);
    for (_, j) in rs.jobs() {
        match j.state.as_str() {
            "failed" | "cancelled" => l.failed_checks.push(j.name.clone()),
            "pending" => l.running.push(j.name.clone()),
            _ => {}
        }
        l.jobs
            .push(crate::live::LiveJob::of(j, at.notes.get(&j.id)));
    }
    l
}

/// Reads why each job of rs that failed did, once a job, from its log's
/// first cause: for the task row (Rust-only).
fn note_failures(o: &RunOptions, rs: &Runs, notes: &mut HashMap<i64, String>) {
    for (_, j) in rs.jobs() {
        if j.state != "failed" || !j.actions || j.id == 0 || notes.contains_key(&j.id) {
            continue;
        }
        let why = o
            .logs(j.id)
            .map(|b| crate::live::first_cause(&String::from_utf8_lossy(&b)))
            .unwrap_or_default();
        notes.insert(j.id, why);
    }
}

/// Where a run watch stands between looks, for its Live.
#[derive(Debug, Clone, Default)]
pub struct Looked {
    pub runs: Option<Runs>,
    pub asked: Time,
    pub next: Time,
    pub limited: bool,
    pub expect: Time,
    /// Each failed job's first line of its log that says why, by its id
    /// (Rust-only: the task row's).
    pub notes: HashMap<i64, String>,
}

fn watching(o: &RunOptions, start: Time, st: &mut State) -> i32 {
    let mut prev: Option<Runs> = None;
    let mut looked = Looked::default();
    // The runs the expected end was worked out for: again when they change.
    let mut expected_for: Vec<i64> = Vec::new();
    let mut fails = 0;
    let mut limited = false;
    // Looks in a row GitHub said it's limited, and the resume time last said.
    let mut strikes = 0;
    let mut said_for: Option<Time> = None;
    let mut said_waiting = false;
    loop {
        let (res, r) = o.fetch(Duration::from_secs(10));
        if r >= 0 {
            st.remaining = r;
        }
        if o.stopped() {
            st.why = "stopped".into();
            return EXIT_ERROR;
        }
        // How long to wait for a spent limit, when it is.
        let mut spent: Option<Duration> = None;
        match res {
            Err(Error::Limited) => {
                st.remaining = 0;
                let until = o.limited();
                if !limited {
                    limited = true;
                    o.log(
                        "watch-run-limited",
                        fields(json!({"repo": o.repo(), "what": o.label(),
                            "why": Error::Limited.to_string()})),
                    );
                }
                // Waits it out, and says so once: a spent limit isn't a
                // failure, unless it resets after the timeout.
                let now = o.now();
                let wait = limit::wait_for(until, now, strikes);
                strikes += 1;
                let resume = now + wait;
                if !o.timeout.is_zero() && since(resume, start) > o.timeout {
                    o.say(&limit::timed_out_line(resume.local(), now, o.timeout));
                    st.why = "timed out".into();
                    return EXIT_TIMED_OUT;
                }
                if said_for.is_none_or(|s| since(resume, s) > Duration::from_secs(30)) {
                    said_for = Some(resume);
                    o.say(&limit::waiting_line(resume.local(), now));
                }
                spent = Some(wait);
            }
            Err(err) => {
                // A blip is asked again; three in a row end it.
                fails += 1;
                if fails >= 3 || prev.is_none() {
                    o.say(&format!("rush watch: {err}"));
                    st.why = format!("GitHub couldn't be asked: {err}");
                    return EXIT_ERROR;
                }
            }
            Ok(rs) => {
                fails = 0;
                limited = false;
                strikes = 0;
                said_for = None;
                st.last = Some(rs.clone());
                looked.asked = o.now();
                if !o.task.is_empty() {
                    note_failures(o, &rs, &mut st.notes);
                }
                let ids: Vec<i64> = rs.runs.iter().map(|r| r.id).collect();
                if !ids.is_empty() && ids != expected_for && !rs.done() {
                    looked.expect = o.expect(&rs);
                    expected_for = ids;
                }
                if let Some(code) = step(
                    o,
                    prev.as_ref(),
                    &rs,
                    since(o.now(), start),
                    &mut said_waiting,
                ) {
                    return code;
                }
                prev = Some(rs);
            }
        }
        let gone = since(o.now(), start);
        // A spent limit is waited for until it resets, and not long after.
        let wait = spent.unwrap_or_else(|| interval(gone, st.remaining));
        if !o.task.is_empty() {
            looked.runs = prev.clone();
            looked.limited = limited;
            looked.notes.clone_from(&st.notes);
            looked.next = o.now().add_nanos(wait.as_nanos() as i64);
            crate::live::save_live(&live(o, start, &looked));
        }
        if !o.timeout.is_zero() && gone + wait > o.timeout {
            if let Some(left) = o.timeout.checked_sub(gone).filter(|l| !l.is_zero())
                && o.sleep(left)
            {
                // one last look before giving up
                if let (Ok(rs), _) = o.fetch(Duration::ZERO) {
                    st.last = Some(rs.clone());
                    if let Some(code) = step(
                        o,
                        prev.as_ref(),
                        &rs,
                        since(o.now(), start),
                        &mut said_waiting,
                    ) {
                        return code;
                    }
                    prev = Some(rs);
                }
            }
            o.say(&waiting(o, prev.as_ref(), since(o.now(), start)));
            st.why = "timed out".into();
            return EXIT_TIMED_OUT;
        }
        if !o.sleep(wait) {
            st.why = "stopped".into();
            return EXIT_ERROR;
        }
    }
}

/// A branch's runs cancelled, none failed, and the branch on GitHub at
/// another commit: most likely a push replaced them (a workflow's
/// concurrency cancels the old run). Says where the branch is and whether
/// that commit has the one watched, so whoever watches can decide whether
/// to wait on it; the watch still ends as cancelled.
fn replaced(o: &RunOptions, rs: &Runs) -> Option<String> {
    let (_, failed, cancelled, _, _) = rs.counts();
    if failed > 0 || cancelled == 0 {
        return None;
    }
    let t = o.target();
    let What::Sha { sha, branch } = &t.what else {
        return None;
    };
    if branch.is_empty() {
        return None;
    }
    let (new, has) = o
        .head(&t.repo, branch, sha)
        .filter(|(n, _)| !n.is_empty() && n != sha)?;
    let what = match has {
        Some(true) => format!("a newer push, on top of {}", short(sha)),
        Some(false) => format!(
            "a push without {} (rebased or force-pushed?): what you pushed may not be in it",
            short(sha)
        ),
        None => "another push".to_string(),
    };
    o.log(
        "watch-run-replaced",
        fields(json!({"repo": t.repo, "from": sha, "to": new, "branch": branch, "has": has})),
    );
    Some(format!(
        "replaced: {branch} is at {} now, {what}; to wait on it: rush watch run {branch}",
        short(&new)
    ))
}

/// Whether commit new has commit old in its history, as GitHub compares
/// them; None when it couldn't say.
fn has_commit(repo: &str, old: &str, new: &str) -> Option<bool> {
    #[derive(Deserialize)]
    struct Compare {
        #[serde(default, deserialize_with = "nd")]
        status: String,
    }
    let (b, _) = gh::get(
        &format!("repos/{repo}/compare/{old}...{new}?per_page=1"),
        Duration::from_secs(60),
    )
    .ok()?;
    match decode::<Compare>(&b).ok()?.status.as_str() {
        "ahead" | "identical" => Some(true),
        "behind" | "diverged" => Some(false),
        _ => None,
    }
}

/// A job's key across looks.
fn key(r: &WorkflowRun, j: &Check) -> (i64, i64, String) {
    (r.id, j.id, j.name.clone())
}

/// Looks at rs, after prev (None the first time), and says whether the
/// watch is over, and with what code.
fn step(
    o: &RunOptions,
    prev: Option<&Runs>,
    rs: &Runs,
    since: Duration,
    said_waiting: &mut bool,
) -> Option<i32> {
    let label = o.label();
    if rs.runs.is_empty() {
        if since >= NO_RUNS_AFTER {
            o.say(&format!(
                "{label}: no workflow run after {}: was it pushed, and does a workflow run on it?",
                round(since)
            ));
            return Some(EXIT_ERROR);
        }
        if !*said_waiting {
            *said_waiting = true;
            o.say(&format!(
                "{label}: no run yet; waiting up to {} for one to start",
                round(NO_RUNS_AFTER)
            ));
        }
        return None;
    }
    let mut finished: Vec<&Check> = Vec::new(); // since the last look
    if let Some(prev) = prev {
        let before: HashSet<(i64, i64, String)> = prev
            .jobs()
            .filter(|(_, j)| j.state != "pending")
            .map(|(r, j)| key(r, j))
            .collect();
        for (r, j) in rs.jobs() {
            if j.state != "pending" && !before.contains(&key(r, j)) {
                finished.push(j);
            }
        }
    }
    if o.until == Until::Each {
        for j in &finished {
            o.say(&job_line(j));
        }
    }
    let (_, failed, cancelled, _, _) = rs.counts();
    let done = rs.done();
    if o.until == Until::Fail && failed > 0 || done {
        let mut said = summary(o, rs, since);
        if done && let Some(r) = replaced(o, rs) {
            said += "\n";
            said += &r;
        }
        o.say(&said);
        return Some(if failed > 0 {
            EXIT_FAILED
        } else if cancelled > 0 {
            EXIT_ERROR
        } else {
            EXIT_PASSED
        });
    }
    if o.until == Until::First && !finished.is_empty() {
        let mut lines = vec![format!("{label}: {}", progress(rs))];
        let mut code = EXIT_PASSED;
        for j in finished {
            lines.push(job_line(j));
            if j.state == "failed" {
                code = EXIT_FAILED;
                lines.extend(error_lines(o, j));
            }
        }
        o.say(&lines.join("\n"));
        return Some(code);
    }
    None
}

/// What a finished watch says: the counts, each failed job with the lines
/// of its log that say why, and the failed runs' URLs. Passes aren't
/// named.
pub fn summary(o: &RunOptions, rs: &Runs, since: Duration) -> String {
    let (passed, failed, cancelled, pending, skipped) = rs.counts();
    let mut head = format!(
        "{} {}",
        o.label(),
        if rs.runs.len() == 1 { "run" } else { "runs" }
    );
    if pending == 0 {
        head += " finished";
    }
    let mut parts = vec![format!("{passed} passed")];
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if cancelled > 0 {
        parts.push(format!("{cancelled} cancelled"));
    }
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
    }
    if pending > 0 {
        parts.push(format!("{pending} still running"));
    }
    let mut lines = vec![format!(
        "{head} after {}: {} · {}",
        round(since),
        parts.join(", "),
        rs.names()
    )];
    let mut shown = 0;
    for (_, j) in rs.jobs() {
        if j.state != "failed" {
            continue;
        }
        shown += 1;
        if shown > 5 {
            lines.push(format!("… and {} more failed", failed - 5));
            break;
        }
        lines.push(job_line(j));
        lines.extend(error_lines(o, j));
    }
    if failed == 0 {
        for (_, j) in rs.jobs().filter(|(_, j)| j.state == "cancelled").take(5) {
            lines.push(job_line(j));
        }
    }
    for r in &rs.runs {
        if r.jobs
            .iter()
            .any(|j| j.state == "failed" || j.state == "cancelled")
            && !r.url.is_empty()
        {
            lines.push(format!("run: {}", r.url));
        }
    }
    lines.join("\n")
}

fn job_line(j: &Check) -> String {
    let mark = match j.state.as_str() {
        "passed" => "✓",
        "failed" => "✗",
        "cancelled" => "⊘",
        "skipped" => "-",
        _ => "",
    };
    let mut line = format!("{mark} {} {}", j.name, j.state);
    if j.state == "failed" && !j.url.is_empty() {
        line += " · ";
        line += &j.url;
    }
    line
}

/// The first lines that say why j failed, indented: its log's errors, or
/// what it says of itself.
fn error_lines(o: &RunOptions, j: &Check) -> Vec<String> {
    let mut lines = Vec::new();
    if j.actions
        && j.id != 0
        && let Ok(b) = o.logs(j.id)
    {
        lines = crate::causes::log_lines(&String::from_utf8_lossy(&b), 8);
    }
    if lines.is_empty() && !j.note.is_empty() {
        lines = vec![j.note.clone()];
    }
    lines.into_iter().map(|l| format!("    {l}")).collect()
}

/// How far its jobs have got: "5/9 jobs finished, 1 failed".
fn progress(rs: &Runs) -> String {
    let all = rs.jobs().count();
    if all == 0 {
        return "no jobs yet".into();
    }
    let (_, failed, _, pending, _) = rs.counts();
    let mut s = format!("{}/{all} jobs finished", all - pending);
    if failed > 0 {
        s += &format!(", {failed} failed");
    }
    s
}

/// What a watch that timed out says: how far its runs got.
fn waiting(o: &RunOptions, rs: Option<&Runs>, since: Duration) -> String {
    let Some(rs) = rs.filter(|rs| !rs.runs.is_empty()) else {
        return format!(
            "{}: still waiting after {}: no run has started yet; run it again",
            o.label(),
            round(since)
        );
    };
    let mut running: Vec<&str> = rs
        .jobs()
        .filter(|(_, j)| j.state == "pending")
        .map(|(_, j)| j.name.as_str())
        .collect();
    if running.len() > 8 {
        running.truncate(8);
        running.push("…");
    }
    let mut line = format!(
        "{} still waiting after {}: {}; run it again",
        o.label(),
        round(since),
        progress(rs)
    );
    if !running.is_empty() {
        line += "\n    running: ";
        line += &running.join(", ");
    }
    line
}
