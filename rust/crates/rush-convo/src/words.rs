//! A step keeps its call in Claude Code's words (Step::tool, Step::input,
//! Step::result) beside rush's own, for what still reads those: another
//! agent's call is put as the Claude tool of its kind. Port of
//! internal/convo/words.go.

use serde_json::{Map, Value, json};

use crate::text::exit_code_in;
use crate::tool::{Call, Kind, Output, TodoItem, claude_kind_of};

/// A call as a step's tool and input: Claude Code's own as it sent them
/// (`raw`, its input JSON), or another agent's as the Claude tool of its
/// kind.
pub fn step_tool(c: &Call, raw: &str) -> (String, String) {
    if claudes(c, raw) {
        return (c.name.clone(), raw.to_owned());
    }
    claude_tool(c, raw)
}

/// Whether a call is Claude Code's own: one of its tools, of that tool's
/// kind, with the input it sent.
pub fn claudes(c: &Call, raw: &str) -> bool {
    raw.starts_with('{') && claude_kind_of(&c.name) == c.kind
}

fn claude_name(k: Kind) -> Option<&'static str> {
    Some(match k {
        Kind::Shell => "Bash",
        Kind::Read => "Read",
        Kind::Edit => "Edit",
        Kind::Write => "Write",
        Kind::Search => "Grep",
        Kind::Glob => "Glob",
        Kind::Fetch => "WebFetch",
        Kind::WebSearch => "WebSearch",
        Kind::Subagent => "Agent",
        Kind::Todo => "TodoWrite",
        Kind::Question => "AskUserQuestion",
        Kind::PlanMode => "ExitPlanMode",
        Kind::Notebook => "NotebookEdit",
        _ => return None,
    })
}

/// A call as the Claude tool of its kind, with that tool's input; a call
/// of no Claude kind keeps its name and input.
pub fn claude_tool(c: &Call, raw: &str) -> (String, String) {
    let inp = &c.input;
    let mut name = match (c.kind, claude_name(c.kind)) {
        (Kind::Mcp, _) => format!("mcp__{}__{}", inp.server, inp.tool),
        (_, Some(n)) => n.to_owned(),
        (_, None) if c.name.is_empty() => c.kind.as_str().to_owned(),
        (_, None) => c.name.clone(),
    };
    let v = match c.kind {
        Kind::Shell => {
            json!({"command": inp.command, "description": inp.description, "run_in_background": inp.background})
        }
        Kind::Read => {
            let mut m = Map::new();
            m.insert("file_path".into(), json!(inp.path));
            if inp.offset > 0 {
                m.insert("offset".into(), json!(inp.offset));
            }
            if inp.limit > 0 {
                m.insert("limit".into(), json!(inp.limit));
            }
            Value::Object(m)
        }
        Kind::Edit => {
            let mut m = Map::new();
            m.insert("file_path".into(), json!(inp.path));
            match inp.edits.len() {
                0 => {}
                1 => {
                    let e = &inp.edits[0];
                    m.insert("old_string".into(), json!(e.old));
                    m.insert("new_string".into(), json!(e.new));
                    m.insert("replace_all".into(), json!(e.all));
                }
                _ => {
                    name = "MultiEdit".into();
                    let edits: Vec<Value> = inp
                        .edits
                        .iter()
                        .map(|e| json!({"old_string": e.old, "new_string": e.new, "replace_all": e.all}))
                        .collect();
                    m.insert("edits".into(), Value::Array(edits));
                }
            }
            Value::Object(m)
        }
        Kind::Write => json!({"file_path": inp.path, "content": inp.content}),
        Kind::Notebook => json!({"notebook_path": inp.path}),
        Kind::Search | Kind::Glob => json!({"pattern": inp.pattern, "path": inp.path}),
        Kind::Fetch => json!({"url": inp.url, "prompt": inp.prompt}),
        Kind::WebSearch => json!({"query": inp.query}),
        Kind::Subagent => {
            json!({"description": inp.description, "prompt": inp.prompt, "subagent_type": inp.agent})
        }
        Kind::Todo => json!({"todos": claude_todos(&inp.todos)}),
        Kind::Question => json!({"questions": [{"question": c.title}]}),
        _ => {
            if raw.starts_with('{') {
                return (name, raw.to_owned());
            }
            let mut m = Map::new();
            for (k, x) in [
                ("description", &inp.description),
                ("command", &inp.command),
                ("file_path", &inp.path),
                ("pattern", &inp.pattern),
                ("url", &inp.url),
                ("query", &inp.query),
                ("prompt", &inp.prompt),
            ] {
                if !x.is_empty() {
                    m.insert(k.into(), json!(x));
                }
            }
            if m.is_empty() && !c.title.is_empty() {
                m.insert("description".into(), json!(c.title));
            }
            Value::Object(m)
        }
    };
    (name, v.to_string())
}

/// An output as Claude Code's structured account of a run; "" when it
/// has nothing to say.
pub fn claude_result(o: &Output) -> String {
    let mut v = Map::new();
    if !o.stdout.is_empty() || !o.stderr.is_empty() {
        v.insert("stdout".into(), json!(o.stdout));
        v.insert("stderr".into(), json!(o.stderr));
    }
    if !o.patches.is_empty() {
        let ps: Vec<Value> = o
            .patches
            .iter()
            .map(|p| {
                let mut m = Map::new();
                if !p.path.is_empty() {
                    m.insert("path".into(), json!(p.path));
                }
                m.insert("oldStart".into(), json!(p.old_start));
                m.insert("oldLines".into(), json!(p.old_lines));
                m.insert("newStart".into(), json!(p.new_start));
                m.insert("newLines".into(), json!(p.new_lines));
                m.insert("lines".into(), json!(p.lines));
                Value::Object(m)
            })
            .collect();
        v.insert("structuredPatch".into(), Value::Array(ps));
    }
    if o.created {
        v.insert("type".into(), json!("create"));
    }
    if let Some(l) = o.lines {
        v.insert(
            "file".into(),
            json!({"numLines": l.count, "startLine": l.start, "totalLines": l.total}),
        );
    }
    if v.is_empty() {
        return String::new();
    }
    Value::Object(v).to_string()
}

/// What a call returned, saying how a command exited when it failed, as
/// Claude Code's text does.
pub fn result_text(o: &Output) -> String {
    let mut text = o.text.clone();
    if text.is_empty() && (!o.stdout.is_empty() || !o.stderr.is_empty()) {
        text = format!("{}\n{}", o.stdout, o.stderr)
            .trim_end_matches('\n')
            .to_owned();
    }
    if o.is_error
        && let Some(exit) = o.exit
        && exit > 0
        && exit_code_in(&text).is_none()
    {
        text = format!("Exit code {exit}\n{text}");
    }
    text
}

/// A plan as TodoWrite's todos.
pub fn claude_todos(todos: &[TodoItem]) -> Vec<Value> {
    todos
        .iter()
        .map(|t| json!({"content": t.label, "activeForm": t.active, "status": t.status}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Input, Replace, claude_call};

    #[test]
    fn claude_words_for_other_agents() {
        let shell = Call {
            name: "shell".into(),
            kind: Kind::Shell,
            input: Input {
                command: "ls".into(),
                ..Input::default()
            },
            ..Call::default()
        };
        assert_eq!(
            step_tool(&shell, r#"{"cmd":["ls"]}"#),
            (
                "Bash".into(),
                r#"{"command":"ls","description":"","run_in_background":false}"#.into()
            )
        );
        let multi = Call {
            name: "apply_patch".into(),
            kind: Kind::Edit,
            input: Input {
                path: "/a".into(),
                edits: vec![Replace::default(), Replace::default()],
                ..Input::default()
            },
            ..Call::default()
        };
        assert_eq!(step_tool(&multi, "").0, "MultiEdit");
        let own = claude_call("1", "Bash", r#"{"command":"ls"}"#);
        assert_eq!(
            step_tool(&own, r#"{"command":"ls"}"#).1,
            r#"{"command":"ls"}"#
        );
        let other = Call {
            name: "frob".into(),
            title: "Frobbing".into(),
            ..Call::default()
        };
        assert_eq!(
            step_tool(&other, ""),
            ("frob".into(), r#"{"description":"Frobbing"}"#.into())
        );
    }

    #[test]
    fn result_texts() {
        let o = Output {
            stdout: "out".into(),
            stderr: "err\n".into(),
            is_error: true,
            exit: Some(2),
            ..Output::default()
        };
        assert_eq!(result_text(&o), "Exit code 2\nout\nerr");
        let o = Output {
            text: "Exit code 2\nx".into(),
            is_error: true,
            exit: Some(2),
            ..Output::default()
        };
        assert_eq!(result_text(&o), "Exit code 2\nx");
        assert_eq!(claude_result(&Output::default()), "");
        let o = Output {
            created: true,
            ..Output::default()
        };
        assert_eq!(claude_result(&o), r#"{"type":"create"}"#);
    }
}
