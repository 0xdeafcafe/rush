//! Codex's thread items (what the app-server sends a live client, and a
//! rollout's item_completed records) as rush's tool calls, their outputs
//! and messages. Port of internal/adapters/codex/items.go:1-329.

use std::sync::OnceLock;

use base64::Engine;
use rush_core::event::{ImageData, Message, Part, PartKind};
use rush_core::tool::{Call, Kind, Output};
use rush_core::wire::RawJson;
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::diff::{is_diff, parse_diff, whole_file};
use crate::json::{arr, boolean, get, int, object, s, st, strs};

/// Codex's ThreadItem, with the fields of every type rush reads; which are
/// set depends on typ.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ThreadItem {
    pub typ: String,
    pub id: String,

    /// agentMessage
    pub text: String,
    /// userMessage: []UserInput; reasoning: []string
    pub content: Value,
    /// reasoning
    pub summary: Vec<String>,

    /// commandExecution
    pub command: String,
    pub cwd: String,
    pub command_actions: Vec<CommandAction>,
    pub aggregated_output: Option<String>,
    pub exit_code: Option<i64>,
    pub status: String,
    /// Its terminal, which can outlive the call.
    pub process_id: String,

    /// fileChange
    pub changes: Vec<FileUpdate>,

    /// mcpToolCall
    pub server: String,
    /// mcpToolCall, dynamicToolCall, collabAgentToolCall
    pub tool: String,
    pub arguments: Option<RawJson>,
    pub result: Value,
    pub error: Option<String>,
    /// dynamicToolCall
    pub content_items: Vec<ContentItem>,
    pub success: Option<bool>,

    /// webSearch
    pub query: String,
    pub action: Option<Action>,

    /// imageView
    pub path: String,

    /// collabAgentToolCall
    pub prompt: String,
    pub receivers: Vec<String>,
    pub model: String,

    /// subAgentActivity: started, interacted, interrupted, completed
    pub kind: String,
    pub agent_thread_id: String,
    pub agent_path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CommandAction {
    /// read, listFiles, search, unknown
    pub typ: String,
    pub name: String,
    pub path: String,
    pub query: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileUpdate {
    pub path: String,
    /// add, delete, update
    pub kind: String,
    pub move_path: String,
    pub diff: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContentItem {
    pub typ: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Action {
    pub typ: String,
    pub query: String,
    pub queries: Vec<String>,
    pub url: String,
    pub pattern: String,
}

/// Only an item's arguments, kept as written.
#[derive(Deserialize)]
struct Args<'a> {
    #[serde(borrow)]
    arguments: Option<&'a RawValue>,
}

pub(crate) fn content_items(v: &[Value]) -> Vec<ContentItem> {
    v.iter()
        .map(|c| ContentItem {
            typ: st(c, "type"),
            text: st(c, "text"),
        })
        .collect()
}

pub(crate) fn action_of(v: &Value) -> Option<Action> {
    v.as_object()?;
    Some(Action {
        typ: st(v, "type"),
        query: st(v, "query"),
        queries: strs(v, "queries"),
        url: st(v, "url"),
        pattern: st(v, "pattern"),
    })
}

/// A string at key, or the text of a number there (a process id).
fn id_text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

impl ThreadItem {
    /// The item in raw; None when it isn't one.
    pub fn read(raw: &str) -> Option<ThreadItem> {
        let v: Value = serde_json::from_str(raw).ok()?;
        let mut it = ThreadItem::from_value(&v)?;
        if matches!(it.typ.as_str(), "mcpToolCall" | "dynamicToolCall")
            && let Ok(a) = serde_json::from_str::<Args>(raw)
        {
            it.arguments = a
                .arguments
                .filter(|r| r.get() != "null")
                .map(|r| RawJson(r.to_owned()));
        }
        Some(it)
    }

    /// The item in v: null is an empty one, anything else not an object
    /// none.
    pub fn from_value(v: &Value) -> Option<ThreadItem> {
        if v.is_null() {
            return Some(ThreadItem::default());
        }
        v.as_object()?;
        Some(ThreadItem {
            typ: st(v, "type"),
            id: st(v, "id"),
            text: st(v, "text"),
            content: v.get("content").cloned().unwrap_or(Value::Null),
            summary: strs(v, "summary"),
            command: st(v, "command"),
            cwd: st(v, "cwd"),
            command_actions: arr(v, "commandActions")
                .iter()
                .map(|a| CommandAction {
                    typ: st(a, "type"),
                    name: st(a, "name"),
                    path: st(a, "path"),
                    query: st(a, "query"),
                })
                .collect(),
            aggregated_output: get(v, "aggregatedOutput")
                .and_then(Value::as_str)
                .map(str::to_string),
            exit_code: int(v, "exitCode"),
            status: st(v, "status"),
            process_id: id_text(v, "processId"),
            changes: arr(v, "changes")
                .iter()
                .map(|c| {
                    let k = c.get("kind").unwrap_or(&Value::Null);
                    FileUpdate {
                        path: st(c, "path"),
                        kind: st(k, "type"),
                        move_path: st(k, "move_path"),
                        diff: st(c, "diff"),
                    }
                })
                .collect(),
            server: st(v, "server"),
            tool: st(v, "tool"),
            arguments: get(v, "arguments").and_then(crate::json::raw_of),
            result: v.get("result").cloned().unwrap_or(Value::Null),
            error: get(v, "error").map(|e| st(e, "message")),
            content_items: content_items(arr(v, "contentItems")),
            success: boolean(v, "success"),
            query: st(v, "query"),
            action: get(v, "action").and_then(action_of),
            path: st(v, "path"),
            prompt: st(v, "prompt"),
            receivers: strs(v, "receiverThreadIds"),
            model: st(v, "model"),
            kind: st(v, "kind"),
            agent_thread_id: st(v, "agentThreadId"),
            agent_path: st(v, "agentPath"),
        })
    }
}

/// The last element of a slash-separated path, as Go's path.Base: "." for
/// an empty one.
pub(crate) fn path_base(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/".into();
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// The tool call an item is, if it is one.
pub(crate) fn call_of(it: &ThreadItem, raw: Option<RawJson>) -> Option<Call> {
    let mut c = Call {
        id: it.id.clone(),
        raw,
        ..Call::default()
    };
    match it.typ.as_str() {
        "commandExecution" => {
            (c.name, c.kind) = ("shell".into(), Kind::Shell);
            (c.input.command, c.input.cwd) = (script(&it.command), it.cwd.clone());
            // Codex reads one-step commands itself; a lone read or search
            // is drawn as one.
            if let [a] = it.command_actions.as_slice() {
                match a.typ.as_str() {
                    "read" => (c.kind, c.input.path) = (Kind::Read, a.path.clone()),
                    "search" if !a.query.is_empty() => {
                        c.kind = Kind::Search;
                        (c.input.pattern, c.input.path) = (a.query.clone(), a.path.clone());
                    }
                    _ => {}
                }
            }
        }
        "fileChange" => {
            (c.name, c.kind) = ("apply_patch".into(), Kind::Edit);
            if let Some(ch) = it.changes.first() {
                c.input.path = ch.path.clone();
                if it.changes.len() == 1 {
                    match ch.kind.as_str() {
                        "add" => {
                            c.kind = Kind::Write;
                            if !is_diff(&ch.diff) {
                                c.input.content = ch.diff.clone();
                            }
                        }
                        "delete" => c.kind = Kind::Delete,
                        "update" if !ch.move_path.is_empty() => {
                            (c.kind, c.input.to) = (Kind::Move, ch.move_path.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
        "mcpToolCall" => {
            // As Claude Code names them.
            c.name = format!("mcp__{}__{}", it.server, it.tool);
            c.kind = Kind::Mcp;
            (c.input.server, c.input.tool) = (it.server.clone(), it.tool.clone());
            c.raw = it.arguments.clone();
        }
        "dynamicToolCall" => {
            (c.name, c.raw) = (it.tool.clone(), it.arguments.clone());
        }
        "webSearch" => {
            (c.name, c.kind, c.input.query) =
                ("web_search".into(), Kind::WebSearch, it.query.clone());
            if let Some(a) = &it.action {
                match a.typ.as_str() {
                    "search" => {
                        if c.input.query.is_empty() {
                            c.input.query = a.query.clone();
                            if a.query.is_empty() && !a.queries.is_empty() {
                                c.input.query = a.queries.join(", ");
                            }
                        }
                    }
                    "openPage" | "findInPage" => {
                        c.kind = Kind::Fetch;
                        (c.input.url, c.input.pattern) = (a.url.clone(), a.pattern.clone());
                    }
                    _ => {}
                }
            }
        }
        "imageView" => {
            (c.name, c.kind, c.input.path) =
                ("view_image".into(), Kind::Read, local_path(&it.path));
        }
        "collabAgentToolCall" => {
            let to = it.receivers.first().cloned().unwrap_or_default();
            match it.tool.as_str() {
                "spawnAgent" => {
                    (c.name, c.kind) = (it.tool.clone(), Kind::Subagent);
                    c.input.prompt = it.prompt.clone();
                    c.input.agent = it.model.clone();
                    c.input.child = to;
                }
                "sendInput" | "sendMessage" | "followupTask" | "resumeAgent" => as_claude(
                    &mut c,
                    "SendMessage",
                    &[("to", to.into()), ("message", it.prompt.clone().into())],
                ),
                "interruptAgent" | "closeAgent" => {
                    as_claude(&mut c, "TaskStop", &[("task_id", to.into())])
                }
                "listAgents" => as_claude(&mut c, "ListAgents", &[]),
                _ => {
                    // wait
                    let to = if to.is_empty() {
                        "its subagents".to_string()
                    } else {
                        to
                    };
                    as_claude(&mut c, "TaskOutput", &[("task_id", to.into())])
                }
            }
        }
        "subAgentActivity" => {
            // Codex 0.155 says its agent tools this way: the thread, by its
            // task.
            let task = path_base(&it.agent_path);
            match it.kind.as_str() {
                "started" => {
                    (c.name, c.kind) = ("spawn_agent".into(), Kind::Subagent);
                    (c.input.child, c.input.description) = (it.agent_thread_id.clone(), task);
                }
                "interacted" => as_claude(&mut c, "SendMessage", &[("to", task.into())]),
                "interrupted" => as_claude(&mut c, "TaskStop", &[("task_id", task.into())]),
                _ => return None, // completed: its turn ended, which its task says
            }
        }
        _ => return None,
    }
    Some(c)
}

/// Makes c a call to Claude Code's tool name, with its input, so it's
/// drawn as that: a message, a stop, a check on its agents.
pub(crate) fn as_claude(c: &mut Call, name: &str, input: &[(&str, Value)]) {
    (c.name, c.kind) = (name.to_string(), Kind::Other);
    c.raw = Some(object(input));
}

/// What a finished tool call item returned.
pub(crate) fn output_of(it: &ThreadItem, raw: Option<RawJson>) -> Output {
    let mut o = Output {
        call_id: it.id.clone(),
        raw,
        ..Output::default()
    };
    let failed = it.status == "failed" || it.status == "declined";
    match it.typ.as_str() {
        "commandExecution" => {
            if let Some(a) = &it.aggregated_output {
                (o.text, o.stdout) = (a.clone(), a.clone());
            }
            o.exit = it.exit_code;
            o.is_error = failed || it.exit_code.is_some_and(|c| c != 0);
            if it.status == "declined" && o.text.is_empty() {
                o.text = "declined".into();
            }
        }
        "fileChange" => {
            for ch in &it.changes {
                if is_diff(&ch.diff) {
                    o.patches.extend(parse_diff(&ch.path, &ch.diff));
                } else if ch.kind == "add" {
                    o.patches.extend(whole_file(&ch.path, &ch.diff, true));
                } else if ch.kind == "delete" {
                    o.patches.extend(whole_file(&ch.path, &ch.diff, false));
                }
                if ch.kind == "add" {
                    o.created = true;
                }
            }
            o.is_error = failed;
        }
        "mcpToolCall" => {
            o.text = texts(&content_items(arr(&it.result, "content")));
            if let Some(e) = &it.error {
                (o.is_error, o.text) = (true, e.clone());
            }
            o.is_error = o.is_error || failed;
        }
        "dynamicToolCall" => {
            o.text = texts(&it.content_items);
            o.is_error = failed || it.success == Some(false);
        }
        _ => o.is_error = failed,
    }
    o
}

/// The texts of content parts, a line each.
pub(crate) fn texts(cs: &[ContentItem]) -> String {
    let b: Vec<&str> = cs
        .iter()
        .filter(|c| !c.text.is_empty())
        .map(|c| c.text.as_str())
        .collect();
    b.join("\n")
}

/// A userMessage item as a user Message.
pub(crate) fn user_message(it: &ThreadItem) -> Message {
    let mut m = Message {
        role: "user".into(),
        id: it.id.clone(),
        ..Message::default()
    };
    for p in it.content.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        match s(p, "type") {
            "text" => m.parts.push(text_part(PartKind::Text, st(p, "text"))),
            "localImage" => m.parts.push(image_part(ImageData {
                path: st(p, "path"),
                ..ImageData::default()
            })),
            "image" => {
                let url = s(p, "url");
                let img = data_image(url).unwrap_or_else(|| ImageData {
                    path: url.to_string(),
                    ..ImageData::default()
                });
                m.parts.push(image_part(img));
            }
            _ => {}
        }
    }
    m
}

pub(crate) fn text_part(kind: PartKind, text: String) -> Part {
    Part {
        kind,
        text,
        ..Part::default()
    }
}

pub(crate) fn image_part(img: ImageData) -> Part {
    Part {
        kind: PartKind::Image,
        image: Some(Box::new(img)),
        ..Part::default()
    }
}

/// A reasoning item's summary and, when Codex sends it, its raw reasoning.
pub(crate) fn reasoning_text(it: &ThreadItem) -> String {
    let content: Vec<String> = it
        .content
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    let all: Vec<&str> = it
        .summary
        .iter()
        .chain(content.iter())
        .map(String::as_str)
        .collect();
    all.join("\n\n").trim().to_string()
}

/// A plan step's status in rush's words.
pub(crate) fn todo_status(s: &str) -> String {
    if s == "inProgress" {
        "in_progress".into()
    } else {
        s.into()
    }
}

/// A command Codex wrapped in a login shell: /bin/zsh -lc '…', or the same
/// in double quotes.
fn shell_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r#"^(?:\S*/)?(?:ba|z)?sh -l?c (?:'((?:[^']|'\\'')*)'|"((?:[^"\\]|\\.)*)")$"#,
        )
        .expect("shell pattern")
    })
}

/// What a command runs, out of the shell Codex runs it in.
pub(crate) fn script(cmd: &str) -> String {
    let Some(m) = shell_re().captures(cmd) else {
        return cmd.to_string();
    };
    let single = m.get(1).map_or("", |g| g.as_str());
    if !single.is_empty() || !cmd.contains('"') {
        return single.replace(r"'\''", "'");
    }
    dquoted(m.get(2).map_or("", |g| g.as_str()))
}

/// The escapes a shell undoes inside double quotes.
fn dquoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\'
            && let Some(&n) = it.peek()
            && matches!(n, '"' | '\\' | '$' | '`')
        {
            out.push(n);
            it.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// A file:// URL, as Codex records the images it views, as the path it
/// names; anything else as it is.
pub(crate) fn local_path(p: &str) -> String {
    let Some((scheme, rest)) = p.split_once(':') else {
        return p.to_string();
    };
    if !scheme.eq_ignore_ascii_case("file") {
        return p.to_string();
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let path = match rest.strip_prefix("//") {
        Some(auth) => match auth.find('/') {
            Some(i) => &auth[i..],
            None => "",
        },
        None => rest,
    };
    unescape(path).unwrap_or_else(|| p.to_string())
}

/// %xx escapes undone; None when one is malformed.
fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A data: URL's image.
pub(crate) fn data_image(url: &str) -> Option<ImageData> {
    let rest = url.strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    let media = head.strip_suffix(";base64")?;
    let b = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some(ImageData {
        media_type: media.to_string(),
        data: b,
        path: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(s: &str) -> ThreadItem {
        ThreadItem::read(s).expect("an item")
    }

    // codex_test.go:TestCallOf
    #[test]
    fn calls_of_items() {
        let cases: Vec<(&str, &str, Call)> = vec![
            (
                "read",
                r#"{"type":"commandExecution","id":"a","command":"cat x.go","cwd":"/w","commandActions":[{"type":"read","command":"cat x.go","name":"x.go","path":"/w/x.go"}]}"#,
                Call {
                    id: "a".into(),
                    name: "shell".into(),
                    kind: Kind::Read,
                    input: rush_core::tool::Input {
                        command: "cat x.go".into(),
                        cwd: "/w".into(),
                        path: "/w/x.go".into(),
                        ..Default::default()
                    },
                    ..Call::default()
                },
            ),
            (
                "write",
                r#"{"type":"fileChange","id":"b","changes":[{"path":"/w/n","kind":{"type":"add"},"diff":"hi\n"}]}"#,
                Call {
                    id: "b".into(),
                    name: "apply_patch".into(),
                    kind: Kind::Write,
                    input: rush_core::tool::Input {
                        path: "/w/n".into(),
                        content: "hi\n".into(),
                        ..Default::default()
                    },
                    ..Call::default()
                },
            ),
            (
                "move",
                r#"{"type":"fileChange","id":"c","changes":[{"path":"/w/a","kind":{"type":"update","move_path":"/w/b"},"diff":""}]}"#,
                Call {
                    id: "c".into(),
                    name: "apply_patch".into(),
                    kind: Kind::Move,
                    input: rush_core::tool::Input {
                        path: "/w/a".into(),
                        to: "/w/b".into(),
                        ..Default::default()
                    },
                    ..Call::default()
                },
            ),
            (
                "mcp",
                r#"{"type":"mcpToolCall","id":"d","server":"linear","tool":"get_issue","arguments":{"id":1}}"#,
                Call {
                    id: "d".into(),
                    name: "mcp__linear__get_issue".into(),
                    kind: Kind::Mcp,
                    input: rush_core::tool::Input {
                        server: "linear".into(),
                        tool: "get_issue".into(),
                        ..Default::default()
                    },
                    raw: Some(RawJson::new(r#"{"id":1}"#).unwrap()),
                    ..Call::default()
                },
            ),
            (
                "search",
                r#"{"type":"webSearch","id":"e","query":"","action":{"type":"search","query":"go generics"}}"#,
                Call {
                    id: "e".into(),
                    name: "web_search".into(),
                    kind: Kind::WebSearch,
                    input: rush_core::tool::Input {
                        query: "go generics".into(),
                        ..Default::default()
                    },
                    ..Call::default()
                },
            ),
        ];
        for (name, raw, want) in cases {
            let got = call_of(&item(raw), None);
            assert_eq!(got, Some(want), "{name}");
        }
    }

    // children_test.go:TestSubAgentActivitySpawn
    #[test]
    fn sub_agent_activity_spawn() {
        let it = ThreadItem {
            typ: "subAgentActivity".into(),
            id: "call_1".into(),
            kind: "started".into(),
            agent_thread_id: "kid".into(),
            agent_path: "/root/pong".into(),
            ..ThreadItem::default()
        };
        let c = call_of(&it, None).expect("a spawn");
        assert_eq!(
            (c.kind, c.input.child.as_str(), c.input.description.as_str()),
            (Kind::Subagent, "kid", "pong")
        );
        let done = ThreadItem {
            kind: "completed".into(),
            ..it
        };
        assert!(call_of(&done, None).is_none(), "completed read as a spawn");
    }

    // codex_test.go:TestScript
    #[test]
    fn scripts_out_of_their_shell() {
        for (input, want) in [
            (r"/bin/zsh -lc 'ls -la'", "ls -la"),
            (r"bash -c 'echo '\''hi'\'' && pwd'", "echo 'hi' && pwd"),
            (r"/bin/zsh -lc 'a' extra", r"/bin/zsh -lc 'a' extra"),
            ("go test ./...", "go test ./..."),
            (
                r#"/bin/zsh -lc "printf 'hi\\n' > a.txt""#,
                r"printf 'hi\n' > a.txt",
            ),
            (r#"/bin/zsh -lc "echo \"\$HOME\"""#, r#"echo "$HOME""#),
        ] {
            assert_eq!(script(input), want, "{input}");
        }
    }

    // items_test.go:TestLocalPath
    #[test]
    fn local_paths() {
        for (input, want) in [
            ("file:///tmp/a%20b/Dark.png", "/tmp/a b/Dark.png"),
            ("/tmp/Dark.png", "/tmp/Dark.png"),
            ("shots/Dark.png", "shots/Dark.png"),
        ] {
            assert_eq!(local_path(input), want, "{input}");
        }
    }

    #[test]
    fn outputs_of_items() {
        let it = item(
            r#"{"type":"commandExecution","id":"c1","command":"x","status":"declined","aggregatedOutput":null,"exitCode":null}"#,
        );
        let o = output_of(&it, None);
        assert!(
            o.is_error && o.text == "declined" && o.exit.is_none(),
            "{o:?}"
        );
        let it = item(
            r#"{"type":"mcpToolCall","id":"m","server":"s","tool":"t","result":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]},"error":null}"#,
        );
        assert_eq!(output_of(&it, None).text, "a\nb");
        let it = item(
            r#"{"type":"dynamicToolCall","id":"d","tool":"t","contentItems":[{"type":"inputText","text":"x"}],"success":false}"#,
        );
        let o = output_of(&it, None);
        assert!(o.is_error && o.text == "x");
        assert_eq!(
            data_image("data:image/png;base64,iVBORw==").map(|i| i.media_type),
            Some("image/png".into())
        );
        assert_eq!(path_base("/root/echoer"), "echoer");
        assert_eq!(path_base(""), ".");
    }
}
