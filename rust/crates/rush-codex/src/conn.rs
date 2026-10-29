//! A Codex thread run through `codex app-server`: what it says turned
//! into rush's events, pushed into the caller's sink from the client's
//! reader thread, and what rush sends it. Port of
//! internal/adapters/codex/conn.go:1-507 (Conn, Start, begin, readQuota,
//! Send, Interrupt, SetModel, SetEffort, SetMode, Answer, AnswerQuestion,
//! Close, carried, envNames), handle.go:1-388 (handle, notification,
//! request, the items), kids.go:1-181 (spawned threads as tasks) and
//! shells.go:1-84 (commands gone to the background).
//!
//! Go's Conn queues events for a channel a goroutine pumps; here the sink
//! never blocks, so the reader pushes each in order as it reads it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rush_core::adapter::{
    self, Answerer, Conn as ConnTrait, Input, Sink, StartOptions, TaskStopper,
};
use rush_core::agent::PreviewLine;
use rush_core::event::{
    Approval, ApprovalCancelled, ApprovalOption, Ask, Background, BackgroundTask, Billing,
    CallUpdated, Choice, Compacted, Context, Delta, Event, Init, Limited, Message, MessageStart,
    OptionKind, Other, PartKind, PartStart, Plan, Question, Status, TaskDone, TaskKind,
    TaskProgress, TaskStarted, TurnEnd,
};
use rush_core::tool::{Call, Kind, TodoItem};
use rush_core::usage::TokenUsage;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::items::{
    ThreadItem, call_of, output_of, path_base, reasoning_text, script, text_part, todo_status,
    user_message,
};
use crate::json::{arr, get, int, keep, s, st};
use crate::models::{CachedModel, model_id, read_models};
use crate::quota::{Snapshot, account_billing, quota_from, read_quota};
use crate::rollout::{call_part, result};
use crate::rpc::{Client, Handler, Incoming, Message as RpcMessage, RpcError, id_string, spawn};

// Server requests rush answers.
pub(crate) const REQ_COMMAND: &str = "item/commandExecution/requestApproval";
pub(crate) const REQ_FILE_CHANGE: &str = "item/fileChange/requestApproval";
pub(crate) const REQ_PERMISSIONS: &str = "item/permissions/requestApproval";
pub(crate) const REQ_USER_INPUT: &str = "item/tool/requestUserInput";
pub(crate) const REQ_ELICITATION: &str = "mcpServer/elicitation/request";

/// The MCP server rush serves its own tools as (Go's agtools.Server).
const TOOLS_SERVER: &str = "rush";

/// One of Codex's approval presets, as a thread starts with it and as a
/// turn overrides it.
struct Mode {
    id: &'static str,
    approval: &'static str,
    /// thread/start's SandboxMode
    sandbox: &'static str,
    /// turn/start's SandboxPolicy
    turn: fn() -> Value,
}

/// The presets Codex's own /approvals offers.
const MODES: [Mode; 3] = [
    Mode {
        id: "read-only",
        approval: "on-request",
        sandbox: "read-only",
        turn: || json!({"type": "readOnly", "networkAccess": false}),
    },
    Mode {
        id: "auto",
        approval: "on-request",
        sandbox: "workspace-write",
        turn: || {
            json!({"type": "workspaceWrite", "writableRoots": [], "networkAccess": false,
                "excludeTmpdirEnvVar": false, "excludeSlashTmp": false})
        },
    },
    Mode {
        id: "full-access",
        approval: "never",
        sandbox: "danger-full-access",
        turn: || json!({"type": "dangerFullAccess"}),
    },
];

fn mode(id: &str) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.id == id)
}

/// The preset a thread's approval policy and sandbox type are, or ""
/// when they're none of them.
pub(crate) fn mode_of(approval: &str, sandbox: &str) -> &'static str {
    MODES
        .iter()
        .find(|m| m.approval == approval && s(&(m.turn)(), "type") == sandbox)
        .map_or("", |m| m.id)
}

/// How a command or file change can be answered.
fn decisions() -> Vec<ApprovalOption> {
    [
        ("accept", "Yes", OptionKind::AllowOnce),
        (
            "acceptForSession",
            "Yes, for this session",
            OptionKind::AllowAlways,
        ),
        ("decline", "No", OptionKind::RejectOnce),
        ("cancel", "No, and stop", OptionKind::RejectAlways),
    ]
    .into_iter()
    .map(option)
    .collect()
}

/// How a request for more permissions can be answered.
fn grants() -> Vec<ApprovalOption> {
    [
        ("turn", "Yes, for this turn", OptionKind::AllowOnce),
        ("session", "Yes, for this session", OptionKind::AllowAlways),
        ("decline", "No", OptionKind::RejectOnce),
    ]
    .into_iter()
    .map(option)
    .collect()
}

fn option((id, label, kind): (&str, &str, OptionKind)) -> ApprovalOption {
    ApprovalOption {
        id: id.into(),
        label: label.into(),
        kind,
    }
}

/// A server request waiting on the user.
struct Pending {
    id: Box<RawValue>,
    method: String,
    /// The permissions asked for.
    perms: serde_json::Map<String, Value>,
    /// A question's text → its id.
    qs: HashMap<String, String>,
}

/// A spawned thread: the call that spawned it, its task, and the turn it's
/// running ("" when it's idle).
#[derive(Default)]
struct Kid {
    call: String,
    label: String,
    turn: String,
    busy: bool,
    tokens: i64,
    tools: HashSet<String>,
}

/// A command whose item hasn't completed: its terminal, and whether it
/// runs beside the turn yet.
#[derive(Default)]
struct Shell {
    process: String,
    bg: bool,
    stopping: bool,
}

/// What's applied on the next turn/start.
#[derive(Clone, Default, PartialEq, Eq)]
struct Next {
    model: String,
    effort: String,
    mode: String,
}

/// What a thread's connection keeps between messages.
#[derive(Default)]
struct State {
    thread: String,
    /// The running turn's id.
    turn: String,
    /// The last turn to end, so a late turn/start answer doesn't revive it.
    ended: String,
    model: String,
    account: String,
    version: String,
    /// What its account offers, to name a model by.
    models: Vec<CachedModel>,
    next: Next,
    asks: HashMap<String, Pending>,
    /// Tool calls by item id, for approvals.
    calls: HashMap<String, Call>,
    /// Items streaming as a message.
    open: HashSet<String>,
    /// This turn's.
    tokens: TokenUsage,
    /// The threads it spawned, by id.
    kids: BTreeMap<String, Kid>,
    /// Commands not yet completed, by item id.
    shells: BTreeMap<String, Shell>,
}

/// What the reader thread and the Conn share.
struct Inner {
    sink: Arc<dyn Sink>,
    st: Mutex<State>,
    /// Ended has been said.
    said_end: AtomicBool,
    /// Starting failed: its end isn't the caller's to hear.
    quiet: AtomicBool,
}

impl Inner {
    fn st(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, evs: Vec<Event>) {
        for ev in evs {
            self.sink.event(ev);
        }
    }

    /// What the app-server says, turned into rush's events, and the
    /// requests the user has to answer kept.
    fn handle(&self, rpc: &Client, inc: Incoming) {
        match inc {
            Incoming::Message(m) if m.id.is_some() => self.request(rpc, m),
            Incoming::Message(m) => {
                let evs = self.st().notification(&m.method, m.params());
                self.emit(evs);
            }
            Incoming::Closed => {
                if !self.quiet.load(Ordering::SeqCst) && !self.said_end.swap(true, Ordering::SeqCst)
                {
                    self.sink.ended(None);
                }
            }
        }
    }

    /// Keeps a server request for the user to answer, or answers it at
    /// once when rush has nothing to ask.
    fn request(&self, rpc: &Client, m: RpcMessage) {
        let raw_id = m.id.expect("a request");
        let id = id_string(&raw_id);
        let p: Value = serde_json::from_str(m.params.get()).unwrap_or(Value::Null);
        let item = st(&p, "itemId");
        let reason = st(&p, "reason");
        let mut state = self.st();
        let known = state.calls.get(&item).cloned();
        let mut a = Pending {
            id: raw_id,
            method: m.method.clone(),
            perms: serde_json::Map::new(),
            qs: HashMap::new(),
        };
        let ev = match m.method.as_str() {
            REQ_COMMAND => {
                let mut call = known.unwrap_or_else(|| Call {
                    id: item.clone(),
                    name: "shell".into(),
                    kind: Kind::Shell,
                    ..Call::default()
                });
                let cmd = s(&p, "command");
                if !cmd.is_empty() {
                    (call.input.command, call.input.cwd) = (script(cmd), st_of(&p, "cwd"));
                }
                approval(id.clone(), call, reason, String::new(), decisions())
            }
            REQ_FILE_CHANGE => {
                let call = known.unwrap_or_else(|| Call {
                    id: item.clone(),
                    name: "apply_patch".into(),
                    kind: Kind::Edit,
                    ..Call::default()
                });
                approval(
                    id.clone(),
                    call,
                    reason,
                    st_of(&p, "grantRoot"),
                    decisions(),
                )
            }
            REQ_PERMISSIONS => {
                if let Some(o) = p.get("permissions").and_then(Value::as_object) {
                    a.perms = o.clone();
                }
                let call = Call {
                    id: item.clone(),
                    name: "permissions".into(),
                    title: reason.clone(),
                    raw: Some(keep(&m.params)),
                    ..Call::default()
                };
                let path = first_path(p.get("permissions").and_then(|x| x.get("fileSystem")));
                approval(id.clone(), call, reason, path, grants())
            }
            REQ_USER_INPUT => {
                let mut q = Question {
                    id: id.clone(),
                    call_id: item.clone(),
                    ..Question::default()
                };
                for i in arr(&p, "questions") {
                    let ask = Ask {
                        id: st(i, "id"),
                        header: st(i, "header"),
                        text: st(i, "question"),
                        options: arr(i, "options")
                            .iter()
                            .map(|o| Choice {
                                label: st(o, "label"),
                                description: st(o, "description"),
                                ..Choice::default()
                            })
                            .collect(),
                        ..Ask::default()
                    };
                    a.qs.insert(ask.text.clone(), ask.id.clone());
                    q.asks.push(ask);
                }
                Event::Question(q)
            }
            REQ_ELICITATION => {
                drop(state);
                // rush has no form to fill in yet.
                let _ = rpc.reply(
                    &a.id,
                    json!({"action": "decline", "content": null, "_meta": null}),
                );
                self.emit(vec![other(&m.method, &m.params)]);
                return;
            }
            _ => {
                drop(state);
                let _ = rpc.refuse(&a.id, &format!("rush does not handle {}", m.method));
                self.emit(vec![other(&m.method, &m.params)]);
                return;
            }
        };
        state.asks.insert(id, a);
        drop(state);
        self.emit(vec![ev]);
    }
}

fn st_of(v: &Value, k: &str) -> String {
    st(v, k)
}

fn approval(
    id: String,
    call: Call,
    reason: String,
    path: String,
    options: Vec<ApprovalOption>,
) -> Event {
    Event::Approval(Box::new(Approval {
        id,
        call,
        reason,
        path,
        options,
    }))
}

/// What a notification rush doesn't read is kept as, whole.
fn other(method: &str, params: &RawValue) -> Event {
    Event::Other(Other {
        adapter: "codex".into(),
        kind: method.to_string(),
        raw: (params.get() != "null").then(|| keep(params)),
    })
}

/// The first path a file-system permission asks for.
fn first_path(fs: Option<&Value>) -> String {
    let Some(fs) = fs else {
        return String::new();
    };
    for k in ["write", "read"] {
        for p in arr(fs, k) {
            let p = p.as_str().unwrap_or("").trim();
            if !p.is_empty() {
                return p.to_string();
            }
        }
    }
    String::new()
}

/// Codex's TokenUsageBreakdown as rush counts it. OpenAI's input
/// includes the cached input and its output includes the reasoning, so
/// input is what wasn't cached and output is taken as it is.
fn breakdown(v: &Value) -> TokenUsage {
    let i = |k: &str| int(v, k).unwrap_or(0);
    TokenUsage {
        input: (i("inputTokens") - i("cachedInputTokens")).max(0),
        output: i("outputTokens"),
        cache_read: i("cachedInputTokens"),
        reasoning: i("reasoningOutputTokens"),
        ..TokenUsage::default()
    }
}

fn turn_reason(status: &str) -> String {
    match status {
        "completed" => "done".into(),
        "failed" => "error".into(),
        s => s.into(), // interrupted
    }
}

/// A turn's status as a task's.
fn task_status(s: &str) -> String {
    match s {
        "interrupted" => "stopped",
        "failed" => "failed",
        _ => "completed",
    }
    .into()
}

/// A notification's item, kept as written.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ItemOnly<'a> {
    #[serde(borrow)]
    item: Option<&'a RawValue>,
}

impl State {
    /// What one notification means as events.
    fn notification(&mut self, method: &str, params: &str) -> Vec<Event> {
        let raw: Box<RawValue> = match RawValue::from_string(params.to_string()) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let p: Value = serde_json::from_str(params).unwrap_or(Value::Bool(false));
        if !p.is_object() && !p.is_null() {
            return vec![other(method, &raw)];
        }
        let item = serde_json::from_str::<ItemOnly>(params)
            .ok()
            .and_then(|i| i.item)
            .filter(|r| r.get() != "null");
        let thread_id = s(&p, "threadId");
        let turn = get(&p, "turn");
        if !thread_id.is_empty() && !self.thread.is_empty() && thread_id != self.thread {
            // A subagent's thread: its turns are its task's.
            let mut out = Vec::new();
            if let Some(t) = turn {
                out = self.kid_turn(method, thread_id, s(t, "id"), s(t, "status"));
            }
            if method == "thread/tokenUsage/updated"
                && let Some(u) = get(&p, "tokenUsage")
            {
                let last = breakdown(u.get("last").unwrap_or(&Value::Null));
                out.extend(self.kid_tokens(thread_id, last));
            }
            if method == "item/started"
                && let Some(i) = item
            {
                out.extend(self.kid_tool(thread_id, i));
            }
            out.push(other(method, &raw));
            return out;
        }
        match method {
            "thread/started" => {
                if let Some(t) = get(&p, "thread")
                    && (self.thread.is_empty() || s(t, "id") == self.thread)
                {
                    return Vec::new(); // Start sent Init from thread/start's answer
                }
            }
            "turn/started" => {
                if let Some(t) = turn {
                    (self.turn, self.tokens) = (st(t, "id"), TokenUsage::default());
                }
                return vec![Event::Status(Status {
                    busy: true,
                    text: String::new(),
                })];
            }
            "turn/completed" => {
                if let Some(t) = turn {
                    let mut end = TurnEnd {
                        reason: turn_reason(s(t, "status")),
                        tokens: self.tokens,
                        turns: 1,
                        ..TurnEnd::default()
                    };
                    self.turn.clear();
                    self.ended = st(t, "id");
                    self.open.clear();
                    if let Some(d) = int(t, "durationMs") {
                        end.duration = Duration::from_millis(d.max(0) as u64);
                    }
                    let mut out = self.detach(); // what still runs outlives the turn
                    if let Some(e) = get(t, "error") {
                        end.err = st(e, "message");
                        if s(e, "codexErrorInfo") == "usageLimitExceeded" {
                            out.push(Event::Limited(Limited::default()));
                        }
                    }
                    out.push(Event::TurnEnd(end));
                    return out;
                }
            }
            "item/started" => return item.map_or_else(Vec::new, |i| self.item_started(i)),
            "item/completed" => return item.map_or_else(Vec::new, |i| self.item_completed(i)),
            "item/agentMessage/delta" => {
                return self.delta(s(&p, "itemId"), PartKind::Text, st(&p, "delta"));
            }
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                return self.delta(s(&p, "itemId"), PartKind::Thinking, st(&p, "delta"));
            }
            "turn/plan/updated" => {
                let todos = arr(&p, "plan")
                    .iter()
                    .map(|x| TodoItem {
                        label: st(x, "step"),
                        status: todo_status(s(x, "status")),
                        ..TodoItem::default()
                    })
                    .collect();
                return vec![Event::Plan(Plan { todos })];
            }
            "thread/tokenUsage/updated" => {
                if let Some(u) = get(&p, "tokenUsage") {
                    let last = u.get("last").unwrap_or(&Value::Null);
                    self.tokens.add(breakdown(last));
                    return match int(u, "modelContextWindow") {
                        Some(w) => vec![Event::Context(Context {
                            tokens: int(last, "totalTokens").unwrap_or(0),
                            window: w,
                        })],
                        None => Vec::new(),
                    };
                }
            }
            "account/rateLimits/updated" => {
                if let Some(rl) = get(&p, "rateLimits") {
                    let snap = Snapshot::from_value(rl);
                    let mut q = quota_from(&snap);
                    q.account = self.account.clone();
                    let mut out = vec![Event::Quota(q)];
                    if let Some(b) = snap.billing() {
                        out.push(Event::Billing(Billing { billing: b }));
                    }
                    return out;
                }
            }
            "serverRequest/resolved" => {
                let id = p
                    .get("requestId")
                    .and_then(|r| RawValue::from_string(r.to_string()).ok())
                    .map(|r| id_string(&r))
                    .unwrap_or_default();
                if self.asks.remove(&id).is_some() {
                    return vec![Event::ApprovalCancelled(ApprovalCancelled { id })];
                }
                return Vec::new();
            }
            "error" if p.get("willRetry").and_then(Value::as_bool) == Some(true) => {
                return vec![Event::Status(Status {
                    busy: true,
                    text: "retrying".into(),
                })];
            }
            _ => {}
        }
        vec![other(method, &raw)]
    }

    /// A piece of a streaming item, opening its message first if it isn't
    /// open yet.
    fn delta(&mut self, item: &str, kind: PartKind, text: String) -> Vec<Event> {
        let mut out = self.open_item(item, kind);
        out.push(Event::Delta(Delta {
            index: 0,
            kind,
            text,
        }));
        out
    }

    fn open_item(&mut self, item: &str, kind: PartKind) -> Vec<Event> {
        if !self.open.insert(item.to_string()) {
            return Vec::new();
        }
        vec![
            Event::MessageStart(MessageStart {
                id: item.to_string(),
                model: self.model.clone(),
            }),
            Event::PartStart(PartStart { index: 0, kind }),
        ]
    }

    fn item_started(&mut self, raw: &RawValue) -> Vec<Event> {
        let Some(it) = ThreadItem::read(raw.get()) else {
            return Vec::new();
        };
        match it.typ.as_str() {
            "contextCompaction" => {
                return vec![Event::Status(Status {
                    busy: true,
                    text: "compacting".into(),
                })];
            }
            "agentMessage" => {
                let mut out = self.detach();
                out.extend(self.open_item(&it.id, PartKind::Text));
                return out;
            }
            "reasoning" => {
                let mut out = self.detach();
                out.extend(self.open_item(&it.id, PartKind::Thinking));
                return out;
            }
            _ => {}
        }
        let mut out = Vec::new();
        if let Some(call) = call_of(&it, Some(keep(raw))) {
            self.calls.insert(it.id.clone(), call.clone());
            self.started(&it);
            out.push(self.call_message(call));
        }
        if it.typ == "subAgentActivity" && it.kind == "started" {
            out.extend(self.adopt(&it.agent_thread_id, &it.id, &path_base(&it.agent_path)));
        }
        out
    }

    fn call_message(&self, call: Call) -> Event {
        Event::Message(Message {
            role: "assistant".into(),
            id: call.id.clone(),
            model: self.model.clone(),
            parts: vec![call_part(call)],
            ..Message::default()
        })
    }

    fn item_completed(&mut self, raw: &RawValue) -> Vec<Event> {
        let Some(it) = ThreadItem::read(raw.get()) else {
            return Vec::new();
        };
        self.open.remove(&it.id);
        let assistant = |kind, text| {
            Event::Message(Message {
                role: "assistant".into(),
                id: it.id.clone(),
                model: self.model.clone(),
                parts: vec![text_part(kind, text)],
                ..Message::default()
            })
        };
        match it.typ.as_str() {
            "agentMessage" => return vec![assistant(PartKind::Text, it.text.clone())],
            "reasoning" => {
                let t = reasoning_text(&it);
                if t.is_empty() {
                    return Vec::new();
                }
                return vec![assistant(PartKind::Thinking, t)];
            }
            "userMessage" => return vec![Event::Message(user_message(&it))],
            "contextCompaction" => return vec![Event::Compacted(Compacted::default())],
            _ => {}
        }
        let Some(call) = call_of(&it, Some(keep(raw))) else {
            return vec![Event::Other(Other {
                adapter: "codex".into(),
                kind: format!("item/{}", it.typ),
                raw: Some(keep(raw)),
            })];
        };
        let mut out = Vec::new();
        let started = self.calls.remove(&it.id).is_some();
        if !started || it.typ == "webSearch" {
            // Web searches say what they searched only at the end.
            out.push(self.call_message(call));
        }
        out.push(result(output_of(&it, Some(keep(raw)))));
        out.extend(self.finished(&it));
        out
    }

    // ---- Spawned threads (kids.go) ----
    //
    // A thread's spawn_agent children run in its app-server, which tells
    // rush of their turns too: each is a task beside the turn, as Claude
    // Code's background subagents are, that rush can stop. Codex takes no
    // input for one but through its parent ("direct app-server input is
    // not allowed").

    /// Takes thread, spawned by call, on as a task.
    fn adopt(&mut self, thread: &str, call: &str, label: &str) -> Vec<Event> {
        if thread.is_empty() {
            return Vec::new();
        }
        let k = self.kids.entry(thread.to_string()).or_default();
        (k.call, k.label, k.busy, k.tokens) = (call.into(), label.into(), true, 0);
        k.tools.clear();
        vec![
            task_started(thread, call, TaskKind::Subagent, label),
            Event::Background(self.background()),
        ]
    }

    /// What a turn of another thread starting or ending means: a task
    /// starting again, or ending. Threads it didn't spawn mean nothing.
    fn kid_turn(&mut self, method: &str, thread: &str, turn: &str, status: &str) -> Vec<Event> {
        let k = self.kids.entry(thread.to_string()).or_default();
        let was = k.busy;
        match method {
            "turn/started" => {
                (k.turn, k.busy) = (turn.into(), true);
                if !was {
                    k.tokens = 0;
                    k.tools.clear();
                }
            }
            "turn/completed" => (k.turn, k.busy) = (String::new(), false),
            _ => return Vec::new(),
        }
        let (call, label, busy) = (k.call.clone(), k.label.clone(), k.busy);
        if call.is_empty() || was == busy {
            return Vec::new();
        }
        if busy {
            return vec![
                task_started(thread, &call, TaskKind::Subagent, &label),
                Event::Background(self.background()),
            ];
        }
        vec![
            Event::TaskDone(TaskDone {
                id: thread.into(),
                call_id: call,
                status: task_status(status),
                ..TaskDone::default()
            }),
            Event::Background(self.background()),
        ]
    }

    /// Child-thread tokens as the same progress figures Claude reports for
    /// its native subagents.
    fn kid_tokens(&mut self, thread: &str, u: TokenUsage) -> Vec<Event> {
        let k = self.kids.entry(thread.to_string()).or_default();
        if k.call.is_empty() || !k.busy {
            return Vec::new();
        }
        k.tokens += u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h;
        vec![Event::TaskProgress(TaskProgress {
            id: thread.into(),
            tokens: k.tokens,
            tool_uses: k.tools.len() as i64,
            ..TaskProgress::default()
        })]
    }

    fn kid_tool(&mut self, thread: &str, raw: &RawValue) -> Vec<Event> {
        let Some(it) = ThreadItem::read(raw.get()) else {
            return Vec::new();
        };
        let Some(call) = call_of(&it, None) else {
            return Vec::new();
        };
        let k = self.kids.entry(thread.to_string()).or_default();
        if k.call.is_empty() || !k.busy {
            return Vec::new();
        }
        k.tools.insert(it.id.clone());
        vec![Event::TaskProgress(TaskProgress {
            id: thread.into(),
            last_tool: call.name,
            tokens: k.tokens,
            tool_uses: k.tools.len() as i64,
            ..TaskProgress::default()
        })]
    }

    /// The spawned threads and background shells running now.
    fn background(&self) -> Background {
        let mut tasks: Vec<BackgroundTask> = self
            .kids
            .iter()
            .filter(|(_, k)| k.busy && !k.call.is_empty())
            .map(|(id, k)| BackgroundTask {
                id: id.clone(),
                kind: TaskKind::Subagent,
                label: k.label.clone(),
                ..BackgroundTask::default()
            })
            .collect();
        for (id, sh) in &self.shells {
            if sh.bg {
                tasks.push(BackgroundTask {
                    id: id.clone(),
                    kind: TaskKind::Shell,
                    label: self
                        .calls
                        .get(id)
                        .map(|c| c.input.command.clone())
                        .unwrap_or_default(),
                    ..BackgroundTask::default()
                });
            }
        }
        Background { tasks }
    }

    /// The spawned thread id names (its thread, or the call that spawned
    /// it), and the turn it's running.
    fn child(&self, id: &str) -> Option<(String, String)> {
        self.kids
            .iter()
            .find(|(t, k)| !k.call.is_empty() && (t.as_str() == id || k.call == id))
            .map(|(t, k)| (t.clone(), k.turn.clone()))
    }

    // ---- Commands gone to the background (shells.go) ----
    //
    // Codex's commands run in terminals that can outlive their call: one
    // still running when the model reads on, or when its turn ends, has
    // gone to the background, where it stays until its item completes, as
    // Claude Code's backgrounded Bash commands do. Rush can stop one.

    /// Keeps a command item until it completes.
    fn started(&mut self, it: &ThreadItem) {
        if it.typ != "commandExecution" {
            return;
        }
        self.shells.insert(
            it.id.clone(),
            Shell {
                process: it.process_id.clone(),
                ..Shell::default()
            },
        );
    }

    /// The commands still running when the model reads on, or the turn
    /// ends: their calls returned, and they run beside the turn now.
    fn detach(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        for (id, sh) in self.shells.iter_mut() {
            if sh.bg {
                continue;
            }
            let Some(call) = self.calls.get_mut(id) else {
                continue;
            };
            sh.bg = true;
            call.input.background = true;
            out.push(Event::CallUpdated(Box::new(CallUpdated {
                call: call.clone(),
            })));
            out.push(task_started(id, id, TaskKind::Shell, &call.input.command));
        }
        if out.is_empty() {
            return out;
        }
        out.push(Event::Background(self.background()));
        out
    }

    /// What a command item completing means: a background shell's task is
    /// done.
    fn finished(&mut self, it: &ThreadItem) -> Vec<Event> {
        let Some(sh) = self.shells.remove(&it.id) else {
            return Vec::new();
        };
        if !sh.bg {
            return Vec::new();
        }
        let status = if sh.stopping {
            "stopped"
        } else if it.status == "failed"
            || it.status == "declined"
            || it.exit_code.is_some_and(|c| c != 0)
        {
            "failed"
        } else {
            "completed"
        };
        vec![
            Event::TaskDone(TaskDone {
                id: it.id.clone(),
                call_id: it.id.clone(),
                status: status.into(),
                ..TaskDone::default()
            }),
            Event::Background(self.background()),
        ]
    }

    /// The background shell id names (its item, or its terminal), marked as
    /// being stopped: (thread, process).
    fn shell_of(&mut self, id: &str) -> Option<(String, String)> {
        for (item, sh) in self.shells.iter_mut() {
            if sh.bg && !sh.process.is_empty() && (item == id || sh.process == id) {
                sh.stopping = true;
                return Some((self.thread.clone(), sh.process.clone()));
            }
        }
        None
    }
}

fn task_started(id: &str, call: &str, kind: TaskKind, label: &str) -> Event {
    Event::TaskStarted(TaskStarted {
        id: id.into(),
        call_id: call.into(),
        kind,
        label: label.into(),
        background: true,
        ..TaskStarted::default()
    })
}

/// A Codex thread run through `codex app-server`.
pub struct Conn {
    rpc: Arc<Client>,
    inner: Arc<Inner>,
    closed: AtomicBool,
}

fn err(msg: impl Into<String>) -> adapter::Error {
    msg.into().into()
}

impl Conn {
    /// A connection to the app-server at the other end of r and w, not yet
    /// shaken hands with.
    #[cfg(test)]
    pub(crate) fn connect(
        r: impl std::io::Read + Send + 'static,
        w: impl std::io::Write + Send + 'static,
        sink: Arc<dyn Sink>,
    ) -> Conn {
        let inner = new_inner(sink);
        let rpc = Client::new(r, w, Some(handler(&inner)));
        Conn {
            rpc,
            inner,
            closed: AtomicBool::new(false),
        }
    }

    /// Shakes hands and opens the thread. The thread's own settings go in
    /// thread/start; effort only exists per turn.
    pub(crate) fn begin(&self, o: &StartOptions) -> adapter::Result<()> {
        let rpc = &self.rpc;
        let version = rpc.initialize(None)?;
        let models = read_models(&o.profile.dir);
        let model = model_id(&o.model, &models)?;
        {
            let mut st = self.inner.st();
            (st.version, st.models) = (version.clone(), models);
        }
        let mut params = serde_json::Map::new();
        let mut set = |k: &str, v: &str| {
            if !v.is_empty() {
                params.insert(k.into(), v.into());
            }
        };
        set("model", &model);
        set("cwd", &o.dir);
        // rush's own prompt, which Claude Code gets in --append-system-prompt.
        set("developerInstructions", o.prompt.trim());
        if let Some(t) = o
            .tools
            .iter()
            .find(|t| t.name == TOOLS_SERVER && !t.args.is_empty())
            && let Ok(exe) = std::env::current_exe()
        {
            // rush's own tools, which Claude Code is served in process, as a
            // process; a call may wait on another agent's whole task. Its
            // trusted tools never go to review: they draw, or start what
            // Codex's own subagents would.
            let tools: serde_json::Map<String, Value> = t
                .trusted
                .iter()
                .map(|n| (n.clone(), json!({"approval_mode": "approve"})))
                .collect();
            // Codex starts it with a bare environment; it needs the
            // session's (RUSH_HOME).
            let mut env: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
            env.extend(o.env.iter().cloned());
            params.insert(
                "config".into(),
                json!({ format!("mcp_servers.{TOOLS_SERVER}"): {
                    "command": exe.to_string_lossy(), "args": t.args,
                    "tool_timeout_sec": 3600, "env_vars": env_names(&env), "tools": tools,
                }}),
            );
        }
        if !o.skill_roots.is_empty() {
            // Claude Code's skills beside Codex's own; not reading them is
            // no reason not to start.
            let _ = rpc.call(
                "skills/extraRoots/set",
                json!({"extraRoots": o.skill_roots}),
                None,
            );
        }
        if let Some(m) = mode(&o.mode) {
            params.insert("approvalPolicy".into(), m.approval.into());
            params.insert("sandbox".into(), m.sandbox.into());
        }
        self.inner.st().next.effort = o.effort.clone();
        let mut method = "thread/start";
        if o.fork || o.resume {
            method = if o.fork {
                "thread/fork"
            } else {
                "thread/resume"
            };
            params.insert("threadId".into(), o.session_id.clone().into());
            params.insert("excludeTurns".into(), true.into());
        }
        let res = rpc.call(method, Value::Object(params), None)?;
        let res: Value = serde_json::from_str(res.get()).unwrap_or(Value::Null);
        let thread = res.get("thread").map(|t| st(t, "id")).unwrap_or_default();
        let model = st(&res, "model");
        {
            let mut st = self.inner.st();
            (st.thread, st.model) = (thread.clone(), model.clone());
        }
        if method == "thread/start" && !o.carry.is_empty() {
            // Another agent's conversation, as this thread's own history.
            rpc.call(
                "thread/inject_items",
                json!({"threadId": thread, "items": carried(&o.carry)}),
                None,
            )
            .map_err(|e| err(format!("codex: taking the conversation on: {e}")))?;
        }
        let mut init = Init {
            session_id: thread,
            model,
            cwd: st(&res, "cwd"),
            mode: o.mode.clone(),
            version,
            ..Init::default()
        };
        if init.mode.is_empty() {
            // Its own policy, as the preset that sets it: a raw
            // "on-request" saved as the mode would fail the next start.
            let sandbox = res
                .get("sandbox")
                .map(|x| st(x, "type"))
                .unwrap_or_default();
            init.mode = mode_of(s(&res, "approvalPolicy"), &sandbox).into();
        }
        self.inner.emit(vec![Event::Init(Box::new(init))]);
        let (rpc, inner) = (self.rpc.clone(), self.inner.clone());
        std::thread::Builder::new()
            .name("codex-quota".into())
            .spawn(move || read_quota_once(&rpc, &inner))?;
        Ok(())
    }

    /// Changes the reasoning effort from the next turn on.
    pub fn set_effort(&self, effort: &str) -> adapter::Result<()> {
        self.inner.st().next.effort = effort.to_string();
        Ok(())
    }

    fn take(&self, id: &str) -> adapter::Result<Pending> {
        self.inner
            .st()
            .asks
            .remove(id)
            .ok_or_else(|| err(format!("codex: nothing is waiting on {id}")))
    }

    fn put(&self, id: &str, a: Pending) {
        self.inner.st().asks.insert(id.to_string(), a);
    }
}

fn new_inner(sink: Arc<dyn Sink>) -> Arc<Inner> {
    Arc::new(Inner {
        sink,
        st: Mutex::new(State::default()),
        said_end: AtomicBool::new(false),
        quiet: AtomicBool::new(false),
    })
}

fn handler(inner: &Arc<Inner>) -> Handler {
    let inner = inner.clone();
    Box::new(move |rpc, inc| inner.handle(rpc, inc))
}

/// Reads the account's limits once, which also tells later live readings
/// whose account they are, and how the account pays.
fn read_quota_once(rpc: &Client, inner: &Inner) {
    let within = Some(Duration::from_secs(30));
    if let Ok(a) = rpc.call("account/read", json!({}), within)
        && let Some(b) = account_billing(&crate::json::parse(a.get()))
    {
        inner.emit(vec![Event::Billing(Billing { billing: b })]);
    }
    let Ok(q) = read_quota(rpc, within) else {
        return;
    };
    inner.st().account = q.account.clone();
    inner.emit(vec![Event::Quota(q)]);
}

/// Starts or resumes a thread: CODEX_HOME is the profile's folder, and the
/// model, effort and mode are Codex's own names.
pub fn start(mut o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Conn> {
    if mode(&o.mode).is_none() {
        // A mode saved under another harness or an older name starts on
        // Codex's own, rather than not at all.
        o.mode.clear();
    }
    let inner = new_inner(sink);
    let rpc = spawn(
        &o.binary,
        &o.profile.dir,
        &o.env,
        &o.flags,
        Some(handler(&inner)),
    )?;
    let c = Conn {
        rpc,
        inner,
        closed: AtomicBool::new(false),
    };
    if let Err(e) = c.begin(&o) {
        c.inner.quiet.store(true, Ordering::SeqCst);
        let _ = c.close();
        return Err(e);
    }
    Ok(c)
}

impl ConnTrait for Conn {
    /// Starts a turn, or steers the running one. /compact compacts the
    /// thread, as a turn of its own.
    fn send(&self, input: Input) -> adapter::Result<()> {
        if input.text.trim() == "/compact" {
            if !input.images.is_empty() {
                return Err(err("codex: /compact does not accept attachments"));
            }
            let (thread, turn) = {
                let st = self.inner.st();
                (st.thread.clone(), st.turn.clone())
            };
            if !turn.is_empty() {
                return Err(err("codex: let the current turn finish before compacting"));
            }
            self.rpc
                .call("thread/compact/start", json!({"threadId": thread}), None)?;
            return Ok(());
        }
        let mut parts = vec![json!({"type": "text", "text": input.text, "text_elements": []})];
        for p in &input.images {
            parts.push(json!({"type": "localImage", "path": p}));
        }
        let (thread, turn, next) = {
            let st = self.inner.st();
            (st.thread.clone(), st.turn.clone(), st.next.clone())
        };
        if !turn.is_empty() {
            match self.rpc.call(
                "turn/steer",
                json!({"threadId": thread, "input": parts, "expectedTurnId": turn}),
                None,
            ) {
                Ok(_) => return Ok(()),
                // The turn ended meanwhile or can't be steered: start another.
                Err(e) if e.downcast_ref::<RpcError>().is_some() => {}
                Err(e) => return Err(e), // the app-server is gone
            }
        }
        let mut params = serde_json::Map::new();
        params.insert("threadId".into(), thread.into());
        params.insert("input".into(), parts.into());
        if !next.model.is_empty() {
            params.insert("model".into(), next.model.clone().into());
        }
        if !next.effort.is_empty() {
            params.insert("effort".into(), next.effort.clone().into());
        }
        if let Some(m) = mode(&next.mode) {
            params.insert("approvalPolicy".into(), m.approval.into());
            params.insert("sandboxPolicy".into(), (m.turn)());
        }
        let res = self.rpc.call("turn/start", Value::Object(params), None)?;
        let res: Value = serde_json::from_str(res.get()).unwrap_or(Value::Null);
        let id = res.get("turn").map(|t| st(t, "id")).unwrap_or_default();
        let mut st = self.inner.st();
        if st.turn.is_empty() && st.ended != id {
            st.turn = id;
        }
        // Codex keeps a turn's overrides for the turns after it.
        if st.next == next {
            st.next = Next::default();
        }
        if !next.model.is_empty() {
            st.model = next.model;
        }
        Ok(())
    }

    /// Answers an approval with one of its options' ids.
    fn answer(&self, approval_id: &str, option_id: &str) -> adapter::Result<()> {
        let a = self.take(approval_id)?;
        match a.method.as_str() {
            REQ_COMMAND | REQ_FILE_CHANGE => {
                if matches!(
                    option_id,
                    "accept" | "acceptForSession" | "decline" | "cancel"
                ) {
                    return self.rpc.reply(&a.id, json!({"decision": option_id}));
                }
            }
            REQ_PERMISSIONS => match option_id {
                "turn" | "session" => {
                    let granted: serde_json::Map<String, Value> = a
                        .perms
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    return self
                        .rpc
                        .reply(&a.id, json!({"permissions": granted, "scope": option_id}));
                }
                "decline" => {
                    return self
                        .rpc
                        .reply(&a.id, json!({"permissions": {}, "scope": "turn"}));
                }
                _ => {}
            },
            REQ_USER_INPUT => {
                self.put(approval_id, a);
                return Err(err(format!(
                    "codex: {approval_id} is a question; answer it with AnswerQuestion"
                )));
            }
            _ => {}
        }
        let method = a.method.clone();
        self.put(approval_id, a);
        Err(err(format!(
            "codex: {option_id:?} is no answer to {method}"
        )))
    }

    /// Stops the running turn, if there is one.
    fn interrupt(&self) -> adapter::Result<()> {
        let (thread, turn) = {
            let st = self.inner.st();
            (st.thread.clone(), st.turn.clone())
        };
        if turn.is_empty() {
            return Ok(());
        }
        self.rpc.call(
            "turn/interrupt",
            json!({"threadId": thread, "turnId": turn}),
            None,
        )?;
        Ok(())
    }

    /// Changes the model from the next turn on.
    fn set_model(&self, model: &str) -> adapter::Result<()> {
        let mut st = self.inner.st();
        st.next.model = model_id(model, &st.models)?;
        Ok(())
    }

    /// Changes the approval preset from the next turn on: read-only, auto
    /// or full-access.
    fn set_mode(&self, m: &str) -> adapter::Result<()> {
        if mode(m).is_none() {
            return Err(err(format!("codex: unknown mode {m:?}")));
        }
        self.inner.st().next.mode = m.to_string();
        Ok(())
    }

    /// Ends the app-server and the thread with it.
    fn close(&self) -> adapter::Result<()> {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.rpc.close();
        }
        Ok(())
    }

    fn answerer(&self) -> Option<&dyn Answerer> {
        Some(self)
    }

    fn task_stopper(&self) -> Option<&dyn TaskStopper> {
        Some(self)
    }

    fn pid(&self) -> Option<i32> {
        self.rpc.pid()
    }
}

impl Answerer for Conn {
    /// Answers a request_user_input question, keyed by each Ask's id or
    /// text.
    fn answer_question(
        &self,
        id: &str,
        answers: &HashMap<String, Vec<String>>,
    ) -> adapter::Result<()> {
        let a = self.take(id)?;
        if a.method != REQ_USER_INPUT {
            self.put(id, a);
            return Err(err(format!("codex: {id} is not a question")));
        }
        let out: serde_json::Map<String, Value> = answers
            .iter()
            .map(|(k, v)| {
                let k = a.qs.get(k).cloned().unwrap_or_else(|| k.clone());
                (k, json!({"answers": v}))
            })
            .collect();
        self.rpc.reply(&a.id, json!({"answers": out}))
    }
}

impl TaskStopper for Conn {
    /// Interrupts the turn a spawned thread is running, or ends a
    /// background shell's terminal.
    fn stop_task(&self, id: &str) -> adapter::Result<()> {
        let shell = self.inner.st().shell_of(id);
        if let Some((thread, process)) = shell {
            self.rpc.call(
                "thread/backgroundTerminals/terminate",
                json!({"threadId": thread, "processId": process}),
                None,
            )?;
            return Ok(());
        }
        let Some((thread, turn)) = self.inner.st().child(id) else {
            return Err(err(format!("codex: no subagent or shell {id}")));
        };
        if turn.is_empty() {
            return Ok(()); // done already
        }
        self.rpc.call(
            "turn/interrupt",
            json!({"threadId": thread, "turnId": turn}),
            None,
        )?;
        Ok(())
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        let _ = ConnTrait::close(self);
    }
}

/// Lines as Responses API messages: yours as input, the other agent's
/// answers as output.
fn carried(lines: &[PreviewLine]) -> Vec<Value> {
    lines
        .iter()
        .map(|l| {
            let part = if l.role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            json!({"type": "message", "role": l.role, "content": [{"type": part, "text": l.text}]})
        })
        .collect()
}

/// The names of env's variables, each once.
fn env_names(env: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for kv in env {
        if let Some((k, _)) = kv.split_once('=')
            && !k.is_empty()
            && !names.iter().any(|n| n == k)
        {
            names.push(k.to_string());
        }
    }
    names
}

#[cfg(test)]
mod tests;
