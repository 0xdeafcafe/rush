//! Talking to one pi over JSON lines on its stdin and stdout: commands
//! with an id and their answers, and every other line handed on in order.
//! Port of internal/adapters/pi/rpc.go:1-268.
//!
//! Go waits on an answer with a context; here a call waits on a channel of
//! one with a time limit, and the reader thread fills it. Nothing polls.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rush_sys::pgguard::{self, Guard};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::json::nd;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Pi's answer to a command: its id, and data or an error.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Response {
    #[serde(deserialize_with = "nd")]
    id: String,
    #[serde(deserialize_with = "nd")]
    command: String,
    #[serde(deserialize_with = "nd")]
    success: bool,
    data: Option<Box<RawValue>>,
    #[serde(deserialize_with = "nd")]
    error: String,
}

/// Why a call came to nothing.
#[derive(Debug)]
pub enum CallError {
    /// pi has gone (Go's errClosed).
    Closed,
    /// A command pi refused (Go's rpcError).
    Refused {
        command: String,
        message: String,
    },
    /// No answer in time (Go's context deadline).
    Timeout,
    Io(std::io::Error),
    Decode(serde_json::Error),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Closed => f.write_str("pi: closed"),
            CallError::Refused { command, message } => write!(f, "pi: {command}: {message}"),
            CallError::Timeout => f.write_str("context deadline exceeded"),
            CallError::Io(e) => e.fmt(f),
            CallError::Decode(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CallError {}

/// What's handed each line that isn't an answer to a call: its type, and
/// the line. Called in order, on the reading thread; it must not wait on a
/// call.
pub(crate) type Handle = Box<dyn Fn(&str, &[u8]) + Send + Sync>;

/// Told once the reader has stopped: pi has gone.
pub(crate) type Done = Box<dyn FnOnce(&Client) + Send>;

/// One pi on the far end of a pipe pair.
pub(crate) struct Client {
    w: Mutex<Option<Box<dyn Write + Send>>>,
    st: Mutex<St>,
    child: Mutex<Option<Child>>,
    pub(crate) pid: i32,
    guard: Mutex<Option<Guard>>,
    stderr: Arc<Tail>,
    /// Told when stderr has been read to its end.
    stderr_done: Mutex<Option<Receiver<()>>>,
}

#[derive(Default)]
struct St {
    next: i64,
    pending: HashMap<String, SyncSender<Response>>,
    closed: bool,
}

/// The longest line read: a tool result or a message with an image in it
/// can be large.
const MAX_LINE: u64 = 64 << 20;

impl Client {
    /// A client writing to w; start reads.
    pub(crate) fn new(w: Box<dyn Write + Send>) -> Client {
        Client {
            w: Mutex::new(Some(w)),
            st: Mutex::new(St::default()),
            child: Mutex::new(None),
            pid: 0,
            guard: Mutex::new(None),
            stderr: Arc::new(Tail::default()),
            stderr_done: Mutex::new(None),
        }
    }

    /// Starts cmd in its own process group and talks to it.
    pub(crate) fn spawn(
        cmd: &mut Command,
        handle: Handle,
        done: Done,
    ) -> std::io::Result<Arc<Client>> {
        use std::os::unix::process::CommandExt;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = cmd.spawn()?;
        let (stdin, stdout, mut stderr) = (
            child.stdin.take().expect("piped"),
            child.stdout.take().expect("piped"),
            child.stderr.take().expect("piped"),
        );
        let pid = child.id() as i32;
        let mut c = Client::new(Box::new(stdin));
        c.pid = pid;
        *lock(&c.guard) = pgguard::watch(pid);
        *lock(&c.child) = Some(child);
        let (tx, rx) = mpsc::sync_channel(1);
        *lock(&c.stderr_done) = Some(rx);
        let tail = c.stderr.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                tail.write(&buf[..n]);
            }
            let _ = tx.send(());
        });
        let c = Arc::new(c);
        c.clone().start(stdout, handle, done);
        Ok(c)
    }

    /// Reads r on a thread of its own until it ends.
    pub(crate) fn start(
        self: Arc<Self>,
        r: impl Read + Send + 'static,
        handle: Handle,
        done: Done,
    ) {
        std::thread::spawn(move || {
            self.read(r, &handle);
            // What pi said on its way out is all there before it's asked.
            if let Some(rx) = lock(&self.stderr_done).take() {
                let _ = rx.recv_timeout(Duration::from_secs(1));
            }
            done(&self);
        });
    }

    /// Splits what pi writes on LF alone, as its protocol asks: a JSON
    /// string can hold U+2028, which other line readers split on too.
    fn read(&self, r: impl Read, handle: &Handle) {
        let mut br = BufReader::with_capacity(64 << 10, r);
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut br).take(MAX_LINE + 1).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.len() as u64 > MAX_LINE {
                break;
            }
            let mut b = &line[..];
            while let [rest @ .., b'\r' | b'\n'] = b {
                b = rest;
            }
            if !b.is_empty() {
                self.line(b, handle);
            }
        }
        let mut st = lock(&self.st);
        st.closed = true;
        st.pending.clear(); // each waiting call sees its channel close
    }

    fn line(&self, b: &[u8], handle: &Handle) {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Head {
            #[serde(rename = "type", deserialize_with = "nd")]
            typ: String,
        }
        let Ok(head) = serde_json::from_slice::<Head>(b) else {
            return;
        };
        if head.typ == "response" {
            let Ok(r) = serde_json::from_slice::<Response>(b) else {
                return;
            };
            if r.id.is_empty() {
                return; // an answer to a command sent without an id
            }
            let ch = lock(&self.st).pending.remove(&r.id);
            if let Some(ch) = ch {
                let _ = ch.try_send(r);
            }
            return;
        }
        handle(&head.typ, b);
    }

    fn write(&self, m: &Map<String, Value>) -> Result<(), CallError> {
        let mut b = serde_json::to_vec(m).map_err(CallError::Decode)?;
        b.push(b'\n');
        let mut w = lock(&self.w);
        let Some(w) = w.as_mut() else {
            return Err(CallError::Closed);
        };
        w.write_all(&b)
            .and_then(|_| w.flush())
            .map_err(CallError::Io)
    }

    /// Sends a command, given as its fields less the id, and waits up to
    /// limit for its answer's data (None for none).
    pub(crate) fn call(
        &self,
        cmd: &Map<String, Value>,
        limit: Duration,
    ) -> Result<Option<Box<RawValue>>, CallError> {
        let (tx, rx) = mpsc::sync_channel(1);
        let id = {
            let mut st = lock(&self.st);
            if st.closed {
                return Err(CallError::Closed);
            }
            st.next += 1;
            let id = format!("rush-{}", st.next);
            st.pending.insert(id.clone(), tx);
            id
        };
        let mut msg = cmd.clone();
        msg.insert("id".into(), Value::String(id.clone()));
        if let Err(e) = self.write(&msg) {
            lock(&self.st).pending.remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(limit) {
            Ok(r) if !r.success => Err(CallError::Refused {
                command: r.command,
                message: r.error,
            }),
            Ok(r) => Ok(r.data.filter(|d| d.get() != "null")),
            Err(RecvTimeoutError::Disconnected) => Err(CallError::Closed),
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.st).pending.remove(&id);
                Err(CallError::Timeout)
            }
        }
    }

    /// call, with its data decoded; None when it had none.
    pub(crate) fn call_into<T: DeserializeOwned>(
        &self,
        cmd: &Map<String, Value>,
        limit: Duration,
    ) -> Result<Option<T>, CallError> {
        match self.call(cmd, limit)? {
            None => Ok(None),
            Some(d) => serde_json::from_str(d.get())
                .map(Some)
                .map_err(CallError::Decode),
        }
    }

    /// Sends a command without waiting for its answer.
    pub(crate) fn post(&self, cmd: &Map<String, Value>) -> Result<(), CallError> {
        if lock(&self.st).closed {
            return Err(CallError::Closed);
        }
        self.write(cmd)
    }

    /// Ends pi: stdin first, then its process group, SIGTERM and a while
    /// later SIGKILL.
    pub(crate) fn close(&self) {
        lock(&self.w).take();
        let Some(mut child) = lock(&self.child).take() else {
            return;
        };
        let guard = lock(&self.guard).take();
        let pid = self.pid;
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = child.wait();
            if let Some(g) = guard {
                g.release();
            }
            let _ = tx.send(());
        });
        if rx.recv_timeout(Duration::from_secs(1)).is_ok() {
            return;
        }
        // SAFETY: signalling the process group pi leads.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        if rx.recv_timeout(Duration::from_secs(2)).is_err() {
            // SAFETY: as above.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            let _ = rx.recv();
        }
    }

    /// What pi last wrote to stderr, to say why it went.
    pub(crate) fn said(&self) -> String {
        self.stderr.string()
    }
}

/// Keeps the last few kilobytes written to it.
#[derive(Default)]
struct Tail(Mutex<Vec<u8>>);

const TAIL_SIZE: usize = 4 << 10;

impl Tail {
    fn write(&self, p: &[u8]) {
        let mut b = lock(&self.0);
        b.extend_from_slice(p);
        if b.len() > TAIL_SIZE {
            let cut = b.len() - TAIL_SIZE;
            b.drain(..cut);
        }
    }

    fn string(&self) -> String {
        String::from_utf8_lossy(&lock(&self.0)).trim().to_string()
    }
}

/// s's last line.
pub(crate) fn last_line(s: &str) -> String {
    let s = s.trim();
    match s.rfind('\n') {
        Some(i) => s[i + 1..].trim().to_string(),
        None => s.to_string(),
    }
}
