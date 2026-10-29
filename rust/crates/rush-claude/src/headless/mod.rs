//! Claude Code run as a bare agent loop: `claude -p` speaking stream-json
//! on stdin and stdout. Claude Code keeps the model, tools, hooks, MCP and
//! compaction; rush draws the session and answers its permission prompts,
//! so no terminal UI, PTY host or daemon runs for it.
//!
//! Port of internal/adapters/claude/headless: session.go (Options, Start,
//! the writer, the reader, Send, Content, Allow, Deny, the control
//! requests, Stop and what it ends), context.go (AskContextUsage, Ask),
//! rewind.go (RewindFiles, Recap); event.go and question.go are `event`,
//! neutral.go is `neutral`.
//!
//! Go hands a session's events out on a channel a goroutine reads. Here
//! the reader thread calls the handler [`Reader::spawn`] is given, in
//! order, and then its end: the caller keeps no thread of its own.

pub mod event;
mod neutral;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rush_core::adapter::{FileRewind, Result, Tap};
use rush_core::usage;
use rush_core::wire::RawJson;
use rush_sys::proc;
use serde_json::{Map, Value, json};

pub use event::{ControlReply, Event, PermissionRequest, decode};
pub(crate) use neutral::live_quota;

use crate::account::Account;

/// Says which lines not to decode at all: they reach the tap and nothing
/// else.
pub type Skip = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;

/// Which conversation to run and where.
#[derive(Clone)]
pub struct Options {
    pub account: Account,
    pub dir: String,
    /// Continues a saved conversation by its session id. session_id
    /// instead names a new one; leave both empty to let Claude Code pick.
    pub resume: String,
    pub session_id: String,
    pub model: String,
    /// low, medium, high, xhigh, max; fixed for the process's life.
    pub effort: String,
    /// default, acceptEdits, plan, auto, ...
    pub permission_mode: String,
    pub flags: Vec<String>,
    /// The claude executable; empty means "claude" on PATH.
    pub binary: String,
    /// When set, sees every output line before it is decoded.
    pub tap: Option<Tap>,
    /// When set, says which lines not to decode at all: they reach tap and
    /// nothing else. A host that only relays streamed deltas and tool
    /// results needn't pay to take them apart.
    pub skip: Option<Skip>,
    /// Added to the account's environment.
    pub env: Vec<String>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            account: Account::home(),
            dir: String::new(),
            resume: String::new(),
            session_id: String::new(),
            model: String::new(),
            effort: String::new(),
            permission_mode: String::new(),
            flags: Vec::new(),
            binary: String::new(),
            tap: None,
            skip: None,
            env: Vec::new(),
        }
    }
}

impl Options {
    fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--verbose",
            // stdio routes every permission prompt to us as a can_use_tool
            // request; without it headless sessions deny them silently.
            "--permission-prompt-tool",
            "stdio",
        ]
        .map(String::from)
        .to_vec();
        if !self.resume.is_empty() {
            args.extend(["--resume".into(), self.resume.clone()]);
        } else if !self.session_id.is_empty() {
            args.extend(["--session-id".into(), self.session_id.clone()]);
        }
        for (flag, v) in [
            ("--model", &self.model),
            ("--effort", &self.effort),
            ("--permission-mode", &self.permission_mode),
        ] {
            if !v.is_empty() {
                args.extend([flag.to_string(), v.clone()]);
            }
        }
        args.extend(self.flags.iter().cloned());
        args
    }

    fn program(&self) -> &str {
        if self.binary.is_empty() {
            "claude"
        } else {
            &self.binary
        }
    }
}

/// Whether args are a headless Claude Code on session_id, as start runs
/// it.
pub fn runs_session(args: &[String], session_id: &str) -> bool {
    if !args.iter().any(|a| a == "-p") || !args.iter().any(|a| a == "stream-json") {
        return false;
    }
    args.windows(2)
        .any(|w| (w[0] == "--resume" || w[0] == "--session-id") && w[1] == session_id)
}

/// Why a session ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    /// Claude Code exited (or was ended) on its own: its output was read
    /// to the end. A stop on purpose is no failure.
    pub exited: bool,
    pub text: String,
}

impl std::fmt::Display for Ended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for Ended {}

/// Writes queue here and a thread feeds them to stdin, so a caller holding
/// a lock never blocks on Claude Code (which may itself be blocked writing
/// its output).
#[derive(Default)]
struct WriteQueue {
    buf: Vec<Vec<u8>>,
    closed: bool,
    err: Option<String>,
    /// The process has gone: nothing more is written.
    gone: bool,
}

/// What a session's threads share.
struct Shared {
    w: Mutex<WriteQueue>,
    wsig: Condvar,
    /// Set once the process has exited and its output is read: why it
    /// ended, None when it ended well.
    done: Mutex<Option<Option<Ended>>>,
    dsig: Condvar,
    stderr: Tail,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One running `claude -p`. Any thread may call it.
pub struct Session {
    pid: i32,
    seq: AtomicI64,
    sh: Arc<Shared>,
}

/// A session's output, not yet read: [`Reader::spawn`] reads it on a
/// thread of its own. Dropped unread, the session's output is never read.
pub struct Reader {
    child: Child,
    sh: Arc<Shared>,
    tap: Option<Tap>,
    skip: Option<Skip>,
    guard: Option<rush_sys::pgguard::Guard>,
    stderr_done: mpsc::Receiver<()>,
}

/// Launches Claude Code for o. Its output is read once the Reader is
/// spawned.
pub fn start(o: &Options) -> Result<(Arc<Session>, Reader)> {
    let mut cmd = Command::new(o.program());
    cmd.args(o.args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own process group, so stopping the session takes its shells
        // too.
        .process_group(0)
        .env_clear();
    if !o.dir.is_empty() {
        cmd.current_dir(&o.dir);
    }
    // rush draws a question's option previews, which Claude Code only
    // offers a headless session when told the format.
    let mut env = o.account.env();
    env.push("CLAUDE_CODE_QUESTION_PREVIEW_FORMAT=markdown".into());
    env.extend(o.env.iter().cloned());
    for e in &env {
        if let Some((k, v)) = e.split_once('=') {
            cmd.env(k, v);
        }
    }
    let mut child = cmd.spawn()?;
    let pid = child.id() as i32;
    let sh = Arc::new(Shared {
        w: Mutex::new(WriteQueue::default()),
        wsig: Condvar::new(),
        done: Mutex::new(None),
        dsig: Condvar::new(),
        stderr: Tail::default(),
    });
    let stdin = child.stdin.take().expect("stdin is piped");
    let w = sh.clone();
    std::thread::Builder::new()
        .name("claude-stdin".into())
        .spawn(move || writer(&w, stdin))?;
    let mut err = child.stderr.take().expect("stderr is piped");
    let e = sh.clone();
    let (stderr_tx, stderr_done) = mpsc::channel();
    std::thread::Builder::new()
        .name("claude-stderr".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                e.stderr.write(&buf[..n]);
            }
            let _ = stderr_tx.send(());
        })?;
    let guard = rush_sys::pgguard::watch(pid);
    Ok((
        Arc::new(Session {
            pid,
            seq: AtomicI64::new(0),
            sh: sh.clone(),
        }),
        Reader {
            child,
            sh,
            tap: o.tap.clone(),
            skip: o.skip.clone(),
            guard,
            stderr_done,
        },
    ))
}

/// Feeds queued writes to stdin until the session stops.
fn writer(sh: &Shared, mut stdin: ChildStdin) {
    loop {
        let (batch, closed) = {
            let mut q = lock(&sh.w);
            while q.buf.is_empty() && !q.closed && !q.gone {
                q = sh.wsig.wait(q).unwrap_or_else(|e| e.into_inner());
            }
            if q.gone && !q.closed && q.buf.is_empty() {
                return;
            }
            (std::mem::take(&mut q.buf), q.closed)
        };
        for b in batch {
            if let Err(e) = stdin.write_all(&b) {
                lock(&sh.w).err = Some(e.to_string());
                break;
            }
        }
        let _ = stdin.flush();
        if closed {
            return; // stdin closes as it drops
        }
    }
}

impl Reader {
    /// Reads the session's output on a thread of its own: on gets each
    /// event, in order, then ended why it ended, once.
    pub fn spawn(
        self,
        session: Arc<Session>,
        on: impl FnMut(Event) + Send + 'static,
        ended: impl FnOnce(Option<Ended>) + Send + 'static,
    ) {
        let name = format!("claude-{}", session.pid);
        let run = move || {
            let err = self.read(&session, on);
            ended(err);
        };
        if let Err(e) = std::thread::Builder::new().name(name).spawn(run) {
            // No thread to read with: the session can't be followed.
            eprintln!("rush: can't read Claude Code's output: {e}");
        }
    }

    fn read(mut self, s: &Session, mut on: impl FnMut(Event)) -> Option<Ended> {
        let stdout = self.child.stdout.take().expect("stdout is piped");
        let mut r = BufReader::with_capacity(64 << 10, stdout);
        let mut line = Vec::new();
        let mut read_err = None;
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    read_err = Some(e);
                    break;
                }
            }
            let l = line.strip_suffix(b"\n").unwrap_or(&line);
            if l.is_empty() {
                continue;
            }
            if let Some(tap) = &self.tap {
                tap(l);
            }
            if self.skip.as_ref().is_some_and(|skip| skip(l)) {
                continue;
            }
            let Ok(ev) = decode(l) else { continue };
            if let Event::Other(o) = &ev
                && o.kind == "control_request"
            {
                // A request we never registered for; refuse it rather than
                // leave Claude Code waiting.
                let id = o
                    .raw
                    .as_ref()
                    .and_then(|r| serde_json::from_str::<Value>(r.get()).ok())
                    .and_then(|v| v.get("request_id")?.as_str().map(String::from))
                    .unwrap_or_default();
                let _ = s.reply(&id, Value::Null, "not supported by rush");
                continue;
            }
            on(ev);
        }
        drop(r);
        if read_err.is_some() {
            // Nobody reads its output any more, so it would block forever:
            // end it, and say why.
            // SAFETY: kill(2) takes plain integers.
            unsafe { libc::kill(-s.pid, libc::SIGKILL) };
        }
        let status = self.child.wait();
        if let Some(g) = self.guard.take() {
            g.release();
        }
        {
            let mut q = lock(&self.sh.w);
            q.gone = true;
            self.sh.wsig.notify_all();
        }
        // What it said last, once its stderr has closed; a shell it left
        // running may hold that open, so not for long.
        let _ = self.stderr_done.recv_timeout(Duration::from_millis(200));
        let mut err = match (read_err, status) {
            (Some(e), _) => Some(Ended {
                exited: false,
                text: format!("reading Claude Code's output: {e}"),
            }),
            (None, Ok(st)) if st.success() => None,
            (None, Ok(st)) => Some(Ended {
                exited: true,
                text: exit_text(st),
            }),
            (None, Err(e)) => Some(Ended {
                exited: false,
                text: e.to_string(),
            }),
        };
        if let Some(e) = &mut err {
            let t = self.sh.stderr.text();
            if !t.is_empty() {
                e.text = format!("{}: {t}", e.text);
            }
        }
        *lock(&self.sh.done) = Some(err.clone());
        self.sh.dsig.notify_all();
        err
    }
}

/// How a process ended, as Go's exec error says it: "exit status 3",
/// "signal: killed".
fn exit_text(st: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (st.code(), st.signal()) {
        (Some(c), _) => format!("exit status {c}"),
        (None, Some(libc::SIGKILL)) => "signal: killed".into(),
        (None, Some(libc::SIGTERM)) => "signal: terminated".into(),
        (None, Some(s)) => format!("signal: {s}"),
        _ => "exit status unknown".into(),
    }
}

/// A message's image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    /// image/png, image/jpeg, image/gif, image/webp
    pub media_type: String,
    pub data: Vec<u8>,
}

fn err(msg: impl Into<String>) -> rush_core::adapter::Error {
    msg.into().into()
}

fn raw_value(r: Option<&RawJson>) -> Value {
    r.and_then(|r| serde_json::from_str(r.get()).ok())
        .unwrap_or(Value::Null)
}

impl Session {
    /// Claude Code's process id.
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Whether the process has exited and its output is read.
    pub fn done(&self) -> bool {
        lock(&self.sh.done).is_some()
    }

    /// Waits up to d for the session to end; whether it has.
    pub fn wait_for(&self, d: Duration) -> bool {
        let g = lock(&self.sh.done);
        let (g, _) = self
            .sh
            .dsig
            .wait_timeout_while(g, d, |d| d.is_none())
            .unwrap_or_else(|e| e.into_inner());
        g.is_some()
    }

    /// Why the session ended, once it has: None when it ended well. Blocks
    /// until then.
    pub fn err(&self) -> Option<Ended> {
        let g = lock(&self.sh.done);
        let g = self
            .sh
            .dsig
            .wait_while(g, |d| d.is_none())
            .unwrap_or_else(|e| e.into_inner());
        g.clone().flatten()
    }

    fn write(&self, v: &Value) -> Result<()> {
        let mut b = serde_json::to_vec(v)?;
        b.push(b'\n');
        let mut q = lock(&self.sh.w);
        if q.closed {
            return Err(err("session stopped"));
        }
        if let Some(e) = &q.err {
            return Err(err(e.clone()));
        }
        q.buf.push(b);
        self.sh.wsig.notify_all();
        Ok(())
    }

    /// Queues a user message. Sent mid-turn, Claude Code picks it up at
    /// its next step, as typing into a busy session does.
    pub fn send(&self, text: &str) -> Result<()> {
        self.send_with(text, &[])
    }

    /// Sends a message with images attached.
    pub fn send_with(&self, text: &str, images: &[Image]) -> Result<()> {
        let content = if images.is_empty() {
            Value::String(text.into())
        } else {
            Value::Array(content(text, images))
        };
        self.write(&json!({
            "type": "user",
            "message": {"role": "user", "content": content},
        }))
    }

    /// Lets a requested tool run. input is the tool input to use, the
    /// request's own when None; always adds the request's suggested rules
    /// so Claude Code stops asking for the same thing.
    pub fn allow(
        &self,
        req: &PermissionRequest,
        input: Option<&RawJson>,
        always: bool,
    ) -> Result<()> {
        let mut body = Map::new();
        body.insert("behavior".into(), "allow".into());
        body.insert(
            "updatedInput".into(),
            raw_value(input.or(req.input.as_ref())),
        );
        body.insert("toolUseID".into(), req.tool_use_id.clone().into());
        let sug = req.suggestions.as_ref().filter(|s| !s.get().is_empty());
        match sug {
            Some(s) if always => {
                body.insert("updatedPermissions".into(), raw_value(Some(s)));
                body.insert("decisionClassification".into(), "user_permanent".into());
            }
            _ => {
                body.insert("decisionClassification".into(), "user_temporary".into());
            }
        }
        self.reply(&req.id, Value::Object(body), "")
    }

    /// Refuses a requested tool. message is what Claude sees; interrupt
    /// also stops the turn.
    pub fn deny(&self, req: &PermissionRequest, message: &str, interrupt: bool) -> Result<()> {
        let message = if message.is_empty() {
            "The user declined this tool call."
        } else {
            message
        };
        self.reply(
            &req.id,
            json!({
                "behavior": "deny", "message": message, "interrupt": interrupt,
                "toolUseID": req.tool_use_id, "decisionClassification": "user_reject",
            }),
            "",
        )
    }

    fn reply(&self, id: &str, body: Value, err_text: &str) -> Result<()> {
        let resp = if err_text.is_empty() {
            json!({"subtype": "success", "request_id": id, "response": body})
        } else {
            json!({"subtype": "error", "request_id": id, "error": err_text})
        };
        self.write(&json!({"type": "control_response", "response": resp}))
    }

    /// Sends a request to Claude Code and returns its id; the answer
    /// arrives later as a ControlReply event.
    fn control(&self, req: Map<String, Value>) -> Result<String> {
        let id = format!("rush-{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1);
        self.write(&json!({"type": "control_request", "request_id": id, "request": req}))?;
        Ok(id)
    }

    fn control_of(&self, v: Value) -> Result<String> {
        match v {
            Value::Object(m) => self.control(m),
            _ => Err(err("a control request is an object")),
        }
    }

    /// Asks for the session's slash commands, models and account; the
    /// answer is a ControlReply for the returned id (read it with
    /// event::commands). It also registers the MCP servers the host runs
    /// in-process, whose messages then arrive as McpRequests; send it
    /// before the first message.
    ///
    /// It says rush stops background tasks one at a time (stop_task), so
    /// an interrupt stops only the turn and leaves running subagents be;
    /// without that, Claude Code kills them all with the turn.
    pub fn initialize(&self, servers: &[String]) -> Result<String> {
        let mut req = json!({"subtype": "initialize", "perTaskStopAffordance": true});
        if !servers.is_empty() {
            req["sdkMcpServers"] = json!(servers);
        }
        self.control_of(req)
    }

    /// Answers an McpRequest with its server's JSON-RPC reply.
    pub fn reply_mcp(&self, id: &str, reply: &RawJson) -> Result<()> {
        self.reply(id, json!({"mcp_response": raw_value(Some(reply))}), "")
    }

    /// Stops the current turn, as esc does.
    pub fn interrupt(&self) -> Result<()> {
        self.control_of(json!({"subtype": "interrupt", "reason": "interrupt"}))
            .map(drop)
    }

    /// Stops one background task, a subagent or a background shell, by
    /// its id; the turn and the other tasks carry on.
    pub fn stop_task(&self, id: &str) -> Result<()> {
        self.control_of(json!({"subtype": "stop_task", "task_id": id}))
            .map(drop)
    }

    /// Moves a Bash command or subagent the turn is waiting on into the
    /// background, as ctrl+b does in Claude Code; the turn carries on
    /// without it. An empty tool_use_id moves every one.
    pub fn background(&self, tool_use_id: &str) -> Result<()> {
        let mut req = json!({"subtype": "background_tasks"});
        if !tool_use_id.is_empty() {
            req["tool_use_id"] = tool_use_id.into();
        }
        self.control_of(req).map(drop)
    }

    /// Switches between default, acceptEdits, plan, auto, ...
    pub fn set_permission_mode(&self, mode: &str) -> Result<()> {
        self.control_of(json!({"subtype": "set_permission_mode", "mode": mode}))
            .map(drop)
    }

    /// Switches model for the next turn; empty resets to the default.
    pub fn set_model(&self, model: &str) -> Result<()> {
        let mut req = json!({"subtype": "set_model"});
        if !model.is_empty() {
            req["model"] = model.into();
        }
        self.control_of(req).map(drop)
    }

    /// Asks Claude Code what fills the context window; the answer is a
    /// ControlReply for the returned id (read it with parse_context_usage).
    /// It counts from the last response and local estimates, so it costs
    /// nothing.
    pub fn ask_context_usage(&self) -> Result<String> {
        self.control_of(json!({"subtype": "get_context_usage", "detail": "summary"}))
    }

    /// Sends any control request (req carries its subtype) and returns its
    /// id; the answer is a ControlReply for it.
    pub fn ask(&self, req: &str) -> Result<String> {
        let m: Map<String, Value> = serde_json::from_str(req)?;
        if !m.get("subtype").is_some_and(Value::is_string) {
            return Err(err("a control request needs a subtype"));
        }
        self.control(m)
    }

    /// Asks Claude Code to put the conversation's files back as they were
    /// just before message user_message_id; the answer is a ControlReply
    /// for the returned id.
    fn rewind_files(&self, user_message_id: &str, dry_run: bool) -> Result<String> {
        self.control_of(json!({"subtype": "rewind_files", "user_message_id": user_message_id, "dry_run": dry_run}))
    }

    /// Ends the session and everything it started. Closing stdin lets
    /// Claude Code finish writing the transcript; it is ended if it has
    /// not gone after grace. A clean exit still leaves its background
    /// shells and the sessions it spawned running, so they are ended after
    /// it.
    pub fn stop(&self, grace: Duration) -> Result<()> {
        let started = self.started();
        {
            let mut q = lock(&self.sh.w);
            q.closed = true;
            self.sh.wsig.notify_all();
        }
        if !self.wait_for(grace) {
            // SAFETY: kill(2) takes plain integers.
            unsafe { libc::kill(-self.pid, libc::SIGTERM) };
            if !self.wait_for(Duration::from_secs(2)) {
                // SAFETY: as above.
                unsafe { libc::kill(-self.pid, libc::SIGKILL) };
            }
        }
        let e = self.err();
        end_started(self.pid, &started);
        match e {
            Some(e) if !e.exited => Err(Box::new(e)),
            _ => Ok(()), // stopped on purpose
        }
    }

    /// Every process under Claude Code, read before it ends: once it has
    /// gone, a shell it left in the background belongs to init, and one in
    /// a group of its own isn't in Claude Code's.
    fn started(&self) -> HashMap<i32, SystemTime> {
        if self.done() {
            return HashMap::new();
        }
        let t = proc::snapshot();
        let mut out = HashMap::new();
        let mut stack = vec![self.pid];
        while let Some(p) = stack.pop() {
            for &c in t.children.get(&p).map_or(&[][..], Vec::as_slice) {
                if rush_host(&proc::args(c)) {
                    continue; // a session of its own, spawned: stopped as one, not killed
                }
                if let Some(pr) = t.procs.get(&c) {
                    out.insert(c, pr.start);
                }
                stack.push(c);
            }
        }
        out
    }
}

/// Whether args are a rush session's host, "rush host run <id>".
fn rush_host(args: &[String]) -> bool {
    args.windows(3).any(|w| w[0] == "host" && w[1] == "run")
}

/// Ends Claude Code's process group and each process it started that
/// still runs, the same process and not a new one on its pid: SIGTERM,
/// then SIGKILL for any still there a second later.
fn end_started(pgid: i32, started: &HashMap<i32, SystemTime>) {
    let alive = || {
        // SAFETY: kill(2) with signal 0 only asks.
        let group = unsafe { libc::kill(-pgid, 0) } == 0;
        let pids: Vec<i32> = started
            .iter()
            .filter(|&(&pid, &at)| proc::started(pid) == Some(at) && !proc::zombie(pid))
            .map(|(&pid, _)| pid)
            .collect();
        (group, pids)
    };
    let signal = |sig: i32| {
        let (group, pids) = alive();
        if group {
            // SAFETY: as above.
            unsafe { libc::kill(-pgid, sig) };
        }
        for &pid in &pids {
            let _ = proc::kill(pid, sig);
        }
        group || !pids.is_empty()
    };
    if !signal(libc::SIGTERM) {
        return;
    }
    // As Go does: nothing says when another's process has gone, so look
    // every 50ms for at most a second.
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        let (group, pids) = alive();
        if !group && pids.is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    signal(libc::SIGKILL);
}

/// A message's content blocks. Where the text names an image by its
/// marker, [Image #2] say, the text up to and including the marker comes
/// first and that image right after it, so Claude sees each picture where
/// it was put. Images the text doesn't name follow at the end; a text that
/// names none keeps the images before the text.
pub fn content(text: &str, images: &[Image]) -> Vec<Value> {
    let img = |im: &Image| {
        json!({"type": "image", "source": {
            "type": "base64", "media_type": im.media_type, "data": STANDARD.encode(&im.data)}})
    };
    let txt = |t: &str| json!({"type": "text", "text": t});
    let mut blocks = Vec::new();
    let mut used = vec![false; images.len()];
    let mut last = 0;
    for (start, end, n) in markers(text) {
        let _ = start;
        if n < 1 || n > images.len() || used[n - 1] {
            continue;
        }
        let chunk = &text[last..end];
        if !chunk.trim().is_empty() {
            blocks.push(txt(chunk));
        }
        blocks.push(img(&images[n - 1]));
        used[n - 1] = true;
        last = end;
    }
    if last == 0 {
        blocks.extend(images.iter().map(img));
        if !text.trim().is_empty() {
            blocks.push(txt(text));
        }
        return blocks;
    }
    let rest = &text[last..];
    if !rest.trim().is_empty() {
        blocks.push(txt(rest));
    }
    for (i, im) in images.iter().enumerate() {
        if !used[i] {
            blocks.push(img(im));
        }
    }
    blocks
}

/// Each `[Image #n]` in text: where it starts and ends, and n (0 when n
/// doesn't fit a number).
fn markers(text: &str) -> Vec<(usize, usize, usize)> {
    const OPEN: &str = "[Image #";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(OPEN) {
        let start = from + i;
        let digits = &text[start + OPEN.len()..];
        let len = digits.bytes().take_while(u8::is_ascii_digit).count();
        if len > 0 && digits[len..].starts_with(']') {
            let end = start + OPEN.len() + len + 1;
            out.push((start, end, digits[..len].parse().unwrap_or(0)));
            from = end;
        } else {
            from = start + 1;
        }
    }
    out
}

/// What fills the context window, by category, as Claude Code's /context
/// counts it: a reply to ask_context_usage.
pub fn parse_context_usage(reply: &ControlReply) -> Result<usage::Context> {
    let body = reply.body.as_ref().map_or("null", |b| b.get());
    Ok(serde_json::from_str(body)?)
}

/// Turns on Claude Code's file checkpoints for a headless session: before
/// a turn edits a file, its old contents are kept, so the files can later
/// be put back as they were before any of your messages.
pub const CHECKPOINT_ENV: &str = "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING=1";

/// Puts the files a conversation changed back as they were just before
/// the message user_message_id, the uuid of the transcript line that sent
/// it. dry_run only says which files that would change. It runs a Claude
/// Code of its own on the saved conversation (o.resume), which sends
/// nothing to the model and ends when it has answered.
pub fn rewind_files(o: &Options, user_message_id: &str, dry_run: bool) -> Result<FileRewind> {
    if o.resume.is_empty() {
        return Err(err("no conversation to rewind"));
    }
    let mut o = o.clone();
    o.env.push(CHECKPOINT_ENV.into());
    o.tap = None;
    o.skip = None;
    let (s, reader) = start(&o)?;
    let (tx, rx) = mpsc::channel();
    let ended = tx.clone();
    reader.spawn(
        s.clone(),
        move |ev| {
            if let Event::ControlReply(r) = ev {
                let _ = tx.send(Some(r));
            }
        },
        move |_| {
            let _ = ended.send(None);
        },
    );
    let out = (|| {
        let id = s.rewind_files(user_message_id, dry_run)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Some(r)) if r.id == id => {
                    let mut out = FileRewind::default();
                    if let Some(b) = &r.body {
                        out = read_rewind(b.get());
                    }
                    if !r.error.is_empty() {
                        return Err(err(r.error));
                    }
                    return Ok(out);
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let why = s.err().map_or("<nil>".to_string(), |e| e.text);
                    return Err(err(format!("Claude Code stopped before answering: {why}")));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(err("Claude Code took too long to answer"));
                }
            }
        }
    })();
    let _ = s.stop(Duration::from_secs(5));
    out
}

/// FileRewind as Claude Code's reply writes it.
fn read_rewind(body: &str) -> FileRewind {
    use crate::json::nd;
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct R {
        #[serde(rename = "canRewind", deserialize_with = "nd")]
        can_rewind: bool,
        #[serde(deserialize_with = "nd")]
        error: String,
        #[serde(rename = "filesChanged", deserialize_with = "nd")]
        files: Vec<String>,
        #[serde(deserialize_with = "nd")]
        insertions: i64,
        #[serde(deserialize_with = "nd")]
        deletions: i64,
    }
    let r: R = serde_json::from_str(body).unwrap_or_default();
    FileRewind {
        can_rewind: r.can_rewind,
        error: r.error,
        files: r.files,
        insertions: r.insertions,
        deletions: r.deletions,
    }
}

/// Asks a throwaway copy of a saved conversation (o.resume) what it
/// learned since your message from: what was tried, what worked, what
/// didn't. It's for carrying back to an earlier point that forgets those
/// turns. Nothing is saved, and the conversation itself is left as it is.
pub fn recap(o: &Options, from: &str) -> Result<String> {
    if o.resume.is_empty() {
        return Err(err("no conversation to recap"));
    }
    let from: String = if from.chars().count() > 300 {
        from.chars().take(300).collect::<String>() + "…"
    } else {
        from.to_string()
    };
    let ask = format!(
        "I'm about to rewind this conversation to just before my message:\n\n> {}\n\n\
         Everything from that message on will be forgotten, and I'll try again from there. \
         Write the note I should bring back: what was tried, what worked, what didn't and why, \
         and any facts found (files, causes, commands) that would save time next time. \
         Say what code was changed. Under 200 words, plain bullets, no preamble. Don't use any tools.",
        from.replace('\n', "\n> ")
    );
    let mut args: Vec<String> = [
        "-p",
        "--resume",
        &o.resume,
        "--fork-session",
        "--no-session-persistence",
        "--max-turns",
        "1",
        "--output-format",
        "json",
    ]
    .map(String::from)
    .to_vec();
    if !o.model.is_empty() {
        args.extend(["--model".into(), o.model.clone()]);
    }
    args.push(ask);
    let mut cmd = Command::new(o.program());
    cmd.args(&args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !o.dir.is_empty() {
        cmd.current_dir(&o.dir);
    }
    for e in o.account.env().iter().chain(&o.env) {
        if let Some((k, v)) = e.split_once('=') {
            cmd.env(k, v);
        }
    }
    let (status, out, stderr) = match cmd.output() {
        Ok(o) => (Some(o.status), o.stdout, o.stderr),
        Err(e) => return Err(err(format!("couldn't recap: {e}"))),
    };
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct R {
        result: Option<String>,
        is_error: Option<bool>,
    }
    let parsed: Option<R> = serde_json::from_slice(&out).ok();
    let result = parsed
        .as_ref()
        .and_then(|r| r.result.clone())
        .unwrap_or_default();
    let is_error = parsed.as_ref().and_then(|r| r.is_error).unwrap_or(false);
    if parsed.is_none() || is_error || result.trim().is_empty() {
        let mut e = match status {
            Some(st) if !st.success() => exit_text(st),
            _ => first_non_empty(result.trim(), "no answer").to_string(),
        };
        let tail = Tail::default();
        tail.write(&stderr);
        let msg = tail.text();
        if !msg.is_empty() {
            e = format!("{e}: {msg}");
        }
        return Err(err(format!("couldn't recap: {e}")));
    }
    Ok(result.trim().to_string())
}

fn first_non_empty<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
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

#[cfg(test)]
mod tests;
