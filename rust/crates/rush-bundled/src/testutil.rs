//! What the tests share: a rush folder of their own, the lock on the
//! bundles every test in the process shares, and a broker standing in for
//! the real one over a socket pair.

use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use rush_plugin::Conn;
use rush_plugin::rpc::{Error, Handler};
use serde_json::Value;

/// A rush folder for this test's thread, resolved.
pub(crate) struct Home(#[allow(dead_code)] tempfile::TempDir);

impl Home {
    pub(crate) fn new() -> Home {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().canonicalize().unwrap()));
        Home(d)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
    }
}

static BUNDLES: Mutex<()> = Mutex::new(());

/// Held by tests that register bundled plugins or ask whether one is on:
/// the bundles are the whole process's.
pub(crate) fn bundles_lock() -> MutexGuard<'static, ()> {
    BUNDLES.lock().unwrap_or_else(|e| e.into_inner())
}

/// A broker that answers every call with `{}` (or what answer says), and
/// tells calls of each call it got: "method params".
pub(crate) struct Broker {
    pub conn: Conn,
    pub calls: mpsc::Receiver<(String, Value)>,
    pub plugin: Option<JoinHandle<Result<(), String>>>,
}

pub(crate) type Answer = Arc<dyn Fn(&str, &Value) -> Result<Value, Error> + Send + Sync>;

impl Broker {
    /// Starts run on one end of a socket pair, the broker on the other.
    pub(crate) fn start(
        run: fn(UnixStream) -> Result<(), String>,
        answer: Option<Answer>,
    ) -> Broker {
        let (mine, theirs) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let h: Handler = Arc::new(move |_, _, method, params| {
            let v: Value = params
                .map(|p| serde_json::from_str(p.get()).unwrap_or(Value::Null))
                .unwrap_or(Value::Null);
            let _ = tx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .send((method.to_string(), v.clone()));
            let out = match &answer {
                Some(a) => a(method, &v)?,
                None => serde_json::json!({}),
            };
            crate::answer(&out)
        });
        let conn = Conn::new(mine, Some(h)).unwrap();
        let plugin = std::thread::spawn(move || run(theirs));
        Broker {
            conn,
            calls: rx,
            plugin: Some(plugin),
        }
    }

    /// Calls the plugin.
    pub(crate) fn call(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, rush_plugin::CallError> {
        let ctx = rush_plugin::Ctx::background().with_timeout(std::time::Duration::from_secs(10));
        self.conn.call(&ctx, method, &params)
    }

    /// The next call the plugin made named method, the others it made
    /// before it passed over.
    pub(crate) fn want(&self, method: &str) -> Value {
        loop {
            match self.calls.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok((m, v)) if m == method => return v,
                Ok(_) => continue,
                Err(_) => panic!("no {method}"),
            }
        }
    }

    /// Hangs up, and waits for the plugin to end.
    pub(crate) fn close(mut self) -> Result<(), String> {
        self.conn.close();
        self.plugin.take().unwrap().join().unwrap()
    }
}
