//! A shell command can hang: a test deadlocked, a prompt nobody will
//! answer, a lock never let go. The turn waits on it to its timeout,
//! burning time and showing nothing. Rush watches the processes of the
//! commands the turn waits on: one that has used no CPU for STUCK_AFTER is
//! stopped, and the agent told why, so the turn carries on. CPU use has no
//! event to wait on, so the look is a timer, armed only while the turn
//! waits on a shell.
//!
//! Ported from internal/host/stuck.go (the server's share is in
//! server/watch.rs).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use rush_sys::proc::{self, Pid, Table};

/// How long a shell's tree may use no CPU before it's hung.
pub const STUCK_AFTER: Duration = Duration::from_secs(3 * 60);
/// How often the shells the turn waits on are looked at.
pub const STUCK_LOOK: Duration = Duration::from_secs(20);

/// What the agent is told of a command rush stopped.
pub fn stuck_note(cmd: &str, after: Duration) -> String {
    format!(
        "rush stopped your command `{cmd}`: its processes used no CPU for {}, so it was hung (a deadlock, or waiting on input or a lock). Don't run it again as it was: find why it hung, or run less of it.",
        rush_gate::godur::string(after)
    )
}

/// A shell command the turn waits on.
#[derive(Clone, Debug)]
pub struct Waited {
    pub cmd: String,
    pub at: std::time::SystemTime,
}

/// A shell's tree as last looked at: its CPU time and processes, and since
/// when neither changed.
#[derive(Clone, Copy, Debug)]
pub struct TreeUse {
    cpu: Duration,
    n: usize,
    since: Instant,
}

/// The agent's shells started since from, a second's grace for the clock:
/// the ones the turn waits on, not those it left running in the
/// background before.
pub fn waited_shells(tab: &Table, agent: Pid, from: std::time::SystemTime) -> Vec<Pid> {
    let from = from - Duration::from_secs(1);
    let mut out = Vec::new();
    for c in tab.descendants(agent) {
        let Some(p) = tab.procs.get(&c) else { continue };
        if c == agent || p.start < from {
            continue;
        }
        if tab.procs.contains_key(&p.ppid) && proc::command_line(p.ppid).contains("shell-snapshots")
        {
            continue; // inside one already counted
        }
        if proc::command_line(c).contains("shell-snapshots") {
            out.push(c);
        }
    }
    out
}

/// Notes each shell tree's use in seen, and gives those that used no CPU
/// and started or ended no process for STUCK_AFTER. One waiting its turn
/// at the gate, or whose run the gate paused for memory, isn't hung; nor
/// is one running sleep, which waits on purpose for as long as it said.
pub fn idle_shells(
    tab: &mut Table,
    shells: &[Pid],
    seen: &mut HashMap<Pid, TreeUse>,
    now: Instant,
    gate_waiting: &dyn Fn(Pid) -> bool,
) -> Vec<Pid> {
    let mut out = Vec::new();
    let live: HashSet<Pid> = shells.iter().copied().collect();
    for &sh in shells {
        tab.fill(None, &[sh]);
        let mut u = TreeUse {
            cpu: Duration::ZERO,
            n: 0,
            since: now,
        };
        let mut gated = false;
        for c in tab.descendants(sh) {
            let Some(p) = tab.procs.get(&c) else { continue };
            u.cpu += p.cpu_time;
            u.n += 1;
            let line = proc::command_line(c);
            let leaf = tab.children.get(&c).is_none_or(|k| k.is_empty());
            if !gated && line.contains(" gate ") && (leaf || gate_waiting(c)) {
                gated = true;
            }
            if !gated && sleeping(&line) {
                gated = true;
            }
        }
        if let Some(old) = seen.get(&sh)
            && !gated
            && old.cpu == u.cpu
            && old.n == u.n
        {
            u.since = old.since;
        }
        seen.insert(sh, u);
        if now.saturating_duration_since(u.since) >= STUCK_AFTER {
            out.push(sh);
        }
    }
    seen.retain(|sh, _| live.contains(sh));
    out
}

/// Ends what a shell runs, deepest first, and the shell with it: asked,
/// then made to a few seconds on (by later, which runs a job after a
/// while).
pub fn stop_tree(tab: &Table, sh: Pid, later: impl FnOnce(Duration, Box<dyn FnOnce() + Send>)) {
    let pids = tab.descendants(sh);
    for &p in &pids {
        let _ = proc::kill(p, libc::SIGTERM);
    }
    later(
        Duration::from_secs(3),
        Box::new(move || {
            for p in pids {
                if proc::kill(p, 0).is_ok() {
                    let _ = proc::kill(p, libc::SIGKILL);
                }
            }
        }),
    );
}

/// Whether a process's command line is sleep's.
pub fn sleeping(line: &str) -> bool {
    line.split_whitespace()
        .next()
        .is_some_and(|f| Path::new(f).file_name().is_some_and(|b| b == "sleep"))
}

/// What a shell runs, as the agent wrote it: its command line has it under
/// rush's script runner, or the gate, when rush ran it so.
pub fn shell_command(sh: Pid) -> String {
    let line = proc::command_line(sh);
    let line = match line.rfind("eval ") {
        Some(i) => &line[i + 5..],
        None => &line,
    };
    agent_command(line)
}

/// The script rush's runner is given.
static SCRIPT_ARG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"rush-script '([^']+\.sh)'").expect("a valid pattern"));

/// A command line's command as the agent wrote it.
pub fn agent_command(line: &str) -> String {
    if let Some(m) = SCRIPT_ARG.captures(line)
        && let Ok(b) = std::fs::read_to_string(&m[1])
    {
        return b.trim().to_string();
    }
    rush_gate::unwrap(line)
}

/// A command cut to one line of reasonable length.
pub fn short(cmd: &str) -> String {
    let cmd = cmd.split('\n').next().unwrap_or("").trim();
    let r: Vec<char> = cmd.chars().collect();
    if r.len() > 120 {
        return r[..119].iter().collect::<String>() + "…";
    }
    cmd.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // stuck_test.go:75: a command rush ran as a script, or under the gate,
    // is named as the agent wrote it.
    #[test]
    fn agent_commands() {
        let d = tempfile::tempdir().unwrap();
        let sh = d.path().join("toolu_x.sh");
        std::fs::write(&sh, "cd x\npython3 bench.py\n").unwrap();
        let line = format!(
            "'/opt/homebrew/bin/bash' --noprofile --norc -c '__rush_s=$1; . \"$__rush_s\"' rush-script '{}'",
            sh.display()
        );
        assert_eq!(agent_command(&line), "cd x\npython3 bench.py");
        assert_eq!(
            agent_command("'/r' gate run --name 'go' --dir '/x' -- bash -c 'go test ./...'"),
            "go test ./..."
        );
        assert_eq!(agent_command("ls -la"), "ls -la");
    }

    #[test]
    fn short_cuts_to_a_line() {
        assert_eq!(short("  a\nb"), "a");
        let long = "x".repeat(130);
        assert_eq!(short(&long).chars().count(), 120);
        assert!(short(&long).ends_with('…'));
        assert!(sleeping("/bin/sleep 60") && !sleeping("sleepy 1"));
    }

    // stuck_test.go:17: a shell whose tree uses no CPU for STUCK_AFTER is
    // hung, and stopped; one at work isn't, nor one sleeping. The clock is
    // moved on rather than waited for.
    #[test]
    fn stuck_shell_is_stopped() {
        use std::process::{Command, Stdio};
        let mut idle = Command::new("sh")
            .args(["-c", "read x; true"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut busy = Command::new("sh")
            .args(["-c", "while :; do :; done"])
            .spawn()
            .unwrap();
        let mut waits = Command::new("sh")
            .args(["-c", "sleep 60; true"])
            .spawn()
            .unwrap();
        let shells = [idle.id() as Pid, busy.id() as Pid, waits.id() as Pid];
        // Each has started what it runs (sleep's child is there) before
        // the first look: the table is read again until it is.
        let mut tab;
        loop {
            tab = proc::snapshot();
            let w = shells[2];
            if tab.children.get(&w).is_some_and(|k| !k.is_empty()) {
                break;
            }
            std::thread::yield_now();
        }
        let mut seen = HashMap::new();
        let now = Instant::now();
        let none = |_| false;
        assert!(idle_shells(&mut tab, &shells, &mut seen, now, &none).is_empty());
        // The busy one uses CPU between looks: the next look waits for it
        // to have used some.
        let cpu0 = seen[&shells[1]].cpu;
        let mut tab = loop {
            let mut t = proc::snapshot();
            t.fill(None, &[shells[1]]);
            let c: Duration = t
                .descendants(shells[1])
                .iter()
                .filter_map(|p| t.procs.get(p))
                .map(|p| p.cpu_time)
                .sum();
            if c > cpu0 {
                break t;
            }
            std::thread::yield_now();
        };
        let got = idle_shells(&mut tab, &shells, &mut seen, now + STUCK_AFTER, &none);
        assert_eq!(got, vec![shells[0]], "only the idle one is hung");
        let (tx, rx) = std::sync::mpsc::channel();
        stop_tree(&tab, got[0], |_, f| tx.send(f).unwrap());
        idle.wait().unwrap();
        rx.recv().unwrap()(); // the follow-up finds nothing left to kill
        let _ = busy.kill();
        let _ = waits.kill();
        let _ = busy.wait();
        let _ = waits.wait();
    }
}
