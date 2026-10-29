//! A running `claude -p` as rush's Conn, and Claude as a Driver. Port of
//! internal/adapters/claude/conn.go (Start, relayOnly, conn and its parts,
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
    self, Answerer, Asker, Backgrounder, Conn, ContextReader, Driver, Hearer, Homed, Input,
    Orphans, Responder, Sink, StartOptions, TaskStopper, ToolServer,
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

impl Driver for Claude {
    /// Runs claude -p for rush to draw.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        let mut acct = account(&o.profile);
        let mut root = false;
        if acct.is_default() {
            // ~/.claude runs as the login in use, in its home. Login homes
            // aren't ported yet: the login in use is ~/.claude's own.
            acct = self.run_as();
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
                json!([{"matcher": "Bash", "hooks": [{"type": "command", "command": o.bash_hook, "timeout": 10}]}]),
            );
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
        if !o.temp_dir.is_empty() && std::fs::create_dir_all(&o.temp_dir).is_ok() {
            let t = &o.temp_dir;
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
        // Checkpoints, as Claude Code keeps them in a terminal, so a rewind
        // can put the files back too. The session's own environment goes
        // last, to have the last word.
        ho.env.push(headless::CHECKPOINT_ENV.into());
        ho.env.extend(o.env.iter().cloned());
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let taps = o.tap.is_some();
        if let Some(tap) = o.tap.clone() {
            let tools = tools.clone();
            ho.tap = Some(Arc::new(move |line: &[u8]| {
                if !own_traffic(&tools, line) {
                    tap(line);
                }
            }));
        }
        if o.lightly {
            ho.skip = Some(Arc::new(relay_only));
        }
        let login = acct.signed_in_as();
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
            login,
            taps,
            dir: o.dir.clone(),
            sid: Mutex::new(o.session_id.clone()),
            asks: Mutex::new(HashMap::new()),
            waits: Mutex::new(HashMap::new()),
            stale: AtomicBool::new(false),
            ended: AtomicBool::new(false),
        });
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
        Ok(Box::new(ClaudeConn(inner)))
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
            _ => {}
        }
        false
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

    /// Whether the folder is signed in as another login than the one it
    /// started on.
    fn stale(&self) -> bool {
        let c = &self.0;
        let now = !c.login.is_empty() && c.acct.signed_in_as() != c.login;
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
    /// Whether ~/.claude's new sessions run as another login now. With no
    /// login homes yet they always run in ~/.claude itself.
    fn moved(&self) -> bool {
        false
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
        std::fs::create_dir_all(d)?;
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
