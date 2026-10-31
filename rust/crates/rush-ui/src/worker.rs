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

use rush_convo::{Fresh, Session, Tail};
use rush_core::adapter::{self, AdapterRef, Registry, SLICE, global};
use rush_core::agent::{Agent, Preview};
use rush_core::event::Event;
use rush_core::time::{Ms, now};
use rush_draw::Sender;
use rush_fleet::{HostInfo, Loader};
use rush_sys::fswait::{Stop, Watcher};

/// What a worker tells the UI thread.
pub enum Note {
    Fleet(Fleet),
    Preview(String, Box<Preview>),
    Tail {
        r#gen: u64,
        fresh: Box<Fresh>,
    },
    /// Events another adapter's history gave; reset, start over.
    History {
        r#gen: u64,
        events: Vec<Event>,
        reset: bool,
        /// More is written than this slice took: it's sent at once.
        more: bool,
    },
    TailFailed {
        r#gen: u64,
        err: String,
    },
    /// The whole of a conversation opened on its end, read after it.
    Whole {
        r#gen: u64,
        sess: Box<Session>,
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
}

/// While something is live, how often discovery reads again.
pub const POLL: Duration = Duration::from_secs(2);
const LIVE_WINDOW: i64 = 128 << 10;
pub const PICKED_WINDOW: i64 = 384 << 10;
/// How much of a transcript the tail reads at once: the UI applies each
/// slice in one short step, so a flood or a reread never holds up typing.
pub const TAIL_SLICE: usize = 512 << 10;

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
    /// as Go scans the last snapshot's (model.go:627 scan).
    targets: Option<Vec<(String, rush_core::adapter::SpendTarget)>>,
}

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
        }
    }

    pub fn with(reg: Registry) -> Discovery {
        Discovery {
            loader: Loader::new(reg.clone()),
            reg,
            roots: HashMap::new(),
            hosted: Arc::default(),
            targets: None,
        }
    }

    /// The rows; `price` scans spend first. The transcripts the last
    /// reading had are scanned before one load; only rows new to this one
    /// (or changed) are scanned after it, and read again if their spend
    /// moved: one load a reading, not two, every row as priced as before.
    pub fn load(&mut self, now: Ms, price: bool) -> Fleet {
        let prev = self.targets.take().filter(|_| price);
        if let Some(t) = &prev {
            self.loader.scan_targets(t);
        }
        let mut snap = self.loader.load(now);
        if price {
            let mut fresh = Loader::kind_targets(&snap.agents);
            if let Some(t) = &prev {
                let had: HashSet<&(String, rush_core::adapter::SpendTarget)> = t.iter().collect();
                fresh.retain(|x| !had.contains(x));
            }
            if !fresh.is_empty() && self.loader.scan_targets(&fresh) > 0 {
                snap = self.loader.load(now);
            }
        }
        self.targets = Some(Loader::kind_targets(&snap.agents));
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
        let previews = agents
            .iter()
            .filter(|a| a.job.live() && !a.job.transcript_path.is_empty())
            .map(|a| {
                let p = self
                    .reg
                    .read_preview(a.kind.as_str(), &a.job.transcript_path, LIVE_WINDOW);
                (a.key.clone(), p)
            })
            .collect();
        Fleet {
            agents,
            roots,
            previews,
            priced: price,
        }
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
        let paths = self.watched();
        let new: Vec<PathBuf> = paths.iter().filter(|p| !w.watching(p)).cloned().collect();
        w.watch(&paths);
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
                let live = f.agents.iter().any(|a| a.job.live());
                if f != last {
                    last = f.clone();
                    if !tx.send(Note::Fleet(f)) {
                        return;
                    }
                }
                // Woken for a reading asked for (refresh, everyAgent): a
                // fresh Stop each wait, as a closed one stays so.
                let stop = again.as_ref().and_then(|a| a.next());
                w.wait(stop.as_deref(), live.then_some(POLL));
                // What changed: the adapters read it again, whatever they
                // kept (a project folder listed a second ago included).
                let (mut paths, mut all) = w.changes();
                if !all && !urgent(&paths) && gap(began, stop.as_deref()) {
                    // A busy agent's files, changing on and on: read at
                    // most once a GAP, as Go reads on its tick.
                    let (more, a) = w.changes();
                    paths.extend(more);
                    all |= a;
                }
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
/// first; whether it waited at all.
fn gap(began: std::time::Instant, stop: Option<&Stop>) -> bool {
    let until = began + GAP;
    let mut waited = false;
    loop {
        let now = std::time::Instant::now();
        if now >= until || stop.is_some_and(Stop::is_closed) {
            return waited;
        }
        waited = true;
        std::thread::sleep((until - now).min(Duration::from_millis(50)));
    }
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

/// Reads a transcript's last 2 MB, then what's added as it grows, a
/// TAIL_SLICE at a time; each slice parsed here and sent for the UI
/// thread to apply, the next read while it does.
pub fn spawn_tail(
    path: PathBuf,
    r#gen: u64,
    tx: Sender<Note>,
    decode: rush_convo::Decode,
) -> Option<TailHandle> {
    // One slice waits at most: the next is read while the UI applies it.
    let tx = tx.lane(1);
    let wake = Arc::new(Stop::new().ok()?);
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let wk = wake.clone();
    std::thread::Builder::new()
        .name("rush-tail".into())
        .spawn(move || {
            // Watched before the first read, so nothing written between a
            // read and the wait is missed.
            let w = Watcher::new();
            w.watch(&[&path]);
            let mut tail = Tail::from_end(&path, 2 << 20, decode.clone());
            tail.stop = Some(st.clone());
            loop {
                if st.load(Ordering::Relaxed) {
                    return;
                }
                match tail.fetch_most(TAIL_SLICE) {
                    Ok(f) => {
                        let more = f.more;
                        if (!f.is_empty() || f.reset)
                            && !tx.send(Note::Tail {
                                r#gen,
                                fresh: Box::new(f),
                            })
                        {
                            return;
                        }
                        if more {
                            continue; // the send waited for the UI to take the last
                        }
                        // Its end shown, the whole of a long one is read
                        // after, here, and taken in where the end was; it
                        // is followed from there on (Go's readWhole,
                        // twopass.go:33).
                        if tail.partway() {
                            let mut whole = Tail::new(&path, decode.clone());
                            whole.stop = Some(st.clone());
                            let mut sess = Session::new();
                            if whole.read(&mut sess).is_ok() && !st.load(Ordering::Relaxed) {
                                if !tx.send(Note::Whole {
                                    r#gen,
                                    sess: Box::new(sess),
                                }) {
                                    return;
                                }
                                tail = whole;
                                continue; // what was written while it was read
                            }
                        }
                    }
                    Err(e) => {
                        if !path.exists() {
                            tx.send(Note::TailFailed {
                                r#gen,
                                err: e.to_string(),
                            });
                        }
                    }
                }
                if !w.wait(Some(&wk), None) {
                    return;
                }
                // Replaced or removed: watch what's there now.
                w.watch(&[&path]);
            }
        })
        .ok()?;
    Some(TailHandle { stop, wake })
}

/// Follows an adapter's history as it grows (its HistoryFollower), a
/// SLICE at a time, through fswait::follow (watched before each read, so a
/// write is never missed); one with only a HistoryReader is read once.
/// Each slice is sent for the UI thread to apply, the next read while it
/// does.
pub fn spawn_history(
    ad: &AdapterRef,
    s: &adapter::Session,
    host: &str,
    r#gen: u64,
    tx: Sender<Note>,
) -> Option<TailHandle> {
    let tx = tx.lane(1);
    let wake = Arc::new(Stop::new().ok()?);
    let stop = Arc::new(AtomicBool::new(false));
    let (ad, mut s, wk, host) = (ad.clone(), s.clone(), wake.clone(), host.to_string());
    std::thread::Builder::new()
        .name("rush-history".into())
        .spawn(move || {
            // A stopped rush session's folder is its host's to say: a
            // Codex thread known by its id alone is looked for there
            // (openHistory, history.go:53).
            if s.profile.dir.is_empty()
                && !host.is_empty()
                && let Ok(cfg) = rush_host::client::read_config(&host)
            {
                s.profile.dir = cfg.account.dir;
            }
            let failed = |e: adapter::Error| {
                tx.send(Note::TailFailed {
                    r#gen,
                    err: e.to_string(),
                })
            };
            let sent = |events, reset, more| {
                tx.send(Note::History {
                    r#gen,
                    events,
                    reset,
                    more,
                })
            };
            if let Some(f) = ad.history_follower() {
                let mut f = match f.follow_history(&s) {
                    Ok(f) => f,
                    Err(e) => {
                        failed(e);
                        return;
                    }
                };
                rush_sys::fswait::follow(f.as_mut(), SLICE, &wk, &mut |r| match r {
                    Ok(sl) => sent(sl.events, sl.reset, sl.more),
                    Err(e) => failed(e),
                });
            } else if let Some(r) = ad.history_reader() {
                // Each slice is sent once the next is read, so the last
                // is known to be the last.
                let mut held: Option<Vec<Event>> = None;
                let mut first = true;
                let res = r.history(&s, 0, SLICE, &mut |evs| {
                    let ok = match held.replace(evs) {
                        Some(prev) => {
                            let ok = sent(prev, first, true);
                            first = false;
                            ok
                        }
                        None => true,
                    };
                    ok && !wk.is_closed()
                });
                // Nothing in it: it's read all the same. One cut short by
                // an error isn't read to its end.
                sent(held.take().unwrap_or_default(), first, res.is_err());
                if let Err(e) = res {
                    failed(e);
                }
            }
        })
        .ok()?;
    Some(TailHandle { stop, wake })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Another adapter's history is followed on a worker: what it reads
    /// arrives as a History note, and the worker stops when dropped.
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
                if let Msg::App(Note::History { events, .. }) = m {
                    self.0 += events.len();
                    return Step::Quit;
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        let lp: Loop<Note> = Loop::new();
        let ad: AdapterRef = Arc::new(Hist);
        let h = spawn_history(&ad, &adapter::Session::default(), "", 1, lp.sender()).unwrap();
        let mut app = Notes(0);
        lp.run_with(&mut app, |_| Ok(())).unwrap();
        assert_eq!(app.0, 1);
        drop(h); // its wait ends with the handle
    }

    /// A history read once is sent a slice at a time, the first starting
    /// it over and the last saying it's the last: the conversation is
    /// read to its end only then. One cut short by an error never says so.
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
        struct Notes(Vec<(bool, bool)>);
        impl App<Note> for Notes {
            fn update(&mut self, m: Msg<Note>) -> Step {
                match m {
                    Msg::App(Note::History { reset, more, .. }) => {
                        self.0.push((reset, more));
                        if !more {
                            return Step::Quit;
                        }
                    }
                    Msg::App(Note::TailFailed { .. }) => return Step::Quit,
                    _ => {}
                }
                Step::Idle
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        for (fails, want) in [
            (false, vec![(true, true), (false, true), (false, false)]),
            (true, vec![(true, true), (false, true), (false, true)]),
        ] {
            let lp: Loop<Note> = Loop::new();
            let ad: AdapterRef = Arc::new(Hist(fails));
            let h = spawn_history(&ad, &adapter::Session::default(), "", 1, lp.sender()).unwrap();
            let mut app = Notes(Vec::new());
            lp.run_with(&mut app, |_| Ok(())).unwrap();
            assert_eq!(app.0, want, "fails={fails}");
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
            let ad: AdapterRef = Arc::new(Hist);
            let h = spawn_history(&ad, &adapter::Session::default(), host, 1, lp.sender()).unwrap();
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
                if let Msg::App(Note::Tail { .. }) = m {
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
        let decode = rush_convo::decode(rush_claude::messages);
        let h = spawn_tail(p.clone(), 1, lp.sender(), decode).unwrap();
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
        let decode = rush_convo::decode(rush_claude::messages);
        let h = spawn_tail(p.clone(), 1, lp.sender(), decode).unwrap();
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
