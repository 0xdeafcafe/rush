//! One shell command in a session's folder, for the web app: what `!` in
//! rush's message box does, from a phone. New in the Rust port; it runs
//! the way rush's shell box does (your login shell with `-lc`, nothing to
//! read, a process group of its own), in the folder the session's info
//! names and never one the client names.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

use crate::config::current;
use crate::http::{Request, Response};
use crate::server::{Server, clip, fail, read_json, valid_id, write_json};

/// A command, and how many seconds it gets: a minute when zero, ten at most.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ShellRequest {
    pub command: String,
    pub timeout: u64,
}

/// What it printed (stdout and stderr as they came, the end of it when
/// there was more than is kept), how it ended, and why it didn't run or
/// was stopped.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ShellReply {
    #[serde(serialize_with = "ser_b64")]
    pub out: Vec<u8>,
    /// Its exit status; none when a signal ended it or it never started.
    pub code: Option<i32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// The start of what it printed was dropped.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

fn ser_b64<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&STANDARD.encode(b))
}

/// How much of a command's output is kept: its end.
pub const SHELL_OUT_MOST: usize = 256 << 10;
const SHELL_TIME: u64 = 60;
const SHELL_TIME_MOST: u64 = 600;

/// How long a command asking for secs gets.
pub fn shell_time(secs: u64) -> Duration {
    Duration::from_secs(match secs {
        0 => SHELL_TIME,
        s => s.min(SHELL_TIME_MOST),
    })
}

/// Runs command with shell in dir, as a login shell's one command with
/// nothing to read, keeping the last `most` bytes of what it and anything
/// it started printed, and killing all of it once limit has passed.
pub fn run_shell(
    dir: &Path,
    shell: &str,
    command: &str,
    limit: Duration,
    most: usize,
) -> ShellReply {
    let child = Command::new("/bin/sh")
        .args(["-c", r#"exec "$0" -lc "$1" 2>&1"#, shell, command])
        .current_dir(dir)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return ShellReply {
                error: e.to_string(),
                ..ShellReply::default()
            };
        }
    };
    let pid = child.id() as i32;
    let late = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let flag = late.clone();
    std::thread::spawn(move || {
        if rx.recv_timeout(limit).is_err() {
            flag.store(true, Ordering::SeqCst);
            // SAFETY: kill only signals the group of the shell this started.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    });
    let mut out = Vec::new();
    let mut cut = false;
    if let Some(mut so) = child.stdout.take() {
        let mut buf = [0u8; 64 << 10];
        while let Ok(n) = so.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if out.len() > most * 2 {
                out.drain(..out.len() - most);
                cut = true;
            }
        }
    }
    if out.len() > most {
        out.drain(..out.len() - most);
        cut = true;
    }
    let st = child.wait();
    let _ = tx.send(());
    let late = late.load(Ordering::SeqCst);
    let (code, error) = match st {
        _ if late => (None, format!("stopped after {}s", limit.as_secs())),
        Ok(s) => match s.code() {
            Some(c) => (Some(c), String::new()),
            None => (None, "killed by a signal".into()),
        },
        Err(e) => (None, e.to_string()),
    };
    ShellReply {
        out,
        code,
        error,
        cut,
    }
}

impl Server {
    /// Runs one command in a session's folder: for a session of this
    /// machine, in the folder it works in, unless remote.json's noShell
    /// says no. Each is written to the session's events.
    pub(crate) fn shell(&self, r: &Request, id: &str) -> Response {
        if !valid_id(id) {
            return fail(404, "no such session");
        }
        let req = match read_json::<ShellRequest>(r, 1 << 20) {
            Ok(q) => q,
            Err(e) => return fail(400, &e),
        };
        if req.command.trim().is_empty() {
            return fail(400, "nothing to run");
        }
        if current().no_shell {
            return fail(
                403,
                "the shell is off on this machine: noShell in remote.json",
            );
        }
        let Ok(info) = rush_host::read_info(id) else {
            return fail(404, "no such session");
        };
        if info.is_remote() {
            return fail(403, "that session runs on another machine");
        }
        if info.cwd.is_empty() || !Path::new(&info.cwd).is_dir() {
            return fail(409, "the session's folder isn't there");
        }
        if let Err(e) = self.allowed_cwd(&info.cwd) {
            return fail(403, &e);
        }
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let rep = run_shell(
            Path::new(&info.cwd),
            &shell,
            &req.command,
            shell_time(req.timeout),
            SHELL_OUT_MOST,
        );
        let mut f = json!({ "command": clip(&req.command), "code": rep.code });
        if !rep.error.is_empty() {
            f["err"] = json!(rep.error);
        }
        rush_host::events::log_event(id, "remote-shell", f);
        write_json(&rep)
    }
}
