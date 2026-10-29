//! Stopping on ^C or SIGTERM, as Go's signal.NotifyContext does for rush
//! watch and rush pr: the first one stops what waits, which then ends the
//! way Go's cancelled context does (a watch with exit 3, "stopped").
//! Waiting is a poll() on a pipe the handler writes to: idle costs
//! nothing, and a signal ends the wait at once.
//! Ported from cmd/rush/watch.go:75 and pr.go:43 (signal.NotifyContext)
//! and the Sleep defaults in internal/watch/pr.go:320 and
//! internal/prdrive/follow.go:347.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

static STOPPED: AtomicBool = AtomicBool::new(false);
static WAKE: AtomicI32 = AtomicI32::new(-1); // the pipe's write end
static WAIT: AtomicI32 = AtomicI32::new(-1); // its read end

extern "C" fn on_signal(_: libc::c_int) {
    STOPPED.store(true, Ordering::SeqCst);
    let fd = WAKE.load(Ordering::SeqCst);
    if fd >= 0 {
        // SAFETY: write is async-signal-safe; the byte lives in static memory.
        unsafe {
            libc::write(fd, b"x".as_ptr().cast(), 1);
        }
    }
}

/// Has ^C and SIGTERM stop this process's waits rather than end it. Once
/// is enough; later calls do nothing.
pub fn on_signals() {
    if WAIT.load(Ordering::SeqCst) >= 0 {
        return;
    }
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe writes two descriptors into fds.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    for fd in fds {
        // SAFETY: fd is ours, just made.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
        }
    }
    WAKE.store(fds[1], Ordering::SeqCst);
    WAIT.store(fds[0], Ordering::SeqCst);
    // SAFETY: on_signal only touches atomics and calls write.
    unsafe {
        let h = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, h);
        libc::signal(libc::SIGTERM, h);
    }
}

/// Whether a signal has stopped this process: Go's ctx.Err() != nil.
pub fn stopped() -> bool {
    STOPPED.load(Ordering::SeqCst)
}

/// Waits d, or until a signal stops it; false when it was stopped.
pub fn sleep(d: Duration) -> bool {
    if stopped() {
        return false;
    }
    let fd = WAIT.load(Ordering::SeqCst);
    if fd < 0 {
        std::thread::sleep(d);
        return !stopped();
    }
    let end = Instant::now() + d;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return !stopped();
        }
        let mut p = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = left.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
        // SAFETY: p is one pollfd on this stack.
        unsafe { libc::poll(&mut p, 1, ms) };
        if stopped() {
            return false;
        }
    }
}
