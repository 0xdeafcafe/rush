//! exec: a program the plugin's manifest names, run for it outside the
//! sandbox and as you, with the arguments it adds to its fixed command
//! line.
//! Ported from internal/plugind/exec.go.

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rush_plugin::rpc::{CODE_INVALID_PARAMS, denied};
use rush_plugin::sys::ReadCloser;
use rush_plugin::{Ctx, Error, Plugin, Raw};

use crate::runner::Runner;
use crate::{object, server, to_raw, within};

// Limits on the programs a plugin runs.
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_EXEC_ARGS: usize = 64;
const MAX_EXEC_ARG: usize = 4 << 10;
const MAX_EXEC_IN: usize = 1 << 20;
/// each of stdout and stderr
const MAX_EXEC_OUT: usize = 1 << 20;
/// running at once, per plugin
pub(crate) const MAX_EXECS: usize = 4;
/// How long its output may stay open after it exits (Go's WaitDelay).
const WAIT_DELAY: Duration = Duration::from_secs(2);

fn bad(msg: &str) -> Error {
    Error::new(CODE_INVALID_PARAMS, msg)
}

/// Gives back its place among the programs running, however exec ends.
struct Slot<'a>(&'a std::sync::atomic::AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Runner {
    /// Runs a program the manifest names, outside the sandbox and as you,
    /// with the arguments the plugin adds to its fixed command line: in
    /// cwd if that's in one of its workspaces, else its data folder.
    pub(crate) fn exec(
        &self,
        ctx: &Ctx,
        p: &Plugin,
        name: &str,
        args: &[String],
        stdin: &str,
        cwd: &str,
    ) -> Result<Option<Raw>, Error> {
        let Some(argv) = p.exec_argv(name) else {
            return Err(denied(&format!(
                "exec {name}: its manifest names no such program"
            )));
        };
        if args.len() > MAX_EXEC_ARGS || stdin.len() > MAX_EXEC_IN {
            return Err(bad("too many arguments or too much input"));
        }
        if args
            .iter()
            .any(|a| a.len() > MAX_EXEC_ARG || a.contains('\0'))
        {
            return Err(bad("an argument is too long or has a NUL in it"));
        }
        let mut dir = rush_plugin::data_dir(&p.name)
            .to_string_lossy()
            .into_owned();
        if !cwd.is_empty() {
            let d = rush_plugin::eval_symlinks(cwd);
            let Some(d) = d.filter(|_| cwd.starts_with('/')) else {
                return Err(bad("cwd does not exist"));
            };
            if !p.workspace_dirs().iter().any(|w| within(&d, w)) {
                return Err(denied(&format!("{cwd} is outside the plugin's workspaces")));
            }
            dir = d;
        }
        let taken = self
            .execs
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_EXECS).then_some(n + 1)
            });
        if taken.is_err() {
            return Err(denied(&format!(
                "it already has {MAX_EXECS} programs running"
            )));
        }
        let _slot = Slot(&self.execs);

        self.log.print(&format!("runs {name} {}", args.join(" ")));
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .args(args)
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Nothing else rush holds open goes with it.
        rush_plugin::sandbox::keep_only_stdio(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| server(spawn_text(&argv[0], &e)))?;
        let (inp, out, errp) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let stdin = stdin.to_string();
        // Not waited for: a program that leaves its input unread, to a
        // child of its own that keeps it open, would hold the write up.
        std::thread::spawn(move || {
            if let Some(mut w) = inp {
                let _ = w.write_all(stdin.as_bytes());
            }
        });
        let (Some(out), Some(errp)) = (out, errp) else {
            return Err(server("no pipes to the program"));
        };
        let (out, out_closer) = read(out.into())?;
        let (errt, err_closer) = read(errp.into())?;
        let pid = child.id();
        let (tx, rx) = mpsc::channel();
        let exited = tx.clone();
        std::thread::spawn(move || {
            let _ = exited.send(Some(child.wait()));
        });
        // Whoever asked going (the plugin's connection closing) stops it
        // too, as Go's exec.CommandContext does.
        let _reg = ctx.on_cancel(move || {
            let _ = tx.send(None);
        });
        let ctx = ctx.with_timeout(EXEC_TIMEOUT);
        let left = ctx.deadline().map_or(EXEC_TIMEOUT, |d| {
            d.saturating_duration_since(Instant::now())
        });
        let status = match rx.recv_timeout(left) {
            Ok(Some(st)) => st,
            got => {
                // SAFETY: kills the program we started, by its pid.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
                let _ = rx.recv_timeout(WAIT_DELAY);
                out_closer.close();
                err_closer.close();
                if got.is_ok() {
                    return Err(server("context canceled"));
                }
                return Err(server(format!(
                    "{name} took over {} and was stopped",
                    crate::go_secs(EXEC_TIMEOUT)
                )));
            }
        };
        let st = status.map_err(server)?;
        // Its output may stay open a while after it exits, held by a child
        // it left; then what came is what there is (Go's WaitDelay).
        let deadline = Instant::now() + WAIT_DELAY;
        let mut late = false;
        let mut take = |rx: &mpsc::Receiver<(String, bool)>, closer: &ReadCloser| {
            rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|_| {
                    late = true;
                    closer.close();
                    rx.recv().unwrap_or_default()
                })
        };
        let (stdout, over_out) = take(&out, &out_closer);
        let (stderr, over_err) = take(&errt, &err_closer);
        if late && st.success() {
            return Err(server("exec: WaitDelay expired before I/O complete"));
        }
        let code = match st.code() {
            Some(c) => c as i64,
            None => -1,
        };
        Ok(Some(object(vec![
            ("code", to_raw(&code)),
            ("stdout", to_raw(&stdout)),
            ("stderr", to_raw(&stderr)),
            ("truncated", to_raw(&(over_out || over_err))),
        ])))
    }
}

/// Reads fd to its end (or MAX_EXEC_OUT of it) on a thread: what it read
/// comes on the channel, and the closer ends the reading early.
fn read(fd: OwnedFd) -> Result<(mpsc::Receiver<(String, bool)>, ReadCloser), Error> {
    let (r, closer) = rush_plugin::sys::wakeable(fd).map_err(server)?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(capped(r));
    });
    Ok((rx, closer))
}

/// The first MAX_EXEC_OUT bytes read from r, and whether there was more.
fn capped(mut r: impl Read) -> (String, bool) {
    let mut buf = Vec::new();
    let mut over = false;
    let mut chunk = [0u8; 32 << 10];
    loop {
        match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = MAX_EXEC_OUT - buf.len();
                if n > room {
                    over = true;
                    buf.extend_from_slice(&chunk[..room]);
                } else {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    (String::from_utf8_lossy(&buf).into_owned(), over)
}

fn spawn_text(prog: &str, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound && !prog.contains('/') {
        return format!("exec: \"{prog}\": executable file not found in $PATH");
    }
    format!("fork/exec {prog}: {}", rush_plugin::gopath::errno_text(e))
}
