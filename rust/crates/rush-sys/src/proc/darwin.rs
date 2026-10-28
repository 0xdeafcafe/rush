//! The process table on macOS: sysctl kern.proc.all for the list,
//! proc_pid_rusage for memory and CPU time, kern.procargs2 for arguments.
//!
//! Ported from internal/proc/proc_darwin.go:1-194.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Comms, Pid, Proc};

/// `struct kinfo_proc` from <sys/sysctl.h>, read only by the offsets of the
/// fields we use (checked against the headers with offsetof).
#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct KinfoProc([u8; KINFO_SIZE]);

const KINFO_SIZE: usize = 648;
const OFF_STARTTIME: usize = 0; // kp_proc.p_starttime: timeval {i64 sec; i32 usec}
const OFF_STAT: usize = 36; // kp_proc.p_stat
const OFF_PID: usize = 40; // kp_proc.p_pid
const OFF_COMM: usize = 243; // kp_proc.p_comm[17]
const OFF_PPID: usize = 560; // kp_eproc.e_ppid
const SZOMB: u8 = 5;

impl KinfoProc {
    fn i32_at(&self, off: usize) -> i32 {
        i32::from_ne_bytes(self.0[off..off + 4].try_into().unwrap())
    }
    fn pid(&self) -> Pid {
        self.i32_at(OFF_PID)
    }
    fn ppid(&self) -> Pid {
        self.i32_at(OFF_PPID)
    }
    fn stat(&self) -> u8 {
        self.0[OFF_STAT]
    }
    fn comm(&self) -> &[u8] {
        let c = &self.0[OFF_COMM..OFF_COMM + 17];
        let n = c.iter().position(|&b| b == 0).unwrap_or(c.len());
        &c[..n]
    }
    fn start(&self) -> SystemTime {
        let sec = i64::from_ne_bytes(self.0[OFF_STARTTIME..OFF_STARTTIME + 8].try_into().unwrap());
        let usec = self.i32_at(OFF_STARTTIME + 8);
        let ns = sec as i128 * 1_000_000_000 + usec as i128 * 1000;
        if ns >= 0 {
            UNIX_EPOCH + Duration::from_nanos(ns as u64)
        } else {
            UNIX_EPOCH - Duration::from_nanos((-ns) as u64)
        }
    }
}

/// The process table's read buffer, kept between samples: the view samples
/// every second, and the table is some 600 KB. comms keeps each process
/// name once, as most are the same from one sample to the next.
struct Kinfo {
    buf: Vec<KinfoProc>,
    comms: Comms,
}

static KINFO: Mutex<Option<Kinfo>> = Mutex::new(None);

pub(super) fn list() -> Vec<Proc> {
    let mut g = KINFO.lock().unwrap();
    let k = g.get_or_insert_with(|| Kinfo {
        buf: Vec::new(),
        comms: Comms::new(),
    });
    let Some(n) = read_kinfo(&mut k.buf) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(n);
    for kp in &k.buf[..n] {
        out.push(Proc {
            pid: kp.pid(),
            ppid: kp.ppid(),
            comm: k.comms.get(kp.comm()),
            start: kp.start(),
            ..Proc::default()
        });
    }
    out
}

/// sysctl kern.proc.all into the kept buffer, grown when the table has
/// outgrown it; how many entries it read.
fn read_kinfo(buf: &mut Vec<KinfoProc>) -> Option<usize> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL];
    for _ in 0..8 {
        if !buf.is_empty() {
            let mut n = buf.len() * KINFO_SIZE;
            // SAFETY: buf holds n bytes of kinfo_proc.
            let r = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    mib.len() as u32,
                    buf.as_mut_ptr().cast(),
                    &mut n,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if r == 0 {
                return Some(n / KINFO_SIZE);
            }
            if errno() != libc::ENOMEM {
                return None;
            }
        }
        // Too small, or the first time: ask how big, with room to grow.
        let mut n = 0usize;
        // SAFETY: a null buffer asks only for the size.
        let r = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                std::ptr::null_mut(),
                &mut n,
                std::ptr::null_mut(),
                0,
            )
        };
        if r != 0 {
            return None;
        }
        let want = n / KINFO_SIZE;
        *buf = vec![KinfoProc([0; KINFO_SIZE]); want + want / 8 + 16];
    }
    None
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Apple silicon reports rusage times in timebase ticks, Intel in ns.
fn tick_nanos() -> f64 {
    static T: OnceLock<f64> = OnceLock::new();
    *T.get_or_init(|| {
        let mut f: u64 = 0;
        let mut n = std::mem::size_of::<u64>();
        // SAFETY: hw.tbfrequency is a 64-bit integer.
        let r = unsafe {
            libc::sysctlbyname(
                c"hw.tbfrequency".as_ptr(),
                (&mut f as *mut u64).cast(),
                &mut n,
                std::ptr::null_mut(),
                0,
            )
        };
        if r == 0 && f > 0 && f != 1_000_000_000 {
            1e9 / f as f64
        } else {
            1.0
        }
    })
}

pub(super) fn fill_usage(p: &mut Proc) {
    // SAFETY: rusage_info_v2 is plain data; zero is a valid value.
    let mut ri: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: ri is a rusage_info_v2, the flavour asked for.
    let r = unsafe {
        libc::proc_pid_rusage(
            p.pid,
            libc::RUSAGE_INFO_V2,
            (&mut ri as *mut libc::rusage_info_v2).cast(),
        )
    };
    if r != 0 {
        return;
    }
    p.footprint = ri.ri_phys_footprint;
    p.cpu_time =
        Duration::from_nanos(((ri.ri_user_time + ri.ri_system_time) as f64 * tick_nanos()) as u64);
}

pub(super) fn args(pid: Pid) -> Vec<String> {
    args_env(pid).0
}

pub(super) fn env(pid: Pid) -> Vec<String> {
    args_env(pid).1
}

/// A process's argv and, after it, its environment, through
/// kern.procargs2.
fn args_env(pid: Pid) -> (Vec<String>, Vec<String>) {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut n = 0usize;
    // SAFETY: a null buffer asks only for the size.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut n,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 || n == 0 {
        return (Vec::new(), Vec::new());
    }
    let mut b = vec![0u8; n];
    // SAFETY: b holds n bytes.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            b.as_mut_ptr().cast(),
            &mut n,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return (Vec::new(), Vec::new());
    }
    b.truncate(n);
    parse_procargs2(&b)
}

/// Splits kern.procargs2's bytes: argc, the executable's path, padding,
/// argc arguments, then the environment up to an empty string.
pub(crate) fn parse_procargs2(b: &[u8]) -> (Vec<String>, Vec<String>) {
    if b.len() < 4 {
        return (Vec::new(), Vec::new());
    }
    let argc = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
    let mut b = &b[4..];
    if let Some(i) = b.iter().position(|&c| c == 0) {
        b = &b[i..];
    }
    while let [0, rest @ ..] = b {
        b = rest;
    }
    let s = |x: &[u8]| String::from_utf8_lossy(x).into_owned();
    let mut args = Vec::with_capacity(argc.min(256));
    while args.len() < argc && !b.is_empty() {
        let Some(i) = b.iter().position(|&c| c == 0) else {
            args.push(s(b));
            return (args, Vec::new());
        };
        args.push(s(&b[..i]));
        b = &b[i + 1..];
    }
    let mut env = Vec::new();
    while !b.is_empty() && b[0] != 0 {
        let Some(i) = b.iter().position(|&c| c == 0) else {
            break;
        };
        env.push(s(&b[..i]));
        b = &b[i + 1..];
    }
    (args, env)
}

/// One process's kinfo_proc, None when it isn't running.
fn kinfo_of(pid: Pid) -> Option<KinfoProc> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut k = KinfoProc([0; KINFO_SIZE]);
    let mut n = KINFO_SIZE;
    // SAFETY: k holds one kinfo_proc.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            (&mut k as *mut KinfoProc).cast(),
            &mut n,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0 && n == KINFO_SIZE && k.pid() == pid).then_some(k)
}

pub(super) fn zombie(pid: Pid) -> bool {
    kinfo_of(pid).is_some_and(|k| k.stat() == SZOMB)
}

pub(super) fn started(pid: Pid) -> Option<SystemTime> {
    kinfo_of(pid).map(|k| k.start())
}
