//! A running `claude -p` as rush's Conn, and Claude as a Driver. Port of
//! internal/adapters/claude/conn.go (Start with leanEnv at :105, relayOnly, conn and its parts,
//! RunsSession), claude/carry.go (WriteCarried) and native.go's Neutral.
//!
//! Go's conn reads the session's events on a goroutine of its own and hands
//! them out on a channel. Here the session's reader thread calls the
//! conn's handler, which answers what is the conn's own and pushes the
//! rest, as rush's events, into the caller's Sink: nothing in it waits on
//! the sink, and the caller keeps no thread per session.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rush_core::adapter::{
    self, Adopter, Answerer, Asker, Backgrounder, CacheKeeper, Conn, ContextReader, Driver, Hearer,
    Homed, Input, Orphans, Responder, Sink, StartOptions, SubagentKeeper, TaskStopper, ToolServer,
};
use rush_core::agent::PreviewLine;
use rush_core::event::{self, Event};
use rush_core::time::{Ms, format_rfc3339, now};
use rush_core::usage;
use rush_core::wire::RawJson;
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::account::Account;
use crate::adapter::{Claude, account};
use crate::headless::{self, ControlReply, Event as HEvent, Image, PermissionRequest, Session};
use crate::heard::heard_in;
use crate::neutral::Neutral;

/// Turns on Claude Code's own watchdog for a model stream that goes
/// silent.
pub const STREAM_WATCHDOG_ENV: &str = "CLAUDE_ENABLE_STREAM_WATCHDOG";
const SUBAGENT_CAP_ENV: &str = "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS";
/// How long Claude Code's prompt cache lives for everything outside the
/// main conversation (2.1.316): "5m" or "1h". Unset, it's 5 minutes.
pub const SUBAGENT_CACHE_ENV: &str = "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL";

/// The environment entry that sets the subagents' cache life, when one is
/// chosen ("5m" or "1h"); nothing for automatic (empty) or anything else.
pub fn subagent_cache_env(choice: &str) -> Option<String> {
    matches!(choice, "5m" | "1h").then(|| format!("{SUBAGENT_CACHE_ENV}={choice}"))
}

/// The model every subagent that names none starts on (Claude Code's own
/// variable; a subagent that names its own, or whose definition does,
/// keeps it).
pub const SUBAGENT_MODEL_ENV: &str = "CLAUDE_CODE_SUBAGENT_MODEL";

/// The environment entry that sets it, when a model is chosen.
pub fn subagent_model_env(model: &str) -> Option<String> {
    (!model.trim().is_empty()).then(|| format!("{SUBAGENT_MODEL_ENV}={}", model.trim()))
}

/// Puts rush's context guard (command) into Claude Code's hooks: before
/// each Read (Bash's is the gate's, which asks it); after each Read, edit
/// and Bash call, failed Bash calls too; before a compaction, and as a
/// compacted or cleared session starts again. Added beside hooks already
/// there for the same events (Rust-only).
pub fn context_hooks(hooks: &mut Map<String, Value>, command: &str) {
    let h = |matcher: &str| json!({"matcher": matcher, "hooks": [{"type": "command", "command": command, "timeout": 10}]});
    for (event, matcher) in [
        ("PreToolUse", "Read"),
        ("PostToolUse", "Read|Bash|Edit|Write|MultiEdit|NotebookEdit"),
        ("PostToolUseFailure", "Bash"),
        ("PreCompact", "*"),
        ("SessionStart", "compact|clear"),
    ] {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        if let Value::Array(a) = list {
            a.push(h(matcher));
        }
    }
}

/// How long a control request is waited on: Go's callers give it a minute
/// at most (host.go:1723).
const ASK_WITHIN: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn err(msg: impl Into<String>) -> adapter::Error {
    msg.into().into()
}

/// Whether this process's environment leaves name unset (or empty).
fn unset(name: &str) -> bool {
    std::env::var_os(name).is_none_or(|v| v.is_empty())
}

/// The account a ~/.claude session starts as: run, the login in use, or
/// the home it last ran in while that still has a sign-in (conn.go:33-37).
pub(crate) fn start_account(run: Account, home: &str) -> Account {
    if !home.is_empty() && Path::new(home) != run.config_dir {
        let h = Account::at("", home);
        if crate::home::has_home(&h) {
            return h; // still warm on the login it last ran as
        }
    }
    run
}

/// Whether a ~/.claude session on acct would start as another account now
/// that new ones run as now (conn.go:365-367).
pub(crate) fn moved_off(root: bool, now: &Account, acct: &Account) -> bool {
    root && now.config_dir != acct.config_dir
}

impl Driver for Claude {
    /// Runs claude -p for rush to draw.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        let mut acct = account(&o.profile);
        let mut root = false;
        if acct.is_default() {
            // ~/.claude runs as the login in use, in its home (conn.go:33).
            acct = start_account(self.run_as(), &o.home);
            root = true;
        }
        let tools = o.tools.clone();
        let mut ho = headless::Options {
            account: acct.clone(),
            dir: o.dir.clone(),
            model: o.model.clone(),
            effort: o.effort.clone(),
            permission_mode: o.mode.clone(),
            binary: o.binary.clone(),
            ..headless::Options::default()
        };
        let allowed = trusted(&tools);
        if !allowed.is_empty() {
            // rush's own tools only draw, so they never ask.
            ho.flags
                .extend(["--allowedTools".into(), allowed.join(",")]);
        }
        if !o.agents.is_empty() {
            ho.flags
                .extend(["--agents".into(), serde_json::to_string(&o.agents)?]);
        }
        let p = o.prompt.trim();
        if !p.is_empty() {
            ho.flags
                .extend(["--append-system-prompt".into(), p.to_string()]);
        }
        if !o.without.is_empty() {
            // Taken out of the model's context, not only refused.
            ho.flags
                .extend(["--disallowedTools".into(), o.without.join(",")]);
        }
        let mut hooks = Map::new();
        if !o.inbox.is_empty() {
            // After each tool call, a message waiting for the agent that
            // made it goes into that agent's own context: the one way into
            // a running subagent that isn't the main session's SendMessage.
            let h = json!([{"matcher": "*", "hooks": [{"type": "command", "command": o.inbox, "timeout": 10}]}]);
            hooks.insert("PostToolUse".into(), h.clone());
            hooks.insert("PostToolUseFailure".into(), h);
        }
        if !o.bash_hook.is_empty() {
            hooks.insert(
                "PreToolUse".into(),
                json!([{"matcher": "Bash|Agent|Task|Workflow", "hooks": [{"type": "command", "command": o.bash_hook, "timeout": 10}]}]),
            );
        }
        if !o.tool_hook.is_empty() {
            context_hooks(&mut hooks, &o.tool_hook);
        }
        if !hooks.is_empty() {
            ho.flags.extend([
                "--settings".into(),
                json!({ "hooks": Value::Object(hooks) }).to_string(),
            ]);
        }
        ho.flags.extend(o.flags.iter().cloned());
        let mut carried = String::new();
        if !o.carry.is_empty() && !o.resume && !o.fork && !o.session_id.is_empty() {
            // Another agent's conversation, resumed as this one's own.
            write_carried(&acct, &o.dir, &o.session_id, &o.carry)?;
            o.resume = true;
            carried = o.session_id.clone();
        }
        if o.resume {
            ho.resume = o.session_id.clone();
        } else {
            ho.session_id = o.session_id.clone();
        }
        if o.fork {
            ho.flags.push("--fork-session".into());
        }
        // Its scratch goes in the session's own folder, so what it leaves
        // behind can be seen and cleaned up.
        if !o.temp_dir.is_empty() && crate::files::mkdir_private(Path::new(&o.temp_dir)).is_ok() {
            // Ending in "/", as macOS's own: "$TMPDIR$name" stays inside.
            let t = rush_core::adapter::tmpdir_env(&o.temp_dir);
            ho.env.extend([
                format!("TMPDIR={t}"),
                format!("TMP={t}"),
                format!("TEMP={t}"),
                format!("CLAUDE_CODE_TMPDIR={t}"),
            ]);
        }
        if o.lean {
            ho.env
                .push("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1".into());
        }
        // A stream that goes silent is cut and tried again, not waited on
        // for good; one you've set yourself stands.
        if unset(STREAM_WATCHDOG_ENV) {
            ho.env.push(format!("{STREAM_WATCHDOG_ENV}=1"));
        }
        // Its engine leaner (lean.rs).
        ho.env.extend(crate::lean::lean_env());
        // Claude Code leaves the task list out for some models (Opus) when
        // run headless; rush shows it live, so it's always in.
        if unset("CLAUDE_CODE_ENABLE_TODO_TOOLS") {
            ho.env.push("CLAUDE_CODE_ENABLE_TODO_TOOLS=1".into());
        }
        if !o.api_key.is_empty() {
            // Ahead of the login signed in: paid per token.
            ho.env.push(format!("ANTHROPIC_API_KEY={}", o.api_key));
        }
        if o.subagents > 0 && unset(SUBAGENT_CAP_ENV) {
            ho.env.push(format!("{SUBAGENT_CAP_ENV}={}", o.subagents));
        }
        ho.env.extend(subagent_cache_env(&o.subagent_cache));
        ho.env.extend(subagent_model_env(&o.subagent_model));
        // Checkpoints, as Claude Code keeps them in a terminal, so a rewind
        // can put the files back too. The session's own environment goes
        // last, to have the last word.
        ho.env.push(headless::CHECKPOINT_ENV.into());
        ho.env.extend(o.env.iter().cloned());
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let taps = set_tap(&o, &tools, &mut ho);
        let login = crate::login::signed_in_as(&acct);
        let (s, reader) = headless::start(&ho)?;
        // Every process needs rush's tools registered before its first
        // message.
        let init_id = match s.initialize(&names) {
            Ok(id) => id,
            Err(e) => {
                // Read to its end, so stopping it can tell it has.
                reader.spawn(s.clone(), |_| {}, |_| {});
                let _ = s.stop(Duration::from_secs(1));
                return Err(e);
            }
        };
        let inner = Arc::new(Inner {
            s: s.clone(),
            sink,
            tools,
            init_id,
            carried: Mutex::new(carried),
            acct,
            root,
            run_as: self.config_source(),
            login,
            taps,
            dir: o.dir.clone(),
            sid: Mutex::new(o.session_id.clone()),
            asks: Mutex::new(HashMap::new()),
            waits: Mutex::new(HashMap::new()),
            live: Mutex::new(crate::usage::Usage::default()),
            stale: AtomicBool::new(false),
            ended: AtomicBool::new(false),
        });
        Ok(serve(inner, s, reader))
    }
}

/// Points o's tap at ho's, but for rush's own tools' traffic, and has ho
/// skip what's only relayed: whether it taps.
fn set_tap(o: &StartOptions, tools: &[ToolServer], ho: &mut headless::Options) -> bool {
    let taps = o.tap.is_some();
    if let Some(tap) = o.tap.clone() {
        let tools = tools.to_vec();
        ho.tap = Some(Arc::new(move |line: &[u8]| {
            if !own_traffic(&tools, line) {
                tap(line);
            }
        }));
    }
    if o.lightly {
        ho.skip = Some(Arc::new(relay_only));
    }
    taps
}

/// Reads session s's output into inner's sink: the conn, running.
fn serve(inner: Arc<Inner>, s: Arc<Session>, reader: headless::Reader) -> Box<dyn Conn> {
    {
        let on = inner.clone();
        let end = inner.clone();
        let mut n = Neutral::new();
        reader.spawn(
            s,
            move |ev| on.relay(&mut n, ev),
            move |e| {
                end.ended.store(true, Ordering::SeqCst);
                // Whoever waits on an answer hears the session has ended.
                lock(&end.waits).clear();
                end.sink.ended(e.map(|e| Box::new(e) as adapter::Error));
            },
        );
    }
    Box::new(ClaudeConn(inner))
}

/// What a Claude session passed across an upgrade keeps beyond its
/// files: its session, sign-in and control requests' count (Rust-only).
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct Passed {
    sid: String,
    login: String,
    root: bool,
    acct_name: String,
    acct_dir: String,
    init_id: String,
    seq: i64,
    guard_pid: i32,
}

impl Adopter for Claude {
    /// Takes up the claude -p a host passed to itself across an exec: its
    /// process runs on, told nothing; its output read from where the last
    /// build stopped.
    fn adopt(
        &self,
        o: StartOptions,
        u: adapter::Upgrade,
        sink: Arc<dyn Sink>,
    ) -> adapter::Result<Box<dyn Conn>> {
        let p: Passed = serde_json::from_value(u.state.clone())?;
        let fd = |n: &str| {
            u.fds
                .get(n)
                .copied()
                .ok_or_else(|| err(format!("no {n} was passed")))
        };
        let guard = u.fds.get("guard").map(|&fd| (p.guard_pid, fd));
        let k = headless::Kept {
            pid: u.pid,
            fds: headless::Fds {
                stdin: fd("stdin")?,
                stdout: fd("stdout")?,
                stderr: fd("stderr")?,
                guard,
            },
            unread: u.unread,
            seq: p.seq,
        };
        let acct = Account::at(&p.acct_name, &p.acct_dir);
        let tools = o.tools.clone();
        let mut ho = headless::Options {
            account: acct.clone(),
            dir: o.dir.clone(),
            ..headless::Options::default()
        };
        let taps = set_tap(&o, &tools, &mut ho);
        let (s, reader) = headless::adopt(&ho, k)?;
        let inner = Arc::new(Inner {
            s: s.clone(),
            sink,
            tools,
            init_id: p.init_id,
            carried: Mutex::new(String::new()),
            acct,
            root: p.root,
            run_as: self.config_source(),
            login: p.login,
            taps,
            dir: o.dir.clone(),
            sid: Mutex::new(if p.sid.is_empty() {
                o.session_id.clone()
            } else {
                p.sid
            }),
            asks: Mutex::new(HashMap::new()),
            waits: Mutex::new(HashMap::new()),
            live: Mutex::new(crate::usage::Usage::default()),
            stale: AtomicBool::new(false),
            ended: AtomicBool::new(false),
        });
        Ok(serve(inner, s, reader))
    }
}

impl adapter::Upgrader for ClaudeConn {
    /// Holds the session between two of its lines and says how the next
    /// build reaches it: never while Claude Code waits on an answer, or
    /// rush on one of its.
    fn pass(&self) -> adapter::Result<adapter::Upgrade> {
        let c = &self.0;
        if c.ended.load(Ordering::SeqCst) {
            return Err(err("the session has ended"));
        }
        if !lock(&c.asks).is_empty() {
            return Err(err("it's waiting on your answer"));
        }
        if !lock(&c.waits).is_empty() {
            return Err(err("it's answering a question of rush's"));
        }
        let unread = c.s.pause(Duration::from_secs(5))?;
        if !lock(&c.asks).is_empty() || !lock(&c.waits).is_empty() {
            c.s.resume();
            return Err(err("it asked something as it was passed"));
        }
        let k = c.s.kept(unread);
        let mut fds = std::collections::BTreeMap::new();
        fds.insert("stdin".to_string(), k.fds.stdin);
        fds.insert("stdout".to_string(), k.fds.stdout);
        fds.insert("stderr".to_string(), k.fds.stderr);
        let mut guard_pid = 0;
        if let Some((pid, fd)) = k.fds.guard {
            fds.insert("guard".to_string(), fd);
            guard_pid = pid;
        }
        let p = Passed {
            sid: lock(&c.sid).clone(),
            login: c.login.clone(),
            root: c.root,
            acct_name: c.acct.name.clone(),
            acct_dir: c.acct.config_dir.to_string_lossy().into_owned(),
            init_id: c.init_id.clone(),
            seq: k.seq,
            guard_pid,
        };
        Ok(adapter::Upgrade {
            pid: k.pid,
            fds,
            unread: k.unread,
            state: serde_json::to_value(p)?,
        })
    }

    fn keep(&self) {
        self.0.s.resume();
    }
}

impl Orphans for Claude {
    /// Its sessions are headless Claude Codes.
    fn runs_session(&self, args: &[String], session_id: &str) -> bool {
        headless::runs_session(args, session_id)
    }
}

/// Output a caller that has the lines passes on without reading: streamed
/// deltas and tool results (user messages). They are most of what Claude
/// Code writes, and the biggest lines.
fn relay_only(l: &[u8]) -> bool {
    l.starts_with(br#"{"type":"stream_event""#) || l.starts_with(br#"{"type":"user""#)
}

/// The server a tool of Claude Code's name is one of rush's, and the
/// tool's own name.
fn tool_of<'a>(tools: &'a [ToolServer], name: &'a str) -> Option<(&'a ToolServer, &'a str)> {
    tools.iter().find_map(|t| {
        name.strip_prefix("mcp__")?
            .strip_prefix(t.name.as_str())?
            .strip_prefix("__")
            .map(|rest| (t, rest))
    })
}

/// The tools that never ask, by Claude Code's names.
fn trusted(tools: &[ToolServer]) -> Vec<String> {
    tools
        .iter()
        .flat_map(|t| {
            t.trusted
                .iter()
                .map(move |n| format!("mcp__{}__{n}", t.name))
        })
        .collect()
}

fn is_trusted(tools: &[ToolServer], name: &str) -> bool {
    tool_of(tools, name).is_some_and(|(t, n)| t.trusted.iter().any(|x| x == n))
}

/// A control request for rush's own tools: an MCP message, or a
/// permission check for one that never asks. It stays between Claude Code
/// and rush.
fn own_traffic(tools: &[ToolServer], l: &[u8]) -> bool {
    if !l.starts_with(br#"{"type":"control_request""#) {
        return false;
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct E {
        request: R,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct R {
        subtype: Option<String>,
        server_name: Option<String>,
        tool_name: Option<String>,
    }
    let Ok(e) = serde_json::from_slice::<E>(l) else {
        return false;
    };
    let r = e.request;
    let sub = r.subtype.unwrap_or_default();
    (sub == "mcp_message"
        && r.server_name
            .is_some_and(|s| tools.iter().any(|t| t.name == s)))
        || (sub == "can_use_tool" && r.tool_name.is_some_and(|n| is_trusted(tools, &n)))
}

/// The reading in raw as the login's, when it is one and not the last one
/// again inside half a minute; it is kept as the last then (conn.go:308-330).
fn fresh_reading(
    live: &Mutex<crate::usage::Usage>,
    login: &str,
    raw: &[u8],
    at: Ms,
) -> Option<crate::usage::Usage> {
    let mut u = crate::usage::live_usage(raw, at)?;
    if login.is_empty() {
        return None;
    }
    let mut last = lock(live);
    if u.five_hour == last.five_hour
        && u.seven_day == last.seven_day
        && u.fetched_at - last.fetched_at < 30_000
    {
        return None;
    }
    u.account_id = login.to_string();
    *last = u.clone();
    Some(u)
}

/// What a session's reader thread and its callers share.
struct Inner {
    s: Arc<Session>,
    sink: Arc<dyn Sink>,
    tools: Vec<ToolServer>,
    init_id: String,
    /// Imported history, acknowledged by initialize.
    carried: Mutex<String>,
    acct: Account,
    /// Whether it was started for ~/.claude and runs in the home of the
    /// login in use then.
    root: bool,
    /// Where to read which login new sessions run as now.
    run_as: crate::adapter::ConfigFn,
    /// The account it started signed in as: its plan usage readings are
    /// that login's.
    login: String,
    /// Its lines reach StartOptions.tap.
    taps: bool,
    /// The folder it started in.
    dir: String,
    /// Its session id, as it last said: see heard.
    sid: Mutex<String>,
    /// Approvals and questions not yet answered.
    asks: Mutex<HashMap<String, PermissionRequest>>,
    /// Control requests out, by id.
    waits: Mutex<HashMap<String, mpsc::Sender<ControlReply>>>,
    /// The last plan usage reading it shared (Go's conn.live).
    live: Mutex<crate::usage::Usage>,
    /// As stale last found it, to log when it flips.
    stale: AtomicBool,
    ended: AtomicBool,
}

impl Inner {
    /// One event of the session's, on its reader thread.
    fn relay(&self, n: &mut Neutral, ev: HEvent) {
        if let HEvent::Init(i) = &ev
            && !i.session_id.is_empty()
        {
            *lock(&self.sid) = i.session_id.clone();
        }
        if self.own(&ev) {
            return;
        }
        for out in n.event(ev, now()) {
            self.sink.event(out);
        }
    }

    /// Takes what the conn answers itself rather than passing on, and
    /// notes what it needs of the rest: whether ev was all its own.
    fn own(&self, ev: &HEvent) -> bool {
        match ev {
            HEvent::PermissionRequest(e) => {
                if is_trusted(&self.tools, &e.tool) {
                    // Asked despite --allowedTools (a mode that asks for
                    // everything): they only draw, so yes.
                    let _ = self.s.allow(e, None, false);
                    return true;
                }
                lock(&self.asks).insert(e.id.clone(), e.clone());
            }
            HEvent::PermissionCancelled(id) => {
                lock(&self.asks).remove(id);
            }
            HEvent::McpRequest(e) => {
                for t in &self.tools {
                    let Some(h) = t.handle.clone().filter(|_| t.name == e.server) else {
                        continue;
                    };
                    // A plugin's tool may take a while; the session carries
                    // on.
                    let s = self.s.clone();
                    let (id, msg) = (e.id.clone(), e.message.clone());
                    std::thread::spawn(move || {
                        let null = RawJson::new("null").expect("null is JSON");
                        let reply = h(&msg.unwrap_or(null).0);
                        let _ = s.reply_mcp(&id, &reply);
                    });
                }
                return true;
            }
            HEvent::ControlReply(e) => {
                if let Some(w) = lock(&self.waits).remove(&e.id) {
                    let _ = w.send(e.clone());
                    return true;
                }
                if e.id == self.init_id && e.error.is_empty() {
                    let carried = std::mem::take(&mut *lock(&self.carried));
                    if !carried.is_empty() {
                        self.sink.event(Event::Init(Box::new(event::Init {
                            session_id: carried,
                            ..event::Init::default()
                        })));
                    }
                    let list = headless::event::commands(e)
                        .into_iter()
                        .map(|k| event::Command {
                            name: k.name,
                            description: k.description,
                            argument_hint: k.argument_hint,
                            aliases: k.aliases,
                        })
                        .collect();
                    self.sink.event(Event::Commands(event::Commands { list }));
                }
                return true;
            }
            HEvent::RateLimit(e) => self.share_usage(e),
            _ => {}
        }
        false
    }

    /// Passes the plan usage Claude Code reports with each request to every
    /// rush, as a reading of the login it runs on: the header and switching
    /// accounts then go by it, not by a fetch minutes old. A reading like
    /// the last goes only every half minute (Go's conn.go:308-338
    /// shareUsage).
    fn share_usage(&self, ev: &crate::headless::event::RateLimit) {
        let Some(raw) = ev.raw.as_ref() else { return };
        let Some(u) = fresh_reading(&self.live, &self.login, raw.get().as_bytes(), now()) else {
            return;
        };
        let acct = self.acct.clone();
        // Off the reading thread. Recorded only while the folder is still
        // signed in as it started: after a switch, the reading may be the
        // new login's.
        std::thread::spawn(move || {
            let login = u.account_id.clone();
            let _ = crate::accounts::record_live_usage(&acct, &login, u);
        });
    }

    /// Sends a control request with send, and waits for its reply.
    fn wait_on(
        &self,
        send: impl FnOnce() -> adapter::Result<String>,
    ) -> adapter::Result<ControlReply> {
        let (tx, rx) = mpsc::channel();
        let id = {
            // Held while sending, so the reply can't be read before it's
            // awaited.
            let mut w = lock(&self.waits);
            if self.ended.load(Ordering::SeqCst) {
                return Err(err("the session ended before it answered"));
            }
            let id = send()?;
            w.insert(id.clone(), tx);
            id
        };
        match rx.recv_timeout(ASK_WITHIN) {
            Ok(r) if !r.error.is_empty() => Err(err(r.error)),
            Ok(r) => Ok(r),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                lock(&self.waits).remove(&id);
                Err(err("context deadline exceeded"))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(err("the session ended before it answered"))
            }
        }
    }

    /// The request id asked, once: answering it forgets it.
    fn take(&self, id: &str) -> adapter::Result<PermissionRequest> {
        lock(&self.asks)
            .remove(id)
            .ok_or_else(|| err("nothing is waiting on that answer"))
    }
}

/// A running claude -p as rush's Conn.
pub struct ClaudeConn(Arc<Inner>);

impl Conn for ClaudeConn {
    fn send(&self, input: Input) -> adapter::Result<()> {
        let mut imgs = Vec::with_capacity(input.images.len());
        for p in &input.images {
            imgs.push(Image {
                media_type: media_type(p).into(),
                data: std::fs::read(p)?,
            });
        }
        self.0.s.send_with(&input.text, &imgs)
    }

    /// Answers an approval with one of the options headless's neutral gave
    /// it: allow, always or deny.
    fn answer(&self, approval_id: &str, option_id: &str) -> adapter::Result<()> {
        match option_id {
            "allow" => self.allow(approval_id, None, false),
            "always" => self.allow(approval_id, None, true),
            _ => self.deny(approval_id, "", false),
        }
    }

    fn interrupt(&self) -> adapter::Result<()> {
        self.0.s.interrupt()
    }
    fn set_model(&self, model: &str) -> adapter::Result<()> {
        self.0.s.set_model(model)
    }
    fn set_mode(&self, mode: &str) -> adapter::Result<()> {
        self.0.s.set_permission_mode(mode)
    }
    fn close(&self) -> adapter::Result<()> {
        self.0.s.stop(Duration::from_secs(3))
    }

    fn answerer(&self) -> Option<&dyn Answerer> {
        Some(self)
    }
    fn responder(&self) -> Option<&dyn Responder> {
        Some(self)
    }
    fn asker(&self) -> Option<&dyn Asker> {
        Some(self)
    }
    fn context_reader(&self) -> Option<&dyn ContextReader> {
        Some(self)
    }
    fn task_stopper(&self) -> Option<&dyn TaskStopper> {
        Some(self)
    }
    fn backgrounder(&self) -> Option<&dyn Backgrounder> {
        Some(self)
    }
    fn homed(&self) -> Option<&dyn Homed> {
        Some(self)
    }
    fn hearer(&self) -> Option<&dyn Hearer> {
        Some(self)
    }
    fn cache_keeper(&self) -> Option<&dyn CacheKeeper> {
        Some(self)
    }
    fn subagent_keeper(&self) -> Option<&dyn SubagentKeeper> {
        Some(self)
    }
    fn upgrader(&self) -> Option<&dyn adapter::Upgrader> {
        Some(self)
    }

    /// Its readings go where Claude's usage is kept (Go's conn.go:341
    /// KeepsQuota, an unconditional marker).
    fn keeps_quota(&self) -> bool {
        true
    }

    /// Whether the folder is signed in as another login than the one it
    /// started on.
    fn stale(&self) -> bool {
        let c = &self.0;
        let now = !c.login.is_empty() && crate::login::signed_in_as(&c.acct) != c.login;
        if c.stale.swap(now, Ordering::SeqCst) != now {
            let mut f = rush_events::Fields::new();
            f.insert("stale".into(), now.into());
            f.insert("login".into(), c.login.clone().into());
            f.insert("dir".into(), c.acct.config_dir.display().to_string().into());
            rush_events::log("login-stale", f);
        }
        now
    }

    fn pid(&self) -> Option<i32> {
        Some(self.0.s.pid())
    }

    fn taps(&self) -> bool {
        self.0.taps
    }
}

impl Responder for ClaudeConn {
    /// Lets a call run, with its input as changed when input is set.
    fn allow(
        &self,
        approval_id: &str,
        input: Option<&RawValue>,
        always: bool,
    ) -> adapter::Result<()> {
        let r = self.0.take(approval_id)?;
        let input = match input {
            Some(i) if !i.get().is_empty() => Some(if r.tool == "AskUserQuestion" {
                over(r.input.as_ref(), i)
            } else {
                RawJson(i.to_owned())
            }),
            _ => None,
        };
        self.0.s.allow(&r, input.as_ref(), always)
    }

    /// Refuses a call, saying why when message is set; interrupt stops the
    /// turn too.
    fn deny(&self, approval_id: &str, message: &str, interrupt: bool) -> adapter::Result<()> {
        let r = self.0.take(approval_id)?;
        self.0.s.deny(&r, message, interrupt)
    }
}

/// input with the keys of top written over it: answers go into the
/// question's own input, so whatever else Claude put there stays.
fn over(input: Option<&RawJson>, top: &RawValue) -> RawJson {
    let mine = || RawJson(top.to_owned());
    let base: Option<Map<String, Value>> = match input {
        None => Some(Map::new()),
        Some(i) if i.get() == "null" => Some(Map::new()),
        Some(i) => serde_json::from_str(i.get()).ok(),
    };
    let (Some(mut base), Ok(add)) = (base, serde_json::from_str::<Map<String, Value>>(top.get()))
    else {
        return mine();
    };
    base.extend(add);
    RawJson::new(&Value::Object(base).to_string()).unwrap_or_else(|_| mine())
}

impl Answerer for ClaudeConn {
    /// Answers an AskUserQuestion. Claude takes several choices as one
    /// answer, joined.
    fn answer_question(
        &self,
        id: &str,
        answers: &HashMap<String, Vec<String>>,
    ) -> adapter::Result<()> {
        let r = self.0.take(id)?;
        let (_, qs) = r.questions();
        let joined: BTreeMap<String, String> = answers
            .iter()
            .map(|(q, labels)| (q.clone(), labels.join(", ")))
            .collect();
        let input = r.answer_input(&qs, &joined);
        self.0.s.allow(&r, Some(&input), false)
    }
}

impl Asker for ClaudeConn {
    /// Passes a control request through and returns its reply's body.
    fn ask(&self, req: &RawValue) -> adapter::Result<RawJson> {
        let r = self.0.wait_on(|| self.0.s.ask(req.get()))?;
        Ok(r.body
            .unwrap_or_else(|| RawJson::new("null").expect("null is JSON")))
    }
}

/// What the bump asks: Claude Code answers a side question from the
/// conversation as its last request sent it (getLastCacheSafeParams), so
/// the request reads that whole cached prefix, which starts its life over.
/// An empty history keeps /btw's own threads out of it; nothing is written
/// to the transcript, and no turn starts.
const KEEP_WARM: &str =
    "rush is keeping this conversation's prompt cache warm. Reply with just: ok";

impl CacheKeeper for ClaudeConn {
    fn keep_warm(&self) -> adapter::Result<()> {
        let req = json!({"subtype": "side_question", "question": KEEP_WARM, "history": []});
        let r = self.0.wait_on(|| self.0.s.ask(&req.to_string()))?;
        bumped(r.body.as_ref().map(|b| b.get()))
    }
}

/// Whether a side question's reply says a request was made. No answer
/// (null) means none was: a process with no request of its own yet has
/// nothing cached to answer from. A synthetic one stands in for a request
/// that failed.
fn bumped(body: Option<&str>) -> adapter::Result<()> {
    let v: Value = body
        .and_then(|b| serde_json::from_str(b).ok())
        .unwrap_or(Value::Null);
    if !v.get("response").is_some_and(Value::is_string) {
        return Err(err("no request was made: nothing of its own is cached yet"));
    }
    if v.get("synthetic").and_then(Value::as_bool) == Some(true) {
        return Err(err("the request didn't answer"));
    }
    Ok(())
}

/// A subagent's long call moved to the background, by its tool_use_id:
/// Claude Code's `background_tasks` control request (ctrl+b's), which
/// finds the call among every foreground task, a subagent's too (a Bash
/// call is one once it has run 2 seconds), and answers whether it moved
/// it. The call carries on and tells the subagent when it ends; the
/// subagent's turn goes on at once, so its next request reads its cache
/// while it's warm. Claude Code 2.1.316 has no request that reads a
/// subagent's cache outside its turn: side_question reads only the main
/// conversation's.
impl SubagentKeeper for ClaudeConn {
    fn move_call(&self, call_id: &str) -> adapter::Result<bool> {
        if call_id.is_empty() {
            return Err(err("no call to move"));
        }
        let req = json!({"subtype": "background_tasks", "tool_use_id": call_id});
        let r = self.0.wait_on(|| self.0.s.ask(&req.to_string()))?;
        Ok(moved(r.body.as_ref().map(|b| b.get())))
    }
}

/// Whether a background_tasks reply says the call was found and moved.
fn moved(body: Option<&str>) -> bool {
    body.and_then(|b| serde_json::from_str::<Value>(b).ok())
        .and_then(|v| v.get("backgrounded").and_then(Value::as_bool))
        .unwrap_or(false)
}

impl ContextReader for ClaudeConn {
    /// What fills the context window, as /context counts it.
    fn context_usage(&self) -> adapter::Result<usage::Context> {
        let r = self.0.wait_on(|| self.0.s.ask_context_usage())?;
        let mut u = headless::parse_context_usage(&r)?;
        u.at = now();
        Ok(u)
    }
}

impl TaskStopper for ClaudeConn {
    fn stop_task(&self, id: &str) -> adapter::Result<()> {
        self.0.s.stop_task(id)
    }
}

impl Backgrounder for ClaudeConn {
    fn background(&self, call_id: &str) -> adapter::Result<()> {
        self.0.s.background(call_id)
    }
}

impl Homed for ClaudeConn {
    /// The login home it runs in, for ~/.claude's sessions.
    fn home(&self) -> String {
        if !self.0.root {
            return String::new();
        }
        self.0.acct.config_dir.display().to_string()
    }
    /// Whether ~/.claude's new sessions run as another login now
    /// (conn.go:365-367).
    fn moved(&self) -> bool {
        // Only a session on ~/.claude reads who the login in use is.
        self.0.root
            && moved_off(
                true,
                &crate::adapter::run_as_in(&(self.0.run_as)()),
                &self.0.acct,
            )
    }
    fn login(&self) -> String {
        self.0.login.clone()
    }
}

impl Hearer for ClaudeConn {
    /// Whether Claude Code took in a message of text sent at since: it
    /// writes each to its transcript as it queues it.
    fn heard(&self, text: &str, since: Ms) -> Option<bool> {
        let sid = lock(&self.0.sid).clone();
        if sid.is_empty() {
            return None;
        }
        let path = self.0.acct.find_transcript(&self.0.dir, &sid);
        heard_in(&path, text, since)
    }
}

/// An image's type by its file's extension (Go's mime.TypeByExtension),
/// PNG when it can't say.
fn media_type(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

/// Writes lines, another agent's conversation, as the transcript of
/// session sid in cwd, for Claude Code to resume as its own: your messages
/// as user lines, its answers as synthetic assistant ones, each after the
/// last.
fn write_carried(
    acct: &Account,
    cwd: &str,
    sid: &str,
    lines: &[PreviewLine],
) -> adapter::Result<()> {
    use std::io::Write as _;
    let p = acct.transcript_path(cwd, sid);
    if let Some(d) = p.parent() {
        crate::files::mkdir_private(d)?;
    }
    let mut out = Vec::new();
    let mut parent = Value::Null;
    for l in lines {
        let at = if l.at == 0 { now() } else { l.at };
        let id = uuid()?;
        let mut line = json!({"parentUuid": parent, "isSidechain": false, "userType": "external", "cwd": cwd,
            "sessionId": sid, "type": l.role, "uuid": id, "timestamp": format_rfc3339(at)});
        line["message"] = if l.role == "assistant" {
            json!({"id": format!("msg_{id}"), "type": "message", "role": "assistant", "model": "<synthetic>",
                "content": [{"type": "text", "text": l.text}], "stop_reason": "end_turn", "stop_sequence": null,
                "usage": {"input_tokens": 0, "output_tokens": 0}})
        } else {
            json!({"role": "user", "content": l.text})
        };
        serde_json::to_writer(&mut out, &line)?;
        out.push(b'\n');
        parent = Value::String(id);
    }
    // A start that died left one nothing resumed from: it's written over.
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new()
        .mode(0o600)
        .create(true)
        .truncate(true)
        .write(true)
        .open(&p)?;
    if let Err(e) = f.write_all(&out) {
        drop(f);
        let _ = std::fs::remove_file(&p);
        return Err(e.into());
    }
    Ok(())
}

/// A random (v4) UUID.
fn uuid() -> std::io::Result<String> {
    use std::io::Read as _;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod keep_tests {
    // A bump counts only when Claude Code made the request: a reply with
    // an answer, not null (nothing to answer from) nor a stand-in.
    #[test]
    fn a_bump_needs_a_real_answer() {
        for (body, ok) in [
            (Some(r#"{"response":"ok","synthetic":false}"#), true),
            (Some(r#"{"response":"ok"}"#), true),
            (Some(r#"{"response":null}"#), false),
            (
                Some(r#"{"response":"(No answer available)","synthetic":true}"#),
                false,
            ),
            (None, false),
        ] {
            assert_eq!(super::bumped(body).is_ok(), ok, "{body:?}");
        }
    }
}
