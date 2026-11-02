//! What a running Bash step is doing now, found without the agent's
//! help: Claude Code and others put a foreground command's output in the
//! transcript only when it ends. So rush looks itself, at the processes
//! under the step's shell (the busiest one, its CPU) and at the file the
//! command writes (`> f`, `>> f`, `&> f`, `| tee f`), its last lines. Read
//! off the UI thread every LIVE_EVERY while the step runs and the
//! conversation is open; gone when the step ends. New in the Rust port.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_convo::Status;
use rush_convo::render::StepLive;
use rush_core::time::Ms;
use rush_sys::proc::{self, Pid, Table};
use rush_ui_list_extras::jobs::{Tailed, read_tail};

use super::jobprocs::{base, ms, shell_comm, shell_command};
use super::{agent, conn};
use crate::frame::Out;
use crate::model::Model;

/// How often a running step is looked at.
pub const LIVE_EVERY: Ms = 2000;
/// Files whose tail is read, at most, for one step.
const FILES_MOST: usize = 2;
/// Tail lines shown.
const TAIL_SHOWN: usize = 3;
/// No CPU for this long is "waiting".
const WAIT_AFTER: Duration = Duration::from_secs(10);

/// A running call as last looked at.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    pub reading: bool,
    pub at: Ms,
    /// The files its command writes, worked out once.
    pub files: Option<Vec<String>>,
    pub tails: Vec<Option<Tailed>>,
    /// The last table, for CPU.
    table: Arc<Mutex<Option<Table>>>,
    /// Its tree's CPU time and process count at the last look, and since
    /// when neither moved.
    cpu: Duration,
    procs: usize,
    still: Ms,
    pub view: StepLive,
}

/// The running Bash calls of the latest turn: id, command, start.
fn calls(m: &Model, key: &str) -> Vec<(String, String, Ms)> {
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let Some(t) = v.sess.turns.last() else {
        return Vec::new();
    };
    t.items
        .iter()
        .filter_map(|it| match it {
            rush_convo::model::Item::Step(sid) => Some(v.sess.step(*sid)),
            _ => None,
        })
        .filter(|st| &*st.tool == "Bash" && st.status == Status::Running && !st.id.is_empty())
        .map(|st| {
            (
                st.id.to_string(),
                st.call().input.command.trim().to_string(),
                st.start,
            )
        })
        .collect()
}

/// What one look found.
struct Found {
    now: String,
    cpu: Duration,
    procs: usize,
    tails: Vec<Option<Tailed>>,
}

/// Looks again at the session's running Bash calls where due, and hands
/// the open conversation what they show.
pub fn sync(m: &mut Model, key: &str) {
    let now = m.now;
    let calls = calls(m, key);
    let agent_pid = match conn(m, key).info.agent_pid {
        0 => agent(m, key).map_or(0, |a| a.pid),
        p => p as i32,
    };
    let c = conn(m, key);
    let ids: HashSet<&str> = calls.iter().map(|(id, _, _)| id.as_str()).collect();
    c.lives.retain(|id, _| ids.contains(id.as_str()));
    let mut asks = Vec::new();
    for (id, command, start) in calls {
        let s = c.lives.entry(id.clone()).or_default();
        if s.reading || s.at != 0 && now - s.at < LIVE_EVERY {
            continue;
        }
        (s.at, s.reading) = (now, true);
        asks.push((id, command, start));
    }
    for (id, command, start) in asks {
        // The files it writes are worked out on the UI thread, once.
        let files = match conn(m, key).lives.get(&id).and_then(|s| s.files.clone()) {
            Some(f) => f,
            None => {
                let f = m
                    .session
                    .as_ref()
                    .map(|v| v.sess.writes(&command))
                    .unwrap_or_default();
                if let Some(s) = conn(m, key).lives.get_mut(&id) {
                    s.files = Some(f.clone());
                }
                f
            }
        };
        let s = conn(m, key).lives.get(&id).cloned().unwrap_or_default();
        let (key, call) = (key.to_string(), id.clone());
        m.later(
            move || look(&s, agent_pid, &command, start, &files, now),
            move |m, f| {
                let Some(s) = conn(m, &key).lives.get_mut(&call) else {
                    return Out::Idle;
                };
                s.reading = false;
                if s.cpu != f.cpu || s.procs != f.procs || s.still == 0 {
                    s.still = now;
                }
                s.cpu = f.cpu;
                s.procs = f.procs;
                s.tails = f.tails;
                let view = StepLive {
                    now: f.now,
                    tail: tail_of(&s.tails),
                };
                if s.view == view {
                    return Out::Idle;
                }
                s.view = view;
                Out::Draw
            },
        );
    }
    let views: HashMap<String, StepLive> = conn(m, key)
        .lives
        .iter()
        .filter(|(_, s)| !s.view.now.is_empty() || !s.view.tail.is_empty())
        .map(|(id, s)| (id.clone(), s.view.clone()))
        .collect();
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key)
        && v.o.live != views
    {
        v.o.live = views;
    }
}

/// The last lines of the freshest file written.
pub fn tail_of(tails: &[Option<Tailed>]) -> Vec<String> {
    let Some(t) = tails
        .iter()
        .flatten()
        .filter(|t| t.lines.iter().any(|l| !l.trim().is_empty()))
        .max_by_key(|t| t.mod_at)
    else {
        return Vec::new();
    };
    let lines: Vec<&String> = t.lines.iter().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(TAIL_SHOWN)..]
        .iter()
        .map(|l| l.trim().to_string())
        .collect()
}

/// Reads the processes and files for one call. Blocks: off the UI thread.
fn look(s: &Seen, agent: Pid, command: &str, start: Ms, files: &[String], now: Ms) -> Found {
    let mut tails = Vec::new();
    for (i, f) in files.iter().take(FILES_MOST).enumerate() {
        let was = s.tails.get(i).and_then(|t| t.as_ref());
        tails.push(match read_tail(f, was) {
            Some(t) if t.size < 0 => None,
            Some(t) => Some(t),
            None => was.cloned(),
        });
    }
    let (mut text, mut cpu, mut procs) = (String::new(), s.cpu, s.procs);
    if agent > 0 {
        let mut t = proc::snapshot();
        let mut prev = s.table.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sh) = shell_of(&t, agent, command, start) {
            t.fill(prev.as_ref(), &[sh]);
            let (c, n, d) = describe(&t, sh);
            (cpu, procs) = (c, n);
            let still = s.still != 0 && s.cpu == c && s.procs == n;
            let quiet = Duration::from_millis((now - s.still).max(0) as u64);
            text = if still && quiet >= WAIT_AFTER {
                format!(
                    "waiting (no cpu for {})",
                    rush_convo::render::dur(quiet.as_millis() as Ms)
                )
            } else {
                d
            };
        }
        *prev = Some(t);
    }
    Found {
        now: text,
        cpu,
        procs,
        tails,
    }
}

/// The shell under the agent that runs the call: the one whose -c is its
/// command, else the one started nearest to the call's start (within 5s).
fn shell_of(t: &Table, agent: Pid, command: &str, start: Ms) -> Option<Pid> {
    let mut near: Option<(Ms, Pid)> = None;
    for &k in t.children.get(&agent).into_iter().flatten() {
        let Some(p) = t.procs.get(&k) else { continue };
        if !shell_comm(&p.comm) {
            continue;
        }
        if !command.is_empty() && shell_command(&proc::args(k)).trim() == command {
            return Some(k);
        }
        let gap = (ms(p) - start).abs();
        if gap < 5000 && near.is_none_or(|(g, _)| gap < g) {
            near = Some((gap, k));
        }
    }
    near.map(|(_, k)| k)
}

/// A shell's tree: its total CPU time, process count, and the line saying
/// what runs now.
fn describe(t: &Table, sh: Pid) -> (Duration, usize, String) {
    let nodes: Vec<_> = t.tree(sh).into_iter().skip(1).collect();
    let cpu = nodes.iter().map(|n| n.proc.cpu_time).sum();
    let n = nodes.len();
    // Busiest first; at rest, the newest.
    let best = nodes
        .iter()
        .filter(|n| !shell_comm(&n.proc.comm))
        .max_by(|a, b| {
            a.proc
                .cpu
                .total_cmp(&b.proc.cpu)
                .then(a.proc.start.cmp(&b.proc.start))
        })
        .or_else(|| nodes.last());
    let Some(best) = best else {
        return (cpu, n, String::new());
    };
    let p = best.proc;
    let name = process_name(p.pid, &p.comm);
    let load = if p.cpu >= 1.0 {
        format!(" · {:.0}% cpu", p.cpu)
    } else {
        String::new()
    };
    (cpu, n, format!("{name}{load}"))
}

/// A process by what it is: its program, a rustc's crate, a cargo test
/// binary as its tests, a script runner's script.
pub fn process_name(pid: Pid, comm: &str) -> String {
    name_of(&proc::args(pid), comm)
}

/// process_name from its words.
pub fn name_of(args: &[String], comm: &str) -> String {
    let Some(first) = args.first() else {
        return comm.to_string();
    };
    let prog = base(first).to_string();
    if prog == "rustc"
        && let Some(i) = args.iter().position(|a| a == "--crate-name")
        && let Some(c) = args.get(i + 1)
    {
        return format!("rustc {c}");
    }
    if first.contains("/deps/") {
        return format!("{prog} (tests)");
    }
    if matches!(
        prog.as_str(),
        "node" | "python" | "python3" | "ruby" | "bun" | "deno" | "sh" | "bash"
    ) && let Some(a) = args.iter().skip(1).find(|a| !a.starts_with('-'))
    {
        return format!("{prog} {}", base(a));
    }
    if matches!(prog.as_str(), "cargo" | "npm" | "go" | "git" | "make")
        && let Some(a) = args.iter().skip(1).find(|a| !a.starts_with('-'))
    {
        return format!("{prog} {a}");
    }
    prog
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|a| a.to_string()).collect()
    }

    /// A process is named by what it is.
    #[test]
    fn a_process_by_name() {
        for (args, want) in [
            (
                s(&["/u/bin/rustc", "--crate-name", "rush_ui", "x.rs"]),
                "rustc rush_ui",
            ),
            (
                s(&["/t/debug/deps/rush_ui-3f2a1b"]),
                "rush_ui-3f2a1b (tests)",
            ),
            (
                s(&["/u/bin/node", "--inspect", "/a/b/run.js"]),
                "node run.js",
            ),
            (s(&["cargo", "test", "-p", "x"]), "cargo test"),
            (s(&["/bin/sleep", "30"]), "sleep"),
        ] {
            assert_eq!(name_of(&args, "comm"), want, "{args:?}");
        }
        assert_eq!(name_of(&[], "zzz"), "zzz");
    }

    fn tail(lines: &[&str], at: Ms) -> Option<Tailed> {
        Some(Tailed {
            size: 1,
            mod_at: at,
            lines: s(lines),
            nos: Vec::new(),
        })
    }

    /// The tail shown is the last 3 non-blank lines of the freshest file.
    #[test]
    fn the_freshest_files_tail() {
        let tails = [
            tail(&["old1", "old2"], 10),
            tail(&["a", "", "b", "c", "d"], 20),
        ];
        assert_eq!(tail_of(&tails), ["b", "c", "d"]);
        assert!(tail_of(&[None, tail(&["", " "], 5)]).is_empty());
    }

    /// A growing file is read again as it grows, and not when it hasn't.
    #[test]
    fn a_growing_file() {
        let d = std::env::temp_dir().join(format!("rush-livestep-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("out.log");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        let p = f.to_string_lossy().to_string();
        let a = read_tail(&p, None).unwrap();
        assert_eq!(tail_of(&[Some(a.clone())]), ["one", "two"]);
        assert!(read_tail(&p, Some(&a)).is_none());
        std::fs::write(&f, "one\ntwo\nthree\nfour\n").unwrap();
        let b = read_tail(&p, Some(&a)).unwrap();
        assert_eq!(tail_of(&[Some(b)]), ["two", "three", "four"]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
