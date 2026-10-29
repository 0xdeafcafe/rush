//! One language server process for one checkout: LSP over its stdin and
//! stdout, the files it has open, and the diagnostics it pushes.
//! Ported from internal/bundled/ts/server.go:1-439.
//!
//! A thread reads the server, blocked on its stdout; another waits for it
//! to exit. A call blocks on a channel its reply, its Ctx or the server's
//! end fills.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use rush_plugin::Ctx;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use super::engine::Engine;
use crate::gatecore::Ticket;
use crate::{Got, wait_on};

type Reply = Result<Option<Box<RawValue>>, String>;

#[derive(Clone, Copy)]
struct OpenFile {
    mtime: Option<SystemTime>,
    version: i64,
}

#[derive(Default)]
struct State {
    next_id: i64,
    pending: HashMap<i64, mpsc::Sender<Got<Reply>>>,
    /// by URI
    opened: HashMap<String, OpenFile>,
    diags: HashMap<String, Box<RawValue>>,
    diag_wait: HashMap<String, Vec<mpsc::Sender<Got<()>>>>,
    closed: bool,
    /// why it stopped, for the calls it fails
    why: Option<String>,
    last_used: Option<Instant>,
}

/// One language server process for one checkout.
pub struct Server {
    pub root: String,
    pub engine: Engine,
    pid: i32,
    input: Mutex<ChildStdin>,
    /// its gate ticket; None when the gate is off
    pub(crate) run: Option<Box<dyn Ticket>>,
    caps: Mutex<serde_json::Map<String, Value>>,
    st: Mutex<State>,
    done: Mutex<bool>,
    done_cv: Condvar,
}

/// A member that's there, even as null, read as written.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(d).map(Some)
}

/// A response's error, as LSP sends it.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RpcError {
    message: String,
}

/// Any LSP message coming in.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Incoming {
    #[serde(deserialize_with = "present")]
    id: Option<Box<RawValue>>,
    method: String,
    params: Option<Box<RawValue>>,
    #[serde(deserialize_with = "present")]
    result: Option<Box<RawValue>>,
    error: Option<RpcError>,
}

/// Any LSP message going out, its members as Go's omitzero writes them.
#[derive(Serialize)]
struct Outgoing<'a> {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "str::is_empty")]
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a RawValue>,
}

/// v as JSON, "null" when it's nothing or can't be written.
pub(crate) fn raw<T: Serialize + ?Sized>(v: &T) -> Box<RawValue> {
    rush_state::json::to_vec(v)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| RawValue::from_string(s).ok())
        .unwrap_or_else(null)
}

fn null() -> Box<RawValue> {
    RawValue::from_string("null".into()).expect("null is JSON")
}

/// Reads a result as Go's Unmarshal into out does: nothing, or null,
/// leaves it as it was.
pub(crate) fn decode<T: for<'de> Deserialize<'de> + Default>(
    r: Option<&RawValue>,
) -> Result<T, String> {
    match r {
        None => Ok(T::default()),
        Some(r) if r.get() == "null" => Ok(T::default()),
        Some(r) => rush_state::json::from_slice(r.get().as_bytes()).map_err(|e| e.to_string()),
    }
}

impl Server {
    /// Runs e for the checkout at root and says hello to it. run, if set,
    /// is the gate ticket it holds while it lives.
    pub fn start(
        ctx: &Ctx,
        root: &str,
        e: Engine,
        run: Option<Box<dyn Ticket>>,
    ) -> Result<Arc<Server>, String> {
        let mut cmd = Command::new(&e.bin);
        cmd.args(["--lsp", "--stdio"])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = cmd.spawn().map_err(|err| {
            let s = rush_plugin::gopath::errno_text(&err);
            format!("fork/exec {}: {s}", e.bin)
        })?;
        let input = child.stdin.take().expect("piped");
        let out = child.stdout.take().expect("piped");
        let s = Arc::new(Server {
            root: root.into(),
            engine: e.clone(),
            pid: child.id() as i32,
            input: Mutex::new(input),
            run,
            caps: Mutex::new(serde_json::Map::new()),
            st: Mutex::new(State {
                last_used: Some(Instant::now()),
                ..Default::default()
            }),
            done: Mutex::new(false),
            done_cv: Condvar::new(),
        });
        {
            let s = s.clone();
            std::thread::spawn(move || s.read(BufReader::with_capacity(1 << 16, out)));
        }
        {
            let s = s.clone();
            std::thread::spawn(move || {
                let st = child.wait().ok();
                s.stop(format!(
                    "the {} language server for {} exited: {}",
                    s.engine.name,
                    s.root,
                    crate::go_exit(st)
                ));
            });
        }
        let root_uri = file_uri(root);
        let base = std::path::Path::new(root)
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_default();
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "definition": { "linkSupport": false },
                    "implementation": { "linkSupport": false },
                    "references": {},
                    "callHierarchy": {},
                    "publishDiagnostics": {},
                    "diagnostic": {},
                },
                "workspace": { "symbol": {}, "workspaceFolders": true, "configuration": true },
            },
            "workspaceFolders": [{ "uri": root_uri, "name": base }],
        });
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Init {
            capabilities: serde_json::Map<String, Value>,
        }
        let init = s
            .call(ctx, "initialize", &params)
            .and_then(|r| decode::<Init>(r.as_deref()));
        match init {
            Ok(init) => {
                *s.caps.lock().unwrap_or_else(|e| e.into_inner()) = init.capabilities;
            }
            Err(err) => {
                s.kill(&err);
                return Err(err);
            }
        }
        s.notify("initialized", &serde_json::json!({}));
        Ok(s)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits for the server to stop, up to d; whether it has.
    pub fn wait_done(&self, d: Duration) -> bool {
        let g = self.done.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = self
            .done_cv
            .wait_timeout_while(g, d, |done| !*done)
            .unwrap_or_else(|e| e.into_inner());
        *g
    }

    /// Why the server stopped; None while it runs.
    pub fn why(&self) -> Option<String> {
        self.lock().why.clone()
    }

    /// How long since the server was last asked anything.
    pub fn idle(&self) -> Duration {
        self.lock()
            .last_used
            .map_or(Duration::ZERO, |t| t.elapsed())
    }

    /// The server process's.
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Stops the server at once, failing what's in flight with why.
    pub fn kill(&self, why: &str) {
        self.stop(why.to_string());
        // SAFETY: kill(2) of the process group we started the server in.
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
        }
    }

    /// Asks the server to go, then makes it.
    pub fn close(&self) {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(2));
        let _ = self.call(&ctx, "shutdown", &Value::Null);
        self.notify("exit", &Value::Null);
        self.wait_done(Duration::from_secs(2));
        self.kill("closed");
    }

    fn stop(&self, why: String) {
        let pending = {
            let mut st = self.lock();
            if st.closed {
                return;
            }
            st.closed = true;
            st.why = Some(why.clone());
            for (_, ws) in st.diag_wait.drain() {
                for w in ws {
                    let _ = w.send(Got::Val(()));
                }
            }
            std::mem::take(&mut st.pending)
        };
        for (_, tx) in pending {
            let _ = tx.send(Got::Val(Err(why.clone())));
        }
        if let Some(r) = &self.run {
            r.release();
        }
        *self.done.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.done_cv.notify_all();
    }

    /// Sends a request and gives back its result as sent.
    pub fn call<P: Serialize + ?Sized>(
        &self,
        ctx: &Ctx,
        method: &str,
        params: &P,
    ) -> Result<Option<Box<RawValue>>, String> {
        let (tx, rx) = mpsc::channel();
        let id = {
            let mut st = self.lock();
            if st.closed {
                return Err(st.why.clone().unwrap_or_default());
            }
            st.next_id += 1;
            let id = st.next_id;
            st.pending.insert(id, tx.clone());
            st.last_used = Some(Instant::now());
            id
        };
        let _reg = ctx.on_cancel(move || {
            let _ = tx.send(Got::Stop);
        });
        let id_raw = RawValue::from_string(id.to_string()).expect("a number is JSON");
        let params = raw(params);
        self.send(&Outgoing {
            jsonrpc: "2.0",
            id: Some(&id_raw),
            method,
            params: Some(&params),
            result: None,
        })?;
        match wait_on(ctx, &rx) {
            Ok(r) => r,
            Err(e) => {
                self.lock().pending.remove(&id);
                self.notify("$/cancelRequest", &serde_json::json!({ "id": id }));
                Err(format!("{method}: {e}"))
            }
        }
    }

    pub(crate) fn notify<P: Serialize + ?Sized>(&self, method: &str, params: &P) {
        let params = raw(params);
        let _ = self.send(&Outgoing {
            jsonrpc: "2.0",
            id: None,
            method,
            params: Some(&params),
            result: None,
        });
    }

    fn send(&self, m: &Outgoing) -> Result<(), String> {
        let body = rush_state::json::to_vec(m).map_err(|e| e.to_string())?;
        let mut w = self.input.lock().unwrap_or_else(|e| e.into_inner());
        write!(w, "Content-Length: {}\r\n\r\n", body.len()).map_err(|e| e.to_string())?;
        w.write_all(&body).map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())
    }

    /// Takes the server's messages until it closes: replies to calls,
    /// diagnostics it pushes, and requests it makes of us, which get the
    /// answer that asks for nothing.
    fn read(&self, mut r: impl BufRead) {
        let mut line = String::new();
        loop {
            let mut n: Option<usize> = None;
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                if !line.ends_with('\n') {
                    return;
                }
                let l = line.trim();
                if l.is_empty() {
                    break;
                }
                if let Some((k, v)) = l.split_once(':')
                    && k.trim().eq_ignore_ascii_case("Content-Length")
                {
                    n = Some(v.trim().parse().unwrap_or(0));
                }
            }
            let Some(n) = n else {
                continue;
            };
            let mut body = vec![0; n];
            if r.read_exact(&mut body).is_err() {
                return;
            }
            let Ok(m) = rush_state::json::from_slice::<Incoming>(&body) else {
                continue;
            };
            if !m.method.is_empty() && m.id.is_some() {
                self.answer(&m);
            } else if m.method == "textDocument/publishDiagnostics" {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Pushed {
                    uri: String,
                    diagnostics: Option<Box<RawValue>>,
                }
                if let Ok(p) = decode::<Pushed>(m.params.as_deref()) {
                    let mut st = self.lock();
                    st.diags
                        .insert(p.uri.clone(), p.diagnostics.unwrap_or_else(null));
                    for w in st.diag_wait.remove(&p.uri).unwrap_or_default() {
                        let _ = w.send(Got::Val(()));
                    }
                }
            } else if m.method.is_empty()
                && let Some(id) = &m.id
            {
                let id: i64 = id.get().parse().unwrap_or(0);
                let Some(tx) = self.lock().pending.remove(&id) else {
                    continue;
                };
                let reply = match m.error {
                    Some(e) => Err(format!("{}: {}", self.engine.name, e.message)),
                    None => Ok(m.result),
                };
                let _ = tx.send(Got::Val(reply));
            }
        }
    }

    /// Replies to the server's own requests: a null for each setting it
    /// asks for, and nothing for the rest (progress, registrations).
    fn answer(&self, m: &Incoming) {
        let mut result = null();
        if m.method == "workspace/configuration" {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Items {
                items: Vec<Value>,
            }
            let p: Items = decode(m.params.as_deref()).unwrap_or_default();
            result = raw(&vec![Value::Null; p.items.len()]);
        }
        let _ = self.send(&Outgoing {
            jsonrpc: "2.0",
            id: m.id.as_deref(),
            method: "",
            params: None,
            result: Some(&result),
        });
    }

    /// Makes sure the server has file as it is on disk now: opened, or
    /// sent again when it changed since. It is the URI to ask about.
    pub fn open(&self, file: &str) -> Result<String, String> {
        let st = std::fs::metadata(file)
            .map_err(|e| rush_plugin::gopath::os_err("stat", std::path::Path::new(file), &e))?;
        let mtime = st.modified().ok();
        let uri = file_uri(file);
        let was = self.lock().opened.get(&uri).copied();
        if let Some(w) = was
            && w.mtime == mtime
        {
            return Ok(uri);
        }
        let text = std::fs::read(file)
            .map_err(|e| rush_plugin::gopath::os_err("open", std::path::Path::new(file), &e))?;
        let text = String::from_utf8_lossy(&text).into_owned();
        let version = was.map_or(0, |w| w.version) + 1;
        {
            let mut s = self.lock();
            s.opened.insert(uri.clone(), OpenFile { mtime, version });
            s.diags.remove(&uri);
        }
        if was.is_none() {
            self.notify(
                "textDocument/didOpen",
                &serde_json::json!({ "textDocument": {
                    "uri": uri, "languageId": language_of(file), "version": version, "text": text,
                }}),
            );
        } else {
            self.notify(
                "textDocument/didChange",
                &serde_json::json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": text }],
                }),
            );
        }
        Ok(uri)
    }

    /// Closes uri if it was open: it's gone from where it was.
    pub(crate) fn forget(&self, uri: &str) {
        let was = self.lock().opened.remove(uri).is_some();
        if was {
            self.notify(
                "textDocument/didClose",
                &serde_json::json!({ "textDocument": { "uri": uri } }),
            );
        }
    }

    /// What the server says is wrong in uri: asked for where it answers
    /// that, else the last it pushed, waiting up to wait for one.
    pub fn diagnostics(&self, ctx: &Ctx, uri: &str, wait: Duration) -> Option<Box<RawValue>> {
        let asks = self
            .caps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key("diagnosticProvider");
        if asks {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Report {
                items: Option<Box<RawValue>>,
            }
            if let Ok(r) = self
                .call(
                    ctx,
                    "textDocument/diagnostic",
                    &serde_json::json!({ "textDocument": { "uri": uri } }),
                )
                .and_then(|r| decode::<Report>(r.as_deref()))
            {
                return r.items;
            }
        }
        let (tx, rx) = mpsc::channel();
        {
            let mut st = self.lock();
            if let Some(d) = st.diags.get(uri) {
                return Some(d.clone());
            }
            if st.closed {
                return None;
            }
            st.diag_wait
                .entry(uri.to_string())
                .or_default()
                .push(tx.clone());
        }
        let _reg = ctx.on_cancel(move || {
            let _ = tx.send(Got::Stop);
        });
        let _ = wait_on(&ctx.with_timeout(wait), &rx);
        self.lock().diags.get(uri).cloned()
    }

    /// What the server's processes hold now, as Activity Monitor counts it.
    pub fn footprint(&self) -> u64 {
        let mut tab = rush_sys::proc::snapshot();
        tab.fill(None, &[self.pid]);
        tab.descendants(self.pid)
            .iter()
            .filter_map(|p| tab.procs.get(p))
            .map(|p| p.footprint)
            .sum()
    }
}

/// A path as a file: URI, escaped as Go's url.URL writes a path.
pub fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for &b in path.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'$'
                    | b'&'
                    | b'+'
                    | b','
                    | b'/'
                    | b':'
                    | b';'
                    | b'='
                    | b'@'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The file a file: URI names.
pub fn path_of(uri: &str) -> String {
    let Some(rest) = uri.strip_prefix("file://") else {
        return uri.to_string();
    };
    // Past the host, if any: file://host/path.
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => return uri.to_string(),
    };
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: u8| (c as char).to_digit(16);
            match (
                b.get(i + 1).and_then(|&c| hex(c)),
                b.get(i + 2).and_then(|&c| hex(c)),
            ) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => return uri.to_string(), // a bad escape: url.Parse refuses it
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn language_of(file: &str) -> &'static str {
    let ext = std::path::Path::new(file)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        _ => "typescript",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris() {
        assert_eq!(file_uri("/a b/c.ts"), "file:///a%20b/c.ts");
        assert_eq!(file_uri("/x/é.ts"), "file:///x/%C3%A9.ts");
        assert_eq!(path_of("file:///a%20b/c.ts"), "/a b/c.ts");
        assert_eq!(path_of(&file_uri("/x/é (1).ts")), "/x/é (1).ts");
        assert_eq!(path_of("untitled:x"), "untitled:x");
        assert_eq!(language_of("a.TSX"), "typescriptreact");
        assert_eq!(language_of("a.mjs"), "javascript");
    }
}
