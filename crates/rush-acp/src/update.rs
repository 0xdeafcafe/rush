//! What the agent says of its session, as rush's events: messages streamed
//! a piece at a time, tool calls and their updates, plans, modes, usage,
//! and the background tasks its calls start. Port of
//! internal/adapters/acp/update.go:1-615.

use std::sync::LazyLock;

use base64::Engine;
use regex::Regex;
use rush_core::event::{self, Event, PartKind, TaskKind};
use rush_core::tool::{self, Kind};
use rush_core::wire::RawJson;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::protocol::{
    Chunk, ConfigOption, ContentBlock, Location, PlanEntry, ToolCall, ToolContent,
};
use crate::session::{Inner, State, lock, model, set_config};

/// A tool call as the updates so far describe it.
#[derive(Clone, Default)]
pub(crate) struct CallState {
    pub c: tool::Call,
    pub acp_kind: String,
    pub content: Vec<ToolContent>,
    pub locs: Vec<Location>,
    /// Its ToolCall message has gone out.
    pub sent: bool,
    /// And its ToolResult.
    pub done: bool,
    /// What Vibe says the tool does.
    pub effect: String,
    /// The agent gave c.name.
    pub named: bool,
    /// The task it started, once it has.
    pub task: String,
    /// And that task has ended.
    pub ended: bool,
}

impl Inner {
    /// Takes the agent's notifications.
    pub(crate) fn notify(&self, method: &str, params: Option<&RawValue>) {
        let Some(params) = params else {
            return;
        };
        match method {
            "session/update" => {
                #[derive(Deserialize)]
                struct N<'a> {
                    #[serde(borrow)]
                    update: &'a RawValue,
                }
                if let Ok(n) = serde_json::from_str::<N>(params.get()) {
                    self.update(n.update);
                }
            }
            "$/cancel_request" => {
                #[derive(Deserialize)]
                struct P<'a> {
                    #[serde(rename = "requestId", borrow)]
                    request_id: &'a RawValue,
                }
                if let Ok(p) = serde_json::from_str::<P>(params.get()) {
                    self.withdraw(p.request_id);
                }
            }
            _ => {}
        }
    }

    /// Turns one session/update into events.
    fn update(&self, raw: &RawValue) {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct U {
            #[serde(rename = "sessionUpdate")]
            session_update: String,
        }
        let Ok(u) = serde_json::from_str::<U>(raw.get()) else {
            return;
        };
        let mut st = lock(&self.mu);
        match u.session_update.as_str() {
            k @ ("agent_message_chunk" | "agent_thought_chunk" | "user_message_chunk") => {
                let Ok(c) = serde_json::from_str::<Chunk>(raw.get()) else {
                    return;
                };
                if k == "user_message_chunk" {
                    self.task_notices(&mut st, &c.content.text);
                }
                let (role, kind) = match k {
                    "agent_thought_chunk" => ("assistant", PartKind::Thinking),
                    "user_message_chunk" => ("user", PartKind::Text),
                    _ => ("assistant", PartKind::Text),
                };
                self.chunk(&mut st, role, kind, &c.message_id, c.content);
            }
            "tool_call" | "tool_call_update" => {
                if let Ok(tc) = serde_json::from_str::<ToolCall>(raw.get()) {
                    self.track(&mut st, tc);
                }
            }
            "plan" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    entries: Vec<PlanEntry>,
                }
                let Ok(p) = serde_json::from_str::<P>(raw.get()) else {
                    return;
                };
                let todos = p
                    .entries
                    .into_iter()
                    .map(|e| tool::TodoItem {
                        label: e.content,
                        status: e.status,
                        ..tool::TodoItem::default()
                    })
                    .collect();
                self.emit(Event::Plan(event::Plan { todos }));
            }
            "available_commands_update" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct C {
                    name: String,
                }
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    #[serde(rename = "availableCommands")]
                    available_commands: Vec<C>,
                }
                let Ok(p) = serde_json::from_str::<P>(raw.get()) else {
                    return;
                };
                st.commands = p.available_commands.into_iter().map(|c| c.name).collect();
                self.emit_init(&st);
            }
            "current_mode_update" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    #[serde(rename = "currentModeId")]
                    current_mode_id: String,
                }
                if let Ok(p) = serde_json::from_str::<P>(raw.get())
                    && p.current_mode_id != st.mode
                {
                    st.mode = p.current_mode_id;
                    self.emit_init(&st);
                }
            }
            "config_option_update" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    #[serde(rename = "configOptions")]
                    config_options: Option<Vec<ConfigOption>>,
                }
                if let Ok(p) = serde_json::from_str::<P>(raw.get()) {
                    set_config(&mut st, p.config_options);
                    self.emit_init(&st);
                }
            }
            "usage_update" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Cost {
                    amount: f64,
                    currency: String,
                }
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct P {
                    used: i64,
                    size: i64,
                    cost: Option<Cost>,
                }
                let Ok(p) = serde_json::from_str::<P>(raw.get()) else {
                    return;
                };
                if let Some(c) = p.cost
                    && (c.currency.is_empty() || c.currency.eq_ignore_ascii_case("USD"))
                {
                    st.cost = c.amount;
                }
                self.emit(Event::Context(event::Context {
                    tokens: p.used,
                    window: p.size,
                }));
            }
            other => {
                self.emit(Event::Other(event::Other {
                    adapter: st.o.adapter.clone(),
                    kind: other.to_string(),
                    raw: RawJson::new(raw.get()).ok(),
                }));
            }
        }
    }

    /// Adds a piece of a message. ACP streams messages and never sends them
    /// whole, so a message is kept open and goes out whole, as a Message,
    /// when another begins or a tool call or the turn ends. An assistant's
    /// is streamed as it comes as well.
    fn chunk(&self, st: &mut State, role: &str, kind: PartKind, id: &str, b: ContentBlock) {
        if let Some(m) = &st.open
            && (m.role != role || (!id.is_empty() && !m.id.is_empty() && id != m.id))
        {
            self.flush(st);
        }
        let stream = role == "assistant";
        if st.open.is_none() {
            let m = event::Message {
                role: role.into(),
                id: id.into(),
                model: model(st),
                ..event::Message::default()
            };
            if stream {
                self.emit(Event::MessageStart(event::MessageStart {
                    id: id.into(),
                    model: m.model.clone(),
                }));
            }
            st.open = Some(m);
        }
        let m = st.open.as_mut().expect("opened above");
        match b.kind.as_str() {
            "text" => {
                let mut n = m.parts.len();
                if n == 0 || m.parts[n - 1].kind != kind {
                    m.parts.push(event::Part {
                        kind,
                        ..event::Part::default()
                    });
                    if stream {
                        self.emit(Event::PartStart(event::PartStart {
                            index: n as i64,
                            kind,
                        }));
                    }
                    n += 1;
                }
                m.parts[n - 1].text += &b.text;
                if stream {
                    self.emit(Event::Delta(event::Delta {
                        index: n as i64 - 1,
                        kind,
                        text: b.text,
                    }));
                }
            }
            "image" => {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(b.data.as_bytes())
                    .unwrap_or_default();
                m.parts.push(event::Part {
                    kind: PartKind::Image,
                    image: Some(Box::new(event::ImageData {
                        media_type: b.mime_type,
                        data,
                        ..event::ImageData::default()
                    })),
                    ..event::Part::default()
                });
            }
            "resource_link" => m.parts.push(event::Part {
                kind,
                text: b.uri,
                ..event::Part::default()
            }),
            _ => {}
        }
    }

    /// Sends the open message whole.
    pub(crate) fn flush(&self, st: &mut State) {
        if let Some(m) = st.open.take()
            && !m.parts.is_empty()
        {
            self.emit(Event::Message(m));
        }
    }

    /// Folds a tool call or an update to one into what's known of it, and
    /// says so: the call once, and its result once it has finished. Says
    /// which call it was.
    pub(crate) fn track(&self, st: &mut State, tc: ToolCall) -> String {
        let id = tc.tool_call_id.clone();
        let finds_children = st.o.finds_children;
        let model_now = model(st);
        let mut c = st.calls.remove(&id).unwrap_or_else(|| CallState {
            c: tool::Call {
                id: id.clone(),
                ..tool::Call::default()
            },
            ..CallState::default()
        });
        if let Some(t) = &tc.title {
            c.c.title = t.clone();
        }
        if let Some(n) = &tc.name {
            c.c.name = n.clone();
            c.named = true;
        }
        if let Some(k) = &tc.kind {
            c.acp_kind = k.clone();
        }
        if let Some(m) = &tc.meta
            && !m.effect_kind.is_empty()
        {
            c.effect = m.effect_kind.clone();
        }
        let status = tc.status.clone().unwrap_or_default();
        match tc.raw_input.as_deref().filter(|r| r.get() != "null") {
            Some(raw) => c.c.raw = Some(merge_raw(c.c.raw.as_ref(), raw)),
            None => {
                if c.c.raw.is_none() && status != "completed" && status != "failed" {
                    // Kimi streams a call's input as its text, each update
                    // the whole so far, never rawInput: the last whole one is
                    // the input.
                    if let Some(raw) = tc.content.as_deref().and_then(input_text) {
                        c.c.raw = Some(raw);
                    }
                }
            }
        }
        c.c.kind = call_kind(&c.acp_kind, &c.effect, c.c.raw.as_ref());
        if let Some(content) = &tc.content {
            c.content = content.clone();
        }
        if let Some(locs) = &tc.locations {
            c.locs = locs.clone();
        }
        if !c.named {
            // A call of no kind ACP knows goes by its title: Kimi's is the
            // tool's name.
            c.c.name = c.acp_kind.clone();
            if (c.acp_kind == "other" || c.acp_kind.is_empty()) && !c.c.title.is_empty() {
                c.c.name = c.c.title.clone();
            }
        }
        c.c.input = read_input(c.c.kind, c.c.raw.as_ref(), &c.locs, &c.content);
        if c.c.kind == Kind::Subagent && finds_children {
            c.c.input.child = c.c.id.clone();
        }
        if c.sent
            && (tc.title.is_some()
                || tc.kind.is_some()
                || tc.raw_input.is_some()
                || tc.content.is_some()
                || tc.locations.is_some())
        {
            self.emit(Event::CallUpdated(Box::new(event::CallUpdated {
                call: c.c.clone(),
            })));
        }
        if !c.sent {
            c.sent = true;
            self.flush(st);
            self.emit(Event::Message(event::Message {
                role: "assistant".into(),
                model: model_now,
                parts: vec![event::Part {
                    kind: PartKind::ToolCall,
                    call: Some(Box::new(c.c.clone())),
                    ..event::Part::default()
                }],
                ..event::Message::default()
            }));
        }
        // A subagent the turn waits on is a task from when its prompt is
        // known until its call ends.
        let inp = &c.c.input;
        if c.c.kind == Kind::Subagent
            && !inp.background
            && c.task.is_empty()
            && !c.done
            && !inp.prompt.is_empty()
        {
            c.task = c.c.id.clone();
            let first = inp.prompt.split('\n').next().unwrap_or_default();
            self.emit(Event::TaskStarted(event::TaskStarted {
                id: c.task.clone(),
                call_id: c.c.id.clone(),
                kind: TaskKind::Subagent,
                label: first_of(&[&inp.description, first]),
                agent: inp.agent.clone(),
                background: false,
            }));
        }
        let finished = !c.done && (status == "completed" || status == "failed");
        let mut text = String::new();
        if finished {
            c.done = true;
            let out = read_output(
                &c.c.id,
                status == "failed",
                &c.content,
                tc.raw_output.as_deref(),
            );
            text = out.text.clone();
            self.emit(Event::Message(event::Message {
                role: "user".into(),
                parts: vec![event::Part {
                    kind: PartKind::ToolResult,
                    output: Some(Box::new(out)),
                    ..event::Part::default()
                }],
                ..event::Message::default()
            }));
            if !c.task.is_empty() && !c.c.input.background {
                c.ended = true;
                self.emit(Event::TaskDone(event::TaskDone {
                    id: c.task.clone(),
                    call_id: c.c.id.clone(),
                    status: status.clone(),
                    ..event::TaskDone::default()
                }));
            }
        }
        st.calls.insert(id.clone(), c);
        if finished {
            self.backgrounded(st, &id, &text);
            self.task_notices(st, &text);
        }
        id
    }

    /// Starts the task a background call says it started.
    fn backgrounded(&self, st: &mut State, id: &str, result: &str) {
        let Some(c) = st.calls.get_mut(id) else {
            return;
        };
        // A background question is no task to watch: it ends when
        // answered.
        if !c.c.input.background || !c.task.is_empty() || c.c.kind == Kind::Question {
            return;
        }
        let Some(m) = STARTED.captures(result) else {
            return;
        };
        c.task = format!(
            "{}{}",
            m.get(1).map_or("", |g| g.as_str()),
            m.get(2).map_or("", |g| g.as_str())
        );
        let inp = &c.c.input;
        let ev = event::TaskStarted {
            id: c.task.clone(),
            call_id: c.c.id.clone(),
            kind: task_kind(c),
            label: first_of(&[&inp.description, &inp.command]),
            agent: inp.agent.clone(),
            background: true,
        };
        self.emit(Event::TaskStarted(ev));
        self.emit_background(st);
    }

    /// Ends the background tasks text says have ended.
    pub(crate) fn task_notices(&self, st: &mut State, text: &str) {
        for m in ENDED.captures_iter(text) {
            let g = |i| m.get(i).map_or("", |g| g.as_str());
            let (mut id, mut status) = (g(2).to_string(), g(1).to_string());
            if id.is_empty() {
                id = g(3).to_string();
                status = if g(4) != "0" { "failed" } else { "completed" }.into();
            }
            let mut ended = Vec::new();
            for c in st.calls.values_mut() {
                if c.task == id && c.c.input.background && !c.ended {
                    c.ended = true;
                    ended.push(c.c.id.clone());
                }
            }
            for call_id in ended {
                self.emit(Event::TaskDone(event::TaskDone {
                    id: id.clone(),
                    call_id,
                    status: status.clone(),
                    ..event::TaskDone::default()
                }));
                self.emit_background(st);
            }
        }
    }

    /// Lists the background tasks still running.
    pub(crate) fn emit_background(&self, st: &State) {
        let mut list: Vec<event::BackgroundTask> = st
            .calls
            .values()
            .filter(|c| !c.task.is_empty() && c.c.input.background && !c.ended)
            .map(|c| event::BackgroundTask {
                id: c.task.clone(),
                kind: task_kind(c),
                label: first_of(&[&c.c.input.description, &c.c.input.command]),
                ..event::BackgroundTask::default()
            })
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        self.emit(Event::Background(event::Background { tasks: list }));
    }
}

/// Finds the task a background call's result says it started: Kimi's
/// "task_id: bash-1", Copilot's "started in background with shellId: 0".
static STARTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^task_id: (\S+)$|started in background with shellId: ([^>\s]+)")
        .expect("valid")
});

/// Finds the tasks a message says ended: Kimi's notification of one, or
/// what Copilot says of a shell it reads back.
static ENDED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"<notification [^>]*type="task\.(\w+)"[^>]*source_id="([^"]+)"|<shellId: ([^>\s]+) completed with exit code (\d+)>"#,
    )
    .expect("valid")
});

fn task_kind(c: &CallState) -> TaskKind {
    if c.c.kind == Kind::Subagent {
        TaskKind::Subagent
    } else {
        TaskKind::Shell
    }
}

pub(crate) fn first_of(ss: &[&str]) -> String {
    ss.iter()
        .find(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// What a call does: ACP's kind, or a subagent, which ACP has no kind for.
/// Agents send one as other or think: Vibe says so in _meta, the rest give
/// it a prompt and what it's for or what agent runs it.
pub(crate) fn call_kind(acp_kind: &str, effect: &str, raw: Option<&RawJson>) -> Kind {
    let k = kind_of(acp_kind);
    if k != Kind::Other && k != Kind::Think {
        return k;
    }
    if effect == "subagent" {
        return Kind::Subagent;
    }
    let m = object(raw.map(|r| r.0.get()));
    if matches!(m.get("prompt"), Some(Value::String(_)))
        && (present(&m, "description") || present(&m, "subagent_type") || present(&m, "agent_type"))
    {
        return Kind::Subagent;
    }
    if matches!(m.get("questions"), Some(Value::Array(_))) {
        return Kind::Question; // Kimi's AskUserQuestion
    }
    k
}

/// Go's `m[key] != nil`: there, and not null.
fn present(m: &Map<String, Value>, key: &str) -> bool {
    m.get(key).is_some_and(|v| !v.is_null())
}

/// raw read as an object; empty when it isn't one.
fn object(raw: Option<&str>) -> Map<String, Value> {
    raw.and_then(|r| serde_json::from_str::<Map<String, Value>>(r).ok())
        .unwrap_or_default()
}

/// A call's input with a later one's over it: Copilot asks permission with
/// less of the input than the call had.
fn merge_raw(old: Option<&RawJson>, nw: &RawValue) -> RawJson {
    let fresh = || RawJson(nw.to_owned());
    let Some(old) = old else {
        return fresh();
    };
    let (Ok(mut a), Ok(b)) = (
        serde_json::from_str::<Map<String, Value>>(old.0.get()),
        serde_json::from_str::<Map<String, Value>>(nw.get()),
    ) else {
        return fresh();
    };
    for (k, v) in b {
        a.insert(k, v);
    }
    RawJson::new(&Value::Object(a).to_string()).unwrap_or_else(|_| fresh())
}

/// A call's input carried as its text content, as Kimi streams it: one
/// text block that is the input JSON, whole. None for anything else:
/// prose, pieces, results.
fn input_text(content: &[ToolContent]) -> Option<RawJson> {
    if content.len() != 1 || content[0].kind != "content" || content[0].content.kind != "text" {
        return None;
    }
    let t = content[0].content.text.trim();
    if !t.starts_with('{') {
        return None;
    }
    RawJson::new(t).ok()
}

/// ACP's ToolKind as rush's.
pub(crate) fn kind_of(k: &str) -> Kind {
    match k {
        "read" => Kind::Read,
        "edit" => Kind::Edit,
        "delete" => Kind::Delete,
        "move" => Kind::Move,
        "search" => Kind::Search,
        "execute" => Kind::Shell,
        "think" => Kind::Think,
        "fetch" => Kind::Fetch,
        "switch_mode" => Kind::PlanMode,
        _ => Kind::Other,
    }
}

/// What it can of a call's input. ACP leaves rawInput to each agent, so it
/// looks for the names agents commonly use; locations and diffs, which ACP
/// does define, fill in the rest.
pub(crate) fn read_input(
    k: Kind,
    raw: Option<&RawJson>,
    locs: &[Location],
    content: &[ToolContent],
) -> tool::Input {
    let m = object(raw.map(|r| r.0.get()));
    let s = |keys: &[&str]| -> String {
        for key in keys {
            if let Some(Value::String(v)) = m.get(*key)
                && !v.is_empty()
            {
                return v.clone();
            }
        }
        String::new()
    };
    let mut inp = tool::Input {
        path: s(&[
            "path",
            "file_path",
            "filePath",
            "absolute_path",
            "filename",
            "file",
        ]),
        pattern: s(&["pattern", "regex", "glob"]),
        url: s(&["url"]),
        description: s(&["description"]),
        cwd: s(&["cwd", "workdir"]),
        prompt: s(&["prompt"]),
        ..tool::Input::default()
    };
    match m.get("command") {
        Some(Value::String(v)) => inp.command = v.clone(),
        Some(Value::Array(v)) => {
            inp.command = v
                .iter()
                .filter_map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => {}
    }
    if inp.command.is_empty() {
        inp.command = s(&["cmd"]);
    }
    // Kimi's run_in_background, Copilot's async mode.
    let mode = m.get("mode").and_then(Value::as_str);
    let bg = m.get("run_in_background") == Some(&Value::Bool(true))
        || mode == Some("async")
        || mode == Some("background");
    match k {
        Kind::Shell => inp.background = bg,
        // Kimi's: answered after its call returns.
        Kind::Question => inp.background = m.get("background") == Some(&Value::Bool(true)),
        Kind::Subagent => {
            inp.background = bg;
            if inp.prompt.is_empty() {
                inp.prompt = s(&["task"]); // Vibe's
            }
            inp.agent = s(&["subagent_type", "agent_type", "subagent_name", "agent"]);
        }
        Kind::Search => {
            if inp.pattern.is_empty() {
                inp.pattern = s(&["query"]);
            }
        }
        Kind::Move => {
            inp.to = s(&["new_path", "newPath", "destination", "to"]);
            if inp.path.is_empty() {
                inp.path = s(&["old_path", "oldPath", "source", "from"]);
            }
        }
        Kind::Edit => {
            let (old, nw) = (
                s(&["old_string", "oldString", "old_str"]),
                s(&["new_string", "newString", "new_str"]),
            );
            if !old.is_empty() || !nw.is_empty() {
                let all = m.get("replace_all") == Some(&Value::Bool(true));
                inp.edits = vec![tool::Replace {
                    old,
                    new: nw,
                    all,
                    ..tool::Replace::default()
                }];
            }
            inp.content = s(&["content"]);
        }
        _ => {}
    }
    if inp.path.is_empty()
        && let Some(l) = locs.first()
    {
        inp.path = l.path.clone();
    }
    let mut diffs = Vec::new();
    for c in content.iter().filter(|c| c.kind == "diff") {
        if inp.path.is_empty() {
            inp.path = c.path.clone();
        }
        let mut r = tool::Replace {
            old: c.old_text.clone().unwrap_or_default(),
            new: c.new_text.clone(),
            ..tool::Replace::default()
        };
        if c.path != inp.path {
            r.path = c.path.clone();
        }
        diffs.push(r);
    }
    if !diffs.is_empty() {
        inp.edits = diffs;
    }
    inp
}

/// A finished call's result.
pub(crate) fn read_output(
    id: &str,
    failed: bool,
    content: &[ToolContent],
    raw: Option<&RawValue>,
) -> tool::Output {
    let mut out = tool::Output {
        call_id: id.into(),
        is_error: failed,
        ..tool::Output::default()
    };
    if let Some(r) = raw.filter(|r| r.get() != "null") {
        out.raw = Some(RawJson(r.to_owned()));
    }
    let mut texts = Vec::new();
    for c in content {
        match c.kind.as_str() {
            "content" if c.content.kind == "text" => texts.push(c.content.text.as_str()),
            "diff" => {
                out.patches.push(patch(c));
                out.created = out.created || c.old_text.is_none();
            }
            _ => {}
        }
    }
    out.text = texts.join("\n");
    let Some(raw) = &out.raw else {
        return out;
    };
    if !out.text.is_empty() {
        return out;
    }
    match serde_json::from_str::<Value>(raw.0.get()) {
        Ok(Value::String(s)) => out.text = s,
        Ok(Value::Object(v)) => {
            let get = |keys: &[&str]| -> String {
                keys.iter()
                    .find_map(|k| v.get(*k).and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_string()
            };
            out.stdout = get(&["stdout"]);
            out.stderr = get(&["stderr"]);
            out.text = get(&["output", "content", "result", "response", "error"]);
            if out.text.is_empty() {
                out.text = format!("{}\n{}", out.stdout, out.stderr).trim().to_string();
            }
            for k in ["exit_code", "exitCode", "code"] {
                if let Some(n) = v.get(k).and_then(Value::as_f64) {
                    out.exit = Some(n as i64);
                    break;
                }
            }
        }
        _ => {}
    }
    out
}

/// A diff as one hunk: the lines that changed, without the ones both sides
/// share at either end.
pub(crate) fn patch(d: &ToolContent) -> tool::Patch {
    fn split(s: &str) -> Vec<&str> {
        if s.is_empty() {
            return Vec::new();
        }
        s.strip_suffix('\n').unwrap_or(s).split('\n').collect()
    }
    let old = split(d.old_text.as_deref().unwrap_or_default());
    let nw = split(&d.new_text);
    let mut pre = 0;
    while pre < old.len() && pre < nw.len() && old[pre] == nw[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < old.len() - pre
        && suf < nw.len() - pre
        && old[old.len() - 1 - suf] == nw[nw.len() - 1 - suf]
    {
        suf += 1;
    }
    let mut p = tool::Patch {
        path: d.path.clone(),
        old_lines: (old.len() - pre - suf) as i64,
        new_lines: (nw.len() - pre - suf) as i64,
        ..tool::Patch::default()
    };
    p.old_start = pre as i64 + i64::from(p.old_lines > 0);
    p.new_start = pre as i64 + i64::from(p.new_lines > 0);
    for l in &old[pre..old.len() - suf] {
        p.lines.push(format!("-{l}"));
    }
    for l in &nw[pre..nw.len() - suf] {
        p.lines.push(format!("+{l}"));
    }
    p
}
