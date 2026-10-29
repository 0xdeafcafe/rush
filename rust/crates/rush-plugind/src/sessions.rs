//! What a plugin asks of rush about sessions, each call checked against
//! what it was approved for: listing and watching them, starting them,
//! queueing to them, sending to, stopping and following the ones it
//! started, and its own log, services, sidebar and programs.
//! Ported from internal/plugind/sessions.go.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use rush_core::event::{Event, Message, PartKind};
use rush_plugin::rpc::{CODE_INVALID_PARAMS, CODE_NO_METHOD, denied};
use rush_plugin::text::go_q;
use rush_plugin::{CAP_CONTROL, CAP_LIST, CAP_QUEUE, CAP_QUEUED, CAP_READ, CAP_SEND, CAP_START};
use rush_plugin::{Ctx, Error, Plugin, Raw};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::host::{HostClient, HostEvent, Hosts, Info, Op, SpawnConfig};
use crate::runner::{Runner, Sub};
use crate::{empty, fields, or, server, to_raw, within};

// Limits on how much a plugin can start, so a runaway one can't spend your
// usage for you.
/// its sessions running at once
const MAX_LIVE: usize = 4;
/// sessions it starts in an hour
const MAX_STARTS_PER_HR: usize = 30;
pub(crate) const MAX_TEXT: usize = 100 << 10;

/// What a plugin is told about a session: what the list shows, never what
/// was said.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub id: String,
    /// Claude Code's id for the conversation, the one its transcript and
    /// hooks go by: how other tools know the session.
    #[serde(rename = "sessionId", skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub cwd: String,
    /// the checkout cwd is in
    #[serde(skip_serializing_if = "String::is_empty")]
    pub repo: String,
    /// what it has checked out
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(skip_serializing_if = "is_false")]
    pub worktree: bool,
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub needs: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(rename = "permissionMode", skip_serializing_if = "String::is_empty")]
    pub permission_mode: String,
    #[serde(rename = "costUsd", skip_serializing_if = "is_zero_f")]
    pub cost_usd: f64,
    #[serde(rename = "contextTokens", skip_serializing_if = "is_zero")]
    pub context_tokens: i64,
    /// messages waiting for its turn to end
    #[serde(skip_serializing_if = "is_zero")]
    pub queued: i64,
    #[serde(rename = "startedBy", skip_serializing_if = "String::is_empty")]
    pub started_by: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, String>,
    #[serde(rename = "startedAt")]
    pub started_at: rush_state::Time,
    #[serde(rename = "updatedAt")]
    pub updated_at: rush_state::Time,
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(n: &i64) -> bool {
    *n == 0
}
fn is_zero_f(n: &f64) -> bool {
    *n == 0.0
}

/// A session as a plugin sees it.
pub(crate) fn session_of(i: &Info) -> Session {
    let mut state = i.state.clone();
    if !crate::host::alive(i.host_pid) {
        state = "stopped".into();
    }
    let g = crate::watch::git_of(&i.cwd);
    Session {
        id: i.id.clone(),
        session_id: i.session_id.clone(),
        name: i.name.clone(),
        cwd: i.cwd.clone(),
        repo: g.repo,
        branch: g.branch,
        worktree: g.worktree,
        state,
        detail: i.detail.clone(),
        needs: i.needs.clone(),
        model: i.model.clone(),
        permission_mode: i.permission_mode.clone(),
        cost_usd: i.cost_usd,
        context_tokens: i.context_tokens,
        queued: i.queue.len() as i64,
        started_by: i.started_by.clone(),
        meta: i.meta.clone(),
        started_at: i.started_at,
        updated_at: i.updated_at,
    }
}

/// ^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$: a session id as rush makes them. An
/// id becomes a path, so nothing else gets near the filesystem.
pub(crate) fn id_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
}

/// ^[A-Za-z0-9._\[\]-]{1,64}$
fn model_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._[]-".contains(&c))
}

/// ^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$: a branch or commit a plugin may
/// name: nothing git could take for an option.
fn ref_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 100
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(c))
}

/// ^[A-Za-z][A-Za-z0-9_.-]{0,63}$
fn meta_key_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphabetic()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(c))
}

/// The permission modes that ask you before anything your settings don't
/// already allow: the only ones a plugin may start a session in, or queue
/// to one in.
const SAFE_MODES: [&str; 3] = ["default", "acceptEdits", "plan"];

/// Asks for the session to run in a new git worktree of the checkout cwd
/// is in.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(crate) struct WorktreeReq {
    /// its folder, under .claude/worktrees
    pub(crate) name: String,
    /// the new branch; worktree-<name> if ""
    pub(crate) branch: String,
    /// what the branch starts from; HEAD if ""
    pub(crate) base: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StartReq {
    pub(crate) cwd: String,
    pub(crate) prompt: String,
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) effort: String,
    pub(crate) mode: String,
    pub(crate) worktree: Option<WorktreeReq>,
    pub(crate) meta: BTreeMap<String, String>,
}

/// The params of a plugin's calls.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct In {
    id: String,
    text: String,
    now: bool,
    cwd: String,
    prompt: String,
    name: String,
    model: String,
    effort: String,
    #[serde(rename = "permissionMode")]
    permission_mode: String,
    worktree: Option<WorktreeReq>,
    meta: Option<BTreeMap<String, String>>,
    message: String,
    args: Option<Vec<String>>,
    stdin: String,
    index: i64,
    was: String,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::new(CODE_INVALID_PARAMS, msg)
}

fn eval(p: &str) -> Option<String> {
    rush_plugin::eval_symlinks(p)
}

impl Runner {
    /// Answers the plugin's own calls, each checked against what it was
    /// approved for. Sending, reading and control reach only sessions it
    /// started: another session may run with permissions it was never
    /// given.
    pub(crate) fn answer_plugin(
        self: &Arc<Self>,
        ctx: &Ctx,
        method: &str,
        params: Option<&RawValue>,
    ) -> Result<Option<Raw>, Error> {
        let p = self.lock().p.clone();
        let need = |c: &str| -> Result<(), Error> {
            if p.can(c) {
                Ok(())
            } else {
                Err(denied(&format!(
                    "{method} needs the {} capability",
                    go_q(c)
                )))
            }
        };
        if method.starts_with("ui.") {
            let Some(b) = self.broker() else {
                return Err(server("rush's screen isn't reachable"));
            };
            return b.0.ui.answer_plugin(&p, method, params);
        }
        let given = params.filter(|p| !p.get().trim().is_empty());
        let inp: In = match given {
            Some(_) => crate::params(given)?,
            None => In::default(),
        };
        // What it did to a session goes in that session's events.
        let noted = |res: Result<Option<Raw>, Error>| {
            let mut f = fields(&[("plugin", json!(p.name)), ("op", json!(method))]);
            if let Err(e) = &res {
                f.insert("err".into(), json!(e.message));
            }
            if !inp.id.is_empty() {
                self.hosts.log_event(&inp.id, "plugin-op", f);
            }
            res
        };
        match method {
            "log" => {
                let mut msg = inp.message.clone();
                if msg.len() > 1024 {
                    let mut cut = 1024;
                    while !msg.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    msg.truncate(cut);
                    msg.push('…');
                }
                self.log.print(&format!("says: {}", msg.replace('\n', " ")));
                Ok(empty())
            }
            "services.ask" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Q {
                    service: String,
                }
                let q: Q = crate::params(params)?;
                if !p.uses.contains(&q.service) {
                    return Err(denied(&format!(
                        "services.ask needs {} in uses",
                        go_q(&q.service)
                    )));
                }
                let Some(b) = self.broker() else {
                    return Err(server("no broker"));
                };
                Ok(Some(to_raw(&b.ask(ctx, &q.service, &self.name))))
            }
            "sessions.list" => {
                need(CAP_LIST)?;
                let out: Vec<Session> = self.hosts.list().iter().map(session_of).collect();
                Ok(Some(to_raw(&out)))
            }
            "sessions.watch" => {
                need(CAP_LIST)?;
                self.watch_list()
            }
            "sessions.unwatch" => {
                if let Some(u) = self.lock().unwatch.take() {
                    u.cancel();
                }
                Ok(empty())
            }
            "sessions.start" => {
                need(CAP_START)?;
                self.start(
                    &p,
                    StartReq {
                        cwd: inp.cwd.clone(),
                        prompt: inp.prompt.clone(),
                        name: inp.name.clone(),
                        model: inp.model.clone(),
                        effort: inp.effort.clone(),
                        mode: inp.permission_mode.clone(),
                        worktree: inp.worktree.clone(),
                        meta: inp.meta.clone().unwrap_or_default(),
                    },
                )
            }
            "sessions.queue" => {
                need(CAP_QUEUE)?;
                self.queueable(&p, &inp.id)?;
                if inp.text.trim().is_empty() || inp.text.len() > MAX_TEXT {
                    return Err(invalid("text is empty or too long"));
                }
                // Said to be from the plugin, so neither you nor Claude
                // takes it for yours.
                let text = format!("[from the rush plugin {}]\n{}", p.name, inp.text);
                noted(with_host(&*self.hosts, &inp.id, |c| {
                    c.op(Op::send(&text, false))
                }))
            }
            "sessions.queued.send" | "sessions.queued.remove" => {
                need(CAP_QUEUED)?;
                self.in_workspaces(&p, &inp.id)?;
                if inp.was.len() > MAX_TEXT {
                    return Err(invalid("was is too long"));
                }
                if !inp.was.is_empty() && !p.bundled {
                    // Naming a message by its text would let a plugin test
                    // guesses at what's queued, which it's never to see.
                    return Err(denied(
                        "only rush's own plugins name a queued message by its text",
                    ));
                }
                noted(queued(
                    &*self.hosts,
                    &inp.id,
                    inp.index,
                    &inp.was,
                    method == "sessions.queued.send",
                ))
            }
            "sidebar.set" => self.set_sidebar(&p, params),
            "exec" => self.exec(
                ctx,
                &p,
                &inp.name,
                inp.args.as_deref().unwrap_or_default(),
                &inp.stdin,
                &inp.cwd,
            ),
            "sessions.send" => {
                need(CAP_SEND)?;
                self.own(&p, &inp.id)?;
                if inp.text.trim().is_empty() || inp.text.len() > MAX_TEXT {
                    return Err(invalid("text is empty or too long"));
                }
                noted(with_host(&*self.hosts, &inp.id, |c| {
                    c.op(Op::send(&inp.text, inp.now))
                }))
            }
            "sessions.interrupt" | "sessions.stop" => {
                need(CAP_CONTROL)?;
                self.own(&p, &inp.id)?;
                noted(with_host(&*self.hosts, &inp.id, |c| {
                    c.op(if method == "sessions.stop" {
                        Op::stop()
                    } else {
                        Op::interrupt()
                    })
                }))
            }
            "sessions.subscribe" => {
                need(CAP_READ)?;
                self.own(&p, &inp.id)?;
                self.subscribe(&inp.id).map_err(server)?;
                Ok(empty())
            }
            "sessions.unsubscribe" => {
                if let Some(s) = self.lock().subs.remove(&inp.id) {
                    s.cancel.cancel();
                }
                Ok(empty())
            }
            _ => Err(Error::new(
                CODE_NO_METHOD,
                format!("method not found: {method}"),
            )),
        }
    }

    /// Keeps how the plugin arranges rush's agent list, once it is checked
    /// and cleaned, where the UI reads it and the plugin can't write.
    fn set_sidebar(&self, p: &Plugin, params: Option<&RawValue>) -> Result<Option<Raw>, Error> {
        if !p.sidebar {
            return Err(denied(r#"sidebar.set needs "sidebar" in the manifest"#));
        }
        let s = rush_plugin::sidebar::parse_sidebar(&p.name, params).map_err(invalid)?;
        rush_plugin::sidebar::save_sidebar(s).map_err(server)?;
        Ok(empty())
    }

    /// Checks the session is one the plugin started.
    pub(crate) fn own(&self, p: &Plugin, id: &str) -> Result<(), Error> {
        if !id_ok(id) {
            return Err(invalid("bad session id"));
        }
        let info = self
            .hosts
            .read_info(id)
            .map_err(|_| invalid(format!("no session {id}")))?;
        if info.started_by != p.name {
            return Err(denied(&format!(
                "session {id} was not started by {}",
                p.name
            )));
        }
        Ok(())
    }

    /// Checks a plugin may queue to the session: one running in its
    /// workspaces, in a mode that still asks you.
    pub(crate) fn queueable(&self, p: &Plugin, id: &str) -> Result<(), Error> {
        let info = self.in_workspaces(p, id)?;
        if info.started_by == p.name {
            return Ok(());
        }
        let m = &info.permission_mode;
        if !m.is_empty() && !SAFE_MODES.contains(&m.as_str()) {
            return Err(denied(&format!(
                "session {id} runs in {m} mode, which doesn't ask you first"
            )));
        }
        Ok(())
    }

    /// Checks the session is one the plugin started, or runs in its
    /// workspaces.
    pub(crate) fn in_workspaces(&self, p: &Plugin, id: &str) -> Result<Info, Error> {
        if !id_ok(id) {
            return Err(invalid("bad session id"));
        }
        let info = self
            .hosts
            .read_info(id)
            .map_err(|_| invalid(format!("no session {id}")))?;
        if info.started_by == p.name {
            return Ok(info);
        }
        let inside =
            eval(&info.cwd).is_some_and(|dir| p.workspace_dirs().iter().any(|w| within(&dir, w)));
        if !inside {
            return Err(denied(&format!(
                "session {id} is outside the plugin's workspaces"
            )));
        }
        Ok(info)
    }

    /// Starts a session for the plugin: in one of its workspaces, on your
    /// account and your settings, but never in a mode that skips asking
    /// you.
    pub(crate) fn start(&self, p: &Plugin, q: StartReq) -> Result<Option<Raw>, Error> {
        let cwd = &q.cwd;
        if !cwd.starts_with('/') {
            return Err(invalid("cwd must be an absolute path"));
        }
        let Some(mut dir) = eval(cwd) else {
            return Err(invalid("cwd does not exist"));
        };
        if !std::fs::metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Err(invalid("cwd is not a folder"));
        }
        let ws = p.workspace_dirs();
        if !ws.iter().any(|w| within(&dir, w)) {
            return Err(denied(&format!("{cwd} is outside the plugin's workspaces")));
        }
        let mode = or(&q.mode, "default");
        if !SAFE_MODES.contains(&mode.as_str()) {
            return Err(denied(&format!(
                "permission mode {mode}: use default, acceptEdits or plan"
            )));
        }
        if q.meta.len() > 16 {
            return Err(invalid("meta: at most 16 keys"));
        }
        if q.meta
            .iter()
            .any(|(k, v)| !meta_key_ok(k) || v.len() > 1024)
        {
            return Err(invalid(
                "meta: keys are letters, digits, _ . - (64 at most); values 1 KB at most",
            ));
        }
        let mut wt_name = String::new();
        let mut wt_branch = String::new();
        if let Some(wt) = &q.worktree {
            wt_name = worktree_name(&or(
                &wt.name,
                &or(&wt.branch, &or(&q.name, &first_words(&q.prompt, 6))),
            ));
            wt_branch = or(&wt.branch, &format!("worktree-{wt_name}"));
            if !ref_ok(&wt_branch)
                || wt_branch.contains("..")
                || (!wt.base.is_empty() && !ref_ok(&wt.base))
            {
                return Err(invalid("worktree: bad branch or base"));
            }
            // The worktree goes in the checkout's own folder, which must be
            // one of the plugin's too.
            let root = rush_sys::actions::repo_root(&dir);
            if root.is_empty() {
                return Err(invalid(format!("{cwd} isn't in a git repository")));
            }
            let root = eval(&root).unwrap_or_default();
            if !ws.iter().any(|w| within(&root, w)) {
                return Err(denied(&format!(
                    "the checkout {root} is outside the plugin's workspaces"
                )));
            }
        }
        if !q.model.is_empty() && !model_ok(&q.model) {
            return Err(invalid("bad model name"));
        }
        if !q.effort.is_empty()
            && !["low", "medium", "high", "xhigh", "max"].contains(&q.effort.as_str())
        {
            return Err(invalid("effort: use low, medium, high, xhigh or max"));
        }
        if q.prompt.len() > MAX_TEXT {
            return Err(invalid("prompt is too long"));
        }

        {
            let mut s = self.lock();
            let now = Instant::now();
            s.starts
                .retain(|t| now.duration_since(*t) <= Duration::from_secs(3600));
            if s.starts.len() >= MAX_STARTS_PER_HR {
                return Err(denied(&format!(
                    "it has started {MAX_STARTS_PER_HR} sessions this hour"
                )));
            }
            s.starts.push(now);
        }
        let live = self
            .hosts
            .list()
            .iter()
            .filter(|i| i.started_by == p.name && crate::host::alive(i.host_pid))
            .count();
        if live >= MAX_LIVE {
            return Err(denied(&format!("it already has {live} sessions running")));
        }

        if let Some(wt) = &q.worktree {
            dir = rush_sys::actions::new_worktree_on(&dir, &wt_name, &wt_branch, &wt.base)
                .map_err(server)?;
        }
        let mut name = q.name.trim().to_string();
        if name.is_empty() {
            name = first_words(&q.prompt, 6);
        }
        let name = format!("{}: {name}", p.name);
        let st = rush_state::Store::load();
        let d = &st.config.dispatch;
        let cfg = SpawnConfig {
            account: st.config.active_account(),
            cwd: dir.clone(),
            prompt: q.prompt.clone(),
            name,
            model: or(&q.model, &d.model),
            effort: or(&q.effort, &d.effort),
            permission_mode: mode,
            limit_mode: d.on_limit.clone(),
            lean: d.quick_start(),
            started_by: p.name.clone(),
            meta: q.meta.clone(),
        };
        let id = match self.hosts.spawn(&cfg) {
            Ok(id) => id,
            Err(e) => {
                self.scope.log(
                    "plugin-spawn",
                    fields(&[
                        ("plugin", json!(p.name)),
                        ("cwd", json!(dir)),
                        ("err", json!(e)),
                    ]),
                );
                return Err(server(e));
            }
        };
        self.log.print(&format!("started session {id} in {dir}"));
        self.hosts.log_event(
            &id,
            "plugin-spawn",
            fields(&[
                ("plugin", json!(p.name)),
                ("cwd", json!(dir)),
                ("worktree", json!(q.worktree.is_some())),
            ]),
        );
        Ok(Some(crate::object(vec![
            ("id", to_raw(&id)),
            ("cwd", to_raw(&dir)),
        ])))
    }

    /// Follows a session and passes its events on as session.event
    /// notifications: its state, Claude's words, the tools it uses (by
    /// what they do, not what they return) and the end of each turn.
    fn subscribe(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let d = self
            .hosts
            .dial(id)
            .map_err(|_| format!("session {id} is not running"))?;
        let (ctx, cancel) = Ctx::background().with_cancel();
        let me = Arc::new(Sub {
            cancel: cancel.clone(),
        });
        let conn = {
            let mut s = self.lock();
            if let Some(old) = s.subs.insert(id.to_string(), me.clone()) {
                old.cancel.cancel();
            }
            s.conn.clone()
        };
        let Some(conn) = conn else {
            cancel.cancel();
            d.client.close();
            return Err("not ready".into());
        };
        let client = d.client.clone();
        let reg = ctx.on_cancel(move || client.close());
        let (r, id) = (self.clone(), id.to_string());
        self.scope.spawn("plugin-subscribe", move || {
            let _reg = reg;
            let emit = |mut ev: Map<String, Value>| {
                if ev.get("type").and_then(Value::as_str) == Some("delta") {
                    return; // not in the protocol: a notification a word is too many
                }
                ev.insert("session".into(), json!(id));
                let _ = conn.notify("session.event", &ev);
            };
            follow(d.lines.iter(), false, &*r.hosts, emit);
            if ctx.err().is_none() {
                emit(obj(json!({"type": "closed"})));
            }
            cancel.cancel();
            let mut s = r.lock();
            if s.subs.get(&id).is_some_and(|s| Arc::ptr_eq(s, &me)) {
                s.subs.remove(&id);
            }
        });
        Ok(())
    }
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Sends now, or drops, the message at index in a session's queue, and
/// waits until the session says it's gone. was, when given, names it by
/// its text, so it's still the one meant if the queue moved.
pub(crate) fn queued(
    hosts: &dyn Hosts,
    id: &str,
    index: i64,
    was: &str,
    send: bool,
) -> Result<Option<Raw>, Error> {
    let d = hosts
        .dial(id)
        .map_err(|_| server(format!("session {id} is not running")))?;
    let res = queued_on(&d, id, index, was, send);
    d.client.close();
    res
}

fn queued_on(
    d: &crate::host::Dialed,
    id: &str,
    index: i64,
    was: &str,
    send: bool,
) -> Result<Option<Raw>, Error> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let next = |want: &mut dyn FnMut(&HostEvent) -> Result<bool, Error>| -> Result<(), Error> {
        loop {
            let evs = match d
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(evs) => evs,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(server(format!("session {id} went away")));
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(server(format!("session {id} didn't answer")));
                }
            };
            for ev in &evs {
                if want(ev)? {
                    return Ok(());
                }
            }
        }
    };
    let mut queue: Vec<String> = Vec::new();
    next(&mut |ev| {
        if let HostEvent::Info(i) = ev {
            queue = i.queue.clone();
            return Ok(true);
        }
        Ok(false)
    })?;
    let mut was = was.to_string();
    if was.is_empty() {
        if index < 0 || index as usize >= queue.len() {
            return Err(invalid(format!("no queued message {index}")));
        }
        was = queue[index as usize].clone();
    }
    if !queue.contains(&was) {
        return Err(invalid("no such message in the queue"));
    }
    let count = |q: &[String]| q.iter().filter(|s| **s == was).count();
    let n = count(&queue);
    let op = if send {
        Op::queue_send(index, &was)
    } else {
        Op::queue_remove(index, &was)
    };
    d.client.op(op).map_err(server)?;
    next(&mut |ev| match ev {
        HostEvent::Error(e) => Err(server(e)),
        HostEvent::Info(i) => Ok(count(&i.queue) < n),
        _ => Ok(false),
    })?;
    Ok(empty())
}

fn with_host(
    hosts: &dyn Hosts,
    id: &str,
    f: impl FnOnce(&dyn HostClient) -> Result<(), String>,
) -> Result<Option<Raw>, Error> {
    let d = hosts
        .dial(id)
        .map_err(|_| server(format!("session {id} is not running")))?;
    let res = f(&*d.client);
    d.client.close();
    res.map(|_| empty()).map_err(server)
}

/// A folder name made from s, as rush's forks name theirs.
pub(crate) fn worktree_name(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    if out.len() > 40 {
        out.truncate(40);
        while out.ends_with('-') {
            out.pop();
        }
    }
    or(&out, "session")
}

/// The first n words of s, or "a new session".
pub(crate) fn first_words(s: &str, n: usize) -> String {
    let w: Vec<&str> = s.split_whitespace().take(n).collect();
    if w.is_empty() {
        return "a new session".into();
    }
    w.join(" ")
}

/// Reads a session's host lines until they end and tells emit of what
/// happens, as session.event has it: info, sent, text, tool and result,
/// and delta for each piece of text as it's written. The host replays what
/// it has before its info; only what comes after is news, unless replay,
/// when the turn under way is told from its start. The caller tells of
/// closed: only it knows whether it hung up itself. hosts words the tool
/// calls, as each agent puts them.
pub fn follow(
    lines: impl IntoIterator<Item = Vec<HostEvent>>,
    replay: bool,
    hosts: &dyn Hosts,
    mut emit: impl FnMut(Map<String, Value>),
) {
    let mut live = false;
    let mut last = Session::default();
    let mut kind = String::new();
    let mut held: Vec<HostEvent> = Vec::new(); // the turn under way, as replayed
    let tell = |ev: &HostEvent, kind: &str, emit: &mut dyn FnMut(Map<String, Value>)| match ev {
        HostEvent::Sent(t) => emit(obj(json!({"type": "sent", "text": t}))),
        HostEvent::Event(e) => match &**e {
            Event::Delta(d) => {
                if d.kind == PartKind::Text && !d.text.is_empty() {
                    emit(obj(json!({"type": "delta", "text": d.text})));
                }
            }
            Event::Message(m) => {
                if m.role == "assistant" && m.parent.is_empty() {
                    said(m, kind, hosts, emit);
                }
            }
            Event::TurnEnd(t) => {
                let failed = !t.err.is_empty() || (t.reason != "done" && t.reason != "interrupted");
                let text = if t.err.is_empty() { &t.text } else { &t.err };
                emit(obj(
                    json!({"type": "result", "text": text, "isError": failed,
                    "costUsd": t.cost, "turns": t.turns}),
                ));
            }
            _ => {}
        },
        _ => {}
    };
    for evs in lines {
        for ev in evs {
            match &ev {
                HostEvent::Info(i) => {
                    kind = i.kind.clone();
                    if !live {
                        live = true;
                        for h in held.drain(..) {
                            tell(&h, &kind, &mut emit);
                        }
                    }
                    let s = session_of(i);
                    if s.state != last.state || s.detail != last.detail || s.needs != last.needs {
                        emit(obj(
                            json!({"type": "info", "state": s.state, "detail": s.detail,
                            "needs": s.needs, "costUsd": s.cost_usd}),
                        ));
                    }
                    last = s;
                }
                HostEvent::Event(e) if matches!(**e, Event::TurnEnd(_)) => {
                    if live {
                        tell(&ev, &kind, &mut emit);
                    }
                    held.clear(); // a turn over is history
                }
                _ => {
                    if live {
                        tell(&ev, &kind, &mut emit);
                    } else if replay {
                        held.push(ev);
                    }
                }
            }
        }
    }
}

/// Tells of what the main agent said: its words, and each tool call in a
/// few words, as agent kind puts it.
fn said(m: &Message, kind: &str, hosts: &dyn Hosts, emit: &mut dyn FnMut(Map<String, Value>)) {
    for p in &m.parts {
        if p.kind == PartKind::Text && !p.text.trim().is_empty() {
            emit(obj(json!({"type": "text", "text": p.text})));
        } else if p.kind == PartKind::ToolCall
            && let Some(c) = &p.call
        {
            emit(obj(
                json!({"type": "tool", "name": c.name, "doing": hosts.doing(kind, c)}),
            ));
        }
    }
}
