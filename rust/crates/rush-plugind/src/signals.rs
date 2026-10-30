//! The signals `rush plugind` takes (Go's signal.Notify, plugind.go:83-93):
//! SIGHUP to reload, SIGTERM and SIGINT to stop. Each is written to a pipe
//! by its handler and read off it by a thread blocked on the pipe.

use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

type Handler = Box<dyn Fn(libc::c_int) + Send>;

/// Who hears the signals now: the broker running, if one is.
static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);

/// The pipe's write end, for the handler.
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = sig as u8;
        // SAFETY: write is async-signal-safe; one byte from the stack.
        unsafe {
            libc::write(fd, (&b as *const u8).cast(), 1);
        }
    }
}

/// Catches SIGHUP, SIGTERM and SIGINT from now on, calling f with each on
/// a thread of its own, until forget. A later call's f takes its place.
pub(crate) fn notify(scope: &crate::Scope, f: impl Fn(libc::c_int) + Send + 'static) {
    *HANDLER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    if WRITE_FD.load(Ordering::SeqCst) >= 0 {
        return;
    }
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe fills fds, a two-element array of our own.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    for fd in fds {
        // SAFETY: fcntl on descriptors we just made.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    if WRITE_FD
        .compare_exchange(-1, fds[1], Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // SAFETY: closing the pipe we made and won't use.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        return;
    }
    for sig in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
        // SAFETY: on_signal only writes a byte to a pipe.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    let r = fds[0];
    scope.spawn("plugind-signals", move || {
        loop {
            let mut b = 0u8;
            // SAFETY: reads one byte into b.
            let n = unsafe { libc::read(r, (&mut b as *mut u8).cast(), 1) };
            if n == 1 {
                if let Some(f) = HANDLER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    f(b as libc::c_int);
                }
            } else if n == 0
                || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                return;
            }
        }
    });
}

/// Stops passing signals on; they're still caught, so one now does nothing.
pub(crate) fn forget() {
    *HANDLER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}
