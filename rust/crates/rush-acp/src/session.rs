//! One ACP session, and the agent process running it when start started
//! one: it introduces rush, opens or resumes the session, and turns what
//! the agent says into rush's events. Port of
//! internal/adapters/acp/session.go:1-651.
//!
//! Go queues events for a pump goroutine to hand out. Here each goes
//! straight into the caller's Sink, which never blocks, in the order the
//! session's lock gives them; the reader thread is the only thread a
//! session keeps, beside one that keeps the last of its stderr.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::Engine;
use rush_core::adapter::{self, Answerer, Conn, Input, Sink};
use rush_core::event::{self, Event, PermissionMode};
use serde_json::json;
use serde_json::value::RawValue;

use crate::protocol::{
    ConfigOption, ConfigValue, ContentBlock, Info, InitializeResult, Mode, PROTOCOL_VERSION,
    PromptResult, SessionResult,
};
use crate::rpc::{self, Error, Handler, Rpc};
use crate::serve::Elicitation;
use crate::update::CallState;

/// How to run an ACP agent and which session to open.
#[derive(Clone, Default)]
pub struct Options {
    /// The agent's program: "kimi", "copilot".
    pub command: String,
    /// What makes it speak ACP: "acp", "--acp".
    pub args: Vec<String>,
    /// Added to rush's own, as NAME=value.
    pub env: Vec<String>,
    /// The session's working directory; rush's when empty.
    pub dir: String,
    /// A past session to open instead of a new one.
    pub resume: String,
    /// Passed to the agent as ACP McpServer objects.
    pub mcp_servers: Vec<serde_json::Value>,
    /// Names the agent in the events nothing else fits: "kimi".
    pub adapter: String,
    /// Keeps file reads and writes with the agent rather than rush.
    pub no_fs: bool,
    /// The tools allowed without asking, as the agent names them:
    /// mcp__rush__spawn_agent.
    pub trusted: Vec<String>,
    /// The agent finds a subagent's own run by the call that started it
    /// (Go asks the registry whether the adapter is a ChildFinder).
    pub finds_children: bool,
    /// How long to wait for the agent to open the session; forever when
    /// None (Go's context).
    pub within: Option<Duration>,
}

/// One ACP session. Any thread may call it at once.
pub struct Session {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    /// Itself, for the threads it starts.
    pub(crate) me: std::sync::Weak<Inner>,
    pub(crate) rpc: Rpc,
    pub(crate) sink: Arc<dyn Sink>,
    info: Mutex<Info>,
    cwd: Mutex<String>,
    pid: AtomicI32,
    stderr: Arc<Tail>,
    /// Set once the agent has gone, and told.
    exited: (Mutex<bool>, Condvar),
    /// Closed: what the session says goes nowhere now.
    closed: AtomicBool,
    /// shut has run.
    shutting: AtomicBool,
    /// sink.ended has been called.
    told: AtomicBool,
    /// Turns sent and not yet ended.
    turns: (Mutex<usize>, Condvar),
    pub(crate) amu: Mutex<Waiting>,
    pub(crate) mu: Mutex<State>,
}

/// What waits on the user: approval id → the agent's request id, and the
/// forms asked.
#[derive(Default)]
pub(crate) struct Waiting {
    pub approvals: HashMap<String, Box<RawValue>>,
    pub questions: HashMap<String, Elicitation>,
}

/// What the session knows, under its lock, which is also the order its
/// events go out in.
#[derive(Default)]
pub(crate) struct State {
    pub o: Options,
    pub id: String,
    pub ready: bool,
    /// The agent has gone: no more turns.
    pub gone: bool,
    pub open: Option<event::Message>,
    pub calls: HashMap<String, CallState>,
    pub modes: Vec<Mode>,
    pub mode: String,
    pub config: Vec<ConfigOption>,
    /// The model, from the unstable models field.
    pub legacy: String,
    pub commands: Vec<String>,
    pub cost: f64,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Session {
    /// Runs the agent, introduces rush and opens the session. What the
    /// session says goes to sink.
    pub fn start(o: Options, sink: Arc<dyn Sink>) -> Result<Session, Error> {
        let mut cmd = Command::new(&o.command);
        cmd.args(&o.args);
        if !o.dir.is_empty() {
            cmd.current_dir(&o.dir);
        }
        for kv in &o.env {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        // Its own process group, so stopping the session takes its shells
        // too.
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::other(format!("exec: {:?}: {e}", o.command)))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let s = Session::new(o, Box::new(stdin), sink);
        let pid = child.id() as i32;
        s.inner.pid.store(pid, Ordering::SeqCst);
        let tail = s.inner.stderr.clone();
        std::thread::Builder::new()
            .name("acp-stderr".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    tail.write(&buf[..n]);
                }
            })
            .map_err(Error::from)?;
        let guard = rush_sys::pgguard::watch(pid);
        let inner = s.inner.clone();
        std::thread::Builder::new()
            .name("acp-read".into())
            .spawn(move || {
                inner.rpc.read(stdout, inner.as_ref());
                let _ = child.wait();
                if let Some(g) = guard {
                    g.release();
                }
                inner.ended();
            })
            .map_err(Error::from)?;
        if let Err(err) = s.begin() {
            s.abandon();
            // The agent's own answer says why; else what it last said on
            // stderr.
            let t = s.inner.stderr.text();
            if !t.is_empty() && err.rpc_error().is_none() {
                return Err(err.suffix(last_line(&t)));
            }
            return Err(err);
        }
        Ok(s)
    }

    /// Opens a session with an agent already running at the other end of r
    /// and w.
    pub fn dial(
        r: impl Read + Send + 'static,
        w: impl Write + Send + 'static,
        o: Options,
        sink: Arc<dyn Sink>,
    ) -> Result<Session, Error> {
        let s = Session::new(o, Box::new(w), sink);
        let inner = s.inner.clone();
        std::thread::Builder::new()
            .name("acp-read".into())
            .spawn(move || {
                inner.rpc.read(r, inner.as_ref());
                inner.ended();
            })
            .map_err(Error::from)?;
        if let Err(e) = s.begin() {
            s.abandon();
            return Err(e);
        }
        Ok(s)
    }

    /// A session with no agent behind it, for tests of its state.
    #[cfg(test)]
    pub(crate) fn bare(o: Options, sink: Arc<dyn Sink>) -> Session {
        Session::new(o, Box::new(std::io::sink()), sink)
    }

    fn new(mut o: Options, w: Box<dyn Write + Send>, sink: Arc<dyn Sink>) -> Session {
        if o.adapter.is_empty() {
            o.adapter = "acp".into();
        }
        Session {
            inner: Arc::new_cyclic(|me| Inner {
                me: me.clone(),
                rpc: Rpc::new(w),
                sink,
                info: Mutex::new(Info::default()),
                cwd: Mutex::new(String::new()),
                pid: AtomicI32::new(0),
                stderr: Arc::new(Tail::default()),
                exited: (Mutex::new(false), Condvar::new()),
                closed: AtomicBool::new(false),
                shutting: AtomicBool::new(false),
                told: AtomicBool::new(false),
                turns: (Mutex::new(0), Condvar::new()),
                amu: Mutex::new(Waiting::default()),
                mu: Mutex::new(State {
                    o,
                    ..State::default()
                }),
            }),
        }
    }

    /// Introduces rush and opens the session.
    fn begin(&self) -> Result<(), Error> {
        let inner = &self.inner;
        let (no_fs, within, dir, resume, servers) = {
            let st = lock(&inner.mu);
            (
                st.o.no_fs,
                st.o.within,
                st.o.dir.clone(),
                st.o.resume.clone(),
                st.o.mcp_servers.clone(),
            )
        };
        let init: InitializeResult = inner
            .rpc
            .call(
                "initialize",
                Some(&json!({
                    "clientCapabilities": {
                        "elicitation": {"form": {}},
                        "fs": {"readTextFile": !no_fs, "writeTextFile": !no_fs},
                        "terminal": false,
                    },
                    "clientInfo": {"name": "rush", "title": "rush", "version": "dev"},
                    "protocolVersion": PROTOCOL_VERSION,
                })),
                within,
            )
            .and_then(rpc::decode)
            .map_err(|e| e.wrap("acp: initialize"))?;
        let caps = &init.agent_capabilities;
        let mut info = Info {
            protocol: init.protocol_version,
            load_session: caps.load_session,
            resume: caps
                .session_capabilities
                .resume
                .as_deref()
                .is_some_and(|r| r.get() != "null"),
            images: caps.prompt_capabilities.image,
            auth_methods: init.auth_methods.clone().unwrap_or_default(),
            ..Info::default()
        };
        if let Some(a) = &init.agent_info {
            info.name = a.name.clone();
            info.title = a.title.clone();
            info.version = a.version.clone();
        }
        *lock(&inner.info) = info.clone();
        let cwd = absolute(&dir)?;
        *lock(&inner.cwd) = cwd.clone();
        let mut params = json!({"cwd": cwd, "mcpServers": servers});
        let res = if resume.is_empty() {
            let res: SessionResult = inner
                .rpc
                .call("session/new", Some(&params), within)
                .and_then(rpc::decode)
                .map_err(|e| e.wrap("acp: open session"))?;
            lock(&inner.mu).id = res.session_id.clone();
            res
        } else if info.load_session || info.resume {
            // session/load replays the conversation as updates first.
            lock(&inner.mu).id = resume.clone();
            params["sessionId"] = json!(resume);
            let method = if info.load_session {
                "session/load"
            } else {
                "session/resume"
            };
            inner
                .rpc
                .call(method, Some(&params), within)
                .and_then(rpc::decode)
                .map_err(|e| e.wrap("acp: open session"))?
        } else {
            return Err(Error::unsupported().wrap("acp: resume"));
        };
        let mut st = lock(&inner.mu);
        inner.flush(&mut st);
        if let Some(m) = res.modes {
            st.modes = m.available_modes;
            st.mode = m.current_mode_id;
        }
        if let Some(m) = res.models {
            st.legacy = m.current_model_id;
        }
        set_config(&mut st, res.config_options);
        st.ready = true;
        inner.emit_init(&st);
        Ok(())
    }

    /// The session's id, as the agent knows it.
    pub fn id(&self) -> String {
        lock(&self.inner.mu).id.clone()
    }

    /// What the agent said about itself.
    pub fn info(&self) -> Info {
        lock(&self.inner.info).clone()
    }

    /// The modes the session can be put in.
    pub fn modes(&self) -> Vec<Mode> {
        lock(&self.inner.mu).modes.clone()
    }

    /// The last of what the agent wrote to stderr.
    pub fn stderr(&self) -> String {
        self.inner.stderr.text()
    }

    /// Prompts the agent. Its reply arrives as events, ending in TurnEnd.
    pub fn prompt(&self, input: Input) -> Result<(), Error> {
        let mut blocks = Vec::new();
        if !input.text.is_empty() {
            blocks.push(ContentBlock {
                kind: "text".into(),
                text: input.text,
                ..ContentBlock::default()
            });
        }
        if !input.images.is_empty() && !lock(&self.inner.info).images {
            return Err(Error::unsupported().wrap("acp: images"));
        }
        for p in &input.images {
            let b = std::fs::read(p).map_err(|e| Error::other(format!("open {p}: {e}")))?;
            let mt = media_type(p, &b);
            blocks.push(ContentBlock {
                kind: "image".into(),
                mime_type: mt,
                data: base64::engine::general_purpose::STANDARD.encode(&b),
                ..ContentBlock::default()
            });
        }
        if blocks.is_empty() {
            return Err(Error::other("acp: nothing to send"));
        }
        let id = {
            let st = lock(&self.inner.mu);
            if st.gone {
                return Err(Error::closed());
            }
            *lock(&self.inner.turns.0) += 1;
            st.id.clone()
        };
        let start = Instant::now();
        let inner = self.inner.clone();
        self.inner.rpc.call_with(
            "session/prompt",
            Some(&json!({"prompt": blocks, "sessionId": id})),
            Box::new(move |a| {
                inner.turn_ended(a, start);
                let mut n = lock(&inner.turns.0);
                *n -= 1;
                inner.turns.1.notify_all();
            }),
        );
        Ok(())
    }

    /// Answers an Approval with one of its options. No option cancels it.
    pub fn answer_approval(&self, approval_id: &str, option_id: &str) -> Result<(), Error> {
        let id = lock(&self.inner.amu).approvals.remove(approval_id);
        let Some(id) = id else {
            return Err(Error::other(format!(
                "acp: no approval {} waiting",
                go_quote(approval_id)
            )));
        };
        let outcome = if option_id.is_empty() {
            json!({"outcome": "cancelled"})
        } else {
            json!({"optionId": option_id, "outcome": "selected"})
        };
        self.inner
            .rpc
            .reply(&id, Some(&json!({ "outcome": outcome })), None)
    }

    /// Cancels the turn. Approvals still waiting are withdrawn, as ACP asks
    /// of a client that cancels.
    pub fn cancel(&self) -> Result<(), Error> {
        let inner = &self.inner;
        let id = lock(&inner.mu).id.clone();
        inner
            .rpc
            .send("session/cancel", Some(&json!({ "sessionId": id })))?;
        let w = std::mem::take(&mut *lock(&inner.amu));
        let _st = lock(&inner.mu);
        for (aid, rid) in w.approvals {
            let _ = inner.rpc.reply(
                &rid,
                Some(&json!({"outcome": {"outcome": "cancelled"}})),
                None,
            );
            inner.emit(Event::ApprovalCancelled(event::ApprovalCancelled {
                id: aid,
            }));
        }
        for (qid, q) in w.questions {
            let _ = inner
                .rpc
                .reply(&q.id, Some(&json!({"action": "cancel"})), None);
            inner.emit(Event::ApprovalCancelled(event::ApprovalCancelled {
                id: qid,
            }));
        }
        Ok(())
    }

    /// Switches the session's model, through its "model" setting or the
    /// older session/set_model.
    pub fn switch_model(&self, model: &str) -> Result<(), Error> {
        let (opt, legacy, id) = {
            let st = lock(&self.inner.mu);
            (
                option(&st.config, "model").map(|o| o.id.clone()),
                !st.legacy.is_empty(),
                st.id.clone(),
            )
        };
        if let Some(opt) = opt {
            return self.set_option(&opt, model);
        }
        if legacy {
            self.inner.rpc.call(
                "session/set_model",
                Some(&json!({"modelId": model, "sessionId": id})),
                None,
            )?;
            let mut st = lock(&self.inner.mu);
            st.legacy = model.to_string();
            self.inner.emit_init(&st);
            return Ok(());
        }
        Err(Error::unsupported().wrap("acp: set model"))
    }

    /// Applies the agent's advertised thinking configuration.
    pub fn set_effort(&self, value: &str) -> Result<(), Error> {
        let id = {
            let st = lock(&self.inner.mu);
            option(&st.config, "thought_level")
                .or_else(|| option(&st.config, "thinking"))
                .map(|o| o.id.clone())
        };
        match id {
            Some(id) if !id.is_empty() => self.set_option(&id, value),
            _ => Err(Error::unsupported().wrap("acp: thinking level")),
        }
    }

    /// Puts the session in one of its Modes.
    pub fn switch_mode(&self, mode: &str) -> Result<(), Error> {
        let (known, via_option, id) = {
            let st = lock(&self.inner.mu);
            let known = st.modes.iter().any(|m| m.id == mode);
            let opt = option(&st.config, "mode").map(|o| o.id.clone());
            let via = opt.filter(|_| st.modes.is_empty());
            (known, via, st.id.clone())
        };
        if known {
            self.inner.rpc.call(
                "session/set_mode",
                Some(&json!({"modeId": mode, "sessionId": id})),
                None,
            )?;
            let mut st = lock(&self.inner.mu);
            st.mode = mode.to_string();
            self.inner.emit_init(&st);
            return Ok(());
        }
        if let Some(opt) = via_option {
            return self.set_option(&opt, mode);
        }
        Err(Error::unsupported().wrap(&format!("acp: mode {}", go_quote(mode))))
    }

    fn set_option(&self, config_id: &str, value: &str) -> Result<(), Error> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Res {
            #[serde(rename = "configOptions")]
            config_options: Option<Vec<ConfigOption>>,
        }
        let id = lock(&self.inner.mu).id.clone();
        let res: Res = self
            .inner
            .rpc
            .call(
                "session/set_config_option",
                Some(&json!({"configId": config_id, "sessionId": id, "value": value})),
                None,
            )
            .and_then(rpc::decode)?;
        let mut st = lock(&self.inner.mu);
        set_config(&mut st, res.config_options);
        self.inner.emit_init(&st);
        Ok(())
    }

    /// Ends the session: the agent is asked to go by closing its stdin,
    /// then made to. What it says from here goes nowhere, and the sink is
    /// told it ended.
    pub fn shut(&self) {
        let inner = &self.inner;
        if inner.shutting.swap(true, Ordering::SeqCst) {
            return;
        }
        inner.tell(None);
        inner.rpc.close();
        let pid = inner.pid.load(Ordering::SeqCst);
        if pid <= 0 {
            return;
        }
        for sig in [0, libc::SIGTERM, libc::SIGKILL] {
            if sig != 0 {
                // signals the agent's own process group.
                let _ = rush_sys::proc::kill_group(pid, sig);
            }
            let (m, c) = &inner.exited;
            let g = lock(m);
            let (g, _) = c
                .wait_timeout_while(g, Duration::from_secs(2), |gone| !*gone)
                .unwrap_or_else(|e| e.into_inner());
            if *g {
                return;
            }
        }
    }

    /// Ends a session that failed to start: the caller has the error as
    /// its answer, so the sink is not told it ended, and hears nothing
    /// more (Go's events channel nobody was given).
    pub(crate) fn abandon(&self) {
        {
            let _st = lock(&self.inner.mu);
            self.inner.closed.store(true, Ordering::SeqCst);
            self.inner.told.store(true, Ordering::SeqCst);
        }
        self.shut();
    }

    /// The agent's process, or 0 when dial connected it.
    pub fn process(&self) -> i32 {
        self.inner.pid.load(Ordering::SeqCst)
    }
}

impl Inner {
    /// Hands ev to the sink, unless the session is closed. Called under the
    /// session's lock, which keeps the order.
    pub(crate) fn emit(&self, ev: Event) {
        if !self.closed.load(Ordering::SeqCst) {
            self.sink.event(ev);
        }
    }

    /// Tells the sink the session has ended, once, and closes the session
    /// to events. It takes the session's lock, which every emit holds, so
    /// no event can reach the sink after it's told.
    fn tell(&self, err: Option<adapter::Error>) {
        let _st = lock(&self.mu);
        self.closed.store(true, Ordering::SeqCst);
        if !self.told.swap(true, Ordering::SeqCst) {
            self.sink.ended(err);
        }
    }

    /// Runs once the agent has gone: turns still out end in an error, then
    /// the sink is told.
    fn ended(&self) {
        lock(&self.mu).gone = true;
        {
            let (m, c) = &self.turns;
            let g = lock(m);
            let _g = c
                .wait_while(g, |n| *n > 0)
                .unwrap_or_else(|e| e.into_inner());
        }
        {
            let (m, c) = &self.exited;
            *lock(m) = true;
            c.notify_all();
        }
        self.tell(None);
    }

    /// A turn's prompt has its answer.
    fn turn_ended(&self, a: rpc::Answer, start: Instant) {
        let (res, err) = match a.and_then(rpc::decode::<PromptResult>) {
            Ok(r) => (r, None),
            Err(e) => (PromptResult::default(), Some(e)),
        };
        let mut st = lock(&self.mu);
        self.flush(&mut st);
        let mut end = event::TurnEnd {
            reason: stop_reason(&res.stop_reason),
            duration: start.elapsed(),
            turns: 1,
            cost: st.cost,
            ..event::TurnEnd::default()
        };
        if let Some(e) = err {
            end.reason = "error".into();
            end.err = match e.rpc_error() {
                Some(r) => r.message.clone(),
                None => e.to_string(),
            };
        }
        if let Some(u) = res.usage {
            end.tokens.input = u.input_tokens;
            end.tokens.output = u.output_tokens;
            end.tokens.cache_read = u.cached_read_tokens;
            end.tokens.cache_write_5m = u.cached_write_tokens;
            end.tokens.reasoning = u.thought_tokens;
        }
        self.emit(Event::TurnEnd(end));
    }

    /// Says what the session is. It goes again whenever that changes: its
    /// model, its mode, its commands.
    pub(crate) fn emit_init(&self, st: &State) {
        if !st.ready {
            return;
        }
        self.emit(Event::Init(Box::new(event::Init {
            session_id: st.id.clone(),
            model: model(st),
            cwd: lock(&self.cwd).clone(),
            mode: mode_name(st),
            mode_id: st.mode.clone(),
            modes: permission_modes(st),
            version: lock(&self.info).version.clone(),
            effort: effort(st),
            commands: st.commands.clone(),
            ..event::Init::default()
        })));
    }
}

impl Handler for Inner {
    fn notified(&self, method: &str, params: Option<&RawValue>) {
        self.notify(method, params);
    }
    fn served(&self, id: Box<RawValue>, method: &str, params: Option<&RawValue>) {
        self.serve(id, method, params);
    }
}

/// ACP's reason a turn stopped in rush's words.
pub(crate) fn stop_reason(r: &str) -> String {
    match r {
        "end_turn" | "" => "done",
        "cancelled" => "interrupted",
        "max_turn_requests" => "max_turns",
        r => r, // max_tokens, refusal
    }
    .to_string()
}

/// The setting of category, if the session has one.
pub(crate) fn option<'a>(config: &'a [ConfigOption], category: &str) -> Option<&'a ConfigOption> {
    config
        .iter()
        .find(|o| o.category == category || (o.category.is_empty() && o.id == category))
}

pub(crate) fn set_config(st: &mut State, opts: Option<Vec<ConfigOption>>) {
    let Some(opts) = opts else {
        return;
    };
    st.config = opts;
    if st.modes.is_empty()
        && let Some(o) = option(&st.config, "mode")
    {
        st.mode = o.current();
    }
}

pub(crate) fn model(st: &State) -> String {
    match option(&st.config, "model") {
        Some(o) => o.current(),
        None => st.legacy.clone(),
    }
}

fn effort(st: &State) -> String {
    for category in ["thought_level", "thinking"] {
        if let Some(o) = option(&st.config, category) {
            return o.current();
        }
    }
    String::new()
}

/// The modes as the agent names them: display names can't be sent as
/// ids.
pub(crate) fn permission_modes(st: &State) -> Vec<PermissionMode> {
    let mut out: Vec<PermissionMode> = st
        .modes
        .iter()
        .map(|m| PermissionMode {
            id: m.id.clone(),
            name: m.name.clone(),
            description: m.description.clone(),
        })
        .collect();
    if out.is_empty()
        && let Some(opt) = option(&st.config, "mode")
    {
        fn add(values: &[ConfigValue], out: &mut Vec<PermissionMode>) {
            for v in values {
                if !v.value.is_empty() {
                    out.push(PermissionMode {
                        id: v.value.clone(),
                        name: v.name.clone(),
                        description: v.description.clone(),
                    });
                }
                add(&v.options, out);
            }
        }
        add(&opt.options, &mut out);
    }
    out
}

/// The mode the session is in, by its name when it has one: some agents'
/// mode ids are URLs.
fn mode_name(st: &State) -> String {
    for m in &st.modes {
        if m.id == st.mode && !m.name.is_empty() {
            return m.name.to_lowercase();
        }
    }
    st.mode.clone()
}

/// Keeps the last few KB of stderr for error messages.
#[derive(Default)]
struct Tail(Mutex<Vec<u8>>);

impl Tail {
    fn write(&self, p: &[u8]) {
        let mut b = lock(&self.0);
        b.extend_from_slice(p);
        let n = b.len();
        if n > 4 << 10 {
            b.drain(..n - (4 << 10));
        }
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&lock(&self.0)).trim().to_string()
    }
}

fn last_line(s: &str) -> &str {
    match s.rfind('\n') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

/// s quoted as Go's %q does, near enough for an error.
pub(crate) fn go_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// dir made absolute and clean, as Go's filepath.Abs: rush's own folder
/// when empty.
pub(crate) fn absolute(dir: &str) -> Result<String, Error> {
    let p = Path::new(dir);
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    Ok(out.to_string_lossy().into_owned())
}

/// What an image is by its extension, else by its first bytes (Go's
/// mime.TypeByExtension, then http.DetectContentType).
fn media_type(path: &str, b: &[u8]) -> String {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let by_ext = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        _ => "",
    };
    if !by_ext.is_empty() {
        return by_ext.into();
    }
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if b.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        "image/gif"
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
    .into()
}

impl Conn for Session {
    fn send(&self, input: Input) -> adapter::Result<()> {
        Ok(self.prompt(input)?)
    }
    fn answer(&self, approval_id: &str, option_id: &str) -> adapter::Result<()> {
        Ok(self.answer_approval(approval_id, option_id)?)
    }
    fn interrupt(&self) -> adapter::Result<()> {
        Ok(self.cancel()?)
    }
    fn set_model(&self, model: &str) -> adapter::Result<()> {
        Ok(self.switch_model(model)?)
    }
    fn set_mode(&self, mode: &str) -> adapter::Result<()> {
        Ok(self.switch_mode(mode)?)
    }
    fn close(&self) -> adapter::Result<()> {
        self.shut();
        Ok(())
    }
    fn answerer(&self) -> Option<&dyn Answerer> {
        Some(self)
    }
    fn pid(&self) -> Option<i32> {
        let p = self.process();
        (p > 0).then_some(p)
    }
    fn end_tasks(&self) -> Vec<String> {
        crate::tasks_ended::end_tasks(&self.inner)
    }
}

impl Answerer for Session {
    fn answer_question(
        &self,
        id: &str,
        answers: &HashMap<String, Vec<String>>,
    ) -> adapter::Result<()> {
        Ok(self.inner.answer_question(id, answers)?)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A session dropped without close still lets its agent go.
        self.shut();
    }
}
