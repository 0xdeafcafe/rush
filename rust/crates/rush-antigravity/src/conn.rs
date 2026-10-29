//! An Antigravity session: agy in its persistent stream-json mode, a turn
//! at a time, its history kept by rush as it goes. Port of
//! internal/adapters/antigravity/conn.go:1-255 and history.go:31-81
//! (remember, saveEvent).
//!
//! Go's reader goroutine sends into a channel; here the reader thread
//! hands each event to the caller's Sink, which never blocks, and ends it
//! last. Close kills agy's process group and waits for the reader.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rush_core::adapter::{self, Conn, Input, Sink, StartOptions};
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};

use crate::history::{append, history_path, open};
use crate::says;
use crate::stream::{Parser, rand_text};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// agy's command line for o.
pub(crate) fn launch_args(o: &StartOptions) -> adapter::Result<Vec<String>> {
    if o.fork || !o.carry.is_empty() {
        return Err(says(
            "Antigravity cannot import or fork a conversation through its streaming interface",
        ));
    }
    if !o.api_key.is_empty() {
        return Err(says(
            "configure modelProvider=gemini in Antigravity's settings and GEMINI_API_KEY before using API billing",
        ));
    }
    let mut args: Vec<String> = [
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
    ]
    .map(String::from)
    .into();
    if o.resume {
        if o.session_id.is_empty() {
            return Err(says("Antigravity resume requires a conversation ID"));
        }
        args.extend(["--conversation".into(), o.session_id.clone()]);
    }
    if !o.model.is_empty() {
        args.extend(["--model".into(), o.model.clone()]);
    }
    if !o.effort.is_empty() {
        args.extend(["--effort".into(), o.effort.clone()]);
    }
    match o.mode.as_str() {
        "" => {}
        "plan" | "accept-edits" => args.extend(["--mode".into(), o.mode.clone()]),
        "always-proceed" => args.push("--dangerously-skip-permissions".into()),
        m => {
            return Err(says(format!(
                "unsupported Antigravity permission mode {m:?}"
            )));
        }
    }
    args.extend(o.flags.iter().cloned());
    Ok(args)
}

/// What a session keeps under its lock.
struct St {
    busy: bool,
    session: String,
    journal: Option<(File, PathBuf)>,
}

struct Inner {
    st: Mutex<St>,
    stdin: Mutex<Option<ChildStdin>>,
    /// rush's folder, where the history is kept.
    dir: PathBuf,
    pid: i32,
    /// Closed by rush, or stopped on a failure (Go's cancelled context).
    cancelled: AtomicBool,
    /// The reader has stopped.
    finished: AtomicBool,
    /// agy has exited and is reaped, or about to be: its pid (and process
    /// group) may be another's now, so it's signalled no more. Signals
    /// are sent holding this lock.
    reaped: Mutex<bool>,
    sink: Arc<dyn Sink>,
}

impl Inner {
    /// Stops agy, all of its process group.
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.signal(-self.pid, libc::SIGKILL);
    }

    /// Sends sig to pid (agy, or its group as -pid) while agy is unreaped;
    /// false once it's gone.
    fn signal(&self, pid: i32, sig: i32) -> bool {
        let reaped = lock(&self.reaped);
        if *reaped {
            return false;
        }
        // SAFETY: agy is unreaped (reaped is held), so pid is still its.
        unsafe { libc::kill(pid, sig) == 0 }
    }

    fn emit(&self, ev: Event) {
        if !self.cancelled.load(Ordering::SeqCst) {
            self.sink.event(ev);
        }
    }

    /// Appends ev to the history; called with st held.
    fn save_event(&self, st: &mut St, ev: &Event) -> adapter::Result<()> {
        let Some(path) = history_path(&self.dir, &st.session) else {
            return Err(says("missing Antigravity conversation ID"));
        };
        if st.journal.is_none() {
            st.journal = Some((open(&path)?, path));
        }
        let (f, _) = st.journal.as_mut().expect("opened");
        append(f, ev)
    }

    /// Keeps what the conversation needs to be read back: messages, its
    /// start and the ends of its turns. An Init naming agy's own id for
    /// the conversation moves the history to that name.
    fn remember(&self, ev: &Event) -> adapter::Result<()> {
        if !matches!(
            ev,
            Event::Message(_) | Event::Init(_) | Event::TurnEnd(_) | Event::CallUpdated(_)
        ) {
            return Ok(());
        }
        let mut st = lock(&self.st);
        if let Event::Init(init) = ev
            && !init.session_id.is_empty()
            && init.session_id != st.session
        {
            let Some(next) = history_path(&self.dir, &init.session_id) else {
                return Err(says("invalid Antigravity conversation ID"));
            };
            if st.journal.is_some() {
                if next.exists() {
                    return Err(says(
                        "Antigravity returned an existing conversation ID for a new session",
                    ));
                }
                let (_, path) = st.journal.take().expect("there");
                std::fs::rename(&path, &next)?;
            }
            st.session = init.session_id.clone();
        }
        self.save_event(&mut st, ev)
    }
}

/// An Antigravity session.
pub struct AgyConn {
    inner: Arc<Inner>,
    done: Mutex<Option<Receiver<()>>>,
}

/// Starts agy at bin, in rush's folder dir for its history.
pub(crate) fn start(
    bin: &str,
    o: &StartOptions,
    dir: PathBuf,
    sink: Arc<dyn Sink>,
) -> adapter::Result<AgyConn> {
    let args = launch_args(o)?;
    if bin.is_empty() {
        return Err(says("Antigravity CLI is not installed"));
    }
    let mut cmd = Command::new(bin);
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if !o.dir.is_empty() {
        cmd.current_dir(&o.dir);
    }
    for kv in &o.env {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    let mut child = cmd.spawn()?;
    let (stdin, stdout, stderr) = (
        child.stdin.take().expect("piped"),
        child.stdout.take().expect("piped"),
        child.stderr.take().expect("piped"),
    );
    let session = if o.session_id.is_empty() {
        rand_text()
    } else {
        o.session_id.clone()
    };
    let inner = Arc::new(Inner {
        st: Mutex::new(St {
            busy: false,
            session,
            journal: None,
        }),
        stdin: Mutex::new(Some(stdin)),
        dir,
        pid: child.id() as i32,
        cancelled: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        reaped: Mutex::new(false),
        sink,
    });
    let (tx, rx) = mpsc::sync_channel(1);
    let r = inner.clone();
    std::thread::spawn(move || {
        read(&r, child, stdout, stderr);
        let _ = tx.send(());
    });
    Ok(AgyConn {
        inner,
        done: Mutex::new(Some(rx)),
    })
}

/// How a process ended, as Go's exec error says it.
fn exit_text(s: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (s.code(), s.signal()) {
        (Some(c), _) => format!("exit status {c}"),
        (None, Some(libc::SIGKILL)) => "signal: killed".into(),
        (None, Some(n)) => format!("signal: {n}"),
        _ => "exit status unknown".into(),
    }
}

/// The longest line read from agy.
const MAX_LINE: u64 = 8 << 20;
/// How much of what agy says on stderr is kept, to say why it went.
const MAX_DIAGNOSTICS: usize = 16384;

/// Reads what agy says until it ends, then ends the sink.
fn read(
    c: &Inner,
    mut child: std::process::Child,
    out: impl Read,
    mut stderr: impl Read + Send + 'static,
) {
    // Keep diagnostics bounded while continuing to drain the pipe.
    let (dtx, diagnostics) = mpsc::sync_channel::<String>(1);
    std::thread::spawn(move || {
        let (mut b, mut buf) = (Vec::new(), [0u8; 4096]);
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            if b.len() < MAX_DIAGNOSTICS {
                let room = MAX_DIAGNOSTICS - b.len();
                b.extend_from_slice(&buf[..n.min(room)]);
            }
        }
        let _ = dtx.send(String::from_utf8_lossy(&b).into_owned());
    });
    let mut p = Parser::default();
    let mut saw_result = false;
    let mut scan_err: Option<String> = None;
    let mut r = BufReader::with_capacity(64 << 10, out);
    let mut line = Vec::new();
    'lines: loop {
        line.clear();
        match (&mut r).take(MAX_LINE + 1).read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                scan_err = Some(e.to_string());
                break;
            }
        }
        if line.len() as u64 > MAX_LINE {
            scan_err = Some("bufio.Scanner: token too long".into());
            break;
        }
        let b = line.strip_suffix(b"\n").unwrap_or(&line);
        let b = b.strip_suffix(b"\r").unwrap_or(b);
        let ended = match p.line(b) {
            Ok(e) => e,
            Err(e) => {
                c.emit(turn_error(e));
                c.cancel();
                break;
            }
        };
        for ev in std::mem::take(&mut p.out) {
            if let Err(e) = c.remember(&ev) {
                c.emit(turn_error(format!("saving Antigravity history: {e}")));
                c.cancel();
                break 'lines;
            }
            c.emit(ev);
        }
        if ended {
            saw_result = true;
            lock(&c.st).busy = false;
            let end = Event::TurnEnd(p.end.clone());
            if let Err(e) = c.remember(&end) {
                c.emit(turn_error(e.to_string()));
                c.cancel();
                break;
            }
            c.emit(end);
        }
    }
    drop(r);
    // Wait for agy to exit without reaping it, so no signal sent meanwhile
    // can reach a process that has taken its pid; then reap it.
    wait_exit(c.pid);
    *lock(&c.reaped) = true;
    let status = child.wait();
    // Go's Wait closes the stderr pipe once agy has exited: a process agy
    // left holding it doesn't keep the session from ending.
    let detail = diagnostics
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    let busy = {
        let mut st = lock(&c.st);
        st.journal = None;
        std::mem::replace(&mut st.busy, false)
    };
    let failed = !matches!(&status, Ok(s) if s.success());
    if !c.cancelled.load(Ordering::SeqCst)
        && (busy || (!saw_result && (failed || scan_err.is_some())))
    {
        let mut detail = detail.trim().to_string();
        if detail.is_empty() {
            detail = "Antigravity exited before completing its turn".into();
            if failed {
                let why = match &status {
                    Ok(s) => exit_text(*s),
                    Err(e) => e.to_string(),
                };
                detail += &format!(": {why}");
            }
            if let Some(e) = &scan_err {
                detail += &format!(": {e}");
            }
        }
        c.emit(turn_error(detail));
    }
    c.finished.store(true, Ordering::SeqCst);
    c.sink.ended(None);
}

/// Blocks until pid has exited, leaving it unreaped.
fn wait_exit(pid: i32) {
    loop {
        // SAFETY: a zeroed siginfo_t is a valid one to fill.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waiting on our own child, without reaping it.
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

fn turn_error(err: impl Into<String>) -> Event {
    Event::TurnEnd(TurnEnd {
        reason: "error".into(),
        err: err.into(),
        ..TurnEnd::default()
    })
}

impl Conn for AgyConn {
    fn send(&self, input: Input) -> adapter::Result<()> {
        if !input.images.is_empty() {
            return Err(says("Antigravity streaming input supports text only"));
        }
        if input.text.trim().starts_with('/') {
            return Err(says(
                "native Antigravity slash commands are unavailable in Rush streaming mode",
            ));
        }
        let c = &self.inner;
        let mut st = lock(&c.st);
        if c.finished.load(Ordering::SeqCst) {
            return Err(says("Antigravity session has ended"));
        }
        if st.busy {
            return Err(says(
                "Antigravity is working; queue this message until the turn ends",
            ));
        }
        let mut b = serde_json::to_vec(
            &serde_json::json!({"event": "user", "message": {"content": input.text}}),
        )?;
        b.push(b'\n');
        {
            let mut w = lock(&c.stdin);
            let Some(w) = w.as_mut() else {
                return Err(says("Antigravity session has ended"));
            };
            w.write_all(&b)?;
            w.flush()?;
        }
        let ev = Event::Message(Message {
            role: "user".into(),
            parts: vec![Part {
                kind: PartKind::Text,
                text: input.text,
                ..Part::default()
            }],
            ..Message::default()
        });
        if let Err(e) = c.save_event(&mut st, &ev) {
            drop(st);
            c.cancel();
            return Err(e);
        }
        st.busy = true;
        Ok(())
    }

    fn answer(&self, _: &str, _: &str) -> adapter::Result<()> {
        Err(says(
            "Antigravity headless permissions are controlled by its native policy",
        ))
    }

    fn interrupt(&self) -> adapter::Result<()> {
        if !self.inner.signal(self.inner.pid, libc::SIGINT) {
            return Err(says("os: process already finished"));
        }
        Ok(())
    }

    fn set_model(&self, _: &str) -> adapter::Result<()> {
        Err(says("Antigravity model changes require a new runtime"))
    }

    fn set_mode(&self, _: &str) -> adapter::Result<()> {
        Err(says("Antigravity permission changes require a new runtime"))
    }

    fn close(&self) -> adapter::Result<()> {
        let Some(done) = lock(&self.done).take() else {
            return Ok(());
        };
        self.inner.cancel();
        lock(&self.inner.stdin).take();
        let _ = done.recv();
        Ok(())
    }

    fn pid(&self) -> Option<i32> {
        Some(self.inner.pid)
    }
}
