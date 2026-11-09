//! The process table on Linux, from /proc: stat for the parent, name, CPU
//! time and start, statm for memory, cmdline and environ for arguments,
//! the task's children file for a tree's walk.
//!
//! Ported from internal/proc/proc_linux.go.

use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Comms, Pid, Proc};

/// One /proc/<pid>/stat: the command, the parent, the state, the CPU time
/// in clock ticks and the start in clock ticks after boot.
#[derive(Debug, PartialEq)]
pub(crate) struct Stat<'a> {
    pub comm: &'a [u8],
    pub state: u8,
    pub ppid: Pid,
    pub cpu_ticks: u64,
    pub start_ticks: u64,
}

/// Reads the stat line. The command sits in parentheses and may hold
/// spaces or parentheses itself, so the fields after it are counted from
/// its last ')'.
pub(crate) fn parse_stat(b: &[u8]) -> Option<Stat<'_>> {
    let open = b.iter().position(|&c| c == b'(')?;
    let end = b.iter().rposition(|&c| c == b')')?;
    if end < open {
        return None;
    }
    let rest = std::str::from_utf8(&b[end + 1..]).ok()?;
    // f[0] is the state (field 3); ppid is field 4, utime 14, stime 15,
    // starttime 22.
    let f: Vec<&str> = rest.split_ascii_whitespace().collect();
    if f.len() < 20 {
        return None;
    }
    let n = |i: usize| f[i].parse::<u64>().unwrap_or(0);
    Some(Stat {
        comm: &b[open + 1..end],
        state: f[0].as_bytes().first().copied().unwrap_or(0),
        ppid: f[1].parse().unwrap_or(0),
        cpu_ticks: n(11) + n(12),
        start_ticks: n(19),
    })
}

fn read(pid: Pid, what: &str) -> Option<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/{what}")).ok()
}

/// USER_HZ: the unit of the times in /proc/<pid>/stat.
fn ticks_per_sec() -> u64 {
    static T: OnceLock<u64> = OnceLock::new();
    // SAFETY: sysconf takes a plain name.
    *T.get_or_init(|| match unsafe { libc::sysconf(libc::_SC_CLK_TCK) } {
        n if n > 0 => n as u64,
        _ => 100,
    })
}

fn ticks(t: u64) -> Duration {
    Duration::from_nanos(t.saturating_mul(1_000_000_000) / ticks_per_sec())
}

/// When the machine started, the zero of a process's start time.
fn boot() -> Option<SystemTime> {
    static B: OnceLock<Option<SystemTime>> = OnceLock::new();
    *B.get_or_init(|| {
        let s = std::fs::read_to_string("/proc/stat").ok()?;
        let secs = s
            .lines()
            .find_map(|l| l.strip_prefix("btime "))?
            .trim()
            .parse::<u64>()
            .ok()?;
        Some(UNIX_EPOCH + Duration::from_secs(secs))
    })
}

fn start_of(s: &Stat) -> SystemTime {
    boot().map_or(UNIX_EPOCH, |b| b + ticks(s.start_ticks))
}

fn page_size() -> u64 {
    static P: OnceLock<u64> = OnceLock::new();
    // SAFETY: sysconf takes a plain name.
    *P.get_or_init(|| match unsafe { libc::sysconf(libc::_SC_PAGESIZE) } {
        n if n > 0 => n as u64,
        _ => 4096,
    })
}

/// Resident memory, the nearest Linux has to macOS's footprint.
fn rss(pid: Pid) -> Option<u64> {
    let b = read(pid, "statm")?;
    let s = std::str::from_utf8(&b).ok()?;
    let pages: u64 = s.split_ascii_whitespace().nth(1)?.parse().ok()?;
    Some(pages * page_size())
}

/// Each process name once, as most are the same from one sample to the
/// next.
static COMMS: Mutex<Option<Comms>> = Mutex::new(None);

fn pids() -> Vec<Pid> {
    let Ok(d) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    d.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<Pid>().ok())
        .filter(|&p| p > 0)
        .collect()
}

pub(super) fn list() -> Vec<Proc> {
    let mut g = COMMS.lock().unwrap();
    let comms = g.get_or_insert_with(Comms::new);
    let mut out = Vec::new();
    for pid in pids() {
        let Some(b) = read(pid, "stat") else {
            continue; // gone since the folder was read
        };
        let Some(s) = parse_stat(&b) else { continue };
        out.push(Proc {
            pid,
            ppid: s.ppid,
            comm: comms.get(s.comm),
            start: start_of(&s),
            ..Proc::default()
        });
    }
    out
}

pub(super) fn fill_usage(p: &mut Proc) {
    if let Some(b) = read(p.pid, "stat")
        && let Some(s) = parse_stat(&b)
    {
        p.cpu_time = ticks(s.cpu_ticks);
    }
    if let Some(m) = rss(p.pid) {
        p.footprint = m;
    }
}

/// A NUL-separated list (cmdline, environ), the last NUL trimmed.
fn split_nul(b: &[u8]) -> Vec<String> {
    let b = b.strip_suffix(&[0]).unwrap_or(b);
    if b.is_empty() {
        return Vec::new();
    }
    b.split(|&c| c == 0)
        .map(|x| String::from_utf8_lossy(x).into_owned())
        .collect()
}

pub(super) fn args(pid: Pid) -> Vec<String> {
    read(pid, "cmdline").map_or_else(Vec::new, |b| split_nul(&b))
}

pub(super) fn env(pid: Pid) -> Vec<String> {
    readable_env(pid).unwrap_or_default()
}

/// None when the kernel won't give it: another user's process (the read
/// is refused), or a zombie's or a gone process's (nothing there).
pub(super) fn readable_env(pid: Pid) -> Option<Vec<String>> {
    let b = read(pid, "environ")?;
    if b.is_empty() && (zombie(pid) || stat(pid).is_none()) {
        return None;
    }
    Some(split_nul(&b))
}

fn stat(pid: Pid) -> Option<(Vec<u8>, u8, SystemTime)> {
    let b = read(pid, "stat")?;
    let s = parse_stat(&b)?;
    Some((s.comm.to_vec(), s.state, start_of(&s)))
}

pub(super) fn comm(pid: Pid) -> Option<String> {
    stat(pid).map(|(c, _, _)| String::from_utf8_lossy(&c).into_owned())
}

pub(super) fn zombie(pid: Pid) -> bool {
    pid > 0 && stat(pid).is_some_and(|(_, st, _)| st == b'Z')
}

pub(super) fn started(pid: Pid) -> Option<SystemTime> {
    if pid <= 0 || boot().is_none() {
        return None;
    }
    stat(pid).map(|(_, _, t)| t)
}

/// pid's resident memory, CPU time in nanoseconds, and start in clock
/// ticks after boot (only to tell a reused pid); None when it's gone. For
/// [`super::Sampler`], which reads no process table.
pub(super) fn usage(pid: Pid) -> Option<(u64, u64, u64)> {
    let b = read(pid, "stat")?;
    let s = parse_stat(&b)?;
    let mem = rss(pid).unwrap_or(0);
    Some((mem, ticks(s.cpu_ticks).as_nanos() as u64, s.start_ticks))
}

/// pid's children, into out: each of its threads' children files, or,
/// on a kernel without them, a walk of the whole table.
pub(super) fn children(pid: Pid, out: &mut Vec<Pid>) {
    out.clear();
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return;
    };
    let mut any = false;
    for t in tasks.flatten() {
        let Ok(s) = std::fs::read_to_string(t.path().join("children")) else {
            continue;
        };
        any = true;
        out.extend(
            s.split_ascii_whitespace()
                .filter_map(|p| p.parse::<Pid>().ok()),
        );
    }
    if !any {
        out.extend(list().into_iter().filter(|p| p.ppid == pid).map(|p| p.pid));
    }
    out.retain(|&p| p > 0);
}

/// The regular file pid's descriptor fd is open on, by its path; None for
/// anything else (a pipe, a socket, a terminal, /dev/null) or a process
/// not ours to read. Nothing is read from the descriptor.
pub(super) fn fd_file(pid: Pid, fd: i32) -> Option<PathBuf> {
    let link = format!("/proc/{pid}/fd/{fd}");
    // Through the link: the file itself, whatever its name now.
    let m = std::fs::metadata(&link).ok()?;
    if !m.file_type().is_file() {
        return None;
    }
    let p = std::fs::read_link(&link).ok()?;
    p.is_absolute().then_some(p)
}

/// A descriptor that reads as ready once pid exits (pidfd_open(2)); None
/// on a kernel without it, or when pid is gone.
pub fn pidfd(pid: Pid) -> Option<OwnedFd> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: pidfd_open takes a pid and flags (none: a pidfd is always
    // close-on-exec); a non-negative result is a descriptor ours to own.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0 as libc::c_uint) };
    // SAFETY: as above.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd as i32) })
}
