//! Waiting for many children on one thread (Rust-only): each child's exit
//! is an event on one queue that one thread, "rush-reap", waits on,
//! rather than a thread blocked in wait(2) per child: kqueue's
//! EVFILT_PROC/NOTE_EXIT on macOS, an epoll of each child's pidfd on
//! Linux. A quiet reaper wakes for nothing; its thread starts with the
//! first child given it.

use std::collections::HashMap;
use std::io;
use std::os::fd::OwnedFd;
use std::process::{Child, ExitStatus};
use std::sync::{Mutex, OnceLock};

type Then = Box<dyn FnOnce(io::Result<ExitStatus>) + Send>;

/// A child waited for, with what tells its exit when that is a descriptor
/// of its own (Linux's pidfd; closing it takes it off the queue).
struct Waiting {
    child: Child,
    then: Then,
    _fd: Option<OwnedFd>,
}

struct Reaper {
    q: OwnedFd,
    waiting: Mutex<HashMap<u32, Waiting>>,
}

fn reaper() -> Option<&'static Reaper> {
    static R: OnceLock<Option<&'static Reaper>> = OnceLock::new();
    *R.get_or_init(|| {
        let q = sys::queue()?;
        let r: &'static Reaper = Box::leak(Box::new(Reaper {
            q,
            waiting: Mutex::new(HashMap::new()),
        }));
        crate::threads::spawn_small("rush-reap", move || r.run()).ok()?;
        Some(r)
    })
}

impl Reaper {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Waiting>> {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(&self) {
        let mut pids = Vec::with_capacity(16);
        loop {
            pids.clear();
            if let Err(e) = sys::wait(&self.q, &mut pids) {
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            for pid in &pids {
                let got = self.lock().remove(pid);
                if let Some(w) = got {
                    finish(w.child, w.then);
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    pub(super) fn queue() -> Option<OwnedFd> {
        // SAFETY: kqueue takes nothing; a non-negative result is ours.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return None;
        }
        // SAFETY: fd is a kqueue we own from here.
        let kq = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: F_SETFD on a descriptor we own.
        unsafe { libc::fcntl(kq.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        Some(kq)
    }

    /// Blocks until children exit; their pids into out.
    pub(super) fn wait(kq: &OwnedFd, out: &mut Vec<u32>) -> io::Result<()> {
        // SAFETY: kevent is plain data.
        let mut evs = [unsafe { std::mem::zeroed::<libc::kevent>() }; 16];
        // SAFETY: evs is valid for its length; no changes, no timeout.
        let n = unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                std::ptr::null(),
                0,
                evs.as_mut_ptr(),
                evs.len() as libc::c_int,
                std::ptr::null(),
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        out.extend(
            evs[..n as usize]
                .iter()
                .filter(|ev| ev.filter == libc::EVFILT_PROC)
                .map(|ev| ev.ident as u32),
        );
        Ok(())
    }

    /// Watches pid's exit; an error when it can't (ESRCH: gone already).
    pub(super) fn add(kq: &OwnedFd, pid: u32) -> io::Result<Option<OwnedFd>> {
        // SAFETY: kevent is plain data.
        let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
        ev.ident = pid as usize;
        ev.filter = libc::EVFILT_PROC;
        ev.flags = libc::EV_ADD | libc::EV_ONESHOT;
        ev.fflags = libc::NOTE_EXIT;
        // SAFETY: one change, valid for its length; no events read.
        let rc = unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                &ev,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(None)
        }
    }
}

#[cfg(target_os = "linux")]
mod sys {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    pub(super) fn queue() -> Option<OwnedFd> {
        // SAFETY: epoll_create1 takes flags; a non-negative result is ours.
        let fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        // SAFETY: as above.
        (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Blocks until children exit; their pids into out.
    pub(super) fn wait(ep: &OwnedFd, out: &mut Vec<u32>) -> io::Result<()> {
        let mut evs = [libc::epoll_event { events: 0, u64: 0 }; 16];
        // SAFETY: evs is valid for its length; no timeout.
        let n = unsafe {
            libc::epoll_wait(
                ep.as_raw_fd(),
                evs.as_mut_ptr(),
                evs.len() as libc::c_int,
                -1,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        out.extend(evs[..n as usize].iter().map(|ev| ev.u64 as u32));
        Ok(())
    }

    /// pid's pidfd, on the epoll; an error when it can't be (ESRCH: gone
    /// already, or a kernel without pidfds).
    pub(super) fn add(ep: &OwnedFd, pid: u32) -> io::Result<Option<OwnedFd>> {
        let fd = crate::proc::pidfd(pid as i32).ok_or_else(io::Error::last_os_error)?;
        let mut ev = libc::epoll_event {
            events: (libc::EPOLLIN | libc::EPOLLONESHOT) as u32,
            u64: pid as u64,
        };
        // SAFETY: both descriptors are open; ev is one valid event.
        let rc = unsafe {
            libc::epoll_ctl(ep.as_raw_fd(), libc::EPOLL_CTL_ADD, fd.as_raw_fd(), &mut ev)
        };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Some(fd))
        }
    }
}

/// Reaps child (it has exited, or is about to be seen to) and runs then.
fn finish(mut child: Child, then: Then) {
    let st = child.wait();
    // One that panics ends only itself, not the reaper.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || then(st)));
}

/// Runs then with how child ended, once it has, on the reaper's thread:
/// then must be quick (set a flag, cancel, send). Where the kernel can't
/// watch the child, a thread of its own waits for it, as before.
pub fn on_exit(child: Child, then: impl FnOnce(io::Result<ExitStatus>) + Send + 'static) {
    let then: Then = Box::new(then);
    let Some(r) = reaper() else {
        wait_on_thread(child, then);
        return;
    };
    let pid = child.id();
    // Watched under the lock, so the reaper, told at once, finds it there
    // with its descriptor.
    let mut w = r.lock();
    w.insert(
        pid,
        Waiting {
            child,
            then,
            _fd: None,
        },
    );
    let failed = match sys::add(&r.q, pid) {
        Ok(fd) => {
            if let Some(x) = w.get_mut(&pid) {
                x._fd = fd;
            }
            None
        }
        // Gone already (ESRCH: it exited before it could be watched), or
        // not watchable: reaped here, or waited for on a thread.
        Err(_) => w.remove(&pid),
    };
    drop(w);
    let Some(Waiting {
        mut child, then, ..
    }) = failed
    else {
        return;
    };
    match child.try_wait() {
        Ok(Some(st)) => {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || then(Ok(st))));
        }
        _ => wait_on_thread(child, then),
    }
}

fn wait_on_thread(child: Child, then: Then) {
    let spawned = crate::threads::spawn_small("rush-reap-one", move || finish(child, then));
    debug_assert!(spawned.is_ok(), "a thread to wait for a child");
}

/// How many children the reaper waits for now.
pub fn waiting() -> usize {
    reaper().map_or(0, |r| r.lock().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::sync::mpsc;

    #[test]
    fn many_children_one_thread() {
        let (tx, rx) = mpsc::channel();
        // Kept alive by their stdin until we let them go, so all are
        // watched at once.
        for i in 0..8 {
            let child = Command::new("/bin/sh")
                .args(["-c", &format!("read _; exit {i}")])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let tx = tx.clone();
            let mut child = child;
            let stdin = child.stdin.take().unwrap();
            on_exit(child, move |st| tx.send((i, st.unwrap())).unwrap());
            drop(stdin);
        }
        let mut got: Vec<(i32, ExitStatus)> = (0..8).map(|_| rx.recv().unwrap()).collect();
        got.sort_by_key(|g| g.0);
        for (i, st) in got {
            assert_eq!(st.code(), Some(i));
        }
        // A child killed is told as killed; one gone before it's watched
        // still is told.
        let mut c = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        c.kill().unwrap();
        let (tx, rx) = mpsc::channel();
        on_exit(c, move |st| tx.send(st.unwrap()).unwrap());
        assert_eq!(rx.recv().unwrap().signal(), Some(libc::SIGKILL));
        let mut c = Command::new("/usr/bin/true").spawn().unwrap();
        // Waited out first, through std, so the pid is gone when watched.
        let st = c.wait().unwrap();
        let (tx, rx) = mpsc::channel();
        on_exit(c, move |s| tx.send(s.map(|s| s.code())).unwrap());
        assert_eq!(rx.recv().unwrap().unwrap(), st.code());
    }
}
