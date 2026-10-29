//! What lets a load that follows nothing new reuse the last reading: most
//! refreshes find every file as it was, and only the processes' CPU and
//! memory to read again. Port of internal/fleet/watch.go.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rush_core::agent::Agent;
use rush_core::time::Ms;
use rush_sys::fswait::Watcher;

use crate::Predicates;
use crate::hosted::host_root;
use crate::loader::{Loader, Snapshot};

/// How often a load looks at everything, watched or not.
const SWEEP_EVERY: Ms = 30_000;

/// The loader's watch on what it reads.
pub(crate) struct Watching {
    pub(crate) w: Option<Watcher>,
    /// A full reading at least this often.
    every: Ms,
    /// The next load may reuse.
    pub(crate) settle: bool,
    /// Something was handed in since: read afresh.
    pub(crate) stale: bool,
    /// The last full reading.
    last: Option<Snapshot>,
    last_full: Ms,
    /// What changed since the last load, drained once as it begins (load
    /// seq): a file watched and not changed since a cache looked at it
    /// needn't be looked at again.
    seq: u64,
    chg: HashSet<PathBuf>,
    /// It can't tell what changed: look at everything.
    all: bool,
    /// The load each path was last told changed in.
    touched: HashMap<PathBuf, u64>,
    /// The last load that couldn't tell.
    all_at: u64,
    /// This load trusts no watch: what one missed heals.
    sweep: bool,
    /// The last sweep.
    pub(crate) swept: Ms,
}

impl Watching {
    /// Its watcher starts with the first load.
    pub(crate) fn start(&mut self) {
        if self.w.is_none() {
            self.w = Some(Watcher::new());
        }
    }
}

/// Whether path is watched and hasn't changed since a cache looked at it
/// in load at: what it read then still holds. A sweep trusts nothing, nor
/// a load after one that couldn't tell what changed.
pub(crate) fn fresh(w: Option<&Watching>, quick: bool, path: &Path, at: u64) -> bool {
    let Some(wt) = w else { return false };
    let Some(watcher) = &wt.w else { return false };
    at != 0
        && !wt.sweep
        && !quick
        && wt.all_at <= at
        && wt.touched.get(path).copied().unwrap_or(0) <= at
        && watcher.watching(path)
}

impl Loader {
    /// Has the loader watch what it reads, so a load after settle reuses
    /// its last reading when none of it changed. It reads everything afresh
    /// at least every `every` ms.
    pub fn watch(&mut self, every: Ms) {
        self.watching = Some(Watching {
            w: None,
            every,
            settle: false,
            stale: false,
            last: None,
            last_full: 0,
            seq: 0,
            chg: HashSet::new(),
            all: false,
            touched: HashMap::new(),
            all_at: 0,
            sweep: false,
            swept: 0,
        });
    }

    /// Lets the next load reuse the last reading if nothing it reads has
    /// changed since.
    pub fn settle(&mut self) {
        self.handle().settle();
    }

    /// Blocks until something the loader watches changes, or timeout (ms)
    /// passes: what to wait on between readings. Without watch, returns at
    /// once.
    pub fn wait(&self, timeout: Option<Ms>) -> bool {
        match self.watching.as_ref().and_then(|w| w.w.as_ref()) {
            Some(w) => w.wait(
                None,
                timeout.map(|t| std::time::Duration::from_millis(t.max(0) as u64)),
            ),
            None => false,
        }
    }

    /// Marks the last reading stale: spend, usage or a nudge was handed
    /// in, which only a full load takes up.
    pub(crate) fn changed_since(&mut self) {
        if let Some(w) = &mut self.watching {
            w.stale = true;
        }
    }

    pub(crate) fn fresh(&self, path: &Path, at: u64) -> bool {
        fresh(self.watching.as_ref(), self.quick_now(), path, at)
    }

    /// The load a cache looking at its file now notes, for fresh to ask
    /// after; 0 when nothing is watched, which fresh never trusts.
    pub(crate) fn checked(&self) -> u64 {
        match &self.watching {
            Some(w) if !self.quick_now() => w.seq,
            _ => 0,
        }
    }

    /// Takes what changed since the last load, once, as one begins.
    pub(crate) fn drain(&mut self, now: Ms) {
        let Some(wt) = &mut self.watching else { return };
        wt.seq += 1;
        let (chg, all) = wt.w.as_ref().map(|w| w.changes()).unwrap_or_default();
        (wt.chg, wt.all) = (chg, all);
        for p in &wt.chg {
            wt.touched.insert(p.clone(), wt.seq);
        }
        if wt.all {
            wt.all_at = wt.seq;
        }
        wt.sweep = wt.all || now - wt.swept >= SWEEP_EVERY;
    }

    /// Starts a full load: what changed up to now, it reads.
    pub(crate) fn began(&mut self, now: Ms) {
        self.sub_watch.clear();
        let quick = self.quick_now();
        if let Some(wt) = &mut self.watching {
            wt.stale = false;
            wt.settle = false;
            if wt.sweep && !quick {
                wt.swept = now;
            }
        }
    }

    /// The last reading with the processes sampled again, when nothing
    /// watched has changed; None when the load should read afresh.
    pub(crate) fn reuse(&mut self, now: Ms) -> Option<Snapshot> {
        let wt = self.watching.as_mut()?;
        let settle = std::mem::take(&mut wt.settle);
        if !settle
            || wt.last.is_none()
            || wt.stale
            || now - wt.last_full >= wt.every
            || wt.sweep
            || wt.all
            || !wt.chg.is_empty()
        {
            return None;
        }
        let last = wt.last.clone()?;
        let mut tab = self.snapshot_procs();
        if last
            .agents
            .iter()
            .any(|a| a.pid != 0 && !tab.procs.contains_key(&a.pid))
        {
            return None; // a process ended: its row changes
        }
        let mut snap = Snapshot {
            at: now,
            accounts: last.accounts,
            logins: last.logins,
            hosted: last.hosted,
            ..Snapshot::default()
        };
        for a in last.agents {
            let mut c: Agent = a;
            self.sample(Some(&mut tab), &mut c);
            c.temp = self.temp.bytes(&c.key);
            snap.agents.push(c);
        }
        snap.machine = self.machine_of(&mut tab, &snap.agents);
        let t = Arc::new(tab);
        self.set_prev_tab(t.clone());
        snap.table = Some(t);
        snap.agents.sort_by_key(|a| a.age(now));
        Some(snap)
    }

    /// Keeps a full load's reading for reuse, and watches what it read.
    pub(crate) fn keep(&mut self, snap: &Snapshot) {
        if self.watching.is_none() {
            return;
        }
        let paths = self.watch_paths(snap);
        let found: Vec<String> = self.found_paths();
        let Some(wt) = &mut self.watching else { return };
        let mut last = snap.clone();
        last.table = None;
        wt.last = Some(last);
        wt.last_full = snap.at;
        let Some(w) = &wt.w else { return };
        w.watch(&paths);
        wt.touched.retain(|p, _| w.watching(p)); // watched again, it's told changed again
        let gone: Vec<String> = found.into_iter().filter(|p| !w.watching(p)).collect();
        self.forget_found(&gone); // not asked for this load, or gone: looked for again
    }

    /// What a full load read that can change under it: the folders
    /// sessions and jobs register in, the hosts' info, the settings it
    /// reads, and each live agent's transcript and subagents.
    fn watch_paths(&self, snap: &Snapshot) -> Vec<PathBuf> {
        let mut paths = vec![rush_state::dir(), host_root()];
        if !self.usage_path.as_os_str().is_empty() {
            paths.push(self.usage_path.clone());
        }
        paths.extend(self.sub_watch.iter().cloned());
        for info in &snap.hosted {
            // Its info.json is replaced on each change: the folder says so.
            paths.push(host_root().join(&info.id));
        }
        for a in &snap.agents {
            if !a.job.live() && a.pid == 0 && !a.busy() {
                continue;
            }
            let p = &a.job.transcript_path;
            if !p.is_empty() {
                let sess = p.strip_suffix(".jsonl").unwrap_or(p);
                let dir = Path::new(p)
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                paths.extend([
                    PathBuf::from(p),
                    dir,
                    PathBuf::from(sess),
                    Path::new(sess).join("subagents"),
                ]);
            }
        }
        paths.extend(self.watched());
        paths.sort();
        paths.dedup();
        paths
    }
}
