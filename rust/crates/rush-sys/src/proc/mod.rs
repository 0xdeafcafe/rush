//! The process table: every process, who started it, and what a tree of
//! them costs in memory and CPU.
//!
//! Ported from internal/proc/proc.go:1-159 and protect.go:1-37; the
//! platform halves are in `darwin.rs` (proc_darwin.go), `linux.rs`
//! (proc_linux.go) and `other.rs` (proc_other.go).

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, BuildHasherDefault, Hasher};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
mod darwin;
#[cfg(target_os = "macos")]
use darwin as os;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod other;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use other as os;

mod sampler;
mod stat;
pub use sampler::{Sampler, Usage};
pub use stat::{Stat, parse_stat};

/// A process id.
pub type Pid = i32;

/// One process.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: Pid,
    pub ppid: Pid,
    pub comm: Arc<str>,
    /// Bytes; what Activity Monitor calls Memory.
    pub footprint: u64,
    pub cpu_time: Duration,
    /// Percent of one core since the previous snapshot.
    pub cpu: f64,
    /// When it started; `UNIX_EPOCH` when unknown.
    pub start: SystemTime,
}

impl Default for Proc {
    fn default() -> Self {
        Proc {
            pid: 0,
            ppid: 0,
            comm: Arc::from(""),
            footprint: 0,
            cpu_time: Duration::ZERO,
            cpu: 0.0,
            start: UNIX_EPOCH,
        }
    }
}

/// One sample of the whole process table.
#[derive(Clone, Debug)]
pub struct Table {
    pub at: Instant,
    pub procs: PidMap<Proc>,
    pub children: PidMap<Vec<Pid>>,
}

/// A map keyed by pid. Pids are small numbers the kernel hands out, not
/// anyone's input, so they're hashed by one multiply rather than SipHash:
/// a sample fills two maps of a thousand-odd entries every second.
pub type PidMap<V> = HashMap<Pid, V, BuildHasherDefault<PidHasher>>;

/// The hasher of [`PidMap`].
#[derive(Default, Clone, Copy)]
pub struct PidHasher(u64);

impl Hasher for PidHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    fn write_i32(&mut self, n: i32) {
        self.0 = (n as u32 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

/// Stands in for the machine's processes when set: the made-up ones the
/// screenshots tool draws. Their memory and CPU time are taken as given,
/// and `args` are their arguments.
pub struct DemoTable {
    pub procs: Box<dyn Fn() -> Vec<Proc> + Send + Sync>,
    pub args: HashMap<Pid, Vec<String>>,
}

static DEMO: RwLock<Option<Arc<DemoTable>>> = RwLock::new(None);

/// Sets (or, with None, clears) the made-up process table.
pub fn set_demo(d: Option<DemoTable>) {
    *DEMO.write().unwrap() = d.map(Arc::new);
}

fn demo() -> Option<Arc<DemoTable>> {
    DEMO.read().unwrap().clone()
}

/// Samples every process. Memory and CPU are left for [`Table::fill`], so
/// a sample never pays for processes nobody is looking at.
pub fn snapshot() -> Table {
    let ps = match demo() {
        Some(d) => (d.procs)(),
        None => os::list(),
    };
    let mut t = Table {
        at: Instant::now(),
        procs: PidMap::with_capacity_and_hasher(ps.len(), Default::default()),
        children: PidMap::with_capacity_and_hasher(ps.len() / 4, Default::default()),
    };
    for p in ps {
        // macOS's kernel_task is pid 0 and its own parent: as its own
        // child, every walk from 0 would never end.
        if p.ppid != p.pid {
            t.children.entry(p.ppid).or_default().push(p.pid);
        }
        t.procs.insert(p.pid, p);
    }
    t
}

impl Table {
    /// Loads memory and CPU for the given roots and all their descendants,
    /// so a sample never pays for processes nobody is looking at. prev
    /// supplies the CPU-time baseline.
    pub fn fill(&mut self, prev: Option<&Table>, roots: &[Pid]) {
        let is_demo = demo().is_some();
        let mut seen = HashSet::<Pid, BuildHasherDefault<PidHasher>>::default();
        let mut stack: Vec<Pid> = roots.iter().rev().copied().collect();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            let Some(p) = self.procs.get_mut(&pid) else {
                continue;
            };
            if !is_demo {
                os::fill_usage(p);
            }
            if let Some(old) = prev.and_then(|prev| prev.procs.get(&pid).map(|o| (prev, o)))
                && old.1.start == p.start
            {
                let dt = self.at.saturating_duration_since(old.0.at);
                if !dt.is_zero() && p.cpu_time >= old.1.cpu_time {
                    p.cpu = (p.cpu_time - old.1.cpu_time).as_nanos() as f64 / dt.as_nanos() as f64
                        * 100.0;
                }
            }
            if let Some(kids) = self.children.get(&pid) {
                stack.extend(kids.iter().rev());
            }
        }
    }

    /// A process and everything under it, in depth-first order, children
    /// by pid.
    pub fn tree(&self, root: Pid) -> Vec<TreeNode<'_>> {
        let mut out = Vec::new();
        self.walk_tree(root, 0, &mut out);
        out
    }

    fn walk_tree<'a>(&'a self, pid: Pid, depth: usize, out: &mut Vec<TreeNode<'a>>) {
        let Some(p) = self.procs.get(&pid) else {
            return;
        };
        out.push(TreeNode { proc: p, depth });
        if let Some(kids) = self.children.get(&pid) {
            let mut kids = kids.clone();
            kids.sort_unstable();
            for c in kids.into_iter().filter(|&c| c != pid) {
                self.walk_tree(c, depth + 1, out);
            }
        }
    }

    /// Totals memory and CPU over a subtree, not descending into stop pids
    /// (those are shown as rows of their own): memory, CPU and how many
    /// processes.
    pub fn sum<S: BuildHasher>(&self, root: Pid, stop: &HashSet<Pid, S>) -> (u64, f64, usize) {
        let (mut mem, mut cpu, mut n) = (0u64, 0f64, 0usize);
        let mut stack = vec![root];
        while let Some(pid) = stack.pop() {
            let Some(p) = self.procs.get(&pid) else {
                continue;
            };
            mem += p.footprint;
            cpu += p.cpu;
            n += 1;
            if let Some(kids) = self.children.get(&pid) {
                stack.extend(kids.iter().filter(|&&c| c != pid && !stop.contains(&c)));
            }
        }
        (mem, cpu, n)
    }

    /// The subtree's pids, deepest first, for killing.
    pub fn descendants(&self, root: Pid) -> Vec<Pid> {
        let mut nodes = self.tree(root);
        nodes.sort_by_key(|n| std::cmp::Reverse(n.depth)); // stable, as Go's SliceStable
        nodes.into_iter().map(|n| n.proc.pid).collect()
    }
}

/// A process in a tree, and how deep it is under the root.
#[derive(Clone, Copy, Debug)]
pub struct TreeNode<'a> {
    pub proc: &'a Proc,
    pub depth: usize,
}

impl std::ops::Deref for TreeNode<'_> {
    type Target = Proc;
    fn deref(&self) -> &Proc {
        self.proc
    }
}

/// A process's arguments.
pub fn args(pid: Pid) -> Vec<String> {
    if let Some(d) = demo() {
        return d.args.get(&pid).cloned().unwrap_or_default();
    }
    os::args(pid)
}

/// A process's environment, as NAME=value; empty when it can't be read
/// (another user's process).
pub fn env(pid: Pid) -> Vec<String> {
    if demo().is_some() {
        return Vec::new();
    }
    os::env(pid)
}

/// The name a process runs as (its comm: the program's file name, cut to
/// 16 bytes on macOS); None when it isn't running. What tells a pid that's
/// still the agent's program from one reused by another.
pub fn comm(pid: Pid) -> Option<String> {
    if let Some(d) = demo() {
        return (d.procs)()
            .into_iter()
            .find(|p| p.pid == pid)
            .map(|p| p.comm.to_string());
    }
    if pid <= 0 {
        return None;
    }
    os::comm(pid)
}

/// A process's arguments, joined by spaces.
pub fn command_line(pid: Pid) -> String {
    args(pid).join(" ")
}

/// Whether pid has exited but its parent hasn't reaped it: it still
/// answers kill(pid, 0), though nothing runs.
pub fn zombie(pid: Pid) -> bool {
    os::zombie(pid)
}

/// When pid started; None when it isn't running (or, on Linux, the boot
/// time is unknown).
pub fn started(pid: Pid) -> Option<SystemTime> {
    os::started(pid)
}

/// Why [`kill`] failed.
#[derive(Debug, PartialEq, Eq)]
pub enum KillError {
    /// The process stands in for a session on another machine.
    Protected,
    /// The target is init, every process (-1), this group (0) or a group
    /// at or below them; or, in a test, a process the test didn't start.
    Refused(Pid),
    /// What kill(2) said.
    Os(i32),
}

impl std::fmt::Display for KillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KillError::Protected => f.write_str(
                "that process stands in for a session on another machine (rush remote attach): stop the session, not it",
            ),
            KillError::Refused(pid) => write!(
                f,
                "refused to signal {pid}: not a process (or group) rush may end"
            ),
            KillError::Os(e) => std::io::Error::from_raw_os_error(*e).fmt(f),
        }
    }
}

impl std::error::Error for KillError {}

/// Sends sig to pid, refusing the processes [`protect`] names (asking
/// whether one runs, sig 0, harms nothing and is always allowed).
///
/// A real signal to pid 1 or below (init, every process, this group, a
/// group) is always refused: those are never one process rush chose. In
/// a test (rush_core::sandbox) only the test's own descendants can be
/// signalled.
pub fn kill(pid: Pid, sig: i32) -> Result<(), KillError> {
    if sig != 0 && pid <= 1 {
        return Err(KillError::Refused(pid));
    }
    if sig != 0 && PROTECTED.read().unwrap().contains(&pid) {
        return Err(KillError::Protected);
    }
    send(pid, sig)
}

/// Sends sig to the process group pgid leads (kill(-pgid, sig)): what a
/// child started in a group of its own, and all it started, is ended
/// with. pgid 1 or below is always refused (kill(-1) is every process the
/// user has); in a test, so is a group whose leader the test didn't start.
pub fn kill_group(pgid: Pid, sig: i32) -> Result<(), KillError> {
    if pgid <= 1 {
        return Err(KillError::Refused(-pgid));
    }
    if sig != 0 && PROTECTED.read().unwrap().contains(&pgid) {
        return Err(KillError::Protected);
    }
    send(-pgid, sig)
}

/// Sends sig again to pid, which [`kill`] signalled a moment ago and whose
/// start time the caller has checked since: in a test it may have been
/// re-parented to init by then (its shell gone first), and is still the
/// test's.
pub(crate) fn kill_again(pid: Pid, sig: i32) -> Result<(), KillError> {
    if pid <= 1 {
        return Err(KillError::Refused(pid));
    }
    if PROTECTED.read().unwrap().contains(&pid) {
        return Err(KillError::Protected);
    }
    // SAFETY: kill(2) takes plain integers.
    if unsafe { libc::kill(pid, sig) } == 0 {
        Ok(())
    } else {
        Err(KillError::Os(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        ))
    }
}

/// kill(2), refusing in a test any target the test didn't start.
fn send(target: Pid, sig: i32) -> Result<(), KillError> {
    if sig != 0 && rush_core::sandbox::active() && !descends_from_us(target.abs()) {
        return Err(KillError::Refused(target));
    }
    // SAFETY: kill(2) takes plain integers.
    if unsafe { libc::kill(target, sig) } == 0 {
        Ok(())
    } else {
        Err(KillError::Os(
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        ))
    }
}

/// Whether pid is this process or was started by it, or by what it
/// started: the only processes a test may signal.
fn descends_from_us(pid: Pid) -> bool {
    let me = std::process::id() as Pid;
    let parents: HashMap<Pid, Pid> = os::list().into_iter().map(|p| (p.pid, p.ppid)).collect();
    let mut p = pid;
    for _ in 0..64 {
        if p == me {
            return true;
        }
        match parents.get(&p) {
            Some(&q) if q > 1 && q != p => p = q,
            _ => return false,
        }
    }
    false
}

static PROTECTED: RwLock<Vec<Pid>> = RwLock::new(Vec::new());

/// Says which processes [`kill`] must refuse: the attach processes whose
/// pid every remote session's stand-in carries, replacing what was said
/// before.
pub fn protect(pids: &[Pid]) {
    *PROTECTED.write().unwrap() = pids.to_vec();
}

/// The comm interner shared by samples: most names are the same from one
/// sample to the next, so each is kept once.
pub(crate) struct Comms(HashMap<Box<[u8]>, Arc<str>>);

impl Comms {
    pub(crate) fn new() -> Self {
        Comms(HashMap::new())
    }

    pub(crate) fn get(&mut self, b: &[u8]) -> Arc<str> {
        if self.0.len() > 4096 {
            self.0.clear();
        }
        if let Some(c) = self.0.get(b) {
            return c.clone();
        }
        let c: Arc<str> = Arc::from(String::from_utf8_lossy(b).as_ref());
        self.0.insert(b.into(), c.clone());
        c
    }
}

#[cfg(test)]
mod tests;
