//! Claude Code's tools as rush's own: what each does (its tool::Kind), a
//! call's input read into a tool::Call, a result read into a tool::Output,
//! and the words Claude has of its own for some of its tools (its
//! Describer). The only place Claude's tool names are read. Port of
//! internal/adapters/claude/claude/tools.go and doing.go.
//!
//! Each field of an input is read on its own, so one of an odd type is
//! left empty without losing the rest.

use rush_core::tool::{self, Call, Input, Kind, Output, Replace, Span, TodoItem};
use rush_core::wire::RawJson;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use crate::json::str_or_empty;

/// What a Claude Code tool does.
pub fn kind_of(name: &str) -> Kind {
    match kind(name) {
        Kind::Other => match name.strip_prefix("mcp__") {
            Some(rest) if rest.contains("__") => Kind::Mcp,
            _ => Kind::Other,
        },
        k => k,
    }
}

/// The kinds tools.go's table names; an MCP tool is Other until its name
/// is split.
fn kind(name: &str) -> Kind {
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
struct In<'a> {
    #[serde(borrow)]
    command: Option<&'a RawValue>,
    #[serde(borrow)]
    description: Option<&'a RawValue>,
    #[serde(borrow)]
    run_in_background: Option<&'a RawValue>,
    #[serde(borrow)]
    timeout: Option<&'a RawValue>,
    #[serde(borrow)]
    file_path: Option<&'a RawValue>,
    #[serde(borrow)]
    notebook_path: Option<&'a RawValue>,
    #[serde(borrow)]
    path: Option<&'a RawValue>,
    #[serde(borrow)]
    pattern: Option<&'a RawValue>,
    #[serde(borrow)]
    query: Option<&'a RawValue>,
    #[serde(borrow)]
    url: Option<&'a RawValue>,
    #[serde(borrow)]
    prompt: Option<&'a RawValue>,
    #[serde(borrow)]
    subagent_type: Option<&'a RawValue>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    #[serde(borrow)]
    old_string: Option<&'a RawValue>,
    #[serde(borrow)]
    new_string: Option<&'a RawValue>,
    #[serde(borrow)]
    replace_all: Option<&'a RawValue>,
    #[serde(borrow)]
    edits: Option<&'a RawValue>,
    #[serde(borrow)]
    offset: Option<&'a RawValue>,
    #[serde(borrow)]
    limit: Option<&'a RawValue>,
    #[serde(borrow)]
    todos: Option<&'a RawValue>,
    #[serde(borrow)]
    questions: Option<&'a RawValue>,
    #[serde(borrow)]
    skill: Option<&'a RawValue>,
    #[serde(borrow)]
    summary: Option<&'a RawValue>,
}

fn input(raw: &str) -> In<'_> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn s(r: Option<&RawValue>) -> String {
    str_or_empty(r).into_owned()
}

/// A field of another type than a string, or its zero value.
fn val<T: DeserializeOwned + Default>(r: Option<&RawValue>) -> T {
    r.and_then(|r| serde_json::from_str(r.get()).ok())
        .unwrap_or_default()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Edit {
    old_string: String,
    new_string: String,
    replace_all: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Todo {
    content: String,
    #[serde(rename = "activeForm")]
    active_form: String,
    status: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Question {
    question: String,
}

/// Reads one of Claude Code's tool calls into rush's own; its input (JSON,
/// None when it had none) is kept as the call's raw.
pub fn call(id: &str, name: &str, input: Option<&RawValue>) -> Call {
    let mut c = read(id, name, input.map_or("", RawValue::get));
    c.raw = input.map(|r| RawJson(r.to_owned()));
    c
}

/// A call read without keeping its input.
fn read(id: &str, name: &str, raw: &str) -> Call {
    let mut c = Call {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind(name),
        ..Call::default()
    };
    let inp = input(raw);
    let x: &mut Input = &mut c.input;
    x.description = s(inp.description);
    x.prompt = s(inp.prompt);
    match c.kind {
        Kind::Shell => {
            x.command = s(inp.command);
            x.background = val(inp.run_in_background);
            x.timeout = val(inp.timeout);
        }
        Kind::Read => {
            x.path = s(inp.file_path);
            x.offset = val(inp.offset);
            x.limit = val(inp.limit);
        }
        Kind::Edit => {
            x.path = s(inp.file_path);
            if name == "MultiEdit" {
                x.edits = val::<Vec<Edit>>(inp.edits)
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
                    old: s(inp.old_string),
                    new: s(inp.new_string),
                    all: val(inp.replace_all),
                }];
            }
        }
        Kind::Write => {
            x.path = s(inp.file_path);
            x.content = s(inp.content);
        }
        Kind::Notebook => x.path = s(inp.notebook_path),
        Kind::Search | Kind::Glob => {
            x.pattern = s(inp.pattern);
            x.path = s(inp.path);
        }
        Kind::Fetch => x.url = s(inp.url),
        Kind::WebSearch => x.query = s(inp.query),
        Kind::Subagent => x.agent = s(inp.subagent_type),
        Kind::Todo => {
            x.todos = val::<Vec<Todo>>(inp.todos)
                .into_iter()
                .map(|t| TodoItem {
                    label: t.content,
                    active: t.active_form,
                    status: t.status,
                })
                .collect();
        }
        Kind::Question => {
            if let Some(q) = val::<Vec<Question>>(inp.questions).into_iter().next() {
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
                x.command = s(inp.command);
                x.path = s(inp.file_path);
                x.pattern = s(inp.pattern);
                x.url = s(inp.url);
                x.query = s(inp.query);
            }
        }
        _ => {}
    }
    c
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Result {
    #[serde(rename = "type")]
    typ: String,
    stdout: String,
    stderr: String,
    #[serde(rename = "structuredPatch", with = "rush_core::wire::nullvec")]
    structured_patch: Vec<tool::Patch>,
    file: File,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct File {
    #[serde(rename = "numLines")]
    num_lines: i64,
    #[serde(rename = "startLine")]
    start_line: i64,
    #[serde(rename = "totalLines")]
    total_lines: i64,
}

/// Reads a tool result into rush's own: text is what Claude read, result
/// Claude Code's structured account of the run (its toolUseResult), kept
/// as the output's raw.
pub fn output(c: &Call, text: &str, is_error: bool, result: Option<&RawValue>) -> Output {
    let mut o = Output {
        call_id: c.id.clone(),
        text: text.to_owned(),
        is_error,
        raw: result.map(|r| RawJson(r.to_owned())),
        ..Output::default()
    };
    let r: Result = result
        .and_then(|r| serde_json::from_str(r.get()).ok())
        .unwrap_or_default();
    match c.kind {
        Kind::Shell => {
            o.stdout = r.stdout;
            o.stderr = r.stderr;
            // "Exit code N" in a success's output is just output.
            o.exit = Some(if is_error {
                exit_code_in(text).unwrap_or(-1)
            } else {
                0
            });
        }
        Kind::Edit | Kind::Write => {
            o.patches = r.structured_patch;
            o.created = r.typ == "create";
        }
        Kind::Read => {
            let f = r.file;
            if f.num_lines > 0 {
                o.lines = Some(Span {
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

/// The N of the first line reading "Exit code N" (or "Error: Exit code
/// N"), as Go's `(?m)^(?:Error: )?Exit code (\d+)`.
pub fn exit_code_in(s: &str) -> Option<i64> {
    s.split('\n').find_map(|l| {
        let l = l.strip_prefix("Error: ").unwrap_or(l);
        let rest = l.strip_prefix("Exit code ")?;
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        Some(rest[..n].parse().unwrap_or(i64::MAX))
    })
}

/// What a Claude Code tool call is doing, in words: Claude's own for the
/// tools it has them for, else tool::doing's. A command's own description
/// wins when Claude gave one.
pub fn doing(name: &str, raw: &str) -> String {
    match name {
        "BashOutput" | "TaskOutput" => "checking a background task".into(),
        "KillShell" | "TaskStop" => "stopping a background task".into(),
        "TaskCreate" | "TaskUpdate" => "updating its todo list".into(),
        "Skill" => match str_or_empty(input(raw).skill).as_ref() {
            "" => "using".into(),
            s => format!("using {s}"),
        },
        "ToolSearch" => "looking up tools".into(),
        "ScheduleWakeup" | "Monitor" => "waiting".into(),
        "SendMessage" => match str_or_empty(input(raw).summary).as_ref() {
            "" => "messaging".into(),
            s => format!("messaging {s}"),
        },
        "SubagentHandback" => "reporting back".into(),
        "ListAgents" => "listing agents".into(),
        "PushNotification" => "notifying you".into(),
        "EnterWorktree" => "entering a worktree".into(),
        _ => tool::doing(&read("", name, raw)),
    }
}

/// Claude's Describer: a call in its words, from its name and input.
pub fn describe(c: &Call) -> String {
    doing(&c.name, c.raw.as_ref().map_or("", RawJson::get))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(s: &str) -> Box<RawValue> {
        RawValue::from_string(s.to_owned()).unwrap()
    }

    // doing_test.go
    #[test]
    fn claude_doing() {
        for (tool, input, want) in [
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
            ("Skill", r#"{"skill":"loop"}"#, "using loop"),
            ("SendMessage", "{}", "messaging"),
            (
                "AskUserQuestion",
                r#"{"questions":[{"question":"Which one?\nmore"}]}"#,
                "Which one?",
            ),
            (
                "Bash",
                r#"{"command":"grep -e foo x"}"#,
                "searching for foo",
            ),
            ("Bash", r#"{"command":"rg -n"}"#, "searching the code"),
            // A field of the wrong type is left out, not the whole input.
            ("Bash", r#"{"command":"ls","timeout":"soon"}"#, "running ls"),
        ] {
            assert_eq!(doing(tool, input), want, "doing({tool}, {input})");
        }
    }

    #[test]
    fn reads_calls() {
        let c = call(
            "t1",
            "MultiEdit",
            Some(&raw(
                r#"{"file_path":"/a/x.go","edits":[{"old_string":"a","new_string":"b"},{"old_string":"c","new_string":"d","replace_all":true}]}"#,
            )),
        );
        assert_eq!(c.kind, Kind::Edit);
        assert_eq!(c.input.path, "/a/x.go");
        assert_eq!(c.input.edits.len(), 2);
        assert!(c.input.edits[1].all);
        assert!(c.raw.is_some());
        let c = call(
            "t2",
            "Bash",
            Some(&raw(
                r#"{"command":"go test ./...","run_in_background":true}"#,
            )),
        );
        assert_eq!(c.kind, Kind::Shell);
        assert_eq!(c.input.command, "go test ./...");
        assert!(c.input.background);
        let c = call("t3", "mcp__claude_ai_Linear__get_issue", Some(&raw("{}")));
        assert_eq!(
            (c.kind, c.input.server.as_str(), c.input.tool.as_str()),
            (Kind::Mcp, "Linear", "get_issue")
        );
        let c = call(
            "t4",
            "TodoWrite",
            Some(&raw(
                r#"{"todos":[{"content":"Test","activeForm":"Testing","status":"in_progress"}]}"#,
            )),
        );
        assert_eq!(c.kind, Kind::Todo);
        assert_eq!(c.input.todos[0].active, "Testing");
        assert_eq!(call("t5", "ScheduleWakeup", None).kind, Kind::Other);
        assert_eq!(kind_of("MultiEdit"), Kind::Edit);
        assert_eq!(kind_of("mcp__a__b"), Kind::Mcp);
        assert_eq!(kind_of("mcp__a"), Kind::Other);
        assert_eq!(kind_of("ScheduleWakeup"), Kind::Other);
    }

    #[test]
    fn reads_outputs() {
        let bash = call("t1", "Bash", Some(&raw(r#"{"command":"false"}"#)));
        let o = output(
            &bash,
            "Error: Exit code 2\nboom",
            true,
            Some(&raw(r#"{"stdout":"","stderr":"boom"}"#)),
        );
        assert_eq!((o.exit, o.stderr.as_str()), (Some(2), "boom"));
        assert!(o.raw.is_some());
        let o = output(&bash, "Exit code 3 is fine", false, None);
        assert_eq!(o.exit, Some(0));
        let o = output(&bash, "killed", true, None);
        assert_eq!(o.exit, Some(-1));
        let w = call("t2", "Write", Some(&raw(r#"{"file_path":"/a/new.go"}"#)));
        let o = output(
            &w,
            "",
            false,
            Some(&raw(
                r#"{"type":"create","structuredPatch":[{"oldStart":0,"oldLines":0,"newStart":1,"newLines":1,"lines":["+x"]}]}"#,
            )),
        );
        assert!(o.created);
        assert_eq!(o.patches[0].lines[0], "+x");
        let r = call("t3", "Read", Some(&raw(r#"{"file_path":"/a/x.go"}"#)));
        let o = output(
            &r,
            "",
            false,
            Some(&raw(
                r#"{"file":{"numLines":10,"startLine":5,"totalLines":90}}"#,
            )),
        );
        assert_eq!(
            o.lines,
            Some(Span {
                start: 5,
                count: 10,
                total: 90
            })
        );
    }

    #[test]
    fn exit_codes() {
        for (s, want) in [
            ("Exit code 2\nx", Some(2)),
            ("Error: Exit code 127", Some(127)),
            ("out\nExit code 1", Some(1)),
            ("  Exit code 1", None),
            ("Exit code x", None),
        ] {
            assert_eq!(exit_code_in(s), want, "{s}");
        }
    }
}
