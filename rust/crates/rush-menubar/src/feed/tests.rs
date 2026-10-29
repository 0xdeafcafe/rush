//! Port of internal/menubar/feed_test.go: a fake host serves a replay as a
//! session's host does and hands back the ops a client writes.

use std::io::{BufReader, Cursor};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use rush_core::time::Ms;
use rush_core::usage::{Scope, Window as QWindow};

use super::*;

const LINE_QUESTION: &str = r#"{"type":"control_request","request_id":"req-q","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which database?","header":"Storage","multiSelect":false,"options":[{"label":"Postgres (Recommended)","description":"d"},{"label":"SQLite","description":"d","preview":"file.db"}]}]},"tool_use_id":"toolu_q"}}"#;
const LINE_OLD: &str = r#"{"type":"control_request","request_id":"req-old","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"toolu_o"}}"#;
const LINE_ANSWERED: &str = r#"{"type":"agtop_answered","request_id":"req-old"}"#;
const LINE_INFO: &str =
    r#"{"type":"agtop_info","info":{"id":"h1","state":"blocked","needs":"asks: Which database?"}}"#;

/// A host listening on a socket of its own: unix socket paths are short,
/// so it lives under /tmp.
struct FakeHost {
    _dir: tempfile::TempDir,
    sock: PathBuf,
}

impl Hosts for FakeHost {
    fn dial(&self, _: &str) -> std::io::Result<Client> {
        Client::connect(&self.sock)
    }
    fn log(&self, _: &str, _: &str, _: serde_json::Map<String, serde_json::Value>) {}
}

fn fake_host(replay: &[&str]) -> (FakeHost, Receiver<serde_json::Value>) {
    let dir = tempfile::Builder::new()
        .prefix("rush-mb")
        .tempdir_in("/tmp")
        .unwrap();
    let sock = dir.path().join("host.sock");
    let ln = UnixListener::bind(&sock).unwrap();
    let (tx, ops) = channel();
    let replay: Vec<String> = replay.iter().map(|s| s.to_string()).collect();
    std::thread::spawn(move || {
        for c in ln.incoming() {
            let Ok(mut c) = c else { return };
            let replay = replay.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in &replay {
                    let _ = c.write_all(format!("{l}\n").as_bytes());
                }
                for l in BufReader::new(c).lines() {
                    let Ok(l) = l else { return };
                    let o: serde_json::Value = serde_json::from_str(&l).unwrap_or_default();
                    if o["op"] == "hello" {
                        assert_eq!(o["proto"], PROTO_SAID);
                        continue; // what every client says first
                    }
                    let _ = tx.send(o);
                }
            });
        }
    });
    (FakeHost { _dir: dir, sock }, ops)
}

const PROTO_SAID: i64 = crate::host::PROTO;

fn op(op: &str, key: &str, req: &str, answer: &str) -> Op {
    Op {
        op: op.into(),
        key: key.into(),
        req: req.into(),
        answer: answer.into(),
        yes: false,
    }
}

#[test]
fn question_answered_from_outside() {
    let (h, ops) = fake_host(&[LINE_OLD, LINE_QUESTION, LINE_ANSWERED, LINE_INFO]);
    let mut a = Row {
        id: "h1".into(),
        state: Doing::Blocked,
        needs: "asks: Which database?".into(),
        key: "acct/a:h1".into(),
        rush: true,
        pid: 1,
        display_name: "db".into(),
        ..Row::default()
    };
    let mut asked = Asked::new();
    let snap = |a: &Row| Snapshot {
        agents: vec![a.clone()],
        ..Snapshot::default()
    };
    let s = build(&snap(&a), &mut asked, &h);
    assert_eq!(s.waiting.len(), 1, "{:?}", s.waiting);
    let w = &s.waiting[0];
    assert_eq!(
        (
            w.kind.as_str(),
            w.req.as_str(),
            w.text.as_str(),
            w.header.as_str()
        ),
        ("question", "req-q", "Which database?", "Storage")
    );
    assert_eq!(w.options, ["Postgres (Recommended)", "SQLite"]);
    do_op(
        &op("answer", &w.key, &w.req, "SQLite"),
        asked.get(&w.key),
        &h,
    )
    .unwrap();
    let o = ops
        .recv_timeout(Duration::from_secs(2))
        .expect("no op reached the host");
    assert_eq!(o["op"], "allow");
    assert_eq!(o["id"], "req-q");
    assert_eq!(o["input"]["answers"]["Which database?"], "SQLite");
    assert!(!o["input"]["annotations"]["Which database?"].is_null());
    // A stale answer, for a request already settled, goes nowhere.
    assert_eq!(
        do_op(&op("answer", &w.key, "req-old", "x"), asked.get(&w.key), &h),
        Err(GONE.to_string())
    );
    // Once it isn't waiting, it's forgotten.
    a.state = Doing::Working;
    build(&snap(&a), &mut asked, &h);
    assert!(asked.is_empty(), "asked kept {asked:?}");
}

#[test]
fn permission_and_limit() {
    let (h, ops) = fake_host(&[
        LINE_OLD,
        r#"{"type":"agtop_info","info":{"id":"h2","state":"blocked","limit":{"ask":true}}}"#,
    ]);
    let a = Row {
        id: "h2".into(),
        state: Doing::Blocked,
        needs: "continue?".into(),
        key: "acct/a:h2".into(),
        rush: true,
        pid: 1,
        ..Row::default()
    };
    let mut asked = Asked::new();
    let s = build(
        &Snapshot {
            agents: vec![a],
            ..Snapshot::default()
        },
        &mut asked,
        &h,
    );
    assert_eq!(s.waiting[0].kind, "limit");
    let mut o = op("limit", "acct/a:h2", "", "");
    o.yes = true;
    do_op(&o, asked.get("acct/a:h2"), &h).unwrap();
    let got = ops.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        (got["op"].as_str(), got["now"].as_bool()),
        (Some("limit"), Some(true))
    );

    // A Claude Code agent's question is shown but can't be answered here.
    let b = Row {
        id: "j".into(),
        state: Doing::Blocked,
        needs: "pick one".into(),
        key: "acct/j".into(),
        pid: 1,
        ..Row::default()
    };
    let s = build(
        &Snapshot {
            agents: vec![b],
            ..Snapshot::default()
        },
        &mut Asked::new(),
        &h,
    );
    assert_eq!(
        (s.waiting[0].kind.as_str(), s.waiting[0].needs.as_str()),
        ("", "pick one")
    );
}

#[test]
fn permission_is_allowed_or_denied() {
    let ask = r#"{"type":"control_request","request_id":"req-p","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm -rf x"},"decision_reason":"  This command requires approval \n","tool_use_id":"toolu_p"}}"#;
    let info = r#"{"type":"agtop_info","info":{"id":"h3","state":"blocked"}}"#;
    let (h, ops) = fake_host(&[ask, info]);
    let a = Row {
        id: "h3".into(),
        state: Doing::Blocked,
        needs: "Bash: rm -rf x".into(),
        key: "k3".into(),
        rush: true,
        pid: 1,
        ..Row::default()
    };
    let mut asked = Asked::new();
    let snap = Snapshot {
        agents: vec![a],
        ..Snapshot::default()
    };
    let w = build(&snap, &mut asked, &h).waiting.remove(0);
    assert_eq!(
        (w.kind.as_str(), w.req.as_str(), w.text.as_str()),
        ("permission", "req-p", "This command requires approval")
    );
    do_op(&op("deny", "k3", "req-p", ""), asked.get("k3"), &h).unwrap();
    let got = ops.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        (got["op"].as_str(), got["id"].as_str()),
        (Some("deny"), Some("req-p"))
    );
    // Asked again with nothing changed, the host isn't dialled again.
    build(&snap, &mut asked, &h);
    do_op(&op("allow", "k3", "req-p", ""), asked.get("k3"), &h).unwrap();
    let got = ops.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(got["op"], "allow");
    assert!(got.get("input").is_none(), "allow keeps the input asked");
}

#[test]
fn windows_are_a_quotas_first_two() {
    let q = Quota {
        windows: vec![
            QWindow {
                id: "primary".into(),
                label: "7d".into(),
                name: "weekly".into(),
                percent: 40.0,
                ..QWindow::default()
            },
            QWindow {
                id: "gpt-reserve:primary".into(),
                label: "gpt 7d".into(),
                percent: 90.0,
                scope: Scope {
                    models: vec!["gpt".into()],
                },
                ..QWindow::default()
            },
        ],
        ..Quota::default()
    };
    let (first, second) = windows(&q);
    assert!(
        first.present && first.label == "7d" && first.percent == 40.0,
        "{first:?}"
    );
    assert!(
        !second.present,
        "a single model's limit took the second place: {second:?}"
    );
}

#[test]
fn one_line_takes_the_first() {
    for (s, want) in [
        ("  hello  ", "hello"),
        ("first \nsecond", "first …"),
        ("", ""),
        ("\n\nx", "x"),
    ] {
        assert_eq!(one_line(s), want, "{s:?}");
    }
}

fn working(key: &str, created_at: Ms) -> Row {
    Row {
        key: key.into(),
        display_name: key.into(),
        state: Doing::Working,
        pid: 1,
        created_at,
        detail: "running tests\nmore".into(),
        ..Row::default()
    }
}

/// The longest-running come first; past five, a count; checking and
/// processless rows aren't working.
#[test]
fn working_longest_first() {
    let mut agents: Vec<Row> = (0..7).map(|i| working(&format!("w{i}"), 100 - i)).collect();
    agents.push(Row {
        checking: true,
        ..working("checking", 1)
    });
    agents.push(Row {
        pid: 0,
        ..working("gone", 1)
    });
    let s = build(
        &Snapshot {
            agents,
            ..Snapshot::default()
        },
        &mut Asked::new(),
        &Quiet,
    );
    let keys: Vec<&str> = s.working.iter().map(|w| w.key.as_str()).collect();
    assert_eq!(keys, ["w6", "w5", "w4", "w3", "w2"]);
    assert_eq!(s.more, 2);
    assert_eq!(s.working[0].doing, "running tests …");
    assert_eq!(s.working[0].since, 94);
}

/// With logins, they are the accounts, and the one in use has the live
/// agents; other agents' accounts follow, with their own live count.
#[test]
fn accounts_from_logins_and_others() {
    use crate::source::{Account as Av, Login, Other};
    let snap = Snapshot {
        agents: vec![Row {
            kind: "codex".into(),
            ..working("c", 1)
        }],
        accounts: vec![Av {
            name: "root".into(),
            live: 3,
            ..Av::default()
        }],
        logins: vec![
            Login {
                name: "me".into(),
                email: "me@x".into(),
                current: true,
                ..Login::default()
            },
            Login {
                name: "team".into(),
                ..Login::default()
            },
        ],
        others: vec![Other {
            name: "Codex".into(),
            kind: "codex".into(),
            quota: Quota {
                balance: "¥110.00 left".into(),
                ..Quota::default()
            },
        }],
    };
    let s = build(&snap, &mut Asked::new(), &Quiet);
    let got: Vec<(&str, i64, bool, &str)> = s
        .accounts
        .iter()
        .map(|a| (a.name.as_str(), a.live, a.current, a.plan.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("me", 3, true, ""),
            ("team", 0, false, ""),
            ("Codex", 1, false, "¥110.00 left")
        ]
    );
}

/// The loop writes the state once, an error for an op it can't carry
/// out, and ends when the ops do.
#[test]
fn feed_writes_state_and_errors() {
    let snap = Snapshot {
        agents: vec![working("w", 5)],
        ..Snapshot::default()
    };
    let input =
        Cursor::new(b"not json\n{\"op\":\"allow\",\"key\":\"nobody\",\"req\":\"r\"}\n".to_vec());
    let mut out = Vec::new();
    run(input, &mut out, Box::new(Fixed(snap)), Arc::new(Quiet)).unwrap();
    let out = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    let s: State = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(s.working[0].key, "w");
    assert_eq!(lines[1], wire::error_line(GONE));
}

/// Hosts that log nowhere, so a test's ops stay out of rush's real log.
struct Quiet;

impl Hosts for Quiet {
    fn dial(&self, id: &str) -> std::io::Result<Client> {
        Err(std::io::Error::other(format!("no host {id}")))
    }
    fn log(&self, _: &str, _: &str, _: serde_json::Map<String, serde_json::Value>) {}
}
