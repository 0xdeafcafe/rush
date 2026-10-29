//! A Kimi subagent's own run: its wire, agents/agent-N/wire.jsonl beside
//! the session's main one. Port of
//! internal/adapters/acp/kimi_children.go:1-160.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use rush_core::adapter::{ChildFinder, Profile, Session};
use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::history::content_parts;
use crate::kimi::{Kimi, kimi_home};
use crate::util::{base, glob, mod_ms, path_str};

/// Finds the subagent an Agent call's result says ran it.
static AGENT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^agent_id: (\S+)$").expect("valid"));

impl ChildFinder for Kimi {
    /// The subagent parent's Agent call child started. Main's wire names it
    /// in a task.started or the call's result; a subagent still running in
    /// the foreground has neither yet, so it's the one asked what the call
    /// asked.
    fn find_child(&self, p: &Profile, parent: &str, child: &str, start: Ms) -> Option<Session> {
        let dir = if p.dir.is_empty() {
            kimi_home()
        } else {
            p.dir.clone()
        };
        // Live, ACP's call id is the wire's after the turn: "0:tool_…".
        let child = child.split_once(':').map_or(child, |(_, id)| id);
        if parent.is_empty() || child.is_empty() || base(parent) != parent {
            return None;
        }
        for agents in glob(Path::new(&dir), &["sessions", "*", parent, "agents"]) {
            let (id, prompt) = child_of(&agents.join("main").join("wire.jsonl"), child);
            let file: Option<PathBuf> = if !id.is_empty() && base(&id) == id {
                Some(agents.join(&id).join("wire.jsonl"))
            } else if !prompt.is_empty() {
                asked(&agents, &prompt, start)
            } else {
                None
            };
            let Some(file) = file else {
                continue;
            };
            if let Ok(st) = std::fs::metadata(&file) {
                let id = file
                    .parent()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let at = mod_ms(&st);
                return Some(Session {
                    kind: self.agent.id.clone(),
                    profile: Profile {
                        dir: dir.clone(),
                        ..p.clone()
                    },
                    id,
                    transcript: path_str(&file),
                    state: "done".into(),
                    created_at: created(&file, at),
                    updated_at: at,
                    ..Session::default()
                });
            }
        }
        None
    }
}

/// What main's wire says of the Agent call child: the subagent it
/// started, once said, and the prompt it was given.
fn child_of(main: &Path, child: &str) -> (String, String) {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Args {
        prompt: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Res<'a> {
        #[serde(borrow)]
        output: Option<&'a RawValue>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Ev<'a> {
        #[serde(rename = "type")]
        kind: String,
        tool_call_id: String,
        args: Args,
        #[serde(borrow)]
        result: Res<'a>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Info {
        agent_id: String,
        parent_tool_call_id: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Rec<'a> {
        #[serde(rename = "type")]
        kind: String,
        #[serde(borrow)]
        event: Ev<'a>,
        info: Info,
    }
    let Ok(f) = File::open(main) else {
        return (String::new(), String::new());
    };
    let mut r = BufReader::with_capacity(64 << 10, f);
    let mut prompt = String::new();
    let mut line = Vec::new();
    let needle = child.as_bytes();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if memchr::memmem::find(&line, needle).is_none() {
            continue;
        }
        let Ok(rec) = serde_json::from_slice::<Rec>(line.trim_ascii_end()) else {
            continue;
        };
        if rec.kind == "task.started"
            && rec.info.parent_tool_call_id == child
            && !rec.info.agent_id.is_empty()
        {
            return (rec.info.agent_id, prompt);
        }
        if rec.event.tool_call_id != child {
            continue;
        }
        match rec.event.kind.as_str() {
            "tool.call" => prompt = rec.event.args.prompt,
            "tool.result" => {
                let out = rec
                    .event
                    .result
                    .output
                    .and_then(|o| serde_json::from_str::<String>(o.get()).ok())
                    .unwrap_or_default();
                if let Some(m) = AGENT_ID.captures(&out) {
                    return (m[1].to_string(), prompt);
                }
            }
            _ => {}
        }
    }
    (String::new(), prompt)
}

/// The subagent wire in dir begun after start whose first message carries
/// prompt (Kimi puts its git context before it).
fn asked(dir: &Path, prompt: &str, start: Ms) -> Option<PathBuf> {
    let prompt = prompt.trim();
    glob(dir, &["agent-*", "wire.jsonl"])
        .into_iter()
        .filter(|f| std::fs::metadata(f).is_ok_and(|st| mod_ms(&st) >= start - 3000))
        .find(|f| first_ask(f).contains(prompt))
}

/// The text of a wire's first context message.
fn first_ask(file: &Path) -> String {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Msg<'a> {
        #[serde(borrow)]
        content: Option<&'a RawValue>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Rec<'a> {
        #[serde(borrow)]
        message: Msg<'a>,
    }
    let Ok(f) = File::open(file) else {
        return String::new();
    };
    let mut r = BufReader::with_capacity(64 << 10, f);
    let mut line = Vec::new();
    for _ in 0..50 {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if memchr::memmem::find(&line, br#""context.append_message""#).is_none() {
            continue;
        }
        let Ok(rec) = serde_json::from_slice::<Rec>(line.trim_ascii_end()) else {
            return String::new();
        };
        return content_parts(rec.message.content)
            .into_iter()
            .map(|p| p.text)
            .collect();
    }
    String::new()
}

/// When a wire's metadata says it began, else at.
fn created(file: &Path, at: Ms) -> Ms {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct M {
        created_at: i64,
    }
    let Ok(f) = File::open(file) else {
        return at;
    };
    let mut line = Vec::new();
    let _ = BufReader::new(f).read_until(b'\n', &mut line);
    match serde_json::from_slice::<M>(line.trim_ascii_end()) {
        Ok(m) if m.created_at > 0 => m.created_at,
        _ => at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::adapter::{Kind, read_history};
    use rush_core::event::Event;
    use rush_core::tool;

    // kimi_children_test.go: TestKimiFindChild
    #[test]
    fn find_child() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let write = |rel: &str, body: &str| {
            let path = home.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        let child = |id: &str, prompt: &str, answer: &str| {
            format!(
                r#"{{"type":"metadata","protocol_version":"1.5","created_at":1000}}
{{"type":"context.append_message","agentId":"{id}","message":{{"role":"user","origin":{{"kind":"system_trigger"}},"content":[{{"type":"text","text":"<git-context>x</git-context>\n{prompt}"}}]}},"time":1100}}
{{"type":"context.append_loop_event","agentId":"{id}","event":{{"type":"content.part","uuid":"a","part":{{"type":"text","text":"{answer}"}}}},"time":1200}}
{{"type":"turn.ended","agentId":"{id}","reason":"completed","time":1300}}
"#
            )
        };
        let s = "sessions/wd_a/session_P/agents/";
        write(
            &format!("{s}main/wire.jsonl"),
            r#"{"type":"context.append_loop_event","agentId":"main","event":{"type":"tool.call","toolCallId":"bg","name":"Agent","args":{"prompt":"find bugs","description":"bugs","run_in_background":true}},"time":1000}
{"type":"task.started","agentId":"main","info":{"taskId":"agent-x","kind":"agent","agentId":"agent-1","parentToolCallId":"bg"},"time":1001}
{"type":"context.append_loop_event","agentId":"main","event":{"type":"tool.call","toolCallId":"done","name":"Agent","args":{"prompt":"say ok"}},"time":1002}
{"type":"context.append_loop_event","agentId":"main","event":{"type":"tool.result","toolCallId":"done","result":{"output":"agent_id: agent-0\nstatus: completed"}},"time":1003}
{"type":"context.append_loop_event","agentId":"main","event":{"type":"tool.call","toolCallId":"fg","name":"Agent","args":{"prompt":"count lines"}},"time":1004}
"#,
        );
        write(
            &format!("{s}agent-0/wire.jsonl"),
            &child("agent-0", "say ok", "ok"),
        );
        write(
            &format!("{s}agent-1/wire.jsonl"),
            &child("agent-1", "find bugs", "none found"),
        );
        write(
            &format!("{s}agent-2/wire.jsonl"),
            &child("agent-2", "count lines", "42"),
        );
        write(
            "sessions/wd_b/session_Q/agents/agent-1/wire.jsonl",
            &child("agent-1", "count lines", "decoy"),
        );
        let k = Kimi::new();
        let p = Profile {
            kind: Kind::from("kimi"),
            dir: path_str(home),
            ..Profile::default()
        };
        let a_minute_ago = rush_core::time::now() - 60_000;
        for (call, want) in [
            ("bg", "agent-1"),
            ("0:bg", "agent-1"),
            ("done", "agent-0"),
            ("fg", "agent-2"),
        ] {
            let got = k
                .find_child(&p, "session_P", call, a_minute_ago)
                .unwrap_or_else(|| panic!("{call}"));
            assert_eq!(got.id, want, "{call}");
            assert_eq!(
                got.transcript,
                path_str(&home.join(format!("{s}{want}/wire.jsonl")))
            );
            assert_eq!(got.kind.as_str(), "kimi");
            assert_eq!(got.created_at, 1000);
        }
        assert!(k.find_child(&p, "session_P", "nope", 0).is_none());

        let got = k.find_child(&p, "session_P", "bg", 0).unwrap();
        let evs = read_history(&k, &got, 0).unwrap();
        let answered = evs.iter().any(|e| {
            matches!(e, Event::Message(m) if m.role == "assistant" && m.parts[0].text == "none found")
        });
        assert!(answered, "{evs:?}");

        let main = crate::kimi_wire::read_kimi_wire_file(
            &path_str(&home.join(format!("{s}main/wire.jsonl"))),
            0,
            0,
        )
        .unwrap()
        .0;
        let Event::Message(m) = &main[0] else {
            panic!()
        };
        let c = m.parts[0].call.as_ref().unwrap();
        assert_eq!(
            (c.kind, c.input.child.as_str()),
            (tool::Kind::Subagent, "bg")
        );
    }
}
