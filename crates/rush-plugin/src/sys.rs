//! The little rush-plugin asks of the system: waiting on a file
//! descriptor that another thread can wake (so a listener or a pipe can be
//! closed under a blocked reader, as Go's poller allows), file locks, and
//! running a program with a time limit. Every wait blocks in poll(2) or on
//! a channel; nothing looks in a loop.

use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

/// A pipe whose read end a waiter polls beside what it waits on: writing
/// to it wakes them all, for good.
pub(crate) struct Waker {
    r: OwnedFd,
    w: OwnedFd,
}

impl Waker {
    pub fn new() -> io::Result<Arc<Waker>> {
        let mut fds = [0 as RawFd; 2];
        // SAFETY: fds is two ints for pipe to fill.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: pipe gave us these two descriptors to own.
        let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        for fd in [&r, &w] {
            // SAFETY: plain fcntl on descriptors we own.
            unsafe {
                libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
                let fl = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
                libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, fl | libc::O_NONBLOCK);
            }
        }
        Ok(Arc::new(Waker { r, w }))
    }

    /// Wakes every waiter, now and later.
    pub fn wake(&self) {
        // SAFETY: writes one byte from a valid buffer; a full pipe is
        // already awake.
        unsafe {
            libc::write(self.w.as_raw_fd(), b"x".as_ptr().cast(), 1);
        }
    }

    pub fn woken(&self) -> bool {
        matches!(
            poll2(self.r.as_raw_fd(), None, Some(Duration::ZERO)),
            Ok(Ready::Fd)
        )
    }
}

/// What a wait ended on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ready {
    Fd,
    Woken,
    Timeout,
}

/// Waits until fd can be read, the waker wakes, or the timeout passes.
pub(crate) fn wait_readable(
    fd: RawFd,
    waker: &Waker,
    timeout: Option<Duration>,
) -> io::Result<Ready> {
    match poll2(fd, Some(waker.r.as_raw_fd()), timeout)? {
        Ready::Fd if waker.woken() => Ok(Ready::Woken),
        r => Ok(r),
    }
}

fn poll2(fd: RawFd, wake: Option<RawFd>, timeout: Option<Duration>) -> io::Result<Ready> {
    let mut fds = [
        libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: wake.unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let n = if wake.is_some() { 2 } else { 1 };
    let ms = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as i32);
    loop {
        // SAFETY: fds holds n valid pollfd structs.
        let r = unsafe { libc::poll(fds.as_mut_ptr(), n, ms) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if r == 0 {
            return Ok(Ready::Timeout);
        }
        if n == 2 && fds[1].revents != 0 {
            return Ok(Ready::Woken);
        }
        return Ok(Ready::Fd);
    }
}

/// A reader of a pipe or socket that a Waker can end while it's blocked:
/// it then reads as closed.
pub struct WakeableReader {
    fd: OwnedFd,
    waker: Arc<Waker>,
}

/// Ends a WakeableReader's reads, from any thread.
#[derive(Clone)]
pub struct ReadCloser(Arc<Waker>);

impl ReadCloser {
    pub fn close(&self) {
        self.0.wake();
    }
}

/// A reader of fd that the returned closer can end mid-read.
pub fn wakeable(fd: impl Into<OwnedFd>) -> io::Result<(WakeableReader, ReadCloser)> {
    let waker = Waker::new()?;
    Ok((
        WakeableReader {
            fd: fd.into(),
            waker: waker.clone(),
        },
        ReadCloser(waker),
    ))
}

impl Read for WakeableReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if wait_readable(self.fd.as_raw_fd(), &self.waker, None)? == Ready::Woken {
                return Err(io::Error::other("file already closed"));
            }
            // SAFETY: reads into buf, at most its length.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                _ => return Err(e),
            }
        }
    }
}

/// Takes an exclusive flock on the file, blocking until it's free.
pub(crate) fn flock_ex(f: &std::fs::File) -> io::Result<()> {
    loop {
        // SAFETY: flock on a descriptor we hold open.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Runs cmd for its output, killing it past the limit, as Go's
/// exec.CommandContext with a timeout does. None when it couldn't start.
pub(crate) fn output_within(mut cmd: Command, limit: Duration) -> Option<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = cmd.spawn().ok()?;
    drop(cmd);
    let pid = child.id() as libc::pid_t;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(limit) {
        Ok(out) => out.ok(),
        Err(_) => {
            kill(pid);
            rx.recv().ok().and_then(Result::ok)
        }
    }
}

/// Runs cmd with its stdout and stderr together, as Go's CombinedOutput,
/// killing it past the limit. Err when it couldn't start.
pub(crate) fn combined_within(
    mut cmd: Command,
    limit: Duration,
) -> io::Result<(ExitStatus, Vec<u8>)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: fds is two ints for pipe to fill.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe gave us these two descriptors to own.
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    // SAFETY: plain fcntl on the read end we own, kept from the child.
    unsafe {
        libc::fcntl(r.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let w2 = w.try_clone()?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(w))
        .stderr(Stdio::from(w2));
    let spawned = cmd.spawn();
    drop(cmd); // its copies of the write end, so the read sees the end
    let mut child = spawned?;
    let pid = child.id() as libc::pid_t;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = std::fs::File::from(r).read_to_end(&mut out);
        out
    });
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    let status = match rx.recv_timeout(limit) {
        Ok(st) => st?,
        Err(_) => {
            kill(pid);
            rx.recv().map_err(io::Error::other)??
        }
    };
    let out = reader.join().unwrap_or_default();
    Ok((status, out))
}

fn kill(pid: libc::pid_t) {
    // SAFETY: kill the child we started, not yet waited for.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_read_ends_when_closed() {
        let (r, _w) = std::os::unix::net::UnixStream::pair().unwrap();
        let (mut reader, closer) = wakeable(r).unwrap();
        let t = std::thread::spawn(move || reader.read(&mut [0u8; 8]));
        closer.close();
        assert!(t.join().unwrap().is_err());
    }

    #[test]
    fn a_slow_program_is_killed() {
        let mut c = Command::new("/bin/sleep");
        c.arg("30");
        let out = output_within(c, Duration::from_millis(50)).unwrap();
        assert!(!out.status.success());
    }
}
