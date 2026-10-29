//! Running a program as Go's exec.CommandContext does: killed when its
//! Ctx is done or its time is up. The wait blocks on a channel that the
//! program's exit, the Ctx's cancel or the deadline ends; nothing polls.

use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use rush_plugin::Ctx;

/// What a program said and how it ended.
pub(crate) struct Out {
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Out {
    pub fn success(&self) -> bool {
        self.status.is_some_and(|s| s.success())
    }
}

enum Done {
    Exited(std::io::Result<ExitStatus>),
    Stop,
}

/// Runs cmd to its end, killing it when ctx is done or limit (if any)
/// passes. Err only when it couldn't start. combined puts stderr in
/// stdout, as Go's CombinedOutput.
pub(crate) fn run(
    cmd: Command,
    ctx: &Ctx,
    limit: Option<Duration>,
    combined: bool,
) -> std::io::Result<Out> {
    run_fed(cmd, None, ctx, limit, combined)
}

/// What cmd prints with input on its stdin, stderr dropped, as Go's
/// Output with Stdin set: empty when it couldn't start.
pub(crate) fn run_with_stdin(
    cmd: Command,
    input: Vec<u8>,
    ctx: &Ctx,
    limit: Option<Duration>,
) -> Vec<u8> {
    run_fed(cmd, Some(input), ctx, limit, false)
        .map(|o| o.stdout)
        .unwrap_or_default()
}

fn run_fed(
    mut cmd: Command,
    input: Option<Vec<u8>>,
    ctx: &Ctx,
    limit: Option<Duration>,
    combined: bool,
) -> std::io::Result<Out> {
    let ctx = match limit {
        Some(l) => ctx.with_timeout(l),
        None => ctx.clone(),
    };
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let (out_r, err_r) = if combined {
        let (r, w) = pipe()?;
        let w2 = w.try_clone()?;
        cmd.stdout(Stdio::from(w)).stderr(Stdio::from(w2));
        (r, None)
    } else {
        let (r, w) = pipe()?;
        let (er, ew) = pipe()?;
        cmd.stdout(Stdio::from(w)).stderr(Stdio::from(ew));
        (r, Some(er))
    };
    let spawned = cmd.spawn();
    drop(cmd); // its copies of the write ends, so reads see the end
    let mut child = spawned?;
    let pid = child.id() as libc::pid_t;
    if let (Some(input), Some(mut w)) = (input, child.stdin.take()) {
        // Fed on its own thread, as Go's goroutine copies Stdin, so a
        // program that doesn't read it can't hold the wait up.
        std::thread::spawn(move || {
            let _ = std::io::Write::write_all(&mut w, &input);
        });
    }
    let read = |r: OwnedFd| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = std::fs::File::from(r).read_to_end(&mut b);
            b
        })
    };
    let out_t = read(out_r);
    let err_t = err_r.map(read);
    let (tx, rx) = mpsc::channel();
    let stop = tx.clone();
    let _reg = ctx.on_cancel(move || {
        let _ = stop.send(Done::Stop);
    });
    std::thread::spawn(move || {
        let _ = tx.send(Done::Exited(child.wait()));
    });
    let got = match ctx.deadline() {
        Some(d) => rx
            .recv_timeout(d.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or(Done::Stop),
        None => rx.recv().unwrap_or(Done::Stop),
    };
    let status = match got {
        Done::Exited(st) => st.ok(),
        Done::Stop => {
            // SAFETY: the child we started, not yet reaped by its waiter.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
            loop {
                match rx.recv() {
                    Ok(Done::Exited(st)) => break st.ok(),
                    Ok(Done::Stop) => continue,
                    Err(_) => break None,
                }
            }
        }
    };
    Ok(Out {
        status,
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t
            .map(|t| t.join().unwrap_or_default())
            .unwrap_or_default(),
    })
}

fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: fds is two ints for pipe to fill.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: pipe gave us these two descriptors to own.
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    // Both ends close on exec: the child gets the write end as its
    // stdout by dup2, which clears it there, and no other program started
    // meanwhile keeps a copy that would hold the read open.
    for fd in [&r, &w] {
        // SAFETY: plain fcntl on a descriptor we own.
        unsafe {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    Ok((r, w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_and_a_kill() {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "echo out; echo err >&2"]);
        let o = run(c, &Ctx::background(), None, false).unwrap();
        assert!(o.success());
        assert_eq!(
            (&o.stdout[..], &o.stderr[..]),
            (&b"out\n"[..], &b"err\n"[..])
        );
        let mut c = Command::new("/bin/sleep");
        c.arg("30");
        let o = run(c, &Ctx::background(), Some(Duration::from_millis(30)), true).unwrap();
        assert!(!o.success());
        let (ctx, cancel) = Ctx::background().with_cancel();
        cancel.cancel();
        let mut c = Command::new("/bin/sleep");
        c.arg("30");
        assert!(!run(c, &ctx, None, true).unwrap().success());
    }
}
