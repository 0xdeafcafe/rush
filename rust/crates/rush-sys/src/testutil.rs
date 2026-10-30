//! Waiting on processes in tests without sleeping: the kernel says when one
//! exits.

use std::time::Duration;

/// Blocks until our child pid has exited, leaving it unreaped (a zombie).
pub fn wait_exited_unreaped(pid: i32) {
    // SAFETY: siginfo_t is plain data; waitid fills it.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: waitid on our own child, not reaping it.
    let r = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOWAIT,
        )
    };
    assert_eq!(r, 0, "waitid: {}", std::io::Error::last_os_error());
}

/// Blocks until our child pid has stopped (WUNTRACED) or, with cont, gone
/// on again (WCONTINUED).
pub fn wait_stopped(pid: i32, cont: bool) {
    let mut status = 0;
    let flag = if cont {
        libc::WCONTINUED
    } else {
        libc::WUNTRACED
    };
    // SAFETY: waitpid on our own child; a stop or continue doesn't reap it.
    let r = unsafe { libc::waitpid(pid, &mut status, flag) };
    assert_eq!(r, pid, "waitpid: {}", std::io::Error::last_os_error());
}

/// Whether pid exits (or is gone already) within d: any process, not only
/// our children. kqueue's EVFILT_PROC says so the moment it does.
#[cfg(target_os = "macos")]
pub fn gone_within(pid: i32, d: Duration) -> bool {
    // SAFETY: plain kqueue calls on a kqueue we own and close.
    unsafe {
        let kq = libc::kqueue();
        assert!(kq >= 0);
        let mut ev: libc::kevent = std::mem::zeroed();
        ev.ident = pid as usize;
        ev.filter = libc::EVFILT_PROC;
        ev.flags = libc::EV_ADD | libc::EV_ONESHOT;
        ev.fflags = libc::NOTE_EXIT;
        if libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) < 0 {
            libc::close(kq);
            return true; // ESRCH: not there
        }
        // Exited, but still a zombie of a parent that hasn't reaped it.
        if crate::proc::zombie(pid) {
            libc::close(kq);
            return true;
        }
        let ts = libc::timespec {
            tv_sec: d.as_secs() as libc::time_t,
            tv_nsec: d.subsec_nanos() as libc::c_long,
        };
        let mut out: libc::kevent = std::mem::zeroed();
        let n = libc::kevent(kq, std::ptr::null(), 0, &mut out, 1, &ts);
        libc::close(kq);
        n > 0
    }
}

/// Whether pid answers kill(pid, 0) and isn't a zombie.
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks.
    unsafe { libc::kill(pid, 0) == 0 && !crate::proc::zombie(pid) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gone_within_waits_for_exit() {
        let me = std::process::id() as i32;
        assert!(
            !gone_within(me, Duration::from_millis(20)),
            "this process read as gone"
        );
        let mut c = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = c.id() as i32;
        assert!(alive(pid));
        assert!(!gone_within(pid, Duration::from_millis(20)));
        c.kill().unwrap();
        assert!(gone_within(pid, Duration::from_secs(3)));
        c.wait().unwrap();
        assert!(gone_within(pid, Duration::from_millis(1)));
    }
}
