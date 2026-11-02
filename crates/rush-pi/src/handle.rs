//! What pi says, line by line, as rush's events. Port of
//! internal/adapters/pi/handle.go:1-302.

use std::time::Instant;

use rush_core::event::{
    Ask, Choice, Compacted, Delta, Event, MessageStart, Other, PartKind, PartStart, Question,
    Status, TaskDone, TaskKind, TaskProgress, TaskStarted, TurnEnd,
};
use rush_core::usage::TokenUsage;
use rush_core::wire::RawJson;
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;

use crate::conn::{CONFIRM_NO, CONFIRM_YES, Shared};
use crate::discover::one_line;
use crate::json::nd;
use crate::messages::{Block, Message, call_of, reason};

/// One event pi writes, with the fields of every type rush reads. Its
/// "message" is an AgentMessage in message events, and text in an
/// extension's dialog.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Line {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    message: Option<Box<RawValue>>,

    /// message_update
    #[serde(rename = "assistantMessageEvent")]
    assistant_message_event: Option<Ame>,

    /// compaction_start, compaction_end
    #[serde(deserialize_with = "nd")]
    reason: String,
    result: Option<CompactResult>,

    /// extension_ui_request
    #[serde(deserialize_with = "nd")]
    id: String,
    #[serde(deserialize_with = "nd")]
    method: String,
    #[serde(deserialize_with = "nd")]
    title: String,
    #[serde(deserialize_with = "nd")]
    options: Vec<String>,

    /// tool_execution_start, _update, _end
    #[serde(rename = "toolCallId", deserialize_with = "nd")]
    tool_call_id: String,
    #[serde(rename = "toolName", deserialize_with = "nd")]
    tool_name: String,
    #[serde(rename = "parentToolCallId", deserialize_with = "nd")]
    parent_tool_call_id: String,
    args: Option<Box<RawValue>>,
    /// Its content and details.
    #[serde(rename = "partialResult")]
    partial_result: Option<Message>,
    #[serde(rename = "isError", deserialize_with = "nd")]
    is_error: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Ame {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    #[serde(rename = "contentIndex", deserialize_with = "nd")]
    content_index: i64,
    #[serde(deserialize_with = "nd")]
    delta: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CompactResult {
    #[serde(rename = "tokensBefore", deserialize_with = "nd")]
    tokens_before: i64,
}

impl Line {
    /// The line's AgentMessage, if it has one.
    fn message(&self) -> Option<Message> {
        let m = self.message.as_ref()?;
        if !m.get().starts_with('{') {
            return None;
        }
        serde_json::from_str(m.get()).ok()
    }

    /// The line's message when it's text.
    fn text(&self) -> String {
        match &self.message {
            Some(m) if m.get().starts_with('"') => {
                serde_json::from_str(m.get()).unwrap_or_default()
            }
            _ => String::new(),
        }
    }
}

fn other(typ: &str, raw: &[u8]) -> Event {
    Event::Other(Other {
        adapter: "pi".into(),
        kind: typ.into(),
        raw: std::str::from_utf8(raw)
            .ok()
            .and_then(|s| RawJson::new(s).ok()),
    })
}

fn first_non_empty(a: &str, b: &str) -> String {
    if a.is_empty() { b } else { a }.to_string()
}

impl Shared {
    /// What pi says, handed on as events.
    pub(crate) fn handle(&self, typ: &str, raw: &[u8]) {
        let evs = self.events(typ, raw);
        if !evs.is_empty() {
            self.emit(evs);
        }
    }

    /// What one line means as events.
    pub(crate) fn events(&self, typ: &str, raw: &[u8]) -> Vec<Event> {
        if matches!(
            typ,
            "agent_settled" | "turn_start" | "turn_end" | "queue_update" | "auto_retry_end"
        ) {
            // What these say comes whole in the message events, or is pi's
            // own bookkeeping.
            return Vec::new();
        }
        let Ok(l) = serde_json::from_slice::<Line>(raw) else {
            return vec![other(typ, raw)];
        };
        match typ {
            "agent_start" => {
                let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
                (s.running, s.began) = (true, Some(Instant::now()));
                s.tokens = TokenUsage::default();
                s.stop.clear();
                s.failed.clear();
                s.last.clear();
                vec![Event::Status(Status {
                    busy: true,
                    ..Status::default()
                })]
            }
            "agent_end" => {
                let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
                let mut end = TurnEnd {
                    reason: reason(&s.stop),
                    err: s.failed.clone(),
                    text: s.last.clone(),
                    cost: s.cost,
                    tokens: s.tokens,
                    turns: 1,
                    ..TurnEnd::default()
                };
                if let Some(b) = s.began {
                    end.duration = b.elapsed();
                }
                (s.running, s.began) = (false, None);
                vec![Event::TurnEnd(end)]
            }
            "message_start" => match l.message() {
                Some(m) if m.role == "assistant" => {
                    let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
                    s.msg = m.id();
                    let model = first_non_empty(&m.model, &s.model.id);
                    vec![Event::MessageStart(MessageStart { id: m.id(), model })]
                }
                _ => Vec::new(),
            },
            "message_update" => update(&l),
            "message_end" => match l.message() {
                Some(m) => self.message_end(&m),
                None => vec![other(typ, raw)],
            },
            "compaction_start" => vec![Event::Status(Status {
                busy: true,
                text: "compacting".into(),
            })],
            "compaction_end" => {
                let Some(r) = &l.result else {
                    return Vec::new(); // aborted, or failed: the compact command says so
                };
                let trigger = if l.reason == "manual" {
                    "manual"
                } else {
                    "auto"
                };
                vec![Event::Compacted(Compacted {
                    trigger: trigger.into(),
                    before: r.tokens_before,
                    ..Compacted::default()
                })]
            }
            "auto_retry_start" => vec![Event::Status(Status {
                busy: true,
                text: "retrying".into(),
            })],
            "extension_ui_request" => self.dialog(&l, raw),
            "tool_execution_start" | "tool_execution_update" | "tool_execution_end" => {
                subagent_task(&l)
            }
            _ => vec![other(typ, raw)],
        }
    }

    /// A whole message. Messages rush sent come back from pi as it takes
    /// them in, and aren't told again: the host showed them as sent.
    fn message_end(&self, m: &Message) -> Vec<Event> {
        let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
        match m.role.as_str() {
            "user" => {
                if !s.sent.is_empty() {
                    s.sent.remove(0);
                    return Vec::new();
                }
                m.events(&format!("pi-user-{}", m.timestamp))
            }
            "assistant" => {
                let id = if s.msg.is_empty() {
                    m.id()
                } else {
                    s.msg.clone()
                };
                s.msg.clear();
                let mut out = m.events(&id);
                if !m.stop_reason.is_empty() {
                    s.stop = m.stop_reason.clone();
                }
                if !m.error_message.is_empty() && m.stop_reason != "aborted" {
                    s.failed = m.error_message.clone();
                }
                let t = m.text();
                let t = t.trim();
                if !t.is_empty() {
                    s.last = t.to_string();
                }
                if let Some(u) = &m.usage {
                    s.tokens.add(u.usage());
                    s.cost += u.cost.total;
                    let n = u.context();
                    if n > 0 {
                        out.push(Event::Context(rush_core::event::Context {
                            tokens: n,
                            window: s.model.context_window,
                        }));
                    }
                }
                if !m.model.is_empty() && m.model != s.model.id && m.provider == s.model.provider {
                    s.model.id = m.model.clone();
                }
                out
            }
            r => m.events(&format!("pi-{r}-{}", m.timestamp)),
        }
    }

    /// An extension asking something. Select and confirm are questions; a
    /// text box rush can't show is cancelled at once, and what needs no
    /// answer is passed on as it is.
    fn dialog(&self, l: &Line, raw: &[u8]) -> Vec<Event> {
        match l.method.as_str() {
            "select" | "confirm" => {
                let text = l.text();
                let mut ask = Ask {
                    text: first_non_empty(&l.title, &text),
                    ..Ask::default()
                };
                let confirm = l.method == "confirm";
                if confirm {
                    if !l.title.is_empty() && !text.is_empty() {
                        (ask.header, ask.text) = (l.title.clone(), text);
                    }
                    ask.options = [CONFIRM_YES, CONFIRM_NO]
                        .into_iter()
                        .map(|o| Choice {
                            label: o.into(),
                            ..Choice::default()
                        })
                        .collect();
                } else {
                    ask.options = l
                        .options
                        .iter()
                        .map(|o| Choice {
                            label: o.clone(),
                            ..Choice::default()
                        })
                        .collect();
                }
                self.st
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .dialogs
                    .insert(l.id.clone(), confirm);
                return vec![Event::Question(Question {
                    id: l.id.clone(),
                    title: l.title.clone(),
                    asks: vec![ask],
                    ..Question::default()
                })];
            }
            "input" | "editor" => {
                if let Some(c) = self.client() {
                    let reply =
                        json!({"type": "extension_ui_response", "id": l.id, "cancelled": true});
                    if let serde_json::Value::Object(m) = reply {
                        let _ = c.post(&m);
                    }
                }
            }
            _ => {}
        }
        vec![other("extension_ui_request", raw)]
    }
}

/// A piece of the assistant message being written.
fn update(l: &Line) -> Vec<Event> {
    let Some(e) = &l.assistant_message_event else {
        return Vec::new();
    };
    let i = e.content_index;
    let start = |kind| vec![Event::PartStart(PartStart { index: i, kind })];
    let delta = |kind| {
        vec![Event::Delta(Delta {
            index: i,
            kind,
            text: e.delta.clone(),
        })]
    };
    match e.typ.as_str() {
        "text_start" => start(PartKind::Text),
        "text_delta" => delta(PartKind::Text),
        "thinking_start" => start(PartKind::Thinking),
        "thinking_delta" => delta(PartKind::Thinking),
        "toolcall_start" => start(PartKind::ToolCall),
        "toolcall_delta" => delta(PartKind::ToolCall),
        _ => Vec::new(),
    }
}

/// The subagent extension's run as a task the turn waits on, as Claude
/// Code tells its subagents'. Other tools' runs show in their calls and
/// results; a tool's own nested calls aren't told.
fn subagent_task(l: &Line) -> Vec<Event> {
    if l.tool_name != "subagent" || !l.parent_tool_call_id.is_empty() {
        return Vec::new();
    }
    match l.typ.as_str() {
        "tool_execution_start" => {
            let c = call_of(&Block {
                id: l.tool_call_id.clone(),
                name: l.tool_name.clone(),
                arguments: l.args.clone(),
                ..Block::default()
            });
            vec![Event::TaskStarted(TaskStarted {
                id: l.tool_call_id.clone(),
                call_id: l.tool_call_id.clone(),
                kind: TaskKind::Subagent,
                label: c.input.description,
                agent: c.input.agent,
                ..TaskStarted::default()
            })]
        }
        "tool_execution_update" => match &l.partial_result {
            Some(r) => vec![Event::TaskProgress(subagent_progress(&l.tool_call_id, r))],
            None => Vec::new(),
        },
        "tool_execution_end" => vec![Event::TaskDone(TaskDone {
            id: l.tool_call_id.clone(),
            call_id: l.tool_call_id.clone(),
            status: if l.is_error { "failed" } else { "completed" }.into(),
            ..TaskDone::default()
        })],
        _ => Vec::new(),
    }
}

/// What a subagent run has done so far, from the details of its partial
/// result: each agent's messages and usage.
fn subagent_progress(id: &str, r: &Message) -> TaskProgress {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct D {
        #[serde(deserialize_with = "nd")]
        results: Vec<Res>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Res {
        #[serde(deserialize_with = "nd")]
        messages: Vec<Message>,
        #[serde(deserialize_with = "nd")]
        usage: U,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct U {
        #[serde(deserialize_with = "nd")]
        input: i64,
        #[serde(deserialize_with = "nd")]
        output: i64,
    }
    let mut p = TaskProgress {
        id: id.into(),
        summary: one_line(&r.text()),
        ..TaskProgress::default()
    };
    let d: D = r
        .details
        .as_ref()
        .and_then(|d| serde_json::from_str::<Option<D>>(d.get()).ok().flatten())
        .unwrap_or_default();
    for res in d.results {
        p.tokens += res.usage.input + res.usage.output;
        for m in res.messages {
            for b in m.blocks() {
                if b.typ == "toolCall" {
                    p.tool_uses += 1;
                    p.last_tool = b.name;
                }
            }
        }
    }
    p
}
