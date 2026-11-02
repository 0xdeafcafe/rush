//! A tool call as rush draws it, whichever agent made it. Each adapter
//! reads its own tool names and input shapes into a Call once, so the
//! renderer switches on Kind and reads Input's fields, not on "Bash" or
//! "shell" or "execute". After Go's internal/agent/tool/tool.go.
//!
//! What a call is doing in words ("running pnpm test") is [`doing`], the
//! one copy every screen says it with (Go's tool/doing.go).

mod doing;

pub use doing::{base, doing, lower_first, one_line, shell_doing};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::wire::{self, RawJson};

/// What a tool call does. On the wire it is its number, as Go's iota.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    /// Anything else: drawn by its Name.
    #[default]
    Other,
    /// Runs a command.
    Shell,
    /// Reads a file.
    Read,
    /// Changes part of a file.
    Edit,
    /// Writes a whole file.
    Write,
    /// Deletes a file.
    Delete,
    /// Moves or renames a file.
    Move,
    /// Searches file contents (grep).
    Search,
    /// Finds files by name.
    Glob,
    /// Reads a web page.
    Fetch,
    /// Searches the web.
    WebSearch,
    /// Hands work to a subagent.
    Subagent,
    /// Changes the agent's todo list or plan.
    Todo,
    /// Asks the user something, with choices.
    Question,
    /// Leaves or enters plan mode with a plan.
    PlanMode,
    /// An MCP server's tool.
    Mcp,
    /// Edits a notebook cell.
    Notebook,
    /// Thinks aloud, runs nothing.
    Think,
}

const KINDS: [Kind; 18] = [
    Kind::Other,
    Kind::Shell,
    Kind::Read,
    Kind::Edit,
    Kind::Write,
    Kind::Delete,
    Kind::Move,
    Kind::Search,
    Kind::Glob,
    Kind::Fetch,
    Kind::WebSearch,
    Kind::Subagent,
    Kind::Todo,
    Kind::Question,
    Kind::PlanMode,
    Kind::Mcp,
    Kind::Notebook,
    Kind::Think,
];

const KIND_NAMES: [&str; 18] = [
    "other",
    "shell",
    "read",
    "edit",
    "write",
    "delete",
    "move",
    "search",
    "glob",
    "fetch",
    "websearch",
    "subagent",
    "todo",
    "question",
    "planmode",
    "mcp",
    "notebook",
    "think",
];

impl Kind {
    /// The kind numbered n, as Go numbers them; Other for one it doesn't.
    pub fn from_i64(n: i64) -> Kind {
        usize::try_from(n)
            .ok()
            .and_then(|i| KINDS.get(i).copied())
            .unwrap_or(Kind::Other)
    }

    pub fn as_str(self) -> &'static str {
        KIND_NAMES[self as usize]
    }

    /// Whether a call of this kind changes files.
    pub fn changes(self) -> bool {
        matches!(
            self,
            Kind::Edit | Kind::Write | Kind::Delete | Kind::Move | Kind::Notebook
        )
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Kind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(*self as u8)
    }
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Kind, D::Error> {
        Ok(Kind::from_i64(Option::<i64>::deserialize(d)?.unwrap_or(0)))
    }
}

/// One tool call.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Call {
    #[serde(rename = "ID")]
    pub id: String,
    /// The agent's own name for the tool: "Bash", "shell",
    /// "mcp__linear__get_issue".
    pub name: String,
    pub kind: Kind,
    /// What the agent says the call is, when it says: ACP's title.
    pub title: String,
    pub input: Input,
    /// The input as the agent sent it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<RawJson>,
}

/// What a call works on, as far as rush reads it. An adapter fills in the
/// fields its tool has, and leaves the rest empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Input {
    /// Shell.
    pub command: String,
    /// Shell, when it isn't the session's.
    pub cwd: String,
    /// Shell: started to run beside the turn.
    pub background: bool,
    /// Shell, milliseconds.
    pub timeout: i64,
    /// Read, Edit, Write, Delete, Notebook; Search and Glob: where.
    pub path: String,
    /// Move.
    pub to: String,
    /// Search, Glob.
    pub pattern: String,
    /// WebSearch.
    pub query: String,
    /// Fetch.
    #[serde(rename = "URL")]
    pub url: String,
    /// What the call is for, in the agent's words.
    pub description: String,
    /// Subagent; Fetch: what to look for.
    pub prompt: String,
    /// Subagent: its type.
    pub agent: String,
    /// Subagent: the run it started, where its harness keeps it apart (a
    /// Codex thread, or task).
    pub child: String,
    /// Write: the whole file.
    pub content: String,
    /// Edit.
    #[serde(with = "wire::nullvec")]
    pub edits: Vec<Replace>,
    /// Read: first line, 1-based; 0 is the start.
    pub offset: i64,
    /// Read: how many lines; 0 is all.
    pub limit: i64,
    /// MCP.
    pub server: String,
    /// MCP.
    pub tool: String,
    /// Todo.
    #[serde(with = "wire::nullvec")]
    pub todos: Vec<TodoItem>,
}

/// One replacement in a file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Replace {
    /// When a call edits several files; else Input.path.
    pub path: String,
    pub old: String,
    pub new: String,
    /// Every occurrence.
    pub all: bool,
}

/// One item of a todo list or plan as a Todo call sets it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TodoItem {
    /// What it is.
    pub label: String,
    /// What it says while it's being done: "Running the tests".
    pub active: String,
    /// pending, in_progress, completed
    pub status: String,
}

/// What a call returned.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Output {
    #[serde(rename = "CallID")]
    pub call_id: String,
    /// Flattened to text, as the agent would read it.
    pub text: String,
    pub is_error: bool,
    /// Shell, when the agent keeps the streams apart.
    pub stdout: String,
    pub stderr: String,
    /// Shell, when known.
    pub exit: Option<i64>,
    /// Edit and Write: what changed.
    #[serde(with = "wire::nullvec")]
    pub patches: Vec<Patch>,
    /// Read: which lines it read.
    pub lines: Option<Span>,
    /// Write: the file didn't exist before.
    pub created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<RawJson>,
}

/// One hunk of a change to a file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Patch {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(rename = "oldStart")]
    pub old_start: i64,
    #[serde(rename = "oldLines")]
    pub old_lines: i64,
    #[serde(rename = "newStart")]
    pub new_start: i64,
    #[serde(rename = "newLines")]
    pub new_lines: i64,
    /// Each prefixed ' ', '-' or '+'.
    #[serde(with = "wire::nullvec")]
    pub lines: Vec<String>,
}

/// A run of lines out of a file's total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Span {
    pub start: i64,
    pub count: i64,
    pub total: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_name_and_number() {
        for (i, name) in KIND_NAMES.iter().enumerate() {
            let k = Kind::from_i64(i as i64);
            assert_eq!(k as usize, i);
            assert_eq!(k.as_str(), *name);
        }
        assert_eq!(Kind::from_i64(99), Kind::Other);
        assert_eq!(Kind::from_i64(-1), Kind::Other);
        let changes: Vec<Kind> = KINDS.iter().copied().filter(|k| k.changes()).collect();
        assert_eq!(
            changes,
            [
                Kind::Edit,
                Kind::Write,
                Kind::Delete,
                Kind::Move,
                Kind::Notebook
            ]
        );
    }
}
