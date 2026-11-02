//! The host's own wait (Rust-only): one kqueue, on the thread that serves
//! the host, holds what Go gives a goroutine each (host.go:698 accept,
//! :661 watchSock, :681 watchOwner): its socket's listener, its folder (it
//! stops once its socket is gone), the process that started it (it stops
//! once that's gone) and its quitting. Nothing in it is a timer: between
//! turns it wakes for nothing, where Go looks every 30 s and every second.
//! The look at its tmp (trim.go) is a job on the host's timers rather than
//! a thread of its own.
//!
//! Where the kqueue can't be made, a thread waits on each as before
//! (serve_on).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::*;

/// A pipe the host's wait reads: written as the host quits.
pub(super) struct Wake {
    r: OwnedFd,
    w: OwnedFd,
}

impl Wake {
    pub(super) fn new() -> Option<Wake> {
        let (r, w) = std::io::pipe().ok()?;
        let (r, w) = (OwnedFd::from(r), OwnedFd::from(w));
        for fd in [r.as_raw_fd(), w.as_raw_fd()] {
            // SAFETY: O_NONBLOCK on our own pipe: a read drains it, a write
            // never waits.
            unsafe {
                let fl = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
            }
        }
        Some(Wake { r, w })
    }

    /// Wakes the wait.
    pub(super) fn ring(&self) {
        // SAFETY: one byte into our own pipe; a full pipe wakes it already.
        unsafe { libc::write(self.w.as_raw_fd(), b"x".as_ptr().cast(), 1) };
    }

    fn drain(&self) {
        let mut b = [0u8; 64];
        // SAFETY: reads into our buffer from our non-blocking pipe.
        while unsafe { libc::read(self.r.as_raw_fd(), b.as_mut_ptr().cast(), b.len()) } > 0 {}
    }
}

fn kevent_of(ident: usize, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    // SAFETY: kevent is plain data.
    let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
    ev.ident = ident;
    ev.filter = filter;
    ev.flags = flags;
    ev.fflags = fflags;
    ev
}

/// Registers one change on kq: whether it took, and the error when not.
fn change(kq: &OwnedFd, ev: libc::kevent) -> std::io::Result<()> {
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
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// What a change to the folder itself (not an entry in it) says: it's
/// gone or moved, so it's watched again by its path.
const FOLDER_GONE: u32 = libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE;

/// Watches dir's entries on kq: its descriptor, which stops the watch as
/// it closes; None when it can't be opened.
fn watch_folder(kq: &OwnedFd, dir: &Path) -> Option<OwnedFd> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: c is a NUL-terminated path.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    // SAFETY: a descriptor we just opened.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let ev = kevent_of(
        fd.as_raw_fd() as usize,
        libc::EVFILT_VNODE,
        libc::EV_ADD | libc::EV_CLEAR,
        libc::NOTE_WRITE | FOLDER_GONE,
    );
    change(kq, ev).ok()?;
    Some(fd)
}

/// Takes the client ln has waiting without waiting for one: a client that
/// dialled and went before it was taken leaves none, and a wait here
/// would wait past the host's quitting. Non-blocking only for the call,
/// as the listener is handed on across an upgrade's exec as it was.
fn accept_now(ln: &UnixListener) -> std::io::Result<(UnixStream, std::os::unix::net::SocketAddr)> {
    ln.set_nonblocking(true)?;
    let c = ln.accept();
    let _ = ln.set_nonblocking(false);
    let (c, a) = c?;
    // What it takes is as accept's always is: blocking.
    c.set_nonblocking(false)?;
    Ok((c, a))
}

impl Host {
    /// Looks every TRIM_EVERY whether to trim the session's tmp, as long
    /// as the host runs.
    pub(super) fn arm_trim(&self) {
        let h = self.clone();
        self.timers
            .after(crate::trim::TRIM_EVERY, move || h.trim_due());
    }

    /// The look: its cheap part on the timers' thread, what's open under
    /// the tmp (which takes a while to tell) and the trim on one of its
    /// own, only when the disk is low.
    fn trim_due(&self) {
        if self.lock().quit {
            return;
        }
        self.arm_trim();
        if !rush_state::Store::load().config.trim_running_tmp {
            return;
        }
        let id = self.lock().cfg.id.clone();
        let tmp = crate::tmp::temp_dir(&id);
        if !crate::trim::disk_low(&tmp) {
            return;
        }
        self.go("trim", move |_| {
            let Some(held) = crate::trim::held_under(&tmp) else {
                return; // without knowing what's open, nothing is safe to remove
            };
            let n = crate::trim::trim_tmp(&tmp, SystemTime::now(), &held);
            if n > 0 {
                log_event(&id, "tmp-trimmed", json!({"removed": n}));
            }
        });
    }

    /// Stops the host for why, on a thread of its own, as a watcher of
    /// Go's does: the wait goes on serving clients meanwhile.
    fn stop_for(&self, why: Value) {
        let st = self.lock();
        if st.quit {
            return;
        }
        self.event(&st, "stopping", why);
        drop(st);
        let h = self.clone();
        self.go("stop", move |_| {
            let _ = h.do_op(Op::named("stop"));
        });
    }

    /// Serves ln's clients and watches sock and owner (when it's given)
    /// in one kqueue on this thread, until the host quits: true then.
    /// False at once when the wait can't be made: the caller's threads
    /// do it. What can't be watched so is watched by a thread as before.
    pub(super) fn serve_kq(&self, ln: &UnixListener, sock: &Path, owner: Option<i64>) -> bool {
        let Some(wake) = &self.wake else {
            return false;
        };
        // SAFETY: kqueue takes nothing; a non-negative result is ours.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return false;
        }
        // SAFETY: as above.
        let kq = unsafe { OwnedFd::from_raw_fd(kq) };
        // SAFETY: kq is ours.
        unsafe { libc::fcntl(kq.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        let listen: RawFd = ln.as_raw_fd();
        let woken: RawFd = wake.r.as_raw_fd();
        for fd in [listen, woken] {
            let ev = kevent_of(fd as usize, libc::EVFILT_READ, libc::EV_ADD, 0);
            if change(&kq, ev).is_err() {
                return false;
            }
        }
        let dir = sock.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut folder = watch_folder(&kq, &dir);
        let mut sock_watched = true;
        if folder.is_none() {
            // Not watched here: by a thread, as before.
            sock_watched = false;
            let h = self.clone();
            let s = sock.to_path_buf();
            self.go("sock", move |_| h.watch_sock(&s));
        }
        let mut owner_gone = false;
        if let Some(pid) = owner {
            let ev = kevent_of(
                pid as usize,
                libc::EVFILT_PROC,
                libc::EV_ADD | libc::EV_ONESHOT,
                libc::NOTE_EXIT,
            );
            match change(&kq, ev) {
                // Watched now, so it can't exit unseen; but it may already
                // be a zombie.
                Ok(()) => owner_gone = !crate::alive(pid),
                Err(e) if e.raw_os_error() == Some(libc::ESRCH) => owner_gone = true,
                Err(_) => {
                    let h = self.clone();
                    self.go("owner", move |_| h.watch_owner(pid));
                }
            }
        }
        let mut accepting = true;
        // SAFETY: kevent is plain data.
        let mut evs: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
        loop {
            if self.lock().quit {
                return true;
            }
            if sock_watched && std::fs::symlink_metadata(sock).is_err() {
                // No client can reach it again, so it would only linger.
                sock_watched = false;
                drop(folder.take());
                self.stop_for(json!({"why": "its socket is gone"}));
            }
            if owner_gone {
                owner_gone = false;
                let owner = owner.unwrap_or_default();
                self.stop_for(
                    json!({"why": "the process that started it is gone", "owner": owner}),
                );
            }
            // SAFETY: room for evs.len() events; no timeout: it waits for
            // one of them.
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
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return false;
            }
            for ev in &evs[..n as usize] {
                match ev.filter {
                    libc::EVFILT_READ if ev.ident == listen as usize => {
                        if !accepting {
                            continue;
                        }
                        let c = accept_now(ln);
                        if self.lock().quit {
                            return true;
                        }
                        match c {
                            Ok((c, _)) => {
                                let h = self.clone();
                                self.go("client", move |_| h.serve(c));
                            }
                            // Dialled and gone before it was taken.
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::ConnectionAborted
                                ) => {}
                            Err(_) => {
                                // As Go's accept loop, it ends on an error.
                                accepting = false;
                                let ev = kevent_of(
                                    listen as usize,
                                    libc::EVFILT_READ,
                                    libc::EV_DELETE,
                                    0,
                                );
                                let _ = change(&kq, ev);
                            }
                        }
                    }
                    libc::EVFILT_READ => wake.drain(),
                    libc::EVFILT_VNODE => {
                        if ev.fflags & FOLDER_GONE != 0 && sock_watched {
                            // The old descriptor's watch ends as it closes.
                            drop(folder.take());
                            folder = watch_folder(&kq, &dir);
                        }
                    }
                    libc::EVFILT_PROC => owner_gone = true,
                    _ => {}
                }
            }
        }
    }
}
