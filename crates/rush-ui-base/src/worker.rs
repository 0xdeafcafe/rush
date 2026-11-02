//! The workers beside the UI thread (DESIGN.md "Threads"): discovery and
//! spend, previews, and the open conversation's tail. Each sends only what
//! changed, and waits on the files (kqueue on macOS) rather than a tick;
//! where it must poll, it polls every 2s and only while something is
//! live. After Go's ui/model.go:835-869 (loadPreview, live previews at
//! 128k, the selected one at 384k), fleet/worktree.go:427 (mainCheckout)
//! and the refresh tick of ui/model.go:1081.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rush_convo::reading::{Reading, Slice, read_on};
use rush_convo::{Options, Renderer, Session};

use crate::changes::{self, FileChange, Ver};
use rush_core::adapter::{self, Registry, global};
use rush_core::agent::{Agent, Preview};

use rush_core::event::Exchange;
use rush_core::time::{Ms, now};
use rush_draw::Sender;
use rush_fleet::{HostInfo, Loader};
use rush_sys::fswait::{Stop, Watcher};

/// What a worker tells the UI thread.
pub enum Note {
    Fleet(Fleet),
    Preview(String, Box<Preview>),
    /// What a conversation open gained, read as it grows.
    Tail {
        r#gen: u64,
        fresh: Box<Slice>,
    },
    TailFailed {
        r#gen: u64,
        err: String,
    },
    /// A conversation just opened: as much of its end as is read first,
    /// laid out as the view draws it, when it has drawn.
    End {
        r#gen: u64,
        sess: Box<Session>,
        warm: Option<Box<Warm>>,
    },
    /// A conversation's end read ahead of time, topped up with what was
    /// written since: the view's first reading (Resume::TopUp), laid
    /// out as End's is.
    Seeded {
        r#gen: u64,
        sess: Box<Session>,
        warm: Option<Box<Warm>>,
    },
    /// The whole of a conversation opened on its end, read after it, and
    /// its end laid out as the view last drew it, when it had.
    Whole {
        r#gen: u64,
        sess: Box<Session>,
        warm: Option<Box<Warm>>,
    },
    /// Work a screen handed off the UI thread, done: run with the model
    /// on it (frame/work.rs, Model::later).
    Apply(crate::frame::work::Apply),
}

/// The list's rows as discovery last read them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fleet {
    pub agents: Vec<Agent>,
    /// Each agent's repository's main checkout, or "".
    pub roots: Vec<String>,
    pub previews: HashMap<String, Preview>,
    /// Their transcripts have been priced.
    pub priced: bool,
    /// Each account's usage, today's spend and running count.
    pub accounts: Vec<rush_fleet::AccountView>,
}

/// While an agent works or has a process, how often discovery reads
/// again: Go's tick.
pub const POLL: Duration = Duration::from_secs(1);
const LIVE_WINDOW: i64 = 128 << 10;
pub const PICKED_WINDOW: i64 = 384 << 10;
/// How much of a transcript the tail reads at once: the UI applies each
/// slice in one short step, so a flood or a reread never holds up typing.
pub const TAIL_SLICE: usize = 512 << 10;
/// How much of a conversation's end is laid out ahead of the frame that
/// shows it, off the UI's thread (Go's warmRows, rushmode.go:2056).
pub const WARM_ROWS: usize = 400;

/// How a conversation's view last drew it, while it shows only the end of
/// a long one: the worker reading the whole makes it as the view will,
/// and lays out its end the same way.
pub type Drawn = Arc<std::sync::Mutex<Option<DrawnAs>>>;

/// What Drawn holds.
#[derive(Clone)]
pub struct DrawnAs {
    pub o: Options,
    /// What the list said of it.
    pub info: rush_convo::Info,
    /// The past exchanges merged into it.
    pub exchanges: Arc<[Exchange]>,
    /// Its agent has stopped: a turn its transcript leaves open is ended
    /// as the view's follow ends it (SessionView::follow).
    pub stopped: bool,
}

/// What the worker that read a whole conversation did ahead of the frame
/// that shows it: the exchanges merged into it, its end laid out, and its
/// edits worked out (the key and folder they're of, and them).
pub struct Warm {
    pub exchanges: Arc<[Exchange]>,
    pub r: Renderer,
    pub changes: (Ver, String, Arc<Vec<FileChange>>),
    /// The steps that ran an agent, and the session and version of its
    /// steps they're of (subs' Runs::spawn_steps).
    pub spawn_steps: Option<SpawnSteps>,
}

/// What a step's spawn is read with: the adapters, and the session's kind.
pub type SpawnsBy = (Registry, String);

/// The steps that ran an agent, by (serial, step version) of the session.
pub type SpawnSteps = ((u64, u64), Vec<rush_convo::StepId>);

/// Reads the list once, as `rush --dump` and `--render` do: rows, a spend
/// scan, rows again.
pub fn load_once(now: Ms) -> Fleet {
    let mut d = Discovery::new();
    d.load(now, true)
}

/// Saves each adapter's spend cache, as the view quits (Go's
/// internal/ui/keys.go:644 quit, sc.Flush(); rush_fleet::Loader::flush).
pub fn flush_spend() {
    for r in global().all().iter() {
        if let Some(s) = r.spend_reader() {
            s.spend_scanner().flush();
        }
    }
}

/// Discovery's state between readings.
pub struct Discovery {
    reg: Registry,
    loader: Loader,
    roots: HashMap<String, String>,
    /// rush's sessions as the last reading listed them: their folders are
    /// watched.
    hosted: Arc<Vec<HostInfo>>,
    /// The transcripts the last reading's rows price from, with each
    /// one's kind: the next priced reading scans these before it loads,
    /// as Go scans the last snapshot's (model.go:627 scan). Those of the
    /// rows a reading of rush's sessions alone keeps, and the rest.
    targets: Option<(Targets, Targets)>,
    /// Each live row's preview, with its transcript's size and time when
    /// read: read again only when those change.
    previews: HashMap<String, (String, u64, std::time::SystemTime, Preview)>,
    /// The last reading read only rush's sessions: nothing new to watch.
    partial: bool,
}

/// Transcripts to price, each with its agent's kind.
type Targets = Vec<(String, rush_core::adapter::SpendTarget)>;

impl Default for Discovery {
    fn default() -> Self {
        Self::new()
    }
}

impl Discovery {
    /// Discovery of every agent in rush's registry.
    /// Discovery as rush runs it: rush's own state (names, groups, what's
    /// put away or hidden) on the rows, GitHub asked about their branches.
    pub fn new() -> Discovery {
        let reg = global().clone();
        Discovery {
            loader: Loader::from_state(reg.clone()),
            reg,
            roots: HashMap::new(),
            hosted: Arc::default(),
            targets: None,
            previews: HashMap::new(),
            partial: false,
        }
    }

    pub fn with(reg: Registry) -> Discovery {
        Discovery {
            loader: Loader::new(reg.clone()),
            reg,
            roots: HashMap::new(),
            hosted: Arc::default(),
            targets: None,
            previews: HashMap::new(),
            partial: false,
        }
    }

    /// The rows; `price` scans spend first. The transcripts the last
    /// reading had are scanned before one load; only rows new to this one
    /// (or changed) are scanned after it, and read again if their spend
    /// moved: one load a reading, not two, every row as priced as before.
    /// A reading that keeps the rows that don't run (Loader::incremental)
    /// prices only the others first: a kept row's transcript written to
    /// makes the reading a whole one, which prices them all.
    pub fn load(&mut self, now: Ms, price: bool) -> Fleet {
        let prev = self.targets.take().filter(|_| price);
        let keeping = self.loader.keeping(now);
        if let Some((kept, rest)) = &prev {
            if !keeping {
                self.loader.scan_targets(kept);
            }
            self.loader.scan_targets(rest);
        }
        let mut snap = self.loader.load(now);
        if keeping
            && !snap.partial
            && let Some((kept, _)) = &prev
            && self.loader.scan_targets(kept) > 0
        {
            snap = self.loader.load(now);
        }
        if price {
            let (all, fresh) = self.targets_of(&snap, prev);
            let reread = !fresh.is_empty() && self.loader.scan_targets(&fresh) > 0;
            self.targets = Some(all);
            if reread {
                snap = self.loader.load(now);
                let (all, _) = self.targets_of(&snap, None);
                self.targets = Some(all);
            }
        }
        self.partial = snap.partial;
        let agents = snap.agents;
        self.hosted = snap.hosted;
        let roots = agents
            .iter()
            .map(|a| {
                if a.repo.is_empty() {
                    return String::new();
                }
                if !a.root.is_empty() {
                    return a.root.clone();
                }
                self.roots
                    .entry(a.repo.clone())
                    .or_insert_with(|| main_checkout(&a.repo).unwrap_or_else(|| a.repo.clone()))
                    .clone()
            })
            .collect();
        let mut previews = HashMap::new();
        for a in agents
            .iter()
            .filter(|a| a.job.live() && !a.job.transcript_path.is_empty())
        {
            let path = &a.job.transcript_path;
            let (len, at) = std::fs::metadata(path)
                .map(|m| (m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH)))
                .unwrap_or((0, std::time::UNIX_EPOCH));
            let p = match self.previews.get(&a.key) {
                Some((p0, l0, t0, p)) if p0 == path && *l0 == len && *t0 == at && len > 0 => {
                    p.clone()
                }
                _ => {
                    let p = self.reg.read_preview(a.kind.as_str(), path, LIVE_WINDOW);
                    self.previews
                        .insert(a.key.clone(), (path.clone(), len, at, p.clone()));
                    p
                }
            };
            previews.insert(a.key.clone(), p);
        }
        self.previews.retain(|k, _| previews.contains_key(k));
        Fleet {
            agents,
            roots,
            previews,
            priced: price,
            accounts: snap.accounts,
        }
    }

    /// What snap's rows price from, as the rows a reading of rush's
    /// sessions alone keeps and the rest; and those prev (the last
    /// reading's) hadn't. A reading that kept rows takes theirs from prev.
    fn targets_of(
        &self,
        snap: &rush_fleet::Snapshot,
        prev: Option<(Targets, Targets)>,
    ) -> ((Targets, Targets), Targets) {
        let kept = |a: &Agent| self.loader.kept(&a.key);
        let had_of = |t: &Targets| -> HashSet<(String, rush_core::adapter::SpendTarget)> {
            t.iter().cloned().collect()
        };
        if snap.partial
            && let Some((pk, pr)) = prev
        {
            // The rows kept are priced as the whole reading's were.
            let rest = Loader::kind_targets_of(snap.agents.iter().filter(|a| !kept(a)));
            let had: HashSet<&(String, rush_core::adapter::SpendTarget)> = pr.iter().collect();
            let fresh = rest.iter().filter(|x| !had.contains(x)).cloned().collect();
            return ((pk, rest), fresh);
        }
        let (k, r): (Vec<&Agent>, Vec<&Agent>) = snap.agents.iter().partition(|a| kept(a));
        let (k, r) = (
            Loader::kind_targets_of(k.into_iter()),
            Loader::kind_targets_of(r.into_iter()),
        );
        let fresh = match &prev {
            Some((pk, pr)) => {
                let had = had_of(pk);
                let had_r = had_of(pr);
                k.iter()
                    .chain(&r)
                    .filter(|x| !had.contains(*x) && !had_r.contains(*x))
                    .cloned()
                    .collect()
            }
            None => k.iter().chain(&r).cloned().collect(),
        };
        ((k, r), fresh)
    }

    /// The folders and files whose changes mean a new reading (what the
    /// last one read).
    fn watched(&self) -> Vec<PathBuf> {
        let mut paths = self.loader.watched();
        // rush's own sessions and state: a session starting a turn, or a
        // row renamed or hidden, wakes the list too.
        paths.extend(self.loader.rush_watched(&self.hosted));
        paths.sort();
        paths.dedup();
        paths
    }

    /// Has w watch what the last reading read. What it didn't watch yet
    /// may have changed since it was read: the next load looks again.
    fn watch(&mut self, w: &Watcher) {
        if self.partial {
            return; // no session, job or folder new since the last
        }
        let paths = self.watched();
        let was: Vec<bool> = paths.iter().map(|p| w.watching(p)).collect();
        w.watch(&paths);
        // Watched only now: read before it was. One not there isn't
        // watched yet; it's told once it appears.
        let new: Vec<PathBuf> = paths
            .iter()
            .zip(was)
            .filter(|(p, was)| !was && w.watching(p))
            .map(|(p, _)| p.clone())
            .collect();
        self.loader.told(Some(&new));
    }

    /// What it watches changed (None: it can't tell what): the adapters
    /// read those again at the next load, however lately they did.
    pub fn changed(&mut self, paths: Option<&[PathBuf]>) {
        self.loader.changed(paths);
        self.loader.told(paths);
        // rush's state changed (a row put away in Earlier, hidden or
        // renamed, here or by another rush): the next reading has it.
        let dir = rush_state::dir();
        let ours = |p: &PathBuf| {
            *p == dir
                || p.parent() == Some(dir.as_path())
                    && p.file_name()
                        .is_some_and(|n| n == "state.json" || n == "config.json")
        };
        if paths.is_none_or(|ps| ps.iter().any(ours)) {
            self.loader.set_store(rush_state::Store::load());
        }
    }
}

/// The main checkout of the repository dir is in: dir itself, or for a
/// linked worktree the checkout it came from.
pub fn main_checkout(dir: &str) -> Option<String> {
    let mut d = Path::new(dir);
    loop {
        let p = d.join(".git");
        if let Ok(m) = std::fs::metadata(&p) {
            if m.is_dir() {
                return Some(d.to_string_lossy().into_owned());
            }
            let b = std::fs::read_to_string(&p).ok()?;
            let g = b.trim().trim_start_matches("gitdir:").trim();
            let g = if g.starts_with('/') {
                g.to_string()
            } else {
                d.join(g).to_string_lossy().into_owned()
            };
            return g.find("/.git/worktrees/").map(|i| g[..i].to_string());
        }
        d = d.parent()?;
        if d.as_os_str() == "/" {
            return None;
        }
    }
}

/// What the UI holds of discovery: the loader's handle, to put past
/// conversations back when it started with them left out (hosted's
/// SkipPast), and a way to wake the reading thread for a reading now (Go's
/// refresh, and everyAgent: SkipPast(false), then refresh).
#[derive(Clone)]
pub struct DiscoveryCtl {
    handle: rush_fleet::Handle,
    again: Arc<Again>,
}

/// Wakes discovery's reading thread for a reading now (Go's m.refresh):
/// what it waits on is closed, and asked while it reads, the next wait
/// doesn't.
#[derive(Default)]
pub struct Again {
    /// The wait now; a closed Stop stays so, so each wait has its own.
    stop: std::sync::Mutex<Option<Arc<Stop>>>,
    /// Asked for while no wait was on.
    wanted: AtomicBool,
}

impl Again {
    /// Asks for a reading now.
    pub fn ask(&self) {
        let g = self.stop.lock().unwrap_or_else(|e| e.into_inner());
        self.wanted.store(true, Ordering::SeqCst);
        if let Some(s) = g.as_ref() {
            s.close();
        }
    }

    /// The next wait's Stop, closed already when a reading was asked for
    /// since the last.
    fn next(&self) -> Option<Arc<Stop>> {
        let mut g = self.stop.lock().unwrap_or_else(|e| e.into_inner());
        let s = Arc::new(Stop::new().ok()?);
        if self.wanted.swap(false, Ordering::SeqCst) {
            s.close();
        }
        *g = Some(s.clone());
        Some(s)
    }

    /// Whether a reading has been asked for and not yet begun.
    pub fn asked(&self) -> bool {
        self.wanted.load(Ordering::SeqCst)
    }
}

/// What the UI hands discovery's loader as it reads (Go's m.loader): a
/// spawned agent's row linked to the session that ran it, and readings
/// asked for. None until discovery has started, and with none running
/// (tests, --render).
#[derive(Default)]
pub struct FleetIn(pub Option<rush_fleet::Handle>, pub Option<Arc<Again>>);

impl FleetIn {
    /// Lists the agent with row key child under the session with row key
    /// parent from now on (LinkSpawn).
    pub fn link_spawn(&self, child: &str, parent: &str) {
        if let Some(h) = &self.0 {
            h.link_spawn(child, parent);
        }
    }

    /// Asks for a new reading of the fleet now (refresh).
    pub fn refresh(&self) {
        if let Some(a) = &self.1 {
            a.ask();
        }
    }
}

impl DiscoveryCtl {
    /// The loader's handle, for what the UI hands it.
    pub fn handle(&self) -> rush_fleet::Handle {
        self.handle.clone()
    }

    /// What asks the reading thread for a reading now.
    pub fn again(&self) -> Arc<Again> {
        self.again.clone()
    }

    /// Puts past conversations back in the readings, at once.
    pub fn every_agent(&self) {
        self.handle.skip_past(false);
        self.again.ask();
    }
}

/// Starts discovery: a reading now, priced at once, then again whenever
/// the agents' folders change, or every POLL while an agent works (an
/// idle terminal's own files say when it starts again). A reading is sent
/// only when it differs from the last one sent.
pub fn spawn_discovery(tx: Sender<Note>) {
    spawn_discovery_with(tx, false, None);
}

/// Discovery as [`spawn_discovery`]; `ctl` gets the loader's handle,
/// before the first reading is sent. With skip_past, past conversations
/// are left out until `ctl` is asked for every agent (hosted's
/// loader.SkipPast(true), NewHosted).
pub fn spawn_discovery_with(
    tx: Sender<Note>,
    skip_past: bool,
    ctl: Option<Box<dyn FnOnce(DiscoveryCtl) + Send>>,
) {
    // One reading waits at most: the next is read once it's taken.
    let tx = tx.lane(1);
    let _ = std::thread::Builder::new()
        .name("rush-discovery".into())
        .spawn(move || {
            let mut d = Discovery::new();
            // The spend caches read beside the first reading, which shows
            // their costs (Loader::spend_of) before the first pricing.
            let reg = d.reg.clone();
            let _ = std::thread::Builder::new()
                .name("rush-spend-cache".into())
                .spawn(move || {
                    for r in reg.all().iter() {
                        if let Some(s) = r.spend_reader() {
                            s.spend_scanner().prepare();
                        }
                    }
                });
            let again = ctl.map(|give| {
                let handle = d.loader.handle();
                handle.skip_past(skip_past);
                let again = Arc::new(Again::default());
                give(DiscoveryCtl {
                    handle,
                    again: again.clone(),
                });
                again
            });
            // Many: a thousand session folders are one stream, not a
            // thousand descriptors. The loader trusts what it says hasn't
            // changed: a session's folder unchanged, its info isn't looked
            // at again.
            let w = Arc::new(Watcher::many());
            d.loader.watch_shared(w.clone());
            // A reading while agents work makes rush's sessions again and
            // keeps the rest, while nothing else changed.
            d.loader.incremental(true);
            let first = d.load(now(), false);
            // Watched before each reading after, so a change during it
            // still wakes; the first says what there is to watch.
            d.watch(&w);
            let mut last = first.clone();
            if !tx.send(Note::Fleet(first)) {
                return;
            }
            loop {
                let began = std::time::Instant::now();
                let f = d.load(now(), true);
                // Every second while any agent works or has a process, as
                // Go's tick reads and samples them (model.go tickMsg).
                let working = f.agents.iter().any(|a| a.job.live());
                let procs = f.agents.iter().any(|a| a.pid != 0);
                if f != last {
                    last = f.clone();
                    if !tx.send(Note::Fleet(f)) {
                        return;
                    }
                }
                // Woken for a reading asked for (refresh, everyAgent): a
                // fresh Stop each wait, as a closed one stays so.
                let stop = again.as_ref().and_then(|a| a.next());
                let (paths, all) = loop {
                    w.wait(stop.as_deref(), (working || procs).then_some(POLL));
                    // What changed: the adapters read it again, whatever
                    // they kept (a project folder listed a second ago
                    // included).
                    let (mut paths, mut all) = w.changes();
                    if !all && !urgent(&paths) && gap(began, stop.as_deref()) {
                        // A busy agent's files, changing on and on: read
                        // at most once a GAP, as Go reads on its tick.
                        let (more, a) = w.changes();
                        paths.extend(more);
                        all |= a;
                    }
                    let asked = stop.as_ref().is_some_and(|s| s.is_closed());
                    if working || asked || all || !paths.is_empty() {
                        break (paths, all);
                    }
                    // Woken by the clock alone with no agent at work:
                    // nothing new on disk, so the last reading with its
                    // processes sampled again and nothing read (Go's tick,
                    // loader.Settle): their CPU and memory as Go shows them.
                    let idx: Vec<usize> = (0..last.agents.len())
                        .filter(|&i| last.agents[i].pid != 0)
                        .collect();
                    let mut few: Vec<Agent> = idx.iter().map(|&i| last.agents[i].clone()).collect();
                    d.loader.resample(&mut few);
                    let moved = few.iter().zip(&idx).any(|(a, &i)| {
                        let b = &last.agents[i];
                        (a.mem, a.cpu, a.procs) != (b.mem, b.cpu, b.procs)
                    });
                    if moved {
                        for (a, &i) in few.into_iter().zip(&idx) {
                            last.agents[i] = a;
                        }
                        if !tx.send(Note::Fleet(last.clone())) {
                            return;
                        }
                    }
                };
                let paths: Vec<PathBuf> = paths.into_iter().collect();
                d.changed((!all).then_some(paths.as_slice()));
                // New jobs or project folders: watch them too.
                d.watch(&w);
            }
        });
}

/// The least time between two readings that follow a busy agent's files
/// (a transcript growing, a host's info rewritten each step): Go reads the
/// list once a second, on its tick (model.go tickMsg).
pub const GAP: Duration = Duration::from_secs(1);

/// What changed that's read at once, however lately the last reading was:
/// rush's state (something you did: a row renamed, hidden, put away) and
/// its sessions folder (a session started or gone).
fn urgent(paths: &HashSet<PathBuf>) -> bool {
    if paths.is_empty() {
        return false;
    }
    let dir = rush_state::dir();
    let root = rush_fleet::host_root();
    paths
        .iter()
        .any(|p| *p == dir || p.parent() == Some(dir.as_path()) || *p == root)
}

/// Waits until GAP has passed since a reading began, unless stop closes
/// first; whether it waited at all. One sleep, woken by stop's close (it
/// napped 50 ms at a time: 20 wakes a second while an agent worked).
fn gap(began: std::time::Instant, stop: Option<&Stop>) -> bool {
    let left = (began + GAP).saturating_duration_since(std::time::Instant::now());
    if left.is_zero() || stop.is_some_and(Stop::is_closed) {
        return false;
    }
    match stop {
        Some(s) => {
            s.wait(left);
        }
        None => std::thread::sleep(left),
    }
    true
}

/// Reads one agent's preview off the UI thread.
pub fn spawn_preview(tx: Sender<Note>, reg: Registry, kind: String, key: String, path: String) {
    let tx = tx.lane(1);
    let _ = std::thread::Builder::new()
        .name("rush-preview".into())
        .spawn(move || {
            let p = reg.read_preview(&kind, &path, PICKED_WINDOW);
            tx.send(Note::Preview(key, Box::new(p)));
        });
}

/// The tail of the conversation open: stops when dropped.
pub struct TailHandle {
    stop: Arc<AtomicBool>,
    // Closing it wakes the worker's wait.
    wake: Arc<Stop>,
}

impl Drop for TailHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.close();
    }
}

/// How much of a transcript's end is read first, before the whole of a
/// long one.
pub const TAIL_END: u64 = 2 << 20;

/// Where a conversation's worker starts.
pub enum Open {
    /// Where it's read from, and for a stopped rush session its host (its
    /// folder says where the history is): the reading is made on the
    /// worker, as it may read the disk.
    Source(crate::session::Source, String),
    /// A reading made already: one read ahead (list_extras/predict.rs).
    Reading(Box<dyn Reading>),
}

/// How a conversation's reading goes on from what was read of it before
/// the view opened.
pub enum Resume {
    /// Nothing read yet: its end first, then the whole.
    Start,
    /// The view shows its end, read ahead, the reading still at its start:
    /// the whole is read and takes the end's place, then it's followed.
    Whole,
    /// The view shows all of it, read ahead with nothing written since:
    /// followed from where that read stopped.
    Follow,
    /// All of it was read ahead but written to since, which the view
    /// doesn't have yet: topped up here and sent whole (Note::Seeded).
    TopUp(Box<Session>),
}

/// Reads a conversation, whatever agent wrote it (rush_convo::reading):
/// its last 2 MB first, found from the end backwards, sent for the view
/// to show at once (Note::End, "reading earlier…" above it); then the
/// whole from its start, here, laid out as the view last drew, to take the
/// end's place (Note::Whole; a short one's whole is its first reading,
/// Note::End); then what's added as it grows, a TAIL_SLICE at a time
/// (Note::Tail), parsed here for the UI thread to apply, the next read
/// while it does. Stops when the handle is dropped.
pub fn spawn_read(
    open: Open,
    resume: Resume,
    r#gen: u64,
    tx: Sender<Note>,
    drawn: Drawn,
    spawns: Option<SpawnsBy>,
) -> Option<TailHandle> {
    // One slice waits at most: the next is read while the UI applies it.
    let tx = tx.lane(1);
    let wake = Arc::new(Stop::new().ok()?);
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let wk = wake.clone();
    std::thread::Builder::new()
        .name("rush-read".into())
        .spawn(move || {
            let failed = |e: adapter::Error| {
                tx.send(Note::TailFailed {
                    r#gen,
                    err: e.to_string(),
                })
            };
            let mut r = match open {
                Open::Reading(r) => r,
                Open::Source(src, host) => match crate::session::reading(&src, &host) {
                    Ok(r) => r,
                    Err(e) => {
                        failed(e);
                        return;
                    }
                },
            };
            r.stop_on(st.clone());
            let says = r.says_itself();
            let warm_of = |s: &mut Session| warm(s, &drawn, spawns.as_ref(), says);
            let gone = || st.load(Ordering::Relaxed);
            match resume {
                Resume::Start => {
                    // Its end, laid out as the view draws it before the
                    // view has it (Go's warmed, rushmode.go:2064): the
                    // frame that shows it draws nothing new.
                    let shown = match r.end(TAIL_END) {
                        Some(mut end) if !gone() => {
                            let warm = warm_of(&mut end);
                            if !tx.send(Note::End {
                                r#gen,
                                sess: Box::new(end),
                                warm,
                            }) {
                                return;
                            }
                            true
                        }
                        _ => false,
                    };
                    if gone() {
                        return;
                    }
                    // Then the whole, here, taken in where the end was
                    // (Go's readWhole, twopass.go:33); a short one's is
                    // its first reading.
                    let mut whole = Session::new();
                    let read = read_on(r.as_mut(), &mut whole, TAIL_SLICE, &st);
                    if gone() {
                        return;
                    }
                    let mut err = read.err();
                    if shown && let Some(e) = err.take() {
                        // What was read of the whole is gone with it: the
                        // end stays as it is.
                        failed(e);
                        return;
                    }
                    let warm = warm_of(&mut whole);
                    let sess = Box::new(whole);
                    let sent = if shown {
                        tx.send(Note::Whole { r#gen, sess, warm })
                    } else {
                        tx.send(Note::End { r#gen, sess, warm })
                    };
                    if !sent {
                        return;
                    }
                    if let Some(e) = err {
                        failed(e);
                    }
                }
                Resume::Whole => {
                    let mut whole = Session::new();
                    match read_on(r.as_mut(), &mut whole, TAIL_SLICE, &st) {
                        Ok(true) => {
                            let warm = warm_of(&mut whole);
                            if !tx.send(Note::Whole {
                                r#gen,
                                sess: Box::new(whole),
                                warm,
                            }) {
                                return;
                            }
                        }
                        Ok(false) => return,
                        Err(e) => {
                            failed(e);
                            return;
                        }
                    }
                }
                Resume::Follow => {}
                Resume::TopUp(mut sess) => match read_on(r.as_mut(), &mut sess, TAIL_SLICE, &st) {
                    Ok(true) => {
                        let warm = warm_of(&mut sess);
                        if !tx.send(Note::Seeded { r#gen, sess, warm }) {
                            return;
                        }
                    }
                    Ok(false) => return,
                    Err(e) => {
                        failed(e);
                        return;
                    }
                },
            }
            // What's added from here on, watched before each read so a
            // write is never missed (fswait::follow_grows).
            rush_sys::fswait::follow_grows(r.as_mut(), TAIL_SLICE, &wk, &mut |res| match res {
                Ok(fresh) => tx.send(Note::Tail {
                    r#gen,
                    fresh: Box::new(fresh),
                }),
                Err(e) => failed(e),
            });
        })
        .ok()?;
    Some(TailHandle { stop, wake })
}

/// The end of s laid out as its view last drew (drawn), WARM_ROWS of it,
/// and its edits worked out: the frame that shows it then finds those
/// turns drawn, rather than drawing a long conversation's end while you
/// wait (Go's warmed, rushmode.go:2064). None when the view hasn't drawn
/// yet. s takes what the list said of it, as the view's will.
fn warm(
    s: &mut Session,
    drawn: &Drawn,
    spawns: Option<&SpawnsBy>,
    says: bool,
) -> Option<Box<Warm>> {
    let DrawnAs {
        mut o,
        info,
        exchanges,
        stopped,
    } = drawn.lock().ok()?.clone()?;
    if o.width == 0 {
        return None;
    }
    o.now = now();
    s.info = crate::session::info_over(says, std::mem::take(&mut s.info), info);
    // As the view merges them once the whole is in (take_whole_drawn), and
    // ends the turn its stopped agent left open (follow): done after, they
    // would change what was laid out here.
    if !exchanges.is_empty() {
        s.restore_exchanges(&exchanges);
    }
    if stopped && !says && s.live().is_some() {
        s.apply(&rush_core::event::Event::TurnEnd(end_left_open()), o.now);
    }
    let mut r = Renderer::new();
    r.tail(s, &o, WARM_ROWS);
    let changes = (
        changes::memo_key_of(s, &o.kids),
        s.info.cwd.clone(),
        Arc::new(changes::changes_with(s, &o.kids)),
    );
    // The steps that ran an agent, as the dock's runs work them out
    // (subs::refresh): every step looked at once.
    let spawn_steps = spawns.map(|(reg, kind)| {
        let ids = s
            .turns
            .iter()
            .flat_map(|t| &t.items)
            .filter_map(|it| match it {
                rush_convo::Item::Step(id) => Some(*id),
                _ => None,
            })
            .filter(|id| crate::spawns::step_spawn(reg, s.step(*id), kind).is_some())
            .collect();
        ((s.serial(), s.step_ver()), ids)
    });
    Some(Box::new(Warm {
        exchanges,
        r,
        changes,
        spawn_steps,
    }))
}

/// How a turn a stopped agent's transcript leaves open is ended.
pub fn end_left_open() -> rush_core::event::TurnEnd {
    rush_core::event::TurnEnd {
        reason: "done".into(),
        ..Default::default()
    }
}

/// Session s as its history is read: a stopped rush session's folder is
/// its host's to say, so a Codex thread known by its id alone is looked
/// for there (openHistory, history.go:53). Reads the disk.
pub fn history_session(mut s: adapter::Session, host: &str) -> adapter::Session {
    if s.profile.dir.is_empty()
        && !host.is_empty()
        && let Ok(cfg) = rush_host::client::read_config(host)
    {
        s.profile.dir = cfg.account.dir;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Source;
    use rush_core::adapter::AdapterRef;
    use rush_core::event::Event;

    /// Opens src on a worker as a view does, its end first.
    fn open(src: Source, host: &str, tx: Sender<Note>) -> TailHandle {
        spawn_read(
            Open::Source(src, host.into()),
            Resume::Start,
            1,
            tx,
            Drawn::default(),
            None,
        )
        .unwrap()
    }

    /// Another agent's history, through ad.
    fn history_of(ad: AdapterRef) -> Source {
        Source::History {
            adapter: ad,
            session: Box::default(),
        }
    }

    /// The gap between a busy agent's readings is one wait its stop
    /// ends: closed before or while it waits, it returns then, not at
    /// GAP's end; a gap already over doesn't wait.
    #[test]
    fn gap_ends_with_its_stop() {
        let began = std::time::Instant::now();
        let stop = Arc::new(Stop::new().unwrap());
        let s2 = stop.clone();
        let t = std::thread::spawn(move || s2.close());
        gap(began, Some(&stop));
        t.join().unwrap();
        assert!(began.elapsed() < GAP, "{:?}", began.elapsed());
        assert!(!gap(began, Some(&stop)), "closed: no wait");
        assert!(!gap(began - GAP, None), "over: no wait");
    }

    /// A transcript in the form Claude Code writes.
    fn transcript_of(p: &Path) -> Source {
        Source::Transcript {
            path: p.to_path_buf(),
            decode: rush_convo::decode(rush_claude::messages),
        }
    }

    /// The worker makes the whole as the view will once it's in: the turn
    /// a stopped agent left open ended, the past exchanges merged; and
    /// lays out its end for that, not for what the view would change.
    #[test]
    fn warm_makes_the_whole_as_the_view_will() {
        let drawn = Drawn::default();
        let mut s = Session::new();
        s.sent("go".into(), Vec::new(), 1000);
        assert!(
            warm(&mut s, &drawn, None, false).is_none(),
            "nothing drawn yet"
        );
        let as_drawn = |stopped| DrawnAs {
            o: Options {
                width: 80,
                ..Options::default()
            },
            info: rush_convo::Info::default(),
            exchanges: Arc::default(),
            stopped,
        };
        *drawn.lock().unwrap() = Some(as_drawn(false));
        assert!(warm(&mut s, &drawn, None, false).is_some());
        assert!(s.live().is_some(), "a running agent's turn stays open");
        *drawn.lock().unwrap() = Some(as_drawn(true));
        let w = warm(&mut s, &drawn, None, false).unwrap();
        assert!(s.live().is_none(), "a stopped agent's turn ended");
        assert!(w.r.kept_rows() > 0);
    }

    #[test]
    fn main_checkout_of_a_worktree() {
        let t = std::env::temp_dir().join(format!("rush-ui-mc-{}", std::process::id()));
        let main = t.join("main");
        let tree = t.join("trees").join("x");
        std::fs::create_dir_all(main.join(".git").join("worktrees").join("x")).unwrap();
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(
            tree.join(".git"),
            format!("gitdir: {}/.git/worktrees/x\n", main.display()),
        )
        .unwrap();
        let m = main.to_string_lossy().into_owned();
        assert_eq!(main_checkout(&m).as_deref(), Some(m.as_str()));
        assert_eq!(
            main_checkout(&tree.join("sub").to_string_lossy()).as_deref(),
            Some(m.as_str())
        );
        std::fs::remove_dir_all(&t).unwrap();
    }

    /// Another adapter's history is read on a worker: its first reading
    /// arrives as the end, and the worker stops when dropped.
    #[test]
    fn history_is_followed() {
        use rush_core::adapter::{
            Adapter, Features, Follow, HistoryFollower, Kind, Level, Profile, Slice, WaitOn,
        };
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        struct Hist;
        impl Adapter for Hist {
            fn kind(&self) -> Kind {
                Kind::from("histworker")
            }
            fn name(&self) -> String {
                "Hist".into()
            }
            fn features(&self) -> Features {
                Features::default()
            }
            fn level(&self) -> Level {
                Level::default()
            }
            fn profiles(&self) -> Vec<Profile> {
                Vec::new()
            }
            rush_core::has!(history_follower: HistoryFollower);
        }
        struct Once(bool);
        impl Follow for Once {
            fn next(&mut self, _: usize) -> adapter::Result<Slice> {
                let first = std::mem::replace(&mut self.0, false);
                Ok(Slice {
                    events: if first {
                        vec![Event::Status(rush_core::event::Status::default())]
                    } else {
                        Vec::new()
                    },
                    ..Slice::default()
                })
            }
            fn wait_on(&self) -> WaitOn {
                WaitOn::default()
            }
        }
        impl HistoryFollower for Hist {
            fn follow_history(&self, _: &adapter::Session) -> adapter::Result<Box<dyn Follow>> {
                Ok(Box::new(Once(true)))
            }
        }
        struct Notes(usize);
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                if let Msg::App(Note::End { .. }) = m {
                    self.0 += 1;
                    return Step::Quit;
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        let lp: Loop<Note> = Loop::new();
        let h = open(history_of(Arc::new(Hist)), "", lp.sender());
        let mut app = Notes(0);
        lp.run_with(&mut app, |_| Ok(())).unwrap();
        assert_eq!(app.0, 1);
        drop(h); // its wait ends with the handle
    }

    /// A history that can only be read once is read whole, then sent as
    /// its first reading; one cut short by an error is sent as far as it
    /// got, and the error told.
    #[test]
    fn history_read_once_says_its_last_slice() {
        use rush_core::adapter::{Adapter, Features, HistoryReader, Kind, Level, Profile};
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        struct Hist(bool);
        impl Adapter for Hist {
            fn kind(&self) -> Kind {
                Kind::from("histonce")
            }
            fn name(&self) -> String {
                "Hist".into()
            }
            fn features(&self) -> Features {
                Features::default()
            }
            fn level(&self) -> Level {
                Level::default()
            }
            fn profiles(&self) -> Vec<Profile> {
                Vec::new()
            }
            rush_core::has!(history_reader: HistoryReader);
        }
        impl HistoryReader for Hist {
            fn history(
                &self,
                _: &adapter::Session,
                _: Ms,
                _: usize,
                each: &mut dyn FnMut(Vec<Event>) -> bool,
            ) -> adapter::Result<()> {
                for _ in 0..3 {
                    each(vec![Event::Status(rush_core::event::Status::default())]);
                }
                if self.0 {
                    return Err("cut short".into());
                }
                Ok(())
            }
        }
        struct Notes(Vec<&'static str>, bool);
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                match m {
                    Msg::App(Note::End { .. }) => {
                        self.0.push("end");
                        if !self.1 {
                            return Step::Quit;
                        }
                    }
                    Msg::App(Note::TailFailed { .. }) => {
                        self.0.push("failed");
                        return Step::Quit;
                    }
                    _ => {}
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        for (fails, want) in [(false, vec!["end"]), (true, vec!["end", "failed"])] {
            let lp: Loop<Note> = Loop::new();
            let h = open(history_of(Arc::new(Hist(fails))), "", lp.sender());
            let mut app = Notes(Vec::new(), fails);
            lp.run_with(&mut app, |_| Ok(())).unwrap();
            assert_eq!(app.0, want, "fails={fails}");
            drop(h);
        }
    }

    /// A long history whose adapter reads its end alone shows that end
    /// first (partial), then the whole takes its place; one whose end is
    /// all of it is read as before.
    #[test]
    fn a_long_history_shows_its_end_first() {
        use rush_core::adapter::{
            Adapter, Features, HistoryReader, Kind, Level, Profile, TailReader,
        };
        use rush_core::event::{Message, Part, PartKind};
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        fn prompt(t: &str) -> Event {
            Event::Message(Message {
                role: "user".into(),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text: t.into(),
                    ..Part::default()
                }],
                ..Message::default()
            })
        }
        struct Long(bool);
        impl Adapter for Long {
            fn kind(&self) -> Kind {
                Kind::from("histlong")
            }
            fn name(&self) -> String {
                "Long".into()
            }
            fn features(&self) -> Features {
                Features::default()
            }
            fn level(&self) -> Level {
                Level::default()
            }
            fn profiles(&self) -> Vec<Profile> {
                Vec::new()
            }
            rush_core::has!(history_reader: HistoryReader, tail_reader: TailReader);
        }
        impl HistoryReader for Long {
            fn history(
                &self,
                _: &adapter::Session,
                _: Ms,
                _: usize,
                each: &mut dyn FnMut(Vec<Event>) -> bool,
            ) -> adapter::Result<()> {
                let done = || Event::TurnEnd(end_left_open());
                each(vec![prompt("one"), done(), prompt("two"), done()]);
                each(vec![prompt("three"), done()]);
                Ok(())
            }
        }
        impl TailReader for Long {
            fn history_tail(
                &self,
                _: &adapter::Session,
                most: i64,
            ) -> adapter::Result<(Vec<Event>, bool)> {
                assert_eq!(most, TAIL_END as i64);
                Ok((
                    vec![prompt("three"), Event::TurnEnd(end_left_open())],
                    self.0,
                ))
            }
        }
        #[derive(Default)]
        struct Notes(Vec<(&'static str, usize, bool)>);
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                match m {
                    Msg::App(Note::End { sess, .. }) => {
                        self.0.push(("end", sess.turns.len(), sess.partial));
                        if !sess.partial {
                            return Step::Quit;
                        }
                    }
                    Msg::App(Note::Whole { sess, .. }) => {
                        self.0.push(("whole", sess.turns.len(), sess.partial));
                        return Step::Quit;
                    }
                    Msg::App(Note::TailFailed { .. }) => return Step::Quit,
                    _ => {}
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        for (cut, want) in [
            (true, vec![("end", 1, true), ("whole", 3, false)]),
            (false, vec![("end", 3, false)]),
        ] {
            let lp: Loop<Note> = Loop::new();
            let h = open(history_of(Arc::new(Long(cut))), "", lp.sender());
            let mut app = Notes::default();
            lp.run_with(&mut app, |_| Ok(())).unwrap();
            assert_eq!(app.0, want, "cut={cut}");
            drop(h);
        }
    }

    /// A stopped rush session's history is looked for in the folder its
    /// host saved when the list doesn't say (openHistory, history.go:53):
    /// with none, Codex would look for sessions/ where rush runs.
    #[test]
    fn a_rush_sessions_history_is_read_from_its_hosts_folder() {
        use rush_core::adapter::{Adapter, Features, HistoryReader, Kind, Level, Profile};
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        struct Hist;
        impl Adapter for Hist {
            fn kind(&self) -> Kind {
                Kind::from("histdir")
            }
            fn name(&self) -> String {
                "Hist".into()
            }
            fn features(&self) -> Features {
                Features::default()
            }
            fn level(&self) -> Level {
                Level::default()
            }
            fn profiles(&self) -> Vec<Profile> {
                Vec::new()
            }
            rush_core::has!(history_reader: HistoryReader);
        }
        impl HistoryReader for Hist {
            fn history(
                &self,
                s: &adapter::Session,
                _: Ms,
                _: usize,
                _: &mut dyn FnMut(Vec<Event>) -> bool,
            ) -> adapter::Result<()> {
                Err(format!("read in {:?}", s.profile.dir).into())
            }
        }
        struct Said(String);
        impl App<Note> for Said {
            fn update(&mut self, m: Msg<Note>) -> Step {
                if let Msg::App(Note::TailFailed { err, .. }) = m {
                    self.0 = err;
                    return Step::Quit;
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        let id = format!("hist-dir-{}", std::process::id());
        let d = rush_host::dir(&id);
        std::fs::create_dir_all(&d).unwrap();
        let mut cfg = rush_host::Config::default();
        cfg.account.dir = "/profiles/work".into();
        std::fs::write(d.join("config.json"), cfg.to_json_indent()).unwrap();
        for (host, want) in [(id.as_str(), "/profiles/work"), ("", "")] {
            let lp: Loop<Note> = Loop::new();
            let h = open(history_of(Arc::new(Hist)), host, lp.sender());
            let mut app = Said(String::new());
            lp.run_with(&mut app, |_| Ok(())).unwrap();
            assert_eq!(app.0, format!("read in {want:?}"), "host {host:?}");
            drop(h);
        }
        let _ = std::fs::remove_dir_all(d);
    }

    /// The open conversation's tail reads what's there, then wakes for
    /// what's appended (fswait's Watcher), and stops when dropped.
    #[test]
    fn the_tail_follows_appends() {
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        let d = std::env::temp_dir().join(format!("rush-ui-tail-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("t.jsonl");
        let line = |t: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-09-29T10:00:00Z","message":{{"id":"{t}","role":"assistant","content":[{{"type":"text","text":"{t}"}}]}}}}"#
            ) + "\n"
        };
        std::fs::write(&p, line("one")).unwrap();
        struct Notes {
            n: usize,
            path: PathBuf,
            more: String,
        }
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                // The first read comes as the end, what's added after as
                // slices.
                if let Msg::App(Note::End { .. } | Note::Tail { .. }) = m {
                    self.n += 1;
                    if self.n == 1 {
                        use std::io::Write;
                        std::fs::OpenOptions::new()
                            .append(true)
                            .open(&self.path)
                            .unwrap()
                            .write_all(self.more.as_bytes())
                            .unwrap();
                    } else {
                        return Step::Quit;
                    }
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        let lp: Loop<Note> = Loop::new();
        let h = open(transcript_of(&p), "", lp.sender());
        let mut app = Notes {
            n: 0,
            path: p.clone(),
            more: line("two"),
        };
        lp.run_with(&mut app, |_| Ok(())).unwrap();
        assert_eq!(app.n, 2, "the append wasn't read");
        drop(h);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A conversation's end read ahead of time is topped up with what was
    /// written since, from where that read stopped, and sent whole before
    /// anything else; one handed on as it was read sends only what's new.
    #[test]
    fn a_tail_read_ahead_is_topped_up() {
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        let d = std::env::temp_dir().join(format!("rush-ui-ahead-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("t.jsonl");
        let ask = |i: usize| {
            format!(
                r#"{{"type":"user","timestamp":"2026-09-29T10:00:00Z","message":{{"role":"user","content":"ask {i}"}}}}"#
            ) + "\n"
        };
        let append = |p: &PathBuf, s: String| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(p)
                .unwrap()
                .write_all(s.as_bytes())
                .unwrap();
        };
        let decode = rush_convo::decode(rush_claude::messages);
        let ahead = || {
            let mut t = rush_convo::reading::Transcript::new(&p, decode.clone());
            let mut s = Session::new();
            read_on(&mut t, &mut s, TAIL_SLICE, &AtomicBool::new(false)).unwrap();
            (t, s)
        };
        /// Each note's turns: Seeded's as sent, Tail's applied alone.
        struct Notes(Vec<(&'static str, usize)>, PathBuf, String);
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                let n = match m {
                    Msg::App(Note::Seeded { sess, .. }) => ("seeded", sess.turns.len()),
                    Msg::App(Note::Tail { fresh, .. }) => {
                        let mut s = Session::new();
                        fresh.apply(&mut s);
                        ("tail", s.turns.len())
                    }
                    _ => return Step::Idle,
                };
                self.0.push(n);
                if self.0.len() == 1 && !self.2.is_empty() {
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .append(true)
                        .open(&self.1)
                        .unwrap()
                        .write_all(std::mem::take(&mut self.2).as_bytes())
                        .unwrap();
                    return Step::Idle;
                }
                Step::Quit
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        // Written to since: topped up and sent whole, then followed.
        std::fs::write(&p, (0..3).map(ask).collect::<String>()).unwrap();
        let (t, s) = ahead();
        assert_eq!(s.turns.len(), 3);
        append(&p, ask(3) + &ask(4));
        let lp: Loop<Note> = Loop::new();
        let h = spawn_read(
            Open::Reading(Box::new(t)),
            Resume::TopUp(Box::new(s)),
            1,
            lp.sender(),
            Drawn::default(),
            None,
        )
        .unwrap();
        let mut app = Notes(Vec::new(), p.clone(), ask(5));
        lp.run_with(&mut app, |_| Ok(())).unwrap();
        assert_eq!(app.0, vec![("seeded", 5), ("tail", 1)]);
        drop(h);
        // As it was read: nothing sent until something is written.
        std::fs::write(&p, (0..3).map(ask).collect::<String>()).unwrap();
        let (t, _) = ahead();
        let lp: Loop<Note> = Loop::new();
        let h = spawn_read(
            Open::Reading(Box::new(t)),
            Resume::Follow,
            1,
            lp.sender(),
            Drawn::default(),
            None,
        )
        .unwrap();
        append(&p, ask(3));
        let mut app = Notes(Vec::new(), p.clone(), String::new());
        lp.run_with(&mut app, |_| Ok(())).unwrap();
        assert_eq!(app.0, vec![("tail", 1)]);
        drop(h);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A history read ahead goes on from where its reading stopped: what
    /// came since is taken into it and sent whole first, then followed.
    #[test]
    fn a_history_read_ahead_is_topped_up() {
        use rush_convo::reading::{Got, Slice};
        use rush_core::adapter::WaitOn;
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        struct Feed(Vec<(bool, bool)>);
        impl Reading for Feed {
            fn end(&mut self, _: u64) -> Option<Session> {
                None
            }
            fn next(&mut self, _: usize) -> adapter::Result<Slice> {
                let (some, more) = if self.0.is_empty() {
                    (false, false)
                } else {
                    self.0.remove(0)
                };
                let events = if some {
                    vec![Event::Status(rush_core::event::Status::default())]
                } else {
                    Vec::new()
                };
                Ok(Slice {
                    got: Got::Events {
                        events,
                        reset: false,
                    },
                    more,
                })
            }
            fn wait_on(&self) -> WaitOn {
                WaitOn {
                    every: Some(Duration::from_millis(5)),
                    ..WaitOn::default()
                }
            }
            fn says_itself(&self) -> bool {
                true
            }
        }
        struct Notes(Vec<&'static str>);
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                match m {
                    Msg::App(Note::Seeded { .. }) => self.0.push("seeded"),
                    Msg::App(Note::Tail { .. }) => {
                        self.0.push("tail");
                        return Step::Quit;
                    }
                    _ => {}
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        let lp: Loop<Note> = Loop::new();
        // Two slices written since the read; then one more as it follows.
        let f = Feed(vec![
            (true, true),
            (true, false),
            (false, false),
            (true, false),
        ]);
        let h = spawn_read(
            Open::Reading(Box::new(f)),
            Resume::TopUp(Box::default()),
            1,
            lp.sender(),
            Drawn::default(),
            None,
        )
        .unwrap();
        let mut app = Notes(Vec::new());
        lp.run_with(&mut app, |_| Ok(())).unwrap();
        assert_eq!(app.0, vec!["seeded", "tail"]);
        drop(h);
    }

    /// A transcript longer than the end a session opens on is read whole
    /// after it, and sent to take its place (readWhole, twopass.go).
    #[test]
    fn the_whole_follows_the_end() {
        use rush_draw::{App, Canvas, Loop, Msg, Step};
        let d = std::env::temp_dir().join(format!("rush-ui-whole-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("t.jsonl");
        let pad = "x".repeat(4000);
        let mut body = String::new();
        for i in 0..700 {
            body += &format!(
                r#"{{"type":"user","timestamp":"2026-09-29T10:00:00Z","message":{{"role":"user","content":"ask {i} {pad}"}}}}"#
            );
            body.push('\n');
        }
        std::fs::write(&p, &body).unwrap();
        assert!(body.len() > 2 << 20, "longer than the end read first");
        struct Notes {
            tails: usize,
            whole: Option<Box<Session>>,
        }
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                match m {
                    Msg::App(Note::Tail { fresh, .. }) => {
                        let mut s = Session::new();
                        fresh.apply(&mut s);
                        assert!(s.partial, "the end first");
                        self.tails += 1;
                        Step::Idle
                    }
                    Msg::App(Note::End { sess, .. }) => {
                        assert!(sess.partial, "the end first");
                        self.tails += 1;
                        Step::Idle
                    }
                    Msg::App(Note::Whole { sess, .. }) => {
                        self.whole = Some(sess);
                        Step::Quit
                    }
                    _ => Step::Idle,
                }
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        let lp: Loop<Note> = Loop::new();
        let h = open(transcript_of(&p), "", lp.sender());
        let mut app = Notes {
            tails: 0,
            whole: None,
        };
        lp.run_with(&mut app, |_| Ok(())).unwrap();
        let whole = app.whole.expect("the whole sent");
        assert!(app.tails > 0);
        assert!(!whole.partial);
        assert_eq!(whole.turns.len(), 700);
        drop(h);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A reading asked for while discovery reads isn't lost: its next
    /// wait ends at once; asked during a wait, that wait ends.
    #[test]
    fn again_wakes_the_wait_or_the_next() {
        let a = Again::default();
        a.ask();
        let s = a.next().expect("a stop");
        assert!(s.is_closed(), "asked between waits: the next ends at once");
        assert!(!a.asked());
        let s = a.next().expect("a stop");
        assert!(!s.is_closed());
        a.ask();
        assert!(s.is_closed(), "asked during a wait: it ends");
    }
}
