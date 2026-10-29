//! The list's rows, read from every source: the home agent's background
//! jobs (with the pins, workers and pull requests its daemon keeps), its
//! sessions open in a terminal, the sessions rush runs itself (through
//! the Hosts the host module implements), past conversations, and every
//! other agent's sessions, each with what you've set on it in rush's
//! state (names, groups, put away, seen, hidden), what its transcripts
//! cost and what its processes use. Port of internal/fleet/fleet.go
//! (Loader :260, the inbox :347-470, subagents :494, isPrint :594,
//! NewLoader :621, Load :647-1007, liveOf/isProgram :1011-1033,
//! hostedAgent :1036, transcriptOf :1067, hosted :1138, sample :1263,
//! readUsage :1273, gitFor/follows :1312-1333, cmdline :1365, isPrint
//! :1538, jobCwd :1875), fleet/past.go, others.go, idle_rows.go,
//! compact.go, memo.go and scan.go.
//!
//! Nothing here names an agent: each adapter's sessions come through its
//! Discoverer, its daemon's workers through JobKeeper, where its
//! transcripts are through TranscriptLocator and Transcripts, their cost
//! through SpendReader and their subagents through RunFollower.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rush_core::adapter::{
    AdapterRef, Compaction, Profile, Registry, Session, SpendTarget, SubagentRuns,
};
use rush_core::agent::{Agent, Job, Spend, State, SubagentStats, SubagentTile, Task};
use rush_core::time::{Ms, now as clock_now};
use rush_core::usage::{Quota, Reading};
use rush_gh::ghpr::Gh;
use rush_state::{Folder, Store};
use rush_sys::proc::{self, Pid, Proc, Table};

use crate::Predicates;
use crate::git::{GitInfo, git_for};
use crate::hosted::{
    HostConfig, HostInfo, Hosts, NoHosts, away_detail, clock, host_root, needs_decisions,
};
use crate::logins::{AccountView, LoginView, freshest, logins, reading_key};
use crate::machine::{Machine, ProgProcs, is_program_pid, machine};
use crate::temp::{TempSizes, temp_dirs};
use crate::watch::Watching;
use crate::worktree::{main_checkout_in, sub_worktrees, transcript_cwd};

const SECOND: Ms = 1000;
const MINUTE: Ms = 60 * SECOND;
const HOUR: Ms = 60 * MINUTE;

/// How often a past conversation's subagents are looked at again, and an
/// idle hosted row made again: none can start.
const PAST_EVERY: Ms = 15 * SECOND;
/// How often other agents' past sessions are listed again: listing reads
/// the head of every one.
const OTHERS_EVERY: Ms = MINUTE;
/// How long a row's compaction, as last worked out, holds.
const COMPACT_EVERY: Ms = 10 * SECOND;

/// The machine's processes, as the loader reads them: the real ones, or a
/// test's.
pub trait ProcSource: Send {
    /// Every process; memory and CPU are filled only for what's asked.
    fn snapshot(&mut self) -> Table;
    /// A process's arguments.
    fn args(&self, pid: Pid) -> Vec<String>;
    /// A process's environment, as NAME=value.
    fn env(&self, pid: Pid) -> Vec<String>;
}

/// This machine's processes.
pub struct System;

impl ProcSource for System {
    fn snapshot(&mut self) -> Table {
        proc::snapshot()
    }
    fn args(&self, pid: Pid) -> Vec<String> {
        proc::args(pid)
    }
    fn env(&self, pid: Pid) -> Vec<String> {
        proc::env(pid)
    }
}

/// One reading of the list.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub at: Ms,
    /// Newest change first.
    pub agents: Vec<Agent>,
    pub accounts: Vec<AccountView>,
    pub logins: Vec<LoginView>,
    pub machine: Machine,
    /// The process table this reading sampled, when it did.
    pub table: Option<Arc<Table>>,
    /// Every rush session's info as this reading listed it.
    pub hosted: Vec<HostInfo>,
}

/// What was handed in since the last load began.
#[derive(Default)]
struct Inbox {
    settle: bool,
    stale: bool,
    spend: HashMap<String, Spend>,
    nudged: HashMap<String, Ms>,
    /// In the order they came.
    fetched: Vec<(String, Reading)>,
    links: HashMap<String, (String, &'static str)>,
}

/// What the UI hands a loader while it reads: never waits for a load.
#[derive(Clone)]
pub struct Handle {
    inbox: Arc<Mutex<Inbox>>,
    skip_past: Arc<AtomicBool>,
}

impl Handle {
    fn with<R>(&self, f: impl FnOnce(&mut Inbox) -> R) -> R {
        f(&mut self.inbox.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Marks an agent the user just sent something to as working until
    /// its own files catch up.
    pub fn nudge(&self, key: &str) {
        self.with(|i| {
            i.nudged.insert(key.to_string(), clock_now());
            i.stale = true;
        });
    }

    /// Cost totals from the background scanner.
    pub fn set_spend(&self, m: HashMap<String, Spend>) {
        if m.is_empty() {
            return;
        }
        self.with(|i| {
            i.spend.extend(m);
            i.stale = true;
        });
    }

    /// A plan reading fetched for an account, kept under key.
    pub fn set_fetched(&self, key: &str, u: Reading) {
        self.with(|i| {
            i.fetched.push((key.to_string(), u));
            i.stale = true;
        });
    }

    /// The session with row key parent ran the agent with row key child
    /// from its shell: child is listed with it from then on.
    pub fn link_spawn(&self, child: &str, parent: &str) {
        self.link(child, parent, "opened");
    }

    /// What looking for a run's asker found: its session, or "" for none.
    pub(crate) fn link_found(&self, child: &str, parent: &str) {
        self.link(child, parent, "prompt in transcript");
    }

    fn link(&self, child: &str, parent: &str, how: &'static str) {
        self.with(|i| {
            i.links.insert(child.to_string(), (parent.to_string(), how));
            i.stale = true;
        });
    }

    /// Lets the next load reuse the last reading if nothing it reads has
    /// changed since. A refresh on a timer settles; one after something
    /// you did doesn't.
    pub fn settle(&self) {
        self.with(|i| i.settle = true);
    }

    /// Leaves the conversations nothing has open out of each reading, or
    /// puts them back in the next one.
    pub fn skip_past(&self, on: bool) {
        if self.skip_past.swap(on, Ordering::Relaxed) == on || on {
            return;
        }
        self.with(|i| i.stale = true);
    }
}

/// A row's subagents as last counted.
#[derive(Default)]
struct SubsEntry {
    st: SubagentStats,
    at: Ms,
    /// The subagents folder's time, and the transcript's size, when counted.
    dir: Ms,
    main: u64,
    gone: bool,
    runs: Option<Box<dyn SubagentRuns>>,
    /// The load dir and main were looked at in (see fresh); none, that it
    /// had no subagents folder then, nor no_sess its session's.
    chk: u64,
    none: bool,
    no_sess: bool,
    tiles: Vec<SubagentTile>,
    /// What it counted: a rewind changes it.
    transcript: String,
}

/// A file's read, kept until its size or time changes (memo.go).
struct FileMemo {
    modified: Option<SystemTime>,
    size: i64,
    v: Option<HostConfig>,
    at: u64,
}

/// A past conversation's row as last made, and what it was made from.
#[derive(PartialEq)]
struct PastIn {
    profile: Profile,
    path: String,
    title: String,
    cwd: String,
    modified: Ms,
    started: Ms,
    name: String,
    group: String,
    repo: String,
    branch: String,
    done: bool,
    recent: bool,
    spend: u64,
    subs: SubagentStats,
}

/// A finished hosted row, kept while nothing about it changes.
struct IdleRow {
    profile: Profile,
    updated: Ms,
    at: Ms,
    sid: String,
    cwd: String,
    state: String,
    sleeping: bool,
    row: Agent,
}

/// Where transcript_of last found a session's transcript, and the load it
/// looked.
#[derive(Clone, PartialEq, Eq, Hash)]
struct FoundKey {
    kind: String,
    name: String,
    dir: String,
    cwd: String,
    sid: String,
}

/// Keeps the cheap caches between readings.
pub struct Loader {
    reg: Registry,
    store: Store,
    hosts: Box<dyn Hosts>,
    procs: Box<dyn ProcSource>,
    gh: Option<Gh>,
    handle: Handle,
    /// The login new sessions run as, by the logins agent's own word.
    using_login: Option<Box<dyn Fn(&rush_state::Config) -> String + Send>>,
    spend: HashMap<String, Spend>,
    /// Counts each agent's spend updates, so an unchanged past row is
    /// reused.
    spend_ver: HashMap<String, u64>,
    nudged: HashMap<String, Ms>,
    git: HashMap<String, GitInfo>,
    /// Folder → main checkout.
    roots: HashMap<String, String>,
    prev_tab: Option<Arc<Table>>,
    /// Command lines and whether a process runs print mode, by pid and
    /// its start.
    args: HashMap<Pid, (SystemTime, String)>,
    print: HashMap<Pid, (SystemTime, bool, bool)>,
    subs: HashMap<String, SubsEntry>,
    /// This load leaves out what can wait: see load_quick.
    quick: bool,
    fetched: HashMap<String, Reading>,
    /// Each login's last reading a minute or more old.
    burns: HashMap<String, Quota>,
    /// The readings every rush process and session shares, and its time
    /// when last read.
    pub usage_path: PathBuf,
    usage_mod: Option<SystemTime>,
    files: HashMap<PathBuf, FileMemo>,
    compact: HashMap<String, (Ms, Compaction)>,
    compact_folder: HashMap<String, (Ms, Compaction)>,
    /// The files and folders subagents and transcript_of looked at this
    /// load: watched, they needn't be looked at again until they change.
    pub(crate) sub_watch: Vec<PathBuf>,
    /// Transcripts found away from where their session started, by id.
    moved: HashMap<String, String>,
    found: HashMap<FoundKey, (String, u64)>,
    /// Sessions whose transcript wasn't anywhere: when to look again, and
    /// how long was waited before.
    unfound: HashMap<String, (Ms, Ms)>,
    envs: HashMap<Pid, (SystemTime, Vec<String>)>,
    pub temp: Arc<TempSizes>,
    past_rows: HashMap<String, (PastIn, Agent)>,
    idle_rows: HashMap<String, IdleRow>,
    past_keys: HashMap<String, String>,
    /// Other agents' past sessions, by profile folder, as last listed.
    others: HashMap<String, (Ms, Vec<Session>)>,
    pub(crate) watching: Option<Watching>,
    /// The sessions that ran agents from their shells: each spawned agent's
    /// row key to its session's, kept across restarts.
    pub(crate) links: HashMap<String, String>,
    pub(crate) links_dirty: bool,
    links_path: Option<PathBuf>,
    /// Headless runs looked for in transcripts.
    asked: HashSet<String>,
    /// Look for askers on this thread (tests), not in the background.
    pub(crate) ask_inline: bool,
    /// Each profile read, with its jobs' ids at the last reading.
    read: Vec<(AdapterRef, Profile, Vec<String>)>,
    /// Each transcripts folder's subfolders, as listed when it was last
    /// modified.
    dirs: RefCell<HashMap<PathBuf, (SystemTime, Vec<PathBuf>)>>,
}

impl Loader {
    /// A loader reading the registry's agents, with no state of rush's
    /// (no names, nothing put away), no rush sessions, the machine's own
    /// processes, and GitHub never asked. Nothing is read yet.
    pub fn new(reg: Registry) -> Loader {
        Loader {
            reg,
            store: Store::default(),
            hosts: Box::new(NoHosts),
            procs: Box::new(System),
            gh: None,
            handle: Handle {
                inbox: Arc::default(),
                skip_past: Arc::default(),
            },
            using_login: None,
            spend: HashMap::new(),
            spend_ver: HashMap::new(),
            nudged: HashMap::new(),
            git: HashMap::new(),
            roots: HashMap::new(),
            prev_tab: None,
            args: HashMap::new(),
            print: HashMap::new(),
            subs: HashMap::new(),
            quick: false,
            fetched: HashMap::new(),
            burns: HashMap::new(),
            usage_path: PathBuf::new(),
            usage_mod: None,
            files: HashMap::new(),
            compact: HashMap::new(),
            compact_folder: HashMap::new(),
            sub_watch: Vec::new(),
            moved: HashMap::new(),
            found: HashMap::new(),
            unfound: HashMap::new(),
            envs: HashMap::new(),
            temp: Arc::new(TempSizes::new()),
            past_rows: HashMap::new(),
            idle_rows: HashMap::new(),
            past_keys: HashMap::new(),
            others: HashMap::new(),
            watching: None,
            links: HashMap::new(),
            links_dirty: false,
            links_path: None,
            asked: HashSet::new(),
            ask_inline: false,
            read: Vec::new(),
            dirs: RefCell::new(HashMap::new()),
        }
    }

    /// The loader rush runs (NewLoader): rush's state and cache folders,
    /// its sessions as their hosts leave them there, the spawn links kept
    /// there, GitHub through gh.
    pub fn from_state(reg: Registry) -> Loader {
        let mut l = Loader::new(reg);
        l.store = Store::load();
        l.hosts = Box::new(crate::hostfiles::InfoFiles::new());
        l.usage_path = rush_state::dir().join("usage.json");
        l.gh = Some(Gh::shared().clone());
        l.set_links_path(rush_state::cache_path("spawns.json"));
        l
    }

    /// Reads rush's config and overlay from s from now on (LoadFrom).
    pub fn set_store(&mut self, s: Store) {
        self.store = s;
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Lists rush's own sessions through h.
    pub fn set_hosts(&mut self, h: Box<dyn Hosts>) {
        self.hosts = h;
    }

    /// Reads processes through p.
    pub fn set_procs(&mut self, p: Box<dyn ProcSource>) {
        self.procs = p;
    }

    /// Asks GitHub about rows' pull requests through gh.
    pub fn set_gh(&mut self, gh: Option<Gh>) {
        self.gh = gh;
    }

    /// Says the login new sessions run as (the logins agent's UsingLogin).
    pub fn set_using_login(&mut self, f: impl Fn(&rush_state::Config) -> String + Send + 'static) {
        self.using_login = Some(Box::new(f));
    }

    /// Keeps which sessions ran which agents at path, across restarts.
    pub fn set_links_path(&mut self, path: PathBuf) {
        self.links = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        self.links_path = Some(path);
    }

    /// What the UI hands in, from any thread.
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Takes spend priced elsewhere, by row key.
    pub fn set_spend(&mut self, m: HashMap<String, Spend>) {
        self.handle.set_spend(m);
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
            by_kind.entry(a.acct.kind.as_str()).or_default().push(t);
        }
        let mut got = HashMap::new();
        for (k, targets) in by_kind {
            let Some(sr) = self
                .reg
                .get(k)
                .and_then(|r| r.spend_reader().map(|s| s.spend_scanner()))
            else {
                continue;
            };
            got.extend(sr.run(&targets));
        }
        let n = got.len();
        // Taken in at once: a scan is the loader's own.
        for (k, v) in got {
            self.spend.insert(k.clone(), v);
            *self.spend_ver.entry(k).or_default() += 1;
        }
        self.changed_since();
        n
    }

    /// A full reading, its processes sampled (Load(true)).
    pub fn load(&mut self, now: Ms) -> Snapshot {
        self.load_with(now, true)
    }

    /// A reading without the process table (Load(false)): no row is taken
    /// as having lost its process, and no machine.
    pub fn load_unsampled(&mut self, now: Ms) -> Snapshot {
        self.load_with(now, false)
    }

    /// A load without reading subagent runs, which means reading every
    /// recent session's whole transcript: rush's first list. The next
    /// load is a whole one.
    pub fn load_quick(&mut self, now: Ms) -> Snapshot {
        self.quick = true;
        let snap = self.load_with(now, true);
        self.quick = false;
        if let Some(w) = &mut self.watching {
            w.stale = true;
        }
        snap
    }

    fn take_in(&mut self) {
        let inb = std::mem::take(&mut *self.handle.inbox.lock().unwrap_or_else(|e| e.into_inner()));
        for (k, v) in inb.spend {
            *self.spend_ver.entry(k.clone()).or_default() += 1;
            self.spend.insert(k, v);
        }
        self.nudged.extend(inb.nudged);
        for (k, u) in inb.fetched {
            self.set_fetched(&k, u);
        }
        for (k, (p, how)) in inb.links {
            self.link(&k, &p, how);
        }
        if inb.stale {
            self.changed_since();
        }
        if inb.settle
            && let Some(w) = &mut self.watching
        {
            w.settle = true;
        }
    }

    /// Lists child with parent ("" for none found); how is how that was
    /// decided, for the log.
    pub(crate) fn link(&mut self, child: &str, parent: &str, how: &str) {
        let old = self.links.get(child);
        if child != parent && old.is_none_or(|o| o != parent) {
            rush_sys::events::log(
                "spawn-linked",
                serde_json::json!({"child": child, "parent": parent, "was": old.cloned().unwrap_or_default(), "how": how}),
            );
            self.links.insert(child.to_string(), parent.to_string());
            self.links_dirty = true;
        }
    }

    fn set_fetched(&mut self, key: &str, u: Reading) {
        if u.fetched_at == 0
            && let Some(old) = self.fetched.get_mut(key)
        {
            old.problem = u.problem; // keep the last good numbers, note why they're not refreshing
            return;
        }
        self.fetched.insert(key.to_string(), u);
    }

    /// The readings shared through usage_path that are newer than those
    /// it has.
    fn sync_usage(&mut self) {
        if self.usage_path.as_os_str().is_empty() {
            return;
        }
        let Ok(m) = std::fs::metadata(&self.usage_path).and_then(|m| m.modified()) else {
            return;
        };
        if self.usage_mod == Some(m) {
            return;
        }
        self.usage_mod = Some(m);
        let Some(n) = self.native() else { return };
        let Some(pr) = n.plan_reader() else { return };
        let all = pr.readings(&self.usage_path.to_string_lossy());
        for (k, v) in all {
            if self
                .fetched
                .get(&k)
                .is_some_and(|o| v.fetched_at <= o.fetched_at)
            {
                continue;
            }
            self.fetched.insert(k, v);
        }
    }

    fn native(&self) -> Option<AdapterRef> {
        self.reg.native_agent()
    }

    /// The home agent's profile: where every one of its sessions runs, by
    /// the name rush's config gives its folder.
    fn home_profile(&self) -> Option<(AdapterRef, Profile, Folder)> {
        let a = self.native()?;
        // Its home (~/.claude), whatever folder this process's environment
        // names: every session runs there (Go's ActiveAccount).
        let mut p = match a.homer() {
            Some(h) => h.home(),
            None => a.profiles().into_iter().next()?,
        };
        let mut f = Folder {
            name: p.name.clone(),
            config_dir: p.dir.clone(),
        };
        for c in &self.store.config.folders {
            if c.config_dir == p.dir && !c.name.is_empty() {
                p.name.clone_from(&c.name);
                f.name.clone_from(&c.name);
            }
        }
        Some((a, p, f))
    }

    /// p's plan: the agent's own record of it, or rush's reading when
    /// that's newer.
    fn read_usage(&self, a: &AdapterRef, p: &Profile, now: Ms) -> Reading {
        let r = a.plan_reader().map(|pr| pr.plan(p)).unwrap_or_default();
        freshest(&self.fetched, &p.dir, r, now)
    }

    fn load_with(&mut self, now: Ms, sample: bool) -> Snapshot {
        self.temp.load();
        if let Some(w) = &mut self.watching {
            w.start();
        }
        self.take_in();
        self.drain(now);
        if sample && let Some(snap) = self.reuse(now) {
            return snap;
        }
        self.began(now);
        let skip_past = self.handle.skip_past.load(Ordering::Relaxed);
        let mut snap = Snapshot {
            at: now,
            ..Snapshot::default()
        };
        let mut tab = sample.then(|| self.procs.snapshot());

        let mut seen: HashSet<String> = HashSet::new();
        let checked = self.checked();
        let hosted = {
            let w = self.watching.as_ref();
            let quick = self.quick;
            let fresh = |p: &Path, at: u64| crate::watch::fresh(w, quick, p, at);
            self.hosts.list(&fresh, checked)
        };
        for info in hosted.iter().filter(|i| i.lost) {
            self.hosts.revive(info); // its next reading shows it working again
        }
        if !self.idle_rows.is_empty() {
            let present: HashSet<&str> = hosted.iter().map(|i| i.id.as_str()).collect();
            self.idle_rows.retain(|id, _| present.contains(id.as_str()));
        }
        // The agent processes rush's own hosts run: they register as
        // sessions too, but they're the rush agents.
        let ours: HashSet<String> = hosted.iter().map(|i| i.session_id.clone()).collect();
        let ours_pid: HashSet<Pid> = hosted
            .iter()
            .filter(|i| i.claude_pid != 0)
            .map(|i| i.claude_pid)
            .collect();
        // Processes of sessions a row stands for: a print-mode run under
        // one of them is its spawn, counted with its subagents.
        let mut parents: HashSet<Pid> = HashSet::new();
        for i in &hosted {
            parents.insert(i.claude_pid);
            parents.insert(i.host_pid);
        }
        let mut spawned: Vec<(String, Pid)> = Vec::new();
        // Conversations a row already stands for: the rest are past ones.
        let mut claimed: HashSet<String> = ours.clone();
        self.branches(&hosted, &mut claimed);
        self.sync_usage();
        let ov = self.store.overlay.clone();
        let mut read = Vec::new();
        let home = self.home_profile();
        let home_kind = home
            .as_ref()
            .map(|(_, p, _)| p.kind.clone())
            .unwrap_or_default();
        if let Some((ad, p, folder)) = &home {
            let (workers, pins, prs) = match ad.job_keeper() {
                Some(k) => (k.workers(p), k.pins(p), k.linked_prs(p)),
                None => Default::default(),
            };
            let mut av = AccountView {
                folder: folder.clone(),
                current: true,
                ..AccountView::default()
            };
            if let Some(j) = ad.joiner() {
                av.daemon = j.service_up(p);
            }
            av.usage = self.read_usage(ad, p, now);
            av.quota = av.usage.quota(&reading_key(&p.dir, &av.usage));
            // Its sessions, as its adapter finds them: background jobs,
            // then every session whose process is alive.
            let live = ad.discoverer().map(|d| d.live(p)).unwrap_or_default();
            let (jobs, sessions): (Vec<Session>, Vec<Session>) =
                live.into_iter().partition(|s| s.job.is_some());
            for ss in &sessions {
                parents.insert(ss.pid);
            }
            let mut by_job: HashMap<&str, &Session> = HashMap::new();
            for ss in &sessions {
                claimed.insert(ss.id.clone());
                if !ss.job_id.is_empty() {
                    by_job.insert(&ss.job_id, ss);
                }
            }
            let mut ids = Vec::new();
            for js in &jobs {
                let Some(j) = js.job.as_deref() else { continue };
                ids.push(j.id.clone());
                let key = rush_state::key(&p.name, &j.id);
                seen.insert(key.clone());
                claimed.insert(j.session_id.clone());
                let mut a = Agent {
                    kind: p.kind.clone(),
                    job: j.clone(),
                    key: key.clone(),
                    acct: p.clone(),
                    display_name: j.name.clone(),
                    ..Agent::default()
                };
                let mut st = js.state.as_str();
                if let Some(ss) = by_job.get(j.id.as_str()) {
                    st = apply_status(&mut a, st, ss);
                }
                if st == "running" {
                    st = "done"; // only a busy session makes it work
                }
                a.job.state = State::from_word(st);
                if let Some(&t) = self.nudged.get(&key) {
                    if now - t > 20 * SECOND || j.updated_at > t && js.state == "working" {
                        self.nudged.remove(&key);
                    } else if !a.job.live() {
                        a.job.state = State::Working;
                        a.job.needs.clear();
                        a.checking = false;
                        a.job.detail.clear();
                    }
                }
                overlay(&ov, &mut a, j.updated_at, true);
                a.pinned = pins.get(&j.id).is_some_and(|&n| n > 0);
                a.worker = workers.get(&j.id).copied();
                // A job's worktree, not its cwd, is where it actually runs.
                a.job.cwd = job_cwd(&j.cwd, &j.worktree_path).to_string();
                (a.repo, a.branch) = git_for(&mut self.git, &a.job.cwd, now);
                if a.branch.is_empty() && !j.worktree_branch.is_empty() {
                    // The agent's record of the branch it made the
                    // worktree on, for before the checkout exists to read.
                    a.branch.clone_from(&j.worktree_branch);
                }
                a.spend = self.spend.get(&key).cloned().unwrap_or_default();
                if !j.transcript_path.is_empty()
                    && (a.job.live() || a.pid != 0 || now - j.updated_at < 24 * HOUR)
                {
                    // Its process isn't always known, so it's never taken
                    // as gone.
                    (a.subs, a.subagents) =
                        self.subagents(&p.kind, &key, &j.transcript_path, false, now);
                }
                for u in &a.spend.prs {
                    // Only PRs its agent linked to a session; a URL merely
                    // mentioned in a transcript is not this agent's PR.
                    if let Some(pr) = prs.get(u) {
                        let mut pr = pr.clone();
                        pr.url.clone_from(u);
                        a.prs.push(pr);
                    }
                }
                if let Some(w) = a.worker {
                    a.pid = w;
                    // A roster left behind by a crashed daemon names pids
                    // that are gone or reused; only a live process of the
                    // agent's program counts.
                    if let Some(t) = &tab
                        && !is_program_pid(&self.reg, t, a.pid, p.kind.as_str())
                    {
                        a.pid = 0;
                        a.worker = None;
                    }
                }
                if tab.is_some() && a.job.live() && a.pid == 0 {
                    a.job.state = State::Stopped;
                    a.job.detail = "lost its process".into();
                }
                self.sample(tab.as_mut(), &mut a);
                count(&mut av, &a);
                snap.agents.push(a);
            }
            for ss in &sessions {
                if !ss.interactive
                    || ss.id.len() < 8
                    || !ss.id.is_char_boundary(8)
                    || tab
                        .as_ref()
                        .is_some_and(|t| !is_program_pid(&self.reg, t, ss.pid, p.kind.as_str()))
                    || ours.contains(&ss.id)
                    || ours_pid.contains(&ss.pid)
                {
                    continue;
                }
                let key = rush_state::key(&p.name, &format!("i:{}", &ss.id[..8]));
                seen.insert(key.clone());
                let working = ss.status == "busy" || ss.status == "shell";
                let transcript = self.transcript_of(p, &ss.cwd, &ss.id, now);
                let (headless, unstored) = self.is_print(tab.as_deref_table(), ss.pid);
                if unstored {
                    continue; // a one-shot print run keeps no transcript to open
                }
                if headless && crate::machine::spawn_of(tab.as_deref_table(), ss.pid, &parents) != 0
                {
                    spawned.push((key, ss.pid));
                    continue;
                }
                let mut a = Agent {
                    kind: p.kind.clone(),
                    key: key.clone(),
                    acct: p.clone(),
                    display_name: ss.name.clone(),
                    interactive: true,
                    headless,
                    pid: ss.pid,
                    ..Agent::default()
                };
                let j = &mut a.job;
                j.id = ss.id[..8].to_string();
                j.account.clone_from(&p.name);
                j.name.clone_from(&ss.name);
                j.state = if working { State::Working } else { State::Idle };
                j.cwd.clone_from(&ss.cwd);
                j.session_id.clone_from(&ss.id);
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
                if Path::new(&ss.cwd) == rush_state::dir().join("advisor") {
                    // rush's own advisor at work (advisor.Dir), not yours.
                    a.advisor = true;
                    a.display_name = "✦ advisor".into();
                    a.job.detail = "looking over your sessions for savings".into();
                    if let Some(pr) = tab.as_ref().and_then(|t| t.procs.get(&ss.pid))
                        && cmdline_of(self.procs.as_ref(), &mut self.args, pr)
                            .contains("--model opus")
                    {
                        a.job.detail = "checking a saving it found".into();
                    }
                }
                overlay(&ov, &mut a, 0, false);
                a.spend = self.spend.get(&key).cloned().unwrap_or_default();
                // ss.cwd is only ever where the session started; its own
                // transcript's cwd says where it last actually worked.
                let mut dir = ss.cwd.clone();
                if !a.spend.dir.is_empty() && self.follows(&dir, &a.spend.dir.clone(), now) {
                    dir.clone_from(&a.spend.dir);
                }
                (a.repo, a.branch) = git_for(&mut self.git, &dir, now);
                a.job.cwd = dir;
                let tp = a.job.transcript_path.clone();
                (a.subs, a.subagents) = self.subagents(&p.kind, &key, &tp, false, now); // listed only while its process runs
                self.sample(tab.as_mut(), &mut a);
                count(&mut av, &a);
                snap.agents.push(a);
            }
            for info in hosted.iter().filter(|i| i.kind == p.kind.as_str()) {
                let a = self.hosted_agent(p, info, tab.as_mut(), now);
                count(&mut av, &a);
                snap.agents.push(a);
            }
            read.push((ad.clone(), p.clone(), ids));
            if !skip_past {
                // Past rows are kept from one reading to the next.
                for a in self.past_agents(ad, p, &claimed, &mut seen, now) {
                    av.agents += 1;
                    av.spend += a.spend.cost;
                    av.today += a.spend.today;
                    snap.agents.push(a);
                }
            }
            snap.accounts.push(av);
        }
        // Other agents' rush sessions belong to no home folder: they're
        // listed under their own profile's name.
        for info in hosted.iter().filter(|i| i.kind != home_kind.as_str()) {
            let p = Profile {
                kind: home_kind.clone(),
                name: info.account.clone(),
                dir: String::new(),
            };
            let a = self.hosted_agent(&p, info, tab.as_mut(), now);
            snap.agents.push(a);
        }
        let others = self.other_agents(
            home_kind.as_str(),
            &mut claimed,
            &mut seen,
            now,
            skip_past,
            &mut read,
        );
        snap.agents.extend(others);
        self.read = read;
        // What each row's subagents were counted under is kept while the
        // row is read, folded into its parent's or not.
        let listed: HashSet<String> = snap.agents.iter().map(|a| a.key.clone()).collect();
        let spawned = self.hosted_spawns(&hosted, &snap.agents, spawned);
        let agents = std::mem::take(&mut snap.agents);
        snap.agents = self.fold_spawns(tab.as_deref_table(), agents, spawned, &mut parents);
        if !ov.hidden.is_empty() {
            snap.agents.retain(|a| !ov.hidden.contains_key(&a.key));
        }
        if let Some(root) = snap.accounts.first() {
            let using = self
                .using_login
                .as_ref()
                .map(|f| f(&self.store.config))
                .unwrap_or_default();
            snap.logins = logins(
                &self.store.config,
                root,
                &using,
                &self.fetched,
                &mut self.burns,
                now,
            );
        }
        if let Some(mut t) = tab.take() {
            snap.machine = self.machine_of(&mut t, &snap.agents);
            let t = Arc::new(t);
            self.prev_tab = Some(t.clone());
            snap.table = Some(t);
        }
        self.temp.save();
        self.save_links();
        let gh = self.gh.clone();
        for a in &mut snap.agents {
            a.temp = self.temp.bytes(&a.key);
            if a.temp > 0 {
                a.scratch = temp_dirs(&self.reg, a);
            }
            if !a.repo.is_empty() {
                let m = main_checkout_in(&a.repo, &mut self.roots);
                a.root = if m.is_empty() { a.repo.clone() } else { m };
            }
            crate::pr::find_pr(a, gh.as_ref());
        }
        self.compactions(&mut snap.agents, &hosted, now);
        self.subs.retain(|k, _| listed.contains(k)); // a row no longer listed: its runs' reader goes
        snap.agents.sort_by_key(|a| a.age(now));
        if sample {
            self.keep(&snap);
        }
        snap.hosted = hosted;
        snap
    }

    /// Forgets the command lines and print modes of processes gone.
    fn forget_gone(&mut self, tab: &Table) {
        self.args.retain(|pid, _| tab.procs.contains_key(pid));
        self.print.retain(|pid, _| tab.procs.contains_key(pid));
    }

    /// An agent with a process: its tree's memory, CPU and process count.
    pub(crate) fn sample(&self, tab: Option<&mut Table>, a: &mut Agent) {
        let Some(tab) = tab else { return };
        if a.pid == 0 {
            return;
        }
        tab.fill(self.prev_tab.as_deref(), &[a.pid]);
        let (mem, cpu, n) = tab.sum(a.pid, &HashSet::<Pid>::new());
        (a.mem, a.cpu, a.procs) = (mem, cpu as f32, n as u32);
    }

    /// Whether pid runs print mode, reading its arguments once a process.
    fn is_print(&mut self, tab: Option<&Table>, pid: Pid) -> (bool, bool) {
        let start = tab.and_then(|t| t.procs.get(&pid)).map(|p| p.start);
        if let Some(s) = start
            && let Some(&(st, p, u)) = self.print.get(&pid)
            && st == s
        {
            return (p, u);
        }
        let r = is_print(&self.procs.args(pid));
        if let Some(s) = start {
            self.print.insert(pid, (s, r.0, r.1));
        }
        r
    }

    /// Whether a row in cwd goes where its transcript last worked, dir: a
    /// checkout, or a folder above its own.
    fn follows(&mut self, cwd: &str, dir: &str, now: Ms) -> bool {
        if !git_for(&mut self.git, dir, now).0.is_empty() {
            return true;
        }
        cwd.is_empty() || (cwd.starts_with(dir) && cwd.as_bytes().get(dir.len()) == Some(&b'/'))
    }

    /// Where a session's conversation is now: under the folder it started
    /// in, or, once entering a worktree has moved it, wherever it was
    /// found, remembered so it's looked for only once.
    fn transcript_of(&mut self, p: &Profile, cwd: &str, sid: &str, _now: Ms) -> String {
        let k = FoundKey {
            kind: p.kind.0.clone(),
            name: p.name.clone(),
            dir: p.dir.clone(),
            cwd: cwd.to_string(),
            sid: sid.to_string(),
        };
        if let Some((path, at)) = self.found.get(&k)
            && self.fresh(Path::new(path), *at)
        {
            let path = path.clone();
            self.sub_watch.push(PathBuf::from(&path));
            return path;
        }
        if let Some(found) = self.moved.get(sid).cloned()
            && Path::new(&found).exists()
        {
            self.found_at(k, &found);
            return found;
        }
        let at = self.reg.transcript_path(p.kind.as_str(), p, cwd, sid);
        if !at.is_empty() && Path::new(&at).exists() {
            self.moved.remove(sid);
            self.unfound.remove(sid);
            self.found_at(k, &at);
            return at;
        }
        self.found.remove(&k);
        let t = clock_now();
        let (next, wait) = self.unfound.get(sid).copied().unwrap_or((0, 0));
        if next != 0 && t < next {
            return at;
        }
        let Some(tr) = self.reg.get(p.kind.as_str()) else {
            return at;
        };
        let Some(ts) = tr.transcripts() else {
            return at;
        };
        let found = ts.find_transcript(p, cwd, sid);
        if !found.is_empty() && found != at {
            self.moved.insert(sid.to_string(), found.clone());
            self.unfound.remove(sid);
            return found;
        }
        self.moved.remove(sid);
        // Each look globs every project's folder: one still not found is
        // looked for half as often, down to every ten minutes.
        let wait = (2 * wait).clamp(10 * SECOND, 10 * MINUTE);
        self.unfound.insert(sid.to_string(), (t + wait, wait));
        if found.is_empty() { at } else { found }
    }

    /// Keeps path, just seen there, as k's transcript, watched from now.
    fn found_at(&mut self, k: FoundKey, path: &str) {
        let c = self.checked();
        self.found.insert(k, (path.to_string(), c));
        self.sub_watch.push(PathBuf::from(path));
    }

    /// Counts an agent's subagent runs, and those still working. Whether
    /// one is working is read from the transcripts: its call without a
    /// result, or a background launch not yet reported done, unless gone
    /// says the session's process is known to have exited. It's counted
    /// again when a run started (the folder changed), when the transcript
    /// grew, when some were working, or after 30s.
    fn subagents(
        &mut self,
        k: &rush_core::adapter::Kind,
        key: &str,
        transcript: &str,
        gone: bool,
        now: Ms,
    ) -> (SubagentStats, Vec<SubagentTile>) {
        let Some(ad) = self.reg.get(k.as_str()) else {
            return Default::default();
        };
        let Some(f) = ad.run_follower() else {
            return Default::default();
        };
        if transcript.is_empty() {
            return Default::default();
        }
        let sub = PathBuf::from(f.subagents_dir(transcript));
        let sess = sub.parent().map(Path::to_path_buf).unwrap_or_default();
        let proj = Path::new(transcript)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let mut e = self.subs.remove(key).unwrap_or_default();
        let mut ok = !e.transcript.is_empty();
        if ok && e.transcript != transcript {
            e = SubsEntry::default(); // another conversation: counted afresh
            ok = false;
        }
        // Watched and quiet since they were looked at, the folder and the
        // transcript are as they were.
        if ok && e.none && self.fresh(&proj, e.chk) && (e.no_sess || self.fresh(&sess, e.chk)) {
            self.sub_watch.push(sess);
            self.sub_watch.push(proj);
            self.subs.insert(key.to_string(), e);
            return Default::default();
        }
        let (dir, main);
        if ok && !e.none && self.fresh(&sub, e.chk) && self.fresh(Path::new(transcript), e.chk) {
            (dir, main) = (e.dir, e.main);
        } else {
            let chk = self.checked();
            let Some(d) = mtime(&sub) else {
                let no_sess = !sess.exists();
                self.subs.insert(
                    key.to_string(),
                    SubsEntry {
                        chk,
                        none: true,
                        no_sess,
                        transcript: transcript.to_string(),
                        ..SubsEntry::default()
                    },
                );
                self.sub_watch.push(sess);
                self.sub_watch.push(proj);
                return Default::default();
            };
            dir = d;
            main = std::fs::metadata(transcript).map_or(0, |m| m.len());
            if ok && e.none {
                e = SubsEntry::default(); // a folder now: counted afresh
                ok = false;
            }
            e.chk = chk;
            e.transcript = transcript.to_string();
        }
        self.sub_watch.push(sub);
        self.sub_watch.push(PathBuf::from(transcript));
        if self.quick {
            let r = if !ok || e.runs.is_none() {
                Default::default()
            } else {
                (e.st, e.tiles.clone())
            };
            self.subs.insert(key.to_string(), e);
            return r;
        }
        let working = e.st.direct + e.st.nested > 0;
        if ok
            && e.runs.is_some()
            && e.dir == dir
            && e.main == main
            && e.gone == gone
            && now - e.at < 30 * SECOND
            && (!working || now - e.at < 3 * SECOND)
        {
            let r = (e.st, e.tiles.clone());
            self.subs.insert(key.to_string(), e);
            return r;
        }
        if e.runs.is_none() {
            e.runs = f.subagent_runs();
        }
        match &mut e.runs {
            Some(runs) => {
                runs.set_gone(gone);
                e.st = runs.stats(transcript, now);
                e.tiles = subagent_tiles(transcript, &runs.running());
            }
            None => {
                // An adapter that only counts its subagents.
                e.st = f.count_subagents(transcript, now);
                e.tiles.clear();
            }
        }
        (e.at, e.dir, e.main, e.gone) = (now, dir, main, gone);
        let r = (e.st, e.tiles.clone());
        self.subs.insert(key.to_string(), e);
        r
    }

    /// A rush session's row, with what you've set on it.
    fn hosted_agent(
        &mut self,
        p: &Profile,
        info: &HostInfo,
        tab: Option<&mut Table>,
        now: Ms,
    ) -> Agent {
        let mut a = self.hosted_base(p, info, tab, now);
        let ov = self.store.overlay.clone();
        overlay(&ov, &mut a, info.updated_at, true);
        a.spend = self.spend.get(&a.key).cloned().unwrap_or_default();
        // The host follows its agent's own cwd, which a Bash cd never
        // moves; the transcript sees the cd. Whichever moved last wins.
        if !a.spend.dir.is_empty() {
            let d = a.spend.dir.clone();
            let cwd = a.job.cwd.clone();
            if cwd.is_empty()
                || d != cwd && a.spend.dir_at > info.cwd_at && self.follows(&cwd, &d, now)
            {
                a.job.cwd = d;
                (a.repo, a.branch) = git_for(&mut self.git, &a.job.cwd, now);
            }
        }
        // A transcript is priced call by call; the host's own figure is
        // only for agents that leave none.
        if !a.spend.ready && a.spend.cost < info.cost_usd {
            a.spend.cost = info.cost_usd;
        }
        a
    }

    /// Finished hosted rows are often hundreds: one is made again only
    /// when its host says something new, or after PAST_EVERY.
    fn hosted_base(
        &mut self,
        p: &Profile,
        info: &HostInfo,
        tab: Option<&mut Table>,
        now: Ms,
    ) -> Agent {
        let idle = info.state == "stopped" || info.sleeping;
        if idle
            && !self.quick
            && let Some(e) = self.idle_rows.get(&info.id)
            && e.profile == *p
            && e.updated == info.updated_at
            && e.sid == info.session_id
            && e.cwd == info.cwd
            && e.state == info.state
            && e.sleeping == info.sleeping
            && now - e.at < PAST_EVERY
        {
            return e.row.clone();
        }
        let watched = self.sub_watch.len();
        let mut a = self.hosted(p, info, tab, now);
        if !a.job.transcript_path.is_empty() {
            let gone =
                a.pid == 0 || info.proto >= 3 && info.claude_pid == 0 && info.state != "working";
            let (k, key, tp) = (
                a.acct.kind.clone(),
                a.key.clone(),
                a.job.transcript_path.clone(),
            );
            (a.subs, a.subagents) = self.subagents(&k, &key, &tp, gone, now);
        }
        if idle {
            // Read again on its own clock, an idle row's files aren't
            // watched.
            self.sub_watch.truncate(watched);
        }
        if idle && !self.quick {
            self.idle_rows.insert(
                info.id.clone(),
                IdleRow {
                    profile: p.clone(),
                    updated: info.updated_at,
                    at: now,
                    sid: info.session_id.clone(),
                    cwd: info.cwd.clone(),
                    state: info.state.clone(),
                    sleeping: info.sleeping,
                    row: a.clone(),
                },
            );
        } else {
            self.idle_rows.remove(&info.id);
        }
        a
    }

    /// A rush session's info as a row.
    fn hosted(&mut self, p: &Profile, info: &HostInfo, tab: Option<&mut Table>, now: Ms) -> Agent {
        let mut st = match info.state.as_str() {
            "idle" | "starting" => "done",
            "" => "stopped",
            s => s,
        };
        let name = if !info.name.is_empty() {
            info.name.clone()
        } else if !info.detail.is_empty() {
            info.detail.clone()
        } else {
            format!("rush session {}", info.id)
        };
        let mut j = Job {
            id: info.id.clone(),
            account: p.name.clone(),
            name: name.clone(),
            detail: info.detail.clone(),
            needs: info.needs.clone(),
            cwd: info.cwd.clone(),
            session_id: info.session_id.clone(),
            created_at: info.started_at,
            updated_at: info.updated_at,
            ..Job::default()
        };
        if info.idle_since != 0 {
            j.updated_at = info.idle_since; // a restart while it waits isn't a finish
        }
        if let Some(d) = &info.decisions
            && st == "done"
        {
            // Its turn ended asking you to decide: it waits on you, as a
            // question would.
            st = "blocked";
            if j.needs.is_empty() {
                j.needs = needs_decisions(d);
            }
        }
        if info.kind == p.kind.as_str() {
            j.transcript_path = self.transcript_of(p, &info.cwd, &info.session_id, now);
        }
        // What it runs in the background, as the agent's own background
        // sessions record theirs, so the list says so alike.
        for t in &info.background {
            let kind = match t.kind.as_str() {
                "local_agent" | "remote_agent" | "in_process_teammate" => {
                    j.subagents += 1;
                    "agent"
                }
                "monitor_mcp" | "monitor_ws" => "monitor",
                "local_bash" => "shell",
                _ => "task",
            };
            j.running.push(Task {
                kind: kind.into(),
                label: t.label.clone(),
                started_at: t.started_at,
            });
            j.background.push(format!("{kind}\x00{}", t.label));
            j.in_flight += 1;
        }
        if let Some(l) = &info.limit {
            j.detail = "usage limit".into();
            if l.resets_at != 0 {
                j.detail += &format!(" · resets {}", clock(l.resets_at));
            }
            if l.ask {
                st = "blocked";
                j.needs = "continue when the limit resets?".into();
            }
        } else if let Some(r) = info.retry.as_ref().filter(|r| r.gave_up) {
            j.detail = format!("API error · {}", r.why);
        } else if info.retry.as_ref().is_some_and(|r| r.offline) {
            j.detail = "offline · continues when the network is back".into();
        } else if info.retry.as_ref().is_some_and(|r| r.proof) {
            j.detail = "API error · continues once the connection holds".into();
        } else if let Some(r) = &info.retry {
            j.detail = format!("API error · retry {} of {}", r.attempt, r.max);
        } else if let Some(aw) = info.away.as_ref().filter(|_| st == "done") {
            j.detail = away_detail(aw, &info.detail, now);
        } else if !info.error.is_empty() && st == "done" {
            j.detail = "stopped mid-turn · your next message resumes it".into();
        } else if info.lost {
            j.detail = "its host went away mid-turn · bringing it back".into();
        } else if info.stalled != 0 && info.state == "working" {
            j.detail = format!(
                "stalled · nothing from the model for {}m",
                (now - info.stalled) / MINUTE
            );
        }
        j.state = State::from_word(st);
        let blocked = j.state == State::Blocked;
        let mut a = Agent {
            key: rush_state::key(&p.name, &format!("a:{}", info.id)),
            acct: Profile {
                kind: rush_core::adapter::Kind::from(info.kind.as_str()),
                name: p.name.clone(),
                dir: p.dir.clone(),
            },
            kind: rush_core::adapter::Kind::from(info.kind.as_str()),
            display_name: name,
            rush: true,
            profile: info.profile.clone(),
            peer: info.remote.clone(),
            decides: blocked && info.decisions.is_some(),
            job: j,
            ..Agent::default()
        };
        if info.state == "working" {
            a.stalled = info.stalled;
        }
        if a.decides
            && let Some(d) = &info.decisions
        {
            for h in d {
                a.decisions_key += h;
                a.decisions_key.push('\x1f');
            }
        }
        // Idle before its first turn, a new agent hasn't finished anything:
        // not your turn. Nor is one away or looping.
        let done = a.job.state == State::Done;
        a.seen = done && info.idle_since == 0 || done && info.away.as_ref().is_some_and(|w| w.on());
        // A sleeping host has gone; its pid is the one it had.
        if info.state != "stopped"
            && !info.sleeping
            && info.host_pid > 0
            && tab
                .as_ref()
                .is_none_or(|t| t.procs.contains_key(&info.host_pid))
        {
            a.pid = info.host_pid;
        }
        if !info.is_remote() {
            // Another machine's: no folder or process here is its.
            a.workspace.clone_from(&info.workspace);
            (a.repo, a.branch) = git_for(&mut self.git, &info.cwd, now);
            a.left.clone_from(&info.left);
            self.sample(tab, &mut a);
        }
        a
    }

    /// The conversations rush sessions left behind when rewound: each
    /// session shows them itself.
    fn branches(&mut self, hosted: &[HostInfo], claimed: &mut HashSet<String>) {
        for info in hosted {
            if let Some(cfg) = self.config(info) {
                claimed.extend(cfg.branches);
            }
        }
    }

    /// The config a session was started with, read again only when it's
    /// changed.
    fn config(&mut self, info: &HostInfo) -> Option<HostConfig> {
        let path = host_root().join(&info.id).join("config.json");
        if let Some(e) = self.files.get(&path)
            && (info.state == "stopped"
                || self.fresh(path.parent().unwrap_or(Path::new("/")), e.at))
        {
            // Only its running host writes it, by renaming it in, which
            // its watched folder tells.
            return e.v.clone();
        }
        let (modified, size) = match std::fs::metadata(&path) {
            Ok(m) => (m.modified().ok(), m.len() as i64),
            Err(_) => (None, -1),
        };
        let at = self.checked();
        if let Some(e) = self.files.get_mut(&path)
            && e.size == size
            && e.modified == modified
        {
            e.at = at;
            return e.v.clone();
        }
        let v = self.hosts.config(&info.id);
        self.files.insert(
            path,
            FileMemo {
                modified,
                size,
                v: v.clone(),
                at,
            },
        );
        v
    }

    /// An account's conversations nothing has open: a terminal session
    /// that has closed keeps its row, and ones from before rush ran are
    /// there too.
    fn past_agents(
        &mut self,
        ad: &AdapterRef,
        p: &Profile,
        claimed: &HashSet<String>,
        seen: &mut HashSet<String>,
        now: Ms,
    ) -> Vec<Agent> {
        let Some(d) = ad.discoverer() else {
            return Vec::new();
        };
        let ov = self.store.overlay.clone();
        let mut out = Vec::new();
        for s in d.past(p) {
            let sid = &s.id;
            if sid.len() < 8 || !sid.is_char_boundary(8) || claimed.contains(sid) {
                continue;
            }
            let key = self
                .past_keys
                .entry(sid.clone())
                .or_insert_with(|| rush_state::key(&p.name, &format!("i:{}", &sid[..8])))
                .clone();
            if !seen.insert(key.clone()) {
                continue;
            }
            let spend_dir = self
                .spend
                .get(&key)
                .map(|s| s.dir.clone())
                .unwrap_or_default();
            let dir = if spend_dir.is_empty() {
                s.cwd.clone()
            } else {
                spend_dir
            };
            let (repo, branch) = git_for(&mut self.git, &dir, now);
            let recent = now - s.updated_at < 24 * HOUR;
            let mut inp = PastIn {
                profile: p.clone(),
                path: s.transcript.clone(),
                title: s.name.clone(),
                cwd: s.cwd.clone(),
                modified: s.updated_at,
                started: s.created_at,
                name: ov.names.get(&key).cloned().unwrap_or_default(),
                group: ov.groups.get(&key).cloned().unwrap_or_default(),
                repo,
                branch,
                done: ov.done.contains_key(&key),
                recent,
                spend: self.spend_ver.get(&key).copied().unwrap_or(0),
                subs: SubagentStats::default(),
            };
            if recent {
                inp.subs = self.past_subagents(ad, &key, &s.transcript, now);
            }
            // Most rows are just as they were a second ago: the same row
            // does.
            if let Some((was, a)) = self.past_rows.get(&key)
                && *was == inp
            {
                out.push(a.clone());
                continue;
            }
            let mut a = Agent {
                kind: p.kind.clone(),
                key: key.clone(),
                acct: p.clone(),
                display_name: if inp.name.is_empty() {
                    s.name.clone()
                } else {
                    inp.name.clone()
                },
                past: true,
                done: inp.done,
                group: inp.group.clone(),
                repo: inp.repo.clone(),
                branch: inp.branch.clone(),
                subs: inp.subs,
                ..Agent::default()
            };
            let j = &mut a.job;
            j.id = sid[..8].to_string();
            j.account.clone_from(&p.name);
            j.name.clone_from(&s.name);
            j.state = State::Stopped;
            j.cwd.clone_from(&s.cwd);
            j.session_id.clone_from(sid);
            j.created_at = s.created_at;
            j.updated_at = s.updated_at;
            j.transcript_path.clone_from(&s.transcript);
            a.spend = self.spend.get(&key).cloned().unwrap_or_default();
            self.past_rows.insert(key, (inp, a.clone()));
            out.push(a);
        }
        out
    }

    /// Subagents for a conversation nothing has open: none can start, so
    /// the folder is looked at again only as often as the listing, and its
    /// transcript isn't read for which are working.
    fn past_subagents(
        &mut self,
        ad: &AdapterRef,
        key: &str,
        transcript: &str,
        now: Ms,
    ) -> SubagentStats {
        if let Some(e) = self.subs.get(key)
            && e.st.direct + e.st.nested == 0
            && now - e.at < PAST_EVERY
        {
            return e.st;
        }
        let Some(f) = ad.run_follower() else {
            return SubagentStats::default();
        };
        if transcript.is_empty() {
            return SubagentStats::default();
        }
        let st = f.count_subagents(transcript, now);
        self.subs.insert(
            key.to_string(),
            SubsEntry {
                st,
                at: now,
                ..SubsEntry::default()
            },
        );
        st
    }

    /// The sessions of every agent that can list them but home's: running
    /// ones (a codex in a terminal), which rush can only show, and past
    /// ones, unless skip_past, which a message resumes in rush mode.
    #[allow(clippy::too_many_arguments)]
    fn other_agents(
        &mut self,
        home: &str,
        claimed: &mut HashSet<String>,
        seen: &mut HashSet<String>,
        now: Ms,
        skip_past: bool,
        read: &mut Vec<(AdapterRef, Profile, Vec<String>)>,
    ) -> Vec<Agent> {
        let mut out = Vec::new();
        for a in self.reg.all().iter() {
            let Some(d) = a.discoverer() else { continue };
            if a.kind().as_str() == home {
                continue;
            }
            for p in a.profiles() {
                for s in d.live(&p) {
                    if s.id.len() < 8 || !s.id.is_char_boundary(8) || claimed.contains(&s.id) {
                        continue;
                    }
                    claimed.insert(s.id.clone());
                    let key = rush_state::key(&p.name, &format!("i:{}", &s.id[..8]));
                    seen.insert(key.clone());
                    let mut ag = self.other_row(a, &p, &s, &key, now);
                    ag.job.state = State::from_word(&s.state);
                    ag.interactive = true;
                    out.push(ag);
                }
                read.push((a.clone(), p.clone(), Vec::new()));
                if skip_past {
                    continue;
                }
                let listing = match self.others.get(&p.dir) {
                    Some((at, l)) if now - at < OTHERS_EVERY => l.clone(),
                    _ => {
                        let l = d.past(&p);
                        self.others.insert(p.dir.clone(), (now, l.clone()));
                        l
                    }
                };
                for s in &listing {
                    if s.id.len() < 8 || !s.id.is_char_boundary(8) || claimed.contains(&s.id) {
                        continue;
                    }
                    let key = rush_state::key(&p.name, &format!("i:{}", &s.id[..8]));
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let mut ag = self.other_row(a, &p, s, &key, now);
                    // One on the agent's servers stays there: rush only
                    // shows it.
                    ag.past = !s.remote;
                    ag.interactive = s.remote;
                    if s.remote {
                        ag.job.state = State::from_word(&s.state);
                    }
                    out.push(ag);
                }
            }
        }
        out
    }

    /// Another agent's session as a row, with what you've set on it.
    fn other_row(&mut self, a: &AdapterRef, p: &Profile, s: &Session, key: &str, now: Ms) -> Agent {
        let mut ag = Agent {
            key: key.to_string(),
            acct: p.clone(),
            display_name: s.name.clone(),
            kind: a.kind(),
            history: s.transcript.clone(),
            remote: s.remote,
            prs: s.prs.clone(),
            headless: s.headless,
            ..Agent::default()
        };
        let j = &mut ag.job;
        j.id = s.id[..8].to_string();
        j.account.clone_from(&p.name);
        j.name.clone_from(&s.name);
        j.state = State::Stopped;
        j.cwd.clone_from(&s.cwd);
        j.session_id.clone_from(&s.id);
        j.created_at = s.created_at;
        j.updated_at = s.updated_at;
        j.detail.clone_from(&s.detail);
        j.needs.clone_from(&s.needs);
        let ov = self.store.overlay.clone();
        overlay(&ov, &mut ag, 0, false);
        if s.remote {
            ag.repo.clone_from(&s.repo);
        } else {
            (ag.repo, ag.branch) = git_for(&mut self.git, &s.cwd, now);
        }
        ag
    }

    /// Fills in each row's compaction.
    fn compactions(&mut self, agents: &mut [Agent], hosted: &[HostInfo], now: Ms) {
        let by_id: HashMap<&str, &HostInfo> = hosted.iter().map(|i| (i.id.as_str(), i)).collect();
        let mut listed = HashSet::new();
        for a in agents.iter_mut() {
            listed.insert(a.key.clone());
            if let Some(&(at, c)) = self.compact.get(&a.key)
                && now - at < COMPACT_EVERY
            {
                a.compaction = c;
                continue;
            }
            let info = if a.rush {
                by_id.get(a.job.id.as_str()).copied()
            } else {
                None
            };
            // Rows with no process of their own all read rush's
            // environment: those in one folder on one profile compact
            // alike.
            let folder = if info.is_none() && a.pid == 0 {
                format!("{}\x00{}\x00{}", a.kind.as_str(), a.acct.dir, a.job.cwd)
            } else {
                String::new()
            };
            match self.compact_folder.get(&folder) {
                Some(&(at, c)) if !folder.is_empty() && now - at < COMPACT_EVERY => {
                    a.compaction = c;
                }
                _ => {
                    let (p, env) = self.session_env(a, info);
                    a.compaction = self
                        .reg
                        .compaction_of(a.kind.as_str(), &p, &a.job.cwd, &env);
                    if !folder.is_empty() {
                        self.compact_folder.insert(folder, (now, a.compaction));
                    }
                }
            }
            self.compact.insert(a.key.clone(), (now, a.compaction));
        }
        self.compact.retain(|k, _| listed.contains(k));
        if let Some(t) = self.prev_tab.clone() {
            self.envs
                .retain(|pid, (start, _)| t.procs.get(pid).is_some_and(|p| p.start == *start));
        }
        self.compact_folder
            .retain(|_, (at, _)| now - *at < COMPACT_EVERY);
    }

    /// The profile a's session runs on and the environment its agent's
    /// process has: read off the process while it runs; for a rush session
    /// asleep, its host's with what rush starts it with on top; else
    /// rush's own.
    fn session_env(&mut self, a: &Agent, info: Option<&HostInfo>) -> (Profile, Vec<String>) {
        let mut p = a.acct.clone();
        let Some(info) = info else {
            if a.pid != 0
                && let Some(env) = self.env_of(a.pid)
            {
                return (p, env);
            }
            return (p, own_env());
        };
        if info.claude_pid != 0
            && let Some(env) = self.env_of(info.claude_pid)
        {
            return (p, env);
        }
        let mut env = own_env();
        if info.host_pid != 0
            && let Some(e) = self.env_of(info.host_pid)
        {
            env = e;
        }
        if let Some(cfg) = self.config(info) {
            if !cfg.account.dir.is_empty() {
                p = cfg.account;
            }
            env.extend(cfg.env);
        }
        (p, env)
    }

    /// pid's environment, read once while it's the same process.
    fn env_of(&mut self, pid: Pid) -> Option<Vec<String>> {
        let start = self
            .prev_tab
            .as_ref()
            .and_then(|t| t.procs.get(&pid))
            .map(|p| p.start);
        let Some(start) = start.filter(|s| *s != SystemTime::UNIX_EPOCH) else {
            let env = self.procs.env(pid);
            return (!env.is_empty()).then_some(env);
        };
        if let Some((s, env)) = self.envs.get(&pid)
            && *s == start
        {
            return Some(env.clone());
        }
        let env = self.procs.env(pid);
        if env.is_empty() {
            return None;
        }
        self.envs.insert(pid, (start, env.clone()));
        Some(env)
    }

    /// Writes the links if they changed.
    /// ponytail: never pruned; an entry per spawned agent, a few dozen bytes.
    fn save_links(&mut self) {
        if !self.links_dirty {
            return;
        }
        if let Some(path) = &self.links_path
            && let Ok(b) = serde_json::to_vec(&self.links)
        {
            let _ = rush_state::write_bytes(path, &b);
        }
        self.links_dirty = false;
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
    /// agent's JobKeeper names for the jobs last read, each one's
    /// transcripts folder with the project folders in it, rush's state
    /// folder and its sessions'.
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

/// A process's command line, kept by its pid and start.
fn cmdline_of(
    procs: &dyn ProcSource,
    args: &mut HashMap<Pid, (SystemTime, String)>,
    p: &Proc,
) -> String {
    if let Some((s, c)) = args.get(&p.pid)
        && *s == p.start
    {
        return c.clone();
    }
    let mut c = procs.args(p.pid).join(" ");
    if c.is_empty() {
        c = p.comm.to_string();
    }
    args.insert(p.pid, (p.start, c.clone()));
    c
}

/// rush's own environment, which a session it starts inherits.
fn own_env() -> Vec<String> {
    std::env::vars_os()
        .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
        .collect()
}

/// Counts a row into its account's view.
fn count(av: &mut AccountView, a: &Agent) {
    if a.job.live() {
        av.live += 1;
    }
    av.agents += 1;
    av.spend += a.spend.cost;
    av.today += a.spend.today;
}

/// What you've set on a row in rush's state: its name, put away, its
/// group, and (with seen) whether you've seen it since it last changed.
fn overlay(ov: &rush_state::Overlay, a: &mut Agent, updated: Ms, seen: bool) {
    if let Some(n) = ov.names.get(&a.key).filter(|n| !n.is_empty()) {
        a.display_name.clone_from(n);
    }
    a.done = ov.done.contains_key(&a.key);
    a.group = ov.groups.get(&a.key).cloned().unwrap_or_default();
    if seen
        && let Some(t) = ov.seen.get(&a.key)
        && updated <= t.to_ms()
    {
        a.seen = true;
    }
}

/// The runs a SubagentRuns calls still working, as tiles: oldest first,
/// as they began, each with the checkout it worked in when not its
/// session's.
fn subagent_tiles(transcript: &str, runs: &[rush_core::adapter::SubagentRun]) -> Vec<SubagentTile> {
    if runs.is_empty() {
        return Vec::new();
    }
    let paths: Vec<&str> = runs.iter().map(|r| r.path.as_str()).collect();
    let wts = sub_worktrees(&paths, &transcript_cwd(transcript));
    let mut out: Vec<SubagentTile> = runs
        .iter()
        .map(|r| SubagentTile {
            id: r.id.clone(),
            kind: r.kind.clone(),
            description: r.description.clone(),
            path: r.path.clone(),
            worktree: wts.get(&r.path).cloned().unwrap_or_default(),
            tool_use_id: r.tool_use_id.clone(),
            mod_time: r.mod_time,
            born: r.born,
        })
        .collect();
    // Oldest first, as they began: a new one joins the end.
    out.sort_by(|a, b| a.born.cmp(&b.born).then_with(|| a.id.cmp(&b.id)));
    out
}

fn mtime(p: &Path) -> Option<Ms> {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .map(crate::temp::ms_of)
}

/// A table as an optional reference.
trait AsTable {
    fn as_deref_table(&self) -> Option<&Table>;
}

impl AsTable for Option<Table> {
    fn as_deref_table(&self) -> Option<&Table> {
        self.as_ref()
    }
}

/// Trusts the session's live busy/idle flag over the job file, whose
/// state the agent only re-summarises every 15-40s. Returns the state.
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
/// inside it.
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

/// Whether an agent's command line runs it non-interactively, and whether
/// it keeps no session on disk.
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

/// A process table's procs, sorted: for tests.
#[cfg(test)]
pub(crate) fn table(procs: Vec<Proc>) -> Table {
    let mut t = Table {
        at: std::time::Instant::now(),
        procs: Default::default(),
        children: Default::default(),
    };
    for p in procs {
        if p.ppid != p.pid {
            t.children.entry(p.ppid).or_default().push(p.pid);
        }
        t.procs.insert(p.pid, p);
    }
    t
}

impl Loader {
    pub(crate) fn quick_now(&self) -> bool {
        self.quick
    }

    pub(crate) fn snapshot_procs(&mut self) -> Table {
        self.procs.snapshot()
    }

    pub(crate) fn set_prev_tab(&mut self, t: Arc<Table>) {
        self.prev_tab = Some(t);
    }

    /// The machine's processes the view lists, from tab.
    pub(crate) fn machine_of(&mut self, tab: &mut Table, agents: &[Agent]) -> Machine {
        self.forget_gone(tab);
        let home = self.native().map(|a| a.kind()).unwrap_or_default();
        let prev = self.prev_tab.clone();
        let procs = &self.procs;
        let args = &mut self.args;
        let mut cmdline = |p: &Proc| cmdline_of(procs.as_ref(), args, p);
        machine(
            &self.reg,
            home.as_str(),
            tab,
            prev.as_deref(),
            agents,
            &mut cmdline,
        )
    }

    /// Finds the process of an agent's that started nearest a time.
    pub(crate) fn prog_procs<'a>(&'a self, tab: Option<&'a Table>) -> ProgProcs<'a> {
        ProgProcs::new(&self.reg, tab)
    }

    pub(crate) fn asked_before(&self, key: &str) -> bool {
        self.asked.contains(key)
    }

    pub(crate) fn mark_asked(&mut self, key: &str) {
        self.asked.insert(key.to_string());
    }

    pub(crate) fn found_paths(&self) -> Vec<String> {
        self.found.values().map(|(p, _)| p.clone()).collect()
    }

    pub(crate) fn forget_found(&mut self, gone: &[String]) {
        if !gone.is_empty() {
            self.found.retain(|_, (p, _)| !gone.contains(p));
        }
    }
}
