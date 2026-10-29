//! A Conn against a fake app-server on the far end of two pipes. Port of
//! codex_test.go (TestHandshake, TestThreadStartParams, TestTurn,
//! TestCommandApproval, TestPermissionsAndQuestion, TestFileChange,
//! TestRateLimitNotification, TestTrustedToolsApproved), compact_test.go,
//! kids_test.go, shells_test.go and modeof_test.go.

use super::*;
use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::sync::mpsc;

use rush_core::adapter::ToolServer;
use rush_core::tool::Patch;

/// What the sink was told.
#[derive(Debug)]
enum Said {
    Event(Event),
    Ended,
}

struct Collect(Mutex<mpsc::Sender<Said>>);

impl Sink for Collect {
    fn event(&self, ev: Event) {
        let _ = self.0.lock().unwrap().send(Said::Event(ev));
    }
    fn ended(&self, _: Option<adapter::Error>) {
        let _ = self.0.lock().unwrap().send(Said::Ended);
    }
}

/// An app-server on the far end of two pipes.
struct Fake {
    r: BufReader<PipeReader>,
    w: PipeWriter,
}

impl Fake {
    /// The next line rush sends, answering the account and rate-limit
    /// reads a new thread makes on its own with an error.
    fn line(&mut self) -> Value {
        loop {
            let mut b = String::new();
            assert!(self.r.read_line(&mut b).unwrap() > 0, "pipe closed");
            let m: Value =
                serde_json::from_str(&b).unwrap_or_else(|e| panic!("bad line {b:?}: {e}"));
            let method = s(&m, "method");
            if method == "account/rateLimits/read" || method == "account/read" {
                let id = m["id"].clone();
                self.write(
                    json!({"id": id, "error": {"code": -32000, "message": "not signed in"}}),
                );
                continue;
            }
            return m;
        }
    }

    /// The next message rush sends, which must be method.
    fn expect(&mut self, method: &str) -> Value {
        let m = self.line();
        assert_eq!(s(&m, "method"), method, "got {m}");
        m
    }

    fn write(&mut self, v: Value) {
        let mut b = serde_json::to_vec(&v).unwrap();
        b.push(b'\n');
        self.w.write_all(&b).unwrap();
    }

    fn respond(&mut self, req: &Value, result: Value) {
        self.write(json!({"id": req["id"], "result": result}));
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write(json!({"method": method, "params": params, "emittedAtMs": 1}));
    }

    fn request(&mut self, id: Value, method: &str, params: Value) {
        self.write(json!({"id": id, "method": method, "params": params}));
    }
}

struct Run {
    c: Arc<Conn>,
    f: Fake,
    rx: mpsc::Receiver<Said>,
}

impl Run {
    /// The next event the Conn sends.
    fn next(&self) -> Event {
        match self.rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Said::Event(e)) => e,
            Ok(Said::Ended) => panic!("ended"),
            Err(_) => panic!("no event"),
        }
    }
}

/// A Conn and a fake, connected, before the handshake.
fn connect() -> (Arc<Conn>, Fake, mpsc::Receiver<Said>) {
    let (cr, fw) = std::io::pipe().unwrap(); // fake → conn
    let (fr, cw) = std::io::pipe().unwrap(); // conn → fake
    let (tx, rx) = mpsc::channel();
    let c = Arc::new(Conn::connect(cr, cw, Arc::new(Collect(Mutex::new(tx)))));
    (
        c,
        Fake {
            r: BufReader::new(fr),
            w: fw,
        },
        rx,
    )
}

fn begin(c: &Arc<Conn>, o: StartOptions) -> std::thread::JoinHandle<adapter::Result<()>> {
    let c = c.clone();
    std::thread::spawn(move || c.begin(&o))
}

/// A Conn run against a fake through the handshake and thread/start; the
/// initialize request it sent.
fn start(o: StartOptions) -> (Run, Value) {
    let (c, mut f, rx) = connect();
    let resume = o.resume;
    let h = begin(&c, o);
    let init = f.expect("initialize");
    f.respond(
        &init,
        json!({"userAgent": "rush/0.155.1 (Mac OS 27.0.0; arm64) test", "codexHome": "/x", "platformFamily": "unix", "platformOs": "macos"}),
    );
    f.expect("initialized");
    let ts = f.expect(if resume {
        "thread/resume"
    } else {
        "thread/start"
    });
    f.notify("thread/started", json!({"thread": {"id": "th1"}}));
    f.respond(
        &ts,
        json!({"thread": {"id": "th1"}, "model": "gpt-5.5", "cwd": "/work", "approvalPolicy": "on-request"}),
    );
    h.join().unwrap().unwrap();
    (Run { c, f, rx }, init)
}

fn spawn_call<T: Send + 'static>(
    c: &Arc<Conn>,
    f: impl FnOnce(&Conn) -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    let c = c.clone();
    std::thread::spawn(move || f(&c))
}

fn message(e: Event) -> Message {
    match e {
        Event::Message(m) => m,
        e => panic!("not a message: {e:?}"),
    }
}

#[test]
fn handshake() {
    let o = StartOptions {
        dir: "/work".into(),
        model: "gpt-5.5".into(),
        mode: "auto".into(),
        ..StartOptions::default()
    };
    let (run, init) = start(o);
    assert_eq!(init["params"]["clientInfo"]["name"], "rush");
    assert_eq!(
        run.next(),
        Event::Init(Box::new(Init {
            session_id: "th1".into(),
            model: "gpt-5.5".into(),
            cwd: "/work".into(),
            mode: "auto".into(),
            version: "0.155.1".into(),
            ..Init::default()
        }))
    );
}

#[test]
fn thread_start_params() {
    let (c, mut f, _rx) = connect();
    let _h = begin(
        &c,
        StartOptions {
            dir: "/w".into(),
            model: "m".into(),
            mode: "full-access".into(),
            session_id: "old".into(),
            resume: true,
            ..StartOptions::default()
        },
    );
    let init = f.expect("initialize");
    f.respond(&init, json!({"userAgent": "rush/1"}));
    f.expect("initialized");
    let p = f.expect("thread/resume")["params"].clone();
    assert_eq!(
        p,
        json!({"threadId": "old", "cwd": "/w", "model": "m", "approvalPolicy": "never", "sandbox": "danger-full-access", "excludeTurns": true})
    );
}

#[test]
fn turn() {
    let (mut run, _) = start(StartOptions {
        effort: "high".into(),
        ..StartOptions::default()
    });
    run.next(); // Init
    run.c.set_model("gpt-5.6").unwrap();
    run.c.set_mode("read-only").unwrap();

    let sent = spawn_call(&run.c, |c| {
        c.send(Input {
            text: "hi".into(),
            images: vec!["/tmp/a.png".into()],
        })
    });
    let ts = run.f.expect("turn/start");
    let p = &ts["params"];
    assert!(
        p["model"] == "gpt-5.6"
            && p["effort"] == "high"
            && p["approvalPolicy"] == "on-request"
            && p["sandboxPolicy"]["type"] == "readOnly",
        "turn/start overrides = {p}"
    );
    let input = p["input"].as_array().unwrap();
    assert!(
        input.len() == 2 && input[0]["text"] == "hi" && input[1]["type"] == "localImage",
        "input = {p}"
    );
    run.f.notify(
        "turn/started",
        json!({"threadId": "th1", "turn": {"id": "tu1", "status": "inProgress"}}),
    );
    run.f.respond(&ts, json!({"turn": {"id": "tu1"}}));
    sent.join().unwrap().unwrap();

    let with = |mut kv: Value| {
        kv["threadId"] = "th1".into();
        kv["turnId"] = "tu1".into();
        kv
    };
    let f = &mut run.f;
    f.notify(
        "item/started",
        with(json!({"item": {"type": "reasoning", "id": "r1", "summary": [], "content": []}})),
    );
    f.notify(
        "item/reasoning/summaryTextDelta",
        with(json!({"itemId": "r1", "delta": "hmm", "summaryIndex": 0})),
    );
    f.notify(
        "item/completed",
        with(json!({"item": {"type": "reasoning", "id": "r1", "summary": ["hmm"], "content": []}})),
    );
    f.notify(
        "item/agentMessage/delta",
        with(json!({"itemId": "m1", "delta": "Hel"})),
    );
    f.notify(
        "item/agentMessage/delta",
        with(json!({"itemId": "m1", "delta": "lo"})),
    );
    f.notify(
        "item/completed",
        with(json!({"item": {"type": "agentMessage", "id": "m1", "text": "Hello"}})),
    );
    f.notify("turn/plan/updated", with(json!({"explanation": null, "plan": [{"step": "Read", "status": "completed"}, {"step": "Write", "status": "inProgress"}]})));
    f.notify("thread/tokenUsage/updated", with(json!({"tokenUsage": {
        "total": {"totalTokens": 1500},
        "last": {"totalTokens": 1500, "inputTokens": 1200, "cachedInputTokens": 1000, "cacheWriteInputTokens": 0, "outputTokens": 300, "reasoningOutputTokens": 100},
        "modelContextWindow": 272000}})));
    f.notify("turn/completed", json!({"threadId": "th1", "turn": {"id": "tu1", "status": "completed", "error": null, "durationMs": 2500}}));

    let msg = |id: &str, kind, text: &str| {
        Event::Message(Message {
            role: "assistant".into(),
            id: id.into(),
            model: "gpt-5.6".into(),
            parts: vec![text_part(kind, text.into())],
            ..Message::default()
        })
    };
    let start = |id: &str| {
        Event::MessageStart(MessageStart {
            id: id.into(),
            model: "gpt-5.6".into(),
        })
    };
    let delta = |kind, text: &str| {
        Event::Delta(Delta {
            index: 0,
            kind,
            text: text.into(),
        })
    };
    let todo = |label: &str, status: &str| TodoItem {
        label: label.into(),
        status: status.into(),
        ..TodoItem::default()
    };
    let want = vec![
        Event::Status(Status {
            busy: true,
            text: String::new(),
        }),
        start("r1"),
        Event::PartStart(PartStart {
            index: 0,
            kind: PartKind::Thinking,
        }),
        delta(PartKind::Thinking, "hmm"),
        msg("r1", PartKind::Thinking, "hmm"),
        start("m1"),
        Event::PartStart(PartStart {
            index: 0,
            kind: PartKind::Text,
        }),
        delta(PartKind::Text, "Hel"),
        delta(PartKind::Text, "lo"),
        msg("m1", PartKind::Text, "Hello"),
        Event::Plan(Plan {
            todos: vec![todo("Read", "completed"), todo("Write", "in_progress")],
        }),
        Event::Context(Context {
            tokens: 1500,
            window: 272000,
        }),
        Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            tokens: TokenUsage {
                input: 200,
                output: 300,
                cache_read: 1000,
                reasoning: 100,
                ..TokenUsage::default()
            },
            duration: Duration::from_millis(2500),
            turns: 1,
            ..TurnEnd::default()
        }),
    ];
    for (i, w) in want.into_iter().enumerate() {
        assert_eq!(run.next(), w, "event {i}");
    }

    // Overrides stick in Codex, so the next turn sends none.
    let sent = spawn_call(&run.c, |c| {
        c.send(Input {
            text: "again".into(),
            ..Input::default()
        })
    });
    let ts = run.f.expect("turn/start");
    assert!(
        ts["params"].get("model").is_none() && ts["params"].get("approvalPolicy").is_none(),
        "second turn/start resent overrides: {ts}"
    );
    run.f.respond(&ts, json!({"turn": {"id": "tu2"}}));
    sent.join().unwrap().unwrap();

    // A message while a turn runs steers it.
    let sent = spawn_call(&run.c, |c| {
        c.send(Input {
            text: "also".into(),
            ..Input::default()
        })
    });
    let st = run.f.expect("turn/steer");
    assert_eq!(st["params"]["expectedTurnId"], "tu2");
    run.f.respond(&st, json!({"turnId": "tu2"}));
    sent.join().unwrap().unwrap();

    let sent = spawn_call(&run.c, |c| c.interrupt());
    let ti = run.f.expect("turn/interrupt");
    assert!(
        ti["params"]["turnId"] == "tu2" && ti["params"]["threadId"] == "th1",
        "{ti}"
    );
    run.f.respond(&ti, json!({}));
    sent.join().unwrap().unwrap();
}

#[test]
fn command_approval() {
    let (mut run, _) = start(StartOptions::default());
    run.next();
    let mut item = json!({"type": "commandExecution", "id": "c1", "command": "go test ./...", "cwd": "/work", "status": "inProgress",
        "commandActions": [{"type": "unknown", "command": "go test ./..."}], "aggregatedOutput": null, "exitCode": null});
    run.f.notify(
        "item/started",
        json!({"threadId": "th1", "turnId": "tu1", "item": item}),
    );
    let m = message(run.next());
    let call = m.parts[0].call.as_ref().unwrap();
    assert!(
        call.kind == Kind::Shell
            && call.input.command == "go test ./..."
            && call.input.cwd == "/work",
        "{call:?}"
    );

    run.f.request(
        "req-7".into(),
        REQ_COMMAND,
        json!({"kind": "command", "threadId": "th1", "turnId": "tu1", "itemId": "c1", "startedAtMs": 1,
            "reason": "needs network", "command": "go test ./...", "cwd": "/work"}),
    );
    let Event::Approval(a) = run.next() else {
        panic!("not an approval")
    };
    assert!(
        a.id == "req-7"
            && a.reason == "needs network"
            && a.call.input.command == "go test ./..."
            && a.call.id == "c1",
        "{a:?}"
    );
    let kinds: Vec<OptionKind> = a.options.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [
            OptionKind::AllowOnce,
            OptionKind::AllowAlways,
            OptionKind::RejectOnce,
            OptionKind::RejectAlways
        ]
    );
    assert!(
        run.c.answer("req-7", "nonsense").is_err(),
        "took an option it doesn't have"
    );
    let opt = a.options[1].id.clone();
    let h = spawn_call(&run.c, move |c| c.answer("req-7", &opt));
    let reply = run.f.line();
    assert!(
        reply["id"] == "req-7" && reply["result"]["decision"] == "acceptForSession",
        "{reply}"
    );
    h.join().unwrap().unwrap();
    assert!(run.c.answer("req-7", "accept").is_err(), "answered twice");

    // A request resolved without an answer is withdrawn.
    run.f.request(
        8.into(),
        REQ_FILE_CHANGE,
        json!({"threadId": "th1", "turnId": "tu1", "itemId": "f1", "startedAtMs": 1}),
    );
    let Event::Approval(a) = run.next() else {
        panic!("not an approval")
    };
    assert!(a.id == "8" && a.call.kind == Kind::Edit, "{a:?}");
    run.f.notify(
        "serverRequest/resolved",
        json!({"threadId": "th1", "requestId": 8}),
    );
    assert_eq!(
        run.next(),
        Event::ApprovalCancelled(ApprovalCancelled { id: "8".into() })
    );

    item["status"] = "completed".into();
    item["aggregatedOutput"] = "FAIL\n".into();
    item["exitCode"] = 1.into();
    run.f.notify(
        "item/completed",
        json!({"threadId": "th1", "turnId": "tu1", "item": item}),
    );
    let out = message(run.next());
    let o = out.parts[0].output.as_ref().unwrap();
    assert!(
        out.role == "user"
            && o.call_id == "c1"
            && o.stdout == "FAIL\n"
            && o.exit == Some(1)
            && o.is_error,
        "{o:?}"
    );
}

#[test]
fn permissions_and_question() {
    let (mut run, _) = start(StartOptions::default());
    run.next();
    run.f.request(
        1.into(),
        REQ_PERMISSIONS,
        json!({"threadId": "th1", "turnId": "tu1", "itemId": "p1", "environmentId": null, "startedAtMs": 1,
            "cwd": "/work", "reason": "write outside", "permissions": {"network": null, "fileSystem": {"read": null, "write": ["/etc/hosts"]}}}),
    );
    let Event::Approval(a) = run.next() else {
        panic!("not an approval")
    };
    assert!(a.path == "/etc/hosts" && a.options.len() == 3, "{a:?}");
    let h = spawn_call(&run.c, |c| c.answer("1", "session"));
    let b = run.f.line();
    let perms = b["result"]["permissions"].as_object().unwrap();
    assert!(
        b["result"]["scope"] == "session" && perms.contains_key("fileSystem") && perms.len() == 1,
        "{b}"
    );
    h.join().unwrap().unwrap();

    run.f.request(
        2.into(),
        REQ_USER_INPUT,
        json!({"threadId": "th1", "turnId": "tu1", "itemId": "q1", "isBlocking": true, "autoResolutionMs": null,
            "questions": [{"id": "color", "header": "Colour", "question": "Which colour?", "isOther": false, "isSecret": false,
                "options": [{"label": "Red", "description": "warm"}, {"label": "Blue", "description": "cool"}]}]}),
    );
    let choice = |l: &str, d: &str| Choice {
        label: l.into(),
        description: d.into(),
        ..Choice::default()
    };
    assert_eq!(
        run.next(),
        Event::Question(Question {
            id: "2".into(),
            call_id: "q1".into(),
            asks: vec![Ask {
                id: "color".into(),
                header: "Colour".into(),
                text: "Which colour?".into(),
                options: vec![choice("Red", "warm"), choice("Blue", "cool")],
                ..Ask::default()
            }],
            ..Question::default()
        })
    );
    assert!(
        run.c.answer("2", "Red").is_err(),
        "Answer answered a question"
    );
    let h = spawn_call(&run.c, |c| {
        let answers: HashMap<String, Vec<String>> =
            [("Which colour?".to_string(), vec!["Blue".to_string()])].into();
        c.answerer().unwrap().answer_question("2", &answers)
    });
    let b = run.f.line();
    assert_eq!(b["result"]["answers"]["color"]["answers"], json!(["Blue"]));
    h.join().unwrap().unwrap();

    // What rush can't answer is refused rather than left hanging.
    run.f.request(3.into(), "item/tool/call", json!({}));
    let b = run.f.line();
    assert!(b["id"] == 3 && b.get("error").is_some(), "{b}");
    assert_eq!(
        run.next(),
        Event::Other(Other {
            adapter: "codex".into(),
            kind: "item/tool/call".into(),
            raw: rush_core::wire::RawJson::new("{}").ok(),
        })
    );
}

#[test]
fn file_change() {
    let (mut run, _) = start(StartOptions::default());
    run.next();
    let mut item = json!({"type": "fileChange", "id": "e1", "status": "inProgress", "changes": [
        {"path": "/work/new.txt", "kind": {"type": "add"}, "diff": "one\ntwo\n"},
        {"path": "/work/main.go", "kind": {"type": "update", "move_path": null},
            "diff": "@@ -1,3 +1,3 @@\n package main\n-var x = 1\n+var x = 2\n \n"},
    ]});
    run.f.notify(
        "item/started",
        json!({"threadId": "th1", "turnId": "tu1", "item": item}),
    );
    let m = message(run.next());
    let call = m.parts[0].call.as_ref().unwrap();
    assert!(
        call.kind == Kind::Edit && call.input.path == "/work/new.txt",
        "{call:?}"
    );
    item["status"] = "completed".into();
    run.f.notify(
        "item/completed",
        json!({"threadId": "th1", "turnId": "tu1", "item": item}),
    );
    let m = message(run.next());
    let o = m.parts[0].output.as_ref().unwrap();
    let lines = |ls: &[&str]| ls.iter().map(|s| s.to_string()).collect();
    let want = vec![
        Patch {
            path: "/work/new.txt".into(),
            new_start: 1,
            new_lines: 2,
            lines: lines(&["+one", "+two"]),
            ..Patch::default()
        },
        Patch {
            path: "/work/main.go".into(),
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 3,
            lines: lines(&[" package main", "-var x = 1", "+var x = 2", " "]),
        },
    ];
    assert!(o.created && !o.is_error, "{o:?}");
    assert_eq!(o.patches, want);
}

#[test]
fn rate_limit_notification() {
    let (mut run, _) = start(StartOptions::default());
    run.next();
    run.f.notify(
        "account/rateLimits/updated",
        json!({"rateLimits": {"limitId": "codex", "primary": null,
            "secondary": {"usedPercent": 70, "windowDurationMins": 10080, "resetsAt": null}}}),
    );
    let Event::Quota(q) = run.next() else {
        panic!("not a quota")
    };
    assert!(
        q.source == rush_core::usage::Source::Live
            && q.windows.len() == 1
            && q.windows[0].id == "secondary"
            && q.windows[0].label == "7d",
        "{q:?}"
    );
}

#[test]
fn trusted_tools_approved() {
    let (c, mut f, _rx) = connect();
    let _h = begin(
        &c,
        StartOptions {
            tools: vec![ToolServer {
                name: TOOLS_SERVER.into(),
                trusted: vec!["spawn_agent".into()],
                args: vec!["mcp-tools".into()],
                ..ToolServer::default()
            }],
            ..StartOptions::default()
        },
    );
    let init = f.expect("initialize");
    f.respond(&init, json!({"userAgent": "rush/1"}));
    f.expect("initialized");
    let ts = f.expect("thread/start");
    let srv = &ts["params"]["config"]["mcp_servers.rush"];
    assert_eq!(
        srv["tools"],
        json!({"spawn_agent": {"approval_mode": "approve"}})
    );
    assert_eq!(srv["args"], json!(["mcp-tools"]));
    assert!(
        srv["env_vars"]
            .as_array()
            .is_some_and(|v| v.iter().any(|n| n == "PATH"))
    );
}

// compact_test.go:TestNativeCompactionLifecycle
#[test]
fn native_compaction_lifecycle() {
    let (mut run, _) = start(StartOptions::default());
    run.next();
    let sent = spawn_call(&run.c, |c| {
        c.send(Input {
            text: " /compact ".into(),
            ..Input::default()
        })
    });
    let req = run.f.expect("thread/compact/start");
    assert_eq!(req["params"], json!({"threadId": "th1"}));
    run.f.respond(&req, json!({}));
    sent.join().unwrap().unwrap();
    run.f.notify(
        "turn/started",
        json!({"threadId": "th1", "turn": {"id": "compact-turn"}}),
    );
    assert!(matches!(
        run.next(),
        Event::Status(Status { busy: true, .. })
    ));
    assert!(
        run.c
            .send(Input {
                text: "/compact".into(),
                ..Input::default()
            })
            .is_err(),
        "accepted concurrent compaction"
    );
    run.f.notify(
        "item/started",
        json!({"threadId": "th1", "item": {"id": "compact-item", "type": "contextCompaction"}}),
    );
    assert!(matches!(run.next(), Event::Status(Status { text, .. }) if text == "compacting"));
    run.f.notify(
        "item/completed",
        json!({"threadId": "th1", "item": {"id": "compact-item", "type": "contextCompaction"}}),
    );
    assert!(matches!(run.next(), Event::Compacted(_)));
    run.f.notify(
        "turn/completed",
        json!({"threadId": "th1", "turn": {"id": "compact-turn", "status": "completed"}}),
    );
    assert!(matches!(run.next(), Event::TurnEnd(e) if e.reason == "done"));
    assert!(run.c.inner.st().turn.is_empty(), "still busy");
}

// compact_test.go:TestNativeCompactionRejectionLeavesThreadUsable
#[test]
fn native_compaction_rejection_leaves_thread_usable() {
    let (mut run, _) = start(StartOptions::default());
    run.next();
    assert!(
        run.c
            .send(Input {
                text: "/compact".into(),
                images: vec!["photo.png".into()],
            })
            .is_err(),
        "silently dropped attachment"
    );
    let sent = spawn_call(&run.c, |c| {
        c.send(Input {
            text: "/compact".into(),
            ..Input::default()
        })
    });
    let req = run.f.expect("thread/compact/start");
    run.f.write(
        json!({"id": req["id"], "error": {"code": -32601, "message": "method unavailable"}}),
    );
    let e = sent.join().unwrap().unwrap_err();
    assert!(e.to_string().contains("method unavailable"), "{e}");
    let sent = spawn_call(&run.c, |c| {
        c.send(Input {
            text: "continue".into(),
            ..Input::default()
        })
    });
    let req = run.f.expect("turn/start");
    run.f.respond(&req, json!({"turn": {"id": "next"}}));
    sent.join().unwrap().unwrap();
}

#[test]
fn ended_once_the_app_server_goes() {
    let (run, _) = start(StartOptions::default());
    run.next();
    let Run { c, f, rx } = run;
    drop(f);
    loop {
        match rx.recv_timeout(Duration::from_secs(5)).expect("ended") {
            Said::Ended => break,
            Said::Event(_) => {}
        }
    }
    let e = c
        .send(Input {
            text: "hi".into(),
            ..Input::default()
        })
        .unwrap_err();
    assert!(e.downcast_ref::<crate::rpc::Closed>().is_some(), "{e}");
}

fn note(st: &mut State, method: &str, params: Value) -> Vec<Event> {
    st.notification(method, &params.to_string())
}

fn root() -> State {
    State {
        thread: "root".into(),
        ..State::default()
    }
}

// kids_test.go:TestSpawnedThreadIsATask: a spawned thread is a task from
// its spawn until its turn ends, and again while a message it was given
// runs; the agent tools around it draw as Claude Code's of the same use.
#[test]
fn spawned_thread_is_a_task() {
    let mut st = root();
    let item = |kind: &str| {
        json!({"threadId": "root", "item": {"type": "subAgentActivity", "id": format!("call_{kind}"),
            "kind": kind, "agentThreadId": "kid", "agentPath": "/root/echoer"}})
    };
    let turn =
        |id: &str, status: &str| json!({"threadId": "kid", "turn": {"id": id, "status": status}});
    let (mut started, mut done, mut calls) = (Vec::new(), Vec::new(), Vec::new());
    let mut take = |evs: Vec<Event>| {
        for ev in evs {
            match ev {
                Event::TaskStarted(e) => started.push(format!("{}/{}", e.id, e.call_id)),
                Event::TaskDone(e) => done.push(format!("{}/{}", e.id, e.status)),
                Event::Message(m) => calls.extend(m.parts.into_iter().filter_map(|p| p.call)),
                _ => {}
            }
        }
    };
    take(note(&mut st, "item/started", item("started")));
    take(note(&mut st, "turn/started", turn("t1", "inProgress")));
    take(note(&mut st, "turn/completed", turn("t1", "completed")));
    take(note(&mut st, "item/started", item("interacted")));
    take(note(&mut st, "turn/started", turn("t2", "inProgress")));
    assert_eq!(
        st.child("call_started"),
        Some(("kid".into(), "t2".into())),
        "child by its call"
    );
    take(note(&mut st, "item/started", item("interrupted")));
    take(note(&mut st, "turn/completed", turn("t2", "interrupted")));
    take(note(&mut st, "item/started", item("completed")));

    assert_eq!(started, ["kid/call_started", "kid/call_started"]);
    assert_eq!(done, ["kid/completed", "kid/stopped"]);
    assert_eq!(calls.len(), 3);
    assert!(
        calls[0].kind == Kind::Subagent
            && calls[0].input.child == "kid"
            && calls[0].input.description == "echoer"
    );
    assert_eq!(calls[1].name, "SendMessage");
    assert_eq!(calls[1].raw.as_ref().unwrap().get(), r#"{"to":"echoer"}"#);
    assert_eq!(calls[2].name, "TaskStop");
    assert!(st.background().tasks.is_empty(), "still running");
}

// kids_test.go:TestSpawnedThreadReportsProgress
#[test]
fn spawned_thread_reports_progress() {
    let mut st = root();
    st.adopt("kid", "spawn", "worker");
    let mut evs = note(
        &mut st,
        "thread/tokenUsage/updated",
        json!({"threadId": "kid", "tokenUsage": {"last": {"inputTokens": 1200, "cachedInputTokens": 1000}}}),
    );
    evs.extend(note(
        &mut st,
        "item/started",
        json!({"threadId": "kid", "item": {"type": "commandExecution", "id": "tool-1", "command": "rg TODO"}}),
    ));
    let progress: Vec<TaskProgress> = evs
        .into_iter()
        .filter_map(|e| match e {
            Event::TaskProgress(p) => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(progress.len(), 2, "{progress:?}");
    assert_eq!(progress[0].tokens, 1200);
    assert!(progress[1].tool_uses == 1 && !progress[1].last_tool.is_empty());
}

// shells_test.go:TestBackgroundShell: a command still running when the
// model reads on is a background shell until its item completes, listed
// beside the spawned threads.
#[test]
fn background_shell() {
    let mut st = root();
    let cmd = |status: &str, exit: Value| {
        json!({"type": "commandExecution", "id": "exec-1", "command": "/bin/zsh -lc 'sleep 25; echo done'",
            "cwd": "/w", "processId": "78732", "source": "unifiedExecStartup", "status": status, "exitCode": exit, "commandActions": []})
    };
    let (mut log, mut lists) = (Vec::new(), Vec::new());
    let mut take = |evs: Vec<Event>| {
        for ev in evs {
            match ev {
                Event::CallUpdated(e)
                    if e.call.input.background && e.call.input.command == "sleep 25; echo done" =>
                {
                    log.push(format!("backgrounded {}", e.call.id))
                }
                Event::TaskStarted(e) if e.kind == TaskKind::Shell && e.background => {
                    log.push(format!("started {}/{}", e.id, e.call_id))
                }
                Event::TaskDone(e) => log.push(format!("done {}/{}", e.id, e.status)),
                Event::Background(b) => {
                    let mut ids: Vec<String> = b.tasks.into_iter().map(|t| t.id).collect();
                    ids.sort();
                    lists.push(ids.join(","));
                }
                _ => {}
            }
        }
    };
    let n = |st: &mut State, method: &str, mut p: Value| {
        p["threadId"] = "root".into();
        note(st, method, p)
    };
    take(n(
        &mut st,
        "turn/started",
        json!({"turn": {"id": "t1", "status": "inProgress"}}),
    ));
    take(n(
        &mut st,
        "item/started",
        json!({"item": {"type": "subAgentActivity", "id": "call_k",
        "kind": "started", "agentThreadId": "kid", "agentPath": "/root/echoer"}}),
    ));
    take(n(
        &mut st,
        "item/started",
        json!({"item": cmd("inProgress", Value::Null)}),
    ));
    take(n(
        &mut st,
        "item/started",
        json!({"item": {"type": "agentMessage", "id": "m1"}}),
    ));
    take(n(
        &mut st,
        "turn/completed",
        json!({"turn": {"id": "t1", "status": "completed"}}),
    ));
    take(n(
        &mut st,
        "item/completed",
        json!({"item": cmd("completed", 0.into())}),
    ));

    assert_eq!(
        log,
        [
            "backgrounded exec-1",
            "started exec-1/exec-1",
            "done exec-1/completed"
        ]
    );
    // The kid, then the kid and the shell, then the kid alone.
    assert_eq!(lists, ["kid", "exec-1,kid", "kid"]);
    // Stopping it names its terminal, and its end then says stopped.
    st.shells.insert(
        "exec-2".into(),
        Shell {
            process: "99".into(),
            bg: true,
            stopping: false,
        },
    );
    assert_eq!(st.shell_of("99"), Some(("root".into(), "99".into())));
    let done = st.finished(&ThreadItem {
        id: "exec-2".into(),
        ..ThreadItem::default()
    });
    assert!(matches!(&done[0], Event::TaskDone(d) if d.status == "stopped"));
}

// modeof_test.go:TestModeOfPolicy
#[test]
fn mode_of_policy() {
    for m in &MODES {
        assert_eq!(mode_of(m.approval, s(&(m.turn)(), "type")), m.id);
    }
    assert_eq!(mode_of("on-request", "somethingNew"), "");
}

#[test]
fn carried_and_env_names() {
    let lines = [
        PreviewLine {
            role: "user".into(),
            text: "hi".into(),
            ..PreviewLine::default()
        },
        PreviewLine {
            role: "assistant".into(),
            text: "yo".into(),
            ..PreviewLine::default()
        },
    ];
    let c = carried(&lines);
    assert_eq!(c[0]["content"][0]["type"], "input_text");
    assert_eq!(c[1]["content"][0]["type"], "output_text");
    assert_eq!(
        env_names(&["A=1".into(), "B=2".into(), "A=3".into(), "=x".into()]),
        ["A", "B"]
    );
}
