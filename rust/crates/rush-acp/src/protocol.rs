//! ACP's own shapes, as far as rush reads and writes them. Port of
//! internal/adapters/acp/protocol.go:1-163.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The ACP version this client speaks.
pub const PROTOCOL_VERSION: i64 = 1;

/// What the agent said about itself when it started.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub name: String,
    pub title: String,
    pub version: String,
    pub protocol: i64,
    /// It replays a past session.
    pub load_session: bool,
    /// It picks a past session up without replaying it.
    pub resume: bool,
    /// Prompts may carry images.
    pub images: bool,
    pub auth_methods: Vec<AuthMethod>,
}

/// One way the agent can be signed in.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: String,
    /// "terminal" runs the agent's own binary with args.
    #[serde(rename = "type")]
    pub kind: String,
    pub args: Vec<String>,
}

/// One of the session's modes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Mode {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: i64,
    #[serde(rename = "agentCapabilities")]
    pub agent_capabilities: AgentCapabilities,
    #[serde(rename = "authMethods")]
    pub auth_methods: Option<Vec<AuthMethod>>,
    #[serde(rename = "agentInfo")]
    pub agent_info: Option<AgentInfo>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct AgentCapabilities {
    #[serde(rename = "loadSession")]
    pub load_session: bool,
    #[serde(rename = "promptCapabilities")]
    pub prompt_capabilities: PromptCapabilities,
    #[serde(rename = "sessionCapabilities")]
    pub session_capabilities: SessionCapabilities,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct PromptCapabilities {
    pub image: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct SessionCapabilities {
    /// Any value but null says it resumes.
    pub resume: Option<Box<RawValue>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct AgentInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

/// session/new's result, and session/load's and session/resume's without
/// the ID.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct SessionResult {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub modes: Option<Modes>,
    #[serde(rename = "configOptions")]
    pub config_options: Option<Vec<ConfigOption>>,
    /// The unstable model picker some agents still send in place of a
    /// "model" config option.
    pub models: Option<Models>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct Modes {
    #[serde(rename = "currentModeId")]
    pub current_mode_id: String,
    #[serde(rename = "availableModes")]
    pub available_modes: Vec<Mode>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct Models {
    #[serde(rename = "currentModelId")]
    pub current_model_id: String,
}

/// One value a setting can take, or a group of them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ConfigValue {
    pub value: String,
    pub name: String,
    pub description: String,
    pub options: Vec<ConfigValue>,
}

/// One of a session's settings: its model, its mode, how hard it thinks.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConfigOption {
    pub options: Vec<ConfigValue>,
    pub id: String,
    pub name: String,
    pub category: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "currentValue")]
    pub current_value: Option<Box<RawValue>>,
}

impl ConfigOption {
    /// Its value now, when it's a string.
    pub fn current(&self) -> String {
        self.current_value
            .as_deref()
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub data: String,
    #[serde(rename = "mimeType", skip_serializing_if = "String::is_empty")]
    pub mime_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uri: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct Chunk {
    pub content: ContentBlock,
    #[serde(rename = "messageId")]
    pub message_id: String,
}

/// A tool_call, a tool_call_update, or the call a permission request is
/// about. Unset fields leave what an earlier one said.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct ToolCall {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    pub title: Option<String>,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub content: Option<Vec<ToolContent>>,
    pub locations: Option<Vec<Location>>,
    #[serde(rename = "rawInput")]
    pub raw_input: Option<Box<RawValue>>,
    #[serde(rename = "rawOutput")]
    pub raw_output: Option<Box<RawValue>>,
    /// Vibe's word for what the tool does: "subagent" for its task.
    #[serde(rename = "_meta")]
    pub meta: Option<Meta>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct Meta {
    pub effect_kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(crate) struct ToolContent {
    /// content, diff, terminal
    #[serde(rename = "type")]
    pub kind: String,
    pub content: ContentBlock,
    pub path: String,
    #[serde(rename = "oldText")]
    pub old_text: Option<String>,
    #[serde(rename = "newText")]
    pub new_text: String,
    #[serde(rename = "terminalId")]
    pub terminal_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(crate) struct Location {
    pub path: String,
    pub line: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct PlanEntry {
    pub content: String,
    pub status: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct PermissionOption {
    #[serde(rename = "optionId")]
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct PromptResult {
    #[serde(rename = "stopReason")]
    pub stop_reason: String,
    pub usage: Option<PromptUsage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct PromptUsage {
    #[serde(rename = "inputTokens")]
    pub input_tokens: i64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: i64,
    #[serde(rename = "thoughtTokens")]
    pub thought_tokens: i64,
    #[serde(rename = "cachedReadTokens")]
    pub cached_read_tokens: i64,
    #[serde(rename = "cachedWriteTokens")]
    pub cached_write_tokens: i64,
}
