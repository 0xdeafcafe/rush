//! Talking to one `codex app-server` over newline-delimited JSON-RPC 2.0
//! without its "jsonrpc" field. Port of internal/adapters/codex/rpc.go:
//! 1-277 (message, client, spawn, call, notify, reply, refuse,
//! initialize, versionOf, close, idString).
//!
//! A request has an id and a method, a notification only a method, a
//! response an id and a result or an error. The client's own reader
//! thread hands every notification and server request to its handler, in
//! order; the handler must never wait on a call.
//!
//! Rust-only: a client on an app-server's own pipes can be held between
//! two lines and handed on, its process running on, to the same host
//! exec'd onto a newer rush (pause, kept, adopt), as rush-claude's
//! headless sessions are.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rush_core::adapter;
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::KIND;

/// The longest line the app-server may send.
const MAX_LINE: u64 = 64 << 20;

/// An error the app-server answered with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "codex: {} ({})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// What a call gets once the app-server has gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("codex: app-server closed")
    }
}

impl std::error::Error for Closed {}

/// A call that got no answer in time.
#[derive(Debug)]
struct TimedOut(String);

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "codex: {} got no answer in time", self.0)
    }
}

impl std::error::Error for TimedOut {}

/// One line of the protocol, as read.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Line {
    id: Option<Box<RawValue>>,
    method: String,
    params: Option<Box<RawValue>>,
    result: Option<Box<RawValue>>,
    error: Option<WireError>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireError {
    #[serde(deserialize_with = "crate::rpc::nd")]
    code: i64,
    #[serde(deserialize_with = "crate::rpc::nd")]
    message: String,
}

pub(crate) fn nd<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A notification or a server request.
#[derive(Debug)]
pub(crate) struct Message {
    /// Set for a request, which wants a reply.
    pub id: Option<Box<RawValue>>,
    pub method: String,
    /// "null" when it had none.
    pub params: Box<RawValue>,
}

impl Message {
    pub fn params(&self) -> &str {
        self.params.get()
    }
}

/// What the reader hands its handler.
pub(crate) enum Incoming {
    Message(Message),
    /// The app-server has gone: its output ended. Last.
    Closed,
}

pub(crate) type Handler = Box<dyn Fn(&Client, Incoming) + Send + Sync>;

type Reply = Result<Box<RawValue>, RpcError>;

struct Pending {
    next: i64,
    waiting: HashMap<i64, mpsc::SyncSender<Reply>>,
    closed: bool,
    /// Held to be handed on to the next build: nothing more is written
    /// (Rust-only).
    held: bool,
}

/// The reader held between two lines for an upgrade (Rust-only), as
/// rush-claude's headless reader is.
#[derive(Default)]
struct Hold {
    /// Asked to hold.
    asked: bool,
    /// Held, with what it had read and not taken in.
    held: Option<Vec<u8>>,
    /// The output was read to its end: nothing to hold.
    ended: bool,
}

/// What an app-server is reached through, by number, and what the next
/// build needs to go on talking to it (Rust-only): its pipes, kept open
/// across the exec, the guard's shell and pipe, the last request id, and
/// what was read and not taken in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Kept {
    pub pid: i32,
    pub stdin: RawFd,
    pub stdout: RawFd,
    pub guard: Option<(i32, RawFd)>,
    /// The last request id used: the next build's go on from it.
    pub next: i64,
    pub unread: Vec<u8>,
}

/// The app-server's process: one this process started, or one it took
/// over across an exec, which no std Child has (Rust-only).
enum Proc {
    Std(Child),
    Taken(i32),
}

impl Proc {
    fn wait(&mut self) {
        match self {
            Proc::Std(c) => {
                let _ = c.wait();
            }
            Proc::Taken(pid) => {
                let _ = rush_sys::reexec::reap(*pid);
            }
        }
    }
}

/// The pipe the reader is woken through to hold, and the app-server's
/// pipes by number (Rust-only).
struct Wake {
    r: OwnedFd,
    w: OwnedFd,
    stdin: RawFd,
    stdout: RawFd,
}

/// One app-server's connection.
pub(crate) struct Client {
    w: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<Pending>,
    /// Told each time a call stops waiting.
    idle: Condvar,
    proc: Mutex<Option<(Proc, Option<rush_sys::pgguard::Guard>)>>,
    pid: i32,
    /// The guard's shell and pipe, as an upgrade hands them on.
    guard: Option<(i32, RawFd)>,
    hold: Mutex<Hold>,
    hsig: Condvar,
    /// None for a client on a reader it can't poll (a test's).
    wake: Option<Wake>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn null() -> Box<RawValue> {
    RawValue::from_string("null".into()).expect("null is JSON")
}

fn err(msg: &str) -> adapter::Error {
    msg.to_string().into()
}

/// How a client starts: what it reads first, its process, and the last
/// request id used.
#[derive(Default)]
struct Setup {
    unread: Vec<u8>,
    proc: Option<(Proc, Option<rush_sys::pgguard::Guard>)>,
    pid: i32,
    next: i64,
    wake: Option<Wake>,
}

/// The app-server's output, read only while the reader isn't asked to
/// hold: asked, a read says WouldBlock, between two reads (Rust-only).
struct Polled {
    f: std::fs::File,
    wake: RawFd,
}

impl Read for Polled {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let mut fds = [
                libc::pollfd {
                    fd: self.f.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.wake,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: poll on descriptors we hold, into our array.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if fds[1].revents & libc::POLLIN != 0 {
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            if fds[0].revents != 0 {
                return self.f.read(buf);
            }
        }
    }
}

/// Reads the wake pipe empty.
fn drain(fd: RawFd) {
    let mut b = [0u8; 16];
    // SAFETY: reads into our buffer from our non-blocking pipe.
    while unsafe { libc::read(fd, b.as_mut_ptr().cast(), b.len()) } > 0 {}
}

impl Client {
    /// A client reading r and writing w, its reader thread started.
    #[cfg(test)]
    pub fn new(
        r: impl Read + Send + 'static,
        w: impl Write + Send + 'static,
        handle: Option<Handler>,
    ) -> Arc<Client> {
        Client::run(r, w, handle, Setup::default())
    }

    /// A client on an app-server's own pipes, its reader able to hold
    /// between two lines.
    fn on_fds(
        stdin: OwnedFd,
        stdout: OwnedFd,
        handle: Option<Handler>,
        mut s: Setup,
    ) -> adapter::Result<Arc<Client>> {
        let (r, w) = std::io::pipe()?;
        let (r, w) = (OwnedFd::from(r), OwnedFd::from(w));
        // SAFETY: O_NONBLOCK on our own pipe's read end, so it drains
        // without blocking.
        unsafe {
            let fl = libc::fcntl(r.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(r.as_raw_fd(), libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
        let polled = Polled {
            f: std::fs::File::from(stdout),
            wake: r.as_raw_fd(),
        };
        s.wake = Some(Wake {
            stdin: stdin.as_raw_fd(),
            stdout: polled.f.as_raw_fd(),
            r,
            w,
        });
        Ok(Client::run(polled, std::fs::File::from(stdin), handle, s))
    }

    fn run(
        r: impl Read + Send + 'static,
        w: impl Write + Send + 'static,
        handle: Option<Handler>,
        s: Setup,
    ) -> Arc<Client> {
        let guard = s
            .proc
            .as_ref()
            .and_then(|(_, g)| g.as_ref())
            .and_then(|g| match g.parts() {
                (p, Some(fd)) => Some((p, fd)),
                _ => None,
            });
        let c = Arc::new(Client {
            w: Mutex::new(Some(Box::new(w))),
            pending: Mutex::new(Pending {
                next: s.next,
                waiting: HashMap::new(),
                closed: false,
                held: false,
            }),
            idle: Condvar::new(),
            proc: Mutex::new(s.proc),
            pid: s.pid,
            guard,
            hold: Mutex::new(Hold::default()),
            hsig: Condvar::new(),
            wake: s.wake,
        });
        let c2 = c.clone();
        let unread = s.unread;
        std::thread::Builder::new()
            .name("codex-rpc".into())
            .spawn(move || c2.read(r, unread, handle))
            .expect("a thread");
        c
    }

    /// The app-server's process, when it is one of rush's.
    pub fn pid(&self) -> Option<i32> {
        (self.pid > 0).then_some(self.pid)
    }

    fn read(&self, r: impl Read, unread: Vec<u8>, handle: Option<Handler>) {
        let mut r = BufReader::with_capacity(64 << 10, Cursor::new(unread).chain(r));
        let mut buf = Vec::new();
        loop {
            let room = (MAX_LINE + 1).saturating_sub(buf.len() as u64);
            match (&mut r).take(room).read_until(b'\n', &mut buf) {
                Ok(0) if buf.is_empty() => break,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Asked to hold for an upgrade: what's read of a line
                    // so far is the next build's to read first.
                    let mut left = buf.clone();
                    left.extend_from_slice(r.buffer());
                    self.hold(left);
                    continue;
                }
                Err(_) => break,
            }
            if buf.len() as u64 > MAX_LINE {
                break; // as Go's scanner: a line too long ends the reading
            }
            self.line(&buf, &handle);
            buf.clear();
        }
        {
            let mut p = lock(&self.pending);
            p.closed = true;
            p.waiting.clear(); // each caller is told it's closed
            self.idle.notify_all();
        }
        {
            let mut h = lock(&self.hold);
            h.ended = true;
            self.hsig.notify_all();
        }
        if let Some(h) = &handle {
            h(self, Incoming::Closed);
        }
    }

    /// One line the app-server wrote: a call's answer, or what goes to
    /// the handler.
    fn line(&self, buf: &[u8], handle: &Option<Handler>) {
        let Ok(m) = serde_json::from_slice::<Line>(buf) else {
            return;
        };
        let id = m.id.filter(|i| i.get() != "null");
        if m.method.is_empty() && id.is_some() {
            let n = id.as_ref().and_then(|i| i.get().parse::<i64>().ok());
            if let Some(ch) = n.and_then(|n| self.unwait(n)) {
                let reply = match m.error {
                    Some(e) => Err(RpcError {
                        code: e.code,
                        message: e.message,
                    }),
                    None => Ok(m.result.unwrap_or_else(null)),
                };
                let _ = ch.send(reply);
            }
            return;
        }
        if let Some(h) = handle {
            h(
                self,
                Incoming::Message(Message {
                    id,
                    method: m.method,
                    params: m.params.unwrap_or_else(null),
                }),
            );
        }
    }

    /// Call n waits no more.
    fn unwait(&self, n: i64) -> Option<mpsc::SyncSender<Reply>> {
        let ch = lock(&self.pending).waiting.remove(&n);
        self.idle.notify_all();
        ch
    }

    /// Holds, asked to for an upgrade, with left read and not taken in,
    /// until let go ([`Client::resume`]); an exec ends the process
    /// meanwhile, and this thread with it (Rust-only).
    fn hold(&self, left: Vec<u8>) {
        let mut h = lock(&self.hold);
        if h.asked {
            h.held = Some(left);
            self.hsig.notify_all();
            while h.asked {
                h = self.hsig.wait(h).unwrap_or_else(|e| e.into_inner());
            }
            h.held = None;
        }
        if let Some(w) = &self.wake {
            drain(w.r.as_raw_fd());
        }
    }

    /// Holds the connection for an upgrade (Rust-only): once no call of
    /// rush's waits on an answer (within d), nothing more is written, and
    /// the app-server's output isn't read past the line it's on. What was
    /// read and not taken in; an error, and the connection as it was, when
    /// it can't be.
    pub fn pause(&self, d: Duration) -> adapter::Result<Vec<u8>> {
        let Some(wake) = &self.wake else {
            return Err(err("codex: its app-server can't be handed on"));
        };
        let end = Instant::now() + d;
        {
            let p = lock(&self.pending);
            let (mut p, _) = self
                .idle
                .wait_timeout_while(p, d, |p| !p.waiting.is_empty() && !p.closed)
                .unwrap_or_else(|e| e.into_inner());
            if p.closed {
                return Err(Box::new(Closed));
            }
            if !p.waiting.is_empty() {
                return Err(err("codex: a request of rush's still waits on its answer"));
            }
            p.held = true;
        }
        // A write already under way finishes first.
        drop(lock(&self.w));
        let mut h = lock(&self.hold);
        if h.ended {
            drop(h);
            lock(&self.pending).held = false;
            return Err(Box::new(Closed));
        }
        h.asked = true;
        // SAFETY: one byte into our own pipe.
        unsafe { libc::write(wake.w.as_raw_fd(), b"x".as_ptr().cast(), 1) };
        let left = end.saturating_duration_since(Instant::now());
        let (mut h, _) = self
            .hsig
            .wait_timeout_while(h, left, |h| h.held.is_none() && !h.ended)
            .unwrap_or_else(|e| e.into_inner());
        if let Some(b) = &h.held {
            return Ok(b.clone());
        }
        h.asked = false;
        self.hsig.notify_all();
        drop(h);
        lock(&self.pending).held = false;
        Err(err("codex: its output didn't stop to be handed on"))
    }

    /// Lets a connection held by pause go on as it was.
    pub fn resume(&self) {
        {
            let mut h = lock(&self.hold);
            h.asked = false;
            self.hsig.notify_all();
        }
        lock(&self.pending).held = false;
    }

    /// How the next build reaches the app-server, once held: None for one
    /// that isn't rush's own process on its own pipes.
    pub fn kept(&self, unread: Vec<u8>) -> Option<Kept> {
        let w = self.wake.as_ref()?;
        (self.pid > 0).then(|| Kept {
            pid: self.pid,
            stdin: w.stdin,
            stdout: w.stdout,
            guard: self.guard,
            next: lock(&self.pending).next,
            unread,
        })
    }

    /// Takes up the app-server a process before an exec talked to
    /// (Rust-only): k's pipes, kept open across it, are this process's
    /// now, and its process this one's child, as it was. Nothing is sent
    /// to it; the next request's id follows k's last.
    pub fn adopt(k: Kept, handle: Option<Handler>) -> adapter::Result<Arc<Client>> {
        for fd in [k.stdin, k.stdout] {
            if !rush_sys::reexec::is_open(fd) {
                return Err(err(&format!("codex: its file {fd} didn't stay open")));
            }
        }
        let _ = rush_sys::reexec::close_on_exec(k.stdin);
        let _ = rush_sys::reexec::close_on_exec(k.stdout);
        // SAFETY: handed over open across the exec, and owned by nothing
        // else in this process.
        let (stdin, stdout) = unsafe {
            (
                OwnedFd::from_raw_fd(k.stdin),
                OwnedFd::from_raw_fd(k.stdout),
            )
        };
        let guard = k.guard.map(|(pid, fd)| rush_sys::pgguard::adopt(pid, fd));
        let s = Setup {
            unread: k.unread,
            proc: Some((Proc::Taken(k.pid), guard)),
            pid: k.pid,
            next: k.next,
            wake: None,
        };
        Client::on_fds(stdin, stdout, handle, s)
    }

    fn write(&self, v: &impl serde::Serialize) -> adapter::Result<()> {
        let mut b = serde_json::to_vec(v)?;
        b.push(b'\n');
        let mut w = lock(&self.w);
        // Checked with the writer held, so none slips past a pause.
        if lock(&self.pending).held {
            return Err(err("codex: held to be handed on"));
        }
        let Some(w) = w.as_mut() else {
            return Err(Box::new(Closed));
        };
        w.write_all(&b)?;
        w.flush()?;
        Ok(())
    }

    /// Sends a request and waits for its result; within, when given.
    pub fn call(
        &self,
        method: &str,
        params: Value,
        within: Option<Duration>,
    ) -> adapter::Result<Box<RawValue>> {
        let (tx, rx) = mpsc::sync_channel(1);
        let id = {
            let mut p = lock(&self.pending);
            if p.closed {
                return Err(Box::new(Closed));
            }
            if p.held {
                return Err(err("codex: held to be handed on"));
            }
            p.next += 1;
            let id = p.next;
            p.waiting.insert(id, tx);
            id
        };
        let mut req = serde_json::Map::new();
        req.insert("id".into(), id.into());
        req.insert("method".into(), method.into());
        if !params.is_null() {
            req.insert("params".into(), params);
        }
        if let Err(e) = self.write(&req) {
            self.unwait(id);
            return Err(e);
        }
        let got = match within {
            Some(d) => rx.recv_timeout(d).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => {
                    self.unwait(id);
                    Box::new(TimedOut(method.to_string())) as adapter::Error
                }
                mpsc::RecvTimeoutError::Disconnected => Box::new(Closed),
            })?,
            None => rx.recv().map_err(|_| Box::new(Closed) as adapter::Error)?,
        };
        got.map_err(|e| Box::new(e) as adapter::Error)
    }

    /// Sends a notification.
    pub fn notify(&self, method: &str) -> adapter::Result<()> {
        self.write(&serde_json::json!({ "method": method }))
    }

    /// Answers a server request.
    pub fn reply(&self, id: &RawValue, result: Value) -> adapter::Result<()> {
        #[derive(serde::Serialize)]
        struct R<'a> {
            id: &'a RawValue,
            result: Value,
        }
        self.write(&R { id, result })
    }

    /// Answers a server request rush doesn't handle.
    pub fn refuse(&self, id: &RawValue, msg: &str) -> adapter::Result<()> {
        #[derive(serde::Serialize)]
        struct R<'a> {
            id: &'a RawValue,
            error: Value,
        }
        self.write(&R {
            id,
            error: serde_json::json!({ "code": -32601, "message": msg }),
        })
    }

    /// The handshake every connection starts with. It returns the Codex
    /// version, read from the user agent it answers with.
    pub fn initialize(&self, within: Option<Duration>) -> adapter::Result<String> {
        // Experimental for thread/backgroundTerminals/terminate, how rush
        // stops a background shell.
        let params = serde_json::json!({
            "clientInfo": {"name": "rush", "title": "rush", "version": "0"},
            "capabilities": {"experimentalApi": true, "requestAttestation": false},
        });
        let res = self.call("initialize", params, within)?;
        let v: Value = serde_json::from_str(res.get()).unwrap_or(Value::Null);
        self.notify("initialized")?;
        Ok(version_of(crate::json::s(&v, "userAgent")))
    }

    /// Ends the app-server: stdin first, then its process group.
    pub fn close(&self) {
        match self.w.try_lock() {
            Ok(mut w) => drop(w.take()),
            Err(std::sync::TryLockError::Poisoned(e)) => drop(e.into_inner().take()),
            // A write stuck on an app-server that reads no more: its
            // signals below end the write, as Go closing stdin under it
            // would.
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let Some((mut child, guard)) = lock(&self.proc).take() else {
            return;
        };
        let pid = self.pid;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            child.wait();
            if let Some(g) = guard {
                g.release();
            }
            let _ = tx.send(());
        });
        if rx.recv_timeout(Duration::from_secs(1)).is_ok() {
            return;
        }
        // signalling the process group we started.
        let _ = rush_sys::proc::kill_group(pid, libc::SIGTERM);
        if rx.recv_timeout(Duration::from_secs(2)).is_err() {
            // as above.
            let _ = rush_sys::proc::kill_group(pid, libc::SIGKILL);
            let _ = rx.recv();
        }
    }
}

/// Starts `codex app-server` in its own process group, with home as
/// CODEX_HOME when it is set.
pub(crate) fn spawn(
    binary: &str,
    home: &str,
    env: &[String],
    flags: &[String],
    handle: Option<Handler>,
) -> adapter::Result<Arc<Client>> {
    let binary = if binary.is_empty() {
        program()
    } else {
        binary.to_string()
    };
    let mut cmd = Command::new(&binary);
    cmd.arg("app-server").args(flags);
    for kv in env {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    if !home.is_empty() {
        cmd.env("CODEX_HOME", home);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = cmd.spawn()?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("codex: app-server's pipes".into());
    };
    let pid = child.id() as i32;
    let guard = rush_sys::pgguard::watch(pid);
    let s = Setup {
        proc: Some((Proc::Std(child), guard)),
        pid,
        ..Setup::default()
    };
    Client::on_fds(OwnedFd::from(stdin), OwnedFd::from(stdout), handle, s)
}

/// The codex program: on PATH, else where the registry found it.
pub(crate) fn program() -> String {
    if !on_path(KIND) {
        let p = adapter::global().path(KIND);
        if !p.is_empty() {
            return p;
        }
    }
    KIND.to_string()
}

fn on_path(name: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| {
            !d.is_empty() && std::path::Path::new(d).is_absolute() && {
                let p = std::path::Path::new(d).join(name);
                p.is_file()
            }
        })
}

/// The version in a user agent such as "rush/0.155.1 (Mac OS 27.0.0;
/// arm64) …": Codex puts its own version after the client's name.
pub(crate) fn version_of(ua: &str) -> String {
    ua.split_whitespace()
        .next()
        .and_then(|f| f.split_once('/'))
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

/// A request id as rush keys it: a string's text, else the JSON as it is.
pub(crate) fn id_string(id: &RawValue) -> String {
    let s = id.get();
    match serde_json::from_str::<String>(s) {
        Ok(u) if s.starts_with('"') => u,
        _ => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_ids() {
        assert_eq!(
            version_of("rush/0.155.1 (Mac OS 27.0.0; arm64) test"),
            "0.155.1"
        );
        assert_eq!(version_of("rush"), "");
        assert_eq!(version_of(""), "");
        let r = |s: &str| RawValue::from_string(s.into()).unwrap();
        assert_eq!(id_string(&r(r#""req-7""#)), "req-7");
        assert_eq!(id_string(&r("8")), "8");
    }
}
