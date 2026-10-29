//! The machine's processes as the list sees them: each agent's, its
//! daemon's, spares, and what ended sessions left running; which process
//! ran which agent. Port of internal/fleet/fleet.go:212-245 (Role,
//! ProcRow, Machine), 1377-1534 (machine, isProgramPID, hasAgentAncestor,
//! underAgent, orphanLabel, ShellCmd), 1566 (spawnOf), 1800 (ranBy), 1818
//! (progProcs), and fleet/reap.go (the Reaper).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rush_core::adapter::Registry;
use rush_core::agent::Agent;
use rush_core::time::Ms;
use rush_sys::proc::{self, Pid, Proc, Table};
use serde_json::json;

/// What a process is to the list, in the order the machine view lists
/// them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    #[default]
    Other,
    Daemon,
    View,
    Worker,
    Spare,
    Orphan,
}

impl Role {
    /// Its number, as Go's Role prints in `rush --dump`.
    pub fn number(self) -> u8 {
        self as u8
    }
}

/// One process the machine view lists, with everything under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcRow {
    pub pid: Pid,
    pub role: Role,
    pub label: String,
    pub cmd: String,
    pub mem: u64,
    pub cpu: f64,
    pub procs: usize,
    pub start: Ms,
    /// The agent a worker belongs to.
    pub key: String,
}

/// The machine's processes, as the list counts them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub rows: Vec<ProcRow>,
    pub total_mem: u64,
    pub total_cpu: f64,
    pub spares: usize,
    pub spare_mem: u64,
    /// Processes whose session has ended; they run until the user keeps
    /// or kills them.
    pub orphans: usize,
    pub orphan_mem: u64,
}

pub(crate) fn start_ms(t: SystemTime) -> Ms {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as Ms)
}

/// The last path element of a name or path.
fn base(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// Whether a process called comm (a name, or a path) is agent k's
/// program. macOS keeps a name cut to 16 bytes.
pub(crate) fn is_program(reg: &Registry, k: &str, comm: &str) -> bool {
    let p = reg.program_of(k);
    let c = base(comm);
    !p.is_empty() && (c == p || c.len() >= 15 && p.starts_with(c))
}

/// Whether process pid is agent k's program.
pub(crate) fn is_program_pid(reg: &Registry, tab: &Table, pid: Pid, k: &str) -> bool {
    tab.procs
        .get(&pid)
        .is_some_and(|p| is_program(reg, k, &p.comm))
}

/// Whether p runs under an agent's program.
fn has_agent_ancestor(reg: &Registry, tab: &Table, p: &Proc) -> bool {
    let mut pid = p.ppid;
    for _ in 0..64 {
        if pid <= 1 {
            break;
        }
        let Some(q) = tab.procs.get(&pid) else {
            return false;
        };
        if reg.is_program(&q.comm) {
            return true;
        }
        pid = q.ppid;
    }
    false
}

/// Whether p runs inside one of the agents' trees.
fn under_agent(tab: &Table, p: &Proc, agent_of: &HashMap<Pid, usize>) -> bool {
    let mut pid = p.ppid;
    for _ in 0..64 {
        if pid <= 1 {
            break;
        }
        if agent_of.contains_key(&pid) {
            return true;
        }
        let Some(q) = tab.procs.get(&pid) else {
            return false;
        };
        pid = q.ppid;
    }
    false
}

/// What an orphan was: a background job's, or an ended session's.
pub fn orphan_label(cmd: &str) -> String {
    if let Some(i) = cmd.find("/.claude/jobs/")
        && cmd.len() >= i + 22
        && cmd.is_char_boundary(i + 22)
    {
        return format!("job {}", &cmd[i + 14..i + 22]);
    }
    "ended session".into()
}

/// What a Bash-tool shell was asked to run: the command inside Claude
/// Code's eval '…' wrapper, without the snapshot sourcing around it.
pub fn shell_cmd(cmd: &str) -> String {
    let Some(i) = cmd.find("eval '") else {
        return cmd.to_string();
    };
    let mut rest = &cmd.as_bytes()[i + 6..];
    let mut b = Vec::new();
    while !rest.is_empty() {
        if rest.starts_with(b"'\"'\"'") {
            b.push(b'\'');
            rest = &rest[5..];
            continue;
        }
        if rest[0] == b'\'' {
            return String::from_utf8_lossy(&b).into_owned();
        }
        b.push(rest[0]);
        rest = &rest[1..];
    }
    cmd.to_string()
}

/// The machine's processes the view lists: every agent's own, its home
/// agent's daemon, workers, spares and native views, other runs of it in
/// a terminal, and what ended sessions left to launchd. cmdline says a
/// process's command line, kept by its pid and start.
pub(crate) fn machine(
    reg: &Registry,
    home: &str,
    tab: &mut Table,
    prev: Option<&Table>,
    agents: &[Agent],
    cmdline: &mut dyn FnMut(&Proc) -> String,
) -> Machine {
    let mut m = Machine::default();
    let mut worker_of: HashMap<Pid, usize> = HashMap::new();
    let mut agent_of: HashMap<Pid, usize> = HashMap::new(); // every agent's own root
    for (i, a) in agents.iter().enumerate() {
        if let Some(w) = a.worker {
            worker_of.insert(w, i);
        }
        if a.pid != 0 && tab.procs.contains_key(&a.pid) {
            agent_of.insert(a.pid, i);
        }
    }
    let mut pids: Vec<Pid> = tab.procs.keys().copied().collect();
    pids.sort_unstable();
    for pid in pids {
        let p = &tab.procs[&pid];
        let row = if let (Some(&i), false) = (agent_of.get(&pid), worker_of.contains_key(&pid)) {
            // A rush session's host, or an agent in a terminal: the agent.
            let a = &agents[i];
            ProcRow {
                pid,
                cmd: cmdline(p),
                start: start_ms(p.start),
                role: Role::Worker,
                label: a.display_name.clone(),
                key: a.key.clone(),
                ..ProcRow::default()
            }
        } else if is_program(reg, home, &p.comm) {
            let cmd = cmdline(p);
            let mut row = ProcRow {
                pid,
                cmd: cmd.clone(),
                start: start_ms(p.start),
                ..ProcRow::default()
            };
            if cmd.contains(" daemon run") {
                (row.role, row.label) = (Role::Daemon, "daemon".into());
            } else if cmd.contains("--bg-pty-host") {
                if let Some(&i) = worker_of.get(&pid) {
                    row.role = Role::Worker;
                    row.label = agents[i].display_name.clone();
                    row.key = agents[i].key.clone();
                } else {
                    (row.role, row.label) = (Role::Spare, "spare (pre-warmed, unclaimed)".into());
                }
            } else if cmd.contains("--bg-spare") {
                continue; // counted inside its pty host
            } else if cmd.trim().ends_with(" agents") || cmd.contains(" agents ") {
                (row.role, row.label) = (Role::View, "claude agents (native view)".into());
            } else {
                let first = format!("{cmd} x");
                let first = first.split_whitespace().next().unwrap_or("");
                if has_agent_ancestor(reg, tab, p)
                    || under_agent(tab, p, &agent_of)
                    || !is_program(reg, home, first)
                {
                    continue;
                }
                (row.role, row.label) = (Role::Other, "claude (interactive)".into());
            }
            row
        } else if p.ppid == 1 {
            let cmd = cmdline(p);
            if !cmd.contains("/.claude/") {
                continue;
            }
            ProcRow {
                pid,
                role: Role::Orphan,
                cmd: shell_cmd(&cmd),
                start: start_ms(p.start),
                label: orphan_label(&cmd),
                ..ProcRow::default()
            }
        } else {
            continue;
        };
        m.rows.push(row);
    }
    let roots: HashSet<Pid> = m.rows.iter().map(|r| r.pid).collect();
    let all: Vec<Pid> = m.rows.iter().map(|r| r.pid).collect();
    tab.fill(prev, &all);
    for r in &mut m.rows {
        (r.mem, r.cpu, r.procs) = tab.sum(r.pid, &roots);
        m.total_mem += r.mem;
        m.total_cpu += r.cpu;
        if r.role == Role::Spare {
            m.spares += 1;
            m.spare_mem += r.mem;
        }
        if r.role == Role::Orphan {
            m.orphans += 1;
            m.orphan_mem += r.mem;
        }
    }
    m.rows
        .sort_by(|a, b| a.role.cmp(&b.role).then(b.mem.cmp(&a.mem)));
    m
}

/// The nearest process above pid that is one of parents: the session
/// whose shell ran it. 0 when none is.
pub(crate) fn spawn_of(tab: Option<&Table>, pid: Pid, parents: &HashSet<Pid>) -> Pid {
    let Some(tab) = tab else { return 0 };
    let Some(p) = tab.procs.get(&pid) else {
        return 0;
    };
    if parents.contains(&p.ppid) {
        // Run straight from a session's process is no shell's: a host's
        // own agent, before the host says which it is.
        return 0;
    }
    let mut p = Some(p);
    for _ in 0..12 {
        let Some(q) = p else { break };
        if q.ppid <= 1 {
            return 0;
        }
        if parents.contains(&q.ppid) && q.ppid != pid {
            return q.ppid;
        }
        p = tab.procs.get(&q.ppid);
    }
    0
}

/// The agent whose process is nearest above pid's, by its index.
pub(crate) fn ran_by(tab: Option<&Table>, pid: Pid, by_pid: &HashMap<Pid, usize>) -> Option<usize> {
    let tab = tab?;
    let mut p = tab.procs.get(&pid);
    for _ in 0..12 {
        let q = p?;
        if let Some(&i) = by_pid.get(&q.ppid) {
            return Some(i);
        }
        p = tab.procs.get(&q.ppid);
    }
    None
}

/// Finds the process of agent k's that started nearest to at, within
/// seconds of it, for a session its agent lists no process for: each
/// agent's processes picked out once a table, not once a row.
/// ponytail: by start time alone, so two runs of one agent begun within
/// seconds of each other may swap.
pub(crate) struct ProgProcs<'a> {
    pub reg: &'a Registry,
    pub tab: Option<&'a Table>,
    by: HashMap<String, Vec<(Pid, Ms)>>,
}

impl<'a> ProgProcs<'a> {
    pub fn new(reg: &'a Registry, tab: Option<&'a Table>) -> Self {
        ProgProcs {
            reg,
            tab,
            by: HashMap::new(),
        }
    }

    pub fn proc_of(&mut self, k: &str, at: Ms) -> Pid {
        let Some(tab) = self.tab else { return 0 };
        if at == 0 {
            return 0;
        }
        let reg = self.reg;
        let ps = self.by.entry(k.to_string()).or_insert_with(|| {
            tab.procs
                .values()
                .filter(|p| is_program(reg, k, &p.comm))
                .map(|p| (p.pid, start_ms(p.start)))
                .collect()
        });
        let (mut best, mut gap) = (0, 10_000);
        for &(pid, start) in ps.iter() {
            let d = (start - at).abs();
            if d < gap || d == gap && best != 0 && pid < best {
                (best, gap) = (pid, d);
            }
        }
        best
    }
}

// ---- What an agent leaves running when it stops (reap.go) ----

/// Ends what an agent leaves running when it stops: shells, servers and
/// watchers its tools started, which the system hands to launchd once the
/// agent's process is gone. It only ever ends a process it saw under an
/// agent, by its pid and start time, so nothing else is touched.
#[derive(Default)]
pub struct Reaper {
    seen: HashMap<Pid, SeenProc>,
}

struct SeenProc {
    start: SystemTime,
    /// The agent's name, for saying what was ended.
    agent: String,
    /// The agent's process it ran under: only once that has exited is it
    /// left behind.
    owner: Pid,
    owner_start: SystemTime,
}

/// A process an agent left behind, with everything under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Leftover {
    pub pid: Pid,
    pub start: SystemTime,
    pub agent: String,
    pub cmd: String,
    pub procs: usize,
}

impl Reaper {
    pub fn new() -> Reaper {
        Reaper::default()
    }

    /// Notes the processes under every agent that has one, and returns
    /// those noted before that have since been orphaned, with their
    /// command lines.
    pub fn watch(&mut self, reg: &Registry, tab: &Table, agents: &[Agent]) -> Vec<Leftover> {
        let mut out = self.scan(reg, tab, agents);
        for l in &mut out {
            l.describe();
        }
        out
    }

    /// Watch without the command lines: it only reads tab.
    pub fn scan(&mut self, reg: &Registry, tab: &Table, agents: &[Agent]) -> Vec<Leftover> {
        let mut under: HashSet<Pid> = HashSet::new();
        let me = std::process::id() as Pid;
        for a in agents {
            if a.pid == 0 || a.pid == me {
                continue;
            }
            let Some(root) = tab.procs.get(&a.pid) else {
                continue;
            };
            for pid in tab.descendants(a.pid) {
                under.insert(pid);
                if pid == a.pid {
                    continue;
                }
                let Some(p) = tab.procs.get(&pid) else {
                    continue;
                };
                if self.seen.get(&pid).is_some_and(|s| s.start == p.start) {
                    continue;
                }
                let Some(owner) = owner_of(reg, tab, root, p) else {
                    continue;
                };
                self.seen.insert(
                    pid,
                    SeenProc {
                        start: p.start,
                        agent: a.display_name.clone(),
                        owner: owner.pid,
                        owner_start: owner.start,
                    },
                );
            }
        }
        let mut out = Vec::new();
        self.seen.retain(|&pid, s| {
            let Some(p) = tab.procs.get(&pid).filter(|p| p.start == s.start) else {
                return false; // gone, or the pid was reused
            };
            if under.contains(&pid) {
                return true;
            }
            if p.ppid == 1 && !alive(tab, s.owner, s.owner_start) {
                // Adopted by launchd, so the top of what was left: ending
                // it ends what's under it too.
                out.push(Leftover {
                    pid,
                    start: s.start,
                    agent: s.agent.clone(),
                    cmd: String::new(),
                    procs: tab.descendants(pid).len(),
                });
                return false;
            }
            true
        });
        out.sort_by_key(|l| l.pid);
        out
    }
}

impl Leftover {
    /// Fills in its command line, which asks the system.
    pub fn describe(&mut self) {
        self.cmd = proc::command_line(self.pid);
    }

    /// Stops it and everything under it: SIGTERM, then SIGKILL for
    /// whatever is still there after grace. One that isn't the process
    /// noted (its pid reused) is left alone.
    pub fn end(&self, grace: Duration) {
        let tab = proc::snapshot();
        if tab
            .procs
            .get(&self.pid)
            .is_none_or(|p| p.start != self.start)
        {
            return;
        }
        let pids = tab.descendants(self.pid);
        let starts: HashMap<Pid, SystemTime> = pids
            .iter()
            .filter_map(|p| tab.procs.get(p).map(|q| (*p, q.start)))
            .collect();
        for &pid in &pids {
            let _ = proc::kill(pid, libc::SIGTERM);
        }
        std::thread::sleep(grace);
        let now = proc::snapshot();
        let mut killed = 0;
        for &pid in &pids {
            if now
                .procs
                .get(&pid)
                .is_some_and(|p| Some(&p.start) == starts.get(&pid))
            {
                let _ = proc::kill(pid, libc::SIGKILL);
                killed += 1;
            }
        }
        rush_sys::events::log(
            "leftover-ended",
            json!({"pid": self.pid, "cmd": self.cmd, "agent": self.agent, "procs": pids.len(),
                "killed": killed, "why": "its agent exited and left it running"}),
        );
    }
}

/// The agent's process p runs under for an agent whose process is root:
/// root itself when it is an agent's program, else (a rush host) the child
/// of root that p descends from. p itself is never its own owner.
fn owner_of<'a>(reg: &Registry, tab: &'a Table, root: &'a Proc, p: &'a Proc) -> Option<&'a Proc> {
    if reg.is_program(&root.comm) {
        return Some(root);
    }
    let mut q = Some(p);
    for _ in 0..64 {
        let x = q?;
        if x.ppid == root.pid {
            if x.pid == p.pid {
                return None; // the host's own child is the owner, not a leftover
            }
            return Some(x);
        }
        q = tab.procs.get(&x.ppid);
    }
    None
}

fn alive(tab: &Table, pid: Pid, start: SystemTime) -> bool {
    tab.procs.get(&pid).is_some_and(|p| p.start == start)
}
