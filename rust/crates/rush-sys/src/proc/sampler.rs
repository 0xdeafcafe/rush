//! What a few process trees use, sampled without reading the whole
//! process table: each root's tree is walked through its children
//! (proc_listchildpids on macOS), so a sample costs a few calls per tree
//! and keeps only the last CPU times of the processes it saw. The agent
//! list samples each live agent's tree this way every reading.
//!
//! Go samples the whole table (internal/proc/proc.go Snapshot, Fill, Sum;
//! fleet.go:1263 sample); [`super::snapshot`] does that here, for screens
//! that show every process.

use std::collections::HashMap;
use std::time::Instant;

use super::{Pid, os};

/// What one process tree uses.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// Bytes: what Activity Monitor calls Memory.
    pub mem: u64,
    /// Percent of one core since the previous sample.
    pub cpu: f32,
    pub procs: u32,
}

/// One process as sampled: when it started, to tell a reused pid, and the
/// CPU time it had used.
#[derive(Clone, Copy)]
struct Seen {
    start: u64,
    cpu_ns: u64,
}

/// Samples process trees, keeping each process's last CPU time as the
/// baseline for the next sample's percentages.
#[derive(Default)]
pub struct Sampler {
    prev: HashMap<Pid, Seen>,
    at: Option<Instant>,
}

impl Sampler {
    pub fn new() -> Sampler {
        Sampler::default()
    }

    /// Each root's tree, summed: the root and everything under it.
    pub fn sample(&mut self, roots: &[Pid]) -> HashMap<Pid, Usage> {
        let now = Instant::now();
        let dt = self.at.map(|t| now.duration_since(t).as_nanos() as f64);
        let mut seen: HashMap<Pid, Seen> = HashMap::with_capacity(self.prev.len());
        let mut out = HashMap::with_capacity(roots.len());
        let mut stack: Vec<Pid> = Vec::new();
        let mut kids: Vec<Pid> = Vec::new();
        for &root in roots {
            if root <= 0 || out.contains_key(&root) {
                continue;
            }
            let mut u = Usage::default();
            stack.clear();
            stack.push(root);
            // A tree can't loop, but a reused pid could: never walk forever.
            let mut walked = 0;
            while let Some(pid) = stack.pop() {
                walked += 1;
                if walked > 4096 {
                    break;
                }
                let Some((mem, cpu_ns, start)) = os::usage(pid) else {
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
                os::children(pid, &mut kids);
                stack.extend_from_slice(&kids);
            }
            out.insert(root, u);
        }
        self.prev = seen;
        self.at = Some(now);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_tree_is_summed() {
        let me = std::process::id() as Pid;
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
}
