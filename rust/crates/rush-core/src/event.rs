//! What a running session says, whichever agent runs it: the one stream
//! the host keeps, replays and sends to everything that draws a session.
//! Adapters turn their agent's protocol into these; a transcript read back
//! from disk turns into the same ones. After Go's internal/agent/event:
//! event.go, exchange.go and codec.go (the wire: one line of JSON per
//! event, `{"t":name,"e":fields}`, read and written the same as Go's).

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

use crate::time::Ms;
use crate::tool;
use crate::usage::{self, TokenUsage};
use crate::wire::{self, RawJson};

/// One thing a session said or did.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Exchange(Box<Exchange>),
    Init(Box<Init>),
    MessageStart(MessageStart),
    PartStart(PartStart),
    Delta(Delta),
    Message(Message),
    CallUpdated(Box<CallUpdated>),
    Approval(Box<Approval>),
    ApprovalCancelled(ApprovalCancelled),
    Denied(Denied),
    Question(Question),
    Status(Status),
    TurnEnd(TurnEnd),
    Compacted(Compacted),
    /// A fresh reading of the account's limits.
    Quota(usage::Quota),
    Limited(Limited),
    Billing(Billing),
    Context(Context),
    StartNotice(StartNotice),
    /// Not on the wire: Go's codec gives it no name.
    Retry(Retry),
    TaskStarted(TaskStarted),
    TaskUpdated(TaskUpdated),
    TaskProgress(TaskProgress),
    TaskDone(TaskDone),
    Plan(Plan),
    Commands(Commands),
    Background(Background),
    Other(Other),
}

/// A permission mode a running harness offers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionMode {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Arrives once the session is ready.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Init {
    /// Stable permission mode ID, when distinct from its display name.
    #[serde(rename = "ModeID")]
    pub mode_id: String,
    /// Choices reported by the running harness.
    #[serde(with = "wire::nullvec")]
    pub modes: Vec<PermissionMode>,
    #[serde(rename = "SessionID")]
    pub session_id: String,
    pub model: String,
    pub cwd: String,
    /// Its permission mode, in the agent's words.
    pub mode: String,
    /// How hard it thinks, when the agent says.
    pub effort: String,
    /// The agent's.
    pub version: String,
    #[serde(with = "wire::nullvec")]
    pub tools: Vec<String>,
    /// Slash commands it takes, by name: Commands has more.
    #[serde(with = "wire::nullvec")]
    pub commands: Vec<String>,
    #[serde(rename = "MCP", with = "wire::nullvec")]
    pub mcp: Vec<McpServer>,
}

/// One MCP server as the agent last saw it: connected, failed, needs-auth,
/// pending.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServer {
    pub name: String,
    pub status: String,
}

/// What a part of a message is. On the wire it is its number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PartKind {
    #[default]
    Text,
    Thinking,
    ToolCall,
    ToolResult,
    Image,
}

/// Opens an assistant message that Deltas then fill in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MessageStart {
    #[serde(rename = "ID")]
    pub id: String,
    pub model: String,
}

/// Opens part index of the message being written. Thinking often streams
/// nothing readable, so this is how to know it's happening.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PartStart {
    pub index: i64,
    pub kind: PartKind,
}

/// A piece of part index as it's written: text, thinking, or a tool call's
/// input as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Delta {
    pub index: i64,
    pub kind: PartKind,
    pub text: String,
}

/// A whole message. Tool results come back as user messages. Parent is the
/// tool call of the subagent that wrote it, if one did. A user message of
/// text is something you said, unless it's injected: put in the
/// conversation as yours by the agent itself (a task's report, the mark an
/// interrupt leaves, a command's output).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Message {
    /// user, assistant
    pub role: String,
    #[serde(rename = "ID")]
    pub id: String,
    pub model: String,
    pub parent: String,
    #[serde(with = "wire::nullvec")]
    pub parts: Vec<Part>,
    pub tokens: Option<TokenUsage>,
    #[serde(skip_serializing_if = "is_false")]
    pub injected: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One part of a Message.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Part {
    pub kind: PartKind,
    /// Text, Thinking.
    pub text: String,
    /// ToolCall.
    pub call: Option<Box<tool::Call>>,
    /// ToolResult.
    pub output: Option<Box<tool::Output>>,
    /// Image.
    pub image: Option<Box<ImageData>>,
}

/// An image in a message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ImageData {
    pub media_type: String,
    #[serde(with = "wire::bytes")]
    pub data: Vec<u8>,
    /// Where rush keeps a copy, when it does.
    pub path: String,
}

/// A tool call already sent, as it now reads: agents that announce a call
/// before its input is known fill it in later.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CallUpdated {
    pub call: tool::Call,
}

/// Asks whether a tool call may run. Options are the answers the agent
/// takes; the host answers with one of their IDs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Approval {
    #[serde(rename = "ID")]
    pub id: String,
    pub call: tool::Call,
    /// Why the agent asks, when it says.
    pub reason: String,
    /// The file outside the workspace that made it ask, if one did.
    pub path: String,
    #[serde(with = "wire::nullvec")]
    pub options: Vec<ApprovalOption>,
}

/// What an option does. On the wire it is its number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OptionKind {
    #[default]
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

/// One answer to an Approval (Go's event.Option).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ApprovalOption {
    #[serde(rename = "ID")]
    pub id: String,
    pub label: String,
    pub kind: OptionKind,
}

/// Withdraws an Approval or a Question, e.g. after an interrupt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalCancelled {
    #[serde(rename = "ID")]
    pub id: String,
}

/// A tool call refused without asking.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Denied {
    #[serde(rename = "CallID")]
    pub call_id: String,
    pub tool: String,
    pub reason: String,
}

/// Asks the user to choose, as Claude's AskUserQuestion and Codex's
/// request_user_input do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Question {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "CallID")]
    pub call_id: String,
    pub title: String,
    #[serde(with = "wire::nullvec")]
    pub asks: Vec<Ask>,
}

/// One question of a Question.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Ask {
    /// The agent's key for the answer, when it isn't text.
    #[serde(rename = "ID")]
    pub id: String,
    pub header: String,
    pub text: String,
    pub multi: bool,
    #[serde(with = "wire::nullvec")]
    pub options: Vec<Choice>,
}

/// One option of an Ask.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Choice {
    pub label: String,
    pub description: String,
    pub preview: String,
    /// Key and picked are a decisions block's: the option's letter, and
    /// whether the agent picked it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(skip_serializing_if = "is_false")]
    pub picked: bool,
}

/// The session's own busy marker.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Status {
    pub busy: bool,
    /// "requesting", "compacting"
    pub text: String,
}

/// Ends a turn.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TurnEnd {
    /// done, interrupted, max_turns, error, ...
    pub reason: String,
    /// What went wrong, if something did.
    pub err: String,
    /// The turn's last words, as the agent sums it up.
    pub text: String,
    #[serde(with = "wire::float")]
    pub cost: f64,
    pub tokens: TokenUsage,
    #[serde(with = "wire::duration")]
    pub duration: Duration,
    pub turns: i64,
}

/// Where the conversation was compacted: what set it off (manual, auto)
/// and the context before and after.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Compacted {
    pub trigger: String,
    pub before: i64,
    pub after: i64,
}

/// How the session's requests are paid for, once the agent says or it
/// changes: an allowance runs out into extra usage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Billing {
    pub billing: usage::Billing,
}

/// A limit stopped the session until resets_at.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Limited {
    /// The window's ID, if known.
    pub window: String,
    #[serde(with = "wire::time")]
    pub resets_at: Ms,
}

/// A request to the model failed and is tried again after delay: attempt
/// of max, for status (529 overloaded, 429 rate limited, 0 when there was
/// no answer) and err.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Retry {
    pub attempt: i64,
    pub max: i64,
    #[serde(with = "wire::duration")]
    pub delay: Duration,
    pub status: i64,
    pub err: String,
}

/// How much of the model's context the last request used.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Context {
    pub tokens: i64,
    pub window: i64,
}

/// Context the harness gave the agent as its session began. It is shown
/// compactly above the first turn, as Claude Code's start hooks are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct StartNotice {
    pub event: String,
    pub text: String,
}

/// What a task beside the turn is. On the wire it is its number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TaskKind {
    #[default]
    Shell,
    Subagent,
    Monitor,
    Workflow,
    Other,
}

/// The agent started work beside the turn: a shell, a subagent, a monitor.
/// background is false for one the turn waits on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TaskStarted {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "CallID")]
    pub call_id: String,
    pub kind: TaskKind,
    pub label: String,
    /// A subagent's type.
    pub agent: String,
    pub background: bool,
}

/// What changed about a task. status is set when it ends (completed,
/// failed, killed).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TaskUpdated {
    #[serde(rename = "ID")]
    pub id: String,
    pub status: String,
    pub label: String,
    pub background: Option<bool>,
    pub err: String,
}

/// A subagent's running numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TaskProgress {
    #[serde(rename = "ID")]
    pub id: String,
    pub summary: String,
    pub last_tool: String,
    pub tokens: i64,
    pub tool_uses: i64,
}

/// A task finished, and where its output is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TaskDone {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "CallID")]
    pub call_id: String,
    pub status: String,
    pub output_file: String,
    pub summary: String,
}

/// Every task now running beside the turn, each time the list changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Background {
    #[serde(with = "wire::nullvec")]
    pub tasks: Vec<BackgroundTask>,
}

/// One task of a Background list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BackgroundTask {
    #[serde(rename = "ID")]
    pub id: String,
    pub kind: TaskKind,
    /// The agent's own word for it: local_bash, local_agent, ...
    #[serde(rename = "Type")]
    pub kind_word: String,
    pub label: String,
    /// The task of the subagent that started it, if one did.
    pub parent: String,
}

/// The session's slash commands and skills, once it says them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Commands {
    #[serde(with = "wire::nullvec")]
    pub list: Vec<Command>,
}

/// One of a session's slash commands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Command {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    #[serde(with = "wire::nullvec")]
    pub aliases: Vec<String>,
}

/// The agent's whole plan or todo list, each time it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Plan {
    #[serde(with = "wire::nullvec")]
    pub todos: Vec<tool::TodoItem>,
}

/// Anything an adapter doesn't turn into one of the above, kept whole so
/// nothing is lost.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Other {
    pub adapter: String,
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<RawJson>,
}

/// The known identity of a session taking part in an exchange. Empty
/// fields mean unknown; kind is the configured agent or provider.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Peer {
    #[serde(rename = "SessionID")]
    pub session_id: String,
    pub kind: String,
    pub name: String,
}

/// Talk between agents, apart from what you say to them. Both transcripts
/// keep the same ID. direction is sent or received; phase is message,
/// request, or result. Text and attachment references are never
/// shortened.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Exchange {
    #[serde(with = "wire::time")]
    pub at: Ms,
    /// Restored history; never starts a live turn.
    pub archived: bool,
    #[serde(rename = "ID")]
    pub id: String,
    pub direction: String,
    pub phase: String,
    pub text: String,
    #[serde(with = "wire::nullvec")]
    pub images: Vec<String>,
    pub sender: Peer,
    pub receiver: Peer,
}

/// Numbered kinds, as Go's iotas: written as their number, read back from
/// it (one out of range reads as the first).
macro_rules! numbered {
    ($t:ty, [$($v:ident),*]) => {
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_u8(*self as u8)
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<$t, D::Error> {
                const ALL: &[$t] = &[$(<$t>::$v),*];
                let n = Option::<i64>::deserialize(d)?.unwrap_or(0);
                Ok(usize::try_from(n).ok().and_then(|i| ALL.get(i)).copied().unwrap_or_default())
            }
        }
    };
}

numbered!(PartKind, [Text, Thinking, ToolCall, ToolResult, Image]);
numbered!(
    OptionKind,
    [AllowOnce, AllowAlways, RejectOnce, RejectAlways]
);
numbered!(TaskKind, [Shell, Subagent, Monitor, Workflow, Other]);

// ---- The wire (Go's event/codec.go) ----

/// Why an event couldn't be written or read.
#[derive(Debug)]
pub struct CodecError(pub String);

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

impl From<serde_json::Error> for CodecError {
    fn from(e: serde_json::Error) -> CodecError {
        CodecError(e.to_string())
    }
}

/// Every event's name on the wire, in Go's codec order. Retry has none.
pub const NAMES: [&str; 27] = [
    "exchange",
    "init",
    "message_start",
    "part_start",
    "delta",
    "message",
    "call_updated",
    "approval",
    "approval_cancelled",
    "denied",
    "question",
    "status",
    "turn_end",
    "compacted",
    "quota",
    "limited",
    "billing",
    "context",
    "start_notice",
    "task_started",
    "task_updated",
    "task_progress",
    "task_done",
    "plan",
    "background",
    "commands",
    "other",
];

impl Event {
    /// Its name on the wire; None for one Go's codec doesn't name (Retry).
    pub fn name(&self) -> Option<&'static str> {
        Some(match self {
            Event::Exchange(_) => "exchange",
            Event::Init(_) => "init",
            Event::MessageStart(_) => "message_start",
            Event::PartStart(_) => "part_start",
            Event::Delta(_) => "delta",
            Event::Message(_) => "message",
            Event::CallUpdated(_) => "call_updated",
            Event::Approval(_) => "approval",
            Event::ApprovalCancelled(_) => "approval_cancelled",
            Event::Denied(_) => "denied",
            Event::Question(_) => "question",
            Event::Status(_) => "status",
            Event::TurnEnd(_) => "turn_end",
            Event::Compacted(_) => "compacted",
            Event::Quota(_) => "quota",
            Event::Limited(_) => "limited",
            Event::Billing(_) => "billing",
            Event::Context(_) => "context",
            Event::StartNotice(_) => "start_notice",
            Event::Retry(_) => return None,
            Event::TaskStarted(_) => "task_started",
            Event::TaskUpdated(_) => "task_updated",
            Event::TaskProgress(_) => "task_progress",
            Event::TaskDone(_) => "task_done",
            Event::Plan(_) => "plan",
            Event::Commands(_) => "commands",
            Event::Background(_) => "background",
            Event::Other(_) => "other",
        })
    }

    /// Its Go type, for errors that name it as Go's do.
    fn go_type(&self) -> &'static str {
        match self {
            Event::Retry(_) => "event.Retry",
            _ => "event.Event",
        }
    }
}

/// An event as the host sends it: its name, and its fields.
#[derive(Serialize)]
struct WireOut<'a> {
    t: &'a str,
    e: &'a RawValue,
}

#[derive(Deserialize)]
struct WireIn<'a> {
    #[serde(borrow)]
    t: std::borrow::Cow<'a, str>,
    #[serde(borrow)]
    e: Option<&'a RawValue>,
}

/// `ev` as one line of JSON (no newline), which [`unmarshal`] reads back.
pub fn marshal(ev: &Event) -> Result<String, CodecError> {
    let Some(name) = ev.name() else {
        return Err(CodecError(format!("event: {} has no name", ev.go_type())));
    };
    let body = match ev {
        Event::Exchange(e) => serde_json::to_string(e),
        Event::Init(e) => serde_json::to_string(e),
        Event::MessageStart(e) => serde_json::to_string(e),
        Event::PartStart(e) => serde_json::to_string(e),
        Event::Delta(e) => serde_json::to_string(e),
        Event::Message(e) => serde_json::to_string(e),
        Event::CallUpdated(e) => serde_json::to_string(e),
        Event::Approval(e) => serde_json::to_string(e),
        Event::ApprovalCancelled(e) => serde_json::to_string(e),
        Event::Denied(e) => serde_json::to_string(e),
        Event::Question(e) => serde_json::to_string(e),
        Event::Status(e) => serde_json::to_string(e),
        Event::TurnEnd(e) => serde_json::to_string(e),
        Event::Compacted(e) => serde_json::to_string(e),
        Event::Quota(e) => serde_json::to_string(e),
        Event::Limited(e) => serde_json::to_string(e),
        Event::Billing(e) => serde_json::to_string(e),
        Event::Context(e) => serde_json::to_string(e),
        Event::StartNotice(e) => serde_json::to_string(e),
        Event::Retry(e) => serde_json::to_string(e),
        Event::TaskStarted(e) => serde_json::to_string(e),
        Event::TaskUpdated(e) => serde_json::to_string(e),
        Event::TaskProgress(e) => serde_json::to_string(e),
        Event::TaskDone(e) => serde_json::to_string(e),
        Event::Plan(e) => serde_json::to_string(e),
        Event::Commands(e) => serde_json::to_string(e),
        Event::Background(e) => serde_json::to_string(e),
        Event::Other(e) => serde_json::to_string(e),
    }?;
    let body = RawValue::from_string(body)?;
    Ok(serde_json::to_string(&WireOut { t: name, e: &body })?)
}

/// Reads an event [`marshal`] (or Go's event.Marshal) wrote.
pub fn unmarshal(b: &[u8]) -> Result<Event, CodecError> {
    let w: WireIn = serde_json::from_slice(b)?;
    let e = w.e.map_or("null", RawValue::get);
    fn read<T: DeserializeOwned + Default>(e: &str) -> Result<T, CodecError> {
        Ok(serde_json::from_str::<Option<T>>(e)?.unwrap_or_default())
    }
    Ok(match &*w.t {
        "exchange" => Event::Exchange(read(e)?),
        "init" => Event::Init(read(e)?),
        "message_start" => Event::MessageStart(read(e)?),
        "part_start" => Event::PartStart(read(e)?),
        "delta" => Event::Delta(read(e)?),
        "message" => Event::Message(read(e)?),
        "call_updated" => Event::CallUpdated(read(e)?),
        "approval" => Event::Approval(read(e)?),
        "approval_cancelled" => Event::ApprovalCancelled(read(e)?),
        "denied" => Event::Denied(read(e)?),
        "question" => Event::Question(read(e)?),
        "status" => Event::Status(read(e)?),
        "turn_end" => Event::TurnEnd(read(e)?),
        "compacted" => Event::Compacted(read(e)?),
        "quota" => Event::Quota(read(e)?),
        "limited" => Event::Limited(read(e)?),
        "billing" => Event::Billing(read(e)?),
        "context" => Event::Context(read(e)?),
        "start_notice" => Event::StartNotice(read(e)?),
        "task_started" => Event::TaskStarted(read(e)?),
        "task_updated" => Event::TaskUpdated(read(e)?),
        "task_progress" => Event::TaskProgress(read(e)?),
        "task_done" => Event::TaskDone(read(e)?),
        "plan" => Event::Plan(read(e)?),
        "background" => Event::Background(read(e)?),
        "commands" => Event::Commands(read(e)?),
        "other" => Event::Other(read(e)?),
        t => return Err(CodecError(format!("event: unknown {}", go_quote(t)))),
    })
}

/// s quoted as Go's %q does, near enough for an error.
fn go_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_stay_small() {
        // Events queue per session and replay in streams: the big ones
        // (a call with its input) are boxed.
        let n = std::mem::size_of::<Event>();
        assert!(n <= 200, "Event is {n} bytes");
    }

    const AT: Ms = 1_790_510_400_000; // 2026-09-27T12:00:00Z

    fn s(v: &str) -> String {
        v.to_string()
    }

    /// Each event, and the line Go's event.Marshal writes for it.
    fn golden() -> Vec<(Event, &'static str)> {
        let call = tool::Call {
            id: s("c"),
            name: s("Bash"),
            kind: tool::Kind::Shell,
            input: tool::Input {
                command: s("ls"),
                ..Default::default()
            },
            ..Default::default()
        };
        vec![
            (
                Event::Exchange(Box::new(Exchange {
                    id: s("x"),
                    direction: s("sent"),
                    phase: s("message"),
                    text: s("hello"),
                    sender: Peer {
                        session_id: s("a"),
                        kind: s("codex"),
                        ..Default::default()
                    },
                    receiver: Peer {
                        session_id: s("b"),
                        kind: s("kimi"),
                        ..Default::default()
                    },
                    ..Default::default()
                })),
                r#"{"t":"exchange","e":{"At":"0001-01-01T00:00:00Z","Archived":false,"ID":"x","Direction":"sent","Phase":"message","Text":"hello","Images":null,"Sender":{"SessionID":"a","Kind":"codex","Name":""},"Receiver":{"SessionID":"b","Kind":"kimi","Name":""}}}"#,
            ),
            (
                Event::Init(Box::new(Init {
                    session_id: s("s"),
                    model: s("m"),
                    commands: vec![s("compact")],
                    mcp: vec![McpServer {
                        name: s("a"),
                        status: s("connected"),
                    }],
                    ..Default::default()
                })),
                r#"{"t":"init","e":{"ModeID":"","Modes":null,"SessionID":"s","Model":"m","Cwd":"","Mode":"","Effort":"","Version":"","Tools":null,"Commands":["compact"],"MCP":[{"name":"a","status":"connected"}]}}"#,
            ),
            (
                Event::Delta(Delta {
                    index: 1,
                    kind: PartKind::Thinking,
                    text: s("hm"),
                }),
                r#"{"t":"delta","e":{"Index":1,"Kind":1,"Text":"hm"}}"#,
            ),
            (
                Event::Message(Message {
                    role: s("assistant"),
                    parts: vec![
                        Part {
                            kind: PartKind::Text,
                            text: s("hi"),
                            ..Default::default()
                        },
                        Part {
                            kind: PartKind::ToolCall,
                            call: Some(Box::new(call)),
                            ..Default::default()
                        },
                        Part {
                            kind: PartKind::ToolResult,
                            output: Some(Box::new(tool::Output {
                                call_id: s("c"),
                                exit: Some(2),
                                patches: vec![tool::Patch {
                                    lines: vec![s("+a")],
                                    ..Default::default()
                                }],
                                ..Default::default()
                            })),
                            ..Default::default()
                        },
                        Part {
                            kind: PartKind::Image,
                            image: Some(Box::new(ImageData {
                                media_type: s("image/png"),
                                data: b"png!".to_vec(),
                                ..Default::default()
                            })),
                            ..Default::default()
                        },
                    ],
                    tokens: Some(TokenUsage {
                        input: 3,
                        ..Default::default()
                    }),
                    injected: true,
                    ..Default::default()
                }),
                r#"{"t":"message","e":{"Role":"assistant","ID":"","Model":"","Parent":"","Parts":[{"Kind":0,"Text":"hi","Call":null,"Output":null,"Image":null},{"Kind":2,"Text":"","Call":{"ID":"c","Name":"Bash","Kind":1,"Title":"","Input":{"Command":"ls","Cwd":"","Background":false,"Timeout":0,"Path":"","To":"","Pattern":"","Query":"","URL":"","Description":"","Prompt":"","Agent":"","Child":"","Content":"","Edits":null,"Offset":0,"Limit":0,"Server":"","Tool":"","Todos":null}},"Output":null,"Image":null},{"Kind":3,"Text":"","Call":null,"Output":{"CallID":"c","Text":"","IsError":false,"Stdout":"","Stderr":"","Exit":2,"Patches":[{"oldStart":0,"oldLines":0,"newStart":0,"newLines":0,"lines":["+a"]}],"Lines":null,"Created":false},"Image":null},{"Kind":4,"Text":"","Call":null,"Output":null,"Image":{"MediaType":"image/png","Data":"cG5nIQ==","Path":""}}],"Tokens":{"Input":3,"Output":0,"CacheRead":0,"CacheWrite5m":0,"CacheWrite1h":0,"Reasoning":0},"Injected":true}}"#,
            ),
            (
                Event::Approval(Box::new(Approval {
                    id: s("a"),
                    options: vec![ApprovalOption {
                        id: s("allow"),
                        kind: OptionKind::AllowOnce,
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                r#"{"t":"approval","e":{"ID":"a","Call":{"ID":"","Name":"","Kind":0,"Title":"","Input":{"Command":"","Cwd":"","Background":false,"Timeout":0,"Path":"","To":"","Pattern":"","Query":"","URL":"","Description":"","Prompt":"","Agent":"","Child":"","Content":"","Edits":null,"Offset":0,"Limit":0,"Server":"","Tool":"","Todos":null}},"Reason":"","Path":"","Options":[{"ID":"allow","Label":"","Kind":0}]}}"#,
            ),
            (
                Event::Question(Question {
                    id: s("q"),
                    asks: vec![Ask {
                        text: s("?"),
                        options: vec![Choice {
                            label: s("A"),
                            key: s("a"),
                            picked: true,
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                r#"{"t":"question","e":{"ID":"q","CallID":"","Title":"","Asks":[{"ID":"","Header":"","Text":"?","Multi":false,"Options":[{"Label":"A","Description":"","Preview":"","Key":"a","Picked":true}]}]}}"#,
            ),
            (
                Event::TurnEnd(TurnEnd {
                    reason: s("done"),
                    duration: Duration::from_secs(1),
                    ..Default::default()
                }),
                r#"{"t":"turn_end","e":{"Reason":"done","Err":"","Text":"","Cost":0,"Tokens":{"Input":0,"Output":0,"CacheRead":0,"CacheWrite5m":0,"CacheWrite1h":0,"Reasoning":0},"Duration":1000000000,"Turns":0}}"#,
            ),
            (
                Event::Quota(usage::Quota {
                    windows: vec![usage::Window {
                        id: s("five_hour"),
                        percent: 12.0,
                        resets_at: AT,
                        span: Duration::from_secs(5 * 3600),
                        scope: usage::Scope {
                            models: vec![s("opus")],
                        },
                        ..Default::default()
                    }],
                    resets: Some(usage::Resets {
                        available: 1,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                r#"{"t":"quota","e":{"Account":"","Email":"","Plan":"","Balance":"","Windows":[{"ID":"five_hour","Label":"","Name":"","Span":18000000000000,"Percent":12,"Used":0,"Limit":0,"ResetsAt":"2026-09-27T12:00:00Z","Scope":{"Models":["opus"]}}],"FetchedAt":"0001-01-01T00:00:00Z","Source":0,"Problem":"","Resets":{"Available":1}}}"#,
            ),
            (
                Event::Limited(Limited {
                    window: s("five_hour"),
                    resets_at: AT,
                }),
                r#"{"t":"limited","e":{"Window":"five_hour","ResetsAt":"2026-09-27T12:00:00Z"}}"#,
            ),
            (
                Event::Billing(Billing {
                    billing: usage::Billing::Overage,
                }),
                r#"{"t":"billing","e":{"Billing":"overage"}}"#,
            ),
            (
                Event::StartNotice(StartNotice {
                    event: s("SessionStart"),
                    text: s("loaded project guidance"),
                }),
                r#"{"t":"start_notice","e":{"Event":"SessionStart","Text":"loaded project guidance"}}"#,
            ),
            (
                Event::TaskUpdated(TaskUpdated {
                    id: s("t"),
                    background: Some(true),
                    ..Default::default()
                }),
                r#"{"t":"task_updated","e":{"ID":"t","Status":"","Label":"","Background":true,"Err":""}}"#,
            ),
            (
                Event::Plan(Plan {
                    todos: vec![tool::TodoItem {
                        label: s("x"),
                        status: s("pending"),
                        ..Default::default()
                    }],
                }),
                r#"{"t":"plan","e":{"Todos":[{"Label":"x","Active":"","Status":"pending"}]}}"#,
            ),
            (
                Event::Other(Other {
                    adapter: s("codex"),
                    kind: s("x"),
                    raw: Some(RawJson::new(r#"{"a":1}"#).unwrap()),
                }),
                r#"{"t":"other","e":{"Adapter":"codex","Type":"x","Raw":{"a":1}}}"#,
            ),
            (
                Event::Status(Status::default()),
                r#"{"t":"status","e":{"Busy":false,"Text":""}}"#,
            ),
            (
                Event::Commands(Commands {
                    list: vec![Command {
                        name: s("x"),
                        ..Default::default()
                    }],
                }),
                r#"{"t":"commands","e":{"List":[{"Name":"x","Description":"","ArgumentHint":"","Aliases":null}]}}"#,
            ),
            (
                Event::Background(Background {
                    tasks: vec![BackgroundTask {
                        id: s("b"),
                        kind: TaskKind::Subagent,
                        ..Default::default()
                    }],
                }),
                r#"{"t":"background","e":{"Tasks":[{"ID":"b","Kind":1,"Type":"","Label":"","Parent":""}]}}"#,
            ),
            (
                Event::Context(Context {
                    tokens: 1,
                    window: 2,
                }),
                r#"{"t":"context","e":{"Tokens":1,"Window":2}}"#,
            ),
            (
                Event::Compacted(Compacted {
                    trigger: s("auto"),
                    before: 3,
                    after: 1,
                }),
                r#"{"t":"compacted","e":{"Trigger":"auto","Before":3,"After":1}}"#,
            ),
            (
                Event::CallUpdated(Box::new(CallUpdated {
                    call: tool::Call {
                        id: s("c"),
                        raw: Some(RawJson::new(r#"{"x":true}"#).unwrap()),
                        input: tool::Input {
                            edits: vec![tool::Replace {
                                old: s("a"),
                                new: s("b"),
                                all: true,
                                ..Default::default()
                            }],
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                })),
                r#"{"t":"call_updated","e":{"Call":{"ID":"c","Name":"","Kind":0,"Title":"","Input":{"Command":"","Cwd":"","Background":false,"Timeout":0,"Path":"","To":"","Pattern":"","Query":"","URL":"","Description":"","Prompt":"","Agent":"","Child":"","Content":"","Edits":[{"Path":"","Old":"a","New":"b","All":true}],"Offset":0,"Limit":0,"Server":"","Tool":"","Todos":null},"Raw":{"x":true}}}}"#,
            ),
            (
                Event::MessageStart(MessageStart {
                    id: s("m"),
                    ..Default::default()
                }),
                r#"{"t":"message_start","e":{"ID":"m","Model":""}}"#,
            ),
            (
                Event::PartStart(PartStart {
                    index: 2,
                    kind: PartKind::ToolCall,
                }),
                r#"{"t":"part_start","e":{"Index":2,"Kind":2}}"#,
            ),
            (
                Event::ApprovalCancelled(ApprovalCancelled { id: s("a") }),
                r#"{"t":"approval_cancelled","e":{"ID":"a"}}"#,
            ),
            (
                Event::Denied(Denied {
                    call_id: s("c"),
                    ..Default::default()
                }),
                r#"{"t":"denied","e":{"CallID":"c","Tool":"","Reason":""}}"#,
            ),
            (
                Event::TaskStarted(TaskStarted {
                    id: s("t"),
                    kind: TaskKind::Workflow,
                    background: true,
                    ..Default::default()
                }),
                r#"{"t":"task_started","e":{"ID":"t","CallID":"","Kind":3,"Label":"","Agent":"","Background":true}}"#,
            ),
            (
                Event::TaskProgress(TaskProgress {
                    id: s("t"),
                    tokens: 9,
                    ..Default::default()
                }),
                r#"{"t":"task_progress","e":{"ID":"t","Summary":"","LastTool":"","Tokens":9,"ToolUses":0}}"#,
            ),
            (
                Event::TaskDone(TaskDone {
                    id: s("t"),
                    status: s("completed"),
                    ..Default::default()
                }),
                r#"{"t":"task_done","e":{"ID":"t","CallID":"","Status":"completed","OutputFile":"","Summary":""}}"#,
            ),
        ]
    }

    /// Rust writes what Go writes, and reads back what it wrote.
    #[test]
    fn round_trip_as_go_writes_it() {
        for (ev, want) in golden() {
            let line = marshal(&ev).unwrap();
            assert_eq!(line, want);
            assert_eq!(unmarshal(line.as_bytes()).unwrap(), ev, "{line}");
        }
    }

    #[test]
    fn every_event_has_a_name() {
        let mut names: Vec<&str> = golden().iter().filter_map(|(e, _)| e.name()).collect();
        names.sort();
        let mut want = NAMES.to_vec();
        want.sort();
        assert_eq!(names, want, "an event added needs a name and a golden line");
    }

    #[test]
    fn retry_has_no_name_and_unknown_names_fail() {
        let err = marshal(&Event::Retry(Retry::default())).unwrap_err();
        assert_eq!(err.to_string(), "event: event.Retry has no name");
        let err = unmarshal(br#"{"t":"nope","e":{}}"#).unwrap_err();
        assert_eq!(err.to_string(), r#"event: unknown "nope""#);
    }

    /// Go reads `null` for a nil slice and leaves a missing field zero; so
    /// does this.
    #[test]
    fn reads_nulls_and_missing_fields_as_empty() {
        let ev = unmarshal(br#"{"t":"init","e":{"SessionID":"s","Tools":null}}"#).unwrap();
        assert_eq!(
            ev,
            Event::Init(Box::new(Init {
                session_id: s("s"),
                ..Default::default()
            }))
        );
        let ev = unmarshal(br#"{"t":"limited","e":{"ResetsAt":"2026-09-27T13:00:00.5+01:00"}}"#)
            .unwrap();
        assert_eq!(
            ev,
            Event::Limited(Limited {
                resets_at: AT + 500,
                ..Default::default()
            })
        );
    }
}
