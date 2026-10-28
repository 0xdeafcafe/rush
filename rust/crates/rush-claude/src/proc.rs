//! A process's arguments, to tell an interactive-kind session that is
//! really `claude -p` driven by another program, and what an agent's
//! processes use. Port of internal/fleet/fleet.go:1538 isPrint,
//! proc.Args, and proc.Fill/Sum with fleet.go:1263 (sample): memory, CPU
//! and how many processes, summed over each agent's process tree.
//!
//! Go samples the whole process table; this walks only the agents' trees
//! (proc_listchildpids), so a sample costs a few calls per agent and keeps
//! nothing but the last CPU times of the processes it saw.

use std::collections::HashMap;
use std::time::Instant;

/// Whether args run Claude Code in print mode, and whether it keeps no
/// transcript at all.
pub fn is_print<S: AsRef<str>>(args: &[S]) -> (bool, bool) {
    let (mut print, mut unstored) = (false, false);
    for a in args {
        let a = a.as_ref();
        if a == "-p" || a == "--print" || a.starts_with("--output-format") {
            print = true;
        }
        if a == "--no-session-persistence" {
            unstored = true;
        }
    }
    (print, unstored)
}

/// pid's argument list; empty when it can't be read.
#[cfg(target_os = "macos")]
pub fn args(pid: i32) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: sysctl with a null buffer only reports the size.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || size < 4 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size];
    // SAFETY: buf is size bytes long, as sysctl is told.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || size < 4 {
        return Vec::new();
    }
    buf.truncate(size);
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
    let mut rest = &buf[4..];
    // The executable's path, then padding NULs, then argv.
    let Some(end) = rest.iter().position(|&c| c == 0) else {
        return Vec::new();
    };
    rest = &rest[end..];
    let start = rest.iter().position(|&c| c != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    rest.split(|&c| c == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

/// pid's argument list; empty when it can't be read.
#[cfg(not(target_os = "macos"))]
pub fn args(pid: i32) -> Vec<String> {
    let Ok(b) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return Vec::new();
    };
    b.split(|&c| c == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

/// What one process tree uses.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// Bytes: what Activity Monitor calls Memory.
    pub mem: u64,
    /// Percent of one core since the previous sample.
    pub cpu: f32,
    pub procs: u32,
}

/// One process as sampled: when it started, to tell a reused pid, and
/// the CPU time it had used.
#[derive(Clone, Copy)]
struct Seen {
    start: u64,
    cpu_ns: u64,
}

/// Samples process trees, keeping each process's last CPU time as the
/// baseline for the next sample's percentages.
#[derive(Default)]
pub struct Sampler {
    prev: HashMap<i32, Seen>,
    at: Option<Instant>,
}

impl Sampler {
    pub fn new() -> Sampler {
        Sampler::default()
    }

    /// Each root's tree, summed: the root and everything under it.
    pub fn sample(&mut self, roots: &[i32]) -> HashMap<i32, Usage> {
        let now = Instant::now();
        let dt = self.at.map(|t| now.duration_since(t).as_nanos() as f64);
        let mut seen: HashMap<i32, Seen> = HashMap::with_capacity(self.prev.len());
        let mut out = HashMap::with_capacity(roots.len());
        let mut stack: Vec<i32> = Vec::new();
        let mut kids: Vec<i32> = Vec::new();
        for &root in roots {
            if root <= 0 || out.contains_key(&root) {
                continue;
            }
            let mut u = Usage::default();
            stack.clear();
            stack.push(root);
            // A tree can't loop, but a reused pid could: never count one twice.
            let mut walked = 0;
            while let Some(pid) = stack.pop() {
                walked += 1;
                if walked > 4096 {
                    break;
                }
                let Some((mem, cpu_ns, start)) = usage(pid) else {
                    continue;
                };
                u.mem += mem;
                u.procs += 1;
                if let (Some(dt), Some(old)) = (dt, self.prev.get(&pid))
                    && old.start == start
                    && cpu_ns >= old.cpu_ns
                    && dt > 0.0
                {
                    u.cpu += ((cpu_ns - old.cpu_ns) as f64 / dt * 100.0) as f32;
                }
                seen.insert(pid, Seen { start, cpu_ns });
                children(pid, &mut kids);
                stack.extend_from_slice(&kids);
            }
            out.insert(root, u);
        }
        self.prev = seen;
        self.at = Some(now);
        out
    }
}

/// pid's footprint, CPU time in nanoseconds, and start time; None when
/// it's gone or not ours to read.
#[cfg(target_os = "macos")]
fn usage(pid: i32) -> Option<(u64, u64, u64)> {
    use std::sync::OnceLock;
    static FREQ: OnceLock<u64> = OnceLock::new();
    // Apple silicon reports rusage times in timebase ticks, Intel in ns.
    let &freq = FREQ.get_or_init(|| {
        let mut f: u64 = 0;
        let mut n = std::mem::size_of::<u64>();
        // SAFETY: a u64 out-buffer of the size given.
        let r = unsafe {
            libc::sysctlbyname(
                c"hw.tbfrequency".as_ptr(),
                (&mut f as *mut u64).cast(),
                &mut n,
                std::ptr::null_mut(),
                0,
            )
        };
        if r == 0 && f > 0 { f } else { 1_000_000_000 }
    });
    // SAFETY: zeroed is a valid rusage_info_v2.
    let mut ri: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer is a rusage_info_v2, as the flavor says.
    let r = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut ri as *mut libc::rusage_info_v2).cast(),
        )
    };
    if r != 0 {
        return None;
    }
    let ticks = ri.ri_user_time + ri.ri_system_time;
    Some((
        ri.ri_phys_footprint,
        (ticks as u128 * 1_000_000_000 / freq as u128) as u64,
        ri.ri_proc_start_abstime,
    ))
}

/// pid's children, into out.
#[cfg(target_os = "macos")]
fn children(pid: i32, out: &mut Vec<i32>) {
    let mut cap = 64;
    loop {
        out.clear();
        out.resize(cap, 0);
        // SAFETY: the buffer holds cap pids.
        let n = unsafe {
            libc::proc_listchildpids(pid, out.as_mut_ptr().cast(), (cap * 4) as libc::c_int)
        };
        if n <= 0 {
            out.clear();
            return;
        }
        let n = n as usize;
        if n < cap || cap >= 1 << 16 {
            out.truncate(n.min(cap));
            out.retain(|&p| p > 0);
            return;
        }
        cap *= 4;
    }
}

/// Elsewhere: from /proc, the resident set for memory.
#[cfg(not(target_os = "macos"))]
fn usage(pid: i32) -> Option<(u64, u64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Past the name in brackets: state ppid … utime(14) stime(15) … starttime(22) vsize rss(24).
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split(' ').collect();
    let num = |i: usize| {
        f.get(i - 3)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    // SAFETY: as above.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
    Some((
        num(24) * page,
        (num(14) + num(15)) * 1_000_000_000 / hz,
        num(22),
    ))
}

#[cfg(not(target_os = "macos"))]
fn children(pid: i32, out: &mut Vec<i32>) {
    out.clear();
    let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return;
    };
    for t in rd.flatten() {
        if let Ok(s) = std::fs::read_to_string(t.path().join("children")) {
            out.extend(s.split_whitespace().filter_map(|p| p.parse::<i32>().ok()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_mode() {
        assert_eq!(is_print(&["claude", "-p", "hi"]), (true, false));
        assert_eq!(
            is_print(&["claude", "--output-format=json", "--no-session-persistence"]),
            (true, true)
        );
        assert_eq!(is_print(&["claude"]), (false, false));
    }

    #[test]
    fn a_tree_is_summed() {
        let me = std::process::id() as i32;
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let mut s = Sampler::new();
        let u = s.sample(&[me])[&me];
        assert!(u.procs >= 2, "{u:?}");
        assert!(u.mem > 0);
        // The next sample has a baseline; a gone pid is no tree.
        let u = s.sample(&[me, 999_999])[&me];
        assert!(u.cpu >= 0.0);
        assert_eq!(s.sample(&[999_999])[&999_999], Usage::default());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn own_args() {
        let a = args(std::process::id() as i32);
        assert!(!a.is_empty(), "no args for this process");
    }
}
