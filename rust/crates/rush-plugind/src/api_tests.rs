//! What a plugin may ask of rush, and what it's refused.
//! Ported from internal/plugind/api_test.go, sidebar_test.go,
//! queued_test.go and bundled_test.go:107-112.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

use rush_plugin::rpc::{CODE_DENIED, CODE_INVALID_PARAMS};
use rush_plugin::{CAP_QUEUE, CAP_QUEUED, CAP_START, Ctx, Manifest};
use serde_json::{Value, json};

use crate::host::Info;
use crate::raw;
use crate::sessions::{StartReq, WorktreeReq};
use crate::testutil::{Home, denied, test_runner, write_info};

fn out(v: Option<rush_plugin::Raw>) -> Value {
    serde_json::from_str(v.unwrap().get()).unwrap()
}

#[test]
fn exec_runs_what_the_manifest_names() {
    let home = Home::new();
    let ws = home.mkdir("ws");
    let m = Manifest {
        name: "kanban".into(),
        workspaces: vec![ws.clone()],
        exec: BTreeMap::from([(
            "echo".to_string(),
            vec![
                "/bin/sh".into(),
                "-c".into(),
                r#"echo "$@"; cat; pwd; exit 3"#.into(),
                "sh".into(),
            ],
        )]),
        ..Default::default()
    };
    let (r, p) = test_runner(&home, m);
    let ctx = Ctx::background();
    let args = vec!["list".to_string(), "--json".to_string()];
    let res = out(r.exec(&ctx, &p, "echo", &args, "in", &ws).unwrap());
    assert_eq!(res["code"], 3, "{res}");
    assert_eq!(res["stdout"], format!("list --json\nin{ws}\n"));
    assert_eq!(res["truncated"], false);
    let e = r.exec(&ctx, &p, "rm", &[], "", "").unwrap_err();
    assert!(denied(&e), "a program the manifest doesn't name ran: {e:?}");
    let e = r.exec(&ctx, &p, "echo", &[], "", "/").unwrap_err();
    assert!(denied(&e), "ran outside its workspaces: {e:?}");
    let res = out(r.exec(&ctx, &p, "echo", &[], "", "").unwrap());
    let data = rush_plugin::data_dir(&p.name).canonicalize().unwrap();
    assert!(
        res["stdout"]
            .as_str()
            .unwrap()
            .contains(&*data.to_string_lossy()),
        "without a cwd it should run in its data folder: {res}"
    );
    let e = r
        .exec(&ctx, &p, "echo", &["a\0b".to_string()], "", "")
        .unwrap_err();
    assert_eq!(e.code, CODE_INVALID_PARAMS);
}

#[test]
fn queueable_only_in_its_workspaces_and_safe_modes() {
    let home = Home::new();
    let ws = home.mkdir("ws");
    let elsewhere = home.mkdir("elsewhere");
    let m = Manifest {
        name: "kanban".into(),
        workspaces: vec![ws.clone()],
        sessions: vec![CAP_QUEUE.into()],
        ..Default::default()
    };
    let (r, p) = test_runner(&home, m);
    let info = |id: &str, cwd: &str, by: &str, mode: &str| Info {
        id: id.into(),
        cwd: cwd.into(),
        started_by: by.into(),
        permission_mode: mode.into(),
        ..Default::default()
    };
    write_info(&info("mine", &elsewhere, "kanban", ""));
    write_info(&info("inside", &ws, "", "acceptEdits"));
    write_info(&info("outside", &elsewhere, "", ""));
    write_info(&info("yolo", &ws, "", "bypassPermissions"));
    for (id, ok) in [
        ("mine", true),
        ("inside", true),
        ("outside", false),
        ("yolo", false),
    ] {
        let got = r.queueable(&p, id);
        assert_eq!(got.is_ok(), ok, "queueable({id}) = {got:?}");
        if let Err(e) = got {
            assert!(denied(&e), "queueable({id}) = {e:?}");
        }
    }
    assert!(
        r.queueable(&p, "../x").is_err(),
        "a path was taken for a session id"
    );
}

#[test]
fn start_refuses() {
    let home = Home::new();
    let ws = home.mkdir("ws");
    let m = Manifest {
        name: "kanban".into(),
        workspaces: vec![ws.clone()],
        sessions: vec![CAP_START.into()],
        ..Default::default()
    };
    let (r, p) = test_runner(&home, m);
    let wt = |name: &str, branch: &str, base: &str| {
        Some(WorktreeReq {
            name: name.into(),
            branch: branch.into(),
            base: base.into(),
        })
    };
    let cases: Vec<(&str, StartReq)> = vec![
        (
            "meta key",
            StartReq {
                meta: BTreeMap::from([("bad key".into(), "x".into())]),
                ..Default::default()
            },
        ),
        (
            "meta value",
            StartReq {
                meta: BTreeMap::from([("card".into(), "x".repeat(2000))]),
                ..Default::default()
            },
        ),
        (
            "branch option",
            StartReq {
                worktree: wt("", "--upload-pack=evil", ""),
                ..Default::default()
            },
        ),
        (
            "base option",
            StartReq {
                worktree: wt("", "", "-b"),
                ..Default::default()
            },
        ),
        (
            "branch dots",
            StartReq {
                worktree: wt("", "a/../b", ""),
                ..Default::default()
            },
        ),
        (
            "not a repo",
            StartReq {
                worktree: wt("", "task", ""),
                ..Default::default()
            },
        ),
        (
            "unsafe mode",
            StartReq {
                mode: "bypassPermissions".into(),
                ..Default::default()
            },
        ),
        (
            "bad effort",
            StartReq {
                effort: "huge".into(),
                ..Default::default()
            },
        ),
    ];
    for (name, mut q) in cases {
        q.cwd = ws.clone();
        q.prompt = "hi".into();
        assert!(r.start(&p, q).is_err(), "{name}: started");
    }
    let e = r
        .start(
            &p,
            StartReq {
                cwd: "/tmp".into(),
                prompt: "hi".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(
        e.message.contains("outside the plugin's workspaces"),
        "{e:?}"
    );
}

#[test]
fn names_and_words() {
    use crate::sessions::{first_words, id_ok, worktree_name};
    assert_eq!(worktree_name("Fix the Login bug!"), "fix-the-login-bug");
    assert_eq!(worktree_name("--"), "session");
    assert_eq!(worktree_name(&"ab-".repeat(20)).len(), 40);
    assert_eq!(
        first_words("  one two\nthree four five six seven", 6),
        "one two three four five six"
    );
    assert_eq!(first_words(" ", 6), "a new session");
    assert!(id_ok("a1_b-c") && !id_ok("../x") && !id_ok("-a") && !id_ok(""));
}

#[test]
fn services_ask_needs_uses() {
    let home = Home::new();
    let (r, _) = test_runner(&home, crate::testutil::manifest("asker"));
    let e = r
        .answer_plugin(
            &Ctx::background(),
            "services.ask",
            Some(&raw(r#"{"service":"cleanup.report"}"#)),
        )
        .unwrap_err();
    assert_eq!(e.code, CODE_DENIED, "{e:?}");
}

#[test]
fn sidebar_set() {
    let home = Home::new();
    let (r, mut p) = test_runner(&home, crate::testutil::manifest("kanban"));
    let ctx = Ctx::background();
    let set = |r: &std::sync::Arc<crate::runner::Runner>, params: &str| {
        r.answer_plugin(&ctx, "sidebar.set", Some(&raw(params)))
    };
    let board = r#"{"title": "Kanban", "sections": [{"title": "In Progress"}],
        "agents": {"8c76706f-1c00-4aed-9c6d-7509f3033943": {"name": "Fix \u001b[1mlogin", "section": "In Progress"}}}"#;
    let path = rush_plugin::sidebar::sidebar_path("kanban");
    let e = set(&r, board).unwrap_err();
    assert!(denied(&e), "set without the capability: {e:?}");
    assert!(!path.exists(), "a refused sidebar was written");

    p.manifest.sidebar = true;
    r.set_plugin(p);
    let e = set(
        &r,
        r#"{"sections": [{"title": "A"}], "agents": {"s1": {"section": "B"}}}"#,
    )
    .unwrap_err();
    assert_eq!(e.code, CODE_INVALID_PARAMS, "an agent in no section: {e:?}");
    set(&r, board).unwrap();
    let got: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(got["plugin"], "kanban");
    assert!(
        got["updatedAt"]
            .as_str()
            .is_some_and(|t| !t.starts_with("0001"))
    );
    assert_eq!(
        got["agents"]["8c76706f-1c00-4aed-9c6d-7509f3033943"]["name"],
        "Fix login"
    );

    set(&r, "{}").unwrap();
    assert!(!path.exists(), "clearing left the file");

    // Revoked, the broker's reload removes what it had set.
    set(&r, board).unwrap();
    let (b, _) = crate::testutil::test_broker(&home);
    b.reload();
    assert!(
        !path.exists(),
        "a plugin no longer approved kept its sidebar"
    );
    assert!(b.done().err().is_some(), "with nothing to run, it stops");
}

/// A session's host holding a queue, for its queue ops: it sends its info
/// on connect and after each op, as a host does.
fn fake_queue_host(mut info: Info) -> mpsc::Receiver<String> {
    write_info(&info);
    let ln = UnixListener::bind(crate::host::sock_path(&info.id)).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for c in ln.incoming() {
            let Ok(c) = c else { return };
            let mut w = c.try_clone().unwrap();
            let say = |w: &mut std::os::unix::net::UnixStream, i: &Info| {
                let b = serde_json::to_vec(&json!({"type": "agtop_info", "info": i})).unwrap();
                let _ = w.write_all(&[b, b"\n".to_vec()].concat());
            };
            say(&mut w, &info);
            for l in BufReader::new(c).lines() {
                let Ok(l) = l else { break };
                let o: Value = serde_json::from_str(&l).unwrap();
                let was = o["was"].as_str().unwrap_or_default();
                let i = info.queue.iter().position(|q| q == was);
                match (o["op"].as_str().unwrap_or_default(), i) {
                    ("hello", _) => continue,
                    (_, None) => {
                        let b = br#"{"error":"that message has already been sent","type":"agtop_error"}"#;
                        let _ = w.write_all(&[&b[..], b"\n"].concat());
                        continue;
                    }
                    ("queue_send", Some(i)) => {
                        let _ = tx.send(info.queue[i].clone());
                    }
                    _ => {}
                }
                info.queue.remove(i.unwrap());
                say(&mut w, &info);
            }
        }
    });
    rx
}

/// With queued, a plugin sends or drops a queued message by its place, in
/// its workspaces only, and never sees the text.
#[test]
fn queued_send_and_remove() {
    let home = Home::new();
    let ws = home.mkdir("ws");
    let m = Manifest {
        name: "queue".into(),
        sessions: vec![CAP_QUEUED.into()],
        workspaces: vec![ws.clone()],
        ..Default::default()
    };
    let (r, mut p) = test_runner(&home, m);
    let sent = fake_queue_host(Info {
        id: "q1".into(),
        cwd: ws.clone(),
        state: "working".into(),
        queue: vec!["first".into(), "second".into(), "third".into()],
        ..Default::default()
    });
    write_info(&Info {
        id: "out1".into(),
        cwd: home.path().to_string_lossy().into_owned(),
        state: "working".into(),
        queue: vec!["x".into()],
        ..Default::default()
    });
    let ctx = Ctx::background();
    let call = |method: &str, params: &str| r.answer_plugin(&ctx, method, Some(&raw(params)));

    let res = call("sessions.queued.send", r#"{"id": "q1", "index": 1}"#).unwrap();
    assert_eq!(sent.recv().unwrap(), "second");
    assert_eq!(res.unwrap().get(), "{}", "the plugin was told more");
    // Naming one by its text would let a plugin test guesses at the queue:
    // only rush's own plugins may, and then was names the one meant even
    // if the queue moved.
    let e = call(
        "sessions.queued.remove",
        r#"{"id": "q1", "index": 0, "was": "third"}"#,
    )
    .unwrap_err();
    assert!(
        denied(&e),
        "an installed plugin named a message by its text: {e:?}"
    );
    p.bundled = true;
    r.set_plugin(p.clone());
    call(
        "sessions.queued.remove",
        r#"{"id": "q1", "index": 0, "was": "third"}"#,
    )
    .unwrap();
    p.bundled = false;
    r.set_plugin(p.clone());
    assert!(
        call("sessions.queued.remove", r#"{"id": "q1", "index": 5}"#).is_err(),
        "no message 5, it should say so"
    );
    let e = call("sessions.queued.send", r#"{"id": "out1", "index": 0}"#).unwrap_err();
    assert!(denied(&e), "outside its workspaces: {e:?}");
    p.manifest.sessions = vec![];
    r.set_plugin(p);
    let e = call("sessions.queued.send", r#"{"id": "q1", "index": 0}"#).unwrap_err();
    assert!(denied(&e), "without the capability: {e:?}");
}

/// What a plugin hears of a session it follows: news only, unless replay,
/// and never a word at a time.
#[test]
fn follow_tells_what_happens() {
    use crate::host::{FileHosts, HostEvent};
    use rush_core::event::{Delta, Event, Message, Part, PartKind, TurnEnd};
    use rush_core::tool::{Call, Input, Kind};
    let info = |state: &str| {
        HostEvent::Info(Box::new(Info {
            state: state.into(),
            ..Default::default()
        }))
    };
    let said = HostEvent::Event(Box::new(Event::Message(Message {
        role: "assistant".into(),
        parts: vec![
            Part {
                kind: PartKind::Text,
                text: "On it.".into(),
                ..Default::default()
            },
            Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(Call {
                    name: "Read".into(),
                    kind: Kind::Read,
                    input: Input {
                        path: "/a/view.go".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })),
                ..Default::default()
            },
        ],
        ..Default::default()
    })));
    let delta = HostEvent::Event(Box::new(Event::Delta(Delta {
        kind: PartKind::Text,
        text: "On".into(),
        ..Default::default()
    })));
    let end = HostEvent::Event(Box::new(Event::TurnEnd(TurnEnd {
        reason: "done".into(),
        text: "Done.".into(),
        turns: 2,
        ..Default::default()
    })));
    let lines = vec![
        vec![HostEvent::Sent("old".into()), end.clone()],
        vec![HostEvent::Sent("go".into()), delta.clone(), said.clone()],
        vec![info("working")],
        vec![said, end],
    ];
    let mut got = Vec::new();
    crate::follow(lines.clone(), false, &FileHosts, |ev| {
        got.push(Value::Object(ev))
    });
    // A host that's gone reads as stopped.
    assert_eq!(
        got,
        vec![
            json!({"type": "info", "state": "stopped", "detail": "", "needs": "", "costUsd": 0.0}),
            json!({"type": "text", "text": "On it."}),
            json!({"type": "tool", "name": "Read", "doing": "reading view.go"}),
            json!({"type": "result", "text": "Done.", "isError": false, "costUsd": 0.0, "turns": 2}),
        ]
    );
    let mut got = Vec::new();
    crate::follow(lines, true, &FileHosts, |ev| got.push(Value::Object(ev)));
    assert_eq!(got[0], json!({"type": "sent", "text": "go"}), "{got:?}");
    assert_eq!(got[1], json!({"type": "delta", "text": "On"}));
    assert_eq!(got.len(), 8, "{got:?}");
}
