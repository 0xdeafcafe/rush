//! Kimi Code 2's saved sessions: their state files, and the wire journal
//! each agent of a session keeps, read back as rush's events. Port of
//! internal/adapters/acp/kimi_wire.go:1-247.

use std::path::Path;
use std::time::Duration;

use rush_core::adapter::{Kind, Profile, Session};
use rush_core::event::{self, Event, PartKind, TaskKind};
use rush_core::time::Ms;
use rush_core::tool;
use rush_core::usage::TokenUsage;
use rush_core::wire::RawJson;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::history::{HistoryError, Lines, Turns, content_parts, read_tail, text_part};
use crate::update::read_input;
use crate::util::{glob, mod_ms, path_str};

/// Kimi Code 2's sessions, from their state files. No agent process or
/// model request is needed, and a deleted session can't linger through a
/// stale index entry.
pub(crate) fn code_past(kind: &Kind, p: &Profile) -> Vec<Session> {
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct State {
        id: String,
        cwd: String,
        title: String,
        last_prompt: String,
        updated_at: i64,
    }
    let mut out = Vec::new();
    for path in glob(
        Path::new(&p.dir),
        &["sessions", "wd_*", "session_*", "state.json"],
    ) {
        let Ok(b) = std::fs::read(&path) else {
            continue;
        };
        let Ok(state) = serde_json::from_slice::<State>(&b) else {
            continue;
        };
        if state.id.is_empty() || state.cwd.is_empty() {
            continue;
        }
        let dir = path.parent().unwrap_or(Path::new(""));
        let transcript = dir.join("agents").join("main").join("wire.jsonl");
        let Ok(st) = std::fs::metadata(&transcript) else {
            continue;
        };
        if st.len() == 0 {
            continue;
        }
        let mut name = state.title.clone();
        if name.is_empty() {
            name = state
                .last_prompt
                .split('\n')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
        }
        if name.is_empty() {
            name = state.id.clone();
        }
        let at = if state.updated_at > 0 {
            state.updated_at
        } else {
            mod_ms(&st)
        };
        out.push(Session {
            kind: kind.clone(),
            profile: p.clone(),
            id: state.id,
            name,
            cwd: state.cwd,
            transcript: path_str(&transcript),
            state: "done".into(),
            updated_at: at,
            ..Session::default()
        });
    }
    out
}

/// The agent whose records a wire at path holds: main's, or a subagent's
/// (agents/agent-N).
pub(crate) fn own_of(path: &Path) -> String {
    let dir = path.parent().unwrap_or(Path::new(""));
    if dir
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|n| n == "agents")
        && let Some(n) = dir.file_name()
    {
        return n.to_string_lossy().into_owned();
    }
    "main".into()
}

/// A wire's lines, for the agent own: main's, or a subagent's.
pub(crate) fn wire_lines(path: &Path, before: Ms) -> WireLines {
    WireLines::new(&own_of(path), before)
}

pub(crate) fn wire_boxed(path: &Path) -> Box<dyn Lines> {
    Box::new(wire_lines(path, 0))
}

/// A wire's whole conversation before `before`, or its end within most
/// bytes when most is set; the bool is whether that left some out.
pub(crate) fn read_kimi_wire_file(
    path: &str,
    most: i64,
    before: Ms,
) -> Result<(Vec<Event>, bool), HistoryError> {
    read_tail(
        Path::new(path),
        most,
        &mut wire_lines(Path::new(path), before),
    )
}

/// A wire's main conversation from r.
#[cfg(test)]
pub(crate) fn read_kimi_wire(
    r: impl std::io::Read,
    before: Ms,
) -> Result<Vec<Event>, HistoryError> {
    crate::history::read_all(r, &mut WireLines::new("main", before))
}

/// The wire journal's records, read as events. It records accepted
/// prompts, context messages and loop events; only context messages and
/// loop events are shown, since replaying prompt.accepted as well would
/// show every user message twice. Internal context injections are marked
/// so they don't read as the user's.
pub(crate) struct WireLines {
    own: String,
    before: Ms,
    tokens: TokenUsage,
    model: String,
    n: usize,
    turns: Turns,
}

impl WireLines {
    pub fn new(own: &str, before: Ms) -> WireLines {
        WireLines {
            own: own.into(),
            before,
            tokens: TokenUsage::default(),
            model: String::new(),
            n: 0,
            turns: Turns::default(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Origin {
    kind: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireMessage<'a> {
    id: String,
    role: String,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    origin: Origin,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WirePart {
    #[serde(rename = "type")]
    kind: String,
    text: String,
    think: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct WireResult<'a> {
    #[serde(borrow)]
    output: Option<&'a RawValue>,
    is_error: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct WireEvent<'a> {
    #[serde(rename = "type")]
    kind: String,
    uuid: String,
    tool_call_id: String,
    name: String,
    #[serde(borrow)]
    args: Option<&'a RawValue>,
    part: WirePart,
    #[serde(borrow)]
    result: WireResult<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct WireUsage {
    input_other: i64,
    output: i64,
    input_cache_read: i64,
    input_cache_creation: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireError {
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct WireInfo {
    task_id: String,
    description: String,
    status: String,
    kind: String,
    subagent_type: String,
    parent_tool_call_id: String,
    detached: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Record<'a> {
    #[serde(rename = "type")]
    kind: String,
    agent_id: String,
    model_alias: String,
    reason: String,
    time: i64,
    duration_ms: i64,
    tokens_before: i64,
    tokens_after: i64,
    #[serde(borrow)]
    message: WireMessage<'a>,
    #[serde(borrow)]
    event: WireEvent<'a>,
    usage: WireUsage,
    error: WireError,
    info: WireInfo,
}

impl Lines for WireLines {
    fn line(&mut self, b: &[u8], partial: bool, out: &mut Vec<Event>) -> Result<(), HistoryError> {
        self.n += 1;
        let rec: Record = match serde_json::from_slice(b) {
            Ok(r) => r,
            // A live session's last line is often half-written.
            Err(_) if partial => return Ok(()),
            Err(e) => return Err(HistoryError(format!("kimi wire line {}: {e}", self.n))),
        };
        if !rec.agent_id.is_empty() && rec.agent_id != self.own {
            return Ok(());
        }
        if self.before != 0 && rec.time > 0 && rec.time >= self.before {
            return Ok(());
        }
        match rec.kind.as_str() {
            "profile.bind" | "config.update" => {
                if !rec.model_alias.is_empty() {
                    self.model = rec.model_alias;
                }
            }
            "context.append_message" => {
                let m = rec.message;
                if m.role != "user" {
                    return Ok(());
                }
                let parts = content_parts(m.content);
                if !parts.is_empty() {
                    let injected = !m.origin.kind.is_empty() && m.origin.kind != "user";
                    self.turns.push(
                        Event::Message(event::Message {
                            id: m.id,
                            role: "user".into(),
                            parts,
                            injected,
                            ..event::Message::default()
                        }),
                        out,
                    );
                }
            }
            "context.append_loop_event" => {
                let e = rec.event;
                let mut msg = event::Message {
                    id: e.uuid,
                    role: "assistant".into(),
                    model: self.model.clone(),
                    ..event::Message::default()
                };
                match e.kind.as_str() {
                    "content.part" => match e.part.kind.as_str() {
                        "text" => msg.parts = vec![text_part(PartKind::Text, e.part.text)],
                        "think" => msg.parts = vec![text_part(PartKind::Thinking, e.part.think)],
                        _ => {}
                    },
                    "tool.call" => {
                        let args = e.args.map_or("", RawValue::get);
                        let mut c = kimi_call(&e.tool_call_id, &e.name, args);
                        if c.kind == tool::Kind::Subagent {
                            c.input.child = c.id.clone(); // its run, found by find_child
                        }
                        msg.parts = vec![event::Part {
                            kind: PartKind::ToolCall,
                            call: Some(Box::new(c)),
                            ..event::Part::default()
                        }];
                    }
                    "tool.result" => {
                        let raw = e.result.output.map_or("", RawValue::get);
                        let body =
                            serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_string());
                        msg.role = "user".into();
                        msg.parts = vec![event::Part {
                            kind: PartKind::ToolResult,
                            output: Some(Box::new(tool::Output {
                                call_id: e.tool_call_id,
                                text: body,
                                is_error: e.result.is_error,
                                ..tool::Output::default()
                            })),
                            ..event::Part::default()
                        }];
                    }
                    _ => {}
                }
                if !msg.parts.is_empty() {
                    self.turns.push(Event::Message(msg), out);
                }
            }
            "usage.record" => {
                let u = rec.usage;
                self.tokens.add(TokenUsage {
                    input: u.input_other,
                    output: u.output,
                    cache_read: u.input_cache_read,
                    cache_write_5m: u.input_cache_creation,
                    ..TokenUsage::default()
                });
            }
            "turn.ended" => {
                let reason = match rec.reason.as_str() {
                    "cancelled" => "interrupted",
                    "failed" => "error",
                    _ => "done",
                };
                let end = event::TurnEnd {
                    reason: reason.into(),
                    err: rec.error.message,
                    tokens: std::mem::take(&mut self.tokens),
                    duration: Duration::from_millis(rec.duration_ms.max(0) as u64),
                    turns: 1,
                    ..event::TurnEnd::default()
                };
                self.turns.push(Event::TurnEnd(end), out);
            }
            // Work beside the turn: a background shell ("process") or
            // subagent.
            "task.terminated" => {
                let t = rec.info;
                self.turns.push(
                    Event::TaskDone(event::TaskDone {
                        id: t.task_id,
                        call_id: t.parent_tool_call_id,
                        status: t.status,
                        ..event::TaskDone::default()
                    }),
                    out,
                );
            }
            "task.started" => {
                let t = rec.info;
                let kind = if t.kind == "agent" {
                    TaskKind::Subagent
                } else {
                    TaskKind::Shell
                };
                self.turns.push(
                    Event::TaskStarted(event::TaskStarted {
                        id: t.task_id,
                        call_id: t.parent_tool_call_id,
                        kind,
                        label: t.description,
                        agent: t.subagent_type,
                        background: t.detached,
                    }),
                    out,
                );
            }
            "context.apply_compaction" => {
                self.turns.push(
                    Event::Compacted(event::Compacted {
                        before: rec.tokens_before,
                        after: rec.tokens_after,
                        ..event::Compacted::default()
                    }),
                    out,
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut Vec<Event>) {
        self.turns.finish(out);
    }
}

/// What Kimi's tools do. Current Kimi shares these tool inputs with Claude;
/// older spellings are kept for migrated sessions, and Vibe's beside them.
/// Unknown tools keep their raw input.
pub(crate) fn kimi_tool_kind(name: &str) -> tool::Kind {
    use tool::Kind::*;
    match name {
        "Bash" | "Shell" => Shell,
        "Read" | "ReadFile" => Read,
        "Write" | "WriteFile" => Write,
        "Edit" | "StrReplaceFile" => Edit,
        "Grep" => Search,
        "Glob" => Glob,
        "Agent" | "Task" => Subagent,
        "AskUserQuestion" => Question,
        "EnterPlanMode" | "ExitPlanMode" => PlanMode,
        "FetchURL" => Fetch,
        "WebSearch" => WebSearch,
        // The older Python Kimi's, and Vibe's.
        "SearchWeb" => WebSearch,
        "Think" => Think,
        "bash" => Shell,
        "read_file" => Read,
        "write_file" => Write,
        "grep" => Search,
        "web_fetch" => Fetch,
        "web_search" => WebSearch,
        "task" => Subagent,
        _ => Other,
    }
}

/// A Kimi (or Vibe) tool call, its input as sent.
pub(crate) fn kimi_call(id: &str, name: &str, raw: &str) -> tool::Call {
    let kind = kimi_tool_kind(name);
    let raw = RawJson::new(raw).ok();
    tool::Call {
        id: id.into(),
        name: name.into(),
        kind,
        input: read_input(kind, raw.as_ref(), &[], &[]),
        raw,
        ..tool::Call::default()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const FIXTURE: &str = r#"{"type":"profile.bind","agentId":"main","modelAlias":"kimi-code/k3","time":1000}
{"type":"prompt.accepted","agentId":"main","content":[{"type":"text","text":"question"}],"time":2000}
{"type":"turn.prompt","agentId":"main","input":[{"type":"text","text":"question"}],"time":2000}
{"type":"context.append_message","agentId":"main","message":{"id":"u1","role":"user","origin":{"kind":"user"},"content":[{"type":"text","text":"question"}]},"time":2000}
{"type":"context.append_loop_event","agentId":"main","event":{"type":"content.part","uuid":"thinking","part":{"type":"think","think":"checking"}},"time":2100}
{"type":"context.append_loop_event","agentId":"main","event":{"type":"tool.call","uuid":"call","toolCallId":"c1","name":"ReadFile","args":{"path":"file.go"}},"time":2200}
{"type":"context.append_loop_event","agentId":"main","event":{"type":"tool.result","toolCallId":"c1","result":{"output":"file missing","isError":true}},"time":2300}
{"type":"context.append_message","agentId":"main","message":{"id":"injection","role":"user","origin":{"kind":"task"},"content":[{"type":"text","text":"background report"}]},"time":2400}
{"type":"context.append_loop_event","agentId":"agent-1","event":{"type":"content.part","uuid":"child","part":{"type":"text","text":"child output"}},"time":2450}
{"type":"context.append_loop_event","agentId":"main","event":{"type":"content.part","uuid":"answer","part":{"type":"text","text":"answer"}},"time":2500}
{"type":"usage.record","agentId":"main","usage":{"inputOther":10,"output":20,"inputCacheRead":30,"inputCacheCreation":40},"time":2600}
{"type":"turn.ended","agentId":"main","reason":"completed","durationMs":700,"time":2700}
{"type":"context.apply_compaction","agentId":"main","tokensBefore":100,"tokensAfter":20,"time":2800}
{"type":"context.append_message","agentId":"main","message":{"id":"u2","role":"user","origin":{"kind":"user"},"content":[{"type":"text","text":"later question"}]},"time":3000}
{"type":"turn.ended","agentId":"main","reason":"cancelled","time":4000}
"#;

    // kimi_wire_test.go: TestKimiWireHistory
    #[test]
    fn wire_history() {
        let evs = read_kimi_wire(FIXTURE.as_bytes(), 0).unwrap();
        let (mut prompts, mut injected, mut answers, mut ended) = (0, 0, 0, 0);
        for ev in &evs {
            match ev {
                Event::Message(m) => {
                    for p in &m.parts {
                        assert_ne!(p.text, "child output", "a subagent leaked in");
                        if m.role == "user" && p.kind == PartKind::Text {
                            if m.injected {
                                injected += 1
                            } else {
                                prompts += 1
                            }
                        }
                        if p.text == "answer" {
                            answers += 1;
                            assert_eq!(m.model, "kimi-code/k3");
                        }
                        if p.kind == PartKind::ToolCall {
                            let c = p.call.as_ref().unwrap();
                            assert_eq!(
                                (c.kind, c.input.path.as_str()),
                                (tool::Kind::Read, "file.go")
                            );
                        }
                        if p.kind == PartKind::ToolResult {
                            let o = p.output.as_ref().unwrap();
                            assert_eq!(
                                (o.call_id.as_str(), o.is_error, o.text.as_str()),
                                ("c1", true, "file missing")
                            );
                        }
                    }
                }
                Event::TurnEnd(t) => {
                    ended += 1;
                    if ended == 1 {
                        assert_eq!(t.tokens.total(), 100);
                        assert_eq!(t.duration, Duration::from_millis(700));
                        assert_eq!(t.reason, "done");
                    }
                    if ended == 2 {
                        assert_eq!(t.reason, "interrupted");
                    }
                }
                _ => {}
            }
        }
        assert_eq!((prompts, injected, answers, ended), (2, 1, 1, 2));
        let evs = read_kimi_wire(FIXTURE.as_bytes(), 3000).unwrap();
        for ev in &evs {
            if let Event::Message(m) = ev {
                assert_ne!(m.id, "u2", "the cutoff took a newer prompt");
            }
        }
    }

    // kimi_wire_test.go: TestKimiWireMalformedRecord
    #[test]
    fn malformed_record_fails() {
        assert!(read_kimi_wire("not-json\n".as_bytes(), 0).is_err());
        // A last line still being written is passed over.
        assert!(read_kimi_wire("not-json".as_bytes(), 0).is_ok());
    }

    // kimi_wire_test.go: TestKimiWireFailedTurnKeepsItsError
    #[test]
    fn failed_turn_keeps_its_error() {
        let evs = read_kimi_wire(
            r#"{"type":"turn.ended","agentId":"main","reason":"failed","error":{"code":"internal","message":"open plans/x.md: no such file or directory"},"time":1000}
"#
            .as_bytes(),
            0,
        )
        .unwrap();
        assert_eq!(evs.len(), 1);
        let Event::TurnEnd(t) = &evs[0] else {
            panic!("{:?}", evs[0]);
        };
        assert_eq!(t.reason, "error");
        assert_eq!(t.err, "open plans/x.md: no such file or directory");
    }

    // tasks_test.go: TestHistoryTasks (Kimi's half)
    #[test]
    fn wire_keeps_its_tasks() {
        let d = tempfile::tempdir().unwrap();
        let wire = d.path().join("wire.jsonl");
        std::fs::write(&wire, r#"{"type":"context.append_loop_event","agentId":"main","event":{"type":"tool.call","toolCallId":"t1","name":"Agent","args":{"description":"dig","prompt":"Find it.","subagent_type":"coder","run_in_background":true}}}
{"type":"task.started","agentId":"main","info":{"taskId":"agent-1","description":"dig","kind":"agent","subagentType":"coder","parentToolCallId":"t1","detached":true}}
{"type":"task.terminated","agentId":"main","info":{"taskId":"agent-1","status":"completed","kind":"agent","parentToolCallId":"t1","detached":true}}
"#).unwrap();
        let (evs, _) = read_kimi_wire_file(&path_str(&wire), 0, 0).unwrap();
        let Event::Message(m) = &evs[0] else { panic!() };
        let c = m.parts[0].call.as_ref().unwrap();
        assert_eq!(c.kind, tool::Kind::Subagent);
        assert_eq!(
            (
                c.input.agent.as_str(),
                c.input.background,
                c.input.prompt.as_str()
            ),
            ("coder", true, "Find it.")
        );
        assert_eq!(
            evs[1],
            Event::TaskStarted(event::TaskStarted {
                id: "agent-1".into(),
                call_id: "t1".into(),
                kind: TaskKind::Subagent,
                label: "dig".into(),
                agent: "coder".into(),
                background: true,
            })
        );
        assert_eq!(
            evs[2],
            Event::TaskDone(event::TaskDone {
                id: "agent-1".into(),
                call_id: "t1".into(),
                status: "completed".into(),
                ..event::TaskDone::default()
            })
        );
    }
}
