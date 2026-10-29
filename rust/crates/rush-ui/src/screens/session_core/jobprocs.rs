//! The machine's processes under a session's tasks: which process each
//! running shell task runs as, what it uses, the processes under one
//! opened (busiest first when drawn), and the shells the agent runs its
//! Bash calls in, for which command of a chain runs now. Read off the UI
//! thread with the session's runs (subs.rs), and only while a running
//! task is opened in the background view: an idle session reads none.
//! After Go's internal/ui/jobs.go (jobPID :301, jobUsage :347, jobLines'
//! processes :580-597), known.go (shortCmd :102, readProcWords :115),
//! keys.go:1283 (trimCmd) and shells.go (watchShells :24, shellComm :84,
//! shellKids :93, shellCommand :300).

use std::collections::HashMap;
use std::time::UNIX_EPOCH;

use rush_convo::{Shell, ShellProc};
use rush_core::time::Ms;
use rush_sys::proc::{self, Pid, Table};

use crate::screens::list_extras::jobs::strip_ansi;
use crate::style::{one_line, tildify};

/// A process under an opened task: its program, its words, and what it
/// uses now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcRow {
    pub pid: i32,
    /// Its program's name (filepath.Base of its comm).
    pub comm: String,
    /// Its words, short (shortCmd).
    pub cmd: String,
    pub cpu: f64,
    pub footprint: u64,
}

/// What the last read of the processes found.
#[derive(Clone, Debug, Default)]
pub struct JobProcs {
    /// Each running shell task's process, by task id (jobPID).
    pub pids: HashMap<String, i32>,
    /// What each uses, its memory and CPU over its tree (Table.Sum).
    pub usage: HashMap<String, (u64, f64)>,
    /// The processes under each opened task, in tree order.
    pub trees: HashMap<String, Vec<ProcRow>>,
    /// The shells the agent runs its Bash calls in.
    pub shells: Vec<Shell>,
}

/// What to read: the agent's process, the running shell tasks by id with
/// when each started, and the opened ones.
#[derive(Clone, Debug, Default)]
pub struct ProcAsk {
    pub agent: i32,
    pub shells: Vec<(String, Ms)>,
    pub open: Vec<String>,
}

/// A process's start as rush keeps times.
fn ms(p: &proc::Proc) -> Ms {
    p.start
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as Ms)
}

/// Reads the processes asked about; prev is the last read's table, for
/// CPU, and is left as this one's. Blocks on the system: run it off the
/// UI thread.
pub fn read(prev: &mut Option<Table>, a: &ProcAsk) -> JobProcs {
    let mut t = proc::snapshot();
    t.fill(prev.as_ref(), &[a.agent]);
    let mut out = JobProcs::default();
    for (id, start) in &a.shells {
        let pid = job_pid(&t, a.agent, *start);
        if pid == 0 {
            continue;
        }
        out.pids.insert(id.clone(), pid);
        let (mem, cpu, _) = t.sum(pid, &std::collections::HashSet::<Pid>::new());
        out.usage.insert(id.clone(), (mem, cpu));
        if a.open.contains(id) {
            let rows = t
                .tree(pid)
                .into_iter()
                .map(|n| ProcRow {
                    pid: n.pid,
                    comm: base(&n.comm).to_string(),
                    cmd: short_cmd(n.pid, &n.comm),
                    cpu: n.cpu,
                    footprint: n.footprint,
                })
                .collect();
            out.trees.insert(id.clone(), rows);
        }
    }
    out.shells = shells(&t, a.agent);
    *prev = Some(t);
    out
}

/// The process a running shell task runs as: of those the agent started,
/// the one that started with it, within 3s (jobPID). ponytail: matched on
/// start time alone, as Go's.
pub fn job_pid(t: &Table, agent: Pid, start: Ms) -> Pid {
    if start == 0 {
        return 0;
    }
    let (mut best, mut gap) = (0, 3000);
    for k in t.children.get(&agent).into_iter().flatten() {
        if let Some(p) = t.procs.get(k) {
            let d = (ms(p) - start).abs();
            if d < gap {
                best = *k;
                gap = d;
            }
        }
    }
    best
}

/// A path's last element.
fn base(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

/// A process's words, short: its program by name, then its arguments, on
/// one line, the home folder as ~, at most 200 cells; its comm when they
/// can't be read (shortCmd, readProcWords, trimCmd).
fn short_cmd(pid: Pid, comm: &str) -> String {
    let mut args = proc::args(pid);
    if args.is_empty() {
        return comm.to_string();
    }
    args[0] = base(&args[0]).to_string();
    let s = tildify(&one_line(&args.join(" ")));
    rush_draw::truncate(&strip_ansi(&s), 200, "…")
        .trim_end()
        .to_string()
}

/// Whether a program is a shell the agent runs a call in (shellComm).
fn shell_comm(c: &str) -> bool {
    matches!(c.trim_start_matches('-'), "zsh" | "bash" | "sh")
}

/// The command a shell runs with -c, from its words (shellCommand).
fn shell_command(args: &[String]) -> String {
    if args.len() < 3 || !shell_comm(base(&args[0])) {
        return String::new();
    }
    for i in 1..args.len() - 1 {
        let a = &args[i];
        if a == "--" || !a.starts_with('-') {
            return String::new();
        }
        if !a.starts_with("--") && a.contains('c') {
            return args[i + 1].clone();
        }
    }
    String::new()
}

/// The shells under the agent's process, each with the processes under it
/// and their words (watchShells' read).
fn shells(t: &Table, agent: Pid) -> Vec<Shell> {
    let mut out = Vec::new();
    for &k in t.children.get(&agent).into_iter().flatten() {
        let Some(p) = t.procs.get(&k) else { continue };
        if !shell_comm(&p.comm) {
            continue;
        }
        let args = proc::args(k);
        let mut command = shell_command(&args);
        let cmd = args.join(" ");
        if command.is_empty() {
            continue;
        }
        // An eval wrapper's call is matched as its hook rewrote it.
        if cmd.contains(" eval ") {
            command.clear();
        }
        out.push(Shell {
            cmd,
            command,
            start: ms(p),
            kids: kids(t, k, 0),
        });
    }
    out
}

/// The process tree under pid, with each process's words (shellKids).
fn kids(t: &Table, pid: Pid, depth: usize) -> Vec<ShellProc> {
    if depth > 6 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &k in t.children.get(&pid).into_iter().flatten() {
        let Some(p) = t.procs.get(&k) else { continue };
        let mut args = proc::args(k);
        if args.is_empty() {
            args = vec![p.comm.to_string()];
        }
        out.push(ShellProc {
            pid: k,
            args,
            start: ms(p),
            kids: kids(t, k, depth + 1),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| a.to_string()).collect()
    }

    /// The -c command of a shell, none for anything else (shells_test.go
    /// TestShellCommand).
    #[test]
    fn a_shells_command() {
        for (args, want) in [
            (s(&["/bin/zsh", "-c", "ls"]), "ls"),
            (s(&["bash", "-lc", "go test"]), "go test"),
            (s(&["zsh", "--login", "-c", "x"]), "x"),
            (s(&["zsh", "script.sh", "-c"]), ""),
            (s(&["node", "-c", "x"]), ""),
            (s(&["zsh", "-c"]), ""),
        ] {
            assert_eq!(shell_command(&args), want, "{args:?}");
        }
    }
}
