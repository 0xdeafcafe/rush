//! Reading Linux's /proc/<pid>/stat line. Kept apart from the rest of the
//! Linux backend so it is tested on every system.
//!
//! Ported from internal/proc/proc_linux.go:14-71.

use std::time::Duration;

/// USER_HZ, the unit of the times in /proc/<pid>/stat. It is 100 on every
/// Linux architecture.
pub const CLOCK_TICKS: u64 = 100;

/// A count of USER_HZ ticks as a duration, saturating rather than
/// overflowing on a stat line that says nonsense.
pub fn ticks(n: u64) -> Duration {
    Duration::from_nanos(n.saturating_mul(1_000_000_000 / CLOCK_TICKS))
}

/// One /proc/<pid>/stat: the command, the parent, the CPU time and the
/// start time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    pub comm: String,
    pub ppid: i32,
    pub cpu: Duration,
    pub start_ticks: u64,
}

/// Reads the stat line. The command sits in parentheses and may hold
/// spaces or parentheses itself, so the fields after it are counted from
/// its last ')'.
pub fn parse_stat(b: &[u8]) -> Option<Stat> {
    let open = b.iter().position(|&c| c == b'(')?;
    let end = b.iter().rposition(|&c| c == b')')?;
    if end < open {
        return None;
    }
    let rest = String::from_utf8_lossy(&b[end + 1..]);
    let f: Vec<&str> = rest.split_ascii_whitespace().collect();
    // f[0] is the state (field 3); ppid is field 4, utime 14, stime 15,
    // starttime 22.
    if f.len() < 20 {
        return None;
    }
    let ppid = f[1].parse().unwrap_or(0);
    let utime: u64 = f[11].parse().unwrap_or(0);
    let stime: u64 = f[12].parse().unwrap_or(0);
    let start = f[19].parse().unwrap_or(0);
    Some(Stat {
        comm: String::from_utf8_lossy(&b[open + 1..end]).into_owned(),
        ppid,
        cpu: ticks(utime.saturating_add(stime)),
        start_ticks: start,
    })
}
