//! JSON-RPC 2.0 over newline-delimited JSON, both ways: calls to the
//! agent, and the agent's own calls and notifications, handed on in the
//! order they arrive. Port of internal/adapters/acp/rpc.go:1-226.
//!
//! Go waits for each answer on a channel per call. Here a call leaves a
//! callback that the reader thread runs with the answer, so a turn waiting
//! on its prompt holds no thread of its own; `call` waits on a channel
//! when the caller wants to block.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// JSON-RPC error codes ACP uses.
pub const CODE_METHOD_NOT_FOUND: i64 = -32601;
pub const CODE_INVALID_PARAMS: i64 = -32602;
pub const CODE_INTERNAL: i64 = -32603;
pub const CODE_AUTH_REQUIRED: i64 = -32000;
pub const CODE_NOT_FOUND: i64 = -32002;
pub const CODE_CANCELLED: i64 = -32800;

/// A JSON-RPC error, from the agent or for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<rush_core::wire::RawJson>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// What went wrong, as Go's errors say it: `acp: <message> (<code>)` for
/// the agent's own, `acp: connection closed` once it's gone, and the rest
/// with what they were about in front, kept findable by what they are.
#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
    text: String,
}

/// What an [`Error`] is, however much was put in front of it.
#[derive(Clone, Debug, PartialEq)]
pub enum ErrorKind {
    /// The agent's own answer.
    Rpc(RpcError),
    /// The connection has ended (Go's ErrClosed).
    Closed,
    /// The agent can't do it (Go's ErrUnsupported).
    Unsupported,
    /// A fork asked of an agent ACP can't fork (Go's ErrNoFork).
    NoFork,
    /// Waited too long for the agent.
    Timeout,
    Other,
}

impl Error {
    pub fn closed() -> Error {
        Error {
            kind: ErrorKind::Closed,
            text: "acp: connection closed".into(),
        }
    }
    pub fn unsupported() -> Error {
        Error {
            kind: ErrorKind::Unsupported,
            text: "acp: the agent doesn't support this".into(),
        }
    }
    pub fn no_fork() -> Error {
        Error {
            kind: ErrorKind::NoFork,
            text: "acp: this agent can't fork a session".into(),
        }
    }
    pub fn timeout() -> Error {
        Error {
            kind: ErrorKind::Timeout,
            text: "context deadline exceeded".into(),
        }
    }
    pub fn other(text: impl Into<String>) -> Error {
        Error {
            kind: ErrorKind::Other,
            text: text.into(),
        }
    }
    pub fn rpc(e: RpcError) -> Error {
        Error {
            text: format!("acp: {} ({})", e.message, e.code),
            kind: ErrorKind::Rpc(e),
        }
    }
    /// The same error with what it was about in front: "acp: resume: …".
    pub fn wrap(mut self, ctx: &str) -> Error {
        self.text = format!("{ctx}: {}", self.text);
        self
    }
    /// The same error with more said after it.
    pub fn suffix(mut self, more: &str) -> Error {
        self.text = format!("{}: {more}", self.text);
        self
    }
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
    pub fn is_unsupported(&self) -> bool {
        self.kind == ErrorKind::Unsupported
    }
    /// The agent's own error, when it is one.
    pub fn rpc_error(&self) -> Option<&RpcError> {
        match &self.kind {
            ErrorKind::Rpc(e) => Some(e),
            _ => None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::other(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::other(e.to_string())
    }
}

/// One JSON-RPC message as it's written.
#[derive(Serialize)]
struct WireOut<'a> {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "str::is_empty")]
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a RpcError>,
}

/// One JSON-RPC message as it's read, whichever kind it is.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct Wire {
    pub id: Option<Box<RawValue>>,
    pub method: String,
    pub params: Option<Box<RawValue>>,
    pub result: Option<Box<RawValue>>,
    pub error: Option<RpcError>,
}

/// What a call's answer comes to: its result (None when it had none), or
/// why there was none.
pub(crate) type Answer = Result<Option<Box<RawValue>>, Error>;

/// Runs with a call's answer, on the reader thread (or the caller's, when
/// the connection has already ended).
pub(crate) type Done = Box<dyn FnOnce(Answer) + Send>;

/// What the agent's own messages go to.
pub(crate) trait Handler: Send + Sync {
    /// A notification.
    fn notified(&self, method: &str, params: Option<&RawValue>);
    /// A request, to be answered with reply, now or later.
    fn served(&self, id: Box<RawValue>, method: &str, params: Option<&RawValue>);
}

/// A JSON-RPC connection: the writer to the agent, and the calls waiting
/// for their answers.
pub(crate) struct Rpc {
    w: Mutex<Option<Box<dyn Write + Send>>>,
    st: Mutex<State>,
}

struct State {
    next: i64,
    pending: HashMap<i64, Done>,
    /// Why the connection ended, once it has.
    err: Option<Error>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// v as JSON, for params and results.
pub(crate) fn raw(v: &serde_json::Value) -> Box<RawValue> {
    RawValue::from_string(v.to_string()).expect("a Value is JSON")
}

impl Rpc {
    pub fn new(w: Box<dyn Write + Send>) -> Rpc {
        Rpc {
            w: Mutex::new(Some(w)),
            st: Mutex::new(State {
                next: 0,
                pending: HashMap::new(),
                err: None,
            }),
        }
    }

    /// Reads r until it ends, handing the agent's messages to h, then
    /// fails every call still waiting.
    pub fn read(&self, r: impl Read, h: &dyn Handler) {
        let mut br = BufReader::with_capacity(64 << 10, r);
        let mut line = Vec::new();
        let err = loop {
            line.clear();
            match br.read_until(b'\n', &mut line) {
                Ok(0) => break Error::closed(),
                Ok(_) => {
                    let t = line.trim_ascii();
                    if !t.is_empty() {
                        self.dispatch(t, h);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => break Error::from(e),
            }
        };
        let pending = {
            let mut st = lock(&self.st);
            st.err = Some(err.clone());
            std::mem::take(&mut st.pending)
        };
        for (_, done) in pending {
            done(Err(err.clone()));
        }
    }

    fn dispatch(&self, line: &[u8], h: &dyn Handler) {
        let Ok(m) = serde_json::from_slice::<Wire>(line) else {
            return; // not ours: some agents log to stdout
        };
        match (m.method.is_empty(), m.id) {
            (false, Some(id)) => h.served(id, &m.method, m.params.as_deref()),
            (false, None) => h.notified(&m.method, m.params.as_deref()),
            (true, id) => {
                let Some(id) = id.and_then(|id| id.get().trim_matches('"').parse::<i64>().ok())
                else {
                    return;
                };
                let done = lock(&self.st).pending.remove(&id);
                if let Some(done) = done {
                    done(match m.error {
                        Some(e) => Err(Error::rpc(e)),
                        None => Ok(m.result),
                    });
                }
            }
        }
    }

    fn write(&self, m: WireOut) -> Result<(), Error> {
        let mut b = serde_json::to_vec(&m)?;
        b.push(b'\n');
        let mut w = lock(&self.w);
        let Some(w) = w.as_mut() else {
            return Err(Error::closed());
        };
        w.write_all(&b)?;
        w.flush()?;
        Ok(())
    }

    /// Closes the writer: the agent reads the end of its input.
    pub fn close(&self) {
        lock(&self.w).take();
    }

    /// Asks the agent method with params; done has the answer.
    pub fn call_with(&self, method: &str, params: Option<&serde_json::Value>, done: Done) -> i64 {
        let p = params.map(raw);
        let id = {
            let mut st = lock(&self.st);
            if let Some(e) = &st.err {
                let e = e.clone();
                drop(st);
                done(Err(e));
                return 0;
            }
            st.next += 1;
            let id = st.next;
            st.pending.insert(id, done);
            id
        };
        let idv = RawValue::from_string(id.to_string()).expect("a number is JSON");
        let sent = self.write(WireOut {
            jsonrpc: "2.0",
            id: Some(&idv),
            method,
            params: p.as_deref(),
            result: None,
            error: None,
        });
        if let Err(e) = sent
            && let Some(done) = lock(&self.st).pending.remove(&id)
        {
            done(Err(e));
        }
        id
    }

    /// Asks the agent method with params and waits for its answer, at most
    /// within when it's set: past that the call is forgotten, and the agent
    /// told to stop on it.
    pub fn call(
        &self,
        method: &str,
        params: Option<&serde_json::Value>,
        within: Option<Duration>,
    ) -> Answer {
        let (tx, rx) = mpsc::sync_channel(1);
        let id = self.call_with(
            method,
            params,
            Box::new(move |a| {
                let _ = tx.send(a);
            }),
        );
        let got = match within {
            Some(d) => rx.recv_timeout(d).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => None,
                mpsc::RecvTimeoutError::Disconnected => Some(Error::closed()),
            }),
            None => rx.recv().map_err(|_| Some(Error::closed())),
        };
        match got {
            Ok(a) => a,
            Err(Some(e)) => Err(e),
            Err(None) => {
                lock(&self.st).pending.remove(&id);
                let _ = self.send(
                    "$/cancel_request",
                    Some(&serde_json::json!({ "requestId": id })),
                );
                Err(Error::timeout())
            }
        }
    }

    /// Sends a notification.
    pub fn send(&self, method: &str, params: Option<&serde_json::Value>) -> Result<(), Error> {
        let p = params.map(raw);
        self.write(WireOut {
            jsonrpc: "2.0",
            id: None,
            method,
            params: p.as_deref(),
            result: None,
            error: None,
        })
    }

    /// Answers the agent's request id with result (an empty object when
    /// None), or with e.
    pub fn reply(
        &self,
        id: &RawValue,
        result: Option<&serde_json::Value>,
        e: Option<&RpcError>,
    ) -> Result<(), Error> {
        if let Some(e) = e {
            return self.write(WireOut {
                jsonrpc: "2.0",
                id: Some(id),
                method: "",
                params: None,
                result: None,
                error: Some(e),
            });
        }
        let r = result.map_or_else(|| raw(&serde_json::json!({})), raw);
        self.write(WireOut {
            jsonrpc: "2.0",
            id: Some(id),
            method: "",
            params: None,
            result: Some(&r),
            error: None,
        })
    }
}

/// result decoded as T; T's zero when there's none.
pub(crate) fn decode<T: serde::de::DeserializeOwned + Default>(
    result: Option<Box<RawValue>>,
) -> Result<T, Error> {
    match result {
        None => Ok(T::default()),
        Some(r) => Ok(serde_json::from_str::<Option<T>>(r.get())?.unwrap_or_default()),
    }
}
