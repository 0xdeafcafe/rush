//! Running a program for its output with a time limit, as Go's
//! `exec.CommandContext(ctx, …).Output()` with a timeout does: past the
//! limit the program is killed. Waiting blocks on the program; nothing is
//! polled.

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A program that ran: how it ended and what it wrote.
pub struct Out {
    pub status: ExitStatus,
    pub out: Vec<u8>,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.status.success()
    }

    /// How it ended, as Go's exec error says it: "exit status 3",
    /// "signal: killed".
    pub fn err_text(&self) -> String {
        match (self.status.code(), self.status.signal()) {
            (Some(c), _) => format!("exit status {c}"),
            (None, Some(libc::SIGKILL)) => "signal: killed".into(),
            (None, Some(s)) => format!("signal: {s}"),
            _ => "exit status unknown".into(),
        }
    }
}

/// Why a program gave no output.
#[derive(Debug)]
pub enum Error {
    /// It couldn't start.
    Start(std::io::Error),
    /// It ran past its limit and was killed.
    Timeout,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Start(e) => e.fmt(f),
            Error::Timeout => f.write_str("signal: killed"),
        }
    }
}

/// Runs cmd for its stdout.
pub fn output_within(cmd: &mut Command, limit: Duration) -> Result<Out, Error> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(Error::Start)?;
    let out = child.stdout.take().expect("stdout is piped");
    wait(child, Box::new(out), limit)
}

/// Runs cmd with stdout and stderr together, as Go's `CombinedOutput`.
pub fn combined_within(cmd: &mut Command, limit: Duration) -> Result<Out, Error> {
    let (r, w) = std::io::pipe().map_err(Error::Start)?;
    cmd.stdin(Stdio::null())
        .stdout(w.try_clone().map_err(Error::Start)?)
        .stderr(w);
    let child = cmd.spawn();
    // cmd keeps the write ends it was given: swap them out so the read end
    // sees the end once the program exits.
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    wait(child.map_err(Error::Start)?, Box::new(r), limit)
}

fn wait(mut child: Child, mut out: Box<dyn Read + Send>, limit: Duration) -> Result<Out, Error> {
    let pid = child.id() as i32;
    // reaped guards the pid: it's only signalled while the program is
    // unreaped, so never a pid that's since become another's.
    let reaped = Arc::new(Mutex::new(false));
    let (tx, rx) = mpsc::channel();
    let r2 = reaped.clone();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        wait_exit(pid);
        let mut g = r2.lock().unwrap();
        let st = child.wait();
        *g = true;
        drop(g);
        if let Ok(status) = st {
            let _ = tx.send(Out { status, out: buf });
        }
    });
    match rx.recv_timeout(limit) {
        Ok(r) => Ok(r),
        Err(_) => {
            let g = reaped.lock().unwrap();
            if !*g {
                // SAFETY: pid is our unreaped child.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            Err(Error::Timeout)
        }
    }
}

/// Blocks until pid, our child, has exited, without reaping it.
pub(crate) fn wait_exit(pid: i32) {
    loop {
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
        if r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_and_limit() {
        let five = Duration::from_secs(5);
        let o = output_within(
            Command::new("sh").args(["-c", "echo hi; echo err >&2"]),
            five,
        )
        .unwrap();
        assert!(o.ok());
        assert_eq!(o.out, b"hi\n");
        let o = combined_within(
            Command::new("sh").args(["-c", "echo hi; echo err >&2; exit 3"]),
            five,
        )
        .unwrap();
        assert!(!o.ok());
        assert_eq!(o.err_text(), "exit status 3");
        assert_eq!(o.out, b"hi\nerr\n");
        let e = output_within(Command::new("sleep").arg("30"), Duration::from_millis(50));
        assert!(matches!(e, Err(Error::Timeout)));
        assert_eq!(Error::Timeout.to_string(), "signal: killed");
        let e = output_within(&mut Command::new("/no/such/program"), five);
        assert!(matches!(e, Err(Error::Start(_))));
    }
}
