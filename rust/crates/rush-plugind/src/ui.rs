//! The broker stands between plugins and rush's screen. rush's windows
//! attach on broker.sock and tell it what happens there; it passes each
//! plugin what its manifest lets it hear, and passes back what plugins
//! add, checked and cleaned. Nothing here waits on a plugin or a window
//! while holding anything another depends on: every plugin and every
//! window has its own bounded queue, written from its own thread, and every
//! call to a plugin has a deadline.
//! Ported from internal/plugind/ui.go.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use rush_plugin::rpc::{CODE_INVALID_PARAMS, CODE_NO_METHOD, denied, limited};
use rush_plugin::text::go_q;
use rush_plugin::{
    CallError, Conn, Ctx, CtxErr, Error, INTERCEPT_BUDGET, INTERCEPT_EACH, INTERCEPT_STRIKE_OUT,
    Intercept, InterceptResult, MAX_PLUGIN_VIEWS, MAX_SECTIONS, MAX_SESSIONS_TRACKED, Pick, Picked,
    Plugin, Raw, Section, Status, TextBox, UI_INPUT, UI_INTERCEPT, UI_NOTIFY, UI_OVERVIEW, UI_SEND,
    UI_VIEWS, UiDo, UiEvent, UiPlugin, UiSection, UiSession, UiState, UiStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::value::RawValue;

use crate::broker::Inner;
use crate::runner::Runner;
use crate::sessions::MAX_TEXT;
use crate::{Scope, empty, fields, params, server, to_raw, within};

// Limits on the UI side of the broker.
/// notifications waiting for one plugin
pub(crate) const PLUGIN_QUEUE: usize = 256;
/// notifications waiting for one UI
const UI_QUEUE: usize = 64;
const STATE_PUSH_GAP: Duration = Duration::from_millis(100);
const COMMAND_WAIT: Duration = Duration::from_secs(10);
/// a plugin's notices refill at this rate …
const NOTIFY_EVERY: Duration = Duration::from_secs(1);
/// … up to this many
pub(crate) const NOTIFY_BURST: f64 = 3.0;
/// messages a plugin sends with ui.send, a minute
pub(crate) const SENDS_PER_MIN: usize = 10;
const MAX_SESSION_KEY: usize = 256;
/// How many pastes or images a box set by a plugin may hold.
const MAX_CHIPS: usize = 64;

/// Sends notifications on one connection from its own thread, so a peer
/// that stops reading holds up nothing but itself. When it's full the
/// oldest is dropped; a message with a key replaces the one waiting with
/// the same key, and goes last.
pub(crate) struct Outbox {
    st: Mutex<OutState>,
    cv: Condvar,
}

pub(crate) struct OutState {
    pub(crate) q: VecDeque<OutMsg>,
    max: usize,
    pub(crate) dropped: usize,
    done: bool,
}

pub(crate) struct OutMsg {
    method: String,
    pub(crate) params: Raw,
    key: String,
}

impl Outbox {
    /// An outbox that sends nothing until it's started: for tests.
    pub(crate) fn bare(max: usize) -> Arc<Outbox> {
        Arc::new(Outbox {
            st: Mutex::new(OutState {
                q: VecDeque::new(),
                max,
                dropped: 0,
                done: false,
            }),
            cv: Condvar::new(),
        })
    }

    /// An outbox sending on conn, until it's closed or conn is.
    pub(crate) fn new(conn: Conn, max: usize, scope: &Scope) -> Arc<Outbox> {
        let o = Outbox::bare(max);
        let w = Arc::downgrade(&o);
        let reg = conn.ctx().on_cancel(move || {
            if let Some(o) = w.upgrade() {
                o.close();
            }
        });
        let me = o.clone();
        scope.spawn("plugind-outbox", move || {
            let _reg = reg;
            me.run(&conn);
        });
        o
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, OutState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn put<T: Serialize + ?Sized>(&self, method: &str, params: &T, key: &str) {
        let Ok(p) = rush_plugin::rpc::to_raw(params) else {
            return;
        };
        let mut s = self.lock();
        if !key.is_empty() {
            s.q.retain(|m| m.key != key);
        }
        if s.q.len() >= s.max {
            s.q.pop_front();
            s.dropped += 1;
        }
        s.q.push_back(OutMsg {
            method: method.into(),
            params: p,
            key: key.into(),
        });
        drop(s);
        self.cv.notify_one();
    }

    fn run(&self, conn: &Conn) {
        loop {
            let m = {
                let mut s = self.lock();
                loop {
                    if s.done {
                        return;
                    }
                    if let Some(m) = s.q.pop_front() {
                        break m;
                    }
                    s = self.cv.wait(s).unwrap_or_else(|e| e.into_inner());
                }
            };
            if conn.notify_raw(&m.method, Some(m.params)).is_err() {
                return;
            }
        }
    }

    pub(crate) fn close(&self) {
        self.lock().done = true;
        self.cv.notify_all();
    }
}

/// A token bucket.
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn take(&mut self, now: Instant, every: Duration, burst: f64) -> bool {
        self.tokens = burst
            .min(self.tokens + now.duration_since(self.at).as_secs_f64() / every.as_secs_f64());
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// Calls a plugin, giving up at ctx's deadline even when the plugin has
/// stopped reading and the write itself would wait.
pub(crate) fn call_within(
    ctx: &Ctx,
    conn: &Conn,
    method: &str,
    params: Raw,
) -> Result<Option<Raw>, CallError> {
    let (tx, rx) = mpsc::channel();
    let (c, cx, m) = (conn.clone(), ctx.clone(), method.to_string());
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        let _ = tx2.send(c.call_raw(&cx, &m, Some(params)));
    });
    let wake = tx.clone();
    let _reg = ctx.on_cancel(move || {
        let _ = wake.send(Err(CallError::Ctx(CtxErr::Canceled)));
    });
    drop(tx);
    let got = match ctx.deadline() {
        None => rx.recv().map_err(|_| CallError::Closed),
        Some(d) => rx
            .recv_timeout(d.saturating_duration_since(Instant::now()))
            .map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => CallError::Ctx(CtxErr::DeadlineExceeded),
                mpsc::RecvTimeoutError::Disconnected => CallError::Closed,
            }),
    };
    match got {
        Ok(r) => r,
        Err(e) => Err(e),
    }
}

fn bad_params(msg: impl Into<String>) -> Error {
    Error::new(CODE_INVALID_PARAMS, msg)
}

/// One attached rush window.
struct UiConn {
    name: String,
    out: Arc<Outbox>,
    /// Takes it away when its connection closes.
    _gone: rush_plugin::ctx::Registration,
}

/// What the broker holds of rush's screens.
pub(crate) struct UiHub {
    b: Weak<Inner>,
    st: Mutex<HubState>,
}

/// What it holds by plugin, then by session: session, then plugin.
type BySession<V> = BTreeMap<String, BTreeMap<String, V>>;

#[derive(Default)]
struct HubState {
    uis: HashMap<u64, UiConn>,
    /// seen in events, by rush id
    sessions: HashMap<String, UiSession>,
    /// sessions oldest first, to forget
    order: VecDeque<String>,
    sections: BySession<Vec<Section>>,
    statuses: BySession<Status>,
    /// by session, "" the Prompt
    notes: BySession<Status>,
    strikes: HashMap<String, u32>,
    skipped: HashMap<String, String>,
    notices: HashMap<String, Bucket>,
    sends: HashMap<String, Vec<Instant>>,
    /// When each plugin was last run by a UI, a command or a choice, by
    /// UI: what lets it show that UI a pick.
    asked: HashMap<String, HashMap<String, Instant>>,
    pushing: bool,
    last_push: Option<Instant>,
}

/// The params of a plugin's ui.* calls.
#[derive(Default, Deserialize)]
#[serde(default)]
struct UiParams {
    ui: String,
    session: String,
    sections: Option<Vec<Section>>,
    text: String,
    tone: String,
    #[serde(rename = "box")]
    input_box: Option<TextBox>,
    #[serde(rename = "if")]
    if_text: Option<String>,
    pick: Option<Pick>,
    view: Option<crate::host::View>,
    name: String,
}

/// The capability each of a plugin's ui.* calls needs.
fn ui_needs(method: &str) -> Option<&'static str> {
    Some(match method {
        "ui.overview.set" | "ui.status.set" => UI_OVERVIEW,
        "ui.notify" => UI_NOTIFY,
        "ui.input.set" | "ui.box.note" => UI_INPUT,
        "ui.pick" | "ui.settings.get" => "",
        "ui.send" => UI_SEND,
        "ui.view.set" | "ui.view.clear" => UI_VIEWS,
        _ => return None,
    })
}

/// A ui.command as it goes to the plugin.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Command {
    plugin: String,
    command: String,
    ui: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<UiSession>,
    /// Whose message box has the keys: a session's id, or "" for the
    /// Prompt; input is that box, for "input" only.
    #[serde(rename = "box")]
    box_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<TextBox>,
}

impl UiHub {
    pub(crate) fn new(b: Weak<Inner>) -> UiHub {
        UiHub {
            b,
            st: Mutex::new(HubState::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn broker(&self) -> Option<Arc<Inner>> {
        self.b.upgrade()
    }

    fn runner(&self, name: &str) -> Result<(Arc<Runner>, Plugin, Conn), Error> {
        let r = self.broker().and_then(|b| b.runner(name));
        let not = || server(format!("{name} is not running"));
        let r = r.ok_or_else(not)?;
        let (m, conn) = r.live();
        let conn = conn.ok_or_else(not)?;
        Ok((r, m, conn))
    }

    /// Answers a UI's ui.* messages on broker.sock.
    pub(crate) fn answer_ui(
        &self,
        ctx: &Ctx,
        id: u64,
        conn: &Conn,
        method: &str,
        p: Option<&RawValue>,
    ) -> Result<Option<Raw>, Error> {
        match method {
            "ui.attach" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    ui: String,
                }
                let p: P = params(p)?;
                self.attach(id, conn, &p.ui);
                Ok(Some(to_raw(&self.state())))
            }
            "ui.event" => {
                let ev: UiEvent = params(p)?;
                self.event(ev);
                Ok(empty())
            }
            "ui.command" => {
                let mut p: Command = params(p)?;
                let (_, m, conn) = self.runner(&p.plugin)?;
                if !m.commands.iter().any(|c| c.name == p.command) {
                    return Err(bad_params(format!(
                        "{} has no command {}",
                        p.plugin, p.command
                    )));
                }
                if !m.can_ui(UI_INPUT) {
                    p.input = None;
                }
                self.ask(&p.plugin, &p.ui);
                let ctx = ctx.with_timeout(COMMAND_WAIT);
                call_within(&ctx, &conn, "ui.command", to_raw(&p)).map_err(crate::call_err)?;
                Ok(empty())
            }
            "ui.picked" => {
                let mut p: Picked = params(p)?;
                let (_, m, conn) = self.runner(&p.plugin)?;
                if !m.can_ui(UI_INPUT) {
                    p.input = None;
                }
                self.ask(&p.plugin, &p.ui);
                let ctx = ctx.with_timeout(COMMAND_WAIT);
                call_within(&ctx, &conn, "ui.picked", to_raw(&p)).map_err(crate::call_err)?;
                Ok(empty())
            }
            "ui.intercept" => {
                let i: Intercept = params(p)?;
                Ok(Some(to_raw(&self.intercept(ctx, i))))
            }
            "ui.settings.set" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    plugin: String,
                    key: String,
                    value: String,
                }
                let p: P = params(p)?;
                rush_plugin::set_setting(&p.plugin, &p.key, &p.value).map_err(bad_params)?;
                if let Ok((r, m, _)) = self.runner(&p.plugin) {
                    let values = rush_plugin::setting_values_of(&m.manifest);
                    r.notify("ui.settings", &json!({"values": values}), "settings");
                }
                self.changed();
                Ok(empty())
            }
            _ => Err(Error::new(
                CODE_NO_METHOD,
                format!("method not found: {method}"),
            )),
        }
    }

    /// Takes connection id as UI name until it closes.
    fn attach(&self, id: u64, conn: &Conn, name: &str) {
        let scope = match self.broker() {
            Some(b) => b.scope.clone(),
            None => Scope::here(),
        };
        if let Some(old) = self.lock().uis.get_mut(&id) {
            old.name = name.into();
            return;
        }
        // Gone with its connection.
        let b = self.b.clone();
        let reg = conn.ctx().on_cancel(move || {
            if let Some(b) = b.upgrade() {
                b.ui.detach(id);
            }
        });
        let out = Outbox::new(conn.clone(), UI_QUEUE, &scope);
        self.lock().uis.insert(
            id,
            UiConn {
                name: name.into(),
                out,
                _gone: reg,
            },
        );
        if conn.is_done() {
            self.detach(id); // it went before it was taken
        }
    }

    fn detach(&self, id: u64) {
        let u = self.lock().uis.remove(&id);
        if let Some(u) = u {
            u.out.close();
        }
    }

    /// Pushes the state to every UI soon: at once if it hasn't lately, else
    /// once STATE_PUSH_GAP has passed since the last, however many changes
    /// came between.
    pub(crate) fn changed(&self) {
        let delay = {
            let mut s = self.lock();
            if s.pushing {
                return;
            }
            s.pushing = true;
            s.last_push.map_or(Duration::ZERO, |t| {
                STATE_PUSH_GAP.saturating_sub(t.elapsed())
            })
        };
        let b = self.b.clone();
        let scope = self.broker().map(|b| b.scope.clone());
        let push = move || {
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            if let Some(b) = b.upgrade() {
                b.ui.push();
            }
        };
        match scope {
            Some(s) => s.spawn("plugind-push", push),
            None => {
                std::thread::spawn(push);
            }
        }
    }

    fn push(&self) {
        {
            let mut s = self.lock();
            s.pushing = false;
            s.last_push = Some(Instant::now());
        }
        let st = self.state();
        let s = self.lock();
        for u in s.uis.values() {
            u.out.put("ui.state", &st, "state");
        }
    }

    /// Asks the UI named in d, or every UI, to do something now.
    fn do_(&self, d: UiDo) {
        let s = self.lock();
        for u in s.uis.values() {
            if d.ui.is_empty() || d.ui == u.name {
                u.out.put("ui.do", &d, "");
            }
        }
    }

    /// Notes that a UI just ran one of a plugin's commands, or chose from
    /// its pick.
    fn ask(&self, name: &str, ui: &str) {
        self.lock()
            .asked
            .entry(name.into())
            .or_default()
            .insert(ui.into(), Instant::now());
    }

    /// Every plugin up now, by name.
    fn running(&self) -> Vec<Arc<Runner>> {
        let Some(b) = self.broker() else {
            return Vec::new();
        };
        let mut rs: Vec<Arc<Runner>> = b
            .runners()
            .into_iter()
            .filter(|r| r.live().1.is_some())
            .collect();
        rs.sort_by(|a, b| a.name.cmp(&b.name));
        rs
    }

    /// What the UIs draw from.
    pub(crate) fn state(&self) -> UiState {
        let mut st = UiState::default();
        for r in self.running() {
            let (m, _) = r.live();
            if m.ui.is_empty() && m.commands.is_empty() && m.settings.is_empty() {
                continue;
            }
            st.plugins.push(UiPlugin {
                name: r.name.clone(),
                commands: m.commands.clone(),
                settings: m.settings.clone(),
                values: rush_plugin::setting_values_of(&m.manifest),
                ui: m.ui.clone(),
                skipped: String::new(),
            });
        }
        let s = self.lock();
        for p in &mut st.plugins {
            p.skipped = s.skipped.get(&p.name).cloned().unwrap_or_default();
        }
        for (sess, by) in &s.sections {
            for (name, secs) in by {
                for sec in secs {
                    st.sections
                        .entry(sess.clone())
                        .or_default()
                        .push(UiSection {
                            plugin: name.clone(),
                            section: sec.clone(),
                        });
                }
            }
        }
        for (sess, by) in &s.statuses {
            for (name, status) in by {
                st.statuses.entry(sess.clone()).or_default().push(UiStatus {
                    plugin: name.clone(),
                    status: status.clone(),
                });
            }
        }
        for (sess, by) in &s.notes {
            for (name, status) in by {
                st.notes.entry(sess.clone()).or_default().push(UiStatus {
                    plugin: name.clone(),
                    status: status.clone(),
                });
            }
        }
        st
    }

    /// Remembers the session an event is about, and passes the event on to
    /// each plugin that may hear it, without waiting for any.
    fn event(&self, ev: UiEvent) {
        if let Some(sess) = &ev.session
            && !sess.id.is_empty()
            && sess.id.len() <= MAX_SESSION_KEY
        {
            let mut s = self.lock();
            if !s.sessions.contains_key(&sess.id) {
                s.order.push_back(sess.id.clone());
            }
            s.sessions.insert(sess.id.clone(), sess.clone());
            while s.order.len() > MAX_SESSIONS_TRACKED {
                if let Some(id) = s.order.pop_front() {
                    s.sessions.remove(&id);
                }
            }
        }
        let mut key = String::new();
        if ev.kind == rush_plugin::EV_INPUT_CHANGED {
            let id = ev.session.as_ref().map_or("", |s| s.id.as_str());
            key = format!("input.changed\x00{}\x00{id}", ev.ui);
        }
        for r in self.running() {
            let (m, _) = r.live();
            if let Some(e) = ev.for_plugin(&m.manifest) {
                r.notify("ui.event", &e, &key);
            }
        }
    }

    /// Asks each plugin with "intercept", in name order, about a message
    /// before it goes. Each has INTERCEPT_EACH, and all INTERCEPT_BUDGET; a
    /// plugin that doesn't answer in time lets it go as it is, and one that
    /// fails to INTERCEPT_STRIKE_OUT times in a row isn't asked again until
    /// it restarts.
    fn intercept(&self, ctx: &Ctx, i: Intercept) -> InterceptResult {
        let ctx = ctx.with_timeout(INTERCEPT_BUDGET);
        let mut text = i.text.clone();
        let mut changed_by: Vec<String> = Vec::new();
        for r in self.running() {
            let (m, conn) = r.live();
            let Some(conn) = conn else { continue };
            if !m.can_ui(UI_INTERCEPT) {
                continue;
            }
            if self.lock().skipped.contains_key(&r.name) {
                continue;
            }
            if ctx.err().is_some() {
                break;
            }
            let each = ctx.with_timeout(INTERCEPT_EACH);
            let mut ask = i.clone();
            ask.text = text.clone();
            let began = Instant::now();
            let got = call_within(&each, &conn, "ui.intercept", to_raw(&ask))
                .map_err(|e| e.to_string())
                .and_then(|raw| rush_plugin::rpc::from_raw::<InterceptResult>(raw.as_deref()));
            let res = match got {
                Ok(res) => res,
                Err(e) => {
                    // Cut short only by what the others took isn't its fault.
                    if e != CtxErr::DeadlineExceeded.to_string()
                        || began.elapsed() >= INTERCEPT_EACH
                    {
                        self.strike(&r.name, &e);
                    }
                    continue;
                }
            };
            self.lock().strikes.remove(&r.name);
            match res.action.as_str() {
                "block" => {
                    let reason = rush_plugin::clean_notice(&res.reason);
                    self.intercept_event(
                        &i,
                        fields(&[
                            ("action", json!("block")),
                            ("plugin", json!(r.name)),
                            ("reason", json!(reason)),
                        ]),
                    );
                    return InterceptResult {
                        action: "block".into(),
                        plugin: r.name.clone(),
                        reason,
                        ..Default::default()
                    };
                }
                "rewrite" if res.text != text && res.text.len() <= MAX_TEXT => {
                    text = res.text;
                    changed_by.push(r.name.clone());
                }
                _ => {}
            }
        }
        if !changed_by.is_empty() && text != i.text {
            let by = changed_by.join(", ");
            self.intercept_event(
                &i,
                fields(&[
                    ("action", json!("rewrite")),
                    ("plugin", json!(by)),
                    ("charsWas", json!(i.text.len())),
                    ("chars", json!(text.len())),
                ]),
            );
            return InterceptResult {
                action: "rewrite".into(),
                text,
                plugin: by,
                ..Default::default()
            };
        }
        InterceptResult {
            action: "allow".into(),
            ..Default::default()
        }
    }

    fn strike(&self, name: &str, err: &str) {
        let out = {
            let mut s = self.lock();
            let n = s.strikes.entry(name.into()).or_default();
            *n += 1;
            let out = *n >= INTERCEPT_STRIKE_OUT;
            if out {
                s.skipped.insert(
                    name.into(),
                    format!(
                        "didn't answer {INTERCEPT_STRIKE_OUT} times in a row ({err}); skipped until it restarts"
                    ),
                );
            }
            out
        };
        if out {
            if let Some(b) = self.broker() {
                b.scope.log(
                    "plugin-skipped",
                    fields(&[
                        ("plugin", json!(name)),
                        ("why", json!("intercept timed out")),
                        ("err", json!(err)),
                    ]),
                );
            }
            self.changed();
        }
    }

    /// Logs what plugins did to a message, in its session's events when
    /// it's for one.
    fn intercept_event(&self, i: &Intercept, mut f: rush_events::Fields) {
        f.insert("hook".into(), json!(i.hook));
        let Some(b) = self.broker() else { return };
        match &i.session {
            Some(s) if !s.id.is_empty() => b.hosts.log_event(&s.id, "plugin-intercept", f),
            _ => b.scope.log("plugin-intercept", f),
        }
    }

    /// Forgets what a plugin that stopped added, and its strikes.
    pub(crate) fn gone(&self, name: &str) {
        {
            let mut s = self.lock();
            forget(&mut s.sections, name);
            forget(&mut s.statuses, name);
            forget(&mut s.notes, name);
            s.strikes.remove(name);
            s.skipped.remove(name);
            s.asked.remove(name);
        }
        self.changed();
    }

    /// Answers a plugin's ui.* calls, each checked against its approved
    /// manifest.
    pub(crate) fn answer_plugin(
        &self,
        p: &Plugin,
        method: &str,
        params: Option<&RawValue>,
    ) -> Result<Option<Raw>, Error> {
        let Some(c) = ui_needs(method) else {
            return Err(Error::new(
                CODE_NO_METHOD,
                format!("method not found: {method}"),
            ));
        };
        if !c.is_empty() && !p.can_ui(c) {
            return Err(denied(&format!(
                "{method} needs {} in the manifest's ui",
                go_q(c)
            )));
        }
        let given = params.filter(|p| !p.get().trim().is_empty());
        let mut i: UiParams = match given {
            Some(_) => crate::params(given)?,
            None => UiParams::default(),
        };
        match method {
            "ui.overview.set" => self.set_overview(&p.name, &mut i)?,
            "ui.status.set" => self.set_status(&p.name, &i)?,
            "ui.notify" => self.notice(&p.name, &i)?,
            "ui.input.set" => self.set_input(&p.name, i)?,
            "ui.box.note" => self.set_note(&p.name, &i)?,
            "ui.pick" => self.pick(&p.name, i)?,
            "ui.send" => self.send(p, &i)?,
            "ui.view.set" => self.set_view(&p.name, i)?,
            "ui.view.clear" => self.clear_view(&p.name, &i)?,
            "ui.settings.get" => {
                let values = rush_plugin::setting_values_of(&p.manifest);
                return Ok(Some(to_raw(&json!({"values": values}))));
            }
            _ => {}
        }
        Ok(empty())
    }

    /// Replaces a plugin's sections on a session; none removes them.
    fn set_overview(&self, name: &str, i: &mut UiParams) -> Result<(), Error> {
        session_key(&i.session)?;
        let secs = i.sections.take().unwrap_or_default();
        if secs.len() > MAX_SECTIONS {
            return Err(bad_params(format!("at most {MAX_SECTIONS} sections")));
        }
        let mut clean = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for s in secs {
            let c = rush_plugin::clean_section(s).map_err(bad_params)?;
            if !seen.insert(c.id.clone()) {
                return Err(bad_params(format!("section id {} twice", c.id)));
            }
            clean.push(c);
        }
        let remove = clean.is_empty();
        put(&mut self.lock().sections, &i.session, name, clean, remove)?;
        self.changed();
        Ok(())
    }

    /// Draws a plugin's view beside a session's conversation, kept apart
    /// from the agent's and other plugins' by its plugin's name, which the
    /// panel shows.
    fn set_view(&self, name: &str, i: UiParams) -> Result<(), Error> {
        session_key(&i.session)?;
        let Some(mut v) = i.view else {
            return Err(bad_params(
                r#"no view: {"session": …, "view": {"name": "ci", "title": …}}"#,
            ));
        };
        let vname = v.get("name").and_then(|n| n.as_str()).unwrap_or_default();
        if !view_part_ok(vname) {
            return Err(bad_params(
                "a view's name is lowercase letters, digits and dashes, up to 24: like ci",
            ));
        }
        let full = plugin_view(name, vname);
        v.insert("name".into(), json!(full));
        v.insert("by".into(), json!(name));
        let Some(b) = self.broker() else {
            return Err(server("rush's screen isn't reachable"));
        };
        let field = |o: &crate::host::View, k: &str| {
            o.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let mine = b
            .hosts
            .views(&i.session)
            .iter()
            .filter(|o| field(o, "by") == name && field(o, "name") != full)
            .count();
        if mine >= MAX_PLUGIN_VIEWS {
            return Err(bad_params(format!(
                "a plugin keeps at most {MAX_PLUGIN_VIEWS} views on a session: clear one first"
            )));
        }
        b.hosts.save_view(&i.session, v).map_err(bad_params)
    }

    /// Takes a plugin's view away; none is no error.
    fn clear_view(&self, name: &str, i: &UiParams) -> Result<(), Error> {
        session_key(&i.session)?;
        if !view_part_ok(&i.name) {
            return Err(bad_params("bad view name"));
        }
        let Some(b) = self.broker() else {
            return Err(server("rush's screen isn't reachable"));
        };
        b.hosts
            .drop_view(&i.session, &plugin_view(name, &i.name))
            .map_err(server)
    }

    /// Replaces a plugin's status on a session; no text removes it.
    fn set_status(&self, name: &str, i: &UiParams) -> Result<(), Error> {
        session_key(&i.session)?;
        let st = rush_plugin::clean_status(Status {
            text: i.text.clone(),
            tone: i.tone.clone(),
        });
        let remove = st.text.trim().is_empty();
        put(&mut self.lock().statuses, &i.session, name, st, remove)?;
        self.changed();
        Ok(())
    }

    /// Shows a short message, at most NOTIFY_BURST at once and one a second
    /// after.
    fn notice(&self, name: &str, i: &UiParams) -> Result<(), Error> {
        let text = rush_plugin::clean_notice(&i.text);
        if text.trim().is_empty() {
            return Err(bad_params("text is empty"));
        }
        let now = Instant::now();
        let ok = self
            .lock()
            .notices
            .entry(name.into())
            .or_insert(Bucket {
                tokens: NOTIFY_BURST,
                at: now,
            })
            .take(now, NOTIFY_EVERY, NOTIFY_BURST);
        if !ok {
            return Err(limited("notices, one a second"));
        }
        let tone = rush_plugin::clean_status(Status {
            tone: i.tone.clone(),
            ..Default::default()
        })
        .tone;
        self.do_(UiDo {
            plugin: name.into(),
            ui: i.ui.clone(),
            kind: "notify".into(),
            session: i.session.clone(),
            text,
            tone,
            ..Default::default()
        });
        Ok(())
    }

    /// Sets what's in a message box.
    fn set_input(&self, name: &str, i: UiParams) -> Result<(), Error> {
        if !i.session.is_empty() {
            session_key(&i.session)?;
        }
        if i.text.len() > MAX_TEXT {
            return Err(bad_params("text is too long"));
        }
        if let Some(b) = &i.input_box {
            if TextBox::size(Some(b)) > 4 * MAX_TEXT
                || b.cursor < 0
                || b.pastes.len() > MAX_CHIPS
                || b.images.len() > MAX_CHIPS
            {
                return Err(bad_params("box is too big"));
            }
            if b.images.values().any(|p| !p.starts_with('/')) {
                return Err(bad_params("an image is a file's absolute path"));
            }
        }
        self.do_(UiDo {
            plugin: name.into(),
            ui: i.ui,
            kind: "input.set".into(),
            session: i.session,
            text: i.text,
            input_box: i.input_box,
            if_text: i.if_text,
            ..Default::default()
        });
        Ok(())
    }

    /// Shows a UI a list to choose from, only in answer to the plugin's
    /// command or pick there, within COMMAND_WAIT of it.
    fn pick(&self, name: &str, i: UiParams) -> Result<(), Error> {
        let Some(pick) = i.pick else {
            return Err(bad_params("no pick"));
        };
        let at = self
            .lock()
            .asked
            .get(name)
            .and_then(|m| m.get(&i.ui))
            .copied();
        if at.is_none_or(|at| at.elapsed() > COMMAND_WAIT) {
            return Err(denied(
                "a pick is shown only in answer to the plugin's command, run in that window",
            ));
        }
        let p = rush_plugin::clean_pick(pick).map_err(bad_params)?;
        if !p.session.is_empty() {
            session_key(&p.session)?;
        }
        self.do_(UiDo {
            plugin: name.into(),
            ui: i.ui,
            kind: "pick".into(),
            session: p.session.clone(),
            pick: Some(p),
            ..Default::default()
        });
        Ok(())
    }

    /// Replaces a plugin's note on the edge of a session's message box, or
    /// the Prompt's for no session; no text removes it.
    fn set_note(&self, name: &str, i: &UiParams) -> Result<(), Error> {
        if !i.session.is_empty() {
            session_key(&i.session)?;
        }
        let st = rush_plugin::clean_note(Status {
            text: i.text.clone(),
            tone: i.tone.clone(),
        });
        let remove = st.text.trim().is_empty();
        put(&mut self.lock().notes, &i.session, name, st, remove)?;
        self.changed();
        Ok(())
    }

    /// Has a UI send a message as you would: only to a session seen in
    /// rush's screen, in one of the plugin's workspaces, SENDS_PER_MIN a
    /// minute.
    fn send(&self, p: &Plugin, i: &UiParams) -> Result<(), Error> {
        session_key(&i.session)?;
        if i.text.trim().is_empty() || i.text.len() > MAX_TEXT {
            return Err(bad_params("text is empty or too long"));
        }
        let Some(s) = self.lock().sessions.get(&i.session).cloned() else {
            return Err(denied(&format!(
                "session {} hasn't been seen in rush's screen",
                i.session
            )));
        };
        let inside = !s.cwd.is_empty()
            && rush_plugin::eval_symlinks(&s.cwd)
                .is_some_and(|d| p.workspace_dirs().iter().any(|w| within(&d, w)));
        if !inside {
            return Err(denied(&format!(
                "session {} is outside the plugin's workspaces",
                i.session
            )));
        }
        let now = Instant::now();
        {
            let mut st = self.lock();
            let recent = st.sends.entry(p.name.clone()).or_default();
            recent.retain(|t| now.duration_since(*t) <= Duration::from_secs(60));
            if recent.len() >= SENDS_PER_MIN {
                return Err(limited(&format!(
                    "it has sent {SENDS_PER_MIN} messages this minute"
                )));
            }
            recent.push(now);
        }
        self.do_(UiDo {
            plugin: p.name.clone(),
            ui: i.ui.clone(),
            kind: "send".into(),
            session: i.session.clone(),
            text: i.text.clone(),
            ..Default::default()
        });
        Ok(())
    }
}

/// Forgets what plugin name has on every session in m.
fn forget<V>(m: &mut BySession<V>, name: &str) {
    m.retain(|_, by| {
        by.remove(name);
        !by.is_empty()
    });
}

/// Checks a session id a plugin names.
fn session_key(s: &str) -> Result<(), Error> {
    if s.is_empty() || s.len() > MAX_SESSION_KEY || s.chars().any(|c| c < '\x20' || c == '\x7f') {
        return Err(bad_params("bad session id"));
    }
    Ok(())
}

/// ^[a-z0-9][a-z0-9-]{0,23}$: what a plugin names its view.
fn view_part_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 24
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// rush keeps a plugin's view as p-<plugin>--<name>, a name the agent's
/// rush view set can't take.
fn plugin_view(plugin: &str, name: &str) -> String {
    format!("p-{plugin}--{name}")
}

/// Sets or removes what a plugin has on a session, keeping it to
/// MAX_SESSIONS_TRACKED sessions.
fn put<V>(
    m: &mut BySession<V>,
    session: &str,
    name: &str,
    v: V,
    remove: bool,
) -> Result<(), Error> {
    if remove {
        if let Some(by) = m.get_mut(session) {
            by.remove(name);
            if by.is_empty() {
                m.remove(session);
            }
        }
        return Ok(());
    }
    let has = m.get(session).is_some_and(|by| by.contains_key(name));
    if !has && count_sessions(m, name) >= MAX_SESSIONS_TRACKED {
        return Err(denied(&format!(
            "it has something on {MAX_SESSIONS_TRACKED} sessions already"
        )));
    }
    m.entry(session.into()).or_default().insert(name.into(), v);
    Ok(())
}

/// How many sessions a plugin has something on in m.
fn count_sessions<V>(m: &BySession<V>, name: &str) -> usize {
    m.values().filter(|by| by.contains_key(name)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_coalesces_and_drops_oldest() {
        let o = Outbox::bare(3);
        o.put("ui.event", "a", "k");
        o.put("ui.event", "b", "");
        o.put("ui.event", "c", "k"); // replaces a, goes last
        {
            let s = o.lock();
            let got: Vec<&str> = s.q.iter().map(|m| m.params.get()).collect();
            assert_eq!(got, [r#""b""#, r#""c""#]);
        }
        o.put("ui.event", "d", "");
        o.put("ui.event", "e", "");
        let s = o.lock();
        assert_eq!(s.q.len(), 3);
        assert_eq!(s.q[0].params.get(), r#""c""#);
        assert_eq!(s.dropped, 1);
    }

    #[test]
    fn keys_and_names() {
        assert!(session_key("s1").is_ok());
        assert!(session_key("").is_err() && session_key("a\nb").is_err());
        assert!(view_part_ok("ci") && view_part_ok("two-3"));
        assert!(!view_part_ok("Bad Name") && !view_part_ok("-x"));
        assert_eq!(plugin_view("ci", "ci"), "p-ci--ci");
    }

    #[test]
    fn bucket_refills() {
        let t = Instant::now();
        let mut k = Bucket { tokens: 3.0, at: t };
        for _ in 0..3 {
            assert!(k.take(t, NOTIFY_EVERY, NOTIFY_BURST));
        }
        assert!(!k.take(t, NOTIFY_EVERY, NOTIFY_BURST));
        assert!(k.take(t + Duration::from_secs(1), NOTIFY_EVERY, NOTIFY_BURST));
    }
}
