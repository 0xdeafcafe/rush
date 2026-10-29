//! The guard is the gate's last line, as docs/gate-sharing.md rule 6 has
//! it: not every heavy program comes through the gate (a language server
//! an MCP server starts, a binary run by its path), and the kernel's
//! pressure level can stay normal while the machine pages itself to a
//! halt. So when the memory available falls to the floor, it stops the
//! largest program an agent started, one at a time, until there's room
//! again. Ported from internal/gate/guard.go:1-260.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use rush_sys::proc::{self, Pid, Table};
use serde_json::{Value, json};

use crate::gate::{Cancel, Gate, flock, mkdir_all};
use crate::node::base;
use crate::rules::size;
use crate::share::{PRESSURE_CRITICAL, PRESSURE_WARN};
use crate::sys::Machine;

/// The share of memory, in percent, below which the guard stops agents'
/// programs, and the watchdog takes memory to be critical.
pub const DEFAULT_FLOOR: i32 = 5;

/// How often the guard looks; one sysctl a look while there's memory.
pub const GUARD_TICK: Duration = Duration::from_secs(1);

/// How long a program the guard asked to stop has before it's made to.
const GUARD_GRACE: Duration = Duration::from_secs(3);

/// The least a program must hold for the guard to stop it: past that,
/// what fills the memory isn't an agent's.
const MIN_VICTIM: u64 = 256 << 20;

/// The memory pressure the gate acts on: the kernel's level, raised to
/// critical once the memory available is at floor percent or less, and to
/// warning at twice that, since macOS can say normal while it pages itself
/// to a halt.
pub fn pressure(m: &dyn Machine, floor: i32) -> i32 {
    let floor = if floor <= 0 { DEFAULT_FLOOR } else { floor };
    let lvl = m.pressure_level();
    let mem = m.memory_level();
    if mem < 0 {
        return lvl;
    }
    if mem <= floor {
        return PRESSURE_CRITICAL;
    }
    if mem <= 2 * floor && lvl < PRESSURE_WARN {
        return PRESSURE_WARN;
    }
    lvl
}

/// The program the guard stops: its pid, what it is, what it holds, and
/// the agent's program it's under (0 for none known).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Victim {
    pub pid: Pid,
    pub comm: String,
    pub footprint: u64,
    pub agent: Pid,
}

/// Whether p is one of rush's own processes.
fn is_rush(p: Option<&proc::Proc>) -> bool {
    p.is_some_and(|p| base(&p.comm) == "rush")
}

/// The agents' programs in t: each one a rush host runs, that is a process
/// not rush's whose parent and grandparent are.
pub fn agents(t: &Table) -> Vec<Pid> {
    let mut out = Vec::new();
    for (&pid, p) in &t.procs {
        if is_rush(Some(p)) {
            continue;
        }
        let parent = t.procs.get(&p.ppid);
        if is_rush(parent) && is_rush(parent.and_then(|pp| t.procs.get(&pp.ppid))) {
            out.push(pid);
        }
    }
    out
}

/// The largest program in t, its footprints filled, of those under an
/// agent and those named (the gate's programs, wherever they run: a daemon
/// an agent started detaches to launchd, and its tsgo with it). Never
/// rush's own, an agent's program itself, or self; None when none holds
/// MIN_VICTIM.
pub fn choose(t: &Table, agents: &[Pid], names: &HashSet<String>, me: Pid) -> Option<Victim> {
    let mut best = Victim::default();
    let mut consider = |pid: Pid, agent: Pid| {
        let p = t.procs.get(&pid);
        if pid == agent || pid == me || p.is_none() || is_rush(p) {
            return;
        }
        let p = p.unwrap();
        if p.footprint > best.footprint {
            best = Victim {
                pid,
                comm: p.comm.to_string(),
                footprint: p.footprint,
                agent,
            };
        }
    };
    for &a in agents {
        for pid in t.descendants(a) {
            consider(pid, a);
        }
    }
    for pid in named(t, names) {
        consider(pid, 0);
    }
    (best.footprint >= MIN_VICTIM).then_some(best)
}

/// The processes in t whose name is one of names.
pub fn named(t: &Table, names: &HashSet<String>) -> Vec<Pid> {
    let mut out: Vec<Pid> = t
        .procs
        .iter()
        .filter(|(_, p)| names.contains(base(&p.comm)))
        .map(|(&pid, _)| pid)
        .collect();
    out.sort_unstable();
    out
}

/// Tells the agent of a session why its program was stopped.
pub type TellFn = Box<dyn Fn(&str, &str) + Send + Sync>;
/// Records what was done and why: a kind and its fields.
pub type LogFn = Box<dyn Fn(&str, serde_json::Map<String, Value>) + Send + Sync>;

/// Watches the machine's memory and stops agents' programs when it runs
/// out.
pub struct Guard {
    pub gate: Gate,
    /// Percent; 0 for DEFAULT_FLOOR.
    pub floor: i32,
    /// The gate's programs: stopped wherever they run, not only under an
    /// agent.
    pub names: HashSet<String>,
    /// Tells the agent of session why its program was stopped.
    pub tell: Option<TellFn>,
    /// Records what the guard did and why.
    pub log: Option<LogFn>,
}

impl Guard {
    /// Guards until cancelled. Only one guard on the machine acts at a
    /// time: the one holding the guard's lock; the others wait for it.
    pub fn run(&self, cancel: &Cancel) {
        let floor = if self.floor <= 0 {
            DEFAULT_FLOOR
        } else {
            self.floor
        };
        let mut lock: Option<File> = None;
        let mut victim = Victim::default();
        let mut asked = Instant::now();
        let mut killed = false;
        let mut idle_said = false;
        loop {
            if cancel.wait(GUARD_TICK) {
                return;
            }
            if lock.is_none() {
                lock = self.take_lock();
                if lock.is_none() {
                    continue;
                }
            }
            if victim.pid > 0 {
                if proc::kill(victim.pid, 0).is_ok() && !proc::zombie(victim.pid) {
                    if !killed && asked.elapsed() >= GUARD_GRACE {
                        stop_tree(victim.pid, libc::SIGKILL);
                        killed = true;
                        self.say(
                            "guard-killed",
                            json!({"victim": victim.pid, "name": victim.comm,
                                "why": format!("still running {} after it was asked to stop", crate::godur::string(GUARD_GRACE))}),
                        );
                    }
                    continue;
                }
                victim = Victim::default();
                continue; // a tick for the memory it held to come back
            }
            let level = self.gate.machine().memory_level();
            if pressure(self.gate.machine(), floor) != PRESSURE_CRITICAL {
                idle_said = false;
                continue;
            }
            let mut tab = proc::snapshot();
            let ags = agents(&tab);
            let mut roots = ags.clone();
            roots.extend(named(&tab, &self.names));
            tab.fill(None, &roots);
            let Some(v) = choose(&tab, &ags, &self.names, std::process::id() as Pid) else {
                if !idle_said {
                    idle_said = true;
                    self.say(
                        "guard-short",
                        json!({"level": level, "floor": floor,
                            "why": "memory is short, but no agent's program or gated one holds much of it"}),
                    );
                }
                continue;
            };
            let mut why = format!(
                "the machine had {level}% of its memory left, at or under the {floor}% floor, and it held the most of any agent's program ({})",
                size(v.footprint)
            );
            if level < 0 || level > floor {
                why = if level >= 0 {
                    format!(
                        "the kernel said memory was critical ({level}% left), and it held the most of any agent's program ({})",
                        size(v.footprint)
                    )
                } else {
                    format!(
                        "the kernel said memory was critical, and it held the most of any agent's program ({})",
                        size(v.footprint)
                    )
                };
            }
            stop_tree(v.pid, libc::SIGTERM);
            asked = Instant::now();
            killed = false;
            let session = session_of(&[v.pid, v.agent]);
            self.say(
                "guard-stopped",
                json!({"victim": v.pid, "name": v.comm, "footprint": v.footprint, "level": level,
                    "floor": floor, "session": session, "why": why}),
            );
            if let Some(tell) = &self.tell
                && !session.is_empty()
            {
                tell(
                    &session,
                    &format!(
                        "rush stopped `{}` (pid {}), which your session started: {why}. \
                         Don't start it again as it was: run less at once, one typecheck or test run at a time, \
                         and scope it to the paths you touched.",
                        v.comm, v.pid
                    ),
                );
            }
            victim = v;
        }
    }

    fn say(&self, kind: &str, f: Value) {
        if let (Some(log), Value::Object(m)) = (&self.log, f) {
            log(kind, m);
        }
    }

    /// The guard's lock, held, or None while another guard holds it.
    fn take_lock(&self) -> Option<File> {
        mkdir_all(self.gate.root()).ok()?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.gate.root().join("guard"))
            .ok()?;
        flock(&f, libc::LOCK_EX | libc::LOCK_NB).ok()?;
        Some(f)
    }
}

/// Sends sig to pid and everything under it, waking any the gate paused
/// so they hear it.
fn stop_tree(pid: Pid, sig: i32) {
    let tree = proc::snapshot().descendants(pid);
    crate::watch::signal_tree(&tree, sig);
    crate::watch::signal_tree(&tree, libc::SIGCONT);
}

/// The rush session one of pids runs under, from its environment or else
/// its agent's; "" when neither says.
fn session_of(pids: &[Pid]) -> String {
    for &pid in pids {
        for kv in proc::env(pid) {
            if let Some(s) = kv.strip_prefix("RUSH_SESSION=")
                && !s.is_empty()
            {
                return s.to_string();
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::tests::Fake;
    use crate::share::PRESSURE_NORMAL;
    use std::sync::atomic::Ordering;

    // guard_test.go TestPressureFromMemoryLeft: macOS can say normal while
    // the machine pages itself to a halt: memory at the floor is critical
    // whatever the kernel says, near it a warning.
    #[test]
    fn pressure_from_memory_left() {
        let f = Fake::new(0);
        for (kernel, mem, floor, want) in [
            (PRESSURE_NORMAL, 60, 5, PRESSURE_NORMAL),
            (PRESSURE_NORMAL, 4, 5, PRESSURE_CRITICAL),
            (PRESSURE_NORMAL, 5, 0, PRESSURE_CRITICAL),
            (PRESSURE_NORMAL, 9, 5, PRESSURE_WARN),
            (PRESSURE_CRITICAL, 60, 5, PRESSURE_CRITICAL),
            (PRESSURE_NORMAL, -1, 5, PRESSURE_NORMAL),
            (0, -1, 5, 0),
        ] {
            f.pressure.store(kernel, Ordering::SeqCst);
            f.level.store(mem, Ordering::SeqCst);
            assert_eq!(
                pressure(&*f, floor),
                want,
                "kernel {kernel}, memory {mem}%, floor {floor}"
            );
        }
    }

    /// A process table from pid, ppid, comm and footprint in MiB.
    pub(crate) fn table(rows: &[(Pid, Pid, &str, u64)]) -> Table {
        let mut t = Table {
            at: Instant::now(),
            procs: Default::default(),
            children: Default::default(),
        };
        for &(pid, ppid, comm, mib) in rows {
            t.procs.insert(
                pid,
                proc::Proc {
                    pid,
                    ppid,
                    comm: comm.into(),
                    footprint: mib << 20,
                    ..proc::Proc::default()
                },
            );
            t.children.entry(ppid).or_default().push(pid);
        }
        t
    }

    fn names(ns: &[&str]) -> HashSet<String> {
        ns.iter().map(|s| s.to_string()).collect()
    }

    // guard_test.go TestChooseTheLargestOfAgents: as the panic of
    // 2026-10-23 had it, three tsgo language servers under agents, none
    // through the gate, beside a browser that isn't an agent's.
    #[test]
    fn choose_the_largest_of_agents() {
        let tab = table(&[
            (100, 1, "rush", 80),
            (101, 100, "rush", 40),
            (102, 101, "rush", 30),
            (110, 100, "rush", 20),
            (111, 110, "claude", 4000),
            (112, 111, "node", 300),
            (113, 112, "tsgo", 9700),
            (120, 100, "rush", 20),
            (121, 120, "claude", 500),
            (122, 121, "bash", 5),
            (123, 122, "rush", 9999),
            (124, 123, "tsgo", 4100),
            (200, 1, "Arc", 20000),
            (201, 200, "Browser Helper", 12000),
        ]);
        let mut ags = agents(&tab);
        ags.sort();
        assert_eq!(ags, [111, 121]);
        let v = choose(&tab, &ags, &names(&[]), 0).unwrap();
        assert_eq!((v.pid, v.agent), (113, 111));
        let v = choose(&tab, &ags, &names(&[]), 113).unwrap();
        assert_eq!(v.pid, 124);
    }

    // guard_test.go TestChooseNothingSmall
    #[test]
    fn choose_nothing_small() {
        let tab = table(&[
            (100, 1, "rush", 80),
            (110, 100, "rush", 20),
            (111, 110, "claude", 6000),
            (112, 111, "bash", 100),
        ]);
        assert_eq!(choose(&tab, &agents(&tab), &names(&[]), 0), None);
    }

    // guard_test.go TestChooseDetachedByName: a language server's daemon
    // detaches to launchd, and its tsgo with it: out of every agent's
    // tree, it's stopped by its name.
    #[test]
    fn choose_detached_by_name() {
        let tab = table(&[
            (100, 1, "rush", 80),
            (110, 100, "rush", 20),
            (111, 110, "claude", 400),
            (112, 111, "node", 300),
            (300, 1, "node", 200),
            (301, 300, "tsgo", 9700),
            (400, 1, "Arc", 20000),
        ]);
        let v = choose(&tab, &agents(&tab), &names(&["tsgo", "tsc"]), 0).unwrap();
        assert_eq!((v.pid, v.agent), (301, 0));
        let v = choose(&tab, &agents(&tab), &names(&[]), 0).unwrap();
        assert_eq!(v.pid, 112);
    }

    // The guard stops when told, holding its lock only while it runs.
    #[test]
    fn guard_lock_is_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let g = Guard {
            gate: Gate::at(dir.path(), Fake::new(0)),
            floor: 0,
            names: HashSet::new(),
            tell: None,
            log: None,
        };
        let held = g.take_lock().unwrap();
        assert!(g.take_lock().is_none(), "two guards held the lock");
        drop(held);
        assert!(g.take_lock().is_some());
        let cancel = Cancel::new();
        cancel.cancel();
        g.run(&cancel);
    }
}
