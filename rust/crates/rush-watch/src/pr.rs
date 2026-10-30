//! Waiting on a pull request's checks for an agent, and saying once,
//! briefly, what happened.
//! Ported from internal/watch/pr.go.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use rush_state::Time;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::gh::{self, Error};
use crate::go::{clip, duration, quote, round, round_to, short};
use crate::live;

/// One check on a pull request's head commit: a check run, or a commit
/// status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Check {
    /// A check run's; 0 for a commit status.
    pub id: i64,
    pub name: String,
    /// passed, failed, pending or skipped.
    pub state: String,
    pub url: String,
    /// Run by GitHub Actions, so its job's log can be read.
    pub actions: bool,
    /// What it says of itself: its output's title, a status's description.
    pub note: String,
}

impl Check {
    fn finished(&self) -> bool {
        self.state != "pending"
    }
}

/// A pull request and the checks on its head commit, as GitHub last said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PR {
    /// owner/name
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    /// open, merged or closed
    pub state: String,
    /// The head commit's sha.
    pub head: String,
    pub checks: Vec<Check>,
}

impl PR {
    /// Its checks by how they went: passed, failed, pending, skipped.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let (mut passed, mut failed, mut pending, mut skipped) = (0, 0, 0, 0);
        for c in &self.checks {
            match c.state.as_str() {
                "passed" => passed += 1,
                "failed" => failed += 1,
                "skipped" => skipped += 1,
                _ => pending += 1,
            }
        }
        (passed, failed, pending, skipped)
    }
}

/// Reads a JSON null as the type's zero, as Go does.
use crate::gh::nd;

/// Pull request n of repo as GitHub says now, or as another watcher heard
/// within fresh. With it, the rate limit left, -1 when unknown, even when
/// it failed.
pub fn fetch(repo: &str, n: i64, fresh: Duration) -> (Result<PR, Error>, i64) {
    let remaining = Cell::new(-1i64);
    let get = |path: String| -> Result<Vec<u8>, Error> {
        let (b, r) = gh::get(&path, fresh)?;
        if r >= 0 {
            remaining.set(r);
        }
        Ok(b)
    };
    fn decode<T: serde::de::DeserializeOwned>(b: &[u8]) -> Result<T, Error> {
        rush_state::json::from_slice(b).map_err(|e| Error::Other(e.to_string()))
    }
    #[derive(Deserialize, Default)]
    struct Head {
        #[serde(default, deserialize_with = "nd")]
        sha: String,
    }
    #[derive(Deserialize)]
    struct Pull {
        #[serde(default, deserialize_with = "nd")]
        number: i64,
        #[serde(default, deserialize_with = "nd")]
        title: String,
        #[serde(default, deserialize_with = "nd")]
        html_url: String,
        #[serde(default, deserialize_with = "nd")]
        state: String,
        #[serde(default, deserialize_with = "nd")]
        merged: bool,
        #[serde(default, deserialize_with = "nd")]
        head: Head,
    }
    let base = format!("repos/{repo}");
    let pull: Pull = match get(format!("{base}/pulls/{n}")).and_then(|b| decode(&b)) {
        Ok(p) => p,
        Err(e) => return (Err(e), remaining.get()),
    };
    let mut p = PR {
        repo: repo.to_string(),
        number: pull.number,
        title: pull.title,
        url: pull.html_url,
        state: pull.state,
        head: pull.head.sha,
        checks: Vec::new(),
    };
    if pull.merged {
        p.state = "merged".into();
    }
    #[derive(Deserialize, Default)]
    struct App {
        #[serde(default, deserialize_with = "nd")]
        slug: String,
    }
    #[derive(Deserialize, Default)]
    struct Output {
        #[serde(default, deserialize_with = "nd")]
        title: String,
    }
    #[derive(Deserialize)]
    struct Run {
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
        #[serde(default, deserialize_with = "nd")]
        app: App,
        #[serde(default, deserialize_with = "nd")]
        output: Output,
    }
    #[derive(Deserialize)]
    struct Runs {
        #[serde(default, deserialize_with = "nd")]
        check_runs: Vec<Run>,
    }
    // Go keeps what it read before a failure: p as far as it got.
    let runs: Runs = match get(format!("{base}/commits/{}/check-runs?per_page=100", p.head))
        .and_then(|b| decode(&b))
    {
        Ok(r) => r,
        Err(e) => return (Err(e), remaining.get()),
    };
    for r in runs.check_runs {
        p.checks.push(Check {
            id: r.id,
            name: r.name,
            state: run_state(&r.status, &r.conclusion).into(),
            url: r.html_url,
            actions: r.app.slug == "github-actions",
            note: r.output.title,
        });
    }
    #[derive(Deserialize)]
    struct Status {
        #[serde(default, deserialize_with = "nd")]
        context: String,
        #[serde(default, deserialize_with = "nd")]
        state: String,
        #[serde(default, deserialize_with = "nd")]
        target_url: String,
        #[serde(default, deserialize_with = "nd")]
        description: String,
    }
    #[derive(Deserialize)]
    struct Statuses {
        #[serde(default, deserialize_with = "nd")]
        statuses: Vec<Status>,
    }
    let status: Statuses =
        match get(format!("{base}/commits/{}/status", p.head)).and_then(|b| decode(&b)) {
            Ok(s) => s,
            Err(e) => return (Err(e), remaining.get()),
        };
    for s in status.statuses {
        let st = match s.state.as_str() {
            "success" => "passed",
            "failure" | "error" => "failed",
            _ => "pending",
        };
        p.checks.push(Check {
            id: 0,
            name: s.context,
            state: st.into(),
            url: s.target_url,
            actions: false,
            note: s.description,
        });
    }
    (Ok(p), remaining.get())
}

/// A check run's status and conclusion as a Check's state.
fn run_state(status: &str, conclusion: &str) -> &'static str {
    if status != "completed" {
        return "pending";
    }
    match conclusion {
        "success" => "passed",
        "neutral" | "skipped" => "skipped",
        _ => "failed", // failure, cancelled, timed_out, action_required, stale
    }
}

/// When a watch ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Until {
    /// Every check has finished, or the pull request merged or closed.
    #[default]
    Done,
    /// A check failed, else as done.
    Fail,
    /// The first check to finish, or anything else that changes.
    First,
    /// As done, saying each check as it finishes.
    Each,
}

impl Until {
    pub fn as_str(self) -> &'static str {
        match self {
            Until::Done => "done",
            Until::Fail => "fail",
            Until::First => "first",
            Until::Each => "each",
        }
    }

    pub fn parse(s: &str) -> Option<Until> {
        Some(match s {
            "done" => Until::Done,
            "fail" => Until::Fail,
            "first" => Until::First,
            "each" => Until::Each,
            _ => return None,
        })
    }
}

/// Everything passed, or it merged.
pub const EXIT_PASSED: i32 = 0;
/// A check failed.
pub const EXIT_FAILED: i32 = 1;
/// Still waiting: run it again.
pub const EXIT_TIMED_OUT: i32 = 2;
/// Closed, or rush couldn't ask.
pub const EXIT_ERROR: i32 = 3;

/// Asks GitHub for a pull request: repo, number, how fresh an answer
/// another watcher had may be.
pub type FetchFn<'a> = Box<dyn Fn(&str, i64, Duration) -> (Result<PR, Error>, i64) + 'a>;
/// Waits a while; false when it was stopped.
pub type SleepFn<'a> = Box<dyn Fn(Duration) -> bool + 'a>;
/// A job's log: repo, job.
pub type LogsFn<'a> = Box<dyn Fn(&str, i64) -> Result<Vec<u8>, Error> + 'a>;
/// Records what a watch did: kind and fields, as rush's event log has them.
/// Is told each line to print.
pub type SayFn<'a> = Box<dyn Fn(&str) + 'a>;
/// Says the time now.
pub type NowFn<'a> = Box<dyn Fn() -> Time + 'a>;
/// Says whether to stop.
pub type StoppedFn<'a> = Box<dyn Fn() -> bool + 'a>;
pub type LogFn<'a> = Box<dyn Fn(&str, Map<String, Value>) + 'a>;

/// What a watch waits for. Say is told each line to print; fetch, sleep,
/// logs and now are GitHub and the clock, which tests set; log records
/// what it did (rush's event log unless set: rush watch points it at the
/// session it's run for).
#[derive(Default)]
pub struct Options<'a> {
    pub repo: String,
    pub number: i64,
    pub until: Until,
    /// Zero: never.
    pub timeout: Duration,
    /// The background task the watch runs as, if it is one: it keeps a
    /// Live for it while it runs.
    pub task: String,
    pub say: Option<SayFn<'a>>,
    pub fetch: Option<FetchFn<'a>>,
    pub sleep: Option<SleepFn<'a>>,
    pub logs: Option<LogsFn<'a>>,
    pub now: Option<NowFn<'a>>,
    pub log: Option<LogFn<'a>>,
    /// Whether to stop: Go's ctx.Err() != nil. Unset, a signal stops it.
    pub stopped: Option<StoppedFn<'a>>,
}

impl Options<'_> {
    fn say(&self, s: &str) {
        if let Some(f) = &self.say {
            f(s)
        }
    }

    fn fetch(&self, fresh: Duration) -> (Result<PR, Error>, i64) {
        match &self.fetch {
            Some(f) => f(&self.repo, self.number, fresh),
            None => fetch(&self.repo, self.number, fresh),
        }
    }

    fn sleep(&self, d: Duration) -> bool {
        match &self.sleep {
            Some(f) => f(d),
            None => crate::stop::sleep(d),
        }
    }

    fn logs(&self, job: i64) -> Result<Vec<u8>, Error> {
        match &self.logs {
            Some(f) => f(&self.repo, job),
            None => gh::raw(&format!("repos/{}/actions/jobs/{job}/logs", self.repo)),
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
}

/// How long a watch waits for a check to show up before deciding none
/// will.
const NO_CHECKS_AFTER: Duration = Duration::from_secs(3 * 60);

/// How long to wait before asking again, a watch since long: often at
/// first, less as the wait grows, and much less when the user's rate limit
/// runs low.
pub fn interval(since: Duration, remaining: i64) -> Duration {
    let mut d = Duration::from_secs(15);
    if since >= Duration::from_secs(15 * 60) {
        d = Duration::from_secs(60);
    } else if since >= Duration::from_secs(5 * 60) {
        d = Duration::from_secs(30);
    }
    if (0..100).contains(&remaining) {
        d = d.max(Duration::from_secs(5 * 60));
    } else if (0..500).contains(&remaining) {
        d = d.max(Duration::from_secs(2 * 60));
    }
    d
}

fn since(now: Time, start: Time) -> Duration {
    Duration::from_nanos(now.sub_nanos(start).clamp(0, u64::MAX as i128) as u64)
}

fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Waits on pull request o.number as o.until has it, and says once,
/// briefly, what happened; it gives the exit code to end with.
pub fn watch_pr(o: &Options) -> i32 {
    let start = o.now();
    let mut run = Run {
        remaining: -1,
        last: None,
        why: String::new(),
    };
    o.log(
        "watch-pr",
        fields(
            json!({"repo": o.repo, "pr": o.number, "until": o.until.as_str(),
            "timeout": duration(o.timeout)}),
        ),
    );
    let code = watching(o, start, &mut run);
    let mut f = fields(json!({"repo": o.repo, "pr": o.number, "code": code,
        "after": duration(round_to(since(o.now(), start), Duration::from_secs(1))),
        "remaining": run.remaining}));
    if !run.why.is_empty() {
        f.insert("why".into(), run.why.clone().into());
    }
    if let Some(last) = &run.last {
        let (passed, failed, pending, skipped) = last.counts();
        f.insert("state".into(), last.state.clone().into());
        f.insert("head".into(), short(&last.head).into());
        f.insert("passed".into(), passed.into());
        f.insert("failed".into(), failed.into());
        f.insert("pending".into(), pending.into());
        f.insert("skipped".into(), skipped.into());
        let names: Vec<&str> = last
            .checks
            .iter()
            .filter(|c| c.state == "failed")
            .map(|c| c.name.as_str())
            .collect();
        if !names.is_empty() {
            f.insert("failedChecks".into(), names.into());
        }
    }
    o.log("watch-pr-end", f);
    live::drop_live(&o.task);
    code
}

/// What a watch keeps for its last line in the log.
struct Run {
    remaining: i64,
    /// The last answer, for the log.
    last: Option<PR>,
    why: String,
}

fn watching(o: &Options, start: Time, run: &mut Run) -> i32 {
    let mut prev: Option<PR> = None;
    let mut fails = 0;
    let mut limited = false;
    let mut asked = Time::default();
    loop {
        let (res, r) = o.fetch(Duration::from_secs(10));
        if r >= 0 {
            run.remaining = r;
        }
        if o.stopped() {
            run.why = "stopped".into();
            return EXIT_ERROR;
        }
        match res {
            Err(Error::Limited) => {
                run.remaining = 0;
                if !limited {
                    limited = true;
                    o.log(
                        "watch-pr-limited",
                        fields(json!({"repo": o.repo, "pr": o.number,
                            "why": Error::Limited.to_string()})),
                    );
                }
            }
            Err(err) => {
                // A blip is asked again; three in a row end it.
                fails += 1;
                if fails >= 3 || prev.is_none() {
                    o.say(&format!("rush watch: {err}"));
                    run.why = format!("GitHub couldn't be asked: {err}");
                    return EXIT_ERROR;
                }
            }
            Ok(p) => {
                fails = 0;
                limited = false;
                asked = o.now();
                run.last = Some(p.clone());
                if let Some(code) = step(o, prev.as_ref(), &p, since(o.now(), start)) {
                    return code;
                }
                prev = Some(p);
            }
        }
        let gone = since(o.now(), start);
        let wait = interval(gone, run.remaining);
        live::save_live(&live::live(
            o,
            &o.task,
            start,
            prev.as_ref(),
            asked,
            o.now() + wait,
            limited,
        ));
        if !o.timeout.is_zero() && gone + wait > o.timeout {
            if let Some(left) = o.timeout.checked_sub(gone).filter(|l| !l.is_zero())
                && o.sleep(left)
            {
                // one last look before giving up
                if let (Ok(p), _) = o.fetch(Duration::ZERO) {
                    run.last = Some(p.clone());
                    if let Some(code) = step(o, prev.as_ref(), &p, since(o.now(), start)) {
                        return code;
                    }
                    prev = Some(p);
                }
            }
            o.say(&waiting(prev.as_ref(), since(o.now(), start)));
            run.why = "timed out".into();
            return EXIT_TIMED_OUT;
        }
        if !o.sleep(wait) {
            run.why = "stopped".into();
            return EXIT_ERROR;
        }
    }
}

/// Looks at p, after prev (None the first time), and says whether the
/// watch is over, and with what code.
fn step(o: &Options, prev: Option<&PR>, p: &PR, since: Duration) -> Option<i32> {
    if p.state == "merged" || p.state == "closed" {
        o.say(&format!("PR #{} was {} · {}", p.number, p.state, p.title));
        return Some(if p.state == "merged" {
            EXIT_PASSED
        } else {
            EXIT_ERROR
        });
    }
    let new_head = prev.is_some_and(|prev| prev.head != p.head);
    if new_head && o.until == Until::First {
        o.say(&format!(
            "PR #{} has a new commit, {}: its checks start again",
            p.number,
            short(&p.head)
        ));
        return Some(EXIT_PASSED);
    }
    if new_head && o.until == Until::Each {
        o.say(&format!(
            "· new commit {}: checks start again",
            short(&p.head)
        ));
    }
    let mut finished: Vec<&Check> = Vec::new(); // since the last look
    if let Some(prev) = prev {
        let mut before = HashSet::new();
        if !new_head {
            for c in &prev.checks {
                if c.finished() {
                    before.insert(c.name.as_str());
                }
            }
        }
        for c in &p.checks {
            if c.finished() && !before.contains(c.name.as_str()) {
                finished.push(c);
            }
        }
    }
    if o.until == Until::Each {
        for c in &finished {
            o.say(&check_line(c));
        }
    }
    let (_, failed, pending, _) = p.counts();
    let all = !p.checks.is_empty() && pending == 0;
    if o.until == Until::Fail && failed > 0 || all {
        o.say(&summary(o, p, since));
        return Some(if failed > 0 { EXIT_FAILED } else { EXIT_PASSED });
    }
    if p.checks.is_empty() && since >= NO_CHECKS_AFTER {
        o.say(&format!(
            "PR #{}: no checks on {} after {} · {}",
            p.number,
            short(&p.head),
            round(since),
            p.title
        ));
        return Some(EXIT_PASSED);
    }
    if o.until == Until::First && !finished.is_empty() {
        let mut lines = vec![format!("PR #{}: {}", p.number, progress(p))];
        let mut code = EXIT_PASSED;
        for c in finished {
            lines.push(check_line(c));
            if c.state == "failed" {
                code = EXIT_FAILED;
                lines.extend(error_lines(o, c));
            }
        }
        o.say(&lines.join("\n"));
        return Some(code);
    }
    None
}

/// What a finished watch says: the counts, and each failed check with the
/// lines of its log that say why. Passes aren't named.
pub fn summary(o: &Options, p: &PR, since: Duration) -> String {
    let (passed, failed, pending, skipped) = p.counts();
    let mut head = format!("PR #{} checks", p.number);
    if pending == 0 {
        head += " finished";
    }
    let mut parts = vec![format!("{passed} passed")];
    if failed > 0 {
        parts.push(format!("{failed} failed"));
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
        p.title
    )];
    let mut shown = 0;
    for c in &p.checks {
        if c.state != "failed" {
            continue;
        }
        shown += 1;
        if shown > 5 {
            lines.push(format!("… and {} more failed", failed - 5));
            break;
        }
        lines.push(check_line(c));
        lines.extend(error_lines(o, c));
    }
    lines.join("\n")
}

fn check_line(c: &Check) -> String {
    let mark = match c.state.as_str() {
        "passed" => "✓",
        "failed" => "✗",
        "skipped" => "-",
        _ => "",
    };
    let mut line = format!("{mark} {} {}", c.name, c.state);
    if c.state == "failed" && !c.url.is_empty() {
        line += " · ";
        line += &c.url;
    }
    line
}

/// The lines that say why c failed, indented: its job's log's errors when
/// GitHub Actions ran it, else what it says of itself.
fn error_lines(o: &Options, c: &Check) -> Vec<String> {
    let mut lines = Vec::new();
    if c.actions
        && c.id != 0
        && let Ok(b) = o.logs(c.id)
    {
        lines = log_errors(&String::from_utf8_lossy(&b), 12);
    }
    if lines.is_empty() && !c.note.is_empty() {
        lines = vec![c.note.clone()];
    }
    lines.into_iter().map(|l| format!("    {l}")).collect()
}

static STAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{4}-[0-9][0-9]-[0-9][0-9]T[0-9:.]+Z ?").unwrap());
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" {3,}").unwrap());
static ERR_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(^|[^a-z])(error|fail(ed|ure)?|panic|fatal|exception|traceback)([^a-z]|$)|✗|✘|×",
    )
    .unwrap()
});
/// Lines a failed step ends with that say only that it failed.
static NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[ELIFECYCLE\]|^npm (ERR!|error) (code|errno|command|path|A complete log)|^error Command failed with exit code|^Error: Process completed").unwrap()
});

/// At most n lines of an Actions job's log that say what went wrong,
/// without timestamps or colour. For each step that failed, its name and
/// then its errors and the last lines it wrote; the step's command and the
/// steps that passed are left out.
pub fn log_errors(log: &str, n: usize) -> Vec<String> {
    #[derive(Default, Clone)]
    struct Step {
        name: String,
        out: Vec<String>,
    }
    let mut cur = Step::default();
    let mut failed: Vec<Step> = Vec::new();
    let mut all: Vec<String> = Vec::new(); // a log with no steps: what it says before it gives up
    let mut in_group = false;
    for raw in log.split('\n') {
        let raw = raw.trim_end_matches('\r');
        let unstamped = STAMP.replace(raw, "");
        let l = ANSI.replace_all(&unstamped, "");
        let l = l.trim();
        if let Some(name) = l.strip_prefix("##[group]Run ") {
            cur = Step {
                name: name.to_string(),
                out: Vec::new(),
            };
            in_group = true;
            continue;
        }
        if l.starts_with("##[group]") {
            in_group = true; // the command, its shell and env: not what it said
            continue;
        }
        if l.starts_with("##[endgroup]") {
            in_group = false;
            continue;
        }
        if l.starts_with("##[error]Process completed with exit code") {
            if failed.is_empty() && cur.name.is_empty() {
                cur.out = all.clone();
            }
            failed.push(std::mem::take(&mut cur));
            continue;
        }
        if in_group || l.is_empty() || l.starts_with("[command]") || NOISE.is_match(l) {
            continue;
        }
        let l = l.strip_prefix("##[error]").unwrap_or(l);
        let l = clip(&SPACES.replace_all(l, "  "), 200);
        cur.out.push(l.clone());
        all.push(l);
    }
    if failed.is_empty() {
        failed = vec![Step {
            name: String::new(),
            out: all,
        }];
    }
    let mut out = Vec::new();
    let per = (n / failed.len()).max(4);
    for (i, s) in failed.iter().enumerate() {
        if i == 3 {
            out.push(format!("… and {} more failed steps", failed.len() - 3));
            break;
        }
        if !s.name.is_empty() {
            out.push(format!("step: {}", clip(&s.name, 80)));
        }
        out.extend(pick(&s.out, per));
    }
    out
}

/// At most n of a failed step's lines, in order: the last that look like
/// errors, up to half, and the last it wrote.
pub fn pick(lines: &[String], n: usize) -> Vec<String> {
    let mut keep = HashSet::new();
    for i in (0..lines.len()).rev() {
        if keep.len() >= n / 2 {
            break;
        }
        if ERR_LIKE.is_match(&lines[i]) {
            keep.insert(i);
        }
    }
    for i in (0..lines.len()).rev() {
        if keep.len() >= n {
            break;
        }
        keep.insert(i);
    }
    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| keep.contains(i))
        .map(|(_, l)| l.clone())
        .collect()
}

/// What a watch that timed out says: how far the checks got.
fn waiting(p: Option<&PR>, since: Duration) -> String {
    let Some(p) = p else {
        return format!(
            "still waiting after {}: GitHub hasn't answered yet; run it again",
            round(since)
        );
    };
    let mut running: Vec<&str> = p
        .checks
        .iter()
        .filter(|c| !c.finished())
        .map(|c| c.name.as_str())
        .collect();
    if running.len() > 8 {
        running.truncate(8);
        running.push("…");
    }
    let mut line = format!(
        "PR #{} still waiting after {}: {}; run it again",
        p.number,
        round(since),
        progress(p)
    );
    if !running.is_empty() {
        line += "\n    running: ";
        line += &running.join(", ");
    }
    line
}

/// How far its checks have got: "9/12 finished, 1 failed".
fn progress(p: &PR) -> String {
    let (_, failed, pending, _) = p.counts();
    if p.checks.is_empty() {
        return "no checks yet".into();
    }
    let mut s = format!("{}/{} finished", p.checks.len() - pending, p.checks.len());
    if failed > 0 {
        s += &format!(", {failed} failed");
    }
    s
}

static PR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"github\.com/([^/\t\n\x0c\r ]+/[^/\t\n\x0c\r ]+)/pull/([0-9]+)").unwrap()
});

/// Go's strconv.Atoi: an optional sign, then decimal digits.
fn atoi(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// The repository and pull request number what names: a number (in dir's
/// repository), owner/name#number, a pull request's URL, or "" for the
/// pull request of dir's branch.
pub fn resolve(dir: &Path, what: &str) -> Result<(String, i64), String> {
    let what = what.trim();
    if let Some(m) = PR_URL.captures(what) {
        return Ok((m[1].to_string(), atoi(&m[2]).unwrap_or(0)));
    }
    if let Some((repo, num)) = what.split_once('#')
        && repo.matches('/').count() == 1
    {
        return match atoi(num) {
            Some(n) if n > 0 => Ok((repo.to_string(), n)),
            _ => Err(format!("{} isn't a pull request", quote(what))),
        };
    }
    let gh = |args: &[&str]| -> Result<String, String> {
        let out = gh::run(Some(dir), args);
        match &out.err {
            Some(e @ rush_gh::ghpr::RunError::Exit { .. }) => {
                Err(gh::gh_error(0, &[], &out.stderr, Some(e)).to_string())
            }
            Some(e) => Err(e.to_string()),
            None => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        }
    };
    let mut n = 0;
    if !what.is_empty() {
        match atoi(what.strip_prefix('#').unwrap_or(what)) {
            Some(v) if v > 0 => n = v,
            _ => {
                return Err(format!(
                    "{} isn't a pull request: give its number, owner/name#number or its URL",
                    quote(what)
                ));
            }
        }
    }
    let repo = gh(&[
        "repo",
        "view",
        "--json",
        "nameWithOwner",
        "--jq",
        ".nameWithOwner",
    ])?;
    if n == 0 {
        let s = gh(&["pr", "view", "--json", "number", "--jq", ".number"])
            .map_err(|e| format!("no pull request for this branch: {e}"))?;
        n = atoi(&s).ok_or_else(|| "no pull request for this branch".to_string())?;
    }
    Ok((repo, n))
}
