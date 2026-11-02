//! A rollout is the JSONL file Codex writes each thread to, under
//! $CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<uuid>.jsonl. Each line
//! is {timestamp, ordinal, type, payload}. This reads one back as the
//! events a live thread sends. Port of internal/adapters/codex/rollout.go:
//! 1-853.
//!
//! A rollout says most things twice: once as the response_items the model
//! saw (messages, reasoning, the "exec" and "apply_patch" tool calls), and
//! again as event_msg item_completed records, the same ThreadItems the
//! app-server sends a live client. The items are the richer of the two
//! (commands with their exit codes and output, file changes as diffs, user
//! prompts without Codex's injected context), so a turn that has any
//! item_completed record is read from its items alone, and a turn that has
//! none, as older and guardian rollouts do, from its response_items. The
//! event_msg user_message and agent_message records and the
//! token_usage_record lines repeat what is read elsewhere and are skipped.
//!
//! Go reads a turn whole before it gives its events. Here a turn's events
//! are given as soon as which account of it they come from is known (its
//! first item says so), so a history read a slice at a time, and one
//! followed as it grows, gives each event once and never takes one back.

use std::io::{self, BufRead, Read};
use std::time::Duration;

use rush_core::event::{
    Compacted, Context, Event, Init, Limited, Message, Other, Part, PartKind, StartNotice, TurnEnd,
};
use rush_core::tool::{Call, Kind, Output};
use rush_core::usage::TokenUsage;
use rush_core::wire::RawJson;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::items::{
    ContentItem, ThreadItem, action_of, as_claude, call_of, content_items, data_image, image_part,
    local_path, output_of, text_part, texts,
};
use crate::json::{Envelope, arr, get, int, is_null, keep, ms, parse, s, st, strs};
use crate::quota::{Snapshot, quota_from};

/// The longest line a rollout reader takes. Compactions and command
/// outputs can run to several megabytes.
pub(crate) const MAX_LINE: u64 = 256 << 20;

/// Calls f with each line of r (trimmed, empty ones left out) and how many
/// bytes it took, however long, until f says to stop. A line longer than
/// MAX_LINE is an error.
pub(crate) fn read_lines(
    r: &mut impl BufRead,
    mut f: impl FnMut(&[u8], usize) -> bool,
) -> io::Result<()> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = r.take(MAX_LINE + 1).read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if n as u64 > MAX_LINE {
            return Err(io::Error::other("bufio.Scanner: token too long"));
        }
        let line = buf.trim_ascii();
        if !line.is_empty() && !f(line, n) {
            return Ok(());
        }
        if buf.capacity() > 32 << 20 {
            buf = Vec::new(); // one huge line needn't be held on to
        }
    }
}

/// A rollout's token usage, in snake case, as rush counts it. OpenAI's
/// input includes the cached input and its output includes the reasoning,
/// so input is what wasn't cached and output is taken as it is.
pub(crate) fn rollout_tokens(v: &Value) -> TokenUsage {
    let i = |k: &str| int(v, k).unwrap_or(0);
    TokenUsage {
        input: (i("input_tokens") - i("cached_input_tokens")).max(0),
        output: i("output_tokens"),
        cache_read: i("cached_input_tokens"),
        reasoning: i("reasoning_output_tokens"),
        ..TokenUsage::default()
    }
}

/// Whether a session_meta's source says another thread started it: a
/// spawned agent or Codex's guardian reviewer.
pub(crate) fn is_subagent(meta: &Value) -> bool {
    meta.get("source")
        .and_then(Value::as_object)
        .is_some_and(|o| o.contains_key("subagent"))
}

/// Whether a session_meta's source says it's a codex exec run.
pub(crate) fn is_exec(meta: &Value) -> bool {
    meta.get("source").and_then(Value::as_str) == Some("exec")
}

/// A command as one line: the script of a "sh -lc script", or the words
/// joined.
pub(crate) fn argv(raw: &Value) -> String {
    if let Some(s) = raw.as_str() {
        return s.to_string();
    }
    let words: Vec<&str> = raw
        .as_array()
        .map(|a| a.iter().map(|w| w.as_str().unwrap_or("")).collect())
        .unwrap_or_default();
    if words.len() == 3 && (words[1] == "-lc" || words[1] == "-c") {
        return words[2].to_string();
    }
    words.join(" ")
}

/// A rollout's item_completed item (PascalCase types and snake-case
/// fields, unlike the app-server's) as the app-server would have sent it,
/// so call_of and output_of read both alike. None for an item that isn't
/// a tool call.
pub(crate) fn rollout_thread_item(v: &Value) -> Option<ThreadItem> {
    let mut t = ThreadItem {
        id: st(v, "id"),
        status: st(v, "status"),
        ..ThreadItem::default()
    };
    match s(v, "type") {
        "CommandExecution" => {
            t.typ = "commandExecution".into();
            t.command = argv(v.get("command").unwrap_or(&Value::Null));
            t.cwd = st(v, "cwd");
            t.aggregated_output = get(v, "aggregated_output")
                .and_then(Value::as_str)
                .map(str::to_string);
            t.exit_code = int(v, "exit_code");
            for p in arr(v, "parsed_cmd") {
                let kind = match s(p, "type") {
                    "list_files" => "listFiles",
                    k => k,
                };
                t.command_actions.push(crate::items::CommandAction {
                    typ: kind.into(),
                    name: st(p, "name"),
                    path: st(p, "path"),
                    query: st(p, "query"),
                });
            }
        }
        "FileChange" => {
            t.typ = "fileChange".into();
            if let Some(m) = v.get("changes").and_then(Value::as_object) {
                // serde_json's map is ordered by key, as Go sorts them.
                for (path, ch) in m {
                    let typ = st(ch, "type");
                    let diff = if typ == "update" {
                        st(ch, "unified_diff")
                    } else {
                        st(ch, "content")
                    };
                    t.changes.push(crate::items::FileUpdate {
                        path: path.clone(),
                        kind: typ,
                        move_path: st(ch, "move_path"),
                        diff,
                    });
                }
            }
        }
        "ImageView" => (t.typ, t.path) = ("imageView".into(), st(v, "path")),
        "Extension" => {
            if s(v, "kind") != "web.search" {
                return None;
            }
            (t.typ, t.query) = ("webSearch".into(), st(v, "query"));
            t.action = get(v, "action").and_then(action_of);
        }
        _ => return None,
    }
    Some(t)
}

/// Which of a rollout's two accounts of a turn an event comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Src {
    Both,
    Item,
    Response,
}

/// One of a turn's events, and where it came from; None is where Init
/// goes, once its model is known.
struct Tagged {
    src: Src,
    ev: Option<Event>,
}

/// Turns a rollout's lines into events, a turn at a time.
#[derive(Default)]
pub(crate) struct Replay {
    out: Vec<Event>,

    turn: Vec<Tagged>,
    /// How many of turn have been given already.
    given: usize,
    /// The turn has item_completed records.
    items: bool,
    /// The last turn started, and whether it still runs.
    turn_id: String,
    in_turn: bool,

    meta: bool,
    init: Init,
    init_model: bool,
    agent_path: String,

    model: String,
    tokens: TokenUsage,
    context: Option<Context>,
    last_quota: String,
    /// The first user-authored prompt has been seen.
    prompted: bool,
}

impl Replay {
    /// Reads one rollout line.
    pub fn line(&mut self, b: &[u8]) {
        if let Ok(l) = serde_json::from_slice::<Envelope>(b) {
            self.read(&l);
        }
    }

    /// Reads one rollout line, already split into its envelope.
    pub fn read(&mut self, l: &Envelope) {
        let payload = l.payload.map_or("null", RawValue::get);
        match &*l.typ {
            "session_meta" => {
                if self.meta {
                    return; // a fork's parent, after the thread's own
                }
                let m = parse(payload);
                if !m.is_object() {
                    return;
                }
                self.meta = true;
                self.init = Init {
                    session_id: st(&m, "id"),
                    cwd: st(&m, "cwd"),
                    version: st(&m, "cli_version"),
                    ..Init::default()
                };
                self.agent_path = st(&m, "agent_path");
                // Init, once its model is known.
                self.turn.push(Tagged {
                    src: Src::Both,
                    ev: None,
                });
            }
            "turn_context" => {
                let c = parse(payload);
                if !c.is_object() {
                    return;
                }
                let model = s(&c, "model");
                if !model.is_empty() {
                    self.model = model.to_string();
                }
                if !self.init_model {
                    self.init.model = model.to_string();
                    self.init.mode = st(&c, "approval_policy");
                    self.init.effort = st(&c, "effort");
                    self.init_model = true;
                }
            }
            "compacted" => {
                let c = parse(payload);
                let mut ev = Compacted::default();
                if let Some(u) = get(&c, "latest_token_usage_record").and_then(|r| get(r, "usage"))
                {
                    ev.before = int(u, "total_tokens").unwrap_or(0);
                }
                self.add(Src::Both, Event::Compacted(ev));
            }
            "event_msg" => self.event_msg(&l.timestamp, payload),
            "response_item" => self.response_item(payload),
            _ => {}
        }
    }

    fn add(&mut self, src: Src, ev: Event) {
        self.turn.push(Tagged { src, ev: Some(ev) });
    }

    /// Ends the turn: what's left of it is given from the account it is
    /// read from.
    pub fn flush(&mut self) {
        let drop = if self.items { Src::Response } else { Src::Item };
        for t in self.turn.drain(self.given..) {
            match t.ev {
                _ if t.src == drop => {}
                None => self.out.push(Event::Init(Box::new(self.init.clone()))),
                Some(ev) => self.out.push(ev),
            }
        }
        self.turn.clear();
        (self.given, self.items) = (0, false);
    }

    /// The events read so far whose place is settled: every ended turn's,
    /// and those of the turn under way that no later line can take back.
    /// Each is given once.
    pub fn take(&mut self) -> Vec<Event> {
        let mut out = std::mem::take(&mut self.out);
        let (items, init_model) = (self.items, self.init_model);
        while let Some(t) = self.turn.get_mut(self.given) {
            match (t.src, t.ev.take()) {
                (Src::Response, _) if items => {}
                // Undecided until the turn has an item, or ends.
                (Src::Response, ev) => {
                    t.ev = ev;
                    break;
                }
                (_, None) if !init_model => break,
                (_, None) => out.push(Event::Init(Box::new(self.init.clone()))),
                (_, Some(ev)) => out.push(ev),
            }
            self.given += 1;
        }
        out
    }

    fn event_msg(&mut self, timestamp: &str, payload: &str) {
        let m = parse(payload);
        match s(&m, "type") {
            "task_started" => {
                (self.tokens, self.context) = (TokenUsage::default(), None);
                (self.turn_id, self.in_turn) = (st(&m, "turn_id"), true);
            }
            t @ ("task_complete" | "turn_aborted") => {
                self.in_turn = false;
                let mut end = TurnEnd {
                    reason: "done".into(),
                    tokens: self.tokens,
                    turns: 1,
                    ..TurnEnd::default()
                };
                if t == "turn_aborted" {
                    end.reason = st(&m, "reason");
                    if end.reason.is_empty() {
                        end.reason = "interrupted".into();
                    }
                }
                if let Some(d) = int(&m, "duration_ms") {
                    end.duration = Duration::from_millis(d.max(0) as u64);
                }
                if let Some(c) = self.context.take() {
                    self.add(Src::Both, Event::Context(c));
                }
                if let Some(e) = get(&m, "error") {
                    (end.reason, end.err) = ("error".into(), st(e, "message"));
                    if s(e, "codex_error_info") == "usage_limit_exceeded" {
                        self.add(Src::Both, Event::Limited(Limited::default()));
                    }
                }
                self.add(Src::Both, Event::TurnEnd(end));
                (self.tokens, self.context) = (TokenUsage::default(), None);
                self.flush();
            }
            "token_count" => {
                if let Some(info) = get(&m, "info") {
                    let last = info.get("last_token_usage").unwrap_or(&Value::Null);
                    self.tokens.add(rollout_tokens(last));
                    if let Some(w) = int(info, "model_context_window") {
                        self.context = Some(Context {
                            tokens: int(last, "total_tokens").unwrap_or(0),
                            window: w,
                        });
                    }
                }
                if let Some(lim) = m.get("rate_limits").filter(|v| v.is_object()) {
                    let text = serde_json::to_string(lim).unwrap_or_default();
                    if text != self.last_quota {
                        self.last_quota = text;
                        let mut q = quota_from(&Snapshot::from_rollout(lim));
                        let t = ms(timestamp);
                        if t != 0 {
                            q.fetched_at = t;
                        }
                        self.add(Src::Both, Event::Quota(q));
                    }
                }
            }
            "thread_settings_applied" => {
                if let Some(ts) = get(&m, "thread_settings") {
                    let model = s(ts, "model");
                    if !model.is_empty() {
                        self.model = model.to_string();
                    }
                }
            }
            "item_completed" => {
                self.items = true;
                // An item that completes after its turn ended ran beside
                // the turns after it.
                let turn = s(&m, "turn_id");
                let outlived = !turn.is_empty()
                    && !self.turn_id.is_empty()
                    && (turn != self.turn_id || !self.in_turn);
                let raw = item_raw(payload);
                for ev in self.item(m.get("item").unwrap_or(&Value::Null), raw, outlived) {
                    self.add(Src::Item, ev);
                }
            }
            _ => {}
        }
    }

    /// What one completed item means as events. A command that outlived
    /// its turn ran in the background.
    fn item(&self, v: &Value, raw: Option<RawJson>, outlived: bool) -> Vec<Event> {
        if !v.is_object() {
            return Vec::new();
        }
        match s(v, "type") {
            "UserMessage" => {
                let mut m = Message {
                    role: "user".into(),
                    id: st(v, "id"),
                    ..Message::default()
                };
                for p in arr(v, "content") {
                    match s(p, "type") {
                        "text" => m.parts.push(text_part(PartKind::Text, st(p, "text"))),
                        "local_image" => m.parts.push(image_part(rush_core::event::ImageData {
                            path: st(p, "path"),
                            ..Default::default()
                        })),
                        _ => {}
                    }
                }
                if m.parts.is_empty() {
                    return Vec::new();
                }
                vec![Event::Message(m)]
            }
            "AgentMessage" => {
                let t = texts(&content_items(arr(v, "content")));
                if t.is_empty() {
                    return Vec::new();
                }
                vec![self.assistant(&st(v, "id"), text_part(PartKind::Text, t))]
            }
            "Reasoning" => {
                let mut all = strs(v, "summary_text");
                all.extend(strs(v, "raw_content"));
                let t = all.join("\n\n").trim().to_string();
                if t.is_empty() {
                    return Vec::new(); // encrypted, with no summary
                }
                vec![self.assistant(&st(v, "id"), text_part(PartKind::Thinking, t))]
            }
            "ContextCompaction" => Vec::new(), // the compacted line says it
            typ => {
                let Some(t) = rollout_thread_item(v) else {
                    return vec![Event::Other(Other {
                        adapter: "codex".into(),
                        kind: format!("item/{typ}"),
                        raw,
                    })];
                };
                let mut call = call_of(&t, raw.clone()).unwrap_or_default();
                call.input.background = outlived && t.typ == "commandExecution";
                let out = output_of(&t, raw);
                vec![
                    self.assistant(&call.id.clone(), call_part(call)),
                    result(out),
                ]
            }
        }
    }

    fn assistant(&self, id: &str, p: Part) -> Event {
        Event::Message(Message {
            role: "assistant".into(),
            id: id.to_string(),
            model: self.model.clone(),
            parts: vec![p],
            ..Message::default()
        })
    }

    fn response_item(&mut self, payload: &str) {
        let ri = parse(payload);
        if !ri.is_object() {
            return;
        }
        let content = arr(&ri, "content");
        match s(&ri, "type") {
            "message" => match s(&ri, "role") {
                "user" => {
                    if !self.prompted {
                        for c in content {
                            if s(c, "type") == "input_text" && startup_context(s(c, "text")) {
                                self.add(Src::Both, start_notice(st(c, "text")));
                            }
                        }
                    }
                    if let Some(m) = user_prompt(&ri) {
                        self.prompted = true;
                        self.add(Src::Response, Event::Message(m));
                    }
                }
                "developer" => {
                    if !self.prompted {
                        for c in content {
                            if s(c, "type") == "input_text" && !s(c, "text").trim().is_empty() {
                                self.add(Src::Both, start_notice(st(c, "text")));
                            }
                        }
                    }
                }
                "assistant" => {
                    let b: Vec<&str> = content
                        .iter()
                        .map(|c| s(c, "text"))
                        .filter(|t| !t.is_empty())
                        .collect();
                    if !b.is_empty() {
                        let ev =
                            self.assistant(s(&ri, "id"), text_part(PartKind::Text, b.join("\n")));
                        self.add(Src::Response, ev);
                    }
                }
                _ => {}
            },
            "reasoning" => {
                let b: Vec<&str> = arr(&ri, "summary")
                    .iter()
                    .map(|c| s(c, "text"))
                    .filter(|t| !t.is_empty())
                    .collect();
                if !b.is_empty() {
                    let ev =
                        self.assistant(s(&ri, "id"), text_part(PartKind::Thinking, b.join("\n\n")));
                    self.add(Src::Response, ev);
                }
            }
            "function_call" | "custom_tool_call" => {
                let call = response_call(&ri);
                let ev = self.assistant(&call.id.clone(), call_part(call));
                self.add(Src::Response, ev);
            }
            "function_call_output" | "custom_tool_call_output" => {
                let o = Output {
                    call_id: st(&ri, "call_id"),
                    text: output_text(ri.get("output").unwrap_or(&Value::Null)),
                    raw: RawJson::new(payload).ok(),
                    ..Output::default()
                };
                self.add(Src::Response, result(o));
            }
            "agent_message" => {
                // A message from another agent. There is no item for one
                // sent to this thread, and it is how a spawned agent gets
                // its task.
                let me = if self.agent_path.is_empty() {
                    "/root"
                } else {
                    &self.agent_path
                };
                if s(&ri, "recipient") != me {
                    return;
                }
                let mut m = Message {
                    role: "user".into(),
                    id: st(&ri, "id"),
                    ..Message::default()
                };
                for c in content {
                    if s(c, "type") == "input_text" && !s(c, "text").is_empty() {
                        m.parts.push(text_part(PartKind::Text, st(c, "text")));
                    }
                }
                if !m.parts.is_empty() {
                    self.add(Src::Both, Event::Message(m));
                }
            }
            _ => {}
        }
    }
}

/// An item_completed payload's item, kept as written.
fn item_raw(payload: &str) -> Option<RawJson> {
    #[derive(serde::Deserialize)]
    struct P<'a> {
        #[serde(borrow)]
        item: Option<&'a RawValue>,
    }
    let p: P = serde_json::from_str(payload).ok()?;
    p.item.filter(|r| !is_null(r.get())).map(keep)
}

fn start_notice(text: String) -> Event {
    Event::StartNotice(StartNotice {
        event: "SessionStart".into(),
        text,
    })
}

pub(crate) fn call_part(c: Call) -> Part {
    Part {
        kind: PartKind::ToolCall,
        call: Some(Box::new(c)),
        ..Part::default()
    }
}

/// A tool's output as the user message that carries it back.
pub(crate) fn result(o: Output) -> Event {
    Event::Message(Message {
        role: "user".into(),
        id: format!("{}:result", o.call_id),
        parts: vec![Part {
            kind: PartKind::ToolResult,
            output: Some(Box::new(o)),
            ..Part::default()
        }],
        ..Message::default()
    })
}

/// A user message as the user wrote it, without the context Codex adds as
/// user messages of its own. None when nothing is left.
pub(crate) fn user_prompt(ri: &Value) -> Option<Message> {
    let mut m = Message {
        role: "user".into(),
        id: st(ri, "id"),
        ..Message::default()
    };
    for c in arr(ri, "content") {
        match s(c, "type") {
            "input_text" => {
                let t = s(c, "text");
                if !injected(t) {
                    m.parts.push(text_part(PartKind::Text, t.to_string()));
                }
            }
            "input_image" => {
                let url = s(c, "image_url");
                if let Some(img) = data_image(url) {
                    m.parts.push(image_part(img));
                } else if !url.is_empty() {
                    m.parts.push(image_part(rush_core::event::ImageData {
                        path: url.to_string(),
                        ..Default::default()
                    }));
                }
            }
            _ => {}
        }
    }
    m.parts
        .iter()
        .any(|p| matches!(p.kind, PartKind::Text | PartKind::Image))
        .then_some(m)
}

/// Whether a user message's text is context Codex adds rather than
/// something the user typed: AGENTS.md, or a block wrapped in one tag such
/// as <environment_context>, <user_instructions>, <recommended_plugins> or
/// <turn_aborted>. Images come with text parts labelling them, "<image
/// name=...>" and "</image>", which are Codex's too.
pub(crate) fn injected(text: &str) -> bool {
    let s = text.trim();
    if s.starts_with("# AGENTS.md instructions") {
        return true;
    }
    if s.starts_with("<image") || s == "</image>" {
        return true;
    }
    if !s.starts_with('<') {
        return false;
    }
    let Some(end) = s.find(['>', ' ']) else {
        return false;
    };
    if end < 2 {
        return false;
    }
    let tag = &s[1..end];
    if !tag
        .chars()
        .all(|c| c == '_' || c == '-' || c.is_ascii_alphabetic())
    {
        return false;
    }
    s.ends_with(&format!("</{tag}>"))
}

fn startup_context(text: &str) -> bool {
    let s = text.trim();
    injected(s) && !s.starts_with("<image") && s != "</image>" && !s.starts_with("<turn_aborted")
}

/// A tool output as text: a string, or content parts.
fn output_text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    let parts: Vec<ContentItem> = v.as_array().map(|a| content_items(a)).unwrap_or_default();
    texts(&parts)
}

/// One of Codex's agent tools as the Claude Code tool it is like. What it
/// said is kept encrypted, so only a plain message is shown.
fn agent_call(mut c: Call, name: &str, args: &Value) -> Call {
    let target = st(args, "target");
    let mut message = st(args, "message");
    if message.starts_with("gAAAA") {
        message.clear();
    }
    match name {
        "send_message" | "resume_agent" => as_claude(
            &mut c,
            "SendMessage",
            &[("to", target.into()), ("message", message.into())],
        ),
        "followup_task" => as_claude(
            &mut c,
            "SendMessage",
            &[
                ("to", target.into()),
                ("message", message.into()),
                ("summary", "a follow-up task".into()),
            ],
        ),
        "interrupt_agent" | "close_agent" => {
            as_claude(&mut c, "TaskStop", &[("task_id", target.into())])
        }
        "list_agents" => as_claude(&mut c, "ListAgents", &[]),
        _ => as_claude(&mut c, "TaskOutput", &[("task_id", "its subagents".into())]), // wait_agent
    }
    c
}

/// A function or custom tool call as the model made it. Turns without
/// items are read from these; the commands Codex's "exec" tool runs are
/// only known from items.
pub(crate) fn response_call(ri: &Value) -> Call {
    let name = s(ri, "name");
    let arguments = s(ri, "arguments");
    let input = s(ri, "input");
    let mut c = Call {
        id: st(ri, "call_id"),
        name: name.to_string(),
        raw: RawJson::new(arguments).ok(),
        ..Call::default()
    };
    if s(ri, "type") == "custom_tool_call" {
        c.raw = serde_json::to_string(input)
            .ok()
            .and_then(|t| RawJson::new(&t).ok());
    }
    let args = parse(arguments);
    match name {
        "exec_command" | "shell" | "local_shell" | "shell_command" => {
            let cmd = match get(&args, "cmd") {
                Some(c) => c,
                None => args.get("command").unwrap_or(&Value::Null),
            };
            c.kind = Kind::Shell;
            c.input.command = argv(cmd);
            c.input.cwd = st(&args, "workdir");
            c.input.timeout = int(&args, "timeout_ms").unwrap_or(0);
        }
        "apply_patch" => {
            c.kind = Kind::Edit;
            for ln in input.split('\n') {
                for (prefix, k) in [
                    ("*** Update File: ", Kind::Edit),
                    ("*** Add File: ", Kind::Write),
                    ("*** Delete File: ", Kind::Delete),
                ] {
                    if let Some(p) = ln.strip_prefix(prefix)
                        && c.input.path.is_empty()
                    {
                        (c.kind, c.input.path) = (k, p.trim().to_string());
                    }
                }
            }
        }
        "view_image" => {
            (c.kind, c.input.path) = (Kind::Read, local_path(s(&args, "path")));
        }
        "spawn_agent" => {
            // Its message is kept encrypted: the task names it.
            let task = st(&args, "task_name");
            c.kind = Kind::Subagent;
            (c.input.description, c.input.child) = (task.clone(), task);
            c.input.agent = st(&args, "agent_type");
            if c.input.agent.is_empty() {
                c.input.agent = st(&args, "model");
            }
        }
        "send_message" | "followup_task" | "resume_agent" | "interrupt_agent" | "close_agent"
        | "wait_agent" | "list_agents" => return agent_call(c, name, &args),
        _ => {}
    }
    let ns = s(ri, "namespace");
    if !ns.is_empty() {
        c.name = format!("{ns}.{name}");
    }
    c
}

/// Reads a whole rollout from r: the events of the lines written before
/// `before` (0: all of them).
pub(crate) fn read_rollout(r: impl Read, before: i64) -> io::Result<Vec<Event>> {
    let mut rp = Replay::default();
    let mut r = io::BufReader::with_capacity(1 << 20, r);
    let res = read_lines(&mut r, |b, _| feed(&mut rp, b, before));
    rp.flush();
    let out = rp.take();
    res.map(|_| out)
}

/// Feeds one line, unless it was written at or after before: false then,
/// to stop.
pub(crate) fn feed(rp: &mut Replay, b: &[u8], before: i64) -> bool {
    let Ok(l) = serde_json::from_slice::<Envelope>(b) else {
        return true; // a line cut short while Codex writes it
    };
    if before != 0 {
        let t = ms(&l.timestamp);
        if t != 0 && t >= before {
            return false;
        }
    }
    rp.read(&l);
    true
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rush_core::time::{format_rfc3339, parse_rfc3339};
    use serde_json::json;

    /// A synthetic rollout file, a line a second from t0 (rollout_test.go).
    pub(crate) struct Rollout {
        pub t0: i64,
        pub lines: Vec<String>,
    }

    pub(crate) fn t0() -> i64 {
        parse_rfc3339("2026-09-20T10:00:00Z")
    }

    /// t as Go's "2006-01-02T15:04:05.000Z".
    fn stamp(t: i64) -> String {
        let s = format_rfc3339(t - t.rem_euclid(1000));
        format!("{}.{:03}Z", s.trim_end_matches('Z'), t.rem_euclid(1000))
    }

    impl Rollout {
        pub fn new(t0: i64) -> Rollout {
            Rollout {
                t0,
                lines: Vec::new(),
            }
        }
        pub fn at(&self, i: usize) -> i64 {
            self.t0 + i as i64 * 1000
        }
        pub fn add(mut self, typ: &str, payload: Value) -> Rollout {
            let l = json!({"timestamp": stamp(self.at(self.lines.len())), "ordinal": self.lines.len(), "type": typ, "payload": payload});
            self.lines.push(l.to_string());
            self
        }
        pub fn event(self, typ: &str, mut p: Value) -> Rollout {
            p["type"] = typ.into();
            self.add("event_msg", p)
        }
        pub fn item(self, it: Value) -> Rollout {
            self.event(
                "item_completed",
                json!({"thread_id": "t", "turn_id": "u", "item": it}),
            )
        }
        pub fn response(self, typ: &str, mut p: Value) -> Rollout {
            p["type"] = typ.into();
            self.add("response_item", p)
        }
        pub fn text(&self) -> String {
            self.lines.join("\n") + "\n"
        }
        pub fn write(&self, path: &std::path::Path) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, self.text()).unwrap();
        }
    }

    pub(crate) fn user_msg(texts: &[&str]) -> Value {
        let c: Vec<Value> = texts
            .iter()
            .map(|t| json!({"type": "input_text", "text": t}))
            .collect();
        json!({"id": "m", "role": "user", "content": c})
    }

    pub(crate) fn meta(id: &str, source: Value) -> Value {
        json!({"id": id, "timestamp": format_rfc3339(t0()), "cwd": "/work", "cli_version": "0.1.0", "source": source,
            "git": {"branch": "main"}})
    }

    fn token_count(input: i64, cached: i64, out: i64, reasoning: i64, used: f64) -> Value {
        let u = json!({"input_tokens": input, "cached_input_tokens": cached, "output_tokens": out, "reasoning_output_tokens": reasoning,
            "total_tokens": input + out});
        json!({
            "info": {"total_token_usage": u, "last_token_usage": u, "model_context_window": 1000},
            "rate_limits": {"limit_id": "codex", "limit_name": null, "plan_type": "pro",
                "primary": {"used_percent": used, "window_minutes": 300, "resets_at": 1790000000}, "secondary": null},
        })
    }

    /// An event in a line, for comparing.
    pub(crate) fn describe(e: &Event) -> String {
        match e {
            Event::Init(e) => format!(
                "init {} {} {} {} {}",
                e.session_id, e.model, e.cwd, e.mode, e.version
            ),
            Event::Message(m) => {
                let mut parts = Vec::new();
                for p in &m.parts {
                    match p.kind {
                        PartKind::Text => parts.push(format!("text:{}", p.text)),
                        PartKind::Thinking => parts.push(format!("thinking:{}", p.text)),
                        PartKind::Image => {
                            let i = p.image.as_ref().unwrap();
                            parts.push(format!("image:{}{}", i.path, i.media_type))
                        }
                        PartKind::ToolCall => {
                            let c = p.call.as_ref().unwrap();
                            parts.push(format!(
                                "call:{} {} {} {}{}{}",
                                c.id, c.name, c.kind, c.input.command, c.input.path, c.input.query
                            ))
                        }
                        PartKind::ToolResult => {
                            let o = p.output.as_ref().unwrap();
                            let mut s =
                                format!("result:{} {:?} err={}", o.call_id, o.text, o.is_error);
                            if let Some(x) = o.exit {
                                s += &format!(" exit={x}");
                            }
                            for pt in &o.patches {
                                s += &format!(
                                    " patch:{}@{}+{}:{}",
                                    pt.path,
                                    pt.new_start,
                                    pt.new_lines,
                                    pt.lines.join("|")
                                );
                            }
                            parts.push(s)
                        }
                    }
                }
                format!("{}({}) {}", m.role, m.model, parts.join("; "))
            }
            Event::TurnEnd(e) => format!(
                "end {} {:?} in={} out={} cache={} reason={} {}",
                e.reason,
                e.err,
                e.tokens.input,
                e.tokens.output,
                e.tokens.cache_read,
                e.tokens.reasoning,
                go_duration(e.duration)
            ),
            Event::Quota(q) => {
                let w = &q.windows[0];
                let t = format_rfc3339(q.fetched_at);
                format!(
                    "quota {} {} {:.0}% {}",
                    q.plan,
                    w.label,
                    w.percent,
                    &t[11..19]
                )
            }
            Event::Context(c) => format!("context {}/{}", c.tokens, c.window),
            Event::StartNotice(s) => format!("start {} {}", s.event, s.text),
            Event::Compacted(c) => format!("compacted {}", c.before),
            Event::Limited(_) => "limited".into(),
            Event::Other(o) => format!("other {}", o.kind),
            e => format!("{:?}", e.name()),
        }
    }

    fn go_duration(d: Duration) -> String {
        let ms = d.as_millis();
        if ms < 1000 {
            format!("{ms}ms")
        } else if ms.is_multiple_of(1000) {
            format!("{}s", ms / 1000)
        } else {
            format!("{}s", ms as f64 / 1000.0)
        }
    }

    pub(crate) fn describe_all(evs: &[Event]) -> Vec<String> {
        evs.iter().map(describe).collect()
    }

    pub(crate) fn history(r: &Rollout, before: i64) -> Vec<String> {
        describe_all(&read_rollout(r.text().as_bytes(), before).unwrap())
    }

    /// A turn as a current Codex writes it: everything as a response_item
    /// and again as an item.
    pub(crate) fn item_turn(r: Rollout) -> Rollout {
        r.add("session_meta", meta("thread-1", "cli".into()))
            .event("task_started", json!({"turn_id": "u", "model_context_window": 1000}))
            .response("message", json!({"id": "d", "role": "developer", "content": [{"type": "input_text", "text": "<permissions>x</permissions>"}]}))
            .response("message", user_msg(&["<environment_context>\n  <cwd>/work</cwd>\n</environment_context>"]))
            .add("world_state", json!({"full": true}))
            .add("turn_context", json!({"model": "gpt-5", "approval_policy": "never", "effort": "high"}))
            .response("message", user_msg(&["fix the build"]))
            .item(json!({"type": "UserMessage", "id": "um", "content": [
                {"type": "text", "text": "fix the build", "text_elements": []},
                {"type": "local_image", "path": "/tmp/a.png"},
            ]}))
            .event("user_message", json!({"message": "fix the build"}))
            .response("reasoning", json!({"id": "rs", "summary": [], "encrypted_content": "gAAA"}))
            .item(json!({"type": "Reasoning", "id": "rs", "summary_text": [], "raw_content": []}))
            .item(json!({"type": "Reasoning", "id": "rs2", "summary_text": ["Looking"], "raw_content": []}))
            .response("message", json!({"id": "am", "role": "assistant", "content": [{"type": "output_text", "text": "On it."}]}))
            .item(json!({"type": "AgentMessage", "id": "am", "content": [{"type": "Text", "text": "On it."}], "phase": "commentary"}))
            .event("agent_message", json!({"message": "On it."}))
            .response("custom_tool_call", json!({"id": "ct", "call_id": "call_1", "name": "exec", "input": "await tools.exec_command({cmd: 'make'})"}))
            .item(json!({"type": "CommandExecution", "id": "exec-1", "command": ["/bin/zsh", "-lc", "make"], "cwd": "/work",
                "parsed_cmd": [{"type": "unknown", "cmd": "make"}], "status": "failed", "aggregated_output": "boom\n", "exit_code": 2}))
            .item(json!({"type": "CommandExecution", "id": "exec-2", "command": ["/bin/zsh", "-lc", "cat go.mod"], "cwd": "/work",
                "parsed_cmd": [{"type": "read", "cmd": "cat go.mod", "name": "go.mod", "path": "go.mod"}], "status": "completed",
                "aggregated_output": "module x\n", "exit_code": 0}))
            .response("custom_tool_call_output", json!({"id": "co", "call_id": "call_1", "output": [{"type": "input_text", "text": "boom"}]}))
            .add("token_usage_record", json!({"usage": {"input_tokens": 100}}))
            .event("token_count", token_count(100, 40, 10, 5, 12.0))
            .item(json!({"type": "FileChange", "id": "fc", "status": "completed", "changes": {
                "/work/b.go": {"type": "update", "unified_diff": "@@ -1,2 +1,2 @@\n-old\n+new\n ctx\n", "move_path": null},
                "/work/a.go": {"type": "add", "content": "package a\n"},
            }}))
            .item(json!({"type": "Extension", "id": "ws", "kind": "web.search", "query": "go generics",
                "action": {"type": "search", "query": "go generics"}, "results": []}))
            .item(json!({"type": "SubAgentActivity", "id": "sa", "kind": "started"}))
            .event("token_count", token_count(200, 150, 20, 0, 12.0))
            .event("task_complete", json!({"turn_id": "u", "duration_ms": 1500, "error": null}))
    }

    pub(crate) fn item_turn_want() -> Vec<&'static str> {
        vec![
            "init thread-1 gpt-5 /work never 0.1.0",
            "start SessionStart <permissions>x</permissions>",
            "start SessionStart <environment_context>\n  <cwd>/work</cwd>\n</environment_context>",
            "user() text:fix the build; image:/tmp/a.png",
            "assistant(gpt-5) thinking:Looking",
            "assistant(gpt-5) text:On it.",
            "assistant(gpt-5) call:exec-1 shell shell make",
            r#"user() result:exec-1 "boom\n" err=true exit=2"#,
            "assistant(gpt-5) call:exec-2 shell read cat go.modgo.mod",
            r#"user() result:exec-2 "module x\n" err=false exit=0"#,
            "quota pro 5h 12% 10:00:20",
            "assistant(gpt-5) call:fc apply_patch edit /work/a.go",
            r#"user() result:fc "" err=false patch:/work/a.go@1+1:+package a patch:/work/b.go@1+2:-old|+new| ctx"#,
            "assistant(gpt-5) call:ws web_search websearch go generics",
            r#"user() result:ws "" err=false"#,
            "other item/SubAgentActivity",
            "context 220/1000",
            r#"end done "" in=110 out=30 cache=190 reason=5 1.5s"#,
        ]
    }

    #[test]
    fn history_items() {
        let got = history(&item_turn(Rollout::new(t0())), 0);
        assert_eq!(got, item_turn_want());
    }

    // A turn without items, as older and guardian rollouts are, is read
    // from its response_items.
    #[test]
    fn history_responses() {
        let r = Rollout::new(t0())
            .add("session_meta", meta("thread-2", "exec".into()))
            .add("session_meta", meta("parent", "exec".into()))
            .add("turn_context", json!({"model": "gpt-5-mini", "approval_policy": "on-request"}))
            .event("task_started", json!({}))
            .response("message", user_msg(&["# AGENTS.md instructions for /work\n\n<INSTRUCTIONS>be nice</INSTRUCTIONS>", "<environment_context>x</environment_context>"]))
            .response("message", json!({"id": "u1", "role": "user", "content": [
                {"type": "input_text", "text": "<image name=[Image #1]>"},
                {"type": "input_image", "image_url": "data:image/png;base64,iVBORw=="},
                {"type": "input_text", "text": "</image>"},
                {"type": "input_text", "text": "what is <b>this</b>?"},
            ]}))
            .event("user_message", json!({"message": "what is this?"}))
            .response("reasoning", json!({"id": "r", "summary": [{"type": "summary_text", "text": "Hmm"}], "encrypted_content": "x"}))
            .response("function_call", json!({"id": "f", "call_id": "c1", "name": "exec_command", "arguments": r#"{"cmd":"ls","workdir":"/work"}"#}))
            .response("function_call_output", json!({"id": "fo", "call_id": "c1", "output": "a.go"}))
            .response("custom_tool_call", json!({"id": "p", "call_id": "c2", "name": "apply_patch", "input": "*** Begin Patch\n*** Add File: x.go\n+package x\n*** End Patch"}))
            .response("custom_tool_call_output", json!({"id": "po", "call_id": "c2", "output": [{"type": "input_text", "text": "Done"}]}))
            .response("message", json!({"id": "a", "role": "assistant", "content": [{"type": "output_text", "text": "An image."}]}))
            .event("agent_message", json!({"message": "An image."}))
            .event("turn_aborted", json!({"reason": "interrupted", "duration_ms": 2000}));
        assert_eq!(
            history(&r, 0),
            vec![
                "init thread-2 gpt-5-mini /work on-request 0.1.0",
                "start SessionStart # AGENTS.md instructions for /work\n\n<INSTRUCTIONS>be nice</INSTRUCTIONS>",
                "start SessionStart <environment_context>x</environment_context>",
                "user() image:image/png; text:what is <b>this</b>?",
                "assistant(gpt-5-mini) thinking:Hmm",
                "assistant(gpt-5-mini) call:c1 exec_command shell ls",
                r#"user() result:c1 "a.go" err=false"#,
                "assistant(gpt-5-mini) call:c2 apply_patch write x.go",
                r#"user() result:c2 "Done" err=false"#,
                "assistant(gpt-5-mini) text:An image.",
                r#"end interrupted "" in=0 out=0 cache=0 reason=0 2s"#,
            ]
        );
    }

    #[test]
    fn history_ends() {
        let r = Rollout::new(t0())
            .add("session_meta", meta("thread-3", "cli".into()))
            .add("turn_context", json!({"model": "gpt-5"}))
            .event("task_started", json!({}))
            .item(json!({"type": "UserMessage", "id": "u1", "content": [{"type": "text", "text": "one"}]}))
            .event("token_count", token_count(10, 0, 1, 0, 50.0))
            .event("token_count", token_count(10, 0, 1, 0, 50.0)) // the same limits again
            .event("task_complete", json!({"duration_ms": 10, "error": {"message": "limit hit", "codex_error_info": "usage_limit_exceeded"}}))
            .add("compacted", json!({"message": "", "replacement_history": [user_msg(&["old"])],
                "latest_token_usage_record": {"usage": {"total_tokens": 900}}}))
            .item(json!({"type": "ContextCompaction", "id": "cc"}))
            .add("turn_context", json!({"model": "gpt-5.1"}))
            .event("task_started", json!({}))
            .item(json!({"type": "UserMessage", "id": "u2", "content": [{"type": "text", "text": "two"}]}))
            .item(json!({"type": "AgentMessage", "id": "a2", "content": [{"type": "Text", "text": "ok"}]}));
        let all = vec![
            "init thread-3 gpt-5 /work  0.1.0",
            "user() text:one",
            "quota pro 5h 50% 10:00:04",
            "context 11/1000",
            "limited",
            r#"end error "limit hit" in=20 out=2 cache=0 reason=0 10ms"#,
            "compacted 900",
            "user() text:two",
            "assistant(gpt-5.1) text:ok",
        ];
        for (name, before, want) in [
            ("whole", 0, all.clone()),
            ("before the second turn", r.at(9), all[..7].to_vec()),
            ("before the first reply", r.at(4), all[..2].to_vec()),
            ("before anything", t0(), vec![]),
        ] {
            assert_eq!(history(&r, before), want, "{name}");
        }
    }

    // A line of several megabytes, as a command's output can be, is read
    // whole.
    #[test]
    fn history_long_line() {
        let big = "0123456789abcdef".repeat((3 << 20) / 16 * 2); // 6 MB
        let r = Rollout::new(t0())
            .add("session_meta", meta("thread-4", "cli".into()))
            .item(json!({"type": "CommandExecution", "id": "e", "command": ["sh", "-c", "yes"], "status": "completed",
                "aggregated_output": big, "exit_code": 0}))
            .item(json!({"type": "AgentMessage", "id": "a", "content": [{"type": "Text", "text": "after"}]}));
        let evs = read_rollout(r.text().as_bytes(), 0).unwrap();
        assert_eq!(evs.len(), 4, "{:?}", describe_all(&evs));
        let Event::Message(m) = &evs[2] else {
            panic!("not a message")
        };
        assert_eq!(m.parts[0].output.as_ref().unwrap().text.len(), big.len());
        assert_eq!(describe(&evs[3]), "assistant() text:after");
    }

    #[test]
    fn injected_context() {
        for (text, want) in [
            (
                "<environment_context>\n  <cwd>/x</cwd>\n</environment_context>",
                true,
            ),
            ("  <user_instructions>be brief</user_instructions>\n", true),
            (
                "<turn_aborted>\nThe user interrupted.\n</turn_aborted>",
                true,
            ),
            (
                "# AGENTS.md instructions for /x\n\n<INSTRUCTIONS>...</INSTRUCTIONS>",
                true,
            ),
            ("<image name=[Image #1]>", true),
            ("</image>", true),
            ("fix the build", false),
            ("<div> is broken", false),
            ("<b>bold</b> is wrong", false),
            ("what does <br> do", false),
            ("# Plan\n\ndo it", false),
        ] {
            assert_eq!(injected(text), want, "{text:?}");
        }
    }

    #[test]
    fn argvs() {
        for (raw, want) in [
            (r#"["/bin/zsh","-lc","go test ./..."]"#, "go test ./..."),
            (r#"["bash","-c","ls"]"#, "ls"),
            (r#"["git","status"]"#, "git status"),
            (r#""make""#, "make"),
        ] {
            assert_eq!(argv(&parse(raw)), want, "{raw}");
        }
    }

    #[test]
    fn read_call_from_item() {
        let it = json!({"type": "CommandExecution", "id": "e", "command": ["sh", "-lc", "rg foo src"],
            "parsed_cmd": [{"type": "search", "query": "foo", "path": "src"}]});
        let c = call_of(&rollout_thread_item(&it).unwrap(), None).unwrap();
        assert_eq!(
            (c.kind, c.input.pattern.as_str(), c.input.path.as_str()),
            (Kind::Search, "foo", "src")
        );
    }

    // kids_test.go:TestAgentToolsFromRollout
    #[test]
    fn agent_tools_from_rollout() {
        for (name, want) in [
            (
                "send_message",
                r#"SendMessage {"message":"","to":"echoer"}"#,
            ),
            (
                "followup_task",
                r#"SendMessage {"message":"","summary":"a follow-up task","to":"echoer"}"#,
            ),
            ("interrupt_agent", r#"TaskStop {"task_id":"echoer"}"#),
            ("wait_agent", r#"TaskOutput {"task_id":"its subagents"}"#),
            ("list_agents", "ListAgents {}"),
        ] {
            let c = response_call(
                &json!({"type": "function_call", "name": name, "namespace": "collaboration", "call_id": "c",
                "arguments": r#"{"target":"echoer","message":"gAAAABqvggt"}"#}),
            );
            let got = format!("{} {}", c.name, c.raw.as_ref().unwrap().get());
            assert_eq!(got, want, "{name}");
        }
    }

    // children_test.go:TestSpawnAgentChild, its first half.
    #[test]
    fn spawn_agent_names_its_task() {
        let r = Rollout::new(t0())
            .add("session_meta", meta("parent", "cli".into()))
            .response("function_call", json!({"id": "f", "call_id": "c1", "name": "spawn_agent", "namespace": "collaboration",
                "arguments": r#"{"task_name":"trace_bug","model":"gpt-5","message":"gAAAAencrypted"}"#}));
        let evs = read_rollout(r.text().as_bytes(), 0).unwrap();
        let call = evs
            .iter()
            .filter_map(|e| match e {
                Event::Message(m) => m.parts.iter().find_map(|p| p.call.clone()),
                _ => None,
            })
            .next_back()
            .expect("a call");
        assert_eq!(
            (
                call.kind,
                call.input.child.as_str(),
                call.input.description.as_str()
            ),
            (Kind::Subagent, "trace_bug", "trace_bug")
        );
    }

    // shells_test.go:TestBackgroundShellFromRollout
    #[test]
    fn background_shell_from_rollout() {
        let r = Rollout::new(t0())
            .event("task_started", json!({"turn_id": "u1"}))
            .event("task_complete", json!({"turn_id": "u1"}))
            .event("task_started", json!({"turn_id": "u2"}))
            .event("item_completed", json!({"turn_id": "u1", "item": {"type": "CommandExecution", "id": "bg",
                "command": ["/bin/zsh", "-lc", "sleep 25"], "status": "completed", "exit_code": 0}}))
            .event("item_completed", json!({"turn_id": "u2", "item": {"type": "CommandExecution", "id": "fg",
                "command": ["/bin/zsh", "-lc", "ls"], "status": "completed", "exit_code": 0}}))
            .event("task_complete", json!({"turn_id": "u2"}));
        let evs = read_rollout(r.text().as_bytes(), 0).unwrap();
        let bg: std::collections::BTreeMap<String, bool> = evs
            .iter()
            .filter_map(|e| match e {
                Event::Message(m) => m
                    .parts
                    .iter()
                    .find_map(|p| p.call.as_ref().map(|c| (c.id.clone(), c.input.background))),
                _ => None,
            })
            .collect();
        assert_eq!(bg.len(), 2);
        assert!(bg["bg"] && !bg["fg"], "{bg:?}");
    }

    /// Given a slice at a time, a rollout's events are those of a whole
    /// read, each once.
    #[test]
    fn taken_as_read_is_the_whole() {
        let r = item_turn(Rollout::new(t0()));
        let mut rp = Replay::default();
        let mut got = Vec::new();
        for l in &r.lines {
            rp.line(l.as_bytes());
            got.extend(rp.take());
        }
        rp.flush();
        got.extend(rp.take());
        assert_eq!(describe_all(&got), item_turn_want());
    }
}
