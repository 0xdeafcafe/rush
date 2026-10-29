//! exec: a program the plugin's manifest names, run for it outside the
//! sandbox and as you, with the arguments it adds to its fixed command
//! line.
//! Ported from internal/plugind/exec.go.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rush_plugin::rpc::{CODE_INVALID_PARAMS, denied};
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
        let mut child = cmd.spawn().map_err(|e| server(spawn_text(&argv[0], &e)))?;
        let (inp, out, errp) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let stdin = stdin.to_string();
        let feed = std::thread::spawn(move || {
            if let Some(mut w) = inp {
                let _ = w.write_all(stdin.as_bytes());
            }
        });
        let read = |r: Option<Box<dyn Read + Send>>| std::thread::spawn(move || capped(r));
        let out = read(out.map(|r| Box::new(r) as Box<dyn Read + Send>));
        let errt = read(errp.map(|r| Box::new(r) as Box<dyn Read + Send>));
        let pid = child.id();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait());
        });
        let ctx = ctx.with_timeout(EXEC_TIMEOUT);
        let left = ctx.deadline().map_or(EXEC_TIMEOUT, |d| {
            d.saturating_duration_since(Instant::now())
        });
        let status = match rx.recv_timeout(left) {
            Ok(st) => st,
            Err(_) => {
                // SAFETY: kills the program we started, by its pid.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
                let _ = rx.recv_timeout(WAIT_DELAY);
                return Err(server(format!(
                    "{name} took over {} and was stopped",
                    crate::go_secs(EXEC_TIMEOUT)
                )));
            }
        };
        let _ = feed.join();
        let (stdout, over_out) = out.join().unwrap_or_default();
        let (stderr, over_err) = errt.join().unwrap_or_default();
        let st = status.map_err(server)?;
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

/// The first MAX_EXEC_OUT bytes read from r, and whether there was more.
fn capped(r: Option<Box<dyn Read + Send>>) -> (String, bool) {
    let Some(mut r) = r else {
        return (String::new(), false);
    };
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
