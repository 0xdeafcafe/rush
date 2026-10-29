//! Following a history as it grows (rush_core::adapter::Follow), in the
//! one order that misses nothing: watch what it names, read until nothing
//! more is waiting, then wait on the watch. Written once here so no caller
//! gets the order wrong (DESIGN.md "Reading history").

use std::time::Duration;

use rush_core::adapter::{Follow, Result, Slice};

use super::{Stop, Watcher};

/// Follows f until stop closes or `each` returns false: each slice with
/// something in it (events, or a reset) goes to `each`, a read that failed
/// too (a transcript not there yet, say), and then it waits for more.
///
/// The watch on what `f.wait_on()` names is registered before the reads
/// it covers, so a write landing after the last read and before the wait
/// wakes the wait rather than waiting for the next write; a path replaced
/// or removed is watched again before the next read. A history with a
/// descriptor rather than paths waits on that; one with neither waits
/// `every` (a remote log), or until stop.
pub fn follow(
    f: &mut dyn Follow,
    most: usize,
    stop: &Stop,
    each: &mut dyn FnMut(Result<Slice>) -> bool,
) {
    let w = Watcher::new();
    loop {
        let on = f.wait_on();
        // 1. Watched before reading: a write from now on wakes the wait.
        w.watch(&on.paths);
        // 2. Read until nothing more is waiting.
        loop {
            if stop.is_closed() {
                return;
            }
            match f.next(most) {
                Ok(s) => {
                    let more = s.more;
                    if (!s.events.is_empty() || s.reset) && !each(Ok(s)) {
                        return;
                    }
                    if !more {
                        break;
                    }
                }
                Err(e) => {
                    if !each(Err(e)) {
                        return;
                    }
                    break;
                }
            }
        }
        // 3. Wait on the watch.
        let woke = match on.fd {
            Some(fd) if on.paths.is_empty() => wait_fd(fd, stop, on.every),
            _ => w.wait(Some(stop), on.every),
        };
        if !woke {
            return;
        }
    }
}

/// Waits for fd to turn readable, stop to close, or every to pass; false
/// when stop closed.
#[cfg(unix)]
fn wait_fd(fd: i32, stop: &Stop, every: Option<Duration>) -> bool {
    use std::os::fd::AsRawFd;
    let ms = every.map_or(-1, |d| d.as_millis().min(i32::MAX as u128) as i32);
    let mut fds = [
        libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: stop.r.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        // SAFETY: fds is a live array of two pollfds.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, ms) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return !stop.is_closed();
    }
}

#[cfg(not(unix))]
fn wait_fd(_: i32, stop: &Stop, every: Option<Duration>) -> bool {
    !stop.wait(every.unwrap_or(Duration::from_secs(2)))
}
