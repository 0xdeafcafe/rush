//! A rush window's only way to its plugins, and nothing in it can hold the
//! window up.
//!
//! The window never touches the broker's socket. One thread here owns it:
//! it connects (starting the broker if a plugin is enabled), attaches this
//! window, sends the events the window hands it, and turns what the broker
//! pushes into messages. From the window's side:
//!
//!   - emit puts an event in a bounded queue, or drops it when that's full.
//!   - state is what plugins add (commands, settings, overview sections,
//!     statuses), swapped in whole.
//!   - next, command, picked, intercept and set_setting wait, each with a
//!     deadline: the window runs them off its own thread, as Go's tea.Cmds
//!     are run off bubbletea's.
//!
//! An intercept that doesn't answer in time lets the message go as it was.
//! Ported from internal/hooks/hooks.go.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once, RwLock, Weak};
use std::time::Duration;

use rush_plugin::rpc::Handler;
use rush_plugin::{
    Cancel, Conn, Ctx, INTERCEPT_BUDGET, Intercept, InterceptResult, Picked, TextBox, UI_EVENTS,
    UI_INPUT, UI_INTERCEPT, UiDo, UiEvent, UiSession, UiState,
};
use serde::Serialize;
use serde_json::json;

/// Events waiting to go to the broker.
pub const EVENT_QUEUE: usize = 256;
/// What the broker sent waiting for the window to take it.
pub const INBOX: usize = 64;

/// Something for the window: what plugins add changed (state has it), or
/// a plugin asks this window to do something.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    State,
    Do(Box<UiDo>),
}

/// How a client reaches the broker; a test's fake in tests.
pub type Dial = Box<dyn Fn(Handler) -> Result<Conn, String> + Send + Sync>;

const OFFLINE: &str = "the plugin broker isn't running";

/// A bounded queue a thread waits on.
struct Queue<T> {
    q: Mutex<VecDeque<T>>,
    cv: Condvar,
}

impl<T> Queue<T> {
    fn new() -> Queue<T> {
        Queue {
            q: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<T>> {
        self.q.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds v unless the queue holds max already: false then.
    fn try_push(&self, v: T, max: usize) -> bool {
        let mut q = self.lock();
        if q.len() >= max {
            return false;
        }
        q.push_back(v);
        drop(q);
        self.cv.notify_all();
        true
    }

    fn wake(&self) {
        let _q = self.lock();
        self.cv.notify_all();
    }
}

/// One rush window's connection to its plugins.
pub struct Client {
    id: String,
    events: Queue<UiEvent>,
    inbox: Queue<Msg>,
    state: RwLock<Arc<UiState>>,
    conn: Mutex<Option<Conn>>,
    /// Counts events let go because the queue was full.
    pub dropped: AtomicI64,
    /// Counts connections made: a new one is a broker that knows nothing
    /// yet of what's on the screen.
    attaches: AtomicU64,
    /// A Msg::State waiting in the inbox.
    state_queued: AtomicBool,
    start: Once,
    stopped: Ctx,
    stop: Cancel,
    dial: Dial,
    /// Whether the plugins couldn't be asked last time, so a run of
    /// failures is logged once.
    intercept_failing: AtomicBool,
    /// Where its thread runs and its events go.
    scope: crate::Scope,
}

impl Client {
    /// A client for this window. It does nothing until start.
    pub fn new() -> Arc<Client> {
        Client::with_dial(Box::new(dial_broker))
    }

    /// A client reaching the broker through dial.
    pub fn with_dial(dial: Dial) -> Arc<Client> {
        Client::with_dial_in(dial, crate::Scope::here())
    }

    /// A client reaching the broker through dial, logging in scope.
    pub fn with_dial_in(dial: Dial, scope: crate::Scope) -> Arc<Client> {
        let mut b = [0u8; 4];
        // SAFETY: getentropy fills the 4 bytes of b.
        unsafe {
            libc::getentropy(b.as_mut_ptr().cast(), b.len());
        }
        let (stopped, stop) = Ctx::background().with_cancel();
        Arc::new(Client {
            id: b.iter().map(|x| format!("{x:02x}")).collect(),
            events: Queue::new(),
            inbox: Queue::new(),
            state: RwLock::new(Arc::new(UiState::default())),
            conn: Mutex::new(None),
            dropped: AtomicI64::new(0),
            attaches: AtomicU64::new(0),
            state_queued: AtomicBool::new(false),
            start: Once::new(),
            stopped,
            stop,
            dial,
            intercept_failing: AtomicBool::new(false),
            scope,
        })
    }

    /// A client that never connects, holding s: what plugins add, for tests
    /// and a --render of the screen.
    pub fn fixed(s: UiState) -> Arc<Client> {
        let c = Client::with_dial(Box::new(|_| Err(OFFLINE.into())));
        *c.state.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(s);
        c
    }

    /// Counts the times it has connected to the broker.
    pub fn attaches(&self) -> u64 {
        self.attaches.load(Ordering::SeqCst)
    }

    /// Names this window to plugins.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Connects, in the background, and keeps connecting.
    pub fn start(self: &Arc<Self>) {
        let me = self.clone();
        self.start.call_once(move || {
            let scope = me.scope.clone();
            scope.spawn("plugin-hooks", move || me.run());
        });
    }

    /// Stops it.
    pub fn close(&self) {
        self.stop.cancel();
        if let Some(c) = self.conn() {
            c.close();
        }
        self.events.wake();
        self.inbox.wake();
    }

    fn conn(&self) -> Option<Conn> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_conn(&self, c: Option<Conn>) {
        *self.conn.lock().unwrap_or_else(|e| e.into_inner()) = c;
    }

    /// Whether it's attached to the broker now.
    pub fn connected(&self) -> bool {
        self.conn().is_some()
    }

    /// What plugins add now; don't change it.
    pub fn state(&self) -> Arc<UiState> {
        self.state.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Whether any plugin would hear an event of this kind, so the window
    /// needn't make one nobody hears.
    pub fn wants(&self, kind: &str) -> bool {
        if !self.connected() {
            return false;
        }
        let need = if kind.len() > 6 && kind.starts_with("input.") {
            UI_INPUT
        } else {
            UI_EVENTS
        };
        self.state()
            .plugins
            .iter()
            .any(|p| p.ui.iter().any(|u| u == need))
    }

    /// Whether any plugin is asked before a message goes.
    pub fn intercepts(&self) -> bool {
        if !self.connected() {
            return false;
        }
        self.state()
            .plugins
            .iter()
            .any(|p| p.ui.iter().any(|u| u == UI_INTERCEPT) && p.skipped.is_empty())
    }

    /// Hands the broker an event, without waiting: when the queue is full
    /// the event is dropped and counted.
    pub fn emit(&self, mut e: UiEvent) {
        if !self.wants(&e.kind) {
            return;
        }
        e.ui = self.id.clone();
        if e.at.is_zero() {
            e.at = rush_state::Time::now();
        }
        if !self.events.try_push(e, EVENT_QUEUE) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Events waiting to go, for tests.
    pub fn queued_events(&self) -> usize {
        self.events.lock().len()
    }

    /// Waits for the next thing the broker sent; None once closed. Call
    /// it again after each, off the window's own thread.
    pub fn next(&self) -> Option<Msg> {
        let mut q = self.inbox.lock();
        loop {
            if self.stopped.err().is_some() {
                return None;
            }
            if let Some(m) = q.pop_front() {
                if m == Msg::State {
                    self.state_queued.store(false, Ordering::SeqCst);
                }
                return Some(m);
            }
            q = self.inbox.cv.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Runs a plugin's command. box_id is whose message box has the keys:
    /// a session's id, or "" for the Prompt; input is that box, which only
    /// a plugin with "input" is shown.
    pub fn command(
        &self,
        name: &str,
        command: &str,
        s: Option<&UiSession>,
        box_id: &str,
        input: Option<&TextBox>,
    ) -> Result<(), String> {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(12));
        let q = json!({"plugin": name, "command": command, "ui": self.id,
            "session": s, "box": box_id, "input": input});
        self.call(&ctx, "ui.command", &q).map(|_| ())
    }

    /// Tells a plugin what was chosen from its pick.
    pub fn picked(&self, mut p: Picked) -> Result<(), String> {
        p.ui = self.id.clone();
        let ctx = Ctx::background().with_timeout(Duration::from_secs(12));
        self.call(&ctx, "ui.picked", &p).map(|_| ())
    }

    /// Asks the plugins about a message before it goes. It always has an
    /// answer: when they can't be asked, or don't answer in time, it's
    /// "allow".
    pub fn intercept(&self, mut req: Intercept) -> InterceptResult {
        req.ui = self.id.clone();
        let ctx = Ctx::background().with_timeout(INTERCEPT_BUDGET + Duration::from_millis(150));
        let allow = || InterceptResult {
            action: "allow".into(),
            ..Default::default()
        };
        let r = match self.call(&ctx, "ui.intercept", &req) {
            Err(e) => {
                if !self.intercept_failing.swap(true, Ordering::SeqCst) {
                    self.scope.log(
                        "plugin-intercept-failed",
                        crate::fields(&[("why", json!("allowed as it is")), ("err", json!(e))]),
                    );
                }
                return allow();
            }
            Ok(raw) => {
                rush_plugin::rpc::from_raw::<InterceptResult>(raw.as_deref()).unwrap_or_default()
            }
        };
        if self.intercept_failing.swap(false, Ordering::SeqCst) {
            self.scope
                .log("plugin-intercept-back", serde_json::Map::new());
        }
        match r.action.as_str() {
            "rewrite" | "block" => r,
            _ => allow(),
        }
    }

    /// Changes a plugin's setting.
    pub fn set_setting(&self, name: &str, key: &str, value: &str) -> Result<(), String> {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(5));
        let q = json!({"plugin": name, "key": key, "value": value});
        self.call(&ctx, "ui.settings.set", &q).map(|_| ())
    }

    fn call<P: Serialize + ?Sized>(
        &self,
        ctx: &Ctx,
        method: &str,
        params: &P,
    ) -> Result<Option<rush_plugin::Raw>, String> {
        let Some(c) = self.conn() else {
            return Err(OFFLINE.into());
        };
        let p = rush_plugin::rpc::to_raw(params).map_err(|e| e.message)?;
        c.call_raw(ctx, method, Some(p)).map_err(|e| e.to_string())
    }

    /// Keeps a connection to the broker, and sends it the events.
    fn run(self: Arc<Self>) {
        let mut wait = Duration::from_secs(1);
        let mut failed = String::new(); // the last failure logged, so a run of them is logged once
        loop {
            if self.stopped.err().is_some() {
                return;
            }
            let cn = match self.attach() {
                Ok(cn) => cn,
                Err(e) => {
                    if e != failed {
                        failed = e.clone();
                        self.scope.log(
                            "plugins-unreachable",
                            crate::fields(&[("ui", json!(self.id)), ("err", json!(e))]),
                        );
                    }
                    // Events meanwhile would be stale by the time they went.
                    self.events.lock().clear();
                    if !self.stopped.sleep(wait) {
                        return;
                    }
                    wait = (wait * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            wait = Duration::from_secs(1);
            failed.clear();
            self.scope
                .log("plugins-attached", crate::fields(&[("ui", json!(self.id))]));
            self.pump(&cn);
            self.scope
                .log("plugins-detached", crate::fields(&[("ui", json!(self.id))]));
            self.set_conn(None);
            self.set_state(UiState::default());
        }
    }

    /// Connects and says which window this is; the broker answers with what
    /// plugins add now.
    fn attach(self: &Arc<Self>) -> Result<Conn, String> {
        let me: Weak<Client> = Arc::downgrade(self);
        let h: Handler = Arc::new(move |_, _, method, params| {
            let Some(c) = me.upgrade() else {
                return Ok(crate::empty());
            };
            match method {
                "ui.state" => {
                    if let Ok(s) = rush_plugin::rpc::from_raw::<UiState>(params) {
                        c.set_state(s);
                    }
                }
                "ui.do" => {
                    if let Ok(d) = rush_plugin::rpc::from_raw::<UiDo>(params)
                        && (d.ui.is_empty() || d.ui == c.id)
                    {
                        c.push(Msg::Do(Box::new(d)));
                    }
                }
                _ => {}
            }
            Ok(crate::empty())
        });
        let cn = (self.dial)(h)?;
        let ctx = Ctx::background().with_timeout(Duration::from_secs(5));
        let s: Result<UiState, _> = cn.call(&ctx, "ui.attach", &json!({"ui": self.id}));
        let s = match s {
            Ok(s) => s,
            Err(e) => {
                cn.close();
                return Err(e.to_string());
            }
        };
        self.set_conn(Some(cn.clone()));
        self.attaches.fetch_add(1, Ordering::SeqCst);
        self.set_state(s);
        Ok(cn)
    }

    fn pump(self: &Arc<Self>, cn: &Conn) {
        let me = Arc::downgrade(self);
        let _reg = cn.ctx().on_cancel(move || {
            if let Some(c) = me.upgrade() {
                c.events.wake();
            }
        });
        loop {
            let e = {
                let mut q = self.events.lock();
                loop {
                    if self.stopped.err().is_some() || cn.is_done() {
                        return;
                    }
                    if let Some(e) = q.pop_front() {
                        break e;
                    }
                    q = self.events.cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            if cn.notify("ui.event", &e).is_err() {
                cn.close();
                return;
            }
        }
    }

    fn set_state(&self, s: UiState) {
        *self.state.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(s);
        self.push(Msg::State);
    }

    /// Hands the window a message without waiting. One Msg::State waiting
    /// says all there is to say, so another isn't queued behind it.
    fn push(&self, m: Msg) {
        let is_state = m == Msg::State;
        if is_state
            && self
                .state_queued
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return;
        }
        if !self.inbox.try_push(m, INBOX) && is_state {
            self.state_queued.store(false, Ordering::SeqCst);
        }
    }
}

/// Reaches the running broker, starting it if a plugin is enabled and it
/// isn't running.
fn dial_broker(h: Handler) -> Result<Conn, String> {
    if rush_plugin::enabled().is_empty() {
        return Err(OFFLINE.into());
    }
    let mut err = match rush_plugin::dial_broker_with(Some(h.clone())) {
        Ok(c) => return Ok(c),
        Err(e) => e.to_string(),
    };
    rush_plugin::ensure_broker()?;
    // It's starting: a few looks, a moment apart, for it to listen.
    let ctx = Ctx::background();
    for _ in 0..40 {
        ctx.sleep(Duration::from_millis(50));
        match rush_plugin::dial_broker_with(Some(h.clone())) {
            Ok(c) => return Ok(c),
            Err(e) => err = e.to_string(),
        }
    }
    Err(err)
}

/// Dials the broker only if one is running, never starting it: for a
/// rush that follows another, which leaves starting it to the one that
/// leads.
pub fn dial_running(h: Handler) -> Result<Conn, String> {
    if rush_plugin::enabled().is_empty() {
        return Err(OFFLINE.into());
    }
    rush_plugin::dial_broker_with(Some(h)).map_err(|e| e.to_string())
}

/// Tells the running broker an approval changed, or a bundled plugin was
/// turned on or off, starting it if nothing ran before. It dials once and
/// returns; run it off the window's thread.
pub fn reload() -> Result<(), String> {
    rush_plugin::client::reload()
}

#[cfg(test)]
mod tests;
