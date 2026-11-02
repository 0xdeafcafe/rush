//! The connection to rush: JSON-RPC 2.0, either side calling the other.
//!
//! A rush plugin speaks it in frames on fd 3 (a 4-byte big-endian length,
//! then that many bytes of JSON); an MCP plugin one message a line on
//! stdin and stdout. One thread reads the connection. Each request rush
//! sends is answered on a thread of its own, so a slow tool never holds up
//! `tools.list`; notifications are taken in order, on the reading thread,
//! so a stream of events arrives as it was sent. A handler taking a
//! notification must not wait on a call to rush: the answer comes on the
//! thread it is holding up. Start a thread for that.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Bounds one message, as rush does: a bigger frame closes the connection.
pub const MAX_FRAME: usize = 16 << 20;

/// The fd rush gives a plugin its end of the connection on.
pub const IPC_FD: i32 = 3;

// JSON-RPC error codes, and rush's own.
pub const CODE_PARSE: i64 = -32700;
pub const CODE_INVALID_REQUEST: i64 = -32600;
pub const CODE_NO_METHOD: i64 = -32601;
pub const CODE_INVALID_PARAMS: i64 = -32602;
pub const CODE_SERVER: i64 = -32000;
/// The plugin wasn't approved for it.
pub const CODE_DENIED: i64 = -32001;
/// Allowed, but not so often: the same call may go later.
pub const CODE_LIMITED: i64 = -32002;

/// A JSON-RPC error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

impl Error {
    pub fn new(code: i64, message: impl Into<String>) -> Error {
        Error {
            code,
            message: message.into(),
        }
    }

    /// Something failed: -32000.
    pub fn server(message: impl fmt::Display) -> Error {
        Error::new(CODE_SERVER, message.to_string())
    }

    /// A method nobody answers: -32601.
    pub fn no_method(method: &str) -> Error {
        Error::new(CODE_NO_METHOD, format!("method not found: {method}"))
    }

    /// Params that don't read: -32602.
    pub fn invalid_params(message: impl fmt::Display) -> Error {
        Error::new(CODE_INVALID_PARAMS, message.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Why a call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum CallError {
    /// rush answered with an error: `-32001` when the plugin wasn't
    /// approved for it, `-32002` when it was too often.
    Rpc(Error),
    /// The connection closed before it was answered.
    Closed,
    /// No answer came in time; it names the method.
    Timeout(String),
    /// The call couldn't be written, or its answer read.
    Other(String),
}

impl CallError {
    /// The JSON-RPC error rush sent, if it sent one.
    pub fn rpc(&self) -> Option<&Error> {
        match self {
            CallError::Rpc(e) => Some(e),
            _ => None,
        }
    }

    /// Its JSON-RPC code, if rush sent one.
    pub fn code(&self) -> Option<i64> {
        self.rpc().map(|e| e.code)
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Rpc(e) => f.write_str(&e.message),
            CallError::Closed => f.write_str("connection closed"),
            CallError::Timeout(m) => write!(f, "{m}: rush didn't answer"),
            CallError::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for CallError {}

impl From<CallError> for Error {
    fn from(e: CallError) -> Error {
        match e {
            CallError::Rpc(e) => e,
            e => Error::server(e),
        }
    }
}

/// Answers a request, or takes a notification (whose answer is dropped):
/// given the connection it came on, the method and its params (null when
/// there were none). `Ok(Value::Null)` answers null.
pub type Handler = Arc<dyn Fn(&Conn, &str, Value) -> Result<Value, Error> + Send + Sync>;

/// Reads params as T, or says they're bad params.
pub fn params<T: DeserializeOwned>(v: Value) -> Result<T, Error> {
    serde_json::from_value(v).map_err(Error::invalid_params)
}

/// Connects to rush on fd 3 with h answering it, and runs until rush
/// closes the connection; then the process exits. Run from anything but
/// rush, it says so and exits 2.
pub fn serve<F>(h: F) -> !
where
    F: Fn(&Conn, &str, Value) -> Result<Value, Error> + Send + Sync + 'static,
{
    connect(h).wait();
    std::process::exit(0)
}

/// Connects to rush on fd 3 with h answering it, for a plugin with threads
/// of its own that call rush: hand them the connection, then
/// [`Conn::wait`]. Run from anything but rush, it says so and exits 2.
pub fn connect<F>(h: F) -> Conn
where
    F: Fn(&Conn, &str, Value) -> Result<Value, Error> + Send + Sync + 'static,
{
    match Conn::fd3(Arc::new(h)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("run me from rush: I talk on fd 3 ({e})");
            std::process::exit(2)
        }
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
    id: Option<Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<Error>,
}

/// A member that's there, even as null, is Some.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

trait ReadMsg: Send {
    fn read_msg(&mut self) -> io::Result<Vec<u8>>;
}

trait WriteMsg: Send {
    fn write_msg(&mut self, b: &[u8]) -> io::Result<()>;
}

struct FramedReader<R>(BufReader<R>);
struct FramedWriter<W>(W);

impl<R: Read + Send> ReadMsg for FramedReader<R> {
    fn read_msg(&mut self) -> io::Result<Vec<u8>> {
        let mut hdr = [0u8; 4];
        self.0.read_exact(&mut hdr)?;
        let n = u32::from_be_bytes(hdr) as usize;
        if n > MAX_FRAME {
            return Err(io::Error::other(format!(
                "frame of {n} bytes is over the {MAX_FRAME} limit"
            )));
        }
        let mut b = vec![0u8; n];
        self.0.read_exact(&mut b)?;
        Ok(b)
    }
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

struct LineReader<R>(BufReader<R>);
struct LineWriter<W>(W);

impl<R: Read + Send> ReadMsg for LineReader<R> {
    fn read_msg(&mut self) -> io::Result<Vec<u8>> {
        loop {
            let mut line = Vec::new();
            let n = self
                .0
                .by_ref()
                .take(MAX_FRAME as u64 + 1)
                .read_until(b'\n', &mut line)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"));
            }
            if line.len() > MAX_FRAME {
                return Err(io::Error::other(format!(
                    "line over the {MAX_FRAME} byte limit"
                )));
            }
            if line.iter().any(|c| !c.is_ascii_whitespace()) {
                return Ok(line);
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
}

struct Pending {
    closed: bool, // no more replies will come
    map: HashMap<String, mpsc::Sender<Reply>>,
}

struct Inner {
    writer: Mutex<Option<Box<dyn WriteMsg>>>,
    closer: Box<dyn Fn() + Send + Sync>,
    handler: Option<Handler>,
    next: AtomicI64,
    pending: Mutex<Pending>,
    /// Why it closed, once it has.
    done: Mutex<Option<String>>,
    done_cv: Condvar,
}

/// A JSON-RPC 2.0 connection. Either side may call the other; calls run
/// concurrently, and replies are matched to calls by id. Clones are the
/// same connection, to hand to threads of the plugin's own.
#[derive(Clone)]
pub struct Conn(Arc<Inner>);

impl Conn {
    /// The connection rush gave the plugin, on fd 3. An error when fd 3
    /// isn't a socket, as when it's run from a shell.
    pub fn fd3(h: Handler) -> io::Result<Conn> {
        use std::os::fd::FromRawFd;
        // Looked at (fstat: no path the sandbox could refuse) before it's
        // taken: taking an fd that isn't open would close someone else's
        // later. The File is never dropped, so never closes it.
        // SAFETY: IPC_FD isn't -1, and the File only asks fstat about it.
        let probe = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(IPC_FD) });
        let md = probe.metadata()?;
        if !md.file_type().is_socket() {
            return Err(io::Error::other("fd 3 is not a socket"));
        }
        // SAFETY: rush hands the plugin fd 3, a socket, for it to own;
        // nothing else in the process uses it.
        let stream = unsafe { UnixStream::from_raw_fd(IPC_FD) };
        Conn::new(stream, Some(h))
    }

    /// Speaks framed JSON-RPC over a socket. h may be None: every request
    /// then gets "method not found".
    pub fn new(stream: UnixStream, h: Option<Handler>) -> io::Result<Conn> {
        let r = stream.try_clone()?;
        let closer = stream.try_clone()?;
        Ok(Conn::start(
            Box::new(FramedReader(BufReader::with_capacity(64 << 10, r))),
            Box::new(FramedWriter(stream)),
            Box::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            }),
            h,
        ))
    }

    /// Speaks newline-delimited JSON-RPC, MCP's stdio transport, reading r
    /// and writing w: for a plugin with `"protocol": "mcp"`. closer must
    /// make a blocked read of r return, or the reading thread stays until
    /// the other side closes.
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
        let conn = Conn(Arc::new(Inner {
            writer: Mutex::new(Some(writer)),
            closer,
            handler: h,
            next: AtomicI64::new(0),
            pending: Mutex::new(Pending {
                closed: false,
                map: HashMap::new(),
            }),
            done: Mutex::new(None),
            done_cv: Condvar::new(),
        }));
        let c = conn.clone();
        std::thread::Builder::new()
            .name("rush-rpc".into())
            .spawn(move || c.read_loop(&mut *reader))
            .expect("starting the thread that reads from rush");
        conn
    }

    /// Whether the connection has closed.
    pub fn is_done(&self) -> bool {
        lock(&self.0.done).is_some()
    }

    /// Blocks until the connection has closed.
    pub fn wait(&self) {
        let mut d = lock(&self.0.done);
        while d.is_none() {
            d = self.0.done_cv.wait(d).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Blocks until the connection has closed or d has passed, and says
    /// whether it closed: a sleep that ends early when rush goes.
    pub fn wait_timeout(&self, d: Duration) -> bool {
        let until = Instant::now() + d;
        let mut g = lock(&self.0.done);
        while g.is_none() {
            let now = Instant::now();
            if now >= until {
                return false;
            }
            g = self
                .0
                .done_cv
                .wait_timeout(g, until - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }

    /// Why the connection closed, once it has.
    pub fn err(&self) -> String {
        self.wait();
        lock(&self.0.done).clone().unwrap_or_default()
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
            let Ok(m) = serde_json::from_slice::<Message>(&b) else {
                let _ = self.send(Message {
                    error: Some(Error::new(CODE_PARSE, "parse error")),
                    ..Default::default()
                });
                continue;
            };
            match (m.method.is_empty(), m.id.is_some()) {
                (false, true) => self.serve_async(m),
                (false, false) => {
                    if let Some(h) = &self.0.handler {
                        let _ = h(self, &m.method, m.params.unwrap_or(Value::Null));
                    }
                }
                (true, true) => {
                    let id = m.id.as_ref().map(Value::to_string).unwrap_or_default();
                    let ch = lock(&self.0.pending).map.remove(&id);
                    if let Some(ch) = ch {
                        let _ = ch.send(Reply::Msg(m));
                    }
                }
                (true, false) => {}
            }
        };
        let waiting = {
            let mut p = lock(&self.0.pending);
            p.closed = true;
            std::mem::take(&mut p.map)
        };
        for (_, ch) in waiting {
            let _ = ch.send(Reply::Closed);
        }
        *lock(&self.0.done) = Some(err);
        self.0.done_cv.notify_all();
    }

    /// Answers m on a thread of its own.
    fn serve_async(&self, m: Message) {
        let c = self.clone();
        let id = m.id.clone();
        let spawned = std::thread::Builder::new()
            .name("rush-rpc-call".into())
            .spawn(move || {
                let out = c.answer(m);
                let _ = c.send(out);
            });
        if let Err(e) = spawned {
            let _ = self.send(Message {
                id,
                error: Some(Error::server(e)),
                ..Default::default()
            });
        }
    }

    fn answer(&self, m: Message) -> Message {
        let mut out = Message {
            id: m.id,
            ..Default::default()
        };
        match &self.0.handler {
            None => out.error = Some(Error::no_method(&m.method)),
            Some(h) => match h(self, &m.method, m.params.unwrap_or(Value::Null)) {
                Ok(v) => out.result = Some(v),
                Err(e) => out.error = Some(e),
            },
        }
        out
    }

    fn send(&self, mut m: Message) -> Result<(), String> {
        m.jsonrpc = "2.0".into();
        let b = serde_json::to_vec(&m).map_err(|e| e.to_string())?;
        let mut w = lock(&self.0.writer);
        match w.as_mut() {
            Some(w) => w.write_msg(&b).map_err(|e| e.to_string()),
            None => Err("the connection is closed".into()),
        }
    }

    /// Calls method with params and decodes the result, waiting as long as
    /// it takes or until the connection closes. Params that encode as null
    /// (`&()`) are sent as none.
    pub fn call<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, CallError> {
        self.call_within(method, params, None)
    }

    /// Calls method, as [`Conn::call`], giving up after d.
    pub fn call_timeout<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
        d: Duration,
    ) -> Result<R, CallError> {
        self.call_within(method, params, Some(d))
    }

    fn call_within<P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
        d: Option<Duration>,
    ) -> Result<R, CallError> {
        let p = encode(params)?;
        let res = self.call_raw(method, p, d)?;
        serde_json::from_value(res).map_err(|e| CallError::Other(format!("{method}: {e}")))
    }

    /// Calls method with params already encoded (None sends none), and
    /// returns the result as it came (null when there was none).
    pub fn call_raw(
        &self,
        method: &str,
        params: Option<Value>,
        d: Option<Duration>,
    ) -> Result<Value, CallError> {
        let n = self.0.next.fetch_add(1, Ordering::Relaxed) + 1;
        let id = n.to_string();
        let (tx, rx) = mpsc::channel();
        {
            let mut p = lock(&self.0.pending);
            if p.closed {
                return Err(CallError::Closed);
            }
            p.map.insert(id.clone(), tx);
        }
        let forget = || {
            lock(&self.0.pending).map.remove(&id);
        };
        let msg = Message {
            id: Some(Value::from(n)),
            method: method.to_string(),
            params,
            ..Default::default()
        };
        if let Err(e) = self.send(msg) {
            forget();
            return Err(CallError::Other(e));
        }
        let got = match d {
            Some(d) => rx.recv_timeout(d).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => CallError::Timeout(method.to_string()),
                mpsc::RecvTimeoutError::Disconnected => CallError::Closed,
            }),
            None => rx.recv().map_err(|_| CallError::Closed),
        };
        match got {
            Ok(Reply::Msg(m)) => match m.error {
                Some(e) => Err(CallError::Rpc(e)),
                None => Ok(m.result.unwrap_or(Value::Null)),
            },
            Ok(Reply::Closed) => Err(CallError::Closed),
            Err(e) => {
                forget();
                Err(e)
            }
        }
    }

    /// Sends a notification, which has no reply.
    pub fn notify<P: Serialize + ?Sized>(&self, method: &str, params: &P) -> Result<(), CallError> {
        let p = encode(params)?;
        self.send(Message {
            method: method.to_string(),
            params: p,
            ..Default::default()
        })
        .map_err(CallError::Other)
    }

    /// Writes a line to the broker's log (`rush plugin logs`), without
    /// waiting.
    pub fn log(&self, message: impl fmt::Display) {
        let _ = self.notify("log", &serde_json::json!({"message": message.to_string()}));
    }
}

fn encode<P: Serialize + ?Sized>(params: &P) -> Result<Option<Value>, CallError> {
    let v = serde_json::to_value(params).map_err(|e| CallError::Other(e.to_string()))?;
    Ok((!v.is_null()).then_some(v))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A pair of connections, as rush and a plugin, over a socket pair.
    fn pair(rush: Option<Handler>, plugin: Option<Handler>) -> (Conn, Conn) {
        let (a, b) = UnixStream::pair().unwrap();
        (Conn::new(a, rush).unwrap(), Conn::new(b, plugin).unwrap())
    }

    #[test]
    fn calls_go_both_ways() {
        let rush: Handler = Arc::new(|_, method, _| match method {
            "sessions.list" => Ok(json!([{"id": "a1"}])),
            m => Err(Error::no_method(m)),
        });
        let plugin: Handler = Arc::new(|c, method, p| match method {
            // A request may call rush back while it's answered.
            "tools.call" => {
                let list: Vec<Value> = c.call("sessions.list", &())?;
                Ok(json!({"n": list.len(), "args": p["arguments"]}))
            }
            m => Err(Error::no_method(m)),
        });
        let (rush, plugin) = pair(Some(rush), Some(plugin));
        let got: Value = rush
            .call("tools.call", &json!({"arguments": {"x": 1}}))
            .unwrap();
        assert_eq!(got, json!({"n": 1, "args": {"x": 1}}));
        let e = rush.call::<_, Value>("frob", &()).unwrap_err();
        assert_eq!(e.code(), Some(CODE_NO_METHOD));
        assert_eq!(e.to_string(), "method not found: frob");
        plugin.close();
        rush.wait();
        assert!(plugin.is_done() || plugin.wait_timeout(Duration::from_secs(5)));
    }

    #[test]
    fn notifications_come_in_order() {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let plugin: Handler = Arc::new(move |_, method, p| {
            if method == "ui.event" {
                lock(&tx).send(p["n"].as_i64().unwrap()).unwrap();
            }
            Ok(Value::Null)
        });
        let (rush, _plugin) = pair(None, Some(plugin));
        for n in 0..50 {
            rush.notify("ui.event", &json!({"n": n})).unwrap();
        }
        let got: Vec<i64> = (0..50)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert_eq!(got, (0..50).collect::<Vec<_>>());
    }

    /// rush, holding each request it's sent until the test lets it go: it
    /// says on the first channel that one came, and the second, dropped,
    /// lets them all go.
    fn holding() -> (Handler, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (came_tx, came) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let came_tx = Mutex::new(came_tx);
        let release_rx = Mutex::new(release_rx);
        let h: Handler = Arc::new(move |_, _, _| {
            let _ = lock(&came_tx).send(());
            let _ = lock(&release_rx).recv();
            Ok(Value::Null)
        });
        (h, came, release)
    }

    #[test]
    fn a_closed_connection_fails_its_calls() {
        let (h, came, _release) = holding();
        let (rush, plugin) = pair(Some(h), None);
        let waiting = std::thread::spawn({
            let plugin = plugin.clone();
            move || plugin.call::<_, Value>("sessions.watch", &())
        });
        // rush has it, doesn't answer, and goes.
        came.recv_timeout(Duration::from_secs(5)).unwrap();
        rush.close();
        assert_eq!(waiting.join().unwrap().unwrap_err(), CallError::Closed);
        assert!(plugin.wait_timeout(Duration::from_secs(5)));
        assert_eq!(
            plugin.call::<_, Value>("x", &()).unwrap_err(),
            CallError::Closed
        );
    }

    #[test]
    fn a_call_can_time_out() {
        let (h, _came, _release) = holding();
        let (_rush, plugin) = pair(Some(h), None);
        let e = plugin
            .call_timeout::<_, Value>("ui.send", &(), Duration::from_millis(50))
            .unwrap_err();
        assert_eq!(e.to_string(), "ui.send: rush didn't answer");
    }

    #[test]
    fn frames_are_length_prefixed_json() {
        let (mine, theirs) = UnixStream::pair().unwrap();
        let plugin = Conn::new(theirs, None).unwrap();
        plugin.notify("log", &json!({"message": "hi"})).unwrap();
        let mut r = BufReader::new(mine);
        let mut hdr = [0u8; 4];
        r.read_exact(&mut hdr).unwrap();
        let mut b = vec![0u8; u32::from_be_bytes(hdr) as usize];
        r.read_exact(&mut b).unwrap();
        let v: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(
            v,
            json!({"jsonrpc": "2.0", "method": "log", "params": {"message": "hi"}})
        );
    }

    #[test]
    fn lines_speak_mcp_stdio() {
        let (a, b) = UnixStream::pair().unwrap();
        let (r, w) = (b.try_clone().unwrap(), b.try_clone().unwrap());
        let server: Handler = Arc::new(|_, method, _| match method {
            "tools/list" => Ok(json!({"tools": []})),
            m => Err(Error::no_method(m)),
        });
        let _server = Conn::lines(
            r,
            w,
            move || {
                let _ = b.shutdown(std::net::Shutdown::Both);
            },
            Some(server),
        );
        let mut a2 = a.try_clone().unwrap();
        a2.write_all(b"\n{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"method\":\"tools/list\"}\n")
            .unwrap();
        let mut line = String::new();
        BufReader::new(a).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v,
            json!({"jsonrpc": "2.0", "id": "x", "result": {"tools": []}})
        );
    }
}
