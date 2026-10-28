//! What the gate reads of the machine's memory, and lowering a process's
//! priority. Ported from internal/gate/sys_darwin.go:1-89,
//! sys_linux.go:1-63 and sys_other.go; Go's package variables Memory,
//! PressureLevel and MemoryLevel, which tests set, are the [`Machine`]
//! a [`crate::Gate`] is given.

/// What the gate reads of the machine.
pub trait Machine: Send + Sync {
    /// What the machine has: available (free, inactive and purgeable) and
    /// in all, in bytes; 0, 0 when it can't be read.
    fn memory(&self) -> (u64, u64);
    /// The kernel's memory pressure level: 1 normal, 2 warning, 4
    /// critical; 0 when it can't be read.
    fn pressure_level(&self) -> i32;
    /// The share of memory available, 0 to 100; -1 when it can't be read.
    fn memory_level(&self) -> i32;
}

/// This machine, as its kernel says.
#[derive(Clone, Copy, Debug, Default)]
pub struct Real;

impl Machine for Real {
    fn memory(&self) -> (u64, u64) {
        os::memory()
    }
    fn pressure_level(&self) -> i32 {
        os::pressure()
    }
    fn memory_level(&self) -> i32 {
        os::mem_level()
    }
}

/// Lowers pid's priority to nice n, never raising it: one already nicer
/// keeps its own.
pub fn renice(pid: i32, n: i32) {
    os::renice(pid, n)
}

/// The available bytes vm_stat's output says.
pub fn parse_vm_stat(s: &str) -> Option<u64> {
    let mut lines = s.split('\n');
    let first = lines.next().unwrap_or("");
    let mut page = 0u64;
    if let Some(i) = first.find("page size of ")
        && let Some(f) = first[i + "page size of ".len()..].split_whitespace().next()
    {
        page = f.parse().unwrap_or(0);
    }
    if page == 0 {
        return None;
    }
    let mut pages = 0u64;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        if matches!(k, "Pages free" | "Pages inactive" | "Pages purgeable") {
            let v = v.trim();
            pages += v.strip_suffix('.').unwrap_or(v).parse::<u64>().ok()?;
        }
    }
    Some(pages * page)
}

/// Sets errno to 0, so a call that can return -1 for real can be told
/// from one that failed.
fn clear_errno() {
    // SAFETY: errno is this thread's own.
    unsafe {
        #[cfg(target_os = "macos")]
        {
            *libc::__error() = 0;
        }
        #[cfg(target_os = "linux")]
        {
            *libc::__errno_location() = 0;
        }
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// pid's nice, as getpriority(2) says it; None when it can't be read.
#[allow(dead_code)]
fn priority(pid: i32) -> Option<i32> {
    clear_errno();
    // SAFETY: getpriority takes plain integers.
    let p = unsafe { libc::getpriority(libc::PRIO_PROCESS, pid as libc::id_t) };
    if p == -1 && errno() != 0 {
        return None;
    }
    Some(p)
}

#[cfg(target_os = "macos")]
mod os {
    use std::ffi::CString;

    fn sysctl<T: Default + Copy>(name: &str) -> Option<T> {
        let name = CString::new(name).ok()?;
        let mut v = T::default();
        let mut len = std::mem::size_of::<T>();
        // SAFETY: v is a T and len its size; the kernel writes at most len.
        let r = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut v as *mut T).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (r == 0 && len == std::mem::size_of::<T>()).then_some(v)
    }

    /// What the machine has, available and in all, in bytes, as vm_stat
    /// counts it: free, inactive and purgeable pages are available. 0, 0
    /// when it can't be read.
    pub fn memory() -> (u64, u64) {
        let Some(total) = sysctl::<u64>("hw.memsize") else {
            return (0, 0);
        };
        let Ok(out) = std::process::Command::new("/usr/bin/vm_stat")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            return (0, 0);
        };
        if !out.status.success() {
            return (0, 0);
        }
        match super::parse_vm_stat(&String::from_utf8_lossy(&out.stdout)) {
            Some(avail) => (avail, total),
            None => (0, 0),
        }
    }

    /// kern.memorystatus_vm_pressure_level: 1 normal, 2 warning, 4
    /// critical; 0 when it can't be read.
    pub fn pressure() -> i32 {
        sysctl::<u32>("kern.memorystatus_vm_pressure_level").map_or(0, |v| v as i32)
    }

    /// kern.memorystatus_level: the share of memory the kernel counts
    /// available, 0 to 100, as Activity Monitor's pressure graph has it;
    /// -1 when it can't be read. One sysctl, no fork, so it still reads
    /// when the machine is paging hard.
    pub fn mem_level() -> i32 {
        match sysctl::<u32>("kern.memorystatus_level") {
            Some(v) if v <= 100 => v as i32,
            _ => -1,
        }
    }

    pub fn renice(pid: i32, n: i32) {
        if super::priority(pid).is_some_and(|cur| cur >= n) {
            return;
        }
        // SAFETY: setpriority takes plain integers.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, n);
        }
    }
}

#[cfg(target_os = "linux")]
mod os {
    /// What the machine has, available and in all, in bytes, as
    /// /proc/meminfo says; 0, 0 when it can't be read.
    pub fn memory() -> (u64, u64) {
        let Ok(b) = std::fs::read_to_string("/proc/meminfo") else {
            return (0, 0);
        };
        let (mut avail, mut total) = (0, 0);
        for l in b.split('\n') {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 2 {
                continue;
            }
            let n: u64 = f[1].parse().unwrap_or(0);
            match f[0] {
                "MemAvailable:" => avail = n << 10,
                "MemTotal:" => total = n << 10,
                _ => {}
            }
        }
        (avail, total)
    }

    /// MemAvailable as a share of MemTotal, 0 to 100; -1 when it can't be
    /// read.
    pub fn mem_level() -> i32 {
        let (avail, total) = memory();
        if total == 0 {
            return -1;
        }
        (avail * 100 / total) as i32
    }

    /// Unknown here: no run is paused for it.
    pub fn pressure() -> i32 {
        0
    }

    /// Lowers each of pid's threads to nice n, never raising one: Linux
    /// keeps a nice a thread.
    pub fn renice(pid: i32, n: i32) {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            return;
        };
        for t in tasks.flatten() {
            let Ok(tid) = t.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            if super::priority(tid).is_some_and(|cur| cur >= n) {
                continue;
            }
            // SAFETY: setpriority takes plain integers.
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, n);
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod os {
    pub fn memory() -> (u64, u64) {
        (0, 0)
    }
    pub fn pressure() -> i32 {
        0
    }
    pub fn mem_level() -> i32 {
        -1
    }
    pub fn renice(_: i32, _: i32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    // sys_darwin_test.go TestParseVMStat
    #[test]
    fn parses_vm_stat() {
        let out = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
            Pages free:                                   100.\nPages active:                                 999.\n\
            Pages inactive:                                 20.\nPages speculative:                              7.\n\
            Pages purgeable:                                 3.\n";
        assert_eq!(parse_vm_stat(out), Some(123 * 16384));
        assert_eq!(parse_vm_stat("nonsense"), None);
    }

    #[test]
    fn reads_this_machine() {
        let (avail, total) = Real.memory();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(total > 0 && avail <= total, "{avail} of {total}");
            assert!((-1..=100).contains(&Real.memory_level()));
        }
    }
}
