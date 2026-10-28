//! Claude Code's sessions as rows of rush's agent list: background jobs,
//! sessions open in a terminal and past conversations, with what their
//! transcripts cost; and what each row's state means (needs you, your
//! turn, halted, stuck). Claude-only for now. Port of
//! internal/fleet/fleet.go:118-200 (predicates, applyStatus), 677-940
//! (load: jobs, interactive sessions), 1312-1363 (gitFor, follows, gitAt),
//! 1538 (isPrint), 1875 (jobCwd); fleet/past.go:58 (pastAgents);
//! fleet/stall.go; fleet/stuck.go; agent/failure KindOf.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rush_core::agent::{Agent, Spend, State};
use rush_core::time::Ms;

use crate::account::{Account, path_str};
use crate::discover::Discovery;
use crate::jobs::{ClaudeJob, state_of};
use crate::json::{Str, contains, lines};
use crate::sessions;
use crate::sessions::{Session, alive, read_workers};
use crate::spend::{Scanner, Target};
use crate::transcript::read_subagent_stats;
use rush_sys::proc;

const MINUTE: Ms = 60_000;
const HOUR: Ms = 60 * MINUTE;

/// Keeps what's cheap to keep between readings of the list.
pub struct Loader {
    acct: Account,
    disc: Discovery,
    scanner: Scanner,
    spend: HashMap<String, Spend>,
    git: HashMap<String, GitInfo>,
    /// Whether each live process runs print mode, by pid and start.
    print: HashMap<(i32, Ms), (bool, bool)>,
    /// Transcripts found away from their session's folder, by session id;
    /// and when one not found anywhere may be looked for again.
    moved: HashMap<String, PathBuf>,
    unfound: HashMap<String, Ms>,
    /// What the live agents' processes use (fleet.go:1263 sample).
    procs: proc::Sampler,
}

struct GitInfo {
    repo: String,
    branch: String,
    at: Ms,
}

/// Reads the list from disk once, as `rush --dump` does: the rows, then
/// their transcripts priced, then the rows again with their spend.
pub fn load(now: Ms) -> Vec<Agent> {
    let mut l = Loader::new(Account::home());
    let rows = l.load(now);
    l.scan(&rows, now);
    l.load(now)
}

impl Loader {
    pub fn new(acct: Account) -> Loader {
        Loader {
            acct,
            disc: Discovery::new(),
            scanner: Scanner::new(),
            spend: HashMap::new(),
            git: HashMap::new(),
            print: HashMap::new(),
            moved: HashMap::new(),
            unfound: HashMap::new(),
            procs: proc::Sampler::new(),
        }
    }

    /// What each row's transcripts should be priced from.
    pub fn targets(agents: &[Agent]) -> Vec<Target> {
        agents
            .iter()
            .filter(|a| !a.job.transcript_path.is_empty())
            .map(|a| Target {
                key: a.key.clone(),
                path: PathBuf::from(&a.job.transcript_path),
                live: a.job.live() || a.pid != 0,
                past: a.past,
            })
            .collect()
    }

    /// Prices the rows' transcripts, as far as they grew; the next load
    /// shows it. Returns how many rows' spend changed.
    pub fn scan(&mut self, agents: &[Agent], now: Ms) -> usize {
        let got = self.scanner.run(&Loader::targets(agents), now);
        let n = got.len();
        self.set_spend(got);
        n
    }

    /// Takes spend priced elsewhere, by row key.
    pub fn set_spend(&mut self, m: HashMap<String, Spend>) {
        self.spend.extend(m);
    }

    /// The list's rows, newest change first.
    pub fn load(&mut self, now: Ms) -> Vec<Agent> {
        let acct = self.acct.clone();
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        // Conversations a row already stands for: the rest are past ones.
        let mut claimed: HashSet<String> = HashSet::new();
        let jobs = self.disc.jobs(&acct, now);
        let sessions = self.disc.sessions(&acct);
        let workers = read_workers(&acct);
        let mut by_job: HashMap<&str, &Session> = HashMap::new();
        for ss in &sessions {
            claimed.insert(ss.session_id.clone());
            if !ss.job_id.is_empty() {
                by_job.insert(&ss.job_id, ss);
            }
        }
        for cj in &jobs {
            let a = self.job_agent(
                &acct,
                cj,
                by_job.get(cj.job.id.as_str()).copied(),
                &workers,
                now,
            );
            seen.insert(a.key.clone());
            claimed.insert(cj.job.session_id.clone());
            out.push(a);
        }
        for ss in &sessions {
            if ss.kind != "interactive"
                || ss.session_id.len() < 8
                || !ss.session_id.is_char_boundary(8)
            {
                continue;
            }
            if let Some(a) = self.session_agent(&acct, ss, now) {
                seen.insert(a.key.clone());
                out.push(a);
            }
        }
        self.past_agents(&acct, &claimed, &mut seen, now, &mut out);
        out.sort_by_key(|a| a.age(now));
        self.sample(&mut out);
        out
    }

    /// Each agent with a process: its tree's memory, CPU and process
    /// count (fleet.go:1263).
    fn sample(&mut self, agents: &mut [Agent]) {
        let roots: Vec<i32> = agents.iter().map(|a| a.pid).filter(|&p| p > 0).collect();
        if roots.is_empty() {
            return;
        }
        let got = self.procs.sample(&roots);
        for a in agents.iter_mut().filter(|a| a.pid > 0) {
            if let Some(u) = got.get(&a.pid) {
                (a.mem, a.cpu, a.procs) = (u.mem, u.cpu, u.procs);
            }
        }
    }

    fn job_agent(
        &mut self,
        acct: &Account,
        cj: &ClaudeJob,
        ss: Option<&Session>,
        workers: &[(String, i32)],
        now: Ms,
    ) -> Agent {
        let j = &cj.job;
        let mut a = Agent {
            job: j.clone(),
            key: key(acct, &j.id),
            display_name: j.name.clone(),
            ..Agent::default()
        };
        let mut st = cj.raw_state.as_str();
        if let Some(ss) = ss {
            st = apply_status(&mut a, st, ss);
        }
        if st == "running" {
            st = "done"; // only a busy session makes it work
        }
        a.job.state = state_of(st);
        // A job's worktree, not its cwd, is where it actually runs.
        a.job.cwd = job_cwd(&j.cwd, &j.worktree_path).to_string();
        (a.repo, a.branch) = self.git_for(&a.job.cwd, now);
        if a.branch.is_empty() && !j.worktree_branch.is_empty() {
            // Claude Code's record of the branch it made the worktree on,
            // for before the checkout exists to read.
            a.branch = j.worktree_branch.clone();
        }
        a.spend = self.spend.get(&a.key).cloned().unwrap_or_default();
        if !j.transcript_path.is_empty() && (a.job.live() || now - j.updated_at < 24 * HOUR) {
            // Its process isn't always known, so it's never taken as gone.
            a.subs = read_subagent_stats(Path::new(&j.transcript_path), now);
        }
        if let Some(&(_, pid)) = workers.iter().find(|(id, _)| *id == j.id) {
            // A roster left behind by a crashed daemon names pids that are
            // gone; only a live process counts.
            if alive(pid) {
                a.pid = pid;
            }
        }
        if a.job.live() && a.pid == 0 {
            a.job.state = State::Stopped;
            a.job.detail = "lost its process".into();
        }
        a
    }

    fn session_agent(&mut self, acct: &Account, ss: &Session, now: Ms) -> Option<Agent> {
        let short = &ss.session_id[..8];
        let (headless, unstored) = self.is_print(ss.pid, ss.started_ms);
        if unstored {
            return None; // a one-shot claude -p keeps no transcript to open
        }
        let working = ss.status == "busy" || ss.status == "shell";
        let mut a = Agent {
            key: key(acct, &format!("i:{short}")),
            display_name: ss.name.clone(),
            interactive: true,
            headless,
            pid: ss.pid,
            ..Agent::default()
        };
        let j = &mut a.job;
        j.id = short.to_string();
        j.account = acct.name.clone();
        j.name = ss.name.clone();
        j.state = if working { State::Working } else { State::Idle };
        j.cwd = ss.cwd.clone();
        j.session_id = ss.session_id.clone();
        j.created_at = ss.started_at();
        j.updated_at = ss.updated_at();
        j.transcript_path = path_str(&self.transcript_of(acct, &ss.cwd, &ss.session_id, now));
        if !working {
            j.detail = if headless {
                "run by another program"
            } else {
                "open in a terminal"
            }
            .into();
        }
        a.spend = self.spend.get(&a.key).cloned().unwrap_or_default();
        // ss.cwd is only ever where the session started; its own
        // transcript says where it last actually worked.
        let mut dir = ss.cwd.clone();
        if !a.spend.dir.is_empty() && self.follows(&dir, &a.spend.dir.clone(), now) {
            dir.clone_from(&a.spend.dir);
        }
        (a.repo, a.branch) = self.git_for(&dir, now);
        a.job.cwd = dir;
        a.subs = read_subagent_stats(Path::new(&a.job.transcript_path), now);
        Some(a)
    }

    fn past_agents(
        &mut self,
        acct: &Account,
        claimed: &HashSet<String>,
        seen: &mut HashSet<String>,
        now: Ms,
        out: &mut Vec<Agent>,
    ) {
        let Loader {
            disc, spend, git, ..
        } = self;
        for f in disc.past(acct, now) {
            let Some(c) = &f.convo else { continue };
            let sid = &c.session_id;
            if sid.len() < 8 || !sid.is_char_boundary(8) || claimed.contains(sid) {
                continue;
            }
            let k = key(acct, &format!("i:{}", &sid[..8]));
            if !seen.insert(k.clone()) {
                continue;
            }
            let mut a = Agent {
                display_name: c.title.clone(),
                past: true,
                ..Agent::default()
            };
            let j = &mut a.job;
            j.id = sid[..8].to_string();
            j.account = acct.name.clone();
            j.name = c.title.clone();
            j.state = State::Stopped;
            j.cwd = c.cwd.clone();
            j.session_id = sid.clone();
            j.created_at = c.started;
            j.updated_at = f.modified;
            j.transcript_path = path_str(&f.path);
            a.spend = spend.get(&k).cloned().unwrap_or_default();
            let dir = if a.spend.dir.is_empty() {
                &c.cwd
            } else {
                &a.spend.dir
            };
            (a.repo, a.branch) = git_for(git, dir, now);
            if now - f.modified < 24 * HOUR {
                a.subs = read_subagent_stats(&f.path, now);
            }
            a.key = k;
            out.push(a);
        }
    }

    fn git_for(&mut self, dir: &str, now: Ms) -> (String, String) {
        git_for(&mut self.git, dir, now)
    }

    /// Whether a row in cwd goes where its transcript last worked, dir: a
    /// checkout, or a folder above its own. A cd to a folder no checkout
    /// holds (its memory, logs) leaves it where it is.
    fn follows(&mut self, cwd: &str, dir: &str, now: Ms) -> bool {
        if !self.git_for(dir, now).0.is_empty() {
            return true;
        }
        cwd.is_empty() || (cwd.starts_with(dir) && cwd.as_bytes().get(dir.len()) == Some(&b'/'))
    }

    fn is_print(&mut self, pid: i32, started: Ms) -> (bool, bool) {
        if let Some(&r) = self.print.get(&(pid, started)) {
            return r;
        }
        let r = is_print(&proc::args(pid));
        self.print.retain(|(p, _), _| sessions::alive(*p));
        self.print.insert((pid, started), r);
        r
    }

    /// Where a session's conversation is now: under the folder it started
    /// in, or, once entering a worktree has moved it, under whichever
    /// project has it.
    fn transcript_of(&mut self, acct: &Account, cwd: &str, sid: &str, now: Ms) -> PathBuf {
        if let Some(p) = self.moved.get(sid)
            && p.exists()
        {
            return p.clone();
        }
        let at = acct.transcript_path(cwd, sid);
        if at.exists() {
            self.moved.remove(sid);
            self.unfound.remove(sid);
            return at;
        }
        if self.unfound.get(sid).is_some_and(|&next| now < next) {
            return at;
        }
        let name = format!("{sid}.jsonl");
        let found = std::fs::read_dir(acct.projects_dir()).ok().and_then(|r| {
            r.flatten()
                .map(|e| e.path().join(&name))
                .find(|p| p.exists())
        });
        match found {
            Some(p) => {
                self.moved.insert(sid.to_string(), p.clone());
                self.unfound.remove(sid);
                p
            }
            None => {
                self.unfound.insert(sid.to_string(), now + MINUTE);
                at
            }
        }
    }
}

/// Whether args run Claude Code in print mode, and whether it keeps no
/// transcript at all (fleet.go:1538 isPrint).
pub fn is_print<S: AsRef<str>>(args: &[S]) -> (bool, bool) {
    let (mut print, mut unstored) = (false, false);
    for a in args {
        let a = a.as_ref();
        if a == "-p" || a == "--print" || a.starts_with("--output-format") {
            print = true;
        }
        if a == "--no-session-persistence" {
            unstored = true;
        }
    }
    (print, unstored)
}

/// A row's key: its account and id, as rush's state keeps them.
fn key(acct: &Account, id: &str) -> String {
    format!("{}/{id}", acct.name)
}

/// Trusts the session's live busy/idle flag over the job file, whose
/// state Claude Code only re-summarises every 15-40s. Returns the state.
fn apply_status<'a>(a: &mut Agent, st: &'a str, ss: &Session) -> &'a str {
    match (ss.status.as_str(), st) {
        ("busy", "running") => "working",
        ("busy", "blocked") => {
            a.job.needs.clear();
            a.job.detail.clear();
            "working"
        }
        ("busy", "done") if a.job.background.is_empty() => {
            a.job.detail.clear();
            "working"
        }
        ("idle", "working") if ss.status_ms != 0 && ss.status_ms > a.job.updated_at => {
            a.checking = true;
            a.job.needs.clear();
            "blocked"
        }
        _ => st,
    }
}

/// A job's worktree, not its cwd, is where it runs, unless its cwd is
/// inside it: Claude Code doesn't always move cwd once a job's given one.
fn job_cwd<'a>(cwd: &'a str, worktree: &'a str) -> &'a str {
    if worktree.is_empty() {
        return cwd;
    }
    let wt = worktree.trim_end_matches('/');
    if !cwd.is_empty()
        && (cwd == wt || (cwd.starts_with(wt) && cwd.as_bytes().get(wt.len()) == Some(&b'/')))
    {
        return cwd;
    }
    worktree
}

/// The checkout dir is in and its branch, kept 30s.
fn git_for(cache: &mut HashMap<String, GitInfo>, dir: &str, now: Ms) -> (String, String) {
    if dir.is_empty() {
        return Default::default();
    }
    if let Some(g) = cache.get(dir)
        && now - g.at < 30_000
    {
        return (g.repo.clone(), g.branch.clone());
    }
    let (repo, branch) = git_at(dir);
    cache.insert(
        dir.to_string(),
        GitInfo {
            repo: repo.clone(),
            branch: branch.clone(),
            at: now,
        },
    );
    (repo, branch)
}

/// The checkout dir is in, and its branch, read directly.
pub fn git_at(dir: &str) -> (String, String) {
    let mut d = Path::new(dir);
    loop {
        let s = d.as_os_str();
        if s == "/" || s == "." || s.is_empty() {
            return Default::default();
        }
        let p = d.join(".git");
        if let Ok(m) = std::fs::metadata(&p) {
            let mut git_dir = p.clone();
            if !m.is_dir() {
                let b = std::fs::read_to_string(&p).unwrap_or_default();
                let s = b.trim().strip_prefix("gitdir:").unwrap_or(b.trim()).trim();
                git_dir = if s.starts_with('/') {
                    PathBuf::from(s)
                } else {
                    d.join(s)
                };
            }
            let mut branch = String::new();
            if let Ok(h) = std::fs::read_to_string(git_dir.join("HEAD")) {
                let h = h.trim();
                branch = h.strip_prefix("ref: refs/heads/").unwrap_or(h).to_string();
                if branch.len() == 40 {
                    branch.truncate(8);
                }
            }
            return (path_str(d), branch);
        }
        match d.parent() {
            Some(p) => d = p,
            None => return Default::default(),
        }
    }
}

/// How long until an agent at work that has written nothing shows as
/// stuck; the same for a subagent run, whose prompt cache lasts five
/// minutes, so a longer wait buys nothing.
pub const STUCK_AFTER: Ms = 30 * MINUTE;
pub const STUCK_SUB_AFTER: Ms = 5 * MINUTE;

/// What a row's state means. Seen, put-away (done) and rush-hosted
/// sessions aren't known here yet: rush's own state keeps them.
pub trait Predicates {
    /// A live agent asking something the user has not looked at yet.
    fn needs_you(&self) -> bool;
    /// A finished turn whose background work is still running in a live
    /// process; subagents still writing count too.
    fn busy(&self) -> bool;
    /// A turn that ended moments ago; it lingers in Working so a finish is
    /// noticed rather than vanishing into history.
    fn just_finished(&self, now: Ms) -> bool;
    /// Its last turn ended on an error: it sits there until told to go on.
    fn halted(&self) -> bool;
    /// A halt in the last day that trying again gets past.
    fn continues(&self, now: Ms) -> bool;
    /// A halt the network being down caused.
    fn offline(&self) -> bool;
    /// It finished its turn without asking anything: waiting for the next
    /// thing to do.
    fn your_turn(&self, now: Ms) -> bool;
    /// A halt said briefly, for a row.
    fn halt_reason(&self, now: Ms) -> String;
    /// What going on means for it: "continue" after an error, else "keep
    /// going".
    fn continue_text(&self) -> &'static str;
    /// At work and quiet for STUCK_AFTER.
    fn stuck(&self, now: Ms) -> bool;
    /// How long since it last wrote.
    fn quiet(&self, now: Ms) -> Ms;
    /// Time since its last change: what the list prints on the right.
    fn age(&self, now: Ms) -> Ms;
    /// How long it has existed, frozen once it stops.
    fn elapsed(&self, now: Ms) -> Ms;
}

impl Predicates for Agent {
    fn needs_you(&self) -> bool {
        self.job.state == State::Blocked && !self.checking && self.pid != 0
    }

    fn busy(&self) -> bool {
        self.pid != 0 && self.job.busy()
            || !self.job.live() && self.subs.direct + self.subs.nested > 0
    }

    fn just_finished(&self, now: Ms) -> bool {
        self.job.state == State::Done && !self.busy() && now - self.job.updated_at < 2 * MINUTE
    }

    fn halted(&self) -> bool {
        self.spend.halt.is_some()
            && !self.job.live()
            && !self.busy()
            && !self.checking
            && !self.past
            && !self.interactive
            && self.job.state != State::Stopped
    }

    fn continues(&self, now: Ms) -> bool {
        match &self.spend.halt {
            Some(h) if self.halted() && now - h.at < 24 * HOUR => failure_kind(&h.text).retries(),
            _ => false,
        }
    }

    fn offline(&self) -> bool {
        self.spend
            .halt
            .as_ref()
            .is_some_and(|h| failure_kind(&h.text) == Failure::Offline)
    }

    fn your_turn(&self, now: Ms) -> bool {
        self.job.state == State::Done
            && !self.checking
            && !self.busy()
            && !self.past
            && !self.interactive
            && self.spend.halt.is_none()
            && now - self.job.updated_at < 24 * HOUR
    }

    fn halt_reason(&self, now: Ms) -> String {
        let Some(h) = &self.spend.halt else {
            return String::new();
        };
        let t = h.text.strip_prefix("API Error: ").unwrap_or(&h.text);
        if t.is_empty() {
            return h.kind.replace('_', " ");
        }
        if h.kind == "rate_limit" {
            // "You've hit your session limit · resets 5am (Europe/London)"
            let t = t.strip_prefix("You've hit your ").unwrap_or(t);
            return match t.find(" (") {
                Some(i) if i > 0 => t[..i].to_string(),
                _ => t.to_string(),
            };
        }
        if self.continues(now) && self.offline() {
            return "offline · continues when the network is back".into();
        }
        if self.continues(now) {
            return format!("{t} · continues by itself");
        }
        t.to_string()
    }

    fn continue_text(&self) -> &'static str {
        if self.spend.halt.is_some() {
            "continue"
        } else {
            "keep going"
        }
    }

    fn stuck(&self, now: Ms) -> bool {
        if self.past || self.checking || !(self.job.state == State::Working || self.busy()) {
            return false;
        }
        self.quiet(now) >= STUCK_AFTER
    }

    fn quiet(&self, now: Ms) -> Ms {
        now - self.job.updated_at.max(self.job.mod_time)
    }

    fn age(&self, now: Ms) -> Ms {
        now - self.job.updated_at.max(self.job.mod_time)
    }

    fn elapsed(&self, now: Ms) -> Ms {
        let mut end = now;
        if !self.job.live() {
            end = self.job.updated_at;
            if self.spend.last != 0 && self.spend.last < end {
                end = self.spend.last;
            }
        }
        (end - self.job.created_at).max(0)
    }
}

/// What an error says about the provider (Go's failure.Kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Fatal,
    Limit,
    Auth,
    Capacity,
    Offline,
    Transient,
}

impl Failure {
    /// Whether trying again gets past an error of this kind.
    pub fn retries(self) -> bool {
        matches!(
            self,
            Failure::Capacity | Failure::Offline | Failure::Transient
        )
    }
}

/// What each kind's errors say, lowercased, looked for in this order: a
/// usage limit first, as its words come with rate-limit ones too.
const PHRASES: &[(Failure, &[&str])] = &[
    (
        Failure::Limit,
        &["usage limit", "limit reached", "usage_limit_reached"],
    ),
    (
        Failure::Auth,
        &[
            "401",
            "403",
            "invalid_grant",
            "invalid_token",
            "invalid_client",
            "invalid_api_key",
            "invalid api key",
            "invalid x-api-key",
            "unauthorized_client",
            "access_denied",
            "authentication",
            "permission_error",
            "unauthorized",
            "forbidden",
            "revoked",
            "oauth token",
            "failed to authenticate",
            "oauth session expired",
            "token has expired",
            "token expired",
            "expired token",
            "not logged in",
            "/login",
            "please log in",
            "log in again",
            "apikeyhelper",
        ],
    ),
    (
        Failure::Capacity,
        &[
            "429",
            "529",
            "overloaded",
            "rate_limit",
            "rate limit",
            "too many requests",
            "insufficient_quota",
            "quota",
        ],
    ),
    (
        Failure::Offline,
        &[
            "can't reach the api",
            "unable to connect to api",
            "no response from api",
            "went to sleep",
            "internet",
            "enotfound",
            "eai_again",
            "econnrefused",
            "enetunreach",
            "ehostunreach",
            "enetdown",
        ],
    ),
    (
        Failure::Transient,
        &[
            "500",
            "502",
            "503",
            "504",
            "api error: 5",
            "internal server error",
            "server_error",
            "api_error",
            "temporarily",
            "service unavailable",
            "timed out",
            "timeout",
            "etimedout",
            "econnreset",
            "socket hang up",
            "connection error",
            "connection lost",
            "connection reset",
            "connection closed",
            "mid-response",
            "mid-stream",
            "stopped arriving",
        ],
    ),
];

/// What an error the turn stopped on says about the provider.
pub fn failure_kind(err: &str) -> Failure {
    let t = err.to_lowercase();
    if t.contains("hit your") && t.contains("limit") {
        return Failure::Limit; // "You've hit your session limit · resets 5am"
    }
    for (k, says) in PHRASES {
        if says.iter().any(|s| says_it(&t, s)) {
            return *k;
        }
    }
    Failure::Fatal
}

/// Whether t says s; a code of digits only as a number of its own.
fn says_it(t: &str, s: &str) -> bool {
    if !s.bytes().all(|c| c.is_ascii_digit()) {
        return t.contains(s);
    }
    let b = t.as_bytes();
    let alnum = |c: u8| c.is_ascii_alphanumeric();
    let mut from = 0;
    while let Some(i) = t[from..].find(s) {
        let i = from + i;
        let end = i + s.len();
        if (i == 0 || !alnum(b[i - 1])) && (end == b.len() || !alnum(b[end])) {
            return true;
        }
        from = end;
    }
    false
}

/// Whether a subagent's transcript has a tool call still without its
/// result: it's running a command. Reads its last 256 KB.
pub fn ends_in_tool(path: &Path) -> bool {
    use std::os::unix::fs::FileExt;
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(m) = f.metadata() else { return false };
    let off = m.len().saturating_sub(256 << 10);
    let mut b = vec![0u8; (m.len() - off) as usize];
    if f.read_exact_at(&mut b, off).is_err() {
        return false;
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct L<'a> {
        #[serde(borrow)]
        message: M<'a>,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct M<'a> {
        #[serde(borrow)]
        content: Option<&'a serde_json::value::RawValue>,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct B<'a> {
        #[serde(rename = "type", borrow)]
        typ: Str<'a>,
        #[serde(borrow)]
        id: Str<'a>,
        #[serde(borrow)]
        tool_use_id: Str<'a>,
    }
    // Each call is its own line, and each result: one still open, of
    // several made at once, is a command still running.
    let mut open: HashSet<String> = HashSet::new();
    for ln in lines(&b) {
        if !contains(ln, br#""tool_use""#) && !contains(ln, br#""tool_result""#) {
            continue;
        }
        let Ok(l) = serde_json::from_slice::<L>(ln) else {
            continue;
        };
        let Some(c) = l.message.content else { continue };
        let Ok(blocks) = serde_json::from_str::<Vec<B>>(c.get()) else {
            continue;
        };
        for bl in blocks {
            match &*bl.typ {
                "tool_use" => {
                    open.insert(bl.id.to_string());
                }
                "tool_result" => {
                    open.remove(&*bl.tool_use_id);
                }
                _ => {}
            }
        }
    }
    !open.is_empty()
}

#[cfg(test)]
mod tests;
