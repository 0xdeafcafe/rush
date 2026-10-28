//! Waiting for a process that isn't ours to go: the node process a gate
//! holder holds a run for. Go looks every 250ms (cmd/rush/gate.go:416);
//! here kqueue says the moment it exits, and elsewhere it's still a look
//! every 250ms.

use rush_sys::proc::{self, Pid};

/// Whether pid runs: it answers kill and isn't a zombie.
fn runs(pid: Pid) -> bool {
    proc::kill(pid, 0).is_ok() && !proc::zombie(pid)
}

/// Returns once pid has gone.
pub fn wait_gone(pid: Pid) {
    #[cfg(target_os = "macos")]
    if kq_wait(pid) {
        return;
    }
    let c = crate::gate::Cancel::new();
    while runs(pid) {
        c.wait(std::time::Duration::from_millis(250));
    }
}

/// Waits on kqueue for pid's exit; false when it can't, to look instead.
#[cfg(target_os = "macos")]
fn kq_wait(pid: Pid) -> bool {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: kqueue takes nothing; a non-negative result is ours.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return false;
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
    let r = unsafe {
        libc::kevent(
            kq.as_raw_fd(),
            &ev,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    };
    if r < 0 {
        return !runs(pid); // gone already, or not ours to watch
    }
    // Watched now, so it can't exit unseen; it may have before.
    if !runs(pid) {
        return true;
    }
    loop {
        // SAFETY: kevent is plain data.
        let mut out: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: room for one event; no timeout, so it blocks.
        let n = unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                std::ptr::null(),
                0,
                &mut out,
                1,
                std::ptr::null(),
            )
        };
        if n > 0 {
            return true;
        }
        if n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_when_it_goes() {
        let mut c = std::process::Command::new("sh")
            .args(["-c", "read _"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = c.id() as Pid;
        let h = std::thread::spawn(move || wait_gone(pid));
        drop(c.stdin.take()); // it reads the end, and exits
        let _ = c.wait();
        h.join().unwrap();
        wait_gone(pid); // gone already
    }
}
