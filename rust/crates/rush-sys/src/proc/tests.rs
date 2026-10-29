//! Ported from internal/proc/zombie_test.go and proc_linux_test.go, plus
//! the tree arithmetic proc.go does.

use super::*;
use crate::testutil::wait_exited_unreaped;
use std::os::unix::process::CommandExt;
use std::process::Command;

#[test]
fn zombie_is_exited_unreaped() {
    let mut c = Command::new("true").spawn().unwrap();
    let pid = c.id() as Pid;
    wait_exited_unreaped(pid);
    assert!(
        zombie(pid),
        "an exited, unreaped child never read as a zombie"
    );
    assert!(
        !zombie(std::process::id() as Pid),
        "this running process read as a zombie"
    );
    c.wait().unwrap();
}

#[test]
fn parse_stat_odd_comm() {
    // A comm holding a space and parens, as a real one can.
    let line = b"1234 (some (odd) prog) S 1 1234 1234 0 -1 4194304 100 0 0 0 55 20 0 0 20 0 4 0 987654 0 0 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0";
    let s = parse_stat(line).expect("parse_stat failed");
    assert_eq!(s.comm, "some (odd) prog");
    assert_eq!(s.ppid, 1);
    assert_eq!(s.cpu, Duration::from_millis(750));
    assert_eq!(s.start_ticks, 987654);
}

#[test]
fn parse_stat_huge_times_saturate() {
    // Nonsense times read as the longest there is, not an overflow.
    let max = u64::MAX;
    let line = format!("1 (x) S 1 1 1 0 -1 0 0 0 0 0 {max} {max} 0 0 20 0 1 0 {max} 0");
    let s = parse_stat(line.as_bytes()).expect("parse_stat failed");
    assert_eq!(s.cpu, Duration::from_nanos(u64::MAX));
    assert_eq!(s.start_ticks, max);
}

#[test]
fn parse_stat_short() {
    for bad in [&b"1 (x) S 0"[..], b"no parens here", b"1 )x( S"] {
        assert_eq!(parse_stat(bad), None, "{:?}", String::from_utf8_lossy(bad));
    }
}

// list, args and fill read this test's own process.
#[test]
fn list_and_args_self() {
    let pid = std::process::id() as Pid;
    let mut t = snapshot();
    let me = t
        .procs
        .get(&pid)
        .expect("snapshot didn't find this process");
    assert!(!me.comm.is_empty() && me.start != UNIX_EPOCH, "{me:?}");
    assert_eq!(started(pid), Some(me.start));
    assert!(t.children[&me.ppid].contains(&pid));
    t.fill(None, &[pid]);
    let me = &t.procs[&pid];
    assert!(me.footprint > 0, "fill left footprint 0: {me:?}");
    assert!(me.cpu_time > Duration::ZERO, "{me:?}");
    assert!(!args(pid).is_empty(), "args(self) returned nothing");
    assert!(!command_line(pid).is_empty());
    assert!(
        env(pid).iter().any(|e| e.contains('=')),
        "env(self): {:?}",
        env(pid)
    );
    assert_eq!(started(999_999), None);
}

#[cfg(target_os = "macos")]
#[test]
fn procargs2() {
    let mut b = 2u32.to_le_bytes().to_vec();
    b.extend_from_slice(b"/bin/x\0\0\0\0x\0-y\0A=1\0B=2\0\0junk\0");
    let (a, e) = darwin::parse_procargs2(&b);
    assert_eq!(a, ["x", "-y"]);
    assert_eq!(e, ["A=1", "B=2"]);
    let mut b = 3u32.to_le_bytes().to_vec();
    b.extend_from_slice(b"/bin/x\0x\0y");
    assert_eq!(
        darwin::parse_procargs2(&b),
        (vec!["x".into(), "y".into()], vec![])
    );
    assert_eq!(darwin::parse_procargs2(b"ab"), (vec![], vec![]));
}

/// A made-up table: pid → (ppid, footprint, cpu).
fn table(rows: &[(Pid, Pid, u64, f64)]) -> Table {
    let mut t = Table {
        at: Instant::now(),
        procs: PidMap::default(),
        children: PidMap::default(),
    };
    for &(pid, ppid, footprint, cpu) in rows {
        t.children.entry(ppid).or_default().push(pid);
        t.procs.insert(
            pid,
            Proc {
                pid,
                ppid,
                footprint,
                cpu,
                ..Proc::default()
            },
        );
    }
    t
}

#[test]
fn tree_sum_descendants() {
    //  10 ─┬─ 12 ── 13
    //      └─ 11
    //  20
    let t = table(&[
        (10, 1, 100, 1.0),
        (12, 10, 20, 2.0),
        (11, 10, 30, 0.5),
        (13, 12, 5, 4.0),
        (20, 1, 7, 0.0),
    ]);
    let tree: Vec<(Pid, usize)> = t.tree(10).iter().map(|n| (n.pid, n.depth)).collect();
    assert_eq!(tree, [(10, 0), (11, 1), (12, 1), (13, 2)]);
    assert!(t.tree(99).is_empty());
    assert_eq!(t.descendants(10), [13, 11, 12, 10]);

    for (stop, want) in [
        (vec![], (155, 7.5, 4)),
        (vec![12], (130, 1.5, 2)),
        (vec![11, 13], (120, 3.0, 2)),
    ] {
        let stop: HashSet<Pid> = stop.into_iter().collect();
        assert_eq!(t.sum(10, &stop), want, "stop {stop:?}");
    }
    assert_eq!(t.sum(99, &HashSet::new()), (0, 0.0, 0));
}

#[test]
fn fill_cpu_from_previous_sample() {
    // Pids no machine has, so fill reads nothing and keeps the given times.
    let pid = 999_990;
    let mut prev = table(&[(pid, 1, 0, 0.0), (pid + 1, pid, 0, 0.0)]);
    let mut cur = prev.clone();
    cur.at = prev.at + Duration::from_secs(2);
    for (t, secs) in [(&mut prev, 1), (&mut cur, 2)] {
        for p in t.procs.values_mut() {
            p.cpu_time = Duration::from_secs(secs);
        }
    }
    // Restarted under the same pid: no baseline.
    cur.procs.get_mut(&(pid + 1)).unwrap().start = UNIX_EPOCH + Duration::from_secs(5);
    cur.fill(Some(&prev), &[pid]);
    assert_eq!(cur.procs[&pid].cpu, 50.0);
    assert_eq!(cur.procs[&(pid + 1)].cpu, 0.0);
}

#[test]
fn protected_processes_refuse_signals() {
    let mut c = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = c.id() as Pid;
    protect(&[pid]);
    assert_eq!(kill(pid, libc::SIGTERM), Err(KillError::Protected));
    assert_eq!(kill(pid, 0), Ok(()), "asking whether it runs harms nothing");
    protect(&[]);
    assert_eq!(kill(pid, libc::SIGKILL), Ok(()));
    c.wait().unwrap();
    assert_eq!(kill(pid, 0), Err(KillError::Os(libc::ESRCH)));
    assert!(
        KillError::Protected
            .to_string()
            .starts_with("that process stands in")
    );
}

// init, every process (-1), this group (0) and group 1 are never one
// process rush chose; in a test, neither is a process the test didn't
// start (its parent, here), while its own child is.
#[test]
fn signals_only_reach_what_rush_may_end() {
    for pid in [1, 0, -1, -42] {
        assert_eq!(
            kill(pid, libc::SIGTERM),
            Err(KillError::Refused(pid)),
            "{pid}"
        );
    }
    assert_eq!(kill_group(1, libc::SIGTERM), Err(KillError::Refused(-1)));
    assert_eq!(kill_group(0, libc::SIGTERM), Err(KillError::Refused(0)));
    assert!(
        rush_core::sandbox::active(),
        "tests run in rush-testenv's home"
    );
    // SAFETY: getppid takes nothing.
    let parent = unsafe { libc::getppid() };
    assert_eq!(kill(parent, 0), Ok(()), "asking is always allowed");
    assert_eq!(kill(parent, libc::SIGCONT), Err(KillError::Refused(parent)));
    let mut c = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = c.id() as Pid;
    assert_eq!(kill_group(pid, libc::SIGKILL), Ok(()));
    c.wait().unwrap();
    assert!(
        KillError::Refused(-1)
            .to_string()
            .starts_with("refused to signal -1")
    );
}

#[test]
fn a_process_its_own_parent_ends_the_walk() {
    // macOS's kernel_task: pid 0, parent 0.
    let t = table(&[(0, 0, 10, 1.0), (1, 0, 20, 2.0), (5, 1, 30, 0.5)]);
    let tree: Vec<(Pid, usize)> = t.tree(0).iter().map(|n| (n.pid, n.depth)).collect();
    assert_eq!(tree, [(0, 0), (1, 1), (5, 2)]);
    assert_eq!(t.sum(0, &HashSet::new()), (60, 3.5, 3));
    assert_eq!(t.descendants(0), [5, 1, 0]);
    // And a real sample never lists one as its own child.
    let real = snapshot();
    for (ppid, kids) in &real.children {
        assert!(!kids.contains(ppid), "{ppid} is its own child");
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn comm_names_the_program() {
    let mut c = Command::new("sleep").arg("30").spawn().unwrap();
    assert_eq!(comm(c.id() as Pid).as_deref(), Some("sleep"));
    c.kill().unwrap();
    c.wait().unwrap();
    assert_eq!(comm(c.id() as Pid), None);
    assert_eq!(comm(0), None);
}
