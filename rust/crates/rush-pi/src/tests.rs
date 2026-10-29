//! Port of internal/adapters/pi/pi_test.go:1-591: recorded RPC sessions
//! fed through the handler, a fake pi on the far end of two pipes, session
//! files read back.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_core::adapter::{
    Adapter, Conn, Discoverer, HistoryFollower, HistoryReader, Input, Orphans, Profile, Registry,
    Said, Session, Sink, StartOptions, Tagged, read_history,
};
use rush_core::event::{Event, PartKind};
use rush_core::tool::{self, Patch, Replace};
use rush_core::usage::TokenUsage;
use serde_json::{Value, json};

use crate::conn::{PiConn, Shared, args_for};
use crate::history::read_all;
use crate::rpc::Client;
use crate::{Pi, discover};

/// Keeps what a session says.
#[derive(Default)]
struct Keep(Mutex<Vec<Event>>);

impl Sink for Keep {
    fn event(&self, ev: Event) {
        self.0.lock().unwrap().push(ev);
    }
    fn ended(&self, _: Option<rush_core::adapter::Error>) {}
}

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

/// Runs a recorded RPC session through a session's handler, and is what
/// it said.
fn feed(sh: &Shared, name: &str) -> Vec<Event> {
    let b = std::fs::read(testdata(name)).unwrap();
    let mut out = Vec::new();
    for l in b.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        let head: Value = serde_json::from_slice(l).unwrap();
        out.extend(sh.events(head["type"].as_str().unwrap_or_default(), l));
    }
    out
}

fn shared(id: &str, provider: &str, window: i64, sent: &[&str]) -> Arc<Shared> {
    let sh = Shared::new(Arc::new(Keep::default()));
    {
        let mut s = sh.st.lock().unwrap();
        s.model = crate::conn::Model {
            id: id.into(),
            provider: provider.into(),
            context_window: window,
        };
        s.sent = sent.iter().map(|s| s.to_string()).collect();
    }
    sh
}

fn messages(evs: &[Event]) -> Vec<&rush_core::event::Message> {
    evs.iter()
        .filter_map(|e| match e {
            Event::Message(m) => Some(m),
            _ => None,
        })
        .collect()
}

fn turn_ends(evs: &[Event]) -> Vec<&rush_core::event::TurnEnd> {
    evs.iter()
        .filter_map(|e| match e {
            Event::TurnEnd(t) => Some(t),
            _ => None,
        })
        .collect()
}

// A turn on a thinking model that reads a file, edits it, reads it again
// and says so: pi 0.85 with qwen3.8 on Ollama.
#[test]
fn edit_turn() {
    let sh = shared(
        "qwen3.8:27b-mlx",
        "ollama",
        65536,
        &["Use the edit tool to change the line two to three in b.txt. Think briefly first."],
    );
    let evs = feed(&sh, "edit.jsonl");
    assert!(
        matches!(&evs[0], Event::Status(s) if s.busy),
        "first {:?}",
        evs[0]
    );
    assert!(
        sh.st.lock().unwrap().sent.is_empty(),
        "the prompt sent wasn't taken as said back"
    );
    let (mut calls, mut outs) = (Vec::new(), Vec::new());
    for m in messages(&evs) {
        assert!(
            !(m.role == "user" && m.parts[0].kind == PartKind::Text),
            "the prompt rush sent came back as a message: {m:?}"
        );
        for p in &m.parts {
            match p.kind {
                PartKind::ToolCall => calls.push(p.call.as_deref().unwrap().clone()),
                PartKind::ToolResult => outs.push(p.output.as_deref().unwrap().clone()),
                _ => {}
            }
        }
    }
    assert_eq!((calls.len(), outs.len()), (3, 3));
    let edit = &calls[1];
    assert_eq!(edit.kind, tool::Kind::Edit);
    assert_eq!(edit.input.path, "b.txt");
    assert_eq!(
        edit.input.edits,
        vec![Replace {
            old: "two".into(),
            new: "three".into(),
            ..Replace::default()
        }]
    );
    assert_eq!(
        (calls[0].kind, calls[0].input.path.as_str()),
        (tool::Kind::Read, "b.txt")
    );
    let want = vec![Patch {
        old_start: 1,
        old_lines: 2,
        new_start: 1,
        new_lines: 2,
        lines: vec![" one".into(), "-two".into(), "+three".into()],
        ..Patch::default()
    }];
    assert_eq!(outs[1].call_id, edit.id);
    assert_eq!(outs[1].patches, want);
    assert_eq!(outs[0].text, "one\ntwo\n");
    assert!(!outs[0].is_error);

    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::PartStart(p) if p.kind == PartKind::Thinking)),
        "no thinking part started"
    );
    assert!(
        evs.iter().any(
            |e| matches!(e, Event::Delta(d) if d.kind == PartKind::Thinking && !d.text.is_empty())
        ),
        "no thinking streamed"
    );
    let starts: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            Event::MessageStart(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 4);
    assert_eq!(
        (starts[0].id.as_str(), starts[0].model.as_str()),
        ("chatcmpl-300", "qwen3.8:27b-mlx")
    );
    let cx: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            Event::Context(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(cx.len(), 4);
    assert_eq!((cx[3].window, cx[3].tokens), (65536, 1941));

    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 1, "{ends:?}");
    let end = ends[0];
    assert_eq!(end.reason, "done");
    assert!(end.err.is_empty());
    assert!(
        end.text.starts_with("Done. `b.txt` now contains"),
        "{}",
        end.text
    );
    assert_eq!(end.turns, 1);
    let mut sum = TokenUsage::default();
    for m in messages(&evs) {
        if let Some(t) = m.tokens {
            sum.add(t);
        }
    }
    assert_eq!(end.tokens, sum);
    assert!(sum.output > 0);
    assert!(matches!(evs.last(), Some(Event::TurnEnd(_))));
}

// An abort mid-answer ends that run as interrupted; the message steered in
// meanwhile then runs as a run of its own.
#[test]
fn abort_then_steer() {
    let sh = shared(
        "qwen2.5:1.5b",
        "ollama",
        32768,
        &["Write a 500 word poem.", "shorter"],
    );
    let evs = feed(&sh, "abort.jsonl");
    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 2, "{ends:?}");
    assert_eq!(
        (ends[0].reason.as_str(), ends[0].err.as_str()),
        ("interrupted", "")
    );
    assert_eq!(ends[1].reason, "done");
    assert_eq!(
        evs.iter().filter(|e| matches!(e, Event::Status(_))).count(),
        2
    );
    for m in messages(&evs) {
        assert_ne!(m.role, "user", "a message rush sent came back: {m:?}");
    }
}

/// A pi on the far end of two pipes.
struct Fake {
    r: BufReader<PipeReader>,
    w: PipeWriter,
}

impl Fake {
    /// The next command rush sends, which must be of type typ.
    fn next(&mut self, typ: &str) -> Value {
        let mut l = String::new();
        assert!(self.r.read_line(&mut l).unwrap() > 0, "pipe closed");
        let m: Value = serde_json::from_str(&l).unwrap();
        assert_eq!(m["type"], typ, "got {l}");
        m
    }
    fn write(&mut self, v: Value) {
        let mut b = serde_json::to_vec(&v).unwrap();
        b.push(b'\n');
        self.w.write_all(&b).unwrap();
    }
    fn ok(&mut self, cmd: &Value, data: Value) {
        self.write(json!({"id": cmd["id"], "type": "response", "command": cmd["type"], "success": true, "data": data}));
    }
    fn fail(&mut self, cmd: &Value, msg: &str) {
        self.write(json!({"id": cmd["id"], "type": "response", "command": cmd["type"], "success": false, "error": msg}));
    }
}

type Rx = Receiver<Said<u8>>;

/// The next event of rx that pick takes.
fn next<T>(rx: &Rx, pick: impl Fn(Event) -> Option<T>) -> T {
    loop {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Said::Event(_, ev)) => {
                if let Some(t) = pick(ev) {
                    return t;
                }
            }
            Ok(Said::Ended(..)) => panic!("events ended"),
            Err(e) => panic!("no event: {e}"),
        }
    }
}

/// A session against a fake, through get_state and get_commands.
fn start() -> (Arc<PiConn>, Fake, Rx) {
    let (cr, fw) = std::io::pipe().unwrap(); // fake → conn
    let (fr, cw) = std::io::pipe().unwrap(); // conn → fake
    let mut f = Fake {
        r: BufReader::new(fr),
        w: fw,
    };
    let (tx, rx) = mpsc::channel();
    let sh = Shared::new(Arc::new(Tagged { key: 0u8, tx }));
    let rpc = Arc::new(Client::new(Box::new(cw)));
    let h = sh.clone();
    rpc.clone().start(
        cr,
        Box::new(move |t, b| h.handle(t, b)),
        PiConn::done_for(&sh),
    );
    let c = Arc::new(PiConn::on(sh, rpc));
    let c2 = c.clone();
    let begun = std::thread::spawn(move || {
        c2.begin(&StartOptions {
            dir: "/work".into(),
            ..StartOptions::default()
        })
    });
    let st = f.next("get_state");
    f.ok(
        &st,
        json!({"sessionId": "s1", "sessionFile": "/x/s1.jsonl",
            "model": {"id": "qwen2.5:1.5b", "provider": "ollama", "contextWindow": 32768}}),
    );
    begun.join().unwrap().unwrap();
    let gc = f.next("get_commands");
    f.ok(
        &gc,
        json!({"commands": [{"name": "skill:x", "description": "does x", "source": "skill"}]}),
    );
    (c, f, rx)
}

fn bg<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> std::thread::JoinHandle<T> {
    std::thread::spawn(f)
}

#[test]
fn conn() {
    let (c, mut f, rx) = start();
    let init = next(&rx, |e| match e {
        Event::Init(i) => Some(i),
        _ => None,
    });
    assert_eq!(
        (
            init.session_id.as_str(),
            init.model.as_str(),
            init.cwd.as_str()
        ),
        ("s1", "qwen2.5:1.5b", "/work")
    );
    let cx = next(&rx, |e| match e {
        Event::Context(c) => Some(c),
        _ => None,
    });
    assert_eq!(cx.window, 32768);
    let cmds = next(&rx, |e| match e {
        Event::Commands(c) => Some(c),
        _ => None,
    });
    assert_eq!(cmds.list.len(), 1);
    assert_eq!(cmds.list[0].name, "skill:x");

    let send = |c: &Arc<PiConn>, text: &str| {
        let (c, text) = (c.clone(), text.to_string());
        bg(move || {
            c.send(Input {
                text,
                ..Input::default()
            })
            .map_err(|e| e.to_string())
        })
    };

    // Idle, a message is a prompt of its own.
    let done = send(&c, "hi");
    let p = f.next("prompt");
    assert_eq!(p["message"], "hi");
    assert!(p.get("streamingBehavior").is_none());
    f.ok(&p, Value::Null);
    done.join().unwrap().unwrap();
    f.write(json!({"type": "agent_start"}));
    let s = next(&rx, |e| match e {
        Event::Status(s) => Some(s),
        _ => None,
    });
    assert!(s.busy);

    // Busy, it steers.
    let done = send(&c, "and this");
    let p = f.next("prompt");
    assert_eq!(
        (&p["message"], &p["streamingBehavior"]),
        (&json!("and this"), &json!("steer"))
    );
    f.ok(&p, Value::Null);
    done.join().unwrap().unwrap();

    // An interrupt doesn't wait for pi's answer.
    c.interrupt().unwrap();
    let a = f.next("abort");
    assert!(a.get("id").is_none(), "abort waits on an answer: {a}");
    f.write(json!({"type": "agent_end", "messages": []}));
    let e = next(&rx, |e| match e {
        Event::TurnEnd(t) => Some(t),
        _ => None,
    });
    assert_eq!(e.reason, "done");

    // Pi busy on its own when rush thought it idle: the prompt steers.
    let done = send(&c, "late");
    let p = f.next("prompt");
    f.fail(
        &p,
        "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message.",
    );
    let p = f.next("prompt");
    assert_eq!(p["streamingBehavior"], "steer");
    f.ok(&p, Value::Null);
    done.join().unwrap().unwrap();

    // A model by its id, the provider's own first.
    let c2 = c.clone();
    let done = bg(move || c2.set_model("qwen3.8:27b-mlx").map_err(|e| e.to_string()));
    let gm = f.next("get_available_models");
    f.ok(
        &gm,
        json!({"models": [
            {"id": "qwen3.8:27b-mlx", "provider": "other"},
            {"id": "qwen3.8:27b-mlx", "provider": "ollama", "contextWindow": 65536},
        ]}),
    );
    let set = f.next("set_model");
    assert_eq!(
        (&set["provider"], &set["modelId"]),
        (&json!("ollama"), &json!("qwen3.8:27b-mlx"))
    );
    f.ok(
        &set,
        json!({"id": "qwen3.8:27b-mlx", "provider": "ollama", "contextWindow": 65536}),
    );
    done.join().unwrap().unwrap();
    let cx = next(&rx, |e| match e {
        Event::Context(c) => Some(c),
        _ => None,
    });
    assert_eq!(cx.window, 65536);
    assert!(c.set_mode("auto").is_err(), "pi took a mode");

    // /compact compacts, and ends the turn it began.
    c.send(Input {
        text: "/compact keep the tests".into(),
        ..Input::default()
    })
    .unwrap();
    let cp = f.next("compact");
    assert_eq!(cp["customInstructions"], "keep the tests");
    f.write(json!({"type": "compaction_start", "reason": "manual"}));
    f.write(json!({"type": "compaction_end", "reason": "manual", "result": {"summary": "s", "tokensBefore": 900}}));
    let cm = next(&rx, |e| match e {
        Event::Compacted(c) => Some(c),
        _ => None,
    });
    assert_eq!((cm.trigger.as_str(), cm.before), ("manual", 900));
    f.ok(&cp, json!({"summary": "s", "tokensBefore": 900}));
    let e = next(&rx, |e| match e {
        Event::TurnEnd(t) => Some(t),
        _ => None,
    });
    assert_eq!(e.reason, "done");

    // An extension's select is a question.
    f.write(json!({"type": "extension_ui_request", "id": "u1", "method": "select", "title": "Allow rm?", "options": ["Allow", "Block"]}));
    let q = next(&rx, |e| match e {
        Event::Question(q) => Some(q),
        _ => None,
    });
    assert_eq!(q.id, "u1");
    assert_eq!(
        (
            q.asks.len(),
            q.asks[0].options.len(),
            q.asks[0].text.as_str()
        ),
        (1, 2, "Allow rm?")
    );
    let ans = HashMap::from([("Allow rm?".to_string(), vec!["Block".to_string()])]);
    c.answerer().unwrap().answer_question("u1", &ans).unwrap();
    let r = f.next("extension_ui_response");
    assert_eq!((&r["id"], &r["value"]), (&json!("u1"), &json!("Block")));
    f.write(json!({"type": "extension_ui_request", "id": "u2", "method": "confirm", "title": "Clear?", "message": "All is lost."}));
    let q = next(&rx, |e| match e {
        Event::Question(q) => Some(q),
        _ => None,
    });
    assert_eq!(
        (q.asks[0].header.as_str(), q.asks[0].text.as_str()),
        ("Clear?", "All is lost.")
    );
    let ans = HashMap::from([("All is lost.".to_string(), vec!["Yes".to_string()])]);
    c.answerer().unwrap().answer_question("u2", &ans).unwrap();
    let r = f.next("extension_ui_response");
    assert_eq!(r["confirmed"], true);
    f.write(
        json!({"type": "extension_ui_request", "id": "u3", "method": "input", "title": "Name?"}),
    );
    let r = f.next("extension_ui_response");
    assert_eq!((&r["id"], &r["cancelled"]), (&json!("u3"), &json!(true)));
    assert!(c.answer("u1", "x").is_err(), "pi took an approval");

    // pi going ends the session, once and last.
    drop(f);
    loop {
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Said::Ended(_, err) => {
                assert!(err.is_none(), "{err:?}");
                break;
            }
            Said::Event(..) => {}
        }
    }
    c.close().unwrap();
}

const SESSION_ID: &str = "01a0ed43-bcca-7039-ad79-9d3cd0d16072";

fn session_text() -> String {
    std::fs::read_to_string(testdata("session.jsonl")).unwrap()
}

#[test]
fn history() {
    let b = session_text();
    let evs = read_all(&b, 0);
    let Event::Init(init) = &evs[0] else {
        panic!("init: {:?}", evs[0])
    };
    assert_eq!(
        (
            init.session_id.as_str(),
            init.model.as_str(),
            init.cwd.as_str()
        ),
        (SESSION_ID, "qwen3.8:27b-mlx", "/private/tmp/pitest/work")
    );
    let msgs = messages(&evs);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(
        msgs[0].parts[0].text,
        "Use the edit tool to change the line two to three in b.txt. Think briefly first."
    );
    let mut calls = 0;
    for m in &msgs {
        for p in &m.parts {
            if p.kind == PartKind::ToolCall {
                calls += 1;
            }
            if let Some(o) = &p.output
                && !o.patches.is_empty()
            {
                assert_eq!(o.patches[0].lines[2], "+three");
            }
        }
    }
    let ends = turn_ends(&evs);
    assert_eq!(calls, 3);
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].reason, "done");
    assert!(ends[0].duration > Duration::ZERO);

    // A branch left by going back isn't the conversation: a new prompt
    // made a child of the first leaves the rest behind.
    let first = b
        .lines()
        .find(|l| l.contains(r#""role":"user""#))
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .unwrap();
    let branched = format!(
        "{b}{{\"type\":\"message\",\"id\":\"zz000001\",\"parentId\":\"{first}\",\"timestamp\":\"2026-09-29T13:10:00.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"try again\",\"timestamp\":1}}}}\n"
    );
    let evs = read_all(&branched, 0);
    let msgs = messages(&evs);
    assert_eq!(msgs.len(), 2, "{msgs:?}");
    assert_eq!(msgs[1].parts[0].text, "try again");

    // A line Go can't read (its message's timestamp no number) isn't the
    // tip of a branch: the conversation is all still there.
    let bad = format!(
        "{b}{{\"type\":\"message\",\"id\":\"zz000002\",\"parentId\":\"{first}\",\"timestamp\":\"2026-09-29T13:10:00.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"lost\",\"timestamp\":\"soon\"}}}}\n"
    );
    assert_eq!(read_all(&bad, 0), read_all(&b, 0));

    // Before stops at the first entry written at or after it.
    let before = rush_core::time::parse_rfc3339("2026-09-29T13:05:00Z");
    assert_eq!(messages(&read_all(&b, before)).len(), 3);
}

/// A history read hands its events on a slice at a time, the same events
/// as a whole read.
#[test]
fn history_in_slices() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("s.jsonl");
    std::fs::write(&path, session_text()).unwrap();
    let s = Session {
        transcript: path.to_string_lossy().into_owned(),
        ..Session::default()
    };
    let whole = read_history(&Pi::new(), &s, 0).unwrap();
    let mut slices = Vec::new();
    Pi::new()
        .history(&s, 0, 1000, &mut |evs| {
            slices.push(evs);
            true
        })
        .unwrap();
    assert!(slices.len() > 2, "{} slices", slices.len());
    assert_eq!(slices.concat(), whole);
    let mut n = 0;
    Pi::new()
        .history(&s, 0, 1000, &mut |_| {
            n += 1;
            false
        })
        .unwrap();
    assert_eq!(n, 1, "stopped on false");
}

/// Following a session: what's there in slices, then what's added; a
/// prompt that goes back in the tree reads it again, as a reset.
#[test]
fn follows_as_it_grows() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("s.jsonl");
    let b = session_text();
    let lines: Vec<&str> = b.lines().collect();
    let head = lines[..4].join("\n") + "\n";
    std::fs::write(&path, &head).unwrap();
    let s = Session {
        transcript: path.to_string_lossy().into_owned(),
        ..Session::default()
    };
    let mut f = Pi::new().follow_history(&s).unwrap();
    assert_eq!(f.wait_on().paths, vec![path.clone()]);
    let sl = f.next(1 << 20).unwrap();
    assert!(!sl.reset && !sl.more);
    assert!(matches!(&sl.events[0], Event::Init(i) if i.model == "qwen3.8:27b-mlx"));
    assert_eq!(messages(&sl.events).len(), 1);
    // The rest, as pi writes it, a line still being written waiting.
    let rest = lines[4..].join("\n") + "\n";
    {
        let mut w = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        w.write_all(&rest.as_bytes()[..rest.len() - 10]).unwrap();
    }
    let mut got = Vec::new();
    loop {
        let sl = f.next(1500).unwrap();
        assert!(!sl.reset);
        got.extend(sl.events);
        if !sl.more {
            break;
        }
    }
    {
        let mut w = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        w.write_all(&rest.as_bytes()[rest.len() - 10..]).unwrap();
    }
    got.extend(f.next(1 << 20).unwrap().events);
    let whole = read_all(&b, 0);
    assert_eq!(messages(&got).len() + 1, messages(&whole).len());
    assert_eq!(turn_ends(&got).len(), 1);
    // Back in the tree: a reset with only the new branch.
    let first = serde_json::from_str::<Value>(lines[3]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    {
        let mut w = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            w,
            "{{\"type\":\"message\",\"id\":\"zz000001\",\"parentId\":\"{first}\",\"timestamp\":\"2026-09-29T13:10:00.000Z\",\"message\":{{\"role\":\"user\",\"content\":\"try again\",\"timestamp\":1}}}}"
        )
        .unwrap();
    }
    let sl = f.next(1 << 20).unwrap();
    assert!(sl.reset);
    let msgs = messages(&sl.events);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].parts[0].text, "try again");
}

#[test]
fn past_and_resume() {
    let d = tempfile::tempdir().unwrap();
    let p = Profile {
        kind: "ollama-pi".into(),
        dir: d.path().to_string_lossy().into_owned(),
        ..Profile::default()
    };
    let b = session_text();
    let path = d
        .path()
        .join("sessions/--private-tmp-pitest-work--")
        .join(format!("2026-09-29T13-03-54-573Z_{SESSION_ID}.jsonl"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &b).unwrap();
    let path_s = path.to_string_lossy().into_owned();
    let pi = Pi::new();
    let past = pi.past(&p);
    assert_eq!(past.len(), 1);
    let s = &past[0];
    assert_eq!(s.id, SESSION_ID);
    assert_eq!(s.kind.as_str(), "ollama-pi");
    assert_eq!(s.model, "qwen3.8:27b-mlx");
    assert_eq!(s.cwd, "/private/tmp/pitest/work");
    assert!(s.name.starts_with("Use the edit tool"), "{}", s.name);
    assert_eq!(s.transcript, path_s);
    // Written just now, it runs, and is working.
    let live = pi.live(&p);
    assert_eq!((live.len(), live[0].state.as_str()), (1, "working"));

    // A name given with /name wins over the first prompt.
    let tip = serde_json::from_str::<Value>(b.trim().lines().last().unwrap()).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let named = format!(
        "{b}{{\"type\":\"session_info\",\"id\":\"n1\",\"parentId\":\"{tip}\",\"timestamp\":\"2026-09-29T13:10:00.000Z\",\"name\":\"Edit b\"}}\n"
    );
    std::fs::write(&path, named).unwrap();
    assert_eq!(pi.past(&p)[0].name, "Edit b");
    let by_id = Session {
        profile: p.clone(),
        id: s.id.clone(),
        ..Session::default()
    };
    assert!(read_history(&pi, &by_id, 0).unwrap().len() >= 5);

    let o = StartOptions {
        profile: p.clone(),
        session_id: s.id.clone(),
        resume: true,
        model: "ollama/q".into(),
        effort: "low".into(),
        ..StartOptions::default()
    };
    let args = args_for(&o).unwrap();
    assert_eq!(
        args,
        [
            "--mode",
            "rpc",
            "--session",
            &path_s,
            "--model",
            "ollama/q",
            "--thinking",
            "low"
        ]
    );
    let mut cmdline: Vec<String> = vec!["node".into(), "pi".into()];
    cmdline.extend(args.iter().cloned());
    assert!(pi.runs_session(&cmdline, &s.id));
    assert!(!pi.runs_session(&args, "other"));
    let fork = args_for(&StartOptions {
        profile: p.clone(),
        session_id: s.id.clone(),
        fork: true,
        ..StartOptions::default()
    })
    .unwrap();
    assert!(fork.iter().any(|a| a == "--fork"));
    let new = args_for(&StartOptions {
        profile: p.clone(),
        session_id: "new-one".into(),
        ..StartOptions::default()
    })
    .unwrap();
    assert_eq!(new, ["--mode", "rpc", "--session-id", "new-one"]);
    assert!(
        args_for(&StartOptions {
            profile: p,
            session_id: "gone".into(),
            resume: true,
            ..StartOptions::default()
        })
        .is_err(),
        "resumed a session that isn't there"
    );
}

// A pi folder left by an uninstalled pi isn't a profile. Pi's folder is
// PI_CODING_AGENT_DIR, set for the whole test process here: no other test
// reads it.
#[test]
fn profiles_need_pi_installed() {
    let (home, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let dir = home.path().join(".pi/agent");
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: no other test reads PI_CODING_AGENT_DIR.
    unsafe { std::env::set_var("PI_CODING_AGENT_DIR", &dir) };
    let reg = Registry::with_env(
        &bin.path().to_string_lossy(),
        &home.path().to_string_lossy(),
    );
    let pi = Arc::new(Pi::with_registry(reg.clone()));
    reg.register_ref(pi.clone());
    if !reg.path(crate::KIND).is_empty() {
        // pi is installed where every machine looks: only the half that
        // needs it installed can be tried.
        assert_eq!(pi.profiles().len(), 1);
        reg.unregister(crate::KIND);
        return;
    }
    assert!(pi.profiles().is_empty(), "pi isn't installed");
    let exe = bin.path().join("pi");
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    reg.recheck();
    let ps = pi.profiles();
    assert_eq!(ps.len(), 1, "pi is installed, profiles = {ps:?}");
    assert_eq!(ps[0].dir, dir.to_string_lossy());
    reg.unregister(crate::KIND);
}

// A session read back draws the subagent extension's calls as subagents:
// one agent's task, and several in parallel.
#[test]
fn subagent_history() {
    let session = r#"{"type":"session","id":"s1","cwd":"/w"}
{"type":"message","id":"a1","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"c1","name":"subagent","arguments":{"agent":"scout","task":"Find the auth code\nand list it"}},{"type":"toolCall","id":"c2","name":"subagent","arguments":{"tasks":[{"agent":"scout","task":"models"},{"agent":"planner","task":"providers"}]}}]}}
"#;
    let evs = read_all(session, 0);
    let calls: Vec<tool::Call> = messages(&evs)
        .iter()
        .flat_map(|m| m.parts.iter().filter_map(|p| p.call.as_deref().cloned()))
        .collect();
    assert_eq!(calls.len(), 2);
    let c = &calls[0].input;
    assert_eq!(calls[0].kind, tool::Kind::Subagent);
    assert_eq!(
        (c.agent.as_str(), c.prompt.as_str(), c.description.as_str()),
        (
            "scout",
            "Find the auth code\nand list it",
            "Find the auth code and list it"
        )
    );
    let c = &calls[1].input;
    assert_eq!(calls[1].kind, tool::Kind::Subagent);
    assert_eq!(
        (c.agent.as_str(), c.prompt.as_str()),
        ("scout, planner", "scout: models\n\nplanner: providers")
    );
}

// Live, a subagent call's run is a task the turn waits on, from its start
// through its progress to its end; other tools' runs aren't tasks.
#[test]
fn subagent_task() {
    use rush_core::event::{TaskDone, TaskKind, TaskProgress, TaskStarted};
    let sh = Shared::new(Arc::new(Keep::default()));
    let lines = [
        r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{"command":"ls"}}"#,
        r#"{"type":"tool_execution_start","toolCallId":"c2","toolName":"subagent","args":{"agent":"scout","task":"find auth"}}"#,
        r#"{"type":"tool_execution_update","toolCallId":"c2","toolName":"subagent","partialResult":{"content":[{"type":"text","text":"reading"}],"details":{"mode":"single","results":[{"agent":"scout","usage":{"input":100,"output":20},"messages":[{"role":"assistant","content":[{"type":"toolCall","id":"x","name":"read","arguments":{}},{"type":"toolCall","id":"y","name":"grep","arguments":{}}]}]}]}}}"#,
        r#"{"type":"tool_execution_end","toolCallId":"c2","toolName":"subagent","result":{"content":[]},"isError":true}"#,
    ];
    let mut evs = Vec::new();
    for l in lines {
        let head: Value = serde_json::from_str(l).unwrap();
        evs.extend(sh.events(head["type"].as_str().unwrap(), l.as_bytes()));
    }
    let want = vec![
        Event::TaskStarted(TaskStarted {
            id: "c2".into(),
            call_id: "c2".into(),
            kind: TaskKind::Subagent,
            label: "find auth".into(),
            agent: "scout".into(),
            ..TaskStarted::default()
        }),
        Event::TaskProgress(TaskProgress {
            id: "c2".into(),
            summary: "reading".into(),
            last_tool: "grep".into(),
            tokens: 120,
            tool_uses: 2,
        }),
        Event::TaskDone(TaskDone {
            id: "c2".into(),
            call_id: "c2".into(),
            status: "failed".into(),
            ..TaskDone::default()
        }),
    ];
    assert_eq!(evs, want);
}

#[test]
fn registers() {
    let reg = Registry::new();
    reg.register(Pi::new());
    let a = reg.get(crate::KIND).unwrap();
    assert_eq!(a.name(), "Pi");
    assert!(a.driver().is_some() && a.history_reader().is_some() && a.discoverer().is_some());
    assert!(reg.supports(crate::KIND, rush_core::adapter::Feature::Run));
    let _ = discover::home();
}
