//! A message as Claude Code writes it, in its transcripts and its
//! stream-json: decode reads one (Go's headless.Message), and neutral turns
//! it into rush's event::Message. Nothing outside this crate sees it.

use serde_json::value::RawValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Text,
    Thinking,
    ToolUse,
    ToolResult,
}

/// An image a message or tool result carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub media_type: String,
    /// Base64 as the transcript holds it.
    pub data: String,
}

/// One content block of a message.
#[derive(Clone, Debug)]
pub struct Block {
    pub kind: BlockKind,
    /// text, thinking, or a tool result flattened to text.
    pub text: String,
    /// tool_use: its id and tool name.
    pub id: String,
    pub name: String,
    /// tool_use: the input as JSON, kept unparsed.
    pub input: Option<Box<RawValue>>,
    /// tool_result: the tool_use it answers.
    pub tool_use_id: String,
    pub is_error: bool,
    pub images: Vec<Image>,
}

impl Block {
    pub(crate) fn new(kind: BlockKind) -> Block {
        Block {
            kind,
            text: String::new(),
            id: String::new(),
            name: String::new(),
            input: None,
            tool_use_id: String::new(),
            is_error: false,
            images: Vec::new(),
        }
    }
}

/// Tokens one request used, as Claude's API reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
}

/// A whole assistant or user message. Tool results arrive as user messages.
#[derive(Clone, Debug)]
pub struct Message {
    pub role: Role,
    pub id: String,
    /// Assistant messages: the model that wrote it.
    pub model: String,
    /// Set when a subagent wrote it.
    pub parent_tool_use_id: String,
    pub blocks: Vec<Block>,
    pub usage: Option<Usage>,
    /// The agent's structured account of a tool run (an Edit's patch, a
    /// Bash run's stdout and stderr), kept unparsed.
    pub tool_result: Option<Box<RawValue>>,
}
