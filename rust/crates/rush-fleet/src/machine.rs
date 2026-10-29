//! The machine's agent processes as rows: each agent's own, the home
//! agent's daemon and spares, interactive ones no row stands for, and
//! orphans whose session has ended. Go: internal/fleet/fleet.go (Role
//! :212, ProcRow :223, Machine :235, machine :1377, hasAgentAncestor
//! :1475, underAgent :1490, orphanLabel :1504, ShellCmd :1514). It reads
//! the process table: never on the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::SystemTime;

use rush_sys::proc::{Pid, Table};

/// What a process row is.
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

/// One process tree on the machine.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcRow {
    pub pid: Pid,
    pub role: Role,
    pub label: String,
    pub cmd: String,
    pub mem: u64,
    pub cpu: f64,
    pub procs: usize,
    pub start: SystemTime,
    /// The agent a worker belongs to.
    pub key: String,
}

/// The machine's agent processes, and what they come to.
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

/// An agent's process, as the machine's rows need it.
#[derive(Clone, Debug, Default)]
pub struct AgentProc {
    pub pid: Pid,
    pub key: String,
    pub name: String,
}

/// The rows of tab: agents' own, the home program's (program, a command
/// name such as "claude"), and orphans under ~/.claude. is_agent says
/// whether a command name is any agent's program; cmdline reads a
/// process's command line. tab is filled for each row's tree.
pub fn machine(
    tab: &mut Table,
    prev: Option<&Table>,
    agents: &[AgentProc],
    program: &str,
    is_agent: &dyn Fn(&str) -> bool,
    cmdline: &dyn Fn(Pid, &str) -> String,
) -> Machine {
    let mut m = Machine::default();
    let agent_of: HashMap<Pid, &AgentProc> = agents
        .iter()
        .filter(|a| a.pid != 0 && tab.procs.contains_key(&a.pid))
        .map(|a| (a.pid, a))
        .collect();
    let is_home = |comm: &str| !program.is_empty() && base(comm) == program;
    let mut pids: Vec<Pid> = tab.procs.keys().copied().collect();
    pids.sort_unstable();
    for pid in pids {
        let p = &tab.procs[&pid];
        let row = if let Some(a) = agent_of.get(&pid) {
            ProcRow {
                pid,
                role: Role::Worker,
                label: a.name.clone(),
                cmd: cmdline(pid, &p.comm),
                key: a.key.clone(),
                ..row(p.start)
            }
        } else if is_home(&p.comm) {
            let cmd = cmdline(pid, &p.comm);
            let (role, label) = if cmd.contains(" daemon run") {
                (Role::Daemon, "daemon")
            } else if cmd.contains("--bg-pty-host") {
                (Role::Spare, "spare (pre-warmed, unclaimed)")
            } else if cmd.contains("--bg-spare") {
                continue; // counted inside its pty host
            } else if cmd.trim().ends_with(" agents") || cmd.contains(" agents ") {
                (Role::View, "claude agents (native view)")
            } else {
                let first = cmd.split_whitespace().next().unwrap_or("x");
                if has_agent_ancestor(tab, p.ppid, is_agent)
                    || under_agent(tab, p.ppid, &agent_of)
                    || !is_home(first)
                {
                    continue;
                }
                (Role::Other, "claude (interactive)")
            };
            ProcRow {
                pid,
                role,
                label: label.into(),
                cmd,
                ..row(p.start)
            }
        } else if p.ppid == 1 {
            let cmd = cmdline(pid, &p.comm);
            if !cmd.contains("/.claude/") {
                continue;
            }
            ProcRow {
                pid,
                role: Role::Orphan,
                label: orphan_label(&cmd),
                cmd: shell_cmd(&cmd),
                ..row(p.start)
            }
        } else {
            continue;
        };
        m.rows.push(row);
    }
    let roots: HashSet<Pid> = m.rows.iter().map(|r| r.pid).collect();
    let all: Vec<Pid> = roots.iter().copied().collect();
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

fn row(start: SystemTime) -> ProcRow {
    ProcRow {
        pid: 0,
        role: Role::Other,
        label: String::new(),
        cmd: String::new(),
        mem: 0,
        cpu: 0.0,
        procs: 0,
        start,
        key: String::new(),
    }
}

fn base(s: &str) -> &str {
    Path::new(s)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(s)
}

/// Whether a process whose parent is ppid runs under an agent's program.
fn has_agent_ancestor(tab: &Table, mut pid: Pid, is_agent: &dyn Fn(&str) -> bool) -> bool {
    for _ in 0..64 {
        if pid <= 1 {
            return false;
        }
        let Some(q) = tab.procs.get(&pid) else {
            return false;
        };
        if is_agent(base(&q.comm)) {
            return true;
        }
        pid = q.ppid;
    }
    false
}

/// Whether a process whose parent is ppid runs inside an agent's tree.
fn under_agent(tab: &Table, mut pid: Pid, agent_of: &HashMap<Pid, &AgentProc>) -> bool {
    for _ in 0..64 {
        if pid <= 1 {
            return false;
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

/// An orphan's label: the background job it was, else an ended session.
pub fn orphan_label(cmd: &str) -> String {
    if let Some(i) = cmd.find("/.claude/jobs/")
        && cmd.len() >= i + 22
        && cmd.is_char_boundary(i + 14)
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
    let mut rest = &cmd[i + 6..];
    let mut b = String::new();
    while let Some(c) = rest.chars().next() {
        if rest.starts_with(r#"'"'"'"#) {
            b.push('\'');
            rest = &rest[5..];
            continue;
        }
        if c == '\'' {
            return b;
        }
        b.push(c);
        rest = &rest[c.len_utf8()..];
    }
    cmd.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_sys::proc::Proc;
    use std::sync::Arc;
    use std::time::{Instant, UNIX_EPOCH};

    #[test]
    fn shell_commands_unwrapped() {
        for (cmd, want) in [
            (
                "/bin/zsh -c source ~/.claude/shell-snapshots/x.sh && eval 'npm run dev' < /dev/null",
                "npm run dev",
            ),
            (r#"zsh -c eval 'echo '"'"'hi'"'"'' && pwd"#, "echo 'hi'"),
            ("node server.js", "node server.js"),
        ] {
            assert_eq!(shell_cmd(cmd), want, "{cmd}");
        }
    }

    #[test]
    fn orphans_labelled() {
        assert_eq!(
            orphan_label("node /Users/me/.claude/jobs/abcdef12/x.js"),
            "job abcdef12"
        );
        assert_eq!(orphan_label("node /x/.claude/y"), "ended session");
    }

    fn proc(pid: Pid, ppid: Pid, comm: &str, mem: u64) -> Proc {
        Proc {
            pid,
            ppid,
            comm: Arc::from(comm),
            footprint: mem,
            start: UNIX_EPOCH,
            ..Proc::default()
        }
    }

    #[test]
    fn rows_by_role() {
        let mut tab = Table {
            at: Instant::now(),
            procs: Default::default(),
            children: Default::default(),
        };
        for p in [
            proc(10, 1, "claude", 100),
            proc(11, 10, "node", 50),
            proc(20, 1, "zsh", 30),
            proc(21, 20, "node", 70),
            proc(30, 1, "claude", 40),
            proc(40, 1, "bash", 5),
        ] {
            tab.children.entry(p.ppid).or_default().push(p.pid);
            tab.procs.insert(p.pid, p);
        }
        let cmds: HashMap<Pid, &str> = [
            (10, "claude"),
            (
                20,
                "zsh -c eval 'npm run dev' /Users/me/.claude/shell-snapshots/a",
            ),
            (30, "claude daemon run"),
            (40, "bash"),
        ]
        .into_iter()
        .collect();
        let agents = [AgentProc {
            pid: 10,
            key: "k".into(),
            name: "fix it".into(),
        }];
        let m = machine(
            &mut tab,
            None,
            &agents,
            "claude",
            &|c| c == "claude",
            &|pid, comm| cmds.get(&pid).map_or(comm.to_string(), |s| s.to_string()),
        );
        let roles: Vec<(Pid, Role)> = m.rows.iter().map(|r| (r.pid, r.role)).collect();
        assert_eq!(
            roles,
            vec![(30, Role::Daemon), (10, Role::Worker), (20, Role::Orphan)]
        );
        assert_eq!(m.orphans, 1);
        assert_eq!(m.rows[2].cmd, "npm run dev");
    }
}
