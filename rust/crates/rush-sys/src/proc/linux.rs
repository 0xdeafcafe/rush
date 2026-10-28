//! The process table on Linux, from /proc.
//!
//! Ported from internal/proc/proc_linux.go:1-170.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::stat::{Stat, parse_stat, ticks};
use super::{Comms, Pid, Proc};

fn page_size() -> u64 {
    // SAFETY: sysconf takes a plain integer.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as u64 } else { 4096 }
}

/// When the system started, the zero of a process's start time.
fn boot_time() -> Option<SystemTime> {
    static B: OnceLock<Option<SystemTime>> = OnceLock::new();
    *B.get_or_init(|| {
        let s = std::fs::read_to_string("/proc/stat").ok()?;
        s.lines().find_map(|l| {
            let secs: u64 = l.strip_prefix("btime ")?.trim().parse().ok()?;
            Some(UNIX_EPOCH + Duration::from_secs(secs))
        })
    })
}

fn read_stat(pid: Pid) -> Option<Stat> {
    parse_stat(&std::fs::read(format!("/proc/{pid}/stat")).ok()?)
}

fn start_of(s: &Stat) -> SystemTime {
    match boot_time() {
        Some(b) => b.checked_add(ticks(s.start_ticks)).unwrap_or(b),
        None => UNIX_EPOCH,
    }
}

static COMMS: Mutex<Option<Comms>> = Mutex::new(None);

pub(super) fn list() -> Vec<Proc> {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut comms = COMMS.lock().unwrap();
    let comms = comms.get_or_insert_with(Comms::new);
    let mut out = Vec::new();
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<Pid>().ok()) else {
            continue;
        };
        if pid <= 0 {
            continue;
        }
        let Some(s) = read_stat(pid) else {
            continue; // gone since the directory was read
        };
        out.push(Proc {
            pid,
            ppid: s.ppid,
            comm: comms.get(s.comm.as_bytes()),
            start: start_of(&s),
            ..Proc::default()
        });
    }
    out
}

/// Reads the CPU time from stat and the resident memory from statm, the
/// nearest Linux has to the footprint macOS reports.
pub(super) fn fill_usage(p: &mut Proc) {
    if let Some(s) = read_stat(p.pid) {
        p.cpu_time = s.cpu;
    }
    let Ok(b) = std::fs::read_to_string(format!("/proc/{}/statm", p.pid)) else {
        return;
    };
    if let Some(rss) = b
        .split_ascii_whitespace()
        .nth(1)
        .and_then(|f| f.parse::<u64>().ok())
    {
        p.footprint = rss * page_size();
    }
}

fn nul_list(path: String) -> Vec<String> {
    match std::fs::read(path) {
        Ok(b) if !b.is_empty() => {
            let s = String::from_utf8_lossy(&b);
            s.trim_end_matches('\0')
                .split('\0')
                .map(String::from)
                .collect()
        }
        _ => Vec::new(),
    }
}

/// A process's argv, from /proc/<pid>/cmdline.
pub(super) fn args(pid: Pid) -> Vec<String> {
    nul_list(format!("/proc/{pid}/cmdline"))
}

/// A process's environment, from /proc/<pid>/environ.
pub(super) fn env(pid: Pid) -> Vec<String> {
    nul_list(format!("/proc/{pid}/environ"))
}

pub(super) fn zombie(pid: Pid) -> bool {
    let Ok(b) = std::fs::read(format!("/proc/{pid}/stat")) else {
        return false;
    };
    match b.iter().rposition(|&c| c == b')') {
        Some(end) => end + 2 < b.len() && b[end + 2] == b'Z',
        None => false,
    }
}

pub(super) fn started(pid: Pid) -> Option<SystemTime> {
    let s = read_stat(pid)?;
    boot_time()?;
    Some(start_of(&s))
}
