//! Talking to one `codex app-server` over newline-delimited JSON-RPC 2.0
//! without its "jsonrpc" field. Port of internal/adapters/codex/rpc.go:
//! 1-277 (message, client, spawn, call, notify, reply, refuse,
//! initialize, versionOf, close, idString).
//!
//! A request has an id and a method, a notification only a method, a
//! response an id and a result or an error. The client's own reader
//! thread hands every notification and server request to its handler, in
//! order; the handler must never wait on a call.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
}

/// One app-server's connection.
pub(crate) struct Client {
    w: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<Pending>,
    proc: Mutex<Option<(Child, Option<rush_sys::pgguard::Guard>)>>,
    pid: i32,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn null() -> Box<RawValue> {
    RawValue::from_string("null".into()).expect("null is JSON")
}

impl Client {
    /// A client reading r and writing w, its reader thread started.
    #[cfg(test)]
    pub fn new(
        r: impl Read + Send + 'static,
        w: impl Write + Send + 'static,
        handle: Option<Handler>,
    ) -> Arc<Client> {
        Client::with_proc(r, w, handle, None)
    }

    fn with_proc(
        r: impl Read + Send + 'static,
        w: impl Write + Send + 'static,
        handle: Option<Handler>,
        proc: Option<(Child, Option<rush_sys::pgguard::Guard>)>,
    ) -> Arc<Client> {
        let pid = proc.as_ref().map_or(0, |(c, _)| c.id() as i32);
        let c = Arc::new(Client {
            w: Mutex::new(Some(Box::new(w))),
            pending: Mutex::new(Pending {
                next: 0,
                waiting: HashMap::new(),
                closed: false,
            }),
            proc: Mutex::new(proc),
            pid,
        });
        let c2 = c.clone();
        std::thread::Builder::new()
            .name("codex-rpc".into())
            .spawn(move || c2.read(r, handle))
            .expect("a thread");
        c
    }

    /// The app-server's process, when it is one of rush's.
    pub fn pid(&self) -> Option<i32> {
        (self.pid > 0).then_some(self.pid)
    }

    fn read(&self, r: impl Read, handle: Option<Handler>) {
        let mut r = BufReader::with_capacity(64 << 10, r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match (&mut r).take(MAX_LINE + 1).read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if buf.len() as u64 > MAX_LINE {
                break; // as Go's scanner: a line too long ends the reading
            }
            let Ok(m) = serde_json::from_slice::<Line>(&buf) else {
                continue;
            };
            let id = m.id.filter(|i| i.get() != "null");
            if m.method.is_empty() && id.is_some() {
                let n = id.as_ref().and_then(|i| i.get().parse::<i64>().ok());
                let ch = n.and_then(|n| lock(&self.pending).waiting.remove(&n));
                if let Some(ch) = ch {
                    let reply = match m.error {
                        Some(e) => Err(RpcError {
                            code: e.code,
                            message: e.message,
                        }),
                        None => Ok(m.result.unwrap_or_else(null)),
                    };
                    let _ = ch.send(reply);
                }
                continue;
            }
            if let Some(h) = &handle {
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
        {
            let mut p = lock(&self.pending);
            p.closed = true;
            p.waiting.clear(); // each caller is told it's closed
        }
        if let Some(h) = &handle {
            h(self, Incoming::Closed);
        }
    }

    fn write(&self, v: &impl serde::Serialize) -> adapter::Result<()> {
        let mut b = serde_json::to_vec(v)?;
        b.push(b'\n');
        let mut w = lock(&self.w);
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
            lock(&self.pending).waiting.remove(&id);
            return Err(e);
        }
        let got = match within {
            Some(d) => rx.recv_timeout(d).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => {
                    lock(&self.pending).waiting.remove(&id);
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
        lock(&self.w).take();
        let Some((mut child, guard)) = lock(&self.proc).take() else {
            return;
        };
        let pid = child.id() as i32;
        let (tx, rx) = mpsc::channel();
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
        // SAFETY: signalling the process group we started.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        if rx.recv_timeout(Duration::from_secs(2)).is_err() {
            // SAFETY: as above.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
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
    let guard = rush_sys::pgguard::watch(child.id() as i32);
    Ok(Client::with_proc(
        stdout,
        stdin,
        handle,
        Some((child, guard)),
    ))
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
