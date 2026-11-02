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
            pages = pages.saturating_add(v.strip_suffix('.').unwrap_or(v).parse::<u64>().ok()?);
        }
    }
    Some(pages.saturating_mul(page))
}

/// Sets errno to 0, so a call that can return -1 for real can be told
/// from one that failed.
fn clear_errno() {
    // SAFETY: errno is this thread's own.
    unsafe {
        *libc::__error() = 0;
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
        assert!(total > 0 && avail <= total, "{avail} of {total}");
        assert!((-1..=100).contains(&Real.memory_level()));
    }
}
