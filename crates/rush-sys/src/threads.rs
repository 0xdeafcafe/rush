//! The threads rush starts: how many this process has, and helpers to start
//! one with a stack the size of what it does (Rust-only).
//!
//! A thread std starts gets a 2 MB stack (the main thread 8 MB). Most of
//! rush's helpers block in one wait (a read, a kevent, a condvar) and call
//! little: [`SMALL`] is plenty for them. The stack is only reserved, not
//! touched, so the saving is in address space and in the pages a deep
//! call once dirtied, not in a fresh thread's footprint; what costs is the
//! thread itself (its kernel side, its wake-ups), so the first aim is
//! fewer of them.

use std::io;
use std::thread::JoinHandle;

/// For a helper that waits on one thing and hands what it gets on: a
/// reader of a pipe or socket, a waiter on a child, a timer. Big enough for
/// serde_json on a message and a log line.
pub const SMALL: usize = 256 << 10;

/// For a worker that parses, formats or calls into other crates' deep
/// code (a JSON-RPC handler, a plugin's main).
pub const MEDIUM: usize = 1 << 20;

/// Starts f on a thread called name with a stack of stack bytes.
pub fn spawn_sized<T: Send + 'static>(
    name: &str,
    stack: usize,
    f: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(stack)
        .spawn(f)
}

/// Starts f on a thread called name with a [`SMALL`] stack.
pub fn spawn_small<T: Send + 'static>(
    name: &str,
    f: impl FnOnce() -> T + Send + 'static,
) -> io::Result<JoinHandle<T>> {
    spawn_sized(name, SMALL, f)
}

/// How many threads this process runs now; 0 when it can't be told.
pub fn count() -> usize {
    count_of(std::process::id() as i32)
}

/// How many threads process pid runs now; 0 when it can't be told.
pub fn count_of(pid: i32) -> usize {
    // SAFETY: zeroed is a valid proc_taskinfo; proc_pidinfo fills it.
    let mut ti: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: an out-buffer of the size given.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            (&mut ti as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    if n == size {
        ti.pti_threadnum as usize
    } else {
        0
    }
}

/// proc_pidinfo's list of a process's threads (sys/proc_info.h), which
/// libc doesn't name.
const PROC_PIDLISTTHREADS: libc::c_int = 6;

/// The names of this process's threads now, an unnamed one as ""; empty
/// when they can't be told. For a test to say which it didn't expect.
pub fn names() -> Vec<String> {
    let pid = std::process::id() as i32;
    let mut ids = vec![0u64; count() + 16];
    // SAFETY: an out-buffer of the size given.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDLISTTHREADS,
            0,
            ids.as_mut_ptr().cast(),
            (ids.len() * 8) as libc::c_int,
        )
    };
    if n <= 0 {
        return Vec::new();
    }
    ids.truncate(n as usize / 8);
    let size = std::mem::size_of::<libc::proc_threadinfo>() as libc::c_int;
    ids.iter()
        .filter_map(|&t| {
            // SAFETY: zeroed is a valid proc_threadinfo; proc_pidinfo fills it.
            let mut ti: libc::proc_threadinfo = unsafe { std::mem::zeroed() };
            // SAFETY: an out-buffer of the size given.
            let r = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTHREADINFO,
                    t,
                    (&mut ti as *mut libc::proc_threadinfo).cast(),
                    size,
                )
            };
            // SAFETY: pth_name is NUL-terminated within its array.
            (r == size).then(|| {
                unsafe { std::ffi::CStr::from_ptr(ti.pth_name.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn counts_a_thread_started_and_ended() {
        // Other tests start threads of their own meanwhile, so only that a
        // count is had, and that a small stack runs what a helper does.
        assert!(count() >= 1);
        let (tx, rx) = mpsc::channel::<()>();
        let t = spawn_small("rush-test", move || {
            let v: serde_json::Value = serde_json::from_str(r#"{"a":[1,2,{"b":"c"}]}"#).unwrap();
            let _ = rx.recv();
            v.to_string()
        })
        .unwrap();
        assert!(count() >= 2);
        assert!(names().iter().any(|n| n == "rush-test"), "{:?}", names());
        drop(tx);
        assert_eq!(t.join().unwrap(), r#"{"a":[1,2,{"b":"c"}]}"#);
        assert_eq!(count_of(-1), 0);
    }
}
