//! The list's rows, read from every adapter in the registry: each one's
//! background jobs, sessions open in a terminal and past conversations,
//! with what their transcripts cost and what their processes use. Port of
//! internal/fleet/fleet.go:118-200 (applyStatus), 677-940 (load: jobs,
//! interactive sessions), 1067-1110 (transcriptOf), 1263 (sample),
//! 1538 (isPrint), 1875 (jobCwd), 494 (subagents); fleet/past.go:58
//! (pastAgents). Rows rush hosts, overlays (names, seen, done), PRs and
//! the machine's own processes come with the host and the fleet's other
//! screens.
//!
//! Nothing here names an agent: each adapter's sessions come through its
//! Discoverer, its daemon's workers through JobKeeper, where its
//! transcripts are through TranscriptLocator and Transcripts, their cost
//! through SpendReader and their subagents through RunFollower.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rush_core::adapter::{Adapter, AdapterRef, Profile, Registry, Session, SpendTarget};
use rush_core::agent::{Agent, Job, Spend, State, SubagentStats};
use rush_core::time::Ms;
use rush_sys::proc;

use crate::git::{GitInfo, git_for};

const MINUTE: Ms = 60_000;
const HOUR: Ms = 60 * MINUTE;

/// Keeps what's cheap to keep between readings of the list.
pub struct Loader {
    reg: Registry,
    spend: HashMap<String, Spend>,
    git: HashMap<String, GitInfo>,
    /// Whether each live process runs print mode, by pid and start.
    print: HashMap<(i32, Ms), (bool, bool)>,
    /// Transcripts found away from their session's folder, by session id;
    /// and when one not found anywhere may be looked for again.
    moved: HashMap<String, String>,
    unfound: HashMap<String, Ms>,
    /// What the live agents' processes use (fleet.go:1263 sample).
    procs: proc::Sampler,
    /// Each profile read, with its jobs' ids at the last reading: what
    /// watched names.
    read: Vec<(AdapterRef, Profile, Vec<String>)>,
    /// Each transcripts folder's subfolders, as listed when it was last
    /// modified: listed again only once it changes.
    dirs: RefCell<HashMap<PathBuf, (SystemTime, Vec<PathBuf>)>>,
}

impl Loader {
    pub fn new(reg: Registry) -> Loader {
        Loader {
            reg,
            spend: HashMap::new(),
            git: HashMap::new(),
            print: HashMap::new(),
            moved: HashMap::new(),
            unfound: HashMap::new(),
            procs: proc::Sampler::new(),
            read: Vec::new(),
            dirs: RefCell::new(HashMap::new()),
        }
    }

    /// What each row's transcripts should be priced from.
    pub fn targets(agents: &[Agent]) -> Vec<(&Agent, SpendTarget)> {
        agents
            .iter()
            .filter(|a| !a.job.transcript_path.is_empty())
            .map(|a| {
                let t = SpendTarget {
                    key: a.key.clone(),
                    path: a.job.transcript_path.clone(),
                    live: a.job.live() || a.pid != 0,
                    past: a.past,
                };
                (a, t)
            })
            .collect()
    }

    /// Prices the rows' transcripts, as far as they grew, each through its
    /// adapter's scanner; the next load shows it. Returns how many rows'
    /// spend changed.
    pub fn scan(&mut self, agents: &[Agent]) -> usize {
        let mut by_kind: HashMap<&str, Vec<SpendTarget>> = HashMap::new();
        for (a, t) in Loader::targets(agents) {
            by_kind.entry(a.kind.as_str()).or_default().push(t);
        }
        let mut n = 0;
        for (k, targets) in by_kind {
            let Some(r) = self.reg.get(k) else { continue };
            let Some(sr) = r.spend_reader() else { continue };
            let got = sr.spend_scanner().run(&targets);
            n += got.len();
            self.spend.extend(got);
        }
        n
    }

    /// Takes spend priced elsewhere, by row key.
    pub fn set_spend(&mut self, m: HashMap<String, Spend>) {
        self.spend.extend(m);
    }

    /// The list's rows, newest change first.
    pub fn load(&mut self, now: Ms) -> Vec<Agent> {
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        // Conversations a row already stands for: the rest are past ones.
        let mut claimed: HashSet<String> = HashSet::new();
        let mut read = Vec::new();
        for a in self.reg.all().iter() {
            if a.discoverer().is_none() {
                continue;
            }
            for p in a.profiles() {
                let ids = self.profile(a, &p, &mut seen, &mut claimed, now, &mut out);
                read.push((a.clone(), p, ids));
            }
        }
        self.read = read;
        out.sort_by_key(|a| crate::Predicates::age(a, now));
        self.sample(&mut out);
        out
    }

    /// One profile's rows: its jobs, then its sessions open in a terminal,
    /// then its past conversations. Returns its jobs' ids.
    fn profile(
        &mut self,
        a: &AdapterRef,
        p: &Profile,
        seen: &mut HashSet<String>,
        claimed: &mut HashSet<String>,
        now: Ms,
        out: &mut Vec<Agent>,
    ) -> Vec<String> {
        let Some(d) = a.discoverer() else {
            return Vec::new();
        };
        let (jobs, sessions): (Vec<Session>, Vec<Session>) =
            d.live(p).into_iter().partition(|s| s.job.is_some());
        let workers = a.job_keeper().map(|k| k.workers(p)).unwrap_or_default();
        let mut by_job: HashMap<&str, &Session> = HashMap::new();
        for ss in &sessions {
            claimed.insert(ss.id.clone());
            if !ss.job_id.is_empty() {
                by_job.insert(&ss.job_id, ss);
            }
        }
        let mut ids = Vec::with_capacity(jobs.len());
        for js in &jobs {
            let Some(j) = js.job.as_deref() else { continue };
            ids.push(j.id.clone());
            let ag = self.job_agent(
                a,
                p,
                js,
                j,
                by_job.get(j.id.as_str()).copied(),
                &workers,
                now,
            );
            seen.insert(ag.key.clone());
            claimed.insert(j.session_id.clone());
            out.push(ag);
        }
        for ss in &sessions {
            if !ss.interactive || ss.id.len() < 8 || !ss.id.is_char_boundary(8) {
                continue;
            }
            if let Some(ag) = self.session_agent(a, p, ss, now) {
                seen.insert(ag.key.clone());
                out.push(ag);
            }
        }
        self.past_agents(a, p, claimed, seen, now, out);
        ids
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

    #[allow(clippy::too_many_arguments)]
    fn job_agent(
        &mut self,
        ad: &AdapterRef,
        p: &Profile,
        js: &Session,
        j: &Job,
        ss: Option<&Session>,
        workers: &HashMap<String, i32>,
        now: Ms,
    ) -> Agent {
        let mut a = Agent {
            kind: ad.kind(),
            job: j.clone(),
            key: key(&p.name, &j.id),
            display_name: j.name.clone(),
            ..Agent::default()
        };
        // The job file's own word for its state.
        let mut st = js.state.as_str();
        if let Some(ss) = ss {
            st = apply_status(&mut a, st, ss);
        }
        if st == "running" {
            st = "done"; // only a busy session makes it work
        }
        a.job.state = State::from_word(st);
        // A job's worktree, not its cwd, is where it actually runs.
        a.job.cwd = job_cwd(&j.cwd, &j.worktree_path).to_string();
        (a.repo, a.branch) = git_for(&mut self.git, &a.job.cwd, now);
        if a.branch.is_empty() && !j.worktree_branch.is_empty() {
            // The agent's record of the branch it made the worktree on,
            // for before the checkout exists to read.
            a.branch = j.worktree_branch.clone();
        }
        a.spend = self.spend.get(&a.key).cloned().unwrap_or_default();
        if !j.transcript_path.is_empty() && (a.job.live() || now - j.updated_at < 24 * HOUR) {
            // Its process isn't always known, so it's never taken as gone.
            a.subs = subagents(ad.as_ref(), &j.transcript_path, now);
        }
        if let Some(&pid) = workers.get(&j.id) {
            // A roster left behind by a crashed daemon names pids that are
            // gone or reused; only a live process of the agent's program
            // counts (fleet.go:843).
            if self.is_program_pid(ad.kind().as_str(), pid) {
                a.pid = pid;
            }
        }
        if a.job.live() && a.pid == 0 {
            a.job.state = State::Stopped;
            a.job.detail = "lost its process".into();
        }
        a
    }

    fn session_agent(
        &mut self,
        ad: &AdapterRef,
        p: &Profile,
        ss: &Session,
        now: Ms,
    ) -> Option<Agent> {
        let short = &ss.id[..8];
        let (headless, unstored) = self.is_print(ss.pid, ss.created_at);
        if unstored {
            return None; // a one-shot claude -p keeps no transcript to open
        }
        let working = ss.status == "busy" || ss.status == "shell";
        let mut a = Agent {
            kind: ad.kind(),
            key: key(&p.name, &format!("i:{short}")),
            display_name: ss.name.clone(),
            interactive: true,
            headless,
            pid: ss.pid,
            ..Agent::default()
        };
        let transcript = self.transcript_of(ad.as_ref(), p, &ss.cwd, &ss.id, now);
        let j = &mut a.job;
        j.id = short.to_string();
        j.account = p.name.clone();
        j.name = ss.name.clone();
        j.state = if working { State::Working } else { State::Idle };
        j.cwd = ss.cwd.clone();
        j.session_id = ss.id.clone();
        j.created_at = ss.created_at;
        j.updated_at = ss.updated_at;
        j.transcript_path = transcript;
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
        (a.repo, a.branch) = git_for(&mut self.git, &dir, now);
        a.job.cwd = dir;
        a.subs = subagents(ad.as_ref(), &a.job.transcript_path, now);
        Some(a)
    }

    fn past_agents(
        &mut self,
        ad: &AdapterRef,
        p: &Profile,
        claimed: &HashSet<String>,
        seen: &mut HashSet<String>,
        now: Ms,
        out: &mut Vec<Agent>,
    ) {
        let Some(d) = ad.discoverer() else { return };
        for s in d.past(p) {
            let sid = &s.id;
            if sid.len() < 8 || !sid.is_char_boundary(8) || claimed.contains(sid) {
                continue;
            }
            let k = key(&p.name, &format!("i:{}", &sid[..8]));
            if !seen.insert(k.clone()) {
                continue;
            }
            let mut a = Agent {
                kind: ad.kind(),
                display_name: s.name.clone(),
                past: true,
                ..Agent::default()
            };
            let j = &mut a.job;
            j.id = sid[..8].to_string();
            j.account = p.name.clone();
            j.name = s.name.clone();
            j.state = State::Stopped;
            j.cwd = s.cwd.clone();
            j.session_id = sid.clone();
            j.created_at = s.created_at;
            j.updated_at = s.updated_at;
            j.transcript_path = s.transcript.clone();
            a.spend = self.spend.get(&k).cloned().unwrap_or_default();
            let dir = if a.spend.dir.is_empty() {
                &s.cwd
            } else {
                &a.spend.dir
            };
            (a.repo, a.branch) = git_for(&mut self.git, dir, now);
            if now - s.updated_at < 24 * HOUR {
                a.subs = subagents(ad.as_ref(), &a.job.transcript_path, now);
            }
            a.key = k;
            out.push(a);
        }
    }

    /// Whether a row in cwd goes where its transcript last worked, dir: a
    /// checkout, or a folder above its own. A cd to a folder no checkout
    /// holds (its memory, logs) leaves it where it is.
    fn follows(&mut self, cwd: &str, dir: &str, now: Ms) -> bool {
        if !git_for(&mut self.git, dir, now).0.is_empty() {
            return true;
        }
        cwd.is_empty() || (cwd.starts_with(dir) && cwd.as_bytes().get(dir.len()) == Some(&b'/'))
    }

    fn is_print(&mut self, pid: i32, started: Ms) -> (bool, bool) {
        if let Some(&r) = self.print.get(&(pid, started)) {
            return r;
        }
        let r = is_print(&proc::args(pid));
        self.print.retain(|(p, _), _| alive(*p));
        self.print.insert((pid, started), r);
        r
    }

    /// Where a session's conversation is now: where its agent keeps it for
    /// the folder it started in, or, once entering a worktree has moved
    /// it, wherever the agent finds it (fleet.go:1067 transcriptOf).
    fn transcript_of(
        &mut self,
        ad: &dyn Adapter,
        p: &Profile,
        cwd: &str,
        sid: &str,
        now: Ms,
    ) -> String {
        if let Some(found) = self.moved.get(sid)
            && std::path::Path::new(found).exists()
        {
            return found.clone();
        }
        let at = ad
            .transcript_locator()
            .map(|t| t.transcript_path(p, cwd, sid))
            .unwrap_or_default();
        if !at.is_empty() && std::path::Path::new(&at).exists() {
            self.moved.remove(sid);
            self.unfound.remove(sid);
            return at;
        }
        if self.unfound.get(sid).is_some_and(|&next| now < next) {
            return at;
        }
        let found = ad
            .transcripts()
            .map(|t| t.find_transcript(p, cwd, sid))
            .unwrap_or_default();
        if found.is_empty() {
            self.unfound.insert(sid.to_string(), now + MINUTE);
            return at;
        }
        self.moved.insert(sid.to_string(), found.clone());
        self.unfound.remove(sid);
        found
    }

    /// What the list watches changed (None: it can't tell what): each
    /// adapter reads what it kept of those paths again at the next load.
    pub fn changed(&mut self, paths: Option<&[PathBuf]>) {
        for (a, p, _) in &self.read {
            if let Some(d) = a.discoverer() {
                d.changed(p, paths);
            }
        }
    }

    /// top's subfolders, listed again only when top was modified since.
    fn subfolders(&self, top: &Path) -> Vec<PathBuf> {
        let Ok(at) = std::fs::metadata(top).and_then(|m| m.modified()) else {
            self.dirs.borrow_mut().remove(top);
            return Vec::new();
        };
        let mut dirs = self.dirs.borrow_mut();
        if let Some((was, subs)) = dirs.get(top)
            && *was == at
        {
            return subs.clone();
        }
        let mut subs = Vec::new();
        if let Ok(rd) = std::fs::read_dir(top) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    subs.push(e.path());
                }
            }
        }
        dirs.insert(top.to_path_buf(), (at, subs.clone()));
        subs
    }

    /// The folders and files whose changes mean a new reading: what each
    /// agent's JobKeeper names for the jobs last read, and each one's
    /// transcripts folder with the project folders in it (a folder watch
    /// doesn't see what happens in its subfolders).
    pub fn watched(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for (a, p, ids) in &self.read {
            if let Some(k) = a.job_keeper() {
                out.extend(k.watched(p, ids));
            }
            let Some(t) = a.transcripts() else { continue };
            let top = PathBuf::from(t.transcripts_dir(p));
            out.extend(self.subfolders(&top));
            out.push(top);
        }
        out.sort();
        out.dedup();
        out
    }
}

/// A row's subagents, as its agent counts them.
fn subagents(ad: &dyn Adapter, transcript: &str, now: Ms) -> SubagentStats {
    match ad.run_follower() {
        Some(f) if !transcript.is_empty() => f.count_subagents(transcript, now),
        _ => SubagentStats::default(),
    }
}

impl Loader {
    /// Whether pid is a live process of agent k's program (Go's
    /// isProgramPID, fleet.go:1465): the name it runs as is the program's
    /// (cut to 16 bytes, as macOS keeps it).
    fn is_program_pid(&self, k: &str, pid: i32) -> bool {
        let prog = self.reg.program_of(k);
        if prog.is_empty() || pid <= 0 || proc::zombie(pid) {
            return false;
        }
        proc::comm(pid).is_some_and(|c| {
            let c = c.rsplit('/').next().unwrap_or(&c);
            c == prog || (c.len() >= 15 && prog.starts_with(c))
        })
    }
}

/// Whether a process exists.
fn alive(pid: i32) -> bool {
    pid > 0 && proc::kill(pid, 0).is_ok()
}

/// A row's key: its profile's name and id, as rush's state keeps them.
fn key(profile: &str, id: &str) -> String {
    format!("{profile}/{id}")
}

/// Trusts the session's live busy/idle flag over the job file, whose
/// state Claude Code only re-summarises every 15-40s. Returns the state.
pub(crate) fn apply_status<'a>(a: &mut Agent, st: &'a str, ss: &Session) -> &'a str {
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
        ("idle", "working") if ss.status_at != 0 && ss.status_at > a.job.updated_at => {
            a.checking = true;
            a.job.needs.clear();
            "blocked"
        }
        _ => st,
    }
}

/// A job's worktree, not its cwd, is where it runs, unless its cwd is
/// inside it: Claude Code doesn't always move cwd once a job's given one.
pub(crate) fn job_cwd<'a>(cwd: &'a str, worktree: &'a str) -> &'a str {
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

/// Whether a claude command line runs it non-interactively, and whether
/// it keeps no session on disk (fleet.go:1538 isPrint).
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
