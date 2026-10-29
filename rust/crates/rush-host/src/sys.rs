//! Waiting on processes without polling them: kqueue's NOTE_EXIT on macOS,
//! a look every 50ms elsewhere (as Go looks everywhere).

use std::time::{Duration, Instant};

/// Waits until pid has exited, or timeout has passed; whether it exited.
pub fn wait_exit(pid: i64, timeout: Duration) -> bool {
    if !crate::alive(pid) {
        return true;
    }
    #[cfg(target_os = "macos")]
    if let Some(gone) = kq_wait(pid as i32, Some(timeout)) {
        return gone || !crate::alive(pid);
    }
    let end = Instant::now() + timeout;
    while crate::alive(pid) {
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Waits until pid exits, for as long as it takes.
pub fn wait_exit_forever(pid: i64) {
    #[cfg(target_os = "macos")]
    if kq_wait(pid as i32, None).is_some() {
        return;
    }
    while crate::alive(pid) {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Waits on kqueue for pid's exit: Some(true) when it did, Some(false) at
/// the timeout, None when it can't wait so (no kqueue, or no such pid).
#[cfg(target_os = "macos")]
fn kq_wait(pid: i32, timeout: Option<Duration>) -> Option<bool> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: kqueue takes nothing; a non-negative result is ours.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return None;
    }
    // SAFETY: kq is a descriptor we just made.
    let kq = unsafe { OwnedFd::from_raw_fd(kq) };
    // SAFETY: kevent is plain data.
    let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
    ev.ident = pid as usize;
    ev.filter = libc::EVFILT_PROC;
    ev.flags = libc::EV_ADD | libc::EV_ONESHOT;
    ev.fflags = libc::NOTE_EXIT;
    // SAFETY: one valid change, no events read.
    let r = unsafe { libc::kevent(kq.as_raw_fd(), &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    if r < 0 {
        return None; // gone already, or not ours to watch
    }
    // Watched now, so it can't exit unseen; but it may already be a zombie.
    if !crate::alive(pid as i64) {
        return Some(true);
    }
    let ts = timeout.map(|d| libc::timespec {
        tv_sec: d.as_secs() as libc::time_t,
        tv_nsec: d.subsec_nanos() as libc::c_long,
    });
    let tp = ts.as_ref().map_or(std::ptr::null(), |t| t as *const libc::timespec);
    loop {
        // SAFETY: kevent is plain data.
        let mut out: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: room for one event; tp is null or a valid timespec.
        let n = unsafe { libc::kevent(kq.as_raw_fd(), std::ptr::null(), 0, &mut out, 1, tp) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Some(n > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_a_process_to_exit() {
        let mut c = std::process::Command::new("/bin/sh")
            .args(["-c", "read x"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = c.id() as i64;
        assert!(!wait_exit(pid, Duration::from_millis(10)));
        drop(c.stdin.take()); // its read ends: it exits
        let t = std::thread::spawn(move || {
            let _ = c.wait();
        });
        assert!(wait_exit(pid, Duration::from_secs(10)));
        t.join().unwrap();
    }
}
