//! One line of `claude -p --output-format stream-json` read as Claude
//! Code's own event. Port of internal/adapters/claude/headless/event.go
//! (Event and its kinds, Decode, Commands, Patches) and question.go.

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::decode::{RawUsage, decode_message, tokens};
use crate::json::{Str, nd};
use crate::message::{Message, Role, Usage};
use rush_core::wire::RawJson;

/// One decoded line of Claude Code's output.
#[derive(Clone, Debug)]
pub enum Event {
    /// Arrives once, when the session is ready.
    Init(Init),
    /// Opens an assistant message that Deltas then fill in.
    MessageStart(MessageStart),
    /// A piece of text or thinking streamed while a message is written.
    Delta(Delta),
    /// Opens a content block: "thinking", "text" or "tool_use". Thinking
    /// often streams no text at all (only a signature), so this is how to
    /// know it's happening.
    BlockStart(BlockStart),
    /// A whole assistant or user turn. Tool results arrive as user
    /// messages.
    Message(Message),
    /// Asks the host whether a tool may run. Answer it with Session::allow
    /// or Session::deny.
    PermissionRequest(PermissionRequest),
    /// Withdraws a PermissionRequest, by its id, e.g. after an interrupt.
    PermissionCancelled(String),
    /// A tool call refused without asking the host.
    PermissionDenied(PermissionDenied),
    /// The session's own busy marker, e.g. "requesting".
    Status(String),
    /// Where Claude Code compacted the conversation.
    Compact(Compact),
    /// Ends a turn.
    Result(TurnResult),
    /// The account's usage windows.
    RateLimit(RateLimit),
    /// Answers a control request the host sent.
    ControlReply(ControlReply),
    /// One MCP message for a server the host runs in-process (one named in
    /// Session::initialize). Answer it with Session::reply_mcp.
    McpRequest(McpRequest),
    TaskStarted(TaskStarted),
    TaskUpdated(TaskUpdated),
    TaskProgress(TaskProgress),
    TaskDone(TaskDone),
    /// Every task now running in the background, each time the list
    /// changes.
    BackgroundTasks(Vec<BackgroundTask>),
    /// Anything not decoded above, kept whole so nothing is lost.
    Other(Other),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Init {
    #[serde(rename = "session_id", deserialize_with = "nd")]
    pub session_id: String,
    #[serde(deserialize_with = "nd")]
    pub model: String,
    #[serde(deserialize_with = "nd")]
    pub cwd: String,
    #[serde(rename = "permissionMode", deserialize_with = "nd")]
    pub permission_mode: String,
    #[serde(rename = "claude_code_version", deserialize_with = "nd")]
    pub version: String,
    #[serde(deserialize_with = "nd")]
    pub tools: Vec<String>,
    #[serde(rename = "slash_commands", deserialize_with = "nd")]
    pub slash_commands: Vec<String>,
    #[serde(rename = "mcp_servers", deserialize_with = "nd")]
    pub mcp_servers: Vec<McpServer>,
    /// Where its API key comes from: "none" when it runs on a Claude
    /// sign-in instead.
    #[serde(rename = "apiKeySource", deserialize_with = "nd")]
    pub api_key_source: String,
}

/// One MCP server as Claude Code last saw it: connected, failed,
/// needs-auth, pending.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct McpServer {
    #[serde(deserialize_with = "nd")]
    pub name: String,
    #[serde(deserialize_with = "nd")]
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageStart {
    pub message_id: String,
    pub model: String,
}

/// Index is the content block it belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delta {
    pub index: i64,
    pub thinking: bool,
    /// A tool call's input, streamed as JSON.
    pub input: bool,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockStart {
    pub index: i64,
    pub kind: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PermissionRequest {
    pub id: String,
    pub tool: String,
    pub input: Option<RawJson>,
    pub description: String,
    pub reason: String,
    pub reason_type: String,
    pub tool_use_id: String,
    pub blocked_path: String,
    pub suggestions: Option<RawJson>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PermissionDenied {
    #[serde(rename = "tool_name", deserialize_with = "nd")]
    pub tool: String,
    #[serde(deserialize_with = "nd")]
    pub tool_use_id: String,
    #[serde(rename = "decision_reason", deserialize_with = "nd")]
    pub reason: String,
}

/// What set a compaction off (manual or auto) and the context before and
/// after.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compact {
    pub trigger: String,
    pub pre_tokens: i64,
    pub post_tokens: i64,
}

/// How a turn ended (Go's Result).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnResult {
    /// success, error_max_turns, error_during_execution, ...
    pub subtype: String,
    pub is_error: bool,
    pub text: String,
    pub stop_reason: String,
    pub session_id: String,
    pub cost_usd: f64,
    pub duration_ms: i64,
    pub num_turns: i64,
    pub usage: Usage,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RateLimit {
    pub status: String,
    /// Whether the request was paid for as extra usage: the plan's
    /// allowance is spent.
    pub overage: bool,
    pub raw: Option<RawJson>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlReply {
    pub id: String,
    pub error: String,
    pub body: Option<RawJson>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct McpRequest {
    pub id: String,
    pub server: String,
    pub message: Option<RawJson>,
}

/// Claude Code started a task: a Bash command, a subagent, a monitor or a
/// workflow. backgrounded is false for one the turn waits on (it can
/// still be moved to the background, or stopped, by its id).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TaskStarted {
    #[serde(rename = "task_id", deserialize_with = "nd")]
    pub id: String,
    #[serde(deserialize_with = "nd")]
    pub tool_use_id: String,
    /// local_bash, local_agent, monitor_mcp, local_workflow, ...
    #[serde(rename = "task_type", deserialize_with = "nd")]
    pub kind: String,
    #[serde(deserialize_with = "nd")]
    pub description: String,
    #[serde(deserialize_with = "nd")]
    pub subagent_type: String,
    #[serde(rename = "workflow_name", deserialize_with = "nd")]
    pub workflow: String,
    #[serde(rename = "is_backgrounded", deserialize_with = "nd")]
    pub backgrounded: bool,
}

/// What changed about a task: status when it ends (completed, failed,
/// killed); backgrounded when it's moved to the background.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskUpdated {
    pub id: String,
    pub status: String,
    pub description: String,
    pub backgrounded: Option<bool>,
    pub error: String,
}

/// A subagent or workflow task's running numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskProgress {
    pub id: String,
    pub description: String,
    pub summary: String,
    pub last_tool: String,
    pub tokens: i64,
    pub tool_uses: i64,
}

/// A task finished (completed, failed, stopped), and where its output is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TaskDone {
    #[serde(rename = "task_id", deserialize_with = "nd")]
    pub id: String,
    #[serde(deserialize_with = "nd")]
    pub tool_use_id: String,
    #[serde(deserialize_with = "nd")]
    pub status: String,
    #[serde(deserialize_with = "nd")]
    pub output_file: String,
    #[serde(deserialize_with = "nd")]
    pub summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BackgroundTask {
    #[serde(rename = "task_id", deserialize_with = "nd")]
    pub id: String,
    #[serde(rename = "task_type", deserialize_with = "nd")]
    pub kind: String,
    #[serde(deserialize_with = "nd")]
    pub description: String,
    /// The subagent's that started it, if one did.
    #[serde(rename = "parent_task_id", deserialize_with = "nd")]
    pub parent_task: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Other {
    pub kind: String,
    pub subtype: String,
    pub raw: Option<RawJson>,
}

/// Why a line isn't an event.
#[derive(Debug)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

impl From<serde_json::Error> for DecodeError {
    fn from(e: serde_json::Error) -> DecodeError {
        DecodeError(e.to_string())
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Envelope<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(borrow)]
    subtype: Str<'a>,
    #[serde(borrow)]
    parent_tool_use_id: Str<'a>,
    #[serde(borrow)]
    request_id: Str<'a>,
    #[serde(borrow)]
    request: Option<&'a RawValue>,
    #[serde(borrow)]
    response: Option<&'a RawValue>,
    #[serde(borrow)]
    message: Option<&'a RawValue>,
    #[serde(borrow)]
    event: Option<&'a RawValue>,
    #[serde(borrow)]
    tool_use_result: Option<&'a RawValue>,
}

fn raw(r: Option<&RawValue>) -> Option<RawJson> {
    r.map(|r| RawJson(r.to_owned()))
}

fn raw_str(s: &str) -> Option<RawJson> {
    RawJson::new(s).ok()
}

/// Turns one output line into an Event.
pub fn decode(line: &[u8]) -> Result<Event, DecodeError> {
    let line = std::str::from_utf8(line).map_err(|e| DecodeError(e.to_string()))?;
    let line = line.trim_end_matches(['\n', '\r']);
    let mut ev = decode_line(line)?;
    if let Event::Other(o) = &mut ev
        && o.raw.is_none()
    {
        // Only what isn't decoded keeps a copy of its line.
        o.raw = raw_str(line);
    }
    Ok(ev)
}

fn decode_line(line: &str) -> Result<Event, DecodeError> {
    let e: Envelope = serde_json::from_str(line)?;
    let other = Other {
        kind: e.typ.to_string(),
        subtype: e.subtype.to_string(),
        raw: None,
    };
    match &*e.typ {
        "system" => decode_system(&e.subtype, line, other),
        "stream_event" => decode_stream(e.event, other),
        "assistant" | "user" => decode_envelope_message(&e),
        "control_request" => decode_control(&e, other),
        "control_cancel_request" => Ok(Event::PermissionCancelled(e.request_id.to_string())),
        "control_response" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct R<'a> {
                #[serde(deserialize_with = "nd")]
                request_id: String,
                #[serde(deserialize_with = "nd")]
                error: String,
                #[serde(borrow)]
                response: Option<&'a RawValue>,
            }
            let r: R = serde_json::from_str(e.response.map_or("null", RawValue::get))?;
            Ok(Event::ControlReply(ControlReply {
                id: r.request_id,
                error: r.error,
                body: raw(r.response),
            }))
        }
        "result" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct R {
                #[serde(deserialize_with = "nd")]
                subtype: String,
                #[serde(deserialize_with = "nd")]
                is_error: bool,
                #[serde(deserialize_with = "nd")]
                result: String,
                #[serde(deserialize_with = "nd")]
                stop_reason: String,
                #[serde(deserialize_with = "nd")]
                session_id: String,
                #[serde(deserialize_with = "nd")]
                total_cost_usd: f64,
                #[serde(deserialize_with = "nd")]
                duration_ms: i64,
                #[serde(deserialize_with = "nd")]
                num_turns: i64,
                #[serde(deserialize_with = "nd")]
                usage: RawUsage,
            }
            let r: R = serde_json::from_str(line)?;
            Ok(Event::Result(TurnResult {
                subtype: r.subtype,
                is_error: r.is_error,
                text: r.result,
                stop_reason: r.stop_reason,
                session_id: r.session_id,
                cost_usd: r.total_cost_usd,
                duration_ms: r.duration_ms,
                num_turns: r.num_turns,
                usage: tokens(&r.usage),
            }))
        }
        "rate_limit_event" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct R<'a> {
                #[serde(borrow)]
                rate_limit_info: Option<&'a RawValue>,
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct S {
                #[serde(deserialize_with = "nd")]
                status: String,
                #[serde(rename = "isUsingOverage", deserialize_with = "nd")]
                overage: bool,
            }
            let r: R = serde_json::from_str(line).unwrap_or_default();
            let s: S = r
                .rate_limit_info
                .and_then(|i| serde_json::from_str(i.get()).ok())
                .unwrap_or_default();
            Ok(Event::RateLimit(RateLimit {
                status: s.status,
                overage: s.overage,
                raw: raw(r.rate_limit_info),
            }))
        }
        _ => Ok(Event::Other(other)),
    }
}

fn decode_system(subtype: &str, line: &str, other: Other) -> Result<Event, DecodeError> {
    Ok(match subtype {
        "init" => Event::Init(serde_json::from_str(line)?),
        "status" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct R {
                #[serde(deserialize_with = "nd")]
                status: String,
            }
            let r: R = serde_json::from_str(line).unwrap_or_default();
            Event::Status(r.status)
        }
        "compact_boundary" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct R {
                #[serde(deserialize_with = "nd")]
                compact_metadata: Meta,
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Meta {
                #[serde(deserialize_with = "nd")]
                trigger: String,
                #[serde(deserialize_with = "nd")]
                pre_tokens: i64,
            }
            let r: R = serde_json::from_str(line).unwrap_or_default();
            Event::Compact(Compact {
                trigger: r.compact_metadata.trigger,
                pre_tokens: r.compact_metadata.pre_tokens,
                post_tokens: 0,
            })
        }
        "permission_denied" => {
            Event::PermissionDenied(serde_json::from_str(line).unwrap_or_default())
        }
        "task_started" => Event::TaskStarted(serde_json::from_str(line)?),
        "task_updated" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct R {
                #[serde(deserialize_with = "nd")]
                task_id: String,
                #[serde(deserialize_with = "nd")]
                patch: Patch,
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Patch {
                #[serde(deserialize_with = "nd")]
                status: String,
                #[serde(deserialize_with = "nd")]
                description: String,
                is_backgrounded: Option<bool>,
                #[serde(deserialize_with = "nd")]
                error: String,
            }
            let r: R = serde_json::from_str(line)?;
            Event::TaskUpdated(TaskUpdated {
                id: r.task_id,
                status: r.patch.status,
                description: r.patch.description,
                backgrounded: r.patch.is_backgrounded,
                error: r.patch.error,
            })
        }
        "task_progress" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct R {
                #[serde(deserialize_with = "nd")]
                task_id: String,
                #[serde(deserialize_with = "nd")]
                description: String,
                #[serde(deserialize_with = "nd")]
                summary: String,
                #[serde(deserialize_with = "nd")]
                last_tool_name: String,
                #[serde(deserialize_with = "nd")]
                usage: U,
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct U {
                #[serde(deserialize_with = "nd")]
                total_tokens: i64,
                #[serde(deserialize_with = "nd")]
                tool_uses: i64,
            }
            let r: R = serde_json::from_str(line)?;
            Event::TaskProgress(TaskProgress {
                id: r.task_id,
                description: r.description,
                summary: r.summary,
                last_tool: r.last_tool_name,
                tokens: r.usage.total_tokens,
                tool_uses: r.usage.tool_uses,
            })
        }
        "task_notification" => Event::TaskDone(serde_json::from_str(line)?),
        "background_tasks_changed" => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct R {
                #[serde(deserialize_with = "nd")]
                tasks: Vec<BackgroundTask>,
            }
            let r: R = serde_json::from_str(line)?;
            Event::BackgroundTasks(r.tasks)
        }
        _ => Event::Other(other),
    })
}

fn decode_stream(raw_ev: Option<&RawValue>, other: Other) -> Result<Event, DecodeError> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Ev {
        #[serde(rename = "type", deserialize_with = "nd")]
        typ: String,
        #[serde(deserialize_with = "nd")]
        index: i64,
        #[serde(deserialize_with = "nd")]
        message: M,
        #[serde(deserialize_with = "nd")]
        delta: D,
        #[serde(deserialize_with = "nd")]
        content_block: B,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        id: String,
        #[serde(deserialize_with = "nd")]
        model: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct D {
        #[serde(rename = "type", deserialize_with = "nd")]
        typ: String,
        #[serde(deserialize_with = "nd")]
        text: String,
        #[serde(deserialize_with = "nd")]
        thinking: String,
        #[serde(deserialize_with = "nd")]
        partial_json: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct B {
        #[serde(rename = "type", deserialize_with = "nd")]
        typ: String,
    }
    let ev: Ev = serde_json::from_str(raw_ev.map_or("null", RawValue::get))?;
    Ok(match ev.typ.as_str() {
        "message_start" => Event::MessageStart(MessageStart {
            message_id: ev.message.id,
            model: ev.message.model,
        }),
        "content_block_start" => Event::BlockStart(BlockStart {
            index: ev.index,
            kind: ev.content_block.typ,
        }),
        "content_block_delta" => {
            let (thinking, input, text) = match ev.delta.typ.as_str() {
                "text_delta" => (false, false, ev.delta.text),
                "thinking_delta" => (true, false, ev.delta.thinking),
                // A tool call being written: counted, not shown.
                "input_json_delta" => (false, true, ev.delta.partial_json),
                _ => return Ok(Event::Other(other)),
            };
            Event::Delta(Delta {
                index: ev.index,
                thinking,
                input,
                text,
            })
        }
        _ => Event::Other(other),
    })
}

fn decode_envelope_message(e: &Envelope) -> Result<Event, DecodeError> {
    decode_message_parts(&e.typ, e.message, e.tool_use_result)
        .map(|mut m| {
            m.parent_tool_use_id = e.parent_tool_use_id.to_string();
            Event::Message(m)
        })
        .ok_or_else(|| DecodeError("not a message".into()))
}

/// An assistant or user message from its parts, for a reader (like a
/// transcript's) that has already split the line up.
pub fn decode_message_parts(
    typ: &str,
    message: Option<&RawValue>,
    tool_use_result: Option<&RawValue>,
) -> Option<Message> {
    let mut m = decode_message(message?.get())?;
    // Its own role, as the line's type says it.
    m.role = if typ == "user" {
        Role::User
    } else {
        Role::Assistant
    };
    m.tool_result = tool_use_result.map(ToOwned::to_owned);
    Some(m)
}

fn decode_control(e: &Envelope, other: Other) -> Result<Event, DecodeError> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R<'a> {
        #[serde(deserialize_with = "nd")]
        subtype: String,
        #[serde(deserialize_with = "nd")]
        tool_name: String,
        #[serde(borrow)]
        input: Option<&'a RawValue>,
        #[serde(deserialize_with = "nd")]
        description: String,
        #[serde(deserialize_with = "nd")]
        decision_reason: String,
        #[serde(deserialize_with = "nd")]
        decision_reason_type: String,
        #[serde(deserialize_with = "nd")]
        tool_use_id: String,
        #[serde(deserialize_with = "nd")]
        blocked_path: String,
        #[serde(borrow)]
        permission_suggestions: Option<&'a RawValue>,
        #[serde(deserialize_with = "nd")]
        server_name: String,
        #[serde(borrow)]
        message: Option<&'a RawValue>,
    }
    let r: R = serde_json::from_str(e.request.map_or("null", RawValue::get))?;
    if r.subtype == "mcp_message" {
        return Ok(Event::McpRequest(McpRequest {
            id: e.request_id.to_string(),
            server: r.server_name,
            message: raw(r.message),
        }));
    }
    if r.subtype != "can_use_tool" {
        return Ok(Event::Other(other));
    }
    Ok(Event::PermissionRequest(PermissionRequest {
        id: e.request_id.to_string(),
        tool: r.tool_name,
        input: raw(r.input),
        description: r.description,
        reason: r.decision_reason,
        reason_type: r.decision_reason_type,
        tool_use_id: r.tool_use_id,
        blocked_path: r.blocked_path,
        suggestions: raw(r.permission_suggestions),
    }))
}

/// A slash command the session accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Command {
    #[serde(deserialize_with = "nd")]
    pub name: String,
    #[serde(deserialize_with = "nd")]
    pub description: String,
    #[serde(rename = "argumentHint", deserialize_with = "nd")]
    pub argument_hint: String,
    #[serde(deserialize_with = "nd")]
    pub aliases: Vec<String>,
}

/// The slash commands in the reply to Session::initialize.
pub fn commands(reply: &ControlReply) -> Vec<Command> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        #[serde(deserialize_with = "nd")]
        commands: Vec<Command>,
    }
    reply
        .body
        .as_ref()
        .and_then(|b| serde_json::from_str::<R>(b.get()).ok())
        .unwrap_or_default()
        .commands
}

/// One hunk of an edit, as Claude Code reports it in an Edit or Write
/// result's structuredPatch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Patch {
    #[serde(rename = "oldStart", deserialize_with = "nd")]
    pub old_start: i64,
    #[serde(rename = "oldLines", deserialize_with = "nd")]
    pub old_lines: i64,
    #[serde(rename = "newStart", deserialize_with = "nd")]
    pub new_start: i64,
    #[serde(rename = "newLines", deserialize_with = "nd")]
    pub new_lines: i64,
    /// Each prefixed ' ', '-' or '+'.
    #[serde(deserialize_with = "nd")]
    pub lines: Vec<String>,
}

/// The hunks of a tool result, if it has any.
pub fn patches(tool_result: &RawValue) -> Vec<Patch> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        #[serde(rename = "structuredPatch", deserialize_with = "nd")]
        p: Vec<Patch>,
    }
    serde_json::from_str::<R>(tool_result.get())
        .unwrap_or_default()
        .p
}

/// One of the questions Claude asks with the AskUserQuestion tool.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Question {
    #[serde(deserialize_with = "nd")]
    pub question: String,
    #[serde(deserialize_with = "nd")]
    pub header: String,
    #[serde(rename = "multiSelect", deserialize_with = "nd")]
    pub multi_select: bool,
    #[serde(deserialize_with = "nd")]
    pub options: Vec<QuestionOption>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct QuestionOption {
    #[serde(deserialize_with = "nd")]
    pub label: String,
    #[serde(deserialize_with = "nd")]
    pub description: String,
    #[serde(deserialize_with = "nd")]
    pub preview: String,
}

impl PermissionRequest {
    /// What an AskUserQuestion request asks: its title and questions.
    pub fn questions(&self) -> (String, Vec<Question>) {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct In {
            #[serde(deserialize_with = "nd")]
            title: String,
            #[serde(deserialize_with = "nd")]
            questions: Vec<Question>,
        }
        let i: In = self
            .input
            .as_ref()
            .and_then(|r| serde_json::from_str(r.get()).ok())
            .unwrap_or_default();
        (i.title, i.questions)
    }

    /// The tool input that answers it: Claude Code takes the answers keyed
    /// by question text, with the preview of each chosen option beside it.
    /// Questions left unanswered go without one.
    pub fn answer_input(
        &self,
        qs: &[Question],
        answers: &std::collections::BTreeMap<String, String>,
    ) -> RawJson {
        let mut input: serde_json::Map<String, serde_json::Value> = self
            .input
            .as_ref()
            .and_then(|r| serde_json::from_str(r.get()).ok())
            .unwrap_or_default();
        input.insert("answers".into(), serde_json::json!(answers));
        let mut notes = serde_json::Map::new();
        for q in qs {
            for o in &q.options {
                if !o.preview.is_empty() && answers.get(&q.question) == Some(&o.label) {
                    notes.insert(
                        q.question.clone(),
                        serde_json::json!({ "preview": o.preview }),
                    );
                }
            }
        }
        if !notes.is_empty() {
            input.insert("annotations".into(), serde_json::Value::Object(notes));
        }
        RawJson::new(&serde_json::Value::Object(input).to_string()).expect("JSON it wrote")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::message::BlockKind;

    // Lines as Claude Code 2.1.280 writes them, trimmed of fields we ignore
    // (headless_test.go:16-28).
    pub const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/work","session_id":"s-1","tools":["Bash","Read"],"model":"claude-haiku-4-5-20251001","permissionMode":"default","slash_commands":["compact"],"claude_code_version":"2.1.280"}"#;
    pub const START: &str = r#"{"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-haiku-4-5-20251001","id":"msg_1","type":"message","role":"assistant","content":[]}},"session_id":"s-1"}"#;
    pub const DELTA: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hel"}},"session_id":"s-1"}"#;
    pub const THINK: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}},"session_id":"s-1"}"#;
    pub const TOOL_USE: &str = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo hi","description":"Say hi"}}],"usage":{"input_tokens":10,"output_tokens":4,"cache_read_input_tokens":13689,"cache_creation_input_tokens":7787}},"parent_tool_use_id":null,"session_id":"s-1","uuid":"u-1"}"#;
    pub const ASK: &str = r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"echo hi","description":"Say hi"},"description":"Say hi","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"echo hi"}],"behavior":"allow","destination":"localSettings"}],"decision_reason":"This command requires approval","decision_reason_type":"other","tool_use_id":"toolu_1"}}"#;
    pub const TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":[{"type":"text","text":"hi"}],"is_error":false}]},"parent_tool_use_id":null,"session_id":"s-1","uuid":"u-2"}"#;
    pub const DENIED: &str = r#"{"type":"system","subtype":"permission_denied","tool_name":"Bash","tool_use_id":"toolu_2","decision_reason_type":"safetyCheck","decision_reason":"sensitive file","session_id":"s-1"}"#;
    pub const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":6900,"num_turns":2,"result":"Done.","stop_reason":"end_turn","session_id":"s-1","total_cost_usd":0.0247,"usage":{"input_tokens":18,"output_tokens":289}}"#;
    pub const CANCEL: &str = r#"{"type":"control_cancel_request","request_id":"req-1"}"#;
    pub const HOOK: &str = r#"{"type":"system","subtype":"hook_started","hook_name":"SessionStart:startup","session_id":"s-1"}"#;

    pub fn dec(line: &str) -> Event {
        decode(line.as_bytes()).unwrap_or_else(|e| panic!("decode {line}: {e}"))
    }

    #[test]
    fn decodes_each_kind() {
        let Event::Init(i) = dec(INIT) else { panic!() };
        assert_eq!(
            (
                i.session_id.as_str(),
                i.model.as_str(),
                i.version.as_str(),
                i.tools.len()
            ),
            ("s-1", "claude-haiku-4-5-20251001", "2.1.280", 2)
        );
        let Event::MessageStart(s) = dec(START) else {
            panic!()
        };
        assert_eq!(s.message_id, "msg_1");
        let Event::Delta(d) = dec(DELTA) else {
            panic!()
        };
        assert_eq!((d.text.as_str(), d.index, d.thinking), ("Hel", 1, false));
        let Event::Delta(d) = dec(THINK) else {
            panic!()
        };
        assert_eq!((d.text.as_str(), d.thinking), ("hmm", true));
        let Event::Message(m) = dec(TOOL_USE) else {
            panic!()
        };
        assert_eq!(m.role, Role::Assistant);
        assert_eq!(m.blocks.len(), 1);
        assert_eq!(m.blocks[0].name, "Bash");
        assert_eq!(m.usage.unwrap().cache_read, 13689);
        let Event::PermissionRequest(a) = dec(ASK) else {
            panic!()
        };
        assert_eq!(
            (
                a.id.as_str(),
                a.tool.as_str(),
                a.tool_use_id.as_str(),
                a.reason.as_str()
            ),
            ("req-1", "Bash", "toolu_1", "This command requires approval")
        );
        assert!(a.suggestions.is_some());
        let Event::Message(r) = dec(TOOL_RESULT) else {
            panic!()
        };
        let b = &r.blocks[0];
        assert_eq!(
            (b.kind, b.text.as_str(), b.tool_use_id.as_str()),
            (BlockKind::ToolResult, "hi", "toolu_1")
        );
        let Event::PermissionDenied(d) = dec(DENIED) else {
            panic!()
        };
        assert_eq!(
            (d.tool.as_str(), d.reason.as_str()),
            ("Bash", "sensitive file")
        );
        let Event::Result(r) = dec(RESULT) else {
            panic!()
        };
        assert_eq!(
            (r.text.as_str(), r.cost_usd, r.num_turns),
            ("Done.", 0.0247, 2)
        );
        let Event::PermissionCancelled(id) = dec(CANCEL) else {
            panic!()
        };
        assert_eq!(id, "req-1");
        let Event::Other(o) = dec(HOOK) else { panic!() };
        assert_eq!(o.subtype, "hook_started");
        assert_eq!(o.raw.unwrap().get(), HOOK, "kept whole");
    }

    #[test]
    fn decodes_an_mcp_message() {
        let Event::McpRequest(m) = dec(
            r#"{"type":"control_request","request_id":"r9","request":{"subtype":"mcp_message","server_name":"rush","message":{"jsonrpc":"2.0","id":1,"method":"tools/list"}}}"#,
        ) else {
            panic!()
        };
        assert_eq!((m.id.as_str(), m.server.as_str()), ("r9", "rush"));
        assert!(m.message.unwrap().get().contains("tools/list"));
    }

    #[test]
    fn not_a_line() {
        for l in ["", "nope", "[1]"] {
            assert!(decode(l.as_bytes()).is_err(), "{l}");
        }
    }

    #[test]
    fn commands_and_patches() {
        let r = ControlReply {
            id: "rush-1".into(),
            error: String::new(),
            body: RawJson::new(r#"{"commands":[{"name":"review","description":"Review a PR","argumentHint":"<pr>","aliases":null}]}"#).ok(),
        };
        let c = commands(&r);
        assert_eq!(c.len(), 1);
        assert_eq!(
            (c[0].name.as_str(), c[0].argument_hint.as_str()),
            ("review", "<pr>")
        );
        let p = patches(&RawJson::new(r#"{"structuredPatch":[{"oldStart":1,"oldLines":2,"newStart":1,"newLines":3,"lines":[" a","-b","+c"]}]}"#).unwrap().0);
        assert_eq!(p.len(), 1);
        assert_eq!(
            (p[0].old_lines, p[0].new_lines, p[0].lines.len()),
            (2, 3, 3)
        );
    }

    #[test]
    fn answers_a_question() {
        let r = PermissionRequest {
            input: RawJson::new(r##"{"questions":[{"question":"Which?","header":"Pick","options":[{"label":"A","preview":"# a"},{"label":"B"}]},{"question":"Also?","options":[{"label":"Y","preview":"y"}]}],"title":"T"}"##).ok(),
            ..PermissionRequest::default()
        };
        let (title, qs) = r.questions();
        assert_eq!((title.as_str(), qs.len()), ("T", 2));
        let answers = [("Which?".to_string(), "A".to_string())].into();
        let got = r.answer_input(&qs, &answers);
        let v: serde_json::Value = serde_json::from_str(got.get()).unwrap();
        assert_eq!(v["answers"]["Which?"], "A");
        assert_eq!(v["annotations"]["Which?"]["preview"], "# a");
        assert!(v["annotations"].get("Also?").is_none(), "unanswered");
        assert_eq!(v["title"], "T", "the rest of its input stays");
    }
}
