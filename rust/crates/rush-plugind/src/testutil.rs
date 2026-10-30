//! What the tests share: a rush folder of their own, sessions as their
//! hosts publish them, a broker, plugins and windows standing in for real
//! ones over socket pairs.
//! After internal/plugind/api_test.go:17-45 and ui_test.go:22-135.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_plugin::rpc::{CODE_DENIED, CODE_LIMITED, Handler};
use rush_plugin::{CallError, Conn, Ctx, Manifest, Plugin, UiDo, UiEvent, UiSession, UiState};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::broker::Broker;
use crate::host::{Dialed, FileHosts, Hosts, Info, Lister, SpawnConfig, View};
use crate::runner::Runner;
use crate::{Log, Scope};

/// A rush folder for this test's thread: short, under /tmp, as a unix
/// socket's path must be; resolved, as workspaces are.
pub(crate) struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    pub(crate) fn new() -> Home {
        let dir = tempfile::Builder::new()
            .prefix("rpd-")
            .tempdir_in("/tmp")
            .unwrap();
        rush_state::set_thread_dir(Some(dir.path().canonicalize().unwrap()));
        Home { dir }
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    /// A folder in it, made.
    pub(crate) fn mkdir(&self, name: &str) -> String {
        let d = self.path().join(name);
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    /// This test's log of events.
    pub(crate) fn logger(&self) -> Arc<rush_events::Logger> {
        Arc::new(rush_events::Logger::new(Some(
            self.path().join("events.jsonl"),
        )))
    }

    /// The scope a broker of this test runs in: its events in its own log.
    pub(crate) fn scope(&self) -> Scope {
        Scope::with_events(self.logger())
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
    }
}

/// Makes a session as its host would publish it.
pub(crate) fn write_info(i: &Info) {
    let d = crate::host::root().join(&i.id);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("info.json"), serde_json::to_vec(i).unwrap()).unwrap();
}

/// Rush's files for sessions, with views kept here rather than by a host.
#[derive(Default)]
pub(crate) struct TestHosts {
    pub(crate) views: Mutex<HashMap<String, Vec<View>>>,
    pub(crate) spawned: Mutex<Vec<SpawnConfig>>,
}

impl Hosts for TestHosts {
    fn lister(&self) -> Lister {
        FileHosts::default().lister()
    }
    fn read_info(&self, id: &str) -> Result<Info, String> {
        FileHosts::default().read_info(id)
    }
    fn dial(&self, id: &str) -> Result<Dialed, String> {
        FileHosts::default().dial(id)
    }
    fn spawn(&self, cfg: &SpawnConfig) -> Result<String, String> {
        self.spawned.lock().unwrap().push(cfg.clone());
        Err("not in tests".into())
    }
    fn log_event(&self, _: &str, _: &str, _: rush_events::Fields) {}
    fn views(&self, id: &str) -> Vec<View> {
        self.views
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default()
    }
    fn save_view(&self, id: &str, v: View) -> Result<(), String> {
        let mut vs = self.views.lock().unwrap();
        let list = vs.entry(id.into()).or_default();
        list.retain(|o| o.get("name") != v.get("name"));
        list.insert(0, v);
        Ok(())
    }
    fn drop_view(&self, id: &str, name: &str) -> Result<(), String> {
        if let Some(list) = self.views.lock().unwrap().get_mut(id) {
            list.retain(|o| o.get("name").and_then(Value::as_str) != Some(name));
        }
        Ok(())
    }
}

/// A broker in home that runs nothing yet.
pub(crate) fn test_broker(home: &Home) -> (Broker, Arc<TestHosts>) {
    let hosts = Arc::new(TestHosts::default());
    (
        Broker::new(hosts.clone(), Log::discard(), home.scope()),
        hosts,
    )
}

/// A runner standing alone, for plugin m.
pub(crate) fn test_runner(home: &Home, m: Manifest) -> (Arc<Runner>, Plugin) {
    let _ = std::fs::create_dir_all(rush_plugin::data_dir(&m.name));
    let r = Runner::with(
        &m.name,
        "",
        Log::discard(),
        home.scope(),
        Arc::new(FileHosts::logging_to(home.logger())),
        std::sync::Weak::new(),
    );
    let p = Plugin {
        dir: rush_plugin::root()
            .join(&m.name)
            .to_string_lossy()
            .into_owned(),
        manifest: m,
        ..Default::default()
    };
    r.set_plugin(p.clone());
    (r, p)
}

pub(crate) fn manifest(name: &str) -> Manifest {
    Manifest {
        name: name.into(),
        command: vec!["bin".into()],
        ..Default::default()
    }
}

/// Whether e is a call refused for want of a capability.
pub(crate) fn denied(e: &rush_plugin::Error) -> bool {
    e.code == CODE_DENIED
}

/// Whether e is a call allowed, but made too often.
pub(crate) fn limited(e: &CallError) -> bool {
    e.rpc().is_some_and(|e| e.code == CODE_LIMITED)
}

fn call<P: Serialize + ?Sized, R: DeserializeOwned>(
    c: &Conn,
    method: &str,
    params: &P,
) -> Result<R, CallError> {
    c.call(
        &Ctx::background().with_timeout(Duration::from_secs(3)),
        method,
        params,
    )
}

/// A plugin as the broker sees one running: a connection, with the
/// plugin's end answering as h says.
pub(crate) struct FakePlugin {
    pub(crate) r: Arc<Runner>,
    /// the plugin's end
    pub(crate) conn: Conn,
    pub(crate) events: mpsc::Receiver<UiEvent>,
    /// other notifications and calls, method then params
    pub(crate) other: mpsc::Receiver<String>,
}

impl FakePlugin {
    pub(crate) fn call<P: Serialize + ?Sized>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<Value, CallError> {
        call::<_, Option<Value>>(&self.conn, method, params).map(Option::unwrap_or_default)
    }
}

impl Drop for FakePlugin {
    fn drop(&mut self) {
        self.conn.close();
    }
}

pub(crate) fn add_plugin(b: &Broker, m: Manifest, h: Option<Handler>) -> FakePlugin {
    let (mine, theirs) = UnixStream::pair().unwrap();
    let (etx, events) = mpsc::channel();
    let (otx, other) = mpsc::channel();
    let (etx, otx) = (Mutex::new(etx), Mutex::new(otx));
    let ph: Handler = Arc::new(move |c, ctx, method, params| {
        if method == "ui.event" {
            let e: UiEvent = rush_plugin::rpc::from_raw(params).unwrap();
            let _ = etx.lock().unwrap().send(e);
            return Ok(None);
        }
        if let Some(h) = &h {
            return h(c, ctx, method, params);
        }
        let p = params.map_or("null".to_string(), |p| p.get().to_string());
        let _ = otx.lock().unwrap().send(format!("{method} {p}"));
        Ok(Some(crate::raw("{}")))
    });
    let conn = Conn::new(theirs, Some(ph)).unwrap();
    let r = Runner::new(Some(b), &m.name, "");
    let me = Arc::downgrade(&r);
    let bh: Handler = Arc::new(move |_, ctx, method, params| match me.upgrade() {
        Some(r) => r.answer_plugin(ctx, method, params),
        None => Err(crate::server("gone")),
    });
    let bc = Conn::new(mine, Some(bh)).unwrap();
    r.set_running(
        Plugin {
            manifest: m,
            ..Default::default()
        },
        bc,
    );
    b.0.put_runner(r.clone());
    FakePlugin {
        r,
        conn,
        events,
        other,
    }
}

/// A rush window attached to the broker.
pub(crate) struct FakeUi {
    pub(crate) conn: Conn,
    pub(crate) states: mpsc::Receiver<UiState>,
    pub(crate) dos: mpsc::Receiver<UiDo>,
}

impl Drop for FakeUi {
    fn drop(&mut self) {
        self.conn.close();
    }
}

pub(crate) fn attach_ui(b: &Broker, name: &str) -> FakeUi {
    let (mine, theirs) = UnixStream::pair().unwrap();
    b.accept(mine).unwrap();
    let (stx, states) = mpsc::channel();
    let (dtx, dos) = mpsc::channel();
    let (stx, dtx) = (Mutex::new(stx), Mutex::new(dtx));
    let h: Handler = Arc::new(move |_, _, method, params| {
        match method {
            "ui.state" => {
                let _ = stx
                    .lock()
                    .unwrap()
                    .send(rush_plugin::rpc::from_raw(params).unwrap());
            }
            "ui.do" => {
                let _ = dtx
                    .lock()
                    .unwrap()
                    .send(rush_plugin::rpc::from_raw(params).unwrap());
            }
            _ => {}
        }
        Ok(None)
    });
    let u = FakeUi {
        conn: Conn::new(theirs, Some(h)).unwrap(),
        states,
        dos,
    };
    u.call("ui.attach", &serde_json::json!({"ui": name}))
        .unwrap();
    u
}

impl FakeUi {
    pub(crate) fn call<P: Serialize + ?Sized>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<Value, CallError> {
        call::<_, Option<Value>>(&self.conn, method, params).map(Option::unwrap_or_default)
    }

    /// Sends events, and returns once the broker has taken them:
    /// notifications are taken in order, before a later request.
    pub(crate) fn events(&self, evs: &[UiEvent]) {
        for e in evs {
            self.conn.notify("ui.event", e).unwrap();
        }
        self.call("ui.attach", &serde_json::json!({"ui": "main"}))
            .unwrap();
    }

    pub(crate) fn wait_state(&self, what: &str, ok: impl Fn(&UiState) -> bool) -> UiState {
        loop {
            match self.states.recv_timeout(Duration::from_secs(3)) {
                Ok(st) if ok(&st) => return st,
                Ok(_) => {}
                Err(_) => panic!("no ui.state with {what}"),
            }
        }
    }

    pub(crate) fn wait_do(&self) -> UiDo {
        self.dos
            .recv_timeout(Duration::from_secs(3))
            .expect("no ui.do")
    }
}

pub(crate) fn sess(id: &str, cwd: &str) -> Option<UiSession> {
    Some(UiSession {
        id: id.into(),
        cwd: cwd.into(),
        name: id.into(),
        ..Default::default()
    })
}
