//! A tool call as rush draws it, whichever agent made it, and Claude Code's
//! own tools read into it. Port of internal/agent/tool/tool.go,
//! internal/agent/tool/doing.go and internal/adapters/claude/claude/tools.go,
//! doing.go (Claude Code is the native agent: its tool names are what a
//! step's Tool and Input are in).

use serde::Deserialize;

use crate::text::{exit_code_in, one_line};

/// What a tool call does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Anything else: drawn by its name.
    #[default]
    Other,
    Shell,
    Read,
    Edit,
    Write,
    Delete,
    Move,
    /// Searches file contents (grep).
    Search,
    /// Finds files by name.
    Glob,
    /// Reads a web page.
    Fetch,
    WebSearch,
    Subagent,
    /// Changes the agent's todo list or plan.
    Todo,
    /// Asks the user something, with choices.
    Question,
    /// Leaves or enters plan mode with a plan.
    PlanMode,
    Mcp,
    Notebook,
    /// Thinks aloud, runs nothing.
    Think,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Other => "other",
            Kind::Shell => "shell",
            Kind::Read => "read",
            Kind::Edit => "edit",
            Kind::Write => "write",
            Kind::Delete => "delete",
            Kind::Move => "move",
            Kind::Search => "search",
            Kind::Glob => "glob",
            Kind::Fetch => "fetch",
            Kind::WebSearch => "websearch",
            Kind::Subagent => "subagent",
            Kind::Todo => "todo",
            Kind::Question => "question",
            Kind::PlanMode => "planmode",
            Kind::Mcp => "mcp",
            Kind::Notebook => "notebook",
            Kind::Think => "think",
        }
    }

    /// Whether a call of this kind changes files.
    pub fn changes(self) -> bool {
        matches!(
            self,
            Kind::Edit | Kind::Write | Kind::Delete | Kind::Move | Kind::Notebook
        )
    }
}

/// One tool call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Call {
    pub id: String,
    /// The agent's own name for the tool: "Bash", "shell", "mcp__linear__get_issue".
    pub name: String,
    pub kind: Kind,
    /// What the agent says the call is, when it says.
    pub title: String,
    pub input: Input,
}

/// What a call works on, as far as rush reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    pub command: String,
    pub cwd: String,
    pub background: bool,
    /// Shell, milliseconds.
    pub timeout: i64,
    /// Read, Edit, Write, Delete, Notebook; Search and Glob: where.
    pub path: String,
    /// Move.
    pub to: String,
    pub pattern: String,
    pub query: String,
    pub url: String,
    /// What the call is for, in the agent's words.
    pub description: String,
    /// Subagent; Fetch: what to look for.
    pub prompt: String,
    /// Subagent: its type.
    pub agent: String,
    pub child: String,
    /// Write: the whole file.
    pub content: String,
    pub edits: Vec<Replace>,
    /// Read: first line, 1-based; 0 is the start.
    pub offset: i64,
    /// Read: how many lines; 0 is all.
    pub limit: i64,
    pub server: String,
    pub tool: String,
    pub todos: Vec<TodoItem>,
}

/// One replacement in a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Replace {
    pub path: String,
    pub old: String,
    pub new: String,
    /// Every occurrence.
    pub all: bool,
}

/// One item of a todo list or plan as a Todo call sets it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TodoItem {
    pub label: String,
    /// What it says while it's being done: "Running the tests".
    pub active: String,
    /// pending, in_progress, completed
    pub status: String,
}

/// What a call returned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub call_id: String,
    /// Flattened to text, as the agent would read it.
    pub text: String,
    pub is_error: bool,
    pub stdout: String,
    pub stderr: String,
    /// Shell, when known.
    pub exit: Option<i32>,
    /// Edit and Write: what changed.
    pub patches: Vec<Patch>,
    /// Read: which lines it read.
    pub lines: Option<LineSpan>,
    /// Write: the file didn't exist before.
    pub created: bool,
}

/// One hunk of a change to a file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Patch {
    #[serde(default)]
    pub path: String,
    #[serde(rename = "oldStart", default)]
    pub old_start: i64,
    #[serde(rename = "oldLines", default)]
    pub old_lines: i64,
    #[serde(rename = "newStart", default)]
    pub new_start: i64,
    #[serde(rename = "newLines", default)]
    pub new_lines: i64,
    /// Each prefixed ' ', '-' or '+'.
    #[serde(default)]
    pub lines: Vec<String>,
}

/// A run of lines out of a file's total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineSpan {
    pub start: i64,
    pub count: i64,
    pub total: i64,
}

// ---- Claude Code's tools (claude/tools.go) ----

/// What a Claude Code tool does.
pub fn claude_kind_of(name: &str) -> Kind {
    let k = claude_kind(name);
    if k == Kind::Other
        && let Some(rest) = name.strip_prefix("mcp__")
        && rest.contains("__")
    {
        return Kind::Mcp;
    }
    k
}

fn claude_kind(name: &str) -> Kind {
    match name {
        "Bash" => Kind::Shell,
        "Read" => Kind::Read,
        "Edit" | "MultiEdit" => Kind::Edit,
        "Write" => Kind::Write,
        "NotebookEdit" => Kind::Notebook,
        "Grep" => Kind::Search,
        "Glob" => Kind::Glob,
        "WebFetch" => Kind::Fetch,
        "WebSearch" => Kind::WebSearch,
        "Task" | "Agent" => Kind::Subagent,
        "TodoWrite" => Kind::Todo,
        "AskUserQuestion" => Kind::Question,
        "ExitPlanMode" => Kind::PlanMode,
        _ => Kind::Other,
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaudeInput {
    command: String,
    description: String,
    run_in_background: bool,
    timeout: i64,
    file_path: String,
    notebook_path: String,
    path: String,
    pattern: String,
    query: String,
    url: String,
    prompt: String,
    subagent_type: String,
    content: String,
    old_string: String,
    new_string: String,
    replace_all: bool,
    edits: Vec<ClaudeEdit>,
    offset: i64,
    limit: i64,
    todos: Vec<ClaudeTodo>,
    questions: Vec<ClaudeQuestion>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaudeEdit {
    old_string: String,
    new_string: String,
    replace_all: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct ClaudeTodo {
    pub content: String,
    #[serde(rename = "activeForm")]
    pub active_form: String,
    pub status: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaudeQuestion {
    question: String,
}

/// Reads one of Claude Code's tool calls into rush's own. `input` is its
/// input as JSON ("" when it had none).
pub fn claude_call(id: &str, name: &str, input: &str) -> Call {
    let mut c = Call {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: claude_kind(name),
        ..Call::default()
    };
    let inp: ClaudeInput = serde_json::from_str(input).unwrap_or_default();
    let x = &mut c.input;
    x.description = inp.description;
    x.prompt = inp.prompt;
    match c.kind {
        Kind::Shell => {
            x.command = inp.command;
            x.background = inp.run_in_background;
            x.timeout = inp.timeout;
        }
        Kind::Read => {
            x.path = inp.file_path;
            x.offset = inp.offset;
            x.limit = inp.limit;
        }
        Kind::Edit => {
            x.path = inp.file_path;
            if name == "MultiEdit" {
                x.edits = inp
                    .edits
                    .into_iter()
                    .map(|e| Replace {
                        path: String::new(),
                        old: e.old_string,
                        new: e.new_string,
                        all: e.replace_all,
                    })
                    .collect();
            } else {
                x.edits = vec![Replace {
                    path: String::new(),
                    old: inp.old_string,
                    new: inp.new_string,
                    all: inp.replace_all,
                }];
            }
        }
        Kind::Write => {
            x.path = inp.file_path;
            x.content = inp.content;
        }
        Kind::Notebook => x.path = inp.notebook_path,
        Kind::Search | Kind::Glob => {
            x.pattern = inp.pattern;
            x.path = inp.path;
        }
        Kind::Fetch => x.url = inp.url,
        Kind::WebSearch => x.query = inp.query,
        Kind::Subagent => x.agent = inp.subagent_type,
        Kind::Todo => {
            x.todos = inp
                .todos
                .into_iter()
                .map(|t| TodoItem {
                    label: t.content,
                    active: t.active_form,
                    status: t.status,
                })
                .collect();
        }
        Kind::Question => {
            if let Some(q) = inp.questions.into_iter().next() {
                c.title = q.question;
            }
        }
        Kind::Other => {
            if let Some(rest) = name.strip_prefix("mcp__")
                && let Some((server, t)) = rest.split_once("__")
            {
                c.kind = Kind::Mcp;
                x.server = server
                    .strip_prefix("claude_ai_")
                    .unwrap_or(server)
                    .to_owned();
                x.tool = t.to_owned();
            } else {
                // Claude Code's own tools rush has no kind for: what they're
                // about, for drawing them by name.
                x.command = inp.command;
                x.path = inp.file_path;
                x.pattern = inp.pattern;
                x.url = inp.url;
                x.query = inp.query;
            }
        }
        _ => {}
    }
    c
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaudeResult {
    #[serde(rename = "type")]
    typ: String,
    stdout: String,
    stderr: String,
    #[serde(rename = "structuredPatch")]
    structured_patch: Vec<Patch>,
    file: ClaudeFile,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaudeFile {
    #[serde(rename = "numLines")]
    num_lines: i64,
    #[serde(rename = "startLine")]
    start_line: i64,
    #[serde(rename = "totalLines")]
    total_lines: i64,
}

/// Reads a tool result into rush's own: text is what Claude read, result
/// is Claude Code's structured account of the run (its toolUseResult), ""
/// when there is none.
pub fn claude_output(c: &Call, text: &str, is_error: bool, result: &str) -> Output {
    let mut o = Output {
        call_id: c.id.clone(),
        text: text.to_owned(),
        is_error,
        ..Output::default()
    };
    let r: ClaudeResult = if result.is_empty() {
        ClaudeResult::default()
    } else {
        serde_json::from_str(result).unwrap_or_default()
    };
    match c.kind {
        Kind::Shell => {
            o.stdout = r.stdout;
            o.stderr = r.stderr;
            let mut exit = 0;
            if is_error {
                // "Exit code N" in a success's output is just output.
                exit = exit_code_in(text).unwrap_or(-1);
            }
            o.exit = Some(exit);
        }
        Kind::Edit | Kind::Write => {
            o.patches = r.structured_patch;
            o.created = r.typ == "create";
        }
        Kind::Read => {
            let f = r.file;
            if f.num_lines > 0 {
                o.lines = Some(LineSpan {
                    start: f.start_line,
                    count: f.num_lines,
                    total: f.total_lines,
                });
            }
        }
        _ => {}
    }
    o
}

// ---- in words (tool/doing.go, claude/doing.go) ----

#[derive(Deserialize, Default)]
#[serde(default)]
struct SkillIn {
    skill: String,
    summary: String,
}

/// A Claude Code call in a few words, for the one line a list row has:
/// "running pnpm test", "reading view.go", "searching for PrettyModel".
pub fn claude_doing(name: &str, input: &str) -> String {
    match name {
        "BashOutput" | "TaskOutput" => return "checking a background task".into(),
        "KillShell" | "TaskStop" => return "stopping a background task".into(),
        "TaskCreate" | "TaskUpdate" => return "updating its todo list".into(),
        "Skill" => {
            let s: SkillIn = serde_json::from_str(input).unwrap_or_default();
            return if s.skill.is_empty() {
                "using".into()
            } else {
                format!("using {}", s.skill)
            };
        }
        "ToolSearch" => return "looking up tools".into(),
        "ScheduleWakeup" | "Monitor" => return "waiting".into(),
        "SendMessage" => {
            let s: SkillIn = serde_json::from_str(input).unwrap_or_default();
            return if s.summary.is_empty() {
                "messaging".into()
            } else {
                format!("messaging {}", s.summary)
            };
        }
        "SubagentHandback" => return "reporting back".into(),
        "ListAgents" => return "listing agents".into(),
        "PushNotification" => return "notifying you".into(),
        "EnterWorktree" => return "entering a worktree".into(),
        _ => {}
    }
    doing(&claude_call("", name, input))
}

/// What a call is doing, in words. A command's own description wins when
/// the agent gave one.
pub fn doing(c: &Call) -> String {
    let inp = &c.input;
    let with = |verb: &str, what: &str| -> String {
        let what = one_line(what);
        if what.is_empty() {
            verb.to_owned()
        } else {
            format!("{verb} {what}")
        }
    };
    match c.kind {
        Kind::Shell => {
            let d = one_line(&inp.description);
            if !d.is_empty() {
                return lower_first(d);
            }
            return shell_doing(one_line(&inp.command));
        }
        Kind::Read => return with("reading", base(&inp.path)),
        Kind::Edit | Kind::Notebook => return with("editing", base(&inp.path)),
        Kind::Write => return with("writing", base(&inp.path)),
        Kind::Delete => return with("deleting", base(&inp.path)),
        Kind::Move => return with("moving", base(&inp.path)),
        Kind::Search => return with("searching for", &inp.pattern),
        Kind::Glob => return with("finding", &inp.pattern),
        Kind::Fetch => {
            return match url_host(one_line(&inp.url)) {
                Some(h) => format!("reading {}", h.strip_prefix("www.").unwrap_or(h)),
                None => "reading a web page".into(),
            };
        }
        Kind::WebSearch => return with("searching the web for", &inp.query),
        Kind::Subagent => return with("subagent:", &lower_first(one_line(&inp.description))),
        Kind::Todo => return "updating its todo list".into(),
        Kind::Question => {
            let t = one_line(&c.title);
            return if t.is_empty() {
                "asking you".into()
            } else {
                t.to_owned()
            };
        }
        Kind::PlanMode => return "has a plan for you".into(),
        Kind::Think => return "thinking".into(),
        Kind::Mcp => return format!("{} · {}", inp.tool.replace('_', " "), inp.server),
        _ => {}
    }
    let t = one_line(&c.title);
    if !t.is_empty() {
        return t.to_owned();
    }
    for v in [
        &inp.description,
        &inp.command,
        &inp.path,
        &inp.pattern,
        &inp.url,
        &inp.query,
        &inp.prompt,
    ] {
        let v = one_line(v);
        if !v.is_empty() {
            return format!("{} {}", c.name, v);
        }
    }
    c.name.clone()
}

/// The last element of a path, as filepath.Base; "" stays "".
pub fn base(p: &str) -> &str {
    if p.is_empty() {
        return "";
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/";
    }
    match t.rfind('/') {
        Some(i) => &t[i + 1..],
        None => t,
    }
}

/// The host of an absolute URL, port and all; None when it has none.
fn url_host(u: &str) -> Option<&str> {
    let (scheme, rest) = u.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    let host = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    (!host.is_empty()).then_some(host)
}

/// What a shell command does: "searching for X" for grep and rg, else
/// "running" and the program it runs with its first argument, past any cd,
/// env or variables in front of it: "running pnpm test".
pub fn shell_doing(cmd: &str) -> String {
    for part in split_any(cmd) {
        let mut f: Vec<&str> = part.split_whitespace().collect();
        let mut skip = 0;
        while skip < f.len()
            && (f[skip].contains('=') || matches!(f[skip], "env" | "sudo" | "time"))
        {
            skip += 1;
        }
        f.drain(..skip);
        if f.is_empty() || f[0] == "cd" || f[0] == "export" {
            continue;
        }
        let prog = base(f[0]);
        if prog == "grep" || prog == "rg" {
            let pat = pattern(part);
            if !pat.is_empty() {
                return format!("searching for {pat}");
            }
            return "searching the code".into();
        }
        if f.len() > 1
            && !f[1].starts_with('-')
            && !f[1].contains(['"', '\'', '/', '$', '|', '<', '>'])
        {
            return format!("running {prog} {}", f[1]);
        }
        return format!("running {prog}");
    }
    "running a command".into()
}

/// Splits on "&&", ";" and "||", as strings.Split on each in turn.
fn split_any(s: &str) -> Vec<&str> {
    let mut parts = vec![s];
    for sep in ["&&", ";", "||"] {
        parts = parts.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    parts
}

/// grep's first argument that isn't a flag, quotes and all.
fn pattern(cmd: &str) -> String {
    let rest = cmd.trim();
    let mut rest = rest.split_once(' ').map_or("", |(_, r)| r).trim(); // the program
    while !rest.is_empty() {
        let q = rest.as_bytes()[0];
        if q == b'"' || q == b'\'' {
            return match rest[1..].find(q as char) {
                Some(end) => rest[1..1 + end].to_owned(),
                None => String::new(),
            };
        }
        let (word, mut after) = rest.split_once(' ').unwrap_or((rest, ""));
        if !word.starts_with('-') {
            return word.to_owned();
        }
        if matches!(word, "-e" | "-A" | "-B" | "-C" | "-m" | "-g" | "-t") {
            after = after.trim().split_once(' ').map_or("", |(_, r)| r); // its value
            if word == "-e" {
                return pattern(&format!(
                    "grep {}",
                    rest.strip_prefix("-e").unwrap_or(rest).trim()
                ));
            }
        }
        rest = after.trim();
    }
    String::new()
}

/// s with its first letter lower-cased, unless it opens an acronym ("PR").
pub fn lower_first(s: &str) -> String {
    let mut cs = s.chars();
    let Some(first) = cs.next() else {
        return String::new();
    };
    if let Some(second) = cs.next()
        && second.is_ascii_uppercase()
    {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    out.push(first.to_ascii_lowercase());
    out.push_str(&s[first.len_utf8()..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doing_words() {
        let call = |kind, name: &str, title: &str, input: Input| Call {
            kind,
            name: name.into(),
            title: title.into(),
            input,
            ..Call::default()
        };
        let cases = [
            (
                call(
                    Kind::Shell,
                    "",
                    "",
                    Input {
                        command: "cd x && pnpm test".into(),
                        ..Input::default()
                    },
                ),
                "running pnpm test",
            ),
            (
                call(
                    Kind::Shell,
                    "",
                    "",
                    Input {
                        command: "rg -n foo".into(),
                        description: "Find foo".into(),
                        ..Input::default()
                    },
                ),
                "find foo",
            ),
            (
                call(
                    Kind::Delete,
                    "",
                    "",
                    Input {
                        path: "/a/b.go".into(),
                        ..Input::default()
                    },
                ),
                "deleting b.go",
            ),
            (
                call(Kind::Question, "", "Which one?\nmore", Input::default()),
                "Which one?",
            ),
            (
                call(
                    Kind::Mcp,
                    "",
                    "",
                    Input {
                        server: "linear".into(),
                        tool: "get_issue".into(),
                        ..Input::default()
                    },
                ),
                "get issue · linear",
            ),
            (
                call(Kind::Other, "update_plan", "Planning", Input::default()),
                "Planning",
            ),
            (call(Kind::Other, "frob", "", Input::default()), "frob"),
        ];
        for (c, want) in cases {
            assert_eq!(doing(&c), want, "{c:?}");
        }
    }

    #[test]
    fn claude_doing_words() {
        let cases = [
            (
                "Bash",
                r#"{"command":"cd /Users/lw/Source/github.com/langwatch/langwatch && pnpm test"}"#,
                "running pnpm test",
            ),
            (
                "Bash",
                r#"{"command":"grep -rn \"agtools\\.\" internal","description":"Find agtools uses"}"#,
                "find agtools uses",
            ),
            (
                "Bash",
                r#"{"command":"GOFLAGS=-mod=mod go vet ./..."}"#,
                "running go vet",
            ),
            (
                "Bash",
                r#"{"command":"grep -n \"func PrettyModel\" -A12 internal/ui"}"#,
                "searching for func PrettyModel",
            ),
            ("Bash", r#"{"description":"PR checks"}"#, "PR checks"),
            (
                "Bash",
                r#"{"command":"grep -rn \"agtools\\.\\|mcp\" internal"}"#,
                r"searching for agtools\.\|mcp",
            ),
            ("Bash", r#"{"command":"cd x"}"#, "running a command"),
            ("Read", r#"{"file_path":"/a/b/view.go"}"#, "reading view.go"),
            (
                "Grep",
                r#"{"pattern":"func PrettyModel"}"#,
                "searching for func PrettyModel",
            ),
            (
                "WebFetch",
                r#"{"url":"https://www.example.com/x"}"#,
                "reading example.com",
            ),
            (
                "Agent",
                r#"{"description":"Explore the repo"}"#,
                "subagent: explore the repo",
            ),
            (
                "mcp__claude_ai_Notion__notion-search",
                "{}",
                "notion-search · Notion",
            ),
            ("Frobnicate", r#"{"query":"x"}"#, "Frobnicate x"),
        ];
        for (tool, input, want) in cases {
            assert_eq!(claude_doing(tool, input), want, "{tool} {input}");
        }
    }

    #[test]
    fn claude_calls() {
        let c = claude_call(
            "t1",
            "MultiEdit",
            r#"{"file_path":"/a/x.go","edits":[{"old_string":"a","new_string":"b"},{"old_string":"c","new_string":"d","replace_all":true}]}"#,
        );
        assert_eq!(c.kind, Kind::Edit);
        assert_eq!(c.input.path, "/a/x.go");
        assert_eq!(c.input.edits.len(), 2);
        assert!(c.input.edits[1].all);
        let c = claude_call(
            "t2",
            "Bash",
            r#"{"command":"go test ./...","run_in_background":true}"#,
        );
        assert_eq!(c.kind, Kind::Shell);
        assert_eq!(c.input.command, "go test ./...");
        assert!(c.input.background);
        let c = claude_call("t3", "mcp__claude_ai_Linear__get_issue", "{}");
        assert_eq!(
            (c.kind, c.input.server.as_str(), c.input.tool.as_str()),
            (Kind::Mcp, "Linear", "get_issue")
        );
        let c = claude_call(
            "t4",
            "TodoWrite",
            r#"{"todos":[{"content":"Test","activeForm":"Testing","status":"in_progress"}]}"#,
        );
        assert_eq!(c.kind, Kind::Todo);
        assert_eq!(c.input.todos[0].active, "Testing");
        assert_eq!(claude_call("t5", "ScheduleWakeup", "{}").kind, Kind::Other);
        assert_eq!(claude_kind_of("mcp__a__b"), Kind::Mcp);
        assert_eq!(claude_kind_of("mcp__a"), Kind::Other);
    }

    #[test]
    fn claude_outputs() {
        let bash = claude_call("t1", "Bash", r#"{"command":"false"}"#);
        let o = claude_output(
            &bash,
            "Error: Exit code 2\nboom",
            true,
            r#"{"stdout":"","stderr":"boom"}"#,
        );
        assert_eq!((o.exit, o.stderr.as_str()), (Some(2), "boom"));
        let o = claude_output(&bash, "Exit code 3 is fine", false, "");
        assert_eq!(o.exit, Some(0));
        let w = claude_call("t2", "Write", r#"{"file_path":"/a/new.go"}"#);
        let o = claude_output(
            &w,
            "",
            false,
            r#"{"type":"create","structuredPatch":[{"oldStart":0,"oldLines":0,"newStart":1,"newLines":1,"lines":["+x"]}]}"#,
        );
        assert!(o.created);
        assert_eq!(o.patches[0].lines[0], "+x");
        let r = claude_call("t3", "Read", r#"{"file_path":"/a/x.go"}"#);
        let o = claude_output(
            &r,
            "",
            false,
            r#"{"file":{"numLines":10,"startLine":5,"totalLines":90}}"#,
        );
        assert_eq!(
            o.lines,
            Some(LineSpan {
                start: 5,
                count: 10,
                total: 90
            })
        );
    }
}
