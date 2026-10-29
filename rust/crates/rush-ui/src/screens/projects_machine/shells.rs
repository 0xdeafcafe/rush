//! The machine side of a conversation's shells: what its Bash calls have
//! running (the shells the agent runs them in and every process under
//! them), and the commands rush has paused. Go: internal/ui/shells.go
//! (watchShells :24, shellComm :84, shellKids :93, pausedPart :202,
//! paused :210, pauseWord :267, pausedIDs :253, resumeAll :277,
//! signalParts :287, shellCommand :300). Reading is for a worker, never the
//! UI thread; the conversation's side (which command of a chain runs now,
//! the k and p keys) is the session's.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use rush_core::time::Ms;
use rush_sys::proc::{Pid, Table};

/// A shell the agent runs a Bash call in, and what runs under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shell {
    /// Its command line, whole.
    pub cmd: String,
    /// The script it was given (-c), or "" when it can't be matched to a
    /// call by its words alone.
    pub command: String,
    pub start: Option<SystemTime>,
    pub kids: Vec<ShellProc>,
}

/// A process under a shell, with its words.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellProc {
    pub pid: Pid,
    pub args: Vec<String>,
    pub start: Option<SystemTime>,
    pub kids: Vec<ShellProc>,
}

/// A process of a running command, to signal: by pid and start, so a pid
/// reused is never touched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartProc {
    pub pid: Pid,
    pub start: SystemTime,
}

/// Whether a command name is a shell a Bash call runs in.
pub fn shell_comm(c: &str) -> bool {
    matches!(c.trim_start_matches('-'), "zsh" | "bash" | "sh")
}

/// The script a shell was given, read from its argv (spaces and quotes in
/// it count): "" for an interactive shell or one running a script file,
/// which can't be matched to a call.
pub fn shell_command(args: &[String]) -> String {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    if args.len() < 3 || !shell_comm(&base(&args[0])) {
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

fn started(t: SystemTime) -> Option<SystemTime> {
    (t != SystemTime::UNIX_EPOCH).then_some(t)
}

/// The process tree under pid, each with its words (args_of reads them),
/// at most 6 deep.
pub fn shell_kids(
    tab: &Table,
    pid: Pid,
    depth: usize,
    args_of: &dyn Fn(Pid) -> Vec<String>,
) -> Vec<ShellProc> {
    if depth > 6 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &k in tab
        .children
        .get(&pid)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(p) = tab.procs.get(&k) else {
            continue;
        };
        let mut args = args_of(k);
        if args.is_empty() {
            args = vec![p.comm.to_string()];
        }
        out.push(ShellProc {
            pid: k,
            args,
            start: started(p.start),
            kids: shell_kids(tab, k, depth + 1, args_of),
        });
    }
    out
}

/// The shells the agent at pid runs its Bash calls in, and what each runs.
/// A shell that's an eval wrapper keeps no command: Claude Code's own are
/// matched by its hook's rewrite, and anything else (an MCP server, a
/// hook) can't take a call by its timing alone.
pub fn read_shells(tab: &Table, pid: Pid, args_of: &dyn Fn(Pid) -> Vec<String>) -> Vec<Shell> {
    let mut out = Vec::new();
    for &k in tab
        .children
        .get(&pid)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(p) = tab.procs.get(&k) else {
            continue;
        };
        if !shell_comm(&p.comm) {
            continue;
        }
        let args = args_of(k);
        let mut command = shell_command(&args);
        let cmd = args.join(" ");
        if command.is_empty() {
            continue;
        }
        if cmd.contains(" eval ") {
            command.clear();
        }
        out.push(Shell {
            cmd,
            command,
            start: started(p.start),
            kids: shell_kids(tab, k, 0, args_of),
        });
    }
    out
}

/// A command of a running chain rush has paused: when, its first word,
/// and the processes it stopped, to let go on again.
#[derive(Clone, Debug)]
pub struct PausedPart {
    pub at: Ms,
    pub what: String,
    pub procs: Vec<PartProc>,
}

/// The commands paused in every session, by tool call id: let go when
/// rush quits, so none is left stopped with nobody to start it.
static PAUSED: LazyLock<Mutex<HashMap<String, PausedPart>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn paused() -> std::sync::MutexGuard<'static, HashMap<String, PausedPart>> {
    PAUSED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Notes call id's command as paused.
pub fn note_paused(id: &str, p: PausedPart) {
    paused().insert(id.to_string(), p);
}

/// Takes call id's paused command, if it is: it's going on again.
pub fn take_paused(id: &str) -> Option<PausedPart> {
    paused().remove(id)
}

/// The Bash calls paused now, for the conversation to mark.
pub fn paused_ids() -> HashSet<String> {
    paused().keys().cloned().collect()
}

/// What p does to Bash call id now.
pub fn pause_word(id: &str) -> &'static str {
    if paused().contains_key(id) {
        "let it go on"
    } else {
        "pause it"
    }
}

/// Lets every paused command go on: as rush quits. A frame asking
/// paused_ids needn't wait for the signals.
pub fn resume_all() {
    let all = std::mem::take(&mut *paused());
    for p in all.values() {
        let _ = signal_parts(&p.procs, libc::SIGCONT);
    }
}

/// Sends sig to each process and everything under it, off the UI.
pub fn signal_parts(procs: &[PartProc], sig: i32) -> Result<(), String> {
    let errs: Vec<String> = procs
        .iter()
        .filter_map(|p| {
            rush_sys::actions::signal_tree(p.pid, p.start, sig)
                .err()
                .map(|e| e.to_string())
        })
        .collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_sys::proc::Proc;
    use std::sync::Arc;
    use std::time::{Duration, Instant, UNIX_EPOCH};

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn shells_and_their_scripts() {
        for c in ["zsh", "-zsh", "bash", "sh", "-bash"] {
            assert!(shell_comm(c), "{c}");
        }
        for c in ["fish", "node", "zshx"] {
            assert!(!shell_comm(c), "{c}");
        }
        let cases: &[(&[&str], &str)] = &[
            (&["/bin/zsh", "-c", "pnpm test"], "pnpm test"),
            (&["/bin/zsh", "-lc", "echo 'a b'"], "echo 'a b'"),
            (&["bash", "--login", "-c", "make"], "make"),
            (&["zsh", "-l"], ""),
            (&["zsh", "script.sh", "x"], ""),
            (&["zsh", "--", "-c", "x"], ""),
            (&["node", "-c", "x"], ""),
            (&["zsh", "--norc", "x"], ""),
        ];
        for (args, want) in cases {
            assert_eq!(shell_command(&s(args)), *want, "{args:?}");
        }
    }

    fn table(procs: &[(Pid, Pid, &str)]) -> Table {
        let mut t = Table {
            at: Instant::now(),
            procs: Default::default(),
            children: Default::default(),
        };
        for &(pid, ppid, comm) in procs {
            t.procs.insert(
                pid,
                Proc {
                    pid,
                    ppid,
                    comm: Arc::from(comm),
                    start: UNIX_EPOCH + Duration::from_secs(pid as u64),
                    ..Proc::default()
                },
            );
            t.children.entry(ppid).or_default().push(pid);
        }
        t
    }

    #[test]
    fn reads_what_an_agents_shells_run() {
        // claude 10: a Bash call's shell 11 running a test 12 with a
        // child 13, an eval wrapper 14, and an MCP server 15.
        let t = table(&[
            (10, 1, "claude"),
            (11, 10, "zsh"),
            (12, 11, "node"),
            (13, 12, "esbuild"),
            (14, 10, "zsh"),
            (15, 10, "node"),
        ]);
        let args = |pid: Pid| match pid {
            11 => s(&["/bin/zsh", "-c", "pnpm test"]),
            12 => s(&["node", "vitest"]),
            14 => s(&["/bin/zsh", "-c", "source x && eval 'ls' && pwd"]),
            _ => Vec::new(),
        };
        let got = read_shells(&t, 10, &args);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].command, "pnpm test");
        assert_eq!(got[0].cmd, "/bin/zsh -c pnpm test");
        assert_eq!(got[0].kids[0].args, s(&["node", "vitest"]));
        assert_eq!(
            got[0].kids[0].kids[0].args,
            s(&["esbuild"]),
            "its name when its words can't be read"
        );
        assert_eq!(got[1].command, "", "an eval wrapper is matched by the hook");
    }

    #[test]
    fn paused_calls() {
        let id = "toolu_paused_test";
        assert_eq!(pause_word(id), "pause it");
        note_paused(
            id,
            PausedPart {
                at: 0,
                what: "pnpm".into(),
                procs: Vec::new(),
            },
        );
        assert_eq!(pause_word(id), "let it go on");
        assert!(paused_ids().contains(id));
        assert_eq!(take_paused(id).map(|p| p.what), Some("pnpm".into()));
        assert_eq!(pause_word(id), "pause it");
    }
}
