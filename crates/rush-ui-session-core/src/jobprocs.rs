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

use crate::style::{one_line, tildify};
use rush_ui_list_extras::jobs::strip_ansi;

// A process under an opened task is the base's (the Tasks box reads it).
pub use rush_ui_base::jobprocs::ProcRow;

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
pub(crate) fn ms(p: &proc::Proc) -> Ms {
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
pub(crate) fn base(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

/// A process's words, short: its program by name, then its arguments, on
/// one line, the home folder as ~, at most 200 cells; its comm when they
/// can't be read (shortCmd, readProcWords, trimCmd).
fn short_cmd(pid: Pid, comm: &str) -> String {
    let args = proc::args(pid);
    if args.is_empty() {
        return comm.to_string();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let s = proc_words(&args, home.trim_end_matches('/'));
    rush_draw::truncate(&strip_ansi(&s), 200, "…")
        .trim_end()
        .to_string()
}

/// A process's words as they read best: its program by name; a shell's
/// -c script as what it evals when it's a harness's wrapper (Claude
/// Code's source-the-snapshot … eval '…'); home as ~ in every word and
/// doubled slashes made one. Rust-only.
pub fn proc_words(args: &[String], home: &str) -> String {
    let Some(prog) = args.first() else {
        return String::new();
    };
    let prog = base(prog).to_string();
    let script = shell_command(args);
    let s = if script.is_empty() {
        let mut w = vec![prog];
        w.extend(args[1..].iter().cloned());
        w.join(" ")
    } else {
        let script = eval_body(&script).unwrap_or(script);
        format!("{prog} -c {script}")
    };
    let s = one_line(&s);
    let s = if home.is_empty() {
        s
    } else {
        tildify(&s.replace(&format!("{home}/"), "~/"))
    };
    one_slash(&s)
}

/// What an eval wrapper runs: the single-quoted words after its eval.
fn eval_body(script: &str) -> Option<String> {
    let at = script.find("eval '")?;
    let rest = &script[at + 6..];
    let mut out = String::new();
    let mut i = 0;
    while i < rest.len() {
        let r = &rest[i..];
        if r.starts_with("'\\''") {
            out.push('\'');
            i += 4;
        } else if r.starts_with('\'') {
            return Some(out);
        } else {
            let c = r.chars().next()?;
            out.push(c);
            i += c.len_utf8();
        }
    }
    None
}

/// Doubled slashes in paths made one; a URL's // stays.
fn one_slash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '/' && out.ends_with('/') && !out.ends_with(":/") {
            continue;
        }
        out.push(c);
    }
    out
}

/// Whether a program is a shell the agent runs a call in (shellComm).
pub(crate) fn shell_comm(c: &str) -> bool {
    matches!(c.trim_start_matches('-'), "zsh" | "bash" | "sh")
}

/// The command a shell runs with -c, from its words (shellCommand).
pub(crate) fn shell_command(args: &[String]) -> String {
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
    shells_under(t, agent, 0, &mut out);
    out
}

/// Whether a program is a sandbox a harness runs its shells in, which
/// stays their parent (Codex's on Linux, bubblewrap).
fn sandbox_comm(c: &str) -> bool {
    matches!(
        base(c),
        "sandbox-exec" | "bwrap" | "codex-linux-sandbox" | "firejail" | "nsjail"
    )
}

/// The shells under pid, and under the sandboxes right under it.
fn shells_under(t: &Table, pid: Pid, depth: usize, out: &mut Vec<Shell>) {
    for &k in t.children.get(&pid).into_iter().flatten() {
        let Some(p) = t.procs.get(&k) else { continue };
        if !shell_comm(&p.comm) {
            if depth < 2 && sandbox_comm(&p.comm) {
                shells_under(t, k, depth + 1, out);
            }
            continue;
        }
        let args = proc::args(k);
        let mut command = shell_command(&args);
        let cmd = args.join(" ");
        if command.is_empty() {
            continue;
        }
        // An eval wrapper's call (Claude Code's) is matched by what its eval
        // runs, or as its hook rewrote it (rush_convo's runs_call).
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
}

/// The process tree under pid, with each process's words (shellKids).
pub(crate) fn kids(t: &Table, pid: Pid, depth: usize) -> Vec<ShellProc> {
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

    /// A process's words as the Processes rows read them.
    #[test]
    fn a_processs_words() {
        let wrap = "source /h/.claude/shell-snapshots/snapshot-zsh-1.sh 2>/dev/null || true && \
                    export RUSH_SESSION='6a' && eval 'zsh $TMPDIR/tools/i.sh > $TMPDIR/l.log 2>&1' \\< /dev/null && pwd -P >| /tmp/claude-1-cwd";
        for (args, want) in [
            (
                s(&["/bin/zsh", "-c", wrap]),
                "zsh -c zsh $TMPDIR/tools/i.sh > $TMPDIR/l.log 2>&1",
            ),
            (
                s(&["zsh", "-c", r"eval 'echo '\''hi'\'''"]),
                "zsh -c echo 'hi'",
            ),
            (s(&["/bin/zsh", "-c", "ls /h/x"]), "zsh -c ls ~/x"),
            (
                s(&["zsh", "/h/.config/rush/tmp/p/6a//tools/i.sh"]),
                "zsh ~/.config/rush/tmp/p/6a/tools/i.sh",
            ),
            (
                s(&["/usr/bin/curl", "https://example.com"]),
                "curl https://example.com",
            ),
            (s(&["/h"]), "h"),
        ] {
            assert_eq!(proc_words(&args, "/h"), want, "{args:?}");
        }
    }
}
