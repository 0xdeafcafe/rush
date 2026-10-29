//! A session against a fake ACP agent: the agent's end of two pipes,
//! driven by the test. Port of internal/adapters/acp/acp_test.go,
//! tasks_test.go, question_cancel_test.go, kimi_question_test.go,
//! permission_modes_test.go, tasks_ended_test.go, starterr_test.go and
//! kimi_children_test.go (the live half), and glm_test.go's TestStart.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use rush_core::adapter::{Conn, Input, Said, StartOptions, Tagged};
use rush_core::event::{self, Event, OptionKind, PartKind, TaskKind};
use rush_core::tool::{self, Kind};
use serde_json::{Value, json};

use crate::protocol::{ConfigOption, ConfigValue, Location, Mode, ToolContent};
use crate::session::{Options, Session, lock, permission_modes, set_config, stop_reason};
use crate::update::{kind_of, patch, read_input};

/// The agent's end of the pipes.
struct Fake {
    r: BufReader<PipeReader>,
    w: PipeWriter,
}

impl Fake {
    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.r.read_line(&mut line).expect("agent read");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("agent read {line:?}: {e}"))
    }

    /// The next message, which is a call of method.
    fn expect(&mut self, method: &str) -> Value {
        let m = self.recv();
        assert_eq!(m["method"], method, "agent got {m}");
        m
    }

    fn send(&mut self, v: Value) {
        let mut b = v.to_string().into_bytes();
        b.push(b'\n');
        self.w.write_all(&b).expect("agent write");
    }

    fn result(&mut self, id: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn update(&mut self, u: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": u}}));
    }

    fn request(&mut self, id: i64, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }
}

fn text(kind: &str, s: &str) -> Value {
    json!({"sessionUpdate": kind, "content": {"type": "text", "text": s}})
}

/// What a session said, as a test reads it.
struct Heard(Receiver<Said<()>>);

impl Heard {
    fn next(&self) -> Event {
        match self.0.recv_timeout(Duration::from_secs(3)) {
            Ok(Said::Event((), ev)) => ev,
            Ok(Said::Ended((), e)) => panic!("ended: {e:?}"),
            Err(e) => panic!("no event: {e}"),
        }
    }
}

macro_rules! next {
    ($h:expr, $v:ident) => {
        match $h.next() {
            Event::$v(x) => x,
            ev => panic!("got {ev:?}, want {}", stringify!($v)),
        }
    };
}

/// A session opened with a fake agent through initialize and session/new,
/// or session/load when resuming, which sends replay's updates before it
/// answers.
fn open(resume: &str, replay: Option<&dyn Fn(&mut Fake)>, o: Options) -> (Session, Fake, Heard) {
    let (cr, aw) = std::io::pipe().unwrap();
    let (ar, cw) = std::io::pipe().unwrap();
    let mut f = Fake {
        r: BufReader::new(ar),
        w: aw,
    };
    let (tx, rx) = channel();
    let sink = Arc::new(Tagged { key: (), tx });
    let dir = tempfile::tempdir().unwrap().keep();
    let o = Options {
        dir: dir.to_string_lossy().into_owned(),
        adapter: if o.adapter.is_empty() {
            "test".into()
        } else {
            o.adapter.clone()
        },
        resume: resume.into(),
        ..o
    };
    let dialed = std::thread::spawn(move || Session::dial(cr, cw, o, sink));

    let m = f.expect("initialize");
    let caps = &m["params"]["clientCapabilities"];
    assert_eq!(m["params"]["protocolVersion"], 1);
    assert_eq!(caps["fs"]["readTextFile"], true);
    assert_eq!(caps["fs"]["writeTextFile"], true);
    assert_eq!(caps["terminal"], false);
    f.result(
        &m["id"],
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {"loadSession": true, "promptCapabilities": {"image": true}},
            "agentInfo": {"name": "fake", "version": "1.2.3"},
        }),
    );
    let method = if resume.is_empty() {
        "session/new"
    } else {
        "session/load"
    };
    let m = f.expect(method);
    let p = &m["params"];
    assert!(p["cwd"].as_str().unwrap().starts_with('/'), "{p}");
    assert!(p["mcpServers"].is_array(), "{p}");
    if resume.is_empty() {
        assert!(p.get("sessionId").is_none());
    } else {
        assert_eq!(p["sessionId"], resume);
    }
    if let Some(replay) = replay {
        replay(&mut f);
    }
    f.result(
        &m["id"],
        json!({
            "sessionId": "s1",
            "modes": {"currentModeId": "default", "availableModes": [
                {"id": "default", "name": "Default"}, {"id": "plan", "name": "Plan"}]},
            "configOptions": [{"type": "select", "id": "model", "name": "Model", "category": "model",
                "currentValue": "m1", "options": [{"value": "m1", "name": "M1"}]}],
        }),
    );
    let s = dialed.join().unwrap().unwrap();
    (s, f, Heard(rx))
}

fn start() -> (Session, Fake, Heard) {
    open("", None, Options::default())
}

fn call_of(m: &event::Message) -> &tool::Call {
    m.parts[0].call.as_deref().expect("a call")
}

fn output_of(m: &event::Message) -> &tool::Output {
    m.parts[0].output.as_deref().expect("a result")
}

fn input(t: &str) -> Input {
    Input {
        text: t.into(),
        ..Input::default()
    }
}

// acp_test.go: TestTurn
#[test]
fn turn() {
    let (s, mut f, h) = start();
    let init = next!(h, Init);
    assert_eq!(
        (
            init.session_id.as_str(),
            init.model.as_str(),
            init.mode.as_str(),
            init.version.as_str()
        ),
        ("s1", "m1", "default", "1.2.3")
    );
    f.update(json!({"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "compact", "description": "x"}]}));
    assert_eq!(next!(h, Init).commands, ["compact"]);

    s.send(input("hi")).unwrap();
    let prompt = f.expect("session/prompt");
    assert_eq!(prompt["params"]["sessionId"], "s1");
    assert_eq!(
        prompt["params"]["prompt"],
        json!([{"type": "text", "text": "hi"}])
    );

    f.update(text("agent_thought_chunk", "hmm"));
    f.update(text("agent_message_chunk", "Hel"));
    f.update(text("agent_message_chunk", "lo"));
    next!(h, MessageStart);
    assert_eq!(
        next!(h, PartStart),
        event::PartStart {
            index: 0,
            kind: PartKind::Thinking
        }
    );
    assert_eq!(
        next!(h, Delta),
        event::Delta {
            index: 0,
            kind: PartKind::Thinking,
            text: "hmm".into()
        }
    );
    next!(h, PartStart);
    next!(h, Delta);
    assert_eq!(
        next!(h, Delta),
        event::Delta {
            index: 1,
            kind: PartKind::Text,
            text: "lo".into()
        }
    );

    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Edit a.go", "kind": "edit", "status": "pending",
        "locations": [{"path": "/w/a.go"}],
        "content": [{"type": "diff", "path": "/w/a.go", "oldText": "a\nb\nc\n", "newText": "a\nB\nc\n"}]}));
    let whole = next!(h, Message);
    assert_eq!(whole.role, "assistant");
    assert_eq!(whole.parts.len(), 2);
    assert_eq!(
        (whole.parts[0].text.as_str(), whole.parts[1].text.as_str()),
        ("hmm", "Hello")
    );
    let call_msg = next!(h, Message);
    let c = call_of(&call_msg);
    assert_eq!(call_msg.parts[0].kind, PartKind::ToolCall);
    assert_eq!(
        (
            c.id.as_str(),
            c.kind,
            c.title.as_str(),
            c.input.path.as_str()
        ),
        ("c1", Kind::Edit, "Edit a.go", "/w/a.go")
    );
    assert_eq!(
        c.input.edits,
        [tool::Replace {
            old: "a\nb\nc\n".into(),
            new: "a\nB\nc\n".into(),
            ..tool::Replace::default()
        }]
    );

    f.request(
        7,
        "session/request_permission",
        json!({"sessionId": "s1", "toolCall": {"toolCallId": "c1"},
        "options": [{"optionId": "yes", "name": "Allow", "kind": "allow_once"},
            {"optionId": "always", "name": "Always", "kind": "allow_always"},
            {"optionId": "no", "name": "Reject", "kind": "reject_once"}]}),
    );
    let ap = next!(h, Approval);
    assert_eq!(
        (ap.id.as_str(), ap.call.id.as_str(), ap.options.len()),
        ("7", "c1", 3)
    );
    assert_eq!(
        ap.options[1],
        event::ApprovalOption {
            id: "always".into(),
            label: "Always".into(),
            kind: OptionKind::AllowAlways
        }
    );
    s.answer(&ap.id, "yes").unwrap();
    let reply = f.recv();
    assert_eq!(reply["id"], 7);
    assert_eq!(
        reply["result"].to_string(),
        r#"{"outcome":{"optionId":"yes","outcome":"selected"}}"#
    );

    f.update(
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress"}),
    );
    f.update(
        json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
        "content": [{"type": "content", "content": {"type": "text", "text": "edited"}},
            {"type": "diff", "path": "/w/a.go", "oldText": "a\nb\nc\n", "newText": "a\nB\nc\n"}]}),
    );
    let u = next!(h, CallUpdated);
    assert_eq!(
        (u.call.id.as_str(), u.call.input.path.as_str()),
        ("c1", "/w/a.go")
    );
    let res = next!(h, Message);
    let out = output_of(&res);
    assert_eq!(
        (res.role.as_str(), res.parts[0].kind),
        ("user", PartKind::ToolResult)
    );
    assert_eq!(
        (out.call_id.as_str(), out.text.as_str(), out.is_error),
        ("c1", "edited", false)
    );
    assert_eq!(
        out.patches,
        [tool::Patch {
            path: "/w/a.go".into(),
            old_start: 2,
            old_lines: 1,
            new_start: 2,
            new_lines: 1,
            lines: vec!["-b".into(), "+B".into()],
        }]
    );

    f.update(json!({"sessionUpdate": "plan", "entries": [
        {"content": "one", "priority": "high", "status": "completed"},
        {"content": "two", "priority": "low", "status": "in_progress"}]}));
    let todo = |l: &str, st: &str| tool::TodoItem {
        label: l.into(),
        status: st.into(),
        ..tool::TodoItem::default()
    };
    assert_eq!(
        next!(h, Plan).todos,
        [todo("one", "completed"), todo("two", "in_progress")]
    );
    f.update(json!({"sessionUpdate": "usage_update", "used": 1200, "size": 200000, "cost": {"amount": 0.5, "currency": "USD"}}));
    assert_eq!(
        next!(h, Context),
        event::Context {
            tokens: 1200,
            window: 200000
        }
    );

    f.update(text("agent_message_chunk", "done"));
    next!(h, MessageStart);
    next!(h, PartStart);
    next!(h, Delta);
    f.result(
        &prompt["id"],
        json!({"stopReason": "end_turn", "usage": {"totalTokens": 30, "inputTokens": 20, "outputTokens": 10}}),
    );
    assert_eq!(next!(h, Message).parts[0].text, "done");
    let end = next!(h, TurnEnd);
    assert_eq!(
        (
            end.reason.as_str(),
            end.tokens.input,
            end.tokens.output,
            end.cost
        ),
        ("done", 20, 10, 0.5)
    );
}

// acp_test.go: TestInterrupt
#[test]
fn interrupt() {
    let (s, mut f, h) = start();
    next!(h, Init);
    s.send(input("go")).unwrap();
    let prompt = f.expect("session/prompt");
    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "c2", "title": "ls", "kind": "execute", "rawInput": {"command": ["ls", "-la"]}}));
    let m = next!(h, Message);
    let c = call_of(&m);
    assert_eq!((c.kind, c.input.command.as_str()), (Kind::Shell, "ls -la"));
    f.request(
        8,
        "session/request_permission",
        json!({"sessionId": "s1", "toolCall": {"toolCallId": "c2"},
        "options": [{"optionId": "no", "name": "No", "kind": "reject_always"}]}),
    );
    let ap = next!(h, Approval);
    s.interrupt().unwrap();
    f.expect("session/cancel");
    let reply = f.recv();
    assert_eq!(reply["id"], 8);
    assert_eq!(
        reply["result"].to_string(),
        r#"{"outcome":{"outcome":"cancelled"}}"#
    );
    assert_eq!(next!(h, ApprovalCancelled).id, ap.id);
    f.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c2", "status": "failed", "rawOutput": "stopped"}));
    let m = next!(h, Message);
    let out = output_of(&m);
    assert!(out.is_error);
    assert_eq!(out.text, "stopped");
    f.result(&prompt["id"], json!({"stopReason": "cancelled"}));
    assert_eq!(next!(h, TurnEnd).reason, "interrupted");
    assert!(
        s.answer(&ap.id, "no").is_err(),
        "answered a withdrawn approval"
    );
}

// acp_test.go: TestKimiInputStreamsAsText
#[test]
fn kimi_input_streams_as_text() {
    let (s, mut f, h) = start();
    next!(h, Init);
    s.send(input("list")).unwrap();
    let prompt = f.expect("session/prompt");
    let content = |t: &str| json!([{"type": "content", "content": {"type": "text", "text": t}}]);
    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "c9", "title": "Bash", "kind": "execute", "status": "pending", "content": content("")}));
    let m = next!(h, Message);
    assert_eq!(
        (call_of(&m).kind, call_of(&m).input.command.as_str()),
        (Kind::Shell, "")
    );
    f.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c9", "status": "in_progress", "content": content(r#"{"command":"ls"#)}));
    assert_eq!(
        next!(h, CallUpdated).call.input.command,
        "",
        "a piece of the input took"
    );
    f.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c9", "status": "in_progress", "content": content(r#"{"command":"ls -la"}"#)}));
    assert_eq!(next!(h, CallUpdated).call.input.command, "ls -la");
    f.request(9, "session/request_permission", json!({"sessionId": "s1",
        "toolCall": {"toolCallId": "c9", "title": "Bash", "content": content("Requesting approval to Running: ls -la")},
        "options": [{"optionId": "yes", "name": "Approve once", "kind": "allow_once"}]}));
    next!(h, CallUpdated);
    assert_eq!(next!(h, Approval).call.input.command, "ls -la");
    f.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c9", "status": "completed", "content": content(r#"{"exit_code":0}"#)}));
    assert_eq!(
        next!(h, CallUpdated).call.input.command,
        "ls -la",
        "a result undid the input"
    );
    assert!(!output_of(&next!(h, Message)).text.is_empty());
    f.result(&prompt["id"], json!({"stopReason": "end_turn"}));
    assert_eq!(next!(h, TurnEnd).reason, "done");
}

// acp_test.go: TestSettingsAndFiles
#[test]
fn settings_and_files() {
    let (s, mut f, h) = start();
    next!(h, Init);
    let s = Arc::new(s);

    let s2 = s.clone();
    let set = std::thread::spawn(move || s2.set_mode("plan"));
    let m = f.expect("session/set_mode");
    assert_eq!(
        m["params"].to_string(),
        r#"{"modeId":"plan","sessionId":"s1"}"#
    );
    f.result(&m["id"], json!({}));
    set.join().unwrap().unwrap();
    assert_eq!(next!(h, Init).mode, "plan");
    assert!(s.set_mode("nope").is_err(), "set an unknown mode");

    let s2 = s.clone();
    let set = std::thread::spawn(move || s2.set_model("m2"));
    let m = f.expect("session/set_config_option");
    assert_eq!(
        m["params"].to_string(),
        r#"{"configId":"model","sessionId":"s1","value":"m2"}"#
    );
    f.result(&m["id"], json!({"configOptions": [{"type": "select", "id": "model", "name": "Model", "category": "model", "currentValue": "m2"}]}));
    set.join().unwrap().unwrap();
    assert_eq!(next!(h, Init).model, "m2");

    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f.txt");
    std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
    let ps = p.to_string_lossy().into_owned();
    f.request(
        20,
        "fs/read_text_file",
        json!({"sessionId": "s1", "path": ps, "line": 2, "limit": 1}),
    );
    assert_eq!(f.recv()["result"].to_string(), r#"{"content":"two\n"}"#);
    f.request(
        21,
        "fs/write_text_file",
        json!({"sessionId": "s1", "path": ps, "content": "new"}),
    );
    let r = f.recv();
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
    f.request(
        22,
        "terminal/create",
        json!({"sessionId": "s1", "command": "ls"}),
    );
    assert_eq!(f.recv()["error"]["code"], crate::rpc::CODE_METHOD_NOT_FOUND);

    f.request(23, "elicitation/create", json!({"sessionId": "s1", "mode": "form", "message": "Pick",
        "requestedSchema": {"type": "object", "properties": {
            "color": {"type": "string", "title": "Colour", "oneOf": [{"const": "r", "title": "Red"}, {"const": "g", "title": "Green"}]},
            "ok": {"type": "boolean", "title": "Sure?"}}}}));
    let q = next!(h, Question);
    assert_eq!(
        (q.id.as_str(), q.title.as_str(), q.asks.len()),
        ("23", "Pick", 2)
    );
    assert_eq!(q.asks[0].id, "color");
    assert_eq!(q.asks[0].options.len(), 2);
    assert_eq!(q.asks[0].options[1].label, "Green");
    let answers = HashMap::from([
        ("color".to_string(), vec!["Green".to_string()]),
        ("ok".to_string(), vec!["Yes".to_string()]),
    ]);
    s.answerer()
        .unwrap()
        .answer_question(&q.id, &answers)
        .unwrap();
    assert_eq!(
        f.recv()["result"].to_string(),
        r#"{"action":"accept","content":{"color":"g","ok":true}}"#
    );
}

// acp_test.go: TestLoad
#[test]
fn load() {
    let replay = |f: &mut Fake| {
        f.update(text("user_message_chunk", "fix it"));
        f.update(text("agent_message_chunk", "fixed"));
    };
    let (_s, _f, h) = open("old", Some(&replay), Options::default());
    let m = next!(h, Message);
    assert_eq!(
        (m.role.as_str(), m.parts[0].text.as_str()),
        ("user", "fix it")
    );
    next!(h, MessageStart);
    next!(h, PartStart);
    next!(h, Delta);
    let m = next!(h, Message);
    assert_eq!(
        (m.role.as_str(), m.parts[0].text.as_str()),
        ("assistant", "fixed")
    );
    assert_eq!(next!(h, Init).session_id, "old");
}

// acp_test.go: TestStopReason
#[test]
fn stop_reasons() {
    for (from, want) in [
        ("end_turn", "done"),
        ("cancelled", "interrupted"),
        ("max_tokens", "max_tokens"),
        ("max_turn_requests", "max_turns"),
        ("refusal", "refusal"),
    ] {
        assert_eq!(stop_reason(from), want, "{from}");
    }
}

// acp_test.go: TestKindOf
#[test]
fn kinds() {
    for (from, want) in [
        ("read", Kind::Read),
        ("edit", Kind::Edit),
        ("delete", Kind::Delete),
        ("move", Kind::Move),
        ("search", Kind::Search),
        ("execute", Kind::Shell),
        ("think", Kind::Think),
        ("fetch", Kind::Fetch),
        ("other", Kind::Other),
        ("", Kind::Other),
    ] {
        assert_eq!(kind_of(from), want, "{from}");
    }
}

/// A name, a kind, its raw input, locations and content, and the input read.
type Case<'a> = (
    &'a str,
    Kind,
    &'a str,
    Vec<Location>,
    Vec<ToolContent>,
    tool::Input,
);

// acp_test.go: TestReadInput
#[test]
fn inputs() {
    let raw = |s: &str| rush_core::wire::RawJson::new(s).unwrap();
    let rep = |path: &str, old: &str, new: &str| tool::Replace {
        path: path.into(),
        old: old.into(),
        new: new.into(),
        ..tool::Replace::default()
    };
    let diff = |path: &str, old: Option<&str>, new: &str| ToolContent {
        kind: "diff".into(),
        path: path.into(),
        old_text: old.map(String::from),
        new_text: new.into(),
        ..ToolContent::default()
    };
    let loc = |p: &str| Location {
        path: p.into(),
        line: None,
    };
    let cases: Vec<Case> = vec![
        (
            "shell string",
            Kind::Shell,
            r#"{"command":"go test","description":"Run tests"}"#,
            vec![],
            vec![],
            tool::Input {
                command: "go test".into(),
                description: "Run tests".into(),
                ..tool::Input::default()
            },
        ),
        (
            "read by location",
            Kind::Read,
            "{}",
            vec![loc("/a")],
            vec![],
            tool::Input {
                path: "/a".into(),
                ..tool::Input::default()
            },
        ),
        (
            "search query",
            Kind::Search,
            r#"{"query":"foo","path":"/src"}"#,
            vec![],
            vec![],
            tool::Input {
                pattern: "foo".into(),
                path: "/src".into(),
                ..tool::Input::default()
            },
        ),
        (
            "fetch",
            Kind::Fetch,
            r#"{"url":"https://x.dev"}"#,
            vec![],
            vec![],
            tool::Input {
                url: "https://x.dev".into(),
                ..tool::Input::default()
            },
        ),
        (
            "move",
            Kind::Move,
            r#"{"source":"/a","destination":"/b"}"#,
            vec![],
            vec![],
            tool::Input {
                path: "/a".into(),
                to: "/b".into(),
                ..tool::Input::default()
            },
        ),
        (
            "edit strings",
            Kind::Edit,
            r#"{"file_path":"/a","old_string":"x","new_string":"y"}"#,
            vec![],
            vec![],
            tool::Input {
                path: "/a".into(),
                edits: vec![rep("", "x", "y")],
                ..tool::Input::default()
            },
        ),
        (
            "diffs win",
            Kind::Edit,
            r#"{"old_string":"q","new_string":"r"}"#,
            vec![],
            vec![diff("/a", Some("x"), "y"), diff("/b", None, "z")],
            tool::Input {
                path: "/a".into(),
                edits: vec![rep("", "x", "y"), rep("/b", "", "z")],
                ..tool::Input::default()
            },
        ),
    ];
    for (name, k, r, locs, content, want) in cases {
        assert_eq!(
            read_input(k, Some(&raw(r)), &locs, &content),
            want,
            "{name}"
        );
    }
}

// acp_test.go: TestPatch
#[test]
fn patches() {
    let p = |os, ol, ns, nl, lines: &[&str]| tool::Patch {
        old_start: os,
        old_lines: ol,
        new_start: ns,
        new_lines: nl,
        lines: lines.iter().map(|s| s.to_string()).collect(),
        ..tool::Patch::default()
    };
    for (old, new, want) in [
        (None, "a\nb\n", p(0, 0, 1, 2, &["+a", "+b"])),
        (Some("a\nb\nc"), "a\nc", p(2, 1, 1, 0, &["-b"])),
        (Some("a"), "a\nb", p(1, 0, 2, 1, &["+b"])),
    ] {
        let d = ToolContent {
            old_text: old.map(String::from),
            new_text: new.into(),
            ..ToolContent::default()
        };
        assert_eq!(patch(&d), want, "{old:?} → {new:?}");
    }
}

// acp_test.go: TestKnownAgentsRegister
#[test]
fn known_agents_register() {
    let reg = rush_core::adapter::Registry::new();
    crate::register(&reg);
    for k in ["kimi", "opencode", "vibe", "gemini", "glm"] {
        let a = reg.get(k).unwrap_or_else(|| panic!("{k} isn't registered"));
        assert!(a.driver().is_some(), "{k} can't be driven");
    }
    assert_eq!(reg.label("glm"), "GLM (ZCode)");
}

// acp_test.go: TestThinkingConfig
#[test]
fn thinking_config() {
    let (s, mut f, h) = start();
    next!(h, Init);
    f.update(json!({"sessionUpdate": "config_option_update", "configOptions": [{"type": "select", "id": "thinking", "category": "thinking", "currentValue": "low"}]}));
    assert_eq!(next!(h, Init).effort, "low");
    let s = Arc::new(s);
    let s2 = s.clone();
    let done = std::thread::spawn(move || s2.set_effort("high"));
    let m = f.expect("session/set_config_option");
    assert_eq!(
        m["params"].to_string(),
        r#"{"configId":"thinking","sessionId":"s1","value":"high"}"#
    );
    f.result(&m["id"], json!({"configOptions": [{"type": "select", "id": "thinking", "category": "thinking", "currentValue": "high"}]}));
    done.join().unwrap().unwrap();
    assert_eq!(next!(h, Init).effort, "high");
}

// acp_test.go: TestTrustedToolAllowed
#[test]
fn trusted_tool_allowed() {
    let (s, mut f, h) = open(
        "",
        None,
        Options {
            trusted: vec!["mcp__rush__spawn_agent".into()],
            ..Options::default()
        },
    );
    next!(h, Init);
    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "mcp__rush__spawn_agent", "kind": "other", "status": "pending"}));
    assert_eq!(
        call_of(&next!(h, Message)).name,
        "mcp__rush__spawn_agent",
        "an other call goes by its title"
    );
    f.request(
        5,
        "session/request_permission",
        json!({"sessionId": "s1", "toolCall": {"toolCallId": "c1"},
        "options": [{"optionId": "no", "name": "Reject", "kind": "reject_once"},
            {"optionId": "yes", "name": "Allow", "kind": "allow_once"}]}),
    );
    let reply = f.recv();
    assert_eq!(reply["id"], 5);
    assert_eq!(
        reply["result"].to_string(),
        r#"{"outcome":{"optionId":"yes","outcome":"selected"}}"#
    );
    drop(s);
}

// tasks_test.go: TestShellCall
#[test]
fn shell_call() {
    let (_s, mut f, h) = start();
    next!(h, Init);
    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Bash", "kind": "execute", "status": "in_progress",
        "rawInput": {"command": "go test ./...", "description": "Run tests"}}));
    let m = next!(h, Message);
    let c = call_of(&m);
    assert_eq!(
        (c.kind, c.input.command.as_str(), c.input.background),
        (Kind::Shell, "go test ./...", false)
    );
}

// tasks_test.go: TestSubagentCall: Copilot's subagent, kind other, a
// prompt and an agent type. It's a task until its call ends.
#[test]
fn subagent_call() {
    let (_s, mut f, h) = start();
    next!(h, Init);
    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "say ok", "kind": "other", "status": "pending",
        "rawInput": {"name": "say-ok", "prompt": "Reply ok.", "agent_type": "general-purpose", "description": "say ok"}}));
    let m = next!(h, Message);
    let c = call_of(&m);
    assert_eq!(
        (
            c.kind,
            c.input.description.as_str(),
            c.input.prompt.as_str(),
            c.input.agent.as_str()
        ),
        (Kind::Subagent, "say ok", "Reply ok.", "general-purpose")
    );
    assert_eq!(
        next!(h, TaskStarted),
        event::TaskStarted {
            id: "c1".into(),
            call_id: "c1".into(),
            kind: TaskKind::Subagent,
            label: "say ok".into(),
            agent: "general-purpose".into(),
            background: false,
        }
    );
    f.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed", "rawOutput": {"content": "ok"}}));
    assert_eq!(output_of(&next!(h, Message)).text, "ok");
    assert_eq!(
        next!(h, TaskDone),
        event::TaskDone {
            id: "c1".into(),
            call_id: "c1".into(),
            status: "completed".into(),
            ..event::TaskDone::default()
        }
    );
}

// tasks_test.go: TestBackgroundShell: Kimi's background shell, its result
// names the task, and a notification later says it ended.
#[test]
fn background_shell() {
    let (_s, mut f, h) = start();
    next!(h, Init);
    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Bash", "kind": "execute", "status": "in_progress",
        "rawInput": {"command": "sleep 2; echo hi", "run_in_background": true, "description": "sleep then hi"}}));
    let m = next!(h, Message);
    assert_eq!(
        (call_of(&m).kind, call_of(&m).input.background),
        (Kind::Shell, true)
    );
    f.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
        "content": [{"type": "content", "content": {"type": "text", "text": "task_id: bash-x1\npid: 1\nstatus: running"}}]}));
    next!(h, CallUpdated);
    next!(h, Message);
    assert_eq!(
        next!(h, TaskStarted),
        event::TaskStarted {
            id: "bash-x1".into(),
            call_id: "c1".into(),
            kind: TaskKind::Shell,
            label: "sleep then hi".into(),
            agent: String::new(),
            background: true,
        }
    );
    assert_eq!(
        next!(h, Background).tasks,
        [event::BackgroundTask {
            id: "bash-x1".into(),
            kind: TaskKind::Shell,
            label: "sleep then hi".into(),
            ..event::BackgroundTask::default()
        }]
    );
    f.update(text(
        "user_message_chunk",
        r#"<notification id="task:bash-x1:completed" category="task" type="task.completed" source_kind="background_task" source_id="bash-x1">"#,
    ));
    assert_eq!(
        next!(h, TaskDone),
        event::TaskDone {
            id: "bash-x1".into(),
            call_id: "c1".into(),
            status: "completed".into(),
            ..event::TaskDone::default()
        }
    );
    assert!(next!(h, Background).tasks.is_empty());
}

// question_cancel_test.go: TestQuestionCancellationReleasesHostWait
#[test]
fn question_cancellation_releases_the_host() {
    for interrupt in [false, true] {
        let (s, mut f, h) = start();
        next!(h, Init);
        s.send(input("go")).unwrap();
        let prompt = f.expect("session/prompt");
        f.request(41, "elicitation/create", json!({"mode": "form", "message": "Form", "requestedSchema": {"type": "object", "properties": {"answer": {"type": "string", "title": "Choose"}}}}));
        let q = next!(h, Question);
        if interrupt {
            s.interrupt().unwrap();
            f.expect("session/cancel");
        } else {
            let id = serde_json::value::RawValue::from_string("41".into()).unwrap();
            s.inner.withdraw(&id);
        }
        let reply = f.recv();
        assert_eq!(reply["id"], 41, "{reply}");
        if interrupt {
            assert_eq!(reply["result"].to_string(), r#"{"action":"cancel"}"#);
        } else {
            assert_eq!(reply["error"]["code"], crate::rpc::CODE_CANCELLED);
        }
        assert_eq!(next!(h, ApprovalCancelled).id, q.id);
        f.result(&prompt["id"], json!({"stopReason": "cancelled"}));
        assert_eq!(next!(h, TurnEnd).reason, "interrupted");
        let late = HashMap::from([("answer".to_string(), vec!["late".to_string()])]);
        assert!(
            s.answerer().unwrap().answer_question(&q.id, &late).is_err(),
            "a cancelled question still takes answers"
        );
        s.send(input("continue")).unwrap();
        let next = f.expect("session/prompt");
        f.result(&next["id"], json!({"stopReason": "end_turn"}));
        assert_eq!(next!(h, TurnEnd).reason, "done");
    }
}

// kimi_question_test.go: TestKimiBackgroundQuestionOutlivesItsCall
#[test]
fn kimi_background_question_outlives_its_call() {
    let (s, mut f, h) = start();
    next!(h, Init);
    s.send(input("go")).unwrap();
    f.expect("session/prompt");
    let inp = r#"{"background":true,"questions":[{"question":"Tea?","options":[{"label":"Yes"},{"label":"No"}]}]}"#;
    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "1:q", "title": "AskUserQuestion", "kind": "other", "status": "in_progress",
        "content": [{"type": "content", "content": {"type": "text", "text": inp}}]}));
    next!(h, Message); // the call
    f.request(7, "elicitation/create", json!({"sessionId": "s1", "toolCallId": "1:q", "mode": "form", "message": "Tea?",
        "requestedSchema": {"type": "object", "properties": {
            "q0": {"type": "string", "title": "Tea?", "oneOf": [{"const": "Yes", "title": "Yes"}]}}}}));
    assert_eq!(
        next!(h, Question).call_id,
        "",
        "a background question tied to its call"
    );
    assert_eq!(lock(&s.inner.mu).calls["1:q"].c.kind, Kind::Question);
    f.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "1:q", "status": "completed",
        "content": [{"type": "content", "content": {"type": "text", "text": "task_id: question-x\nstatus: running"}}]}));
    next!(h, CallUpdated);
    next!(h, Message); // its result
    // The lock is had once the update has been read whole.
    let st = lock(&s.inner.mu);
    assert!(st.calls["1:q"].done);
    assert_eq!(st.calls["1:q"].task, "", "a question started a task");
}

// permission_modes_test.go: TestPermissionModesKeepProtocolIDs
#[test]
fn permission_modes_keep_protocol_ids() {
    let mut st = crate::session::State {
        modes: vec![Mode {
            id: "urn:modes:ask".into(),
            name: "Ask".into(),
            description: "ask first".into(),
        }],
        mode: "urn:modes:ask".into(),
        ..crate::session::State::default()
    };
    let modes = permission_modes(&st);
    assert_eq!(modes.len(), 1);
    assert_eq!(
        (modes[0].id.as_str(), modes[0].name.as_str()),
        ("urn:modes:ask", "Ask")
    );
    st.modes.clear();
    let v = |value: &str, name: &str| ConfigValue {
        value: value.into(),
        name: name.into(),
        ..ConfigValue::default()
    };
    set_config(
        &mut st,
        Some(vec![ConfigOption {
            id: "approval".into(),
            category: "mode".into(),
            current_value: Some(
                serde_json::value::RawValue::from_string(r#""ask""#.into()).unwrap(),
            ),
            options: vec![ConfigValue {
                name: "Presets".into(),
                options: vec![v("ask", "Ask first"), v("yolo", "Allow tools")],
                ..ConfigValue::default()
            }],
            ..ConfigOption::default()
        }]),
    );
    let modes = permission_modes(&st);
    assert_eq!(modes.len(), 2);
    assert_eq!(modes[1].id, "yolo");
    assert_eq!(st.mode, "ask");
}

// permission_modes_test.go: TestInitReportsRuntimePermissionChoices
#[test]
fn init_reports_the_modes() {
    let (_s, _f, h) = start();
    let init = next!(h, Init);
    assert_eq!(init.mode_id, "default");
    assert!(!init.modes.is_empty());
}

// tasks_ended_test.go: TestEndTasksFromKimiRecords
#[test]
fn end_tasks_from_kimi_records() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path();
    let dir = home.join("sessions/wd_x/session_abc/agents/main/tasks");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bash-done.json"), r#"{"status":"completed"}"#).unwrap();
    std::fs::write(dir.join("bash-live.json"), r#"{"status":"running"}"#).unwrap();
    let (tx, rx) = channel();
    let s = Session::bare(
        Options {
            adapter: "kimi".into(),
            env: vec![format!("KIMI_CODE_HOME={}", home.display())],
            ..Options::default()
        },
        Arc::new(Tagged { key: (), tx }),
    );
    {
        let mut st = lock(&s.inner.mu);
        st.id = "session_abc".into();
        for (id, task) in [("c1", "bash-done"), ("c2", "bash-live")] {
            st.calls.insert(
                id.into(),
                crate::update::CallState {
                    c: tool::Call {
                        id: id.into(),
                        input: tool::Input {
                            background: true,
                            ..tool::Input::default()
                        },
                        ..tool::Call::default()
                    },
                    task: task.into(),
                    ..crate::update::CallState::default()
                },
            );
        }
    }
    assert_eq!(s.end_tasks(), ["bash-done"]);
    {
        let st = lock(&s.inner.mu);
        assert!(st.calls["c1"].ended && !st.calls["c2"].ended);
    }
    assert!(s.end_tasks().is_empty(), "ended twice");
    let h = Heard(rx);
    assert_eq!(next!(h, TaskDone).id, "bash-done");
    assert_eq!(next!(h, Background).tasks.len(), 1);
}

// kimi_children_test.go: TestKimiLiveSubagentChild
#[test]
fn kimi_live_subagent_child() {
    let (_s, mut f, h) = open(
        "",
        None,
        Options {
            adapter: "kimi".into(),
            finds_children: true,
            ..Options::default()
        },
    );
    next!(h, Init);
    f.update(json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Agent", "kind": "other", "status": "pending",
        "rawInput": {"prompt": "say ok", "description": "ok", "subagent_type": "explore"}}));
    let m = next!(h, Message);
    assert_eq!(
        (call_of(&m).kind, call_of(&m).input.child.as_str()),
        (Kind::Subagent, "c1")
    );
}

/// Writes a program at dir/name that runs script.
fn program(dir: &std::path::Path, name: &str, script: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.to_string_lossy().into_owned()
}

// starterr_test.go: TestStartRefusedKeepsTheAgentsWords: an agent that
// refuses the session says why in its answer; what it dumps on stderr
// isn't added to it.
#[test]
fn start_refused_keeps_the_agents_words() {
    let d = tempfile::tempdir().unwrap();
    let script = r#"reply() { id=$(printf '%s' "$1" | sed 's/.*"id":\([0-9]*\).*/\1/'); printf '{"jsonrpc":"2.0","id":%s,%s}\n' "$id" "$2"; }
read -r l; reply "$l" '"result":{"protocolVersion":1}'
read -r l; echo '}' >&2; reply "$l" '"error":{"code":-32000,"message":"no key"}'
cat >/dev/null"#;
    let cmd = program(d.path(), "agent", script);
    let (tx, _rx) = channel();
    let err = Session::start(
        Options {
            command: cmd,
            dir: d.path().to_string_lossy().into_owned(),
            within: Some(Duration::from_secs(10)),
            ..Options::default()
        },
        Arc::new(Tagged { key: (), tx }),
    )
    .err()
    .unwrap();
    assert!(err.to_string().ends_with("no key (-32000)"), "{err}");
    assert_eq!(err.to_string(), "acp: open session: acp: no key (-32000)");
}

// glm_test.go: TestStart: a session starts over ACP through the bridge, in
// the mode asked for.
#[test]
fn glm_starts_through_the_bridge() {
    use rush_core::adapter::Driver;
    let d = tempfile::tempdir().unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/zcode-acp.jsonl");
    let log = d.path().join("requests");
    let script = format!(
        r#"while IFS= read -r resp <&3; do
	IFS= read -r req || exit 0
	printf '%s\n' "$req" >> '{}'
	printf '%s\n' "$resp"
done 3< '{}'
cat > /dev/null"#,
        log.display(),
        fixture.display()
    );
    let bin = program(d.path(), "zcode-acp-server", &script);
    let (tx, rx) = channel();
    let work = tempfile::tempdir().unwrap();
    let conn = crate::glm::Glm::new()
        .start(
            StartOptions {
                binary: bin,
                dir: work.path().to_string_lossy().into_owned(),
                mode: "plan".into(),
                ..StartOptions::default()
            },
            Arc::new(Tagged { key: (), tx }),
        )
        .unwrap();
    assert!(conn.pid().is_some());
    let h = Heard(rx);
    let mut last = next!(h, Init);
    while last.mode != "plan" {
        last = next!(h, Init);
    }
    assert_eq!(
        (last.session_id.as_str(), last.model.as_str()),
        ("tsk_7f3a9c", "glm-5.3")
    );
    let reqs = std::fs::read_to_string(&log).unwrap();
    let reqs: Vec<&str> = reqs.trim().split('\n').collect();
    assert_eq!(reqs.len(), 3, "{reqs:?}");
    assert!(reqs[2].contains(r#""session/set_mode""#), "{reqs:?}");
    conn.close().unwrap();
    // Closed, the sink hears it ended, and nothing after.
    loop {
        match h.0.recv_timeout(Duration::from_secs(3)) {
            Ok(Said::Ended(..)) => break,
            Ok(_) => {}
            Err(e) => panic!("never ended: {e}"),
        }
    }
    assert!(h.0.try_recv().is_err(), "said something after it ended");
}

#[test]
fn the_sink_hears_the_end_once_the_agent_goes() {
    let (s, f, h) = start();
    next!(h, Init);
    s.send(input("go")).unwrap();
    // The agent goes mid-turn: the turn ends in an error, then the session.
    drop(f);
    let end = next!(h, TurnEnd);
    assert_eq!(
        (end.reason.as_str(), end.err.as_str()),
        ("error", "acp: connection closed")
    );
    match h.0.recv_timeout(Duration::from_secs(3)) {
        Ok(Said::Ended((), None)) => {}
        other => panic!("{other:?}"),
    }
    assert!(
        s.send(input("again")).is_err(),
        "sent to a session that's gone"
    );
}
