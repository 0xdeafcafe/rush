//! What rush does to processes and checkouts itself: ending process trees,
//! notifications, git worktrees. What an agent does through its own
//! program is its adapter's.
//!
//! Ported from internal/actions/actions.go:1-122; worktrees in
//! `worktree.rs` (worktree.go).

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime};

use crate::proc::{self, KillError, Pid, Table};

mod worktree;
pub use worktree::{
    Made, main_root, make_worktree, new_worktree, new_worktree_on, repo_root, worktree_at,
};

/// Why a tree wasn't touched.
#[derive(Debug, PartialEq, Eq)]
pub enum TreeError {
    /// Root isn't running, or is another process now (another start time).
    Exited(Pid),
    /// Root is rush or one of its parents.
    IsRush,
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TreeError::Exited(pid) => write!(f, "process {pid} has already exited"),
            TreeError::IsRush => f.write_str("that would kill rush itself"),
        }
    }
}

impl std::error::Error for TreeError {}

/// SIGKILLs a process and everything under it, deepest first. It samples
/// the process table afresh, refuses if root is no longer the process the
/// user chose (same start time), and never touches rush or its parents.
/// It reports how many processes it killed.
pub fn kill_tree(root: Pid, root_start: SystemTime) -> Result<usize, TreeError> {
    let (tab, protected) = tree_of(root, root_start)?;
    let _ = proc::kill(root, libc::SIGSTOP); // freeze it so it can't spawn more while we walk
    Ok(tab
        .descendants(root)
        .into_iter()
        .filter(|pid| !protected.contains(pid) && proc::kill(*pid, libc::SIGKILL).is_ok())
        .count())
}

/// Asks a process and everything under it to stop (SIGTERM, all of them,
/// so a shell's children don't outlive it with nobody to stop them), then
/// SIGKILLs whatever is still there after grace. It reports how many
/// processes it ended.
pub fn end_tree(root: Pid, root_start: SystemTime, grace: Duration) -> Result<usize, TreeError> {
    let (tab, protected) = tree_of(root, root_start)?;
    let pids: Vec<Pid> = tab
        .descendants(root)
        .into_iter()
        .filter(|pid| !protected.contains(pid) && proc::kill(*pid, libc::SIGTERM).is_ok())
        .collect();
    let end = Instant::now() + grace;
    #[cfg(target_os = "macos")]
    let left = exits::wait(&tab, &pids, end).unwrap_or_else(|| poll_exits(&tab, &pids, end));
    #[cfg(not(target_os = "macos"))]
    let left = poll_exits(&tab, &pids, end);
    for pid in left {
        let _ = proc::kill(pid, libc::SIGKILL);
    }
    Ok(pids.len())
}

/// Which of pids (all in tab) are still the processes tab saw at end,
/// sampling the table every 150 ms as Go does: where the kernel can't say
/// when a process exits.
fn poll_exits(tab: &Table, pids: &[Pid], end: Instant) -> Vec<Pid> {
    let alive = || {
        let now = proc::snapshot();
        pids.iter()
            .copied()
            .filter(|pid| {
                now.procs
                    .get(pid)
                    .is_some_and(|p| tab.procs.get(pid).is_some_and(|t| t.start == p.start))
            })
            .collect::<Vec<_>>()
    };
    let mut left = pids.to_vec();
    while !left.is_empty() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(150));
        left = alive();
    }
    left
}

/// Waiting for processes to exit on kqueue: it returns the moment the last
/// one goes, rather than at the next look.
#[cfg(target_os = "macos")]
mod exits {
    use std::collections::HashSet;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::time::Instant;

    use crate::proc::{self, Pid, Table};

    /// Which of pids (all in tab) are still running at end; None when
    /// there's no kqueue to wait on.
    pub(super) fn wait(tab: &Table, pids: &[Pid], end: Instant) -> Option<Vec<Pid>> {
        // SAFETY: kqueue takes nothing; a non-negative result is ours.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return None;
        }
        // SAFETY: kq is a descriptor we just made.
        let kq = unsafe { OwnedFd::from_raw_fd(kq) };
        let mut waiting: HashSet<Pid> = HashSet::new();
        for &pid in pids {
            // SAFETY: kevent is plain data.
            let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
            ev.ident = pid as usize;
            ev.filter = libc::EVFILT_PROC;
            ev.flags = libc::EV_ADD | libc::EV_ONESHOT;
            ev.fflags = libc::NOTE_EXIT;
            // SAFETY: one valid change, no events read.
            let r = unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    &ev,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            if r < 0 {
                continue; // gone already
            }
            // Watched now, so it can't exit unseen; but it may have exited
            // (a zombie) or been replaced before it was.
            let same = proc::started(pid)
                .is_some_and(|s| tab.procs.get(&pid).is_some_and(|p| p.start == s));
            if same && !proc::zombie(pid) {
                waiting.insert(pid);
            }
        }
        // SAFETY: kevent is plain data.
        let mut events =
            vec![unsafe { std::mem::zeroed::<libc::kevent>() }; waiting.len().clamp(1, 64)];
        while !waiting.is_empty() {
            let Some(d) = end
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
            else {
                break;
            };
            let ts = libc::timespec {
                tv_sec: d.as_secs() as libc::time_t,
                tv_nsec: d.subsec_nanos() as libc::c_long,
            };
            // SAFETY: events is valid for its length; ts lives across the call.
            let n = unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as i32,
                    &ts,
                )
            };
            if n < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return None;
            }
            for ev in &events[..n as usize] {
                waiting.remove(&(ev.ident as Pid));
            }
        }
        Some(
            pids.iter()
                .copied()
                .filter(|p| waiting.contains(p))
                .collect(),
        )
    }
}

/// Sends sig to a process and everything under it: SIGSTOP to pause the
/// lot, SIGCONT to let it go on. Root goes first, so a paused tree can't
/// start more while it's walked. It reports how many it signalled.
pub fn signal_tree(root: Pid, root_start: SystemTime, sig: i32) -> Result<usize, TreeError> {
    let (tab, protected) = tree_of(root, root_start)?;
    let _ = proc::kill(root, sig);
    Ok(1 + tab
        .descendants(root)
        .into_iter()
        .filter(|&pid| pid != root && !protected.contains(&pid) && proc::kill(pid, sig).is_ok())
        .count())
}

/// Samples the process table for a tree about to be ended, refusing if
/// root is no longer the process the user chose (same start time); the
/// protected set is rush and its parents.
fn tree_of(root: Pid, root_start: SystemTime) -> Result<(Table, HashSet<Pid>), TreeError> {
    let tab = proc::snapshot();
    match tab.procs.get(&root) {
        Some(p) if p.start == root_start => {}
        _ => return Err(TreeError::Exited(root)),
    }
    let mut protected = HashSet::from([1]);
    let mut pid = std::process::id() as Pid;
    while pid > 1 {
        protected.insert(pid);
        let Some(q) = tab.procs.get(&pid) else {
            break;
        };
        pid = q.ppid;
    }
    if protected.contains(&root) {
        return Err(TreeError::IsRush);
    }
    Ok((tab, protected))
}

/// Asks pid to stop (SIGTERM).
pub fn terminate(pid: Pid) -> Result<(), KillError> {
    proc::kill(pid, libc::SIGTERM)
}

/// Posts a macOS notification; it spawns only on state transitions.
pub fn notify(title: &str, body: &str) {
    let script = format!(
        "display notification {} with title {}",
        go_quote(body),
        go_quote(title)
    );
    if let Ok(mut c) = std::process::Command::new("osascript")
        .args(["-e", &script])
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

/// s as Go's %q writes it, which AppleScript reads as a string: double
/// quotes, with backslash escapes.
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\x0b' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests;
