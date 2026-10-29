//! What an open conversation's Bash calls run: the shells and the
//! command of a chain running now, and pausing one. After Go's
//! internal/ui/shells.go (shellComm :84, paused :217, pausedIDs :253,
//! pauseWord :267, shellCommand :300). Reading the process tree and
//! sending the signals wait on rush hosting sessions (Claude Code's own
//! shells); this keeps which commands are paused, by tool call.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use rush_core::time::Ms;

/// Whether c is a shell's program, a login shell's too (shellComm).
pub fn shell_comm(c: &str) -> bool {
    matches!(c.strip_prefix('-').unwrap_or(c), "zsh" | "bash" | "sh")
}

/// The script a shell runs with -c, read from its argv, not a flattened
/// command line; "" for an interactive shell or a script file
/// (shellCommand).
pub fn shell_command(args: &[String]) -> &str {
    if args.len() < 3 {
        return "";
    }
    let prog = args[0].rsplit('/').next().unwrap_or("");
    if !shell_comm(prog) {
        return "";
    }
    for i in 1..args.len() - 1 {
        let a = &args[i];
        if a == "--" || !a.starts_with('-') {
            return "";
        }
        if !a.starts_with("--") && a.contains('c') {
            return &args[i + 1];
        }
    }
    ""
}

/// A command of a running chain rush has paused: when, what, and the
/// processes it stopped, by pid and start (pausedPart).
#[derive(Clone, Debug, Default)]
pub struct Paused {
    pub at: Ms,
    pub what: String,
    pub procs: Vec<(i32, Ms)>,
}

/// The commands paused in every session, by tool call id: let go when
/// rush quits, so none is left stopped with nobody to start it (paused).
static PAUSED: LazyLock<Mutex<HashMap<String, Paused>>> = LazyLock::new(Default::default);

/// Notes call id's command as paused.
pub fn pause(id: &str, p: Paused) {
    PAUSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.to_string(), p);
}

/// Forgets call id's pause, handing back what was paused to let go on.
pub fn resume(id: &str) -> Option<Paused> {
    PAUSED.lock().unwrap_or_else(|e| e.into_inner()).remove(id)
}

/// Every pause, taken: what rush lets go on as it quits (resumeAll).
pub fn take_all() -> Vec<Paused> {
    let mut m = PAUSED.lock().unwrap_or_else(|e| e.into_inner());
    m.drain().map(|(_, p)| p).collect()
}

/// The Bash calls paused now, for the conversation to mark (pausedIDs).
pub fn paused_ids() -> HashSet<String> {
    PAUSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect()
}

/// What p does to call id now (pauseWord).
pub fn pause_word(id: &str) -> &'static str {
    if PAUSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(id)
    {
        "let it go on"
    } else {
        "pause it"
    }
}
