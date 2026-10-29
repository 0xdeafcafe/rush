//! agy's stream-json output, line by line, as rush's events. Port of
//! internal/adapters/antigravity/stream.go:1-199.
//!
//! Go reads it with encoding/json, which matches field names whatever
//! their case; agy writes them in snake_case, as named here, and a tool's
//! parameters (CommandLine, AbsolutePath, …) are looked up whatever their
//! case.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use rush_core::event::{CallUpdated, Event, Init, Message, MessageStart, Part, PartKind, TurnEnd};
use rush_core::tool::{self, Call, Output};
use rush_core::usage::TokenUsage;
use rush_core::wire::RawJson;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

/// Null as the type's zero value.
pub(crate) fn nd<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// What agy counts, all told for the conversation so far.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
struct Tokens {
    #[serde(deserialize_with = "nd")]
    input_tokens: i64,
    #[serde(deserialize_with = "nd")]
    output_tokens: i64,
    #[serde(deserialize_with = "nd")]
    cache_read_tokens: i64,
    #[serde(deserialize_with = "nd")]
    thinking_tokens: i64,
}

impl Tokens {
    /// What was spent since p.
    fn since(&self, p: &Tokens) -> TokenUsage {
        TokenUsage {
            input: (self.input_tokens - p.input_tokens).max(0),
            output: (self.output_tokens - p.output_tokens).max(0),
            cache_read: (self.cache_read_tokens - p.cache_read_tokens).max(0),
            reasoning: (self.thinking_tokens - p.thinking_tokens).max(0),
            ..TokenUsage::default()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Wire {
    #[serde(deserialize_with = "nd")]
    event: String,
    #[serde(rename = "conversation_id", deserialize_with = "nd")]
    conversation: String,
    #[serde(deserialize_with = "nd")]
    init: WInit,
    #[serde(rename = "step_update", deserialize_with = "nd")]
    step: Step,
    #[serde(deserialize_with = "nd")]
    result: WResult,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WInit {
    #[serde(alias = "Model", deserialize_with = "nd")]
    model: String,
    #[serde(alias = "Cwd", deserialize_with = "nd")]
    cwd: String,
    #[serde(rename = "permission_mode", deserialize_with = "nd")]
    mode: String,
    #[serde(alias = "Tools", deserialize_with = "nd")]
    tools: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Step {
    #[serde(rename = "step_index", deserialize_with = "nd")]
    index: i64,
    #[serde(alias = "State", deserialize_with = "nd")]
    state: String,
    #[serde(rename = "step_type", deserialize_with = "nd")]
    typ: String,
    #[serde(rename = "text_delta", deserialize_with = "nd")]
    text: String,
    #[serde(rename = "tool_name", deserialize_with = "nd")]
    tool_name: String,
    #[serde(rename = "tool_info")]
    tool: Option<ToolInfo>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ToolInfo {
    #[serde(alias = "Name", deserialize_with = "nd")]
    name: String,
    #[serde(alias = "Parameters")]
    parameters: Option<Box<RawValue>>,
    #[serde(alias = "Output")]
    output: Option<Box<RawValue>>,
    #[serde(alias = "Error")]
    error: Option<ToolError>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ToolError {
    #[serde(alias = "Message", deserialize_with = "nd")]
    message: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WResult {
    #[serde(alias = "Status", deserialize_with = "nd")]
    status: String,
    #[serde(alias = "Response", deserialize_with = "nd")]
    response: String,
    #[serde(alias = "Error", deserialize_with = "nd")]
    error: String,
    #[serde(alias = "Usage", deserialize_with = "nd")]
    usage: Tokens,
    #[serde(rename = "duration_seconds", deserialize_with = "nd")]
    duration: f64,
    #[serde(rename = "num_turns", deserialize_with = "nd")]
    turns: i64,
    #[serde(rename = "denied_actions", deserialize_with = "nd")]
    denied: Vec<DeniedAction>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct DeniedAction {
    #[serde(rename = "display_name", deserialize_with = "nd")]
    name: String,
}

/// A run of random letters, as Go's crypto/rand.Text: 26 of base32's.
pub(crate) fn rand_text() -> String {
    use std::io::Read;
    const ABC: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut b = [0u8; 26];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_err()
    {
        // No randomness to be had: the time will do to tell runs apart.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        for (i, c) in b.iter_mut().enumerate() {
            *c = (t >> (i % 16 * 5)) as u8;
        }
    }
    b.iter().map(|c| ABC[(c & 31) as usize] as char).collect()
}

/// A tool's parameter by name, whatever its case.
fn param(m: &serde_json::Map<String, serde_json::Value>, name: &str) -> String {
    m.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One stream's turns: the steps being written, and what was counted so
/// far.
#[derive(Default)]
pub(crate) struct Parser {
    run: String,
    /// What the last line said, for the caller to take.
    pub out: Vec<Event>,
    texts: Option<BTreeMap<i64, String>>,
    tools: HashSet<i64>,
    finished: HashSet<i64>,
    has_text: bool,
    previous: Tokens,
    seconds: f64,
    /// The turn's end, once a line says it ended.
    pub end: TurnEnd,
}

fn text(role: &str, id: String, t: String) -> Event {
    Event::Message(Message {
        role: role.into(),
        id,
        parts: vec![Part {
            kind: PartKind::Text,
            text: t,
            ..Part::default()
        }],
        ..Message::default()
    })
}

impl Parser {
    fn emit(&mut self, e: Event) {
        self.out.push(e);
    }

    fn step_id(&self, i: i64) -> String {
        format!("agy-{}-step-{i}", self.run)
    }

    /// Reads one line; whether it ended the turn (end says how).
    pub fn line(&mut self, b: &[u8]) -> Result<bool, String> {
        if self.run.is_empty() {
            self.run = rand_text();
        }
        let w: Wire =
            serde_json::from_slice(b).map_err(|e| format!("invalid Antigravity stream: {e}"))?;
        match w.event.as_str() {
            "init" => self.emit(Event::Init(Box::new(Init {
                session_id: w.conversation,
                model: w.init.model,
                cwd: w.init.cwd,
                mode: w.init.mode,
                tools: w.init.tools,
                ..Init::default()
            }))),
            "step_update" => self.step(w.step),
            "result" => {
                self.result(w.result);
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    fn step(&mut self, s: Step) {
        let id = self.step_id(s.index);
        if self.finished.contains(&s.index) {
            return;
        }
        if s.typ == "agent_response" {
            let texts = self.texts.get_or_insert_with(BTreeMap::new);
            let fresh = !texts.contains_key(&s.index);
            texts.entry(s.index).or_default().push_str(&s.text);
            if fresh {
                self.emit(Event::MessageStart(MessageStart {
                    id: id.clone(),
                    ..MessageStart::default()
                }));
            }
            if !s.text.is_empty() {
                self.emit(Event::Delta(rush_core::event::Delta {
                    kind: PartKind::Text,
                    text: s.text,
                    ..rush_core::event::Delta::default()
                }));
            }
            if s.state == "DONE" {
                let t = self
                    .texts
                    .as_mut()
                    .and_then(|t| t.remove(&s.index))
                    .unwrap_or_default();
                if !t.is_empty() {
                    self.emit(text("assistant", id, t));
                    self.has_text = true;
                }
                self.finished.insert(s.index);
            }
            return;
        }
        let (true, Some(ti)) = (s.typ == "tool", s.tool) else {
            return;
        };
        let name = if ti.name.is_empty() {
            s.tool_name
        } else {
            ti.name
        };
        let mut call = Call {
            id: id.clone(),
            name: name.clone(),
            title: name.clone(),
            raw: ti.parameters.clone().map(RawJson),
            ..Call::default()
        };
        let args: serde_json::Map<String, serde_json::Value> = ti
            .parameters
            .as_ref()
            .and_then(|p| serde_json::from_str(p.get()).ok())
            .unwrap_or_default();
        match name.as_str() {
            "run_command" => {
                call.kind = tool::Kind::Shell;
                call.input.command = param(&args, "CommandLine");
                call.input.cwd = param(&args, "Cwd");
            }
            "view_file" => {
                call.kind = tool::Kind::Read;
                call.input.path = param(&args, "AbsolutePath");
            }
            "write_to_file" => {
                call.kind = tool::Kind::Write;
                call.input.path = param(&args, "TargetFile");
                call.input.content = param(&args, "FileContent");
            }
            _ => {}
        }
        if self.tools.insert(s.index) {
            self.emit(Event::Message(Message {
                role: "assistant".into(),
                id: id.clone(),
                parts: vec![Part {
                    kind: PartKind::ToolCall,
                    call: Some(Box::new(call.clone())),
                    ..Part::default()
                }],
                ..Message::default()
            }));
        }
        // A denied or failed call ends ERROR, not DONE.
        if s.state == "DONE" || s.state == "ERROR" {
            self.emit(Event::CallUpdated(Box::new(CallUpdated { call })));
            let mut out = match ti.output.as_deref().map(RawValue::get) {
                None | Some("null") => String::new(),
                Some(r) if r.starts_with('"') => serde_json::from_str(r).unwrap_or_default(),
                Some(r) => r.to_string(),
            };
            if let Some(e) = &ti.error {
                out = format!("{out}\n{}", e.message).trim().to_string();
            }
            let failed = ti.error.is_some() || s.state == "ERROR";
            self.emit(Event::Message(Message {
                role: "user".into(),
                id: format!("{id}-result"),
                parts: vec![Part {
                    kind: PartKind::ToolResult,
                    output: Some(Box::new(Output {
                        call_id: id,
                        text: out,
                        is_error: failed,
                        ..Output::default()
                    })),
                    ..Part::default()
                }],
                ..Message::default()
            }));
            self.finished.insert(s.index);
        }
    }

    fn result(&mut self, r: WResult) {
        // Complete any partial response if the CLI omits its final step
        // transition.
        for (i, t) in self.texts.take().unwrap_or_default() {
            if t.is_empty() {
                continue;
            }
            let id = self.step_id(i);
            self.emit(text("assistant", id, t));
            self.has_text = true;
        }
        if !self.has_text && !r.response.is_empty() {
            let id = format!("agy-{}-result-{}", self.run, r.turns);
            self.emit(text("assistant", id, r.response.clone()));
        }
        let mut reason = "done";
        let mut err = r.error.clone();
        match r.status.as_str() {
            "SUCCESS" => {}
            "CANCELED" | "INTERRUPTED" => reason = "interrupted",
            _ => {
                reason = "error";
                if err.is_empty() {
                    err = format!("Antigravity ended with status {}", r.status);
                }
            }
        }
        // Headless, it can't ask: it denies the call and ends "SUCCESS"
        // saying nothing.
        if reason == "done" && !self.has_text && r.response.is_empty() && !r.denied.is_empty() {
            let names: Vec<&str> = r.denied.iter().map(|d| d.name.as_str()).collect();
            reason = "error";
            err = format!(
                "Antigravity denied {} without asking, as it can't headless: allow it under permissions.allow in its settings.json, or run it in always-proceed mode",
                names.join(", ")
            );
        }
        self.end = TurnEnd {
            reason: reason.into(),
            err,
            text: r.response,
            tokens: r.usage.since(&self.previous),
            // A count of seconds past what a Duration holds is none.
            duration: Duration::try_from_secs_f64((r.duration - self.seconds).max(0.0))
                .unwrap_or_default(),
            turns: 1,
            ..TurnEnd::default()
        };
        (self.previous, self.seconds) = (r.usage, r.duration);
        self.tools.clear();
        self.finished.clear();
        self.has_text = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_text_tools_and_cumulative_usage() {
        let mut p = Parser::default();
        let lines = [
            r#"{"event":"init","conversation_id":"session","init":{"cwd":"/work","model":"chosen"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"hello"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"agent_response","text_delta":" world"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"pwd"},"output":"/work"}}}"#,
            r#"{"event":"result","result":{"status":"SUCCESS","response":"hello world","num_turns":1,"usage":{"input_tokens":100,"output_tokens":10,"cache_read_tokens":30}}}"#,
        ];
        for l in lines {
            p.line(l.as_bytes()).unwrap();
        }
        let (mut messages, mut tools) = (0, 0);
        for ev in &p.out {
            if let Event::Message(m) = ev {
                for part in &m.parts {
                    if part.kind == PartKind::Text {
                        messages += 1;
                        assert_eq!(part.text, "hello world");
                    }
                    if let Some(c) = &part.call {
                        tools += 1;
                        assert_eq!(
                            (c.kind, c.input.command.as_str()),
                            (tool::Kind::Shell, "pwd")
                        );
                    }
                }
            }
        }
        assert_eq!(
            (messages, tools),
            (1, 1),
            "duplicated or missing transcript rows: {:?}",
            p.out
        );
        assert!(
            matches!(&p.out[0], Event::Init(i) if i.session_id == "session" && i.model == "chosen")
        );
        assert_eq!(p.end.tokens.input, 100);
        p.line(br#"{"event":"result","result":{"status":"SUCCESS","response":"second","num_turns":2,"usage":{"input_tokens":120,"output_tokens":14,"cache_read_tokens":35}}}"#).unwrap();
        let t = p.end.tokens;
        assert_eq!(
            (t.input, t.output, t.cache_read),
            (20, 4, 5),
            "cumulative tokens double counted"
        );
    }

    // Headless, Antigravity denies a call it can't ask about and ends
    // "SUCCESS" with nothing said: that's an error naming what it denied,
    // not an empty answer.
    #[test]
    fn denied_without_asking_is_an_error() {
        let mut p = Parser::default();
        let mut ended = false;
        // As agy sends it: an empty response step, then the call it denied.
        for l in [
            r#"{"event":"step_update","step_update":{"step_index":1,"state":"DONE","step_type":"agent_response"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo ok"}}}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"ERROR","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo ok"}}}}"#,
            r#"{"event":"result","result":{"status":"SUCCESS","response":"","num_turns":1,"denied_actions":[{"action":"command","display_name":"RunCommand"}]}}"#,
        ] {
            ended = p.line(l.as_bytes()).unwrap();
        }
        assert!(ended);
        assert_eq!(p.end.reason, "error");
        assert!(p.end.err.contains("RunCommand"), "{}", p.end.err);
        let results = p
            .out
            .iter()
            .filter(|e| matches!(e, Event::Message(m) if m.parts[0].kind == PartKind::ToolResult && m.parts[0].output.as_ref().unwrap().is_error))
            .count();
        assert_eq!(results, 1, "the denied call's failed results");
    }

    #[test]
    fn bad_lines_and_statuses() {
        let mut p = Parser::default();
        assert!(
            p.line(b"not json")
                .unwrap_err()
                .starts_with("invalid Antigravity stream")
        );
        p.line(br#"{"event":"result","result":{"status":"QUOTA"}}"#)
            .unwrap();
        assert_eq!(
            (p.end.reason.as_str(), p.end.err.as_str()),
            ("error", "Antigravity ended with status QUOTA")
        );
        p.line(br#"{"event":"result","result":{"status":"CANCELED"}}"#)
            .unwrap();
        assert_eq!(p.end.reason, "interrupted");
        assert_eq!(rand_text().len(), 26);
        // A duration no Duration holds is none, not a panic.
        p.line(br#"{"event":"result","result":{"status":"SUCCESS","duration_seconds":1e300}}"#)
            .unwrap();
        assert_eq!(p.end.duration, Duration::ZERO);
    }
}
