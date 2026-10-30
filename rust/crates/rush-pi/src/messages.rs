//! Pi's AgentMessage, as a live session and a session read back both tell
//! it, read as rush's events; its tools as rush's tool::Call and Output.
//! Port of internal/adapters/pi/messages.go:1-359.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rush_core::event::{Compacted, Event, ImageData, Message as Msg, Part, PartKind};
use rush_core::tool::{self, Call, Output, Patch, Replace};
use rush_core::usage::TokenUsage;
use rush_core::wire::RawJson;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::discover::one_line;
use crate::json::nd;

/// Pi's AgentMessage, with the fields of every role rush reads; which are
/// set depends on role (user, assistant, toolResult, bashExecution,
/// custom, branchSummary, compactionSummary).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Message {
    #[serde(deserialize_with = "nd")]
    pub role: String,
    pub content: Option<Box<RawValue>>,

    // assistant
    #[serde(deserialize_with = "nd")]
    pub model: String,
    #[serde(deserialize_with = "nd")]
    pub provider: String,
    pub usage: Option<Tokens>,
    #[serde(rename = "stopReason", deserialize_with = "nd")]
    pub stop_reason: String,
    #[serde(rename = "errorMessage", deserialize_with = "nd")]
    pub error_message: String,
    #[serde(rename = "responseId", deserialize_with = "nd")]
    pub response_id: String,
    #[serde(deserialize_with = "nd")]
    pub timestamp: i64,

    // toolResult
    #[serde(rename = "toolCallId", deserialize_with = "nd")]
    pub tool_call_id: String,
    #[serde(rename = "toolName", deserialize_with = "nd")]
    pub tool_name: String,
    #[serde(rename = "isError", deserialize_with = "nd")]
    pub is_error: bool,
    pub details: Option<Box<RawValue>>,

    // bashExecution
    #[serde(deserialize_with = "nd")]
    pub command: String,
    #[serde(deserialize_with = "nd")]
    pub output: String,
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i64>,
    #[serde(deserialize_with = "nd")]
    pub cancelled: bool,

    // custom
    #[serde(rename = "customType", deserialize_with = "nd")]
    pub custom_type: String,
    #[serde(deserialize_with = "nd")]
    pub display: bool,

    // branchSummary, compactionSummary
    #[serde(deserialize_with = "nd")]
    pub summary: String,
    #[serde(rename = "tokensBefore", deserialize_with = "nd")]
    pub tokens_before: i64,
}

/// One of a message's content blocks: text, thinking, image, toolCall.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Block {
    #[serde(rename = "type", deserialize_with = "nd")]
    pub typ: String,
    #[serde(deserialize_with = "nd")]
    pub text: String,
    #[serde(deserialize_with = "nd")]
    pub thinking: String,
    #[serde(deserialize_with = "nd")]
    pub data: String,
    #[serde(rename = "mimeType", deserialize_with = "nd")]
    pub mime_type: String,
    #[serde(deserialize_with = "nd")]
    pub id: String,
    #[serde(deserialize_with = "nd")]
    pub name: String,
    pub arguments: Option<Box<RawValue>>,
}

/// Pi's Usage. Its input is what wasn't read from the cache.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Tokens {
    #[serde(deserialize_with = "nd")]
    pub input: i64,
    #[serde(deserialize_with = "nd")]
    pub output: i64,
    #[serde(rename = "cacheRead", deserialize_with = "nd")]
    pub cache_read: i64,
    #[serde(rename = "cacheWrite", deserialize_with = "nd")]
    pub cache_write: i64,
    #[serde(deserialize_with = "nd")]
    pub reasoning: i64,
    #[serde(rename = "totalTokens", deserialize_with = "nd")]
    pub total_tokens: i64,
    #[serde(deserialize_with = "nd")]
    pub cost: Cost,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Cost {
    #[serde(deserialize_with = "nd")]
    pub total: f64,
}

impl Tokens {
    pub fn usage(&self) -> TokenUsage {
        TokenUsage {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write_5m: self.cache_write,
            reasoning: self.reasoning,
            ..TokenUsage::default()
        }
    }

    /// How much of the window the request filled: all it read and wrote.
    pub fn context(&self) -> i64 {
        if self.total_tokens > 0 {
            return self.total_tokens;
        }
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// A raw value kept as rush keeps an agent's own JSON.
pub(crate) fn raw_json(r: &Option<Box<RawValue>>) -> Option<RawJson> {
    r.as_ref().map(|r| RawJson(r.clone()))
}

impl Message {
    /// Its content: a bare string is one text block.
    pub fn blocks(&self) -> Vec<Block> {
        let Some(c) = &self.content else {
            return Vec::new();
        };
        if c.get().starts_with('"') {
            let s: String = serde_json::from_str(c.get()).unwrap_or_default();
            return vec![Block {
                typ: "text".into(),
                text: s,
                ..Block::default()
            }];
        }
        serde_json::from_str::<Option<Vec<Block>>>(c.get())
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// Its text blocks, joined.
    pub fn text(&self) -> String {
        let out: Vec<String> = self
            .blocks()
            .into_iter()
            .filter(|b| b.typ == "text" && !b.text.is_empty())
            .map(|b| b.text)
            .collect();
        out.join("\n")
    }

    /// What an assistant message is known by: the provider's response id,
    /// else its time.
    pub fn id(&self) -> String {
        if !self.response_id.is_empty() {
            return self.response_id.clone();
        }
        format!("pi-{}", self.timestamp)
    }

    /// What a whole message says, as a live session and a session read back
    /// both tell it; empty for one rush doesn't show.
    pub fn events(&self, id: &str) -> Vec<Event> {
        match self.role.as_str() {
            "user" => {
                let out = self.user(id);
                if !out.parts.is_empty() {
                    return vec![Event::Message(out)];
                }
            }
            "assistant" => {
                let out = self.assistant(id);
                if !out.parts.is_empty() || out.tokens.is_some() {
                    return vec![Event::Message(out)];
                }
            }
            "toolResult" => {
                let o = output_of(self);
                return vec![Event::Message(Msg {
                    role: "user".into(),
                    id: format!("{}:result", self.tool_call_id),
                    parts: vec![result_part(o)],
                    ..Msg::default()
                })];
            }
            "bashExecution" => return self.bash(),
            "custom" => {
                let t = self.text();
                if self.display && !t.is_empty() {
                    return vec![Event::Message(injected(id, t))];
                }
            }
            "branchSummary" => {
                if !self.summary.is_empty() {
                    return vec![Event::Message(injected(id, self.summary.clone()))];
                }
            }
            "compactionSummary" => {
                return vec![Event::Compacted(Compacted {
                    before: self.tokens_before,
                    ..Compacted::default()
                })];
            }
            _ => {}
        }
        Vec::new()
    }

    /// A message of yours: its text and images.
    fn user(&self, id: &str) -> Msg {
        let mut out = Msg {
            role: "user".into(),
            id: id.into(),
            ..Msg::default()
        };
        for b in self.blocks() {
            match b.typ.as_str() {
                "text" => out.parts.push(text_part(PartKind::Text, b.text)),
                "image" => out.parts.push(Part {
                    kind: PartKind::Image,
                    image: Some(Box::new(image(&b))),
                    ..Part::default()
                }),
                _ => {}
            }
        }
        out
    }

    /// The model's message: its text, thinking and tool calls, and what it
    /// cost in tokens.
    fn assistant(&self, id: &str) -> Msg {
        let mut out = Msg {
            role: "assistant".into(),
            id: id.into(),
            model: self.model.clone(),
            ..Msg::default()
        };
        for b in self.blocks() {
            match b.typ.as_str() {
                "text" if !b.text.is_empty() => out.parts.push(text_part(PartKind::Text, b.text)),
                "thinking" if !b.thinking.is_empty() => {
                    out.parts.push(text_part(PartKind::Thinking, b.thinking))
                }
                "toolCall" => out.parts.push(call_part(call_of(&b))),
                _ => {}
            }
        }
        if let Some(u) = &self.usage {
            out.tokens = Some(u.usage());
        }
        out
    }

    /// A command you ran yourself with !, shown as its call and result.
    fn bash(&self) -> Vec<Event> {
        let c = Call {
            id: format!("bash-{}", self.timestamp),
            name: "bash".into(),
            kind: tool::Kind::Shell,
            input: tool::Input {
                command: self.command.clone(),
                ..tool::Input::default()
            },
            ..Call::default()
        };
        let o = Output {
            call_id: c.id.clone(),
            text: self.output.clone(),
            stdout: self.output.clone(),
            exit: self.exit_code,
            is_error: self.cancelled || self.exit_code.is_some_and(|e| e != 0),
            ..Output::default()
        };
        let id = c.id.clone();
        vec![
            Event::Message(Msg {
                role: "assistant".into(),
                id: id.clone(),
                parts: vec![call_part(c)],
                ..Msg::default()
            }),
            Event::Message(Msg {
                role: "user".into(),
                id: format!("{id}:result"),
                parts: vec![result_part(o)],
                ..Msg::default()
            }),
        ]
    }
}

pub(crate) fn text_part(kind: PartKind, text: String) -> Part {
    Part {
        kind,
        text,
        ..Part::default()
    }
}

pub(crate) fn call_part(c: Call) -> Part {
    Part {
        kind: PartKind::ToolCall,
        call: Some(Box::new(c)),
        ..Part::default()
    }
}

pub(crate) fn result_part(o: Output) -> Part {
    Part {
        kind: PartKind::ToolResult,
        output: Some(Box::new(o)),
        ..Part::default()
    }
}

/// Text pi put in the conversation itself.
fn injected(id: &str, text: String) -> Msg {
    Msg {
        role: "user".into(),
        id: id.into(),
        injected: true,
        parts: vec![text_part(PartKind::Text, text)],
        ..Msg::default()
    }
}

fn image(b: &Block) -> ImageData {
    ImageData {
        media_type: b.mime_type.clone(),
        data: STANDARD.decode(&b.data).unwrap_or_default(),
        path: String::new(),
    }
}

/// The arguments of Pi's built-in tools, all of them at once.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Args {
    #[serde(deserialize_with = "nd")]
    command: String, // bash
    #[serde(deserialize_with = "nd")]
    timeout: f64, // bash, seconds
    #[serde(deserialize_with = "nd")]
    path: String, // read, edit, write, grep, find, ls
    #[serde(deserialize_with = "nd")]
    offset: i64, // read
    #[serde(deserialize_with = "nd")]
    limit: i64, // read
    #[serde(deserialize_with = "nd")]
    content: String, // write
    #[serde(deserialize_with = "nd")]
    pattern: String, // grep, find
    #[serde(deserialize_with = "nd")]
    edits: Vec<EditArg>, // edit
    #[serde(rename = "oldText", deserialize_with = "nd")]
    old_text: String, // edit, as older pis took one
    #[serde(rename = "newText", deserialize_with = "nd")]
    new_text: String,
    #[serde(deserialize_with = "nd")]
    agent: String, // subagent
    #[serde(deserialize_with = "nd")]
    task: String,
    #[serde(deserialize_with = "nd")]
    tasks: Vec<Subtask>, // subagent, in parallel
    #[serde(deserialize_with = "nd")]
    chain: Vec<Subtask>, // subagent, one after another
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct EditArg {
    #[serde(rename = "oldText", deserialize_with = "nd")]
    old_text: String,
    #[serde(rename = "newText", deserialize_with = "nd")]
    new_text: String,
}

/// A subagent extension's task: which agent, and what to do.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Subtask {
    #[serde(deserialize_with = "nd")]
    agent: String,
    #[serde(deserialize_with = "nd")]
    task: String,
}

/// An object's members in the order written.
struct Fields<'a>(Vec<(String, &'a RawValue)>);

impl<'de: 'a, 'a> Deserialize<'de> for Fields<'a> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Fields<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut m: M,
            ) -> Result<Self::Value, M::Error> {
                let mut out = Vec::new();
                while let Some(kv) = m.next_entry::<String, &'de RawValue>()? {
                    out.push(kv);
                }
                Ok(Fields(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// Reads v into slot; false when it's of a type slot can't hold. Null
/// leaves slot as it was.
fn set<T: serde::de::DeserializeOwned>(slot: &mut T, v: &RawValue) -> bool {
    match serde_json::from_str::<Option<T>>(v.get()) {
        Ok(Some(x)) => {
            *slot = x;
            true
        }
        Ok(None) => true,
        Err(_) => false,
    }
}

/// A call's arguments as Go's decoder reads them into args: field by
/// field in the order written (names matched whatever their case), until
/// one of a type its field can't hold, after which nothing more is.
fn args_of(raw: &str) -> Args {
    let mut a = Args::default();
    let Ok(Fields(fs)) = serde_json::from_str::<Fields>(raw) else {
        return a;
    };
    for (k, v) in fs {
        let fits = match k.to_ascii_lowercase().as_str() {
            "command" => set(&mut a.command, v),
            "timeout" => set(&mut a.timeout, v),
            "path" => set(&mut a.path, v),
            "offset" => set(&mut a.offset, v),
            "limit" => set(&mut a.limit, v),
            "content" => set(&mut a.content, v),
            "pattern" => set(&mut a.pattern, v),
            "edits" => set(&mut a.edits, v),
            "oldtext" => set(&mut a.old_text, v),
            "newtext" => set(&mut a.new_text, v),
            "agent" => set(&mut a.agent, v),
            "task" => set(&mut a.task, v),
            "tasks" => set(&mut a.tasks, v),
            "chain" => set(&mut a.chain, v),
            _ => true,
        };
        if !fits {
            break;
        }
    }
    a
}

/// A tool call as rush draws it. Pi's own tools are read, bash (or
/// powershell), edit, write, grep, find and ls; the subagent extension's
/// is a subagent; other extensions' are drawn by name.
pub(crate) fn call_of(b: &Block) -> Call {
    let mut c = Call {
        id: b.id.clone(),
        name: b.name.clone(),
        raw: raw_json(&b.arguments),
        ..Call::default()
    };
    let a = b
        .arguments
        .as_ref()
        .map(|r| args_of(r.get()))
        .unwrap_or_default();
    let i = &mut c.input;
    match b.name.as_str() {
        "bash" | "powershell" => {
            c.kind = tool::Kind::Shell;
            i.command = a.command;
            i.timeout = (a.timeout * 1000.0) as i64;
        }
        "read" => {
            c.kind = tool::Kind::Read;
            (i.path, i.offset, i.limit) = (a.path, a.offset, a.limit);
        }
        "edit" => {
            c.kind = tool::Kind::Edit;
            i.path = a.path;
            for e in a.edits.iter() {
                i.edits.push(Replace {
                    old: e.old_text.clone(),
                    new: e.new_text.clone(),
                    ..Replace::default()
                });
            }
            if a.edits.is_empty() && (!a.old_text.is_empty() || !a.new_text.is_empty()) {
                i.edits = vec![Replace {
                    old: a.old_text,
                    new: a.new_text,
                    ..Replace::default()
                }];
            }
        }
        "write" => {
            c.kind = tool::Kind::Write;
            (i.path, i.content) = (a.path, a.content);
        }
        "grep" => {
            c.kind = tool::Kind::Search;
            (i.pattern, i.path) = (a.pattern, a.path);
        }
        "find" => {
            c.kind = tool::Kind::Glob;
            (i.pattern, i.path) = (a.pattern, a.path);
        }
        "ls" => {
            c.kind = tool::Kind::Glob;
            (i.pattern, i.path) = ("*".into(), a.path);
        }
        "subagent" => {
            c.kind = tool::Kind::Subagent;
            (i.agent, i.prompt) = (a.agent, a.task);
            let many: Vec<&Subtask> = a.tasks.iter().chain(a.chain.iter()).collect();
            if !many.is_empty() {
                let agents: Vec<&str> = many.iter().map(|t| t.agent.as_str()).collect();
                let tasks: Vec<String> = many
                    .iter()
                    .map(|t| format!("{}: {}", t.agent, t.task))
                    .collect();
                i.agent = agents.join(", ");
                i.prompt = tasks.join("\n\n");
            }
            i.description = one_line(&i.prompt);
        }
        _ => {}
    }
    c
}

/// What a tool result says. An edit's details carry its diff.
pub(crate) fn output_of(m: &Message) -> Output {
    let mut o = Output {
        call_id: m.tool_call_id.clone(),
        text: m.text(),
        is_error: m.is_error,
        raw: raw_json(&m.details),
        ..Output::default()
    };
    if m.tool_name == "bash" || m.tool_name == "powershell" {
        o.stdout = o.text.clone();
    }
    if m.tool_name == "edit"
        && let Some(d) = &m.details
    {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct D {
            #[serde(deserialize_with = "nd")]
            diff: String,
        }
        if let Ok(Some(d)) = serde_json::from_str::<Option<D>>(d.get())
            && !d.diff.is_empty()
        {
            o.patches = parse_diff(&d.diff);
        }
    }
    o
}

/// Reads Pi's own diff: a line per change, "+12 text", "-12 text" or " 12
/// text", numbered in the new file, the old one and the old one again,
/// with a "..." line between hunks.
pub(crate) fn parse_diff(diff: &str) -> Vec<Patch> {
    let mut out = Vec::new();
    let mut cur: Option<Patch> = None;
    let mut shift = 0i64; // lines added less lines removed, before the hunk being read
    for ln in diff.trim_end_matches('\n').split('\n') {
        if ln.is_empty() {
            continue;
        }
        let (sign, rest) = ln.split_at(ln.chars().next().map_or(1, char::len_utf8));
        let rest = rest.trim_start_matches(' ');
        let (num_text, text) = rest.split_once(' ').unwrap_or((rest, ""));
        if sign == " " && num_text == "..." {
            if let Some(c) = cur.take() {
                shift += c.new_lines - c.old_lines;
                out.push(c);
            }
            continue;
        }
        let Ok(n) = num_text.parse::<i64>() else {
            continue;
        };
        if sign != "+" && sign != "-" && sign != " " {
            continue;
        }
        let c = cur.get_or_insert_with(|| {
            let mut p = Patch::default();
            if sign == "+" {
                (p.new_start, p.old_start) = (n, n - shift);
            } else {
                (p.old_start, p.new_start) = (n, n + shift);
            }
            p
        });
        c.lines.push(format!("{sign}{text}"));
        match sign {
            "+" => c.new_lines += 1,
            "-" => c.old_lines += 1,
            _ => {
                c.old_lines += 1;
                c.new_lines += 1;
            }
        }
    }
    if let Some(c) = cur {
        out.push(c);
    }
    out
}

/// How a run ended, in rush's words, from its last message's stop reason.
pub(crate) fn reason(stop: &str) -> String {
    match stop {
        "" | "stop" | "toolUse" => "done",
        "aborted" => "interrupted",
        "length" => "max_tokens",
        s => s, // error
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diff_numbers_hunks_in_both_files() {
        let diff = "  1 a\n- 2 b\n+ 2 B\n+ 3 C\n  4 c\n    ...\n  9 h\n-10 i\n  11 j";
        let want = vec![
            Patch {
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 4,
                lines: vec![" a", "-b", "+B", "+C", " c"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                ..Patch::default()
            },
            Patch {
                old_start: 9,
                old_lines: 3,
                new_start: 10,
                new_lines: 2,
                lines: vec![" h", "-i", " j"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                ..Patch::default()
            },
        ];
        assert_eq!(parse_diff(diff), want);
    }

    // Powershell runs a command as bash does.
    #[test]
    fn powershell_is_shell() {
        let c = call_of(&Block {
            id: "c1".into(),
            name: "powershell".into(),
            arguments: Some(
                RawValue::from_string(r#"{"command":"Get-ChildItem","timeout":5}"#.into()).unwrap(),
            ),
            ..Block::default()
        });
        assert_eq!(c.kind, tool::Kind::Shell);
        assert_eq!(c.input.command, "Get-ChildItem");
        assert_eq!(c.input.timeout, 5000);
        // A field of the wrong type ends the read there, not before.
        let c = call_of(&Block {
            id: "c2".into(),
            name: "read".into(),
            arguments: Some(
                RawValue::from_string(r#"{"path":"/a/x.go","offset":"ten","limit":5}"#.into())
                    .unwrap(),
            ),
            ..Block::default()
        });
        assert_eq!(
            (c.input.path.as_str(), c.input.offset, c.input.limit),
            ("/a/x.go", 0, 0)
        );
    }

    #[test]
    fn reasons() {
        for (stop, want) in [
            ("", "done"),
            ("stop", "done"),
            ("toolUse", "done"),
            ("aborted", "interrupted"),
            ("length", "max_tokens"),
            ("error", "error"),
        ] {
            assert_eq!(reason(stop), want, "{stop}");
        }
    }
}
