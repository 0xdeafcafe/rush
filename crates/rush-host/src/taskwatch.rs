//! An agent's task list as its tool calls leave it, and when it has gone
//! stale: the agent works on, call after call, and the list the user
//! reads stays as it was. Rust-only, nothing of Go's: Claude Code's own
//! reminder ("The task tools haven't been used recently…") names no task
//! and says it may be ignored, and agents do; rush's names what the list
//! still says is in progress, as a notice at the agent's next tool call
//! (inbox::MAIN_NOTICE). The rule is a plain function over the session's
//! own messages, so it is tested by a table.

use std::collections::HashMap;

use rush_core::event::{Message, PartKind};
use rush_core::tool::{Call, Kind};
use rush_core::wire::RawJson;
use serde::{Deserialize, Serialize};

/// Tool calls an agent may make without touching a list that has work
/// open before it is told: more than reading into a task takes, fewer
/// than a task forgotten for a whole turn.
pub const AFTER: usize = 25;
/// And how many more before it is told again, the list still untouched.
pub const AGAIN: usize = 100;
/// The tasks a notice names, at most.
const NAMED: usize = 6;

#[derive(Debug, Serialize, Deserialize)]
struct Task {
    id: String,
    subject: String,
    /// pending, in_progress, completed
    status: String,
}

/// One session's list, and how long the agent has left it alone. Its
/// list is kept in work.json for the list's WORK column (work.rs,
/// Rust-only); what's being made and the count of calls start again
/// with each host.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Watch {
    tasks: Vec<Task>,
    /// The tasks being made, by their call: a task's id is only in its
    /// result ("Task #3 created").
    #[serde(skip)]
    making: HashMap<String, String>,
    /// The tool calls since the list was last written or read.
    #[serde(skip)]
    since: usize,
    /// The list changed since `take_changed` last asked.
    #[serde(skip)]
    changed: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct In {
    #[serde(rename = "taskId")]
    id: String,
    status: String,
    subject: String,
}

impl Watch {
    /// Takes in a message of the session's (a subagent's is none of its
    /// list's business): the notice to give the agent when this one's
    /// calls leave its list stale, naming what it says is going on.
    pub fn saw(&mut self, m: &Message) -> Option<String> {
        if !m.parent.is_empty() {
            return None;
        }
        let mut due = false;
        for p in &m.parts {
            match p.kind {
                PartKind::ToolCall if m.role == "assistant" => {
                    let Some(c) = &p.call else { continue };
                    if self.task_call(c) {
                        self.since = 0;
                        continue;
                    }
                    self.since += 1;
                    due |= self.since == AFTER
                        || self.since > AFTER && (self.since - AFTER).is_multiple_of(AGAIN);
                }
                PartKind::ToolResult => {
                    let Some(o) = &p.output else { continue };
                    let Some(subject) = self.making.remove(&o.call_id) else {
                        continue;
                    };
                    if o.is_error {
                        continue;
                    }
                    let id = created_id(&o.text)
                        .map(str::to_owned)
                        .unwrap_or_else(|| (self.tasks.len() + 1).to_string());
                    self.tasks.push(Task {
                        id,
                        subject,
                        status: "pending".into(),
                    });
                    self.changed = true;
                }
                _ => {}
            }
        }
        if due { self.note() } else { None }
    }

    /// Whether c is a call on the task list, taken in: the whole list
    /// (TodoWrite, another agent's plan), one task made or changed, or
    /// the list read.
    fn task_call(&mut self, c: &Call) -> bool {
        let raw = c.raw.as_ref().map_or("", RawJson::get);
        match c.name.as_str() {
            "TaskCreate" => {
                let inp: In = serde_json::from_str(raw).unwrap_or_default();
                self.making.insert(c.id.clone(), inp.subject);
            }
            "TaskUpdate" => {
                let inp: In = serde_json::from_str(raw).unwrap_or_default();
                if let Some(i) = self.tasks.iter().position(|t| t.id == inp.id) {
                    self.changed = true;
                    if inp.status == "deleted" {
                        self.tasks.remove(i);
                    } else {
                        let t = &mut self.tasks[i];
                        if !inp.status.is_empty() {
                            t.status = inp.status;
                        }
                        if !inp.subject.is_empty() {
                            t.subject = inp.subject;
                        }
                    }
                }
            }
            "TaskList" | "TaskGet" => {}
            _ if c.kind == Kind::Todo => {
                self.tasks = c
                    .input
                    .todos
                    .iter()
                    .enumerate()
                    .map(|(i, td)| Task {
                        id: (i + 1).to_string(),
                        subject: td.label.clone(),
                        status: td.status.clone(),
                    })
                    .collect();
                self.changed = true;
            }
            _ => return false,
        }
        true
    }

    /// Its list's tasks done, and in all.
    pub fn counts(&self) -> (usize, usize) {
        let done = self
            .tasks
            .iter()
            .filter(|t| t.status == "completed")
            .count();
        (done, self.tasks.len())
    }

    /// Whether the list changed since this was last asked.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// What to tell the agent of its list as it stands, None when nothing
    /// in it is open: a list all done, or none, is nothing to keep up.
    fn note(&self) -> Option<String> {
        let open: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.status != "completed")
            .collect();
        if open.is_empty() {
            return None;
        }
        let doing: Vec<&Task> = open
            .iter()
            .copied()
            .filter(|t| t.status == "in_progress")
            .collect();
        let (mut out, named) = if doing.is_empty() {
            (
                format!(
                    "Your task list hasn't changed in {} tool calls: nothing in it is in progress, and {} not done, starting with:\n",
                    self.since,
                    if open.len() == 1 {
                        "1 task is".to_string()
                    } else {
                        format!("{} tasks are", open.len())
                    }
                ),
                open,
            )
        } else {
            (
                format!(
                    "Your task list hasn't changed in {} tool calls and still says you are in the middle of:\n",
                    self.since
                ),
                doing,
            )
        };
        for t in named.iter().take(NAMED) {
            out.push_str(&format!("- {} (task {})\n", t.subject, t.id));
        }
        if named.len() > NAMED {
            out.push_str(&format!("- and {} more\n", named.len() - NAMED));
        }
        out.push_str(
            "The user reads this list to see what you are doing. Mark what is finished completed, put what you are really working on in progress, and delete what no longer applies. If the list is right as it is, carry on.",
        );
        Some(out)
    }
}

/// The id in "Task #N created" (rush-convo's created_task_id).
fn created_id(out: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(k) = out[from..].find("Task #") {
        let rest = &out[from + k + 6..];
        let n = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if n > 0 && rest[n..].starts_with(" created") {
            return Some(&rest[..n]);
        }
        from += k + 6;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::event::Part;
    use rush_core::tool::{Input, Output, TodoItem};

    fn call(id: &str, name: &str, raw: &str) -> Message {
        let c = Call {
            id: id.into(),
            name: name.into(),
            kind: if name == "TodoWrite" {
                Kind::Todo
            } else {
                Kind::default()
            },
            raw: (!raw.is_empty()).then(|| RawJson::new(raw).expect("json")),
            ..Call::default()
        };
        Message {
            role: "assistant".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(c)),
                ..Part::default()
            }],
            ..Message::default()
        }
    }

    fn result(id: &str, text: &str) -> Message {
        Message {
            role: "user".into(),
            parts: vec![Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output {
                    call_id: id.into(),
                    text: text.into(),
                    ..Output::default()
                })),
                ..Part::default()
            }],
            ..Message::default()
        }
    }

    /// n calls that aren't the list's: the notices they bring.
    fn work(w: &mut Watch, n: usize) -> Vec<String> {
        (0..n)
            .filter_map(|i| w.saw(&call(&format!("w{i}"), "Read", "")))
            .collect()
    }

    fn two_tasks(w: &mut Watch) {
        for (c, subject, n) in [("c1", "Port the lexer", 1), ("c2", "Write its tests", 2)] {
            w.saw(&call(
                c,
                "TaskCreate",
                &format!(r#"{{"subject":"{subject}"}}"#),
            ));
            w.saw(&result(
                c,
                &format!("Task #{n} created successfully: {subject}"),
            ));
        }
    }

    #[test]
    fn a_list_left_alone_is_told_once_then_again_much_later() {
        let mut w = Watch::default();
        two_tasks(&mut w);
        w.saw(&call(
            "u1",
            "TaskUpdate",
            r#"{"taskId":"1","status":"in_progress"}"#,
        ));
        assert_eq!(work(&mut w, AFTER - 1), Vec::<String>::new(), "not yet");
        let told = work(&mut w, 1);
        assert_eq!(told.len(), 1);
        assert!(
            told[0].starts_with(&format!(
                "Your task list hasn't changed in {AFTER} tool calls and still says you are in the middle of:\n- Port the lexer (task 1)\nThe user reads"
            )),
            "{}",
            told[0]
        );
        assert_eq!(
            work(&mut w, AGAIN - 1),
            Vec::<String>::new(),
            "once a spell"
        );
        assert_eq!(work(&mut w, 1).len(), 1, "and again, much later");
    }

    #[test]
    fn what_a_list_says_decides_what_is_told() {
        struct Case {
            name: &'static str,
            /// After two tasks are made: the calls on the list.
            calls: &'static [(&'static str, &'static str)],
            want: Option<&'static str>,
        }
        let cases = [
            Case {
                name: "nothing in progress, two not done",
                calls: &[],
                want: Some(
                    "nothing in it is in progress, and 2 tasks are not done, starting with:\n- Port the lexer (task 1)\n- Write its tests (task 2)\n",
                ),
            },
            Case {
                name: "all done",
                calls: &[
                    ("TaskUpdate", r#"{"taskId":"1","status":"completed"}"#),
                    ("TaskUpdate", r#"{"taskId":"2","status":"completed"}"#),
                ],
                want: None,
            },
            Case {
                name: "one done, one deleted",
                calls: &[
                    ("TaskUpdate", r#"{"taskId":"1","status":"completed"}"#),
                    ("TaskUpdate", r#"{"taskId":"2","status":"deleted"}"#),
                ],
                want: None,
            },
            Case {
                name: "renamed and in progress",
                calls: &[(
                    "TaskUpdate",
                    r#"{"taskId":"2","status":"in_progress","subject":"Test the lexer"}"#,
                )],
                want: Some("in the middle of:\n- Test the lexer (task 2)\nThe user"),
            },
        ];
        for c in cases {
            let mut w = Watch::default();
            two_tasks(&mut w);
            for (i, (name, raw)) in c.calls.iter().enumerate() {
                w.saw(&call(&format!("u{i}"), name, raw));
            }
            let told = work(&mut w, AFTER);
            match c.want {
                None => assert_eq!(told, Vec::<String>::new(), "{}", c.name),
                Some(want) => assert!(
                    told.len() == 1 && told[0].contains(want),
                    "{}: {told:?}",
                    c.name
                ),
            }
        }
    }

    #[test]
    fn touching_or_reading_the_list_starts_the_count_again() {
        for (name, raw) in [
            ("TaskUpdate", r#"{"taskId":"1","status":"in_progress"}"#),
            ("TaskList", "{}"),
        ] {
            let mut w = Watch::default();
            two_tasks(&mut w);
            assert!(work(&mut w, AFTER - 1).is_empty());
            w.saw(&call("t", name, raw));
            assert!(work(&mut w, AFTER - 1).is_empty(), "{name}");
            assert_eq!(work(&mut w, 1).len(), 1, "{name}");
        }
    }

    #[test]
    fn a_whole_list_written_at_once_and_a_subagents_calls() {
        let mut w = Watch::default();
        let mut m = call("t1", "TodoWrite", "");
        m.parts[0].call.as_mut().expect("its call").input = Input {
            todos: vec![
                TodoItem {
                    label: "Read it".into(),
                    status: "completed".into(),
                    ..TodoItem::default()
                },
                TodoItem {
                    label: "Fix it".into(),
                    status: "in_progress".into(),
                    ..TodoItem::default()
                },
            ],
            ..Input::default()
        };
        w.saw(&m);
        // A subagent's calls aren't the session's.
        for i in 0..AFTER * 2 {
            let mut sub = call(&format!("s{i}"), "Read", "");
            sub.parent = "agent-call".into();
            assert_eq!(w.saw(&sub), None);
        }
        let told = work(&mut w, AFTER);
        assert!(
            told.len() == 1
                && told[0].contains("- Fix it (task 2)\n")
                && !told[0].contains("Read it"),
            "{told:?}"
        );
    }
}
