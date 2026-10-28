//! The plugin wire protocol: JSON-RPC 2.0, either side calling the other.
//! rush's own plugins speak it in frames on fd 3 (a 4-byte big-endian
//! length, then that many bytes of JSON); an MCP plugin speaks it one
//! message a line on stdio. Calls run concurrently and replies are matched
//! to calls by id; notifications are taken in order.
//! Ported from internal/plugin/rpc.go.
//!
//! Each connection has one thread reading it, blocked on the socket until
//! a message comes; each request it's sent is answered on a thread of its
//! own, as Go answers each in a goroutine.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use crate::ctx::{Ctx, CtxErr};

/// Bounds one message. A tool result bigger than this is a bug, and
/// refusing it keeps a plugin from making rush allocate without limit.
pub const MAX_FRAME: usize = 16 << 20;

/// A JSON value as it was written: jsontext.Value.
pub type Raw = Box<RawValue>;

/// A JSON-RPC error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn new(code: i64, message: impl Into<String>) -> Error {
        Error {
            code,
            message: message.into(),
        }
    }

    /// Any other error, as a handler's error that isn't an *Error is sent.
    pub fn server(message: impl fmt::Display) -> Error {
        Error::new(CODE_SERVER, message.to_string())
    }
}

// JSON-RPC error codes.
pub const CODE_PARSE: i64 = -32700;
pub const CODE_INVALID_REQUEST: i64 = -32600;
pub const CODE_NO_METHOD: i64 = -32601;
pub const CODE_INVALID_PARAMS: i64 = -32602;
pub const CODE_SERVER: i64 = -32000;
/// the plugin lacks the capability
pub const CODE_DENIED: i64 = -32001;
/// allowed, but not so often: try again later
pub const CODE_LIMITED: i64 = -32002;

/// The error for a call made too often: it may go again later.
pub fn limited(what: &str) -> Error {
    Error::new(CODE_LIMITED, format!("too often: {what}"))
}

/// The error for a call the plugin was not approved to make.
pub fn denied(what: &str) -> Error {
    Error::new(CODE_DENIED, format!("not permitted: {what}"))
}

/// The error for a method nobody answers.
pub fn no_method(method: &str) -> Error {
    Error::new(CODE_NO_METHOD, format!("method not found: {method}"))
}

/// Why a call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum CallError {
    /// The other side answered with an error.
    Rpc(Error),
    /// The connection closed before it was answered: Go's ErrClosed.
    Closed,
    /// Whoever asked gave up, or ran out of time.
    Ctx(CtxErr),
    /// The call couldn't be written or its answer read.
    Other(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Rpc(e) => f.write_str(&e.message),
            CallError::Closed => f.write_str("connection closed"),
            CallError::Ctx(e) => e.fmt(f),
            CallError::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for CallError {}

impl CallError {
    /// The JSON-RPC error it carries, if the other side sent one.
    pub fn rpc(&self) -> Option<&Error> {
        match self {
            CallError::Rpc(e) => Some(e),
            _ => None,
        }
    }
}

/// Answers a request, or takes a notification (whose answer is dropped):
/// given the connection it came on, its Ctx (done when the connection
/// closes), the method and its params as sent (None when there were none).
/// Ok(None) answers null.
pub type Handler =
    Arc<dyn Fn(&Conn, &Ctx, &str, Option<&RawValue>) -> Result<Option<Raw>, Error> + Send + Sync>;

/// v as an answer: JSON as Go rush writes it.
pub fn to_raw<T: Serialize + ?Sized>(v: &T) -> Result<Raw, Error> {
    let b = rush_state::json::to_vec(v).map_err(Error::server)?;
    let s = String::from_utf8(b).map_err(Error::server)?;
    RawValue::from_string(s).map_err(Error::server)
}

/// Reads params (or a result) as Go's jsonx does: None reads as null.
pub fn from_raw<T: DeserializeOwned>(v: Option<&RawValue>) -> Result<T, String> {
    let s = v.map_or("null", |v| v.get());
    match serde_json::from_str(s) {
        Ok(t) => Ok(t),
        Err(e) => rush_state::json::from_slice(s.as_bytes()).map_err(|_| format!("json: {e}")),
    }
}

/// One message, either way.
#[derive(Serialize, Deserialize, Default)]
struct Message {
    #[serde(default)]
    jsonrpc: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    id: Option<Raw>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    method: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    params: Option<Raw>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    result: Option<Raw>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<Error>,
}

/// A member that's there, even as null, is Some: as a jsontext.Value is.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Raw>, D::Error> {
    Raw::deserialize(d).map(Some)
}

fn parse_message(b: &[u8]) -> Option<Message> {
    if let Ok(m) = serde_json::from_slice(b) {
        return Some(m);
    }
    // A string cut mid-character still reads, as jsonx allows.
    serde_json::from_str(&String::from_utf8_lossy(b)).ok()
}

trait ReadMsg: Send {
    fn read_msg(&mut self) -> io::Result<Vec<u8>>;
}

trait WriteMsg: Send {
    fn write_msg(&mut self, b: &[u8]) -> io::Result<()>;
}

/// A 4-byte big-endian length, then that many bytes of JSON: a message is
/// read in two reads with no scanning, whatever it holds.
struct FramedReader<R>(BufReader<R>);
struct FramedWriter<W>(W);

impl<R: Read + Send> ReadMsg for FramedReader<R> {
    fn read_msg(&mut self) -> io::Result<Vec<u8>> {
        let mut hdr = [0u8; 4];
        read_full(&mut self.0, &mut hdr, true)?;
        let n = u32::from_be_bytes(hdr) as usize;
        if n > MAX_FRAME {
            return Err(io::Error::other(format!(
                "frame of {n} bytes is over the {MAX_FRAME} limit"
            )));
        }
        let mut b = vec![0u8; n];
        read_full(&mut self.0, &mut b, false)?;
        Ok(b)
    }
}

/// io.ReadFull: "EOF" when nothing came, "unexpected EOF" when part did.
fn read_full(r: &mut impl Read, buf: &mut [u8], first: bool) -> io::Result<()> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) => {
                let what = if got == 0 && first {
                    "EOF"
                } else {
                    "unexpected EOF"
                };
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, what));
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

impl<W: Write + Send> WriteMsg for FramedWriter<W> {
    fn write_msg(&mut self, b: &[u8]) -> io::Result<()> {
        if b.len() > MAX_FRAME {
            return Err(io::Error::other(format!(
                "frame of {} bytes is over the {MAX_FRAME} limit",
                b.len()
            )));
        }
        let mut buf = Vec::with_capacity(4 + b.len());
        buf.extend_from_slice(&(b.len() as u32).to_be_bytes());
        buf.extend_from_slice(b);
        self.0.write_all(&buf)?;
        self.0.flush()
    }
}

/// One JSON message per line: MCP's stdio transport, so any MCP server
/// can be a plugin as it is.
struct LineReader<R>(BufReader<R>);
struct LineWriter<W>(W);

impl<R: Read + Send> ReadMsg for LineReader<R> {
    fn read_msg(&mut self) -> io::Result<Vec<u8>> {
        let mut line = Vec::new();
        loop {
            let (done, used) = {
                let buf = match self.0.fill_buf() {
                    Ok(b) => b,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                };
                if buf.is_empty() {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"));
                }
                match buf.iter().position(|&c| c == b'\n') {
                    Some(i) => (true, i + 1),
                    None => (false, buf.len()),
                }
            };
            if line.len() + used > MAX_FRAME {
                return Err(io::Error::other(format!(
                    "line over the {MAX_FRAME} byte limit"
                )));
            }
            line.extend_from_slice(&self.0.buffer()[..used]);
            self.0.consume(used);
            if done {
                if line.iter().any(|c| !c.is_ascii_whitespace()) {
                    return Ok(line);
                }
                line.clear();
            }
        }
    }
}

impl<W: Write + Send> WriteMsg for LineWriter<W> {
    fn write_msg(&mut self, b: &[u8]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(b.len() + 1);
        buf.extend_from_slice(b);
        buf.push(b'\n');
        self.0.write_all(&buf)?;
        self.0.flush()
    }
}

enum Reply {
    Msg(Message),
    Closed,
    Canceled,
}

struct Pending {
    closed: bool, // no more replies will come
    map: HashMap<String, mpsc::Sender<Reply>>,
}

struct Done {
    done: bool,
    err: String,
}

struct Inner {
    writer: Mutex<Option<Box<dyn WriteMsg>>>,
    closer: Box<dyn Fn() + Send + Sync>,
    handler: Option<Handler>,
    next: AtomicI64,
    pending: Mutex<Pending>,
    done: Mutex<Done>,
    done_cv: Condvar,
    ctx: Ctx,
    cancel: crate::ctx::Cancel,
}

/// A JSON-RPC 2.0 connection. Either side may call the other; calls run
/// concurrently, and replies are matched to calls by id. Clones are the
/// same connection.
#[derive(Clone)]
pub struct Conn(Arc<Inner>);

impl Conn {
    /// Speaks framed JSON-RPC over a socket: fd 3's socket pair, or the
    /// broker's. h may be None: every request then gets "method not found".
    pub fn new(stream: UnixStream, h: Option<Handler>) -> io::Result<Conn> {
        let r = stream.try_clone()?;
        let closer = stream.try_clone()?;
        Ok(Conn::framed(
            r,
            stream,
            move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            },
            h,
        ))
    }

    /// Speaks framed JSON-RPC, reading r and writing w; closer ends both,
    /// and must make a blocked read of r return.
    pub fn framed(
        r: impl Read + Send + 'static,
        w: impl Write + Send + 'static,
        closer: impl Fn() + Send + Sync + 'static,
        h: Option<Handler>,
    ) -> Conn {
        Conn::start(
            Box::new(FramedReader(BufReader::with_capacity(64 << 10, r))),
            Box::new(FramedWriter(w)),
            Box::new(closer),
            h,
        )
    }

    /// Speaks newline-delimited JSON-RPC (MCP's stdio transport), reading r
    /// and writing w. closer must make a blocked read of r return (see
    /// sys::wakeable); closing also drops w, which closes it.
    pub fn lines(
        r: impl Read + Send + 'static,
        w: impl Write + Send + 'static,
        closer: impl Fn() + Send + Sync + 'static,
        h: Option<Handler>,
    ) -> Conn {
        Conn::start(
            Box::new(LineReader(BufReader::with_capacity(64 << 10, r))),
            Box::new(LineWriter(w)),
            Box::new(closer),
            h,
        )
    }

    fn start(
        mut reader: Box<dyn ReadMsg>,
        writer: Box<dyn WriteMsg>,
        closer: Box<dyn Fn() + Send + Sync>,
        h: Option<Handler>,
    ) -> Conn {
        let (ctx, cancel) = Ctx::background().with_cancel();
        let conn = Conn(Arc::new(Inner {
            writer: Mutex::new(Some(writer)),
            closer,
            handler: h,
            next: AtomicI64::new(0),
            pending: Mutex::new(Pending {
                closed: false,
                map: HashMap::new(),
            }),
            done: Mutex::new(Done {
                done: false,
                err: String::new(),
            }),
            done_cv: Condvar::new(),
            ctx,
            cancel,
        }));
        let c = conn.clone();
        std::thread::Builder::new()
            .name("plugin-rpc".into())
            .spawn(move || c.read_loop(&mut *reader))
            .expect("starting a thread");
        conn
    }

    /// Done when the connection has closed: handlers are given it.
    pub fn ctx(&self) -> &Ctx {
        &self.0.ctx
    }

    /// Whether the connection has closed.
    pub fn is_done(&self) -> bool {
        lock(&self.0.done).done
    }

    /// Blocks until the connection has closed.
    pub fn wait(&self) {
        let mut d = lock(&self.0.done);
        while !d.done {
            d = self.0.done_cv.wait(d).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Why the connection closed, once it has: "EOF" when the other side
    /// went.
    pub fn err(&self) -> String {
        self.wait();
        lock(&self.0.done).err.clone()
    }

    /// Ends the connection; calls waiting on it fail.
    pub fn close(&self) {
        (self.0.closer)();
        lock(&self.0.writer).take();
    }

    fn read_loop(&self, reader: &mut dyn ReadMsg) {
        let err = loop {
            let b = match reader.read_msg() {
                Ok(b) => b,
                Err(e) => {
                    self.close();
                    break e.to_string();
                }
            };
            let Some(m) = parse_message(&b) else {
                let _ = self.send(Message {
                    error: Some(Error::new(CODE_PARSE, "parse error")),
                    ..Default::default()
                });
                continue;
            };
            match (m.method.is_empty(), m.id.is_some()) {
                (false, true) => {
                    let c = self.clone();
                    let _ = std::thread::Builder::new()
                        .name("plugin-rpc-call".into())
                        .spawn(move || c.serve(m));
                }
                (false, false) => {
                    // Notifications are taken in order, so a stream of
                    // events arrives as it was sent.
                    if let Some(h) = &self.0.handler {
                        let _ = h(self, &self.0.ctx, &m.method, m.params.as_deref());
                    }
                }
                (true, true) => {
                    let id =
                        m.id.as_ref()
                            .map(|i| i.get().to_string())
                            .unwrap_or_default();
                    let ch = lock(&self.0.pending).map.remove(&id);
                    if let Some(ch) = ch {
                        let _ = ch.send(Reply::Msg(m));
                    }
                }
                (true, false) => {}
            }
        };
        self.0.cancel.cancel();
        let waiting = {
            let mut p = lock(&self.0.pending);
            p.closed = true;
            std::mem::take(&mut p.map)
        };
        for (_, ch) in waiting {
            let _ = ch.send(Reply::Closed);
        }
        let mut d = lock(&self.0.done);
        d.done = true;
        d.err = err;
        self.0.done_cv.notify_all();
    }

    fn serve(&self, m: Message) {
        let mut out = Message {
            id: m.id,
            ..Default::default()
        };
        match &self.0.handler {
            None => out.error = Some(no_method(&m.method)),
            Some(h) => match h(self, &self.0.ctx, &m.method, m.params.as_deref()) {
                Ok(res) => out.result = Some(res.unwrap_or_else(null)),
                Err(e) => out.error = Some(e),
            },
        }
        let _ = self.send(out);
    }

    fn send(&self, mut m: Message) -> Result<(), String> {
        m.jsonrpc = "2.0".into();
        let b = rush_state::json::to_vec(&m).map_err(|e| e.to_string())?;
        let mut w = lock(&self.0.writer);
        match w.as_mut() {
            Some(w) => w.write_msg(&b).map_err(|e| e.to_string()),
            None => Err("use of closed network connection".into()),
        }
    }

    /// Calls method with params already encoded (None sends none), and
    /// returns the raw result (None when the answer had none).
    pub fn call_raw(
        &self,
        ctx: &Ctx,
        method: &str,
        params: Option<Raw>,
    ) -> Result<Option<Raw>, CallError> {
        let id = (self.0.next.fetch_add(1, Ordering::Relaxed) + 1).to_string();
        let (tx, rx) = mpsc::channel();
        {
            let mut p = lock(&self.0.pending);
            if p.closed {
                return Err(CallError::Closed);
            }
            p.map.insert(id.clone(), tx.clone());
        }
        let forget = || {
            lock(&self.0.pending).map.remove(&id);
        };
        let _reg = ctx.on_cancel(move || {
            let _ = tx.send(Reply::Canceled);
        });
        let msg = Message {
            id: RawValue::from_string(id.clone()).ok(),
            method: method.to_string(),
            params,
            ..Default::default()
        };
        if let Err(e) = self.send(msg) {
            forget();
            return Err(CallError::Other(e));
        }
        match ctx.recv(&rx) {
            Ok(Reply::Msg(m)) => match m.error {
                Some(e) => Err(CallError::Rpc(e)),
                None => Ok(m.result),
            },
            Ok(Reply::Closed) => Err(CallError::Closed),
            Ok(Reply::Canceled) => {
                forget();
                Err(CallError::Ctx(ctx.err().unwrap_or(CtxErr::Canceled)))
            }
            Err(e) => {
                forget();
                Err(CallError::Ctx(e))
            }
        }
    }

    /// Calls method with params and decodes the result. Params that encode
    /// as null are sent as none, as Go's nil.
    pub fn call<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        ctx: &Ctx,
        method: &str,
        params: &P,
    ) -> Result<R, CallError> {
        let p = encode_params(params)?;
        let res = self.call_raw(ctx, method, p)?;
        from_raw(res.as_deref()).map_err(CallError::Other)
    }

    /// Sends a notification, which has no reply.
    pub fn notify<P: Serialize + ?Sized>(&self, method: &str, params: &P) -> Result<(), String> {
        let p = encode_params(params).map_err(|e| e.to_string())?;
        self.notify_raw(method, p)
    }

    /// Sends a notification with params already encoded.
    pub fn notify_raw(&self, method: &str, params: Option<Raw>) -> Result<(), String> {
        self.send(Message {
            method: method.to_string(),
            params,
            ..Default::default()
        })
    }
}

fn encode_params<P: Serialize + ?Sized>(params: &P) -> Result<Option<Raw>, CallError> {
    let raw = to_raw(params).map_err(|e| CallError::Other(e.message))?;
    Ok((raw.get() != "null").then_some(raw))
}

fn null() -> Raw {
    RawValue::from_string("null".into()).expect("null is JSON")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn pair(a: Option<Handler>, b: Option<Handler>) -> (Conn, Conn) {
        let (x, y) = UnixStream::pair().unwrap();
        (Conn::new(x, a).unwrap(), Conn::new(y, b).unwrap())
    }

    fn echo() -> Handler {
        Arc::new(|_, _, method, params| match method {
            "echo" => Ok(params.map(|p| p.to_owned())),
            "fail" => Err(Error::new(7, "no")),
            "boom" => Err(Error::server("broke")),
            _ => Err(no_method(method)),
        })
    }

    #[test]
    fn calls_both_ways() {
        let (a, b) = pair(Some(echo()), Some(echo()));
        let ctx = Ctx::background();
        let got: HashMap<String, i64> = a.call(&ctx, "echo", &HashMap::from([("n", 1)])).unwrap();
        assert_eq!(got["n"], 1);
        let got: HashMap<String, i64> = b.call(&ctx, "echo", &HashMap::from([("n", 2)])).unwrap();
        assert_eq!(got["n"], 2);
        let e = a
            .call::<_, serde_json::Value>(&ctx, "fail", &())
            .unwrap_err();
        assert_eq!(e.rpc().map(|e| e.code), Some(7));
        let e = a
            .call::<_, serde_json::Value>(&ctx, "boom", &())
            .unwrap_err();
        assert_eq!(e.rpc(), Some(&Error::new(CODE_SERVER, "broke")));
        let e = a
            .call::<_, serde_json::Value>(&ctx, "nope", &())
            .unwrap_err();
        assert_eq!(e.rpc().map(|e| e.code), Some(CODE_NO_METHOD));
        // Without a handler, every request is "method not found".
        let (c, _d) = pair(None, None);
        let e = c.call::<_, serde_json::Value>(&ctx, "x", &()).unwrap_err();
        assert_eq!(e.to_string(), "method not found: x");
    }

    #[test]
    fn concurrent_calls_match_their_replies() {
        // Each answer waits for the next call's to have gone, so later calls
        // answer first, with no sleeping.
        let gates: Arc<Vec<(Mutex<bool>, Condvar)>> = Arc::new(
            (0..21)
                .map(|_| (Mutex::new(false), Condvar::new()))
                .collect(),
        );
        let g = gates.clone();
        let slow: Handler = Arc::new(move |_, _, _, params| {
            let n: usize = from_raw(params).unwrap();
            let (m, cv) = &g[n + 1];
            let mut open = m.lock().unwrap();
            while !*open && n < 19 {
                open = cv.wait(open).unwrap();
            }
            drop(open);
            let (m, cv) = &g[n];
            *m.lock().unwrap() = true;
            cv.notify_all();
            to_raw(&n).map(Some)
        });
        let (a, _b) = pair(None, Some(slow));
        let ts: Vec<_> = (0..20usize)
            .map(|i| {
                let a = a.clone();
                std::thread::spawn(move || {
                    let got: usize = a.call(&Ctx::background(), "n", &i).unwrap();
                    assert_eq!(got, i);
                })
            })
            .collect();
        for t in ts {
            t.join().unwrap();
        }
    }

    #[test]
    fn notifications_arrive_in_order() {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let h: Handler = Arc::new(move |_, _, _, params| {
            let n: i64 = from_raw(params).unwrap();
            tx.lock().unwrap().send(n).unwrap();
            Ok(None)
        });
        let (a, _b) = pair(None, Some(h));
        for i in 0..100 {
            a.notify("ev", &i).unwrap();
        }
        for i in 0..100 {
            assert_eq!(rx.recv().unwrap(), i);
        }
    }

    #[test]
    fn call_fails_when_the_other_side_goes() {
        let (started_tx, started_rx) = mpsc::channel();
        let started_tx = Mutex::new(started_tx);
        let block: Handler = Arc::new(move |_, ctx, _, _| {
            started_tx.lock().unwrap().send(()).unwrap();
            ctx.wait();
            Ok(None)
        });
        let (a, b) = pair(None, Some(block));
        let a2 = a.clone();
        let t = std::thread::spawn(move || {
            a2.call::<_, serde_json::Value>(&Ctx::background(), "wait", &())
        });
        started_rx.recv().unwrap();
        b.close();
        assert_eq!(t.join().unwrap().unwrap_err(), CallError::Closed);
        a.wait();
        assert!(
            a.call::<_, serde_json::Value>(&Ctx::background(), "again", &())
                .is_err()
        );
    }

    #[test]
    fn a_call_gives_up_with_its_ctx() {
        let block: Handler = Arc::new(|_, ctx, _, _| {
            ctx.wait();
            Ok(None)
        });
        let (a, _b) = pair(None, Some(block));
        let ctx = Ctx::background().with_timeout(Duration::from_millis(20));
        let e = a
            .call::<_, serde_json::Value>(&ctx, "wait", &())
            .unwrap_err();
        assert_eq!(e.to_string(), "context deadline exceeded");
        let (ctx, cancel) = Ctx::background().with_cancel();
        cancel.cancel();
        let e = a
            .call::<_, serde_json::Value>(&ctx, "wait", &())
            .unwrap_err();
        assert_eq!(e.to_string(), "context canceled");
        a.close();
    }

    #[test]
    fn oversized_frame_closes_the_connection() {
        let (x, mut y) = UnixStream::pair().unwrap();
        let c = Conn::new(x, None).unwrap();
        y.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
        c.wait();
        assert!(c.err().contains("over"), "{}", c.err());
    }

    #[test]
    fn wire_bytes_are_gos() {
        // What rush writes, byte for byte, and that a reply's id and a
        // request's "params": null are kept as written.
        let (x, y) = UnixStream::pair().unwrap();
        let h: Handler = Arc::new(|_, _, m, p| {
            assert_eq!(p.map(|p| p.get()), Some("null"));
            to_raw(&serde_json::json!({"m": m})).map(Some)
        });
        let _c = Conn::new(x, Some(h)).unwrap();
        let mut y2 = y.try_clone().unwrap();
        let req = br#"{"jsonrpc":"2.0","id":"a-1","method":"tools.list","params":null}"#;
        y2.write_all(&(req.len() as u32).to_be_bytes()).unwrap();
        y2.write_all(req).unwrap();
        let mut r = FramedReader(BufReader::new(y));
        let got = r.read_msg().unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            r#"{"jsonrpc":"2.0","id":"a-1","result":{"m":"tools.list"}}"#
        );
        // A message that isn't JSON gets a parse error with no id.
        let bad = b"{nope";
        y2.write_all(&(bad.len() as u32).to_be_bytes()).unwrap();
        y2.write_all(bad).unwrap();
        assert_eq!(
            String::from_utf8(r.read_msg().unwrap()).unwrap(),
            r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"parse error"}}"#
        );
    }

    #[test]
    fn line_conn_speaks_mcp_stdio() {
        let (s_in, c_out) = UnixStream::pair().unwrap();
        let (s_out, c_in) = UnixStream::pair().unwrap();
        let h: Handler = Arc::new(|_, _, m, _| to_raw(&HashMap::from([("method", m)])).map(Some));
        let sc = s_in.try_clone().unwrap();
        let server = Conn::lines(
            s_in,
            s_out,
            move || {
                let _ = sc.shutdown(std::net::Shutdown::Both);
            },
            Some(h),
        );
        let cc = c_in.try_clone().unwrap();
        let client = Conn::lines(
            c_in,
            c_out,
            move || {
                let _ = cc.shutdown(std::net::Shutdown::Both);
            },
            None,
        );
        let got: HashMap<String, String> =
            client.call(&Ctx::background(), "tools/list", &()).unwrap();
        assert_eq!(got["method"], "tools/list");
        // A long line crosses the reader's buffer several times.
        let long = "x".repeat(300 << 10);
        let got: HashMap<String, String> = client.call(&Ctx::background(), &long, &()).unwrap();
        assert_eq!(got["method"], long);
        server.close();
        client.close();
    }
}
