//! How the Session follows the runs its session started: its subagents
//! (listed beside its transcript), a workflow's agents, the subagents an
//! agent only says it started (a task with no transcript), and what its
//! background tasks write. All of it is read off the UI thread, a read at
//! a time, and taken in whole; a frame draws what was last read. After
//! Go's internal/ui/rushmode.go (refreshSubs :68, onPane :232,
//! followedSubs :277, readUnread :291, subStateIn :512, newRuns :597,
//! runningSubs :717, dockSubs :620, noteDone :646, doneShown :663,
//! keepRunPick :1251, cycleSub :745, openSub :789, watchRun :809,
//! closeSub :825), jobsubs.go (jobSubs :22, withJobSubs :45, jobSubState
//! :51), jobs.go (jobOwner :74, madeBy :100, looseJobs :134, jobsOf :146,
//! jobOutput :1187, jobTailFrom :817, jobWrites :999) and spawns.go
//! (spawnState :647, stepState :656). The processes under a running task
//! (jobprocs.rs) are read with them while one is opened.
//!
//! Each run's numbers (what it's doing, did, said last, how long, how
//! many steps) are read from its transcript into a [`RunStat`]; the
//! transcripts stay with the reader, never the UI. Reading happens as the
//! dock is drawn, at most once a second and only while something runs,
//! and on each reading of the list: an idle session reads nothing.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use rush_claude::headless::workflow::{Workflow, read_workflow, workflow_dir};
use rush_convo::{Decode, Job, Session, Status, Subagent, Subagents, Tail};
use rush_core::adapter::{RunState, SubagentRun, SubagentRuns};
use rush_core::time::Ms;

use super::{agent, conn, open_key};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::list_extras::jobs::{Tailed, dock_jobs, read_tail};
use crate::screens::list_extras::spawns::{
    SPAWN_EVERY, SPAWN_GRACE, SPAWN_PREFIX, SpawnRun, claim, spawn_subs, windows,
};

/// The agent each shell step ran, once found, for the conversation to
/// draw the step as that agent's row: what its host says of it, filled
/// in from what the command says (SetChildren, spawn.go:574-583). A step
/// that ran several isn't one agent's row.
/// The agents each step ran, found, each with its own session as last
/// read: one is the step's own row, several a row each under it
/// (children, SetChildren).
pub fn kids_of(
    r: &Runs,
    s: &Session,
    reg: &rush_core::adapter::Registry,
) -> HashMap<String, Vec<rush_convo::render::Child>> {
    let mut out: HashMap<String, Vec<rush_convo::render::Child>> = HashMap::new();
    let mut found: Vec<(&String, &SpawnRun)> = r
        .spawns
        .iter()
        .filter(|(_, sp)| !sp.path.is_empty())
        .collect();
    found.sort_by(|a, b| a.1.born.cmp(&b.1.born).then(a.0.cmp(b.0)));
    for (id, sp) in found {
        if s.step_by_call(&sp.step).is_none() {
            continue;
        }
        let mut spawn = rush_convo::render::Spawn {
            kind: sp.kind.clone(),
            name: sp.name.clone(),
            prompt: sp.prompt.clone(),
            model: sp.model.clone(),
            ..Default::default()
        };
        if spawn.name.is_empty() {
            spawn.name = crate::screens::accounts::look::harness_name(reg, &sp.kind);
        }
        out.entry(sp.step.clone())
            .or_default()
            .push(rush_convo::render::Child {
                id: id.clone(),
                spawn,
                sess: r.full.get(&format!("{SPAWN_PREFIX}{id}")).cloned(),
                live: sp.live,
                start: sp.born,
            });
    }
    out
}

/// Whether two steps' agents draw the same: the same agents, their
/// sessions the same reads.
fn same_kids(
    a: &HashMap<String, Vec<rush_convo::render::Child>>,
    b: &HashMap<String, Vec<rush_convo::render::Child>>,
) -> bool {
    let same = |x: &rush_convo::render::Child, y: &rush_convo::render::Child| {
        x.id == y.id
            && x.live == y.live
            && x.start == y.start
            && x.spawn == y.spawn
            && match (&x.sess, &y.sess) {
                (Some(p), Some(q)) => Arc::ptr_eq(p, q),
                (None, None) => true,
                _ => false,
            }
    };
    a.len() == b.len()
        && a.iter().all(|(k, xs)| {
            b.get(k).is_some_and(|ys| {
                xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same(x, y))
            })
        })
}

pub fn ran_of(
    r: &Runs,
    s: &Session,
    reg: &rush_core::adapter::Registry,
) -> HashMap<String, rush_convo::render::Spawn> {
    let mut by: HashMap<&str, Vec<&SpawnRun>> = HashMap::new();
    for sp in r.spawns.values().filter(|sp| !sp.path.is_empty()) {
        by.entry(sp.step.as_str()).or_default().push(sp);
    }
    let mut out = HashMap::new();
    for (step, runs) in by {
        let [k] = runs[..] else {
            continue; // several: a row each under it, not yet drawn
        };
        let Some(id) = s.step_by_call(step) else {
            continue;
        };
        let st = s.step(id);
        let mut sp = rush_convo::render::Spawn {
            kind: k.kind.clone(),
            name: k.name.clone(),
            prompt: k.prompt.clone(),
            model: k.model.clone(),
            ..Default::default()
        };
        let hint = if st.kind == rush_core::tool::Kind::Shell {
            rush_convo::render::spawn_of(reg, &st.call().input.command)
        } else {
            None
        };
        if let Some(h) = hint {
            if sp.name.is_empty() {
                sp.kind = h.kind;
                sp.name = h.name;
            }
            let first = |a: String, b: String| if a.is_empty() { b } else { a };
            sp.prompt = first(sp.prompt, h.prompt);
            sp.from = first(sp.from, h.from);
            sp.model = first(sp.model, h.model);
            sp.dir = first(sp.dir, h.dir);
        }
        out.insert(step.to_string(), sp);
    }
    out
}

/// Marks a subagent known only by its task (jobSubPrefix).
pub const JOB_SUB_PREFIX: &str = "job:";
/// How long a run that left the dock stays in it, ticked off (doneFor).
pub const DONE_FOR: Ms = 2 * 60 * 1000;
/// How many finished runs the dock shows.
pub const DONE_SHOWN: usize = 3;
/// How many running subagents the dock shows at once (dockRunsShown).
pub const DOCK_RUNS_SHOWN: usize = 5;
/// How often what runs is read again while something does.
const READ_EVERY: Ms = 1000;
/// How often a workflow's files are read while it runs.
const FLOW_EVERY: Ms = 2000;

/// What a run's transcript says, for its row (Go's light SubagentStats
/// session): what it's doing and since when, what it did last, its last
/// words, when it began and last wrote, its steps, model and effort, and
/// which of the tasks asked about it started.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStat {
    pub doing: String,
    pub since: Ms,
    pub did: Vec<String>,
    pub last_words: String,
    pub first: Ms,
    pub last: Ms,
    pub steps: usize,
    pub model: String,
    pub effort: String,
    /// The tool calls among those asked about that this run made.
    pub made: Vec<String>,
    /// Its model calls, the tokens they read (input and cache) and
    /// wrote, and what they cost (Totals, Cost).
    pub requests: usize,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost: f64,
    /// The worktree it cd'd into, by name (Session.Worktree).
    pub worktree: String,
}

impl RunStat {
    pub fn of(s: &Session, calls: &[String]) -> RunStat {
        let (doing, since) = s.doing();
        let model = s
            .requests
            .last()
            .map(|r| r.model.to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| s.model.clone());
        RunStat {
            doing,
            since,
            did: s.did(),
            last_words: s.last_words(),
            first: s.first,
            last: s.last,
            steps: s.tool_calls(),
            model,
            // The last turn's that said, else what its session runs at
            // (rushmode.go:1231, subEffort).
            effort: s
                .turns
                .iter()
                .rev()
                .find(|t| !t.effort.is_empty())
                .map(|t| t.effort.to_string())
                .unwrap_or_else(|| s.info.effort.clone()),
            made: calls.iter().filter(|c| s.made_call(c)).cloned().collect(),
            requests: s.requests.len(),
            tokens_in: s
                .requests
                .iter()
                .map(|r| {
                    let u = &r.usage;
                    u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h
                })
                .sum(),
            tokens_out: s.requests.iter().map(|r| r.usage.output).sum(),
            cost: 0.0,
            worktree: s.worktree().to_string(),
        }
    }

    /// Its numbers with what its model calls cost, at the prices of agent
    /// kind (Session.Cost).
    pub fn priced(mut self, s: &Session, ag: &super::overview::Agentry) -> RunStat {
        self.cost = s.requests.iter().map(|r| ag.cost(&r.model, &r.usage)).sum();
        self
    }
}

/// A run that left the dock, and when (doneRun).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoneRun {
    pub sa: Subagent,
    pub at: Ms,
}

/// What a workflow's files last said, and when they were read (and when
/// last asked of while live) (workflowSeen).
#[derive(Clone, Debug, Default)]
pub struct FlowSeen {
    pub at: Ms,
    pub live: Ms,
    pub w: Workflow,
}

/// The reader's side: what stays with the transcripts, never the UI's.
#[derive(Default)]
pub(super) struct Reader {
    list: Subagents,
    follow: Option<Box<dyn SubagentRuns>>,
    tails: HashMap<String, (Tail, Session)>,
    /// The tasks' calls the numbers were last worked out for.
    calls: Vec<String>,
    /// The run shown whole, and its tail.
    detail: Option<(String, Tail)>,
    /// Each run read whole for Changes, by id: how big its transcript
    /// was when read (fullSubs).
    full: HashMap<String, u64>,
    /// The process table last read, for CPU; let go when nothing's open.
    table: Option<rush_sys::proc::Table>,
}

/// Where the run shown whole is read from (subTail, subPeek; a spawned
/// agent's spawnRun.view).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailSrc {
    /// A transcript in Claude Code's format: a subagent's own (its lines
    /// all sidechain), or, whole, a spawned agent's session.
    Transcript { path: String, whole: bool },
    /// Another agent's session, read back by its adapter.
    History { kind: String, path: String },
}

/// The run shown whole: opened in the subagents view, or beside its list
/// when that's wide. Its conversation is drawn as any is.
pub struct Detail {
    pub id: String,
    pub src: DetailSrc,
    pub view: crate::session::SessionView,
}

/// What a read of the run shown whole brought.
enum DetailGot {
    Fresh(rush_convo::Fresh),
    Events(Vec<rush_core::event::Event>),
    Failed(String),
}

/// One session's runs, as last read (Go's hostConn: subs, subTails,
/// subRuns, dockWas, dockDone, workflows, workflowDirs, tails).
#[derive(Default)]
pub struct Runs {
    /// Its runs: those listed beside its transcript, the agents rush
    /// hosted for it, then those only a task says.
    pub subs: Vec<Subagent>,
    /// Those listed beside its transcript, as last read.
    pub listed: Vec<Subagent>,
    /// The hosted spawned agents no step's window holds (hostNone), and
    /// when the hosted ones were last looked for.
    pub(super) host_none: HashSet<String>,
    pub(super) hosted_at: Ms,
    /// Each run's numbers, by its id.
    pub stats: HashMap<String, RunStat>,
    /// How the transcripts say each run stands; None until read, or for an
    /// agent that can't say.
    pub follow: Option<Box<dyn SubagentRuns>>,
    pub(super) dock_was: HashSet<String>,
    pub dock_done: Vec<DoneRun>,
    pub(super) dock_serial: u64,
    /// Each workflow run's files, by its folder; and each workflow task's
    /// folder, by its id.
    pub flows: HashMap<String, FlowSeen>,
    pub flow_dirs: HashMap<String, String>,
    /// What each task's output file last held, by its path.
    pub outs: HashMap<String, Tailed>,
    /// The files each task's command writes, by its call, worked out once
    /// (c.writes).
    pub writes: HashMap<String, Vec<String>>,
    /// The processes under its running tasks, as last read (jobprocs.rs).
    pub procs: super::jobprocs::JobProcs,
    /// Finished tasks' rows in the background view as last drawn.
    pub job_rows: Mutex<HashMap<String, crate::screens::list_extras::jobs::RowsMemo>>,
    /// The session's tasks folder, found under its host's scratch.
    pub task_dir: String,
    pub(super) task_dir_for: String,
    pub(super) task_dir_at: Ms,
    /// Where the pick on the dock's running rows was (runPick).
    pub run_pick: usize,
    /// The agents rush hosted for the session's shell or its spawn_agent
    /// calls, by the step that ran each (spawns.go's c.spawns).
    pub spawns: HashMap<String, SpawnRun>,
    /// What this frame shows of them all (running.rs).
    pub shown: super::running::Shown,
    pub(super) reading: bool,
    pub(super) read_at: Ms,
    pub(super) kick: bool,
    pub(super) asked: HashSet<String>,
    pub(super) reader: Arc<Mutex<Reader>>,
    pub(super) path: String,
    /// The worktree each run worked in, when not the session's, by its id
    /// (subWT).
    pub wt: HashMap<String, String>,
    /// Every run's numbers are followed, not only those working: the
    /// subagents view is on screen (followedSubs).
    pub follow_all: bool,
    /// The run shown whole, when one is.
    pub detail: Option<Detail>,
    /// Changes is open: every run's conversation is read whole, for its
    /// edits (rushmode.go:99-118).
    pub full_want: bool,
    /// Each run's conversation read whole, by id, while Changes was open
    /// (fullSubs); handed to the session as its helpers (setHelpers).
    pub full: HashMap<String, Arc<Session>>,
}

/// What a read is asked for.
struct Ask {
    now: Ms,
    pub(super) path: String,
    kind: String,
    decode: Option<Decode>,
    gone: bool,
    /// When a rush session's host started, and its agent's process, 0
    /// unknown: a run last written before the later was cut off by it.
    host_start: Ms,
    agent_pid: i32,
    stats: Vec<(String, String)>,
    calls: Vec<String>,
    outs: Vec<(String, Option<Tailed>)>,
    flows: Vec<String>,
    task_dir: Option<(String, String)>,
    /// The processes to read, while a running task is opened.
    procs: Option<super::jobprocs::ProcAsk>,
    /// The hosts to ask for the agents rush hosted for this session, and
    /// its rush id.
    spawned: Option<(Arc<dyn super::host::Hosts>, String)>,
    /// Spawned agents' own sessions to read, Claude Code's format.
    whole: HashSet<String>,
    /// The kind each run is, for its prices.
    kinds: HashMap<String, String>,
    /// The runs whose worktree isn't known yet, and the session's folder.
    wt_for: Vec<(String, String)>,
    cwd: String,
    /// The run shown whole.
    detail: Option<(String, DetailSrc)>,
    /// The runs to read whole: id, path, whether it's a spawned agent's
    /// own session rather than a subagent's sidechain, and the kind of
    /// agent that reads it back when it isn't in Claude Code's format.
    full: Vec<(String, String, bool, String)>,
}

/// What a read brought.
struct Got {
    now: Ms,
    pub(super) path: String,
    subs: Option<Vec<Subagent>>,
    follow: Option<Box<dyn SubagentRuns>>,
    stats: HashMap<String, RunStat>,
    outs: HashMap<String, Tailed>,
    flows: HashMap<String, Workflow>,
    task_dir: Option<(String, String)>,
    procs: Option<super::jobprocs::JobProcs>,
    spawned: Option<Vec<super::host::Spawned>>,
    wt: HashMap<String, String>,
    detail: Option<(String, DetailGot)>,
    /// The runs read whole again, as they now are.
    full: HashMap<String, Arc<Session>>,
}

impl Ask {
    /// When the session's agent last (re)started: its host's start, or its
    /// agent's process's if later; 0 unknown.
    fn since(&self) -> Ms {
        let agent = (self.host_start != 0 && self.agent_pid > 0)
            .then(|| rush_sys::proc::started(self.agent_pid))
            .flatten()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as Ms);
        self.host_start.max(agent)
    }

    /// Reads what's asked; blocks on the disk.
    fn read(self, reader: &Mutex<Reader>, reg: &rush_core::adapter::Registry) -> Got {
        let mut r = reader.lock().unwrap_or_else(|e| e.into_inner());
        let mut got = Got {
            now: self.now,
            path: self.path.clone(),
            subs: None,
            follow: None,
            stats: HashMap::new(),
            outs: HashMap::new(),
            flows: HashMap::new(),
            task_dir: None,
            procs: None,
            spawned: None,
            wt: HashMap::new(),
            detail: None,
            full: HashMap::new(),
        };
        if let Some((h, parent)) = &self.spawned {
            got.spawned = Some(h.spawned(parent));
        }
        if !self.path.is_empty() {
            let subs = r.list.list(&self.path);
            if !subs.is_empty() {
                if r.follow.is_none() {
                    r.follow = reg.follow_runs(&self.kind);
                }
                if let Some(f) = r.follow.as_mut() {
                    f.set_gone(self.gone);
                    f.set_since(self.since());
                    f.update_runs(&self.path, &sub_runs(&subs));
                    got.follow = Some(f.clone_runs());
                }
            }
            got.subs = Some(subs);
        }
        if let Some(decode) = &self.decode {
            // Every run's numbers again when the tasks asked about change;
            // else only those whose transcripts grew.
            let again = r.calls != self.calls;
            if again {
                r.calls.clone_from(&self.calls);
            }
            for (id, path) in &self.stats {
                let whole = self.whole.contains(id);
                let mut new = false;
                let (t, s) = r.tails.entry(id.clone()).or_insert_with(|| {
                    new = true;
                    let t = if whole {
                        Tail::new(path, decode.clone())
                    } else {
                        Tail::subagent(path, decode.clone())
                    };
                    (t, Session::new())
                });
                let grew = t.read(s).unwrap_or(false);
                if new || grew || again {
                    let kind = self
                        .kinds
                        .get(id)
                        .map_or(self.kind.as_str(), String::as_str);
                    let ag = super::overview::Agentry { reg, kind };
                    got.stats
                        .insert(id.clone(), RunStat::of(s, &self.calls).priced(s, &ag));
                }
            }
        }
        // The runs read whole, again only once each grew (a conversation
        // shared with the UI is read afresh, not added to): a spawned
        // agent's in another format by its own adapter (spawnRun.view).
        for (id, path, whole, kind) in &self.full {
            let size = std::fs::metadata(path).map_or(0, |m| m.len());
            if r.full.get(id) == Some(&size) {
                continue;
            }
            r.full.insert(id.clone(), size);
            let s = if !kind.is_empty() {
                read_history(reg, kind, path)
            } else if let Some(decode) = &self.decode {
                let mut t = if *whole {
                    Tail::new(path, decode.clone())
                } else {
                    Tail::subagent(path, decode.clone())
                };
                let mut s = Session::new();
                t.read(&mut s).ok().map(|_| s)
            } else {
                None
            };
            // One that can't be read isn't asked again until it grows.
            got.full.insert(id.clone(), Arc::new(s.unwrap_or_default()));
        }
        if let Some((id, src)) = &self.detail {
            got.detail = Some((
                id.clone(),
                read_detail(&mut r, id, src, self.decode.as_ref(), reg),
            ));
        }
        match &self.procs {
            Some(a) => got.procs = Some(super::jobprocs::read(&mut r.table, a)),
            None => r.table = None,
        }
        drop(r);
        if !self.wt_for.is_empty() {
            let paths: Vec<&str> = self.wt_for.iter().map(|(_, p)| p.as_str()).collect();
            let wts = rush_fleet::sub_worktrees(&paths, &self.cwd);
            for (id, p) in &self.wt_for {
                if let Some(wt) = wts.get(p) {
                    got.wt.insert(id.clone(), wt.clone());
                }
            }
        }
        for (p, was) in self.outs {
            if let Some(t) = read_tail(&p, was.as_ref()) {
                got.outs.insert(p, t);
            }
        }
        for d in self.flows {
            got.flows.insert(d.clone(), read_workflow(Path::new(&d)));
        }
        if let Some((tmp, sid)) = self.task_dir {
            got.task_dir = Some((sid.clone(), find_task_dir(&tmp, &sid)));
        }
        got
    }
}

/// Another agent's session read back by its adapter, whole (spawnRun.view's
/// Sess for an agent not in Claude Code's format).
fn read_history(reg: &rush_core::adapter::Registry, kind: &str, path: &str) -> Option<Session> {
    let ad = reg.get(rush_core::adapter::migrated(kind).as_str())?;
    let h = ad.history_reader()?;
    let src = rush_core::adapter::Session {
        kind: rush_core::adapter::Kind::from(kind),
        transcript: path.to_string(),
        ..rush_core::adapter::Session::default()
    };
    let evs = rush_core::adapter::read_history(h, &src, 0).ok()?;
    let mut s = Session::new();
    for e in &evs {
        s.apply(e, 0);
    }
    Some(s)
}

/// Reads what the run shown whole has gained: a transcript's new lines,
/// or another agent's session read again.
fn read_detail(
    r: &mut Reader,
    id: &str,
    src: &DetailSrc,
    decode: Option<&Decode>,
    reg: &rush_core::adapter::Registry,
) -> DetailGot {
    match src {
        DetailSrc::Transcript { path, whole } => {
            let Some(decode) = decode else {
                return DetailGot::Failed("rush can't read this agent's transcripts".into());
            };
            if r.detail.as_ref().is_none_or(|(at, _)| at != id) {
                let t = if *whole {
                    Tail::new(path, decode.clone())
                } else {
                    Tail::subagent(path, decode.clone())
                };
                r.detail = Some((id.to_string(), t));
            }
            let Some((_, t)) = r.detail.as_mut() else {
                unreachable!("the tail was just set")
            };
            match t.fetch_most(crate::worker::TAIL_SLICE) {
                Ok(f) => DetailGot::Fresh(f),
                Err(e) => DetailGot::Failed(e.to_string()),
            }
        }
        DetailSrc::History { kind, path } => {
            r.detail = None;
            let Some(ad) = reg.get(rush_core::adapter::migrated(kind).as_str()) else {
                return DetailGot::Failed(format!("rush can't read {kind}'s sessions"));
            };
            let Some(h) = ad.history_reader() else {
                return DetailGot::Failed(format!("rush can't read {kind}'s sessions"));
            };
            let s = rush_core::adapter::Session {
                kind: rush_core::adapter::Kind::from(kind.as_str()),
                transcript: path.clone(),
                ..rush_core::adapter::Session::default()
            };
            match rush_core::adapter::read_history(h, &s, 0) {
                Ok(evs) => DetailGot::Events(evs),
                Err(e) => DetailGot::Failed(e.to_string()),
            }
        }
    }
}

/// The session's tasks folder under its host's scratch:
/// claude-*/*/<sid>/tasks (jobOutput's glob).
fn find_task_dir(tmp: &str, sid: &str) -> String {
    let dirs = |p: &Path, pre: &str| -> Vec<std::path::PathBuf> {
        let mut v: Vec<_> = std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(pre))
            .map(|e| e.path())
            .collect();
        v.sort();
        v
    };
    for c in dirs(Path::new(tmp), "claude-") {
        for d in dirs(&c, "") {
            let t = d.join(sid).join("tasks");
            if t.is_dir() {
                return t.to_string_lossy().into_owned();
            }
        }
    }
    String::new()
}

/// What the call that ran step's agent says it's for, its title
/// (spawnTitle).
fn spawn_title(s: &Session, step: &str) -> String {
    let Some(i) = s.step_by_call(step) else {
        return String::new();
    };
    let c = s.step(i).call();
    let d = crate::style::one_line(&c.input.description);
    if d.is_empty() {
        crate::style::one_line(&c.title)
    } else {
        d
    }
}

/// The runs as the adapter's follower takes them (twopass.go subRuns).
fn sub_runs(subs: &[Subagent]) -> Vec<SubagentRun> {
    subs.iter()
        .map(|sa| SubagentRun {
            id: sa.id.clone(),
            tool_use_id: sa.tool_use_id.clone(),
            depth: sa.depth,
            mod_time: sa.modified,
            born: sa.born,
            born_ns: sa.born_ns,
            kind: sa.typ.clone(),
            description: sa.description.clone(),
            path: sa.path.clone(),
        })
        .collect()
}

/// Session key's runs.
pub fn runs<'a>(m: &'a mut Model, key: &str) -> &'a mut Runs {
    &mut conn(m, key).runs
}

/// The subagent tasks have that no listed run has (jobSubs): an agent
/// that keeps no transcript for its subagents, or Claude Code's run not
/// listed yet.
pub fn job_subs(s: &Session, have: &[Subagent], claude: bool) -> Vec<Subagent> {
    let mut out = Vec::new();
    for j in s.jobs() {
        if s.job_kind(j) != "subagent" || claude && !j.running() {
            continue;
        }
        // Claude Code's task id is its run's agent id.
        if have.iter().any(|sa| {
            !j.tool_use_id.is_empty() && sa.tool_use_id == j.tool_use_id || claude && sa.id == j.id
        }) {
            continue;
        }
        out.push(Subagent {
            id: format!("{JOB_SUB_PREFIX}{}", j.id),
            typ: if j.agent.is_empty() {
                "subagent".into()
            } else {
                j.agent.clone()
            },
            description: j.label.clone(),
            tool_use_id: j.tool_use_id.clone(),
            depth: 1,
            born: j.start,
            ..Subagent::default()
        });
    }
    out
}

/// subs with the subagent tasks they lack (withJobSubs).
pub fn with_job_subs(s: &Session, mut subs: Vec<Subagent>, claude: bool) -> Vec<Subagent> {
    subs.retain(|sa| !sa.id.starts_with(JOB_SUB_PREFIX));
    let more = job_subs(s, &subs, claude);
    subs.extend(more);
    subs
}

/// How a subagent known by its task stands: as the task (jobSubState).
fn job_sub_state(s: &Session, sa: &Subagent) -> (String, bool) {
    let id = sa.id.trim_start_matches(JOB_SUB_PREFIX);
    match s.job(id) {
        Some(j) if j.running() => (String::new(), true),
        Some(j) => (j.status.clone(), false),
        None => (String::new(), false),
    }
}

/// How the command step id ran stands: still running, or its background
/// shell; else how it ended (stepState).
pub fn step_state(s: &Session, id: &str) -> (String, bool) {
    for j in s.jobs() {
        if j.tool_use_id == id {
            if j.running() {
                return (String::new(), true);
            }
            if j.status == "failed" || j.status == "stopped" {
                return (j.status.clone(), false);
            }
        }
    }
    match s.step_by_call(id).map(|i| s.step(i).status) {
        Some(Status::Running) => (String::new(), true),
        Some(Status::Failed) => ("failed".into(), false),
        Some(Status::Lost) => ("stopped".into(), false),
        _ => (String::new(), false),
    }
}

impl Runs {
    /// Its runs again: those listed, the spawned agents found, and those
    /// only a task says.
    pub fn rebuild(&mut self, s: &Session, claude: bool) {
        let title = |step: &str| spawn_title(s, step);
        let mut subs = self.listed.clone();
        subs.extend(spawn_subs(&self.spawns, &title));
        self.subs = with_job_subs(s, subs, claude);
    }

    /// Takes in the sessions rush hosted for this one: each known one's
    /// state, and a new one under the step that started it, its harness
    /// said which, else the step whose window holds it (onSpawnFound).
    fn take_spawned(&mut self, s: &Session, found: Vec<super::host::Spawned>, now: Ms) {
        let mut wins = None;
        for h in found {
            if let Some(r) = self.spawns.get_mut(&h.id) {
                r.live = h.live;
                continue;
            }
            if self.host_none.contains(&h.id) || h.transcript.is_empty() {
                continue;
            }
            let mut step = h.step.clone();
            if step.is_empty() || s.step_by_call(&step).is_none() {
                if !step.is_empty() && s.partial {
                    continue; // further back than read so far
                }
                let wins = wins.get_or_insert_with(|| windows(s, now, SPAWN_GRACE));
                step = claim(wins, &h.kind, &h.prompt, h.born, false).to_string();
            }
            if step.is_empty() {
                // Begun outside every window, it stays so.
                self.host_none.insert(h.id);
                continue;
            }
            self.spawns.insert(
                h.id.clone(),
                SpawnRun {
                    step,
                    kind: h.kind,
                    name: h.name,
                    prompt: h.prompt,
                    model: h.model,
                    path: h.transcript,
                    born: h.born,
                    hosted: h.id,
                    live: h.live,
                },
            );
        }
    }

    /// The rush session of spawned run id (a "spawn:" run), when rush
    /// hosts it (watchedHost's spawnRunFor).
    pub fn hosted_of(&self, id: &str) -> Option<&SpawnRun> {
        let h = id.strip_prefix(SPAWN_PREFIX)?;
        self.spawns.get(h).filter(|r| !r.hosted.is_empty())
    }

    /// The follower of the session's runs, as last read.
    pub fn follow(&self) -> Option<&dyn SubagentRuns> {
        self.follow.as_deref()
    }

    /// The folder of workflow task j's run, as its step's output says;
    /// "" until it has (workflowDir).
    pub fn flow_dir(&self, j: &Job) -> &str {
        self.flow_dirs.get(&j.id).map_or("", String::as_str)
    }

    /// What the files of the run in dir last said (workflowFiles; read by
    /// refresh).
    pub fn flow_files(&self, dir: &str) -> Option<&Workflow> {
        self.flows.get(dir).map(|f| &f.w)
    }

    /// How a subagent run stands: the status its task-finished notice
    /// gave (or failed), and whether it's still working (subStateIn).
    /// jobs is the session's subagent_jobs.
    pub fn state(
        &self,
        s: &Session,
        sa: &Subagent,
        jobs: &HashMap<String, &Job>,
        now: Ms,
    ) -> (String, bool) {
        if let Some(step) = sa.id.strip_prefix(SPAWN_PREFIX) {
            // A hosted one as its host says; else as the command that ran
            // it, or its background shell (spawnState).
            return match self.spawns.get(step) {
                Some(r) if !r.hosted.is_empty() => (String::new(), r.live),
                _ => step_state(s, &sa.tool_use_id),
            };
        }
        if sa.id.starts_with(JOB_SUB_PREFIX) {
            return job_sub_state(s, sa);
        }
        if !sa.workflow.is_empty()
            && let Some((a, running)) = super::workflow::agent_of(self, s, sa)
        {
            // A workflow's agent: as its workflow last said.
            return match a.state.as_str() {
                "running" | "queued" if running => (String::new(), true),
                "running" | "queued" => ("stopped".into(), false),
                "failed" | "stopped" => (a.state.clone(), false),
                _ => (String::new(), false),
            };
        }
        // When its transcript last grew: known without reading it.
        let last = sa.modified;
        let mut status = s
            .task_status
            .get(sa.id.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let st = s.step_by_call(&sa.tool_use_id).map(|i| s.step(i));
        if st.is_some_and(|st| st.status == Status::Failed) && status.is_empty() {
            status = "failed".into();
        }
        // Claude Code says, in a session rush runs: its task runs until it
        // hears otherwise, however long the run goes quiet.
        let j = jobs.get(&sa.id).copied().or_else(|| {
            (!sa.tool_use_id.is_empty())
                .then(|| jobs.get(&format!("call:{}", sa.tool_use_id)).copied())
                .flatten()
        });
        if let Some(j) = j {
            if !j.running() && status.is_empty() && j.status != "ended" {
                status = j.status.clone();
            }
            // One its session stopped as it restarted, and the resumed
            // parent woke with a message, runs again with no new task.
            if !j.running()
                && let Some(f) = self.follow()
                && f.state(&sa.id, &sa.tool_use_id).0 == RunState::Running
                && f.going(&sa.id, &sa.tool_use_id, last, now).0
            {
                return (String::new(), true);
            }
            return (status, j.running());
        }
        // Otherwise the transcripts say.
        let Some(f) = self.follow() else {
            return (status, false);
        };
        let (rs, _, _) = f.state(&sa.id, &sa.tool_use_id);
        if !status.is_empty() && rs != RunState::Running {
            return (status, false); // said how it ended, and nothing woke it since
        }
        let (mut live, ended) = f.going(&sa.id, &sa.tool_use_id, last, now);
        // Its call still open says nothing once its session's agent has
        // gone, or started again since the run last wrote.
        if st.is_some_and(|st| st.status == Status::Running) && !f.gone() && !f.cut(last) {
            live = true;
        }
        if live {
            return (String::new(), true);
        }
        (if status.is_empty() { ended } else { status }, false)
    }

    /// The runs still working, the latest started first: one writing
    /// doesn't move it, so the dock's rows stay put (runningSubs).
    pub fn running(&self, s: &Session, now: Ms) -> Vec<Subagent> {
        let jobs = s.subagent_jobs();
        let mut out: Vec<Subagent> = self
            .subs
            .iter()
            .rev()
            .filter(|sa| self.state(s, sa, &jobs, now).1)
            .cloned()
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.born_order()));
        out
    }

    /// The running subagents the dock shows: those in the background. One
    /// the turn waits on shows in the conversation, under its call; a
    /// workflow's, under its workflow's row (dockSubs). Notes those that
    /// left it as done.
    pub fn dock_subs(&mut self, s: &Session, running: &[Subagent], now: Ms) -> Vec<Subagent> {
        let out: Vec<Subagent> = running
            .iter()
            .filter(|sa| sa.workflow.is_empty())
            .filter(|sa| {
                !s.step_by_call(&sa.tool_use_id)
                    .is_some_and(|i| matches!(s.step(i).status, Status::Running | Status::Waiting))
            })
            .cloned()
            .collect();
        self.note_done(s, &out, now);
        out
    }

    /// Keeps the runs that were in the dock and now aren't, newest first,
    /// as finished at now, for DONE_FOR (noteDone).
    fn note_done(&mut self, s: &Session, run: &[Subagent], now: Ms) {
        if self.dock_serial != s.serial() {
            self.dock_serial = s.serial();
            self.dock_was.clear();
            self.dock_done.clear();
        }
        let is: HashSet<String> = run.iter().map(|sa| sa.id.clone()).collect();
        self.dock_done
            .retain(|d| !is.contains(&d.sa.id) && now - d.at <= DONE_FOR);
        for sa in &self.subs {
            if self.dock_was.contains(&sa.id) && !is.contains(&sa.id) {
                self.dock_done.insert(
                    0,
                    DoneRun {
                        sa: sa.clone(),
                        at: now,
                    },
                );
            }
        }
        self.dock_was = is;
    }

    /// The finished runs the dock shows (doneShown).
    pub fn done_shown(&self) -> &[DoneRun] {
        &self.dock_done[..self.dock_done.len().min(DONE_SHOWN)]
    }

    /// Whether subagent id started task j (madeBy).
    pub fn made_by(&self, j: &Job, id: &str) -> bool {
        self.stats
            .get(id)
            .is_some_and(|st| st.made.contains(&j.tool_use_id))
    }

    /// The run that started task j, if one did (jobOwner).
    pub fn job_owner(&self, j: &Job) -> Option<&Subagent> {
        self.subs.iter().find(|sa| self.made_by(j, &sa.id))
    }

    /// Task j's output file: the agent's own, else under the session's
    /// tasks folder once found (jobOutput).
    pub fn job_output(&self, j: &Job) -> String {
        if !j.output_file.is_empty() {
            return j.output_file.clone();
        }
        if self.task_dir.is_empty() {
            return String::new();
        }
        Path::new(&self.task_dir)
            .join(format!("{}.output", j.id))
            .to_string_lossy()
            .into_owned()
    }

    /// Task j's output as last read, when it has some.
    pub fn out_of(&self, j: &Job) -> Option<&Tailed> {
        self.outs.get(&self.job_output(j))
    }

    /// The files task j's command writes, as last worked out (jobWrites).
    pub fn writes_of(&self, j: &Job) -> &[String] {
        self.writes
            .get(&j.tool_use_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Task j's output and the file it's from: its own, else one its
    /// command writes (jobTailFrom).
    pub fn from_of(&self, s: &Session, j: &Job, now: Ms) -> (String, Option<&Tailed>) {
        let tailed = |p: &str| self.outs.get(p);
        crate::screens::list_extras::jobs::tail_from(
            s,
            j,
            &self.job_output(j),
            self.writes_of(j),
            &tailed,
            now,
        )
    }

    /// What task j's processes use, while they're read.
    pub fn usage_of(&self, j: &Job) -> Option<(u64, f64)> {
        self.procs.usage.get(&j.id).copied()
    }
}

/// The dock's tasks no running subagent started: those show under their
/// subagent instead (looseJobs).
pub fn loose_jobs<'a>(r: &Runs, s: &'a Session, running: &[Subagent]) -> Vec<&'a Job> {
    dock_jobs(s)
        .into_iter()
        .filter(|j| !running.iter().any(|sa| r.made_by(j, &sa.id)))
        .collect()
}

/// The dock's tasks subagent id started (jobsOf).
pub fn jobs_of<'a>(r: &Runs, s: &'a Session, id: &str) -> Vec<&'a Job> {
    dock_jobs(s)
        .into_iter()
        .filter(|j| r.made_by(j, id))
        .collect()
}

/// Whether the session's agent is known to have exited (sessionGone): a
/// past conversation, or a terminal one with no process.
fn gone(m: &Model, key: &str) -> bool {
    agent(m, key).is_some_and(|a| a.job.state == rush_core::agent::State::Done || a.pid == 0)
}

/// Asks to read session key's runs again at once, whatever the time.
pub fn kick(m: &mut Model, key: &str) {
    runs(m, key).kick = true;
}

/// Reads what session key's runs have gained, off the UI thread, when
/// it's due: once its list is first wanted, kicked, or a second on while
/// anything runs (refreshSubs). With no loop it's read here and now.
pub fn refresh(m: &mut Model, key: &str) {
    let now = m.now;
    let Some(a) = agent(m, key).cloned() else {
        return;
    };
    let kind = a.kind.as_str().to_string();
    let reg = m.reg.clone();
    let claude = reg.reads_as_claude(&kind);
    let decode = crate::session::decoder(&reg, &kind);
    let path = a.job.transcript_path.clone();
    let gone = gone(m, key);
    let host_id = a.job.id.clone();
    let sid = a.job.session_id.clone();
    let rush = a.rush;
    let hosts = super::host::hosts_of(m);
    // The running tasks opened in the background view: their processes
    // and every file they write are read.
    let bg_open: Vec<String> = if super::views::view_name(m, key) == "background" {
        conn(m, key)
            .vs
            .open
            .iter()
            .filter(|(_, on)| **on)
            .filter_map(|(r, _)| r.strip_prefix("job:").map(str::to_string))
            .collect()
    } else {
        Vec::new()
    };
    let agent_pid = match conn(m, key).info.agent_pid {
        0 => a.pid,
        p => p as i32,
    };
    let (host_start, host_agent) = if rush {
        let i = &conn(m, key).info;
        (i.began.started_at, i.agent_pid as i32)
    } else {
        (0, 0)
    };
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return;
    };
    let s = &v.sess;
    let r = &mut kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default()
        .runs;
    if r.path != path {
        // Another transcript: nothing of the last one's is this one's.
        *r = Runs {
            path: path.clone(),
            ..Runs::default()
        };
    }
    // What's known without the disk: the runs only tasks say.
    r.rebuild(s, claude);
    let running = r.running(s, now);
    let busy = !running.is_empty()
        || !r.dock_done.is_empty()
        || s.running_jobs().next().is_some()
        || s.live().is_some()
        || r.detail
            .as_ref()
            .is_some_and(|d| d.view.sess.live().is_some());
    // The agents rush hosted for it, whole: their steps and reply go under
    // the step that ran them (SetChildren). Changes is open: every run's
    // conversation too, for its edits (rushmode.go:99-118). A run not read
    // yet is due at once.
    let mut full = Vec::new();
    for sa in &r.subs {
        let spawn = sa.id.strip_prefix(SPAWN_PREFIX);
        if sa.path.is_empty() || spawn.is_none() && !r.full_want {
            continue;
        }
        // Another agent's own session is read back by its adapter.
        let kind = spawn
            .and_then(|h| r.spawns.get(h))
            .map(|r| r.kind.clone())
            .filter(|k| !reg.reads_as_claude(k))
            .unwrap_or_default();
        full.push((sa.id.clone(), sa.path.clone(), spawn.is_some(), kind));
    }
    let unread = full.iter().any(|(id, ..)| !r.full.contains_key(id));
    let due =
        !r.reading && (r.kick || r.read_at == 0 || unread || busy && now - r.read_at >= READ_EVERY);
    if !due {
        return;
    }
    r.reading = true;
    r.kick = false;
    r.read_at = now;
    // The numbers of the runs working and those just done, or of every
    // run while the subagents view is on screen (followedSubs).
    let mut want: Vec<(String, String)> = Vec::new();
    let mut whole = HashSet::new();
    let mut kinds = HashMap::new();
    let followed: Vec<&Subagent> = if r.follow_all {
        r.subs.iter().collect()
    } else {
        running
            .iter()
            .chain(r.dock_done.iter().map(|d| &d.sa))
            .collect()
    };
    for sa in followed {
        if sa.path.is_empty() || want.iter().any(|(id, _)| *id == sa.id) {
            continue;
        }
        r.asked.insert(sa.id.clone());
        if let Some(h) = sa.id.strip_prefix(SPAWN_PREFIX) {
            // Another agent's session is read by its own adapter: not here.
            let k = r.spawns.get(h).map(|r| r.kind.clone()).unwrap_or_default();
            if !reg.reads_as_claude(&k) {
                continue;
            }
            whole.insert(sa.id.clone());
            kinds.insert(sa.id.clone(), k);
        }
        want.push((sa.id.clone(), sa.path.clone()));
    }
    // Which checkout each run worked in, for those not known yet.
    let wt_for: Vec<(String, String)> = r
        .subs
        .iter()
        .filter(|sa| !sa.path.is_empty() && !r.wt.contains_key(&sa.id))
        .map(|sa| (sa.id.clone(), sa.path.clone()))
        .collect();
    for (id, _) in &wt_for {
        r.wt.insert(id.clone(), String::new()); // asked once
    }
    let cwd = if s.info.cwd.is_empty() {
        s.cwd.clone()
    } else {
        s.info.cwd.clone()
    };
    // The run shown whole.
    let detail = r.detail.as_ref().map(|d| (d.id.clone(), d.src.clone()));
    // The background tasks' calls, to know which run started each.
    let calls: Vec<String> = dock_jobs(s)
        .iter()
        .map(|j| j.tool_use_id.clone())
        .filter(|c| !c.is_empty())
        .collect();
    // Each running task's output, and a workflow's files.
    let mut outs: Vec<(String, Option<Tailed>)> = Vec::new();
    let out_want = |p: &str, r: &Runs, outs: &mut Vec<(String, Option<Tailed>)>| {
        if !p.is_empty() && !outs.iter().any(|(q, _)| q == p) {
            outs.push((p.to_string(), r.outs.get(p).cloned()));
        }
    };
    for j in s.jobs().iter().filter(|j| j.running() || j.background) {
        let p = r.job_output(j);
        out_want(&p, r, &mut outs);
        if s.job_kind(j) != "shell" {
            continue;
        }
        // The files its command writes, worked out once its call is read.
        if !r.writes.contains_key(&j.tool_use_id) && !s.job_command(j).is_empty() {
            r.writes.insert(j.tool_use_id.clone(), s.job_writes(j));
        }
        let open = bg_open.contains(&j.id);
        // Its own file has nothing (or it's opened): what it writes.
        if open || r.outs.get(&p).is_none_or(|t| t.lines.is_empty()) {
            for w in r.writes_of(j).to_vec() {
                out_want(&w, r, &mut outs);
            }
        }
        // Opened, the files what it runs follows too.
        if open {
            let cwd = if s.info.cwd.is_empty() {
                &s.cwd
            } else {
                &s.info.cwd
            };
            let follows: Vec<String> = r
                .procs
                .trees
                .get(&j.id)
                .into_iter()
                .flatten()
                .flat_map(|n| crate::screens::list_extras::jobs::tail_follows(&n.cmd, cwd))
                .collect();
            for f in follows {
                out_want(&f, r, &mut outs);
            }
        }
    }
    // The processes under the running shells, while one is opened.
    let shells: Vec<(String, Ms)> = s
        .running_jobs()
        .filter(|j| s.job_kind(j) == "shell")
        .map(|j| (j.id.clone(), j.start))
        .collect();
    let open: Vec<String> = shells
        .iter()
        .filter(|(id, _)| bg_open.contains(id))
        .map(|(id, _)| id.clone())
        .collect();
    let procs = (!open.is_empty() && agent_pid != 0).then_some(super::jobprocs::ProcAsk {
        agent: agent_pid,
        shells,
        open,
    });
    if procs.is_none() {
        r.procs = Default::default();
    }
    for j in s.jobs() {
        if s.job_kind(j) == "workflow" && !r.flow_dirs.contains_key(&j.id) {
            let d = s
                .step_by_call(&j.tool_use_id)
                .map(|i| workflow_dir(&s.step(i).output))
                .unwrap_or_default();
            if !d.is_empty() {
                r.flow_dirs.insert(j.id.clone(), d);
            }
        }
    }
    let mut flows = Vec::new();
    let mut flow_want = |d: &str, live: bool, r: &mut Runs| {
        if d.is_empty() {
            return;
        }
        let f = r.flows.entry(d.to_string()).or_default();
        if live {
            f.live = now;
        }
        if !f.w.done && now - f.at >= FLOW_EVERY && (f.at == 0 || now - f.live < 30_000) {
            f.at = now;
            if !flows.iter().any(|x: &String| x == d) {
                flows.push(d.to_string());
            }
        }
    };
    let flow_jobs: Vec<(String, bool)> = s
        .jobs()
        .iter()
        .filter(|j| s.job_kind(j) == "workflow")
        .map(|j| (r.flow_dir(j).to_string(), j.running()))
        .collect();
    for (d, live) in flow_jobs {
        flow_want(&d, live, r);
    }
    let run_dirs: Vec<String> = r
        .subs
        .iter()
        .filter(|sa| !sa.workflow.is_empty())
        .filter_map(|sa| {
            Path::new(&sa.path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .collect();
    for d in run_dirs {
        flow_want(&d, false, r);
    }
    // The session's tasks folder, while a task has no file of its own.
    let mut task_dir = None;
    if rush && !sid.is_empty() && r.task_dir_for != sid {
        r.task_dir.clear();
        r.task_dir_at = 0;
        r.task_dir_for = sid.clone();
    }
    if rush
        && !sid.is_empty()
        && r.task_dir.is_empty()
        && now - r.task_dir_at >= 2000
        && s.jobs().iter().any(|j| j.output_file.is_empty())
    {
        r.task_dir_at = now;
        task_dir = Some((rush_fleet::temp_dir(&host_id), sid));
    }
    // The agents rush hosted for it, every two seconds while it works.
    let mut spawned = None;
    if rush && !host_id.is_empty() && (r.hosted_at == 0 || busy && now - r.hosted_at >= SPAWN_EVERY)
    {
        r.hosted_at = now;
        spawned = hosts.map(|h| (h, host_id.clone()));
    }
    let ask = Ask {
        now,
        spawned,
        whole,
        kinds,
        wt_for,
        cwd,
        detail,
        full,
        path: if claude { path } else { String::new() },
        kind,
        decode,
        gone,
        host_start,
        agent_pid: host_agent,
        stats: want,
        calls,
        outs,
        flows,
        task_dir,
        procs,
    };
    let reader = r.reader.clone();
    let key = key.to_string();
    m.later(
        move || ask.read(&reader, &reg),
        move |m, got| took(m, &key, got),
    );
}

/// Closes the last turn of the run shown whole once its step has
/// finished: a run in the background returned its call at once, and is
/// done when its task is (readSub).
fn end_detail_turn(s: &Session, subs: &[Subagent], d: &mut Detail, now: Ms) {
    if d.view.sess.live().is_none() {
        return;
    }
    let call = subs
        .iter()
        .find(|sa| sa.id == d.id)
        .map(|sa| sa.tool_use_id.as_str())
        .unwrap_or("");
    if call.is_empty() || s.subagent_job(&d.id, call).is_some_and(Job::running) {
        return;
    }
    if s.step_by_call(call)
        .is_some_and(|i| s.step(i).status != Status::Running)
    {
        let end = rush_core::event::TurnEnd {
            reason: "done".into(),
            ..Default::default()
        };
        d.view
            .sess
            .apply(&rush_core::event::Event::TurnEnd(end), now);
    }
}

/// Takes in what a read brought (onPane, onSubStats).
fn took(m: &mut Model, key: &str, got: Got) -> Out {
    let claude = agent(m, key).is_some_and(|a| m.reg.reads_as_claude(a.kind.as_str()));
    let reg = m.reg.clone();
    let Model { session, kept, .. } = m;
    let r = &mut kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default()
        .runs;
    r.reading = false;
    if got.path != r.path && !got.path.is_empty() {
        return Out::Idle; // read for a transcript it no longer is
    }
    if let Some(p) = got.procs {
        // Which command of each running chain runs now (onShells).
        if let Some(v) = session.as_mut().filter(|v| v.key == key) {
            v.sess.watch_shells(&p.shells, got.now);
        }
        r.procs = p;
    }
    let s = session.as_ref().filter(|v| v.key == key).map(|v| &v.sess);
    if let Some(subs) = got.subs {
        r.listed = subs;
    }
    let mut ran = None;
    if let (Some(found), Some(s)) = (got.spawned, s) {
        r.take_spawned(s, found, got.now);
        ran = Some(ran_of(r, s, &reg));
    }
    if let Some(s) = s {
        r.rebuild(s, claude);
    }
    if got.follow.is_some() {
        r.follow = got.follow;
    }
    // Runs found working that were never asked about: their numbers now,
    // once each (readUnread).
    let fresh = s.is_some_and(|s| {
        r.running(s, got.now)
            .iter()
            .any(|sa| !sa.path.is_empty() && !r.asked.contains(&sa.id))
    });
    let fresh = fresh
        || r.follow_all
            && r.subs
                .iter()
                .any(|sa| !sa.path.is_empty() && !r.asked.contains(&sa.id));
    r.stats.extend(got.stats);
    r.outs.extend(got.outs);
    for (id, wt) in got.wt {
        r.wt.insert(id, wt);
    }
    r.full.extend(got.full);
    let mut more = false;
    if let (Some((id, d)), Some(det)) = (got.detail, r.detail.as_mut())
        && det.id == id
    {
        match d {
            DetailGot::Fresh(f) => {
                more = f.more;
                det.view.take(f);
            }
            DetailGot::Events(evs) => det.view.take_events(evs, true),
            DetailGot::Failed(e) => {
                det.view.read = true;
                det.view.err = e;
            }
        }
        if let Some(s) = s {
            end_detail_turn(s, &r.subs, det, got.now);
        }
    }
    let fresh = fresh || more;
    // The conversation draws the steps that ran them as the agents, with
    // their steps and replies under them.
    if let Some(ran) = ran
        && let Some(v) = session.as_mut().filter(|v| v.key == key)
        && v.o.ran != ran
    {
        v.o.ran = ran;
    }
    if let Some(v) = session.as_mut().filter(|v| v.key == key) {
        let kids = kids_of(r, &v.sess, &reg);
        if !same_kids(&v.o.kids, &kids) {
            v.o.kids = kids;
        }
    }
    for (d, w) in got.flows {
        r.flows.entry(d).or_default().w = w;
    }
    if let Some((sid, dir)) = got.task_dir
        && sid == r.task_dir_for
    {
        r.task_dir = dir;
    }
    if fresh {
        r.kick = true;
        if open_key(m).as_deref() == Some(key) {
            refresh(m, key);
        }
    }
    set_helpers(m, key);
    // What waits for a run goes once it has taken a step.
    if open_key(m).as_deref() == Some(key) {
        super::relay::flush(m, key);
    }
    Out::Draw
}

/// Hands session key the conversations of its subagents read whole, and
/// of the agents its shell ran, so its Changes has their edits; a run
/// whose calls came in the session's own conversation is left out
/// (ownfiles.go:65-90 setHelpers).
pub fn set_helpers(m: &mut Model, key: &str) {
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_mut().filter(|v| v.key == key) else {
        return;
    };
    let Some(c) = kept.get::<super::Core>().conns.get(key) else {
        return;
    };
    let r = &c.runs;
    let s = &v.sess;
    let mut hs = Vec::new();
    let mut spawns = Vec::new();
    for sa in &r.subs {
        let Some(full) = r.full.get(&sa.id) else {
            continue;
        };
        if sa.id.starts_with(SPAWN_PREFIX) {
            spawns.push((sa, full));
            continue;
        }
        if s.step_by_call(&sa.tool_use_id)
            .is_some_and(|i| !s.step(i).children.is_empty())
        {
            continue; // its calls came in the session's own conversation
        }
        let name = if sa.description.is_empty() {
            &sa.typ
        } else {
            &sa.description
        };
        hs.push(rush_convo::Helper {
            name: helper_name(name),
            sess: full.clone(),
        });
    }
    spawns.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    for (sa, full) in spawns {
        let h = sa.id.trim_start_matches(SPAWN_PREFIX);
        let name = r
            .spawns
            .get(h)
            .map(|r| if r.name.is_empty() { &r.kind } else { &r.name })
            .unwrap_or(&sa.typ);
        hs.push(rush_convo::Helper {
            name: helper_name(name),
            sess: full.clone(),
        });
    }
    let same = hs.len() == v.sess.helpers.len()
        && hs
            .iter()
            .zip(&v.sess.helpers)
            .all(|(a, b)| a.name == b.name && Arc::ptr_eq(&a.sess, &b.sess));
    if !same {
        v.sess.helpers = hs;
    }
}

/// A run's name as a Changes row says it, short (helperName).
pub fn helper_name(s: &str) -> String {
    rush_draw::truncate(&crate::style::one_line(s), 24, "…").into_owned()
}
