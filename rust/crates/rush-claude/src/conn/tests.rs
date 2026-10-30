//! conn_test.go: a fake claude answering rush's control requests.

use std::sync::mpsc::{Receiver, channel};

use rush_core::adapter::{Said, Tagged};

use super::*;
use crate::headless::tests::fake_claude;

/// Answers rush's control requests as claude -p does: the initialize with
/// its commands, then an MCP message and a permission check for rush's own
/// tool, and any other request with its subtype echoed.
const CONTROL: &str = r#"read -r init; printf '%s\n' "$init" >> "$LOG"
echo '{"type":"control_response","response":{"subtype":"success","request_id":"rush-1","response":{"commands":[{"name":"review","description":"Review a PR","argumentHint":"<pr>","aliases":[]}]}}}'
echo '{"type":"control_request","request_id":"m-1","request":{"subtype":"mcp_message","server_name":"t","message":{"jsonrpc":"2.0","id":1,"method":"tools/list"}}}'
echo '{"type":"control_request","request_id":"p-1","request":{"subtype":"can_use_tool","tool_name":"mcp__t__draw","input":{},"tool_use_id":"tu-9"}}'
echo '{"type":"control_request","request_id":"p-2","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"tu-10"}}'
while read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\(rush-[0-9]*\)".*/\1/p')
  sub=$(printf '%s' "$line" | sed -n 's/.*"subtype":"\([a-z_]*\)".*/\1/p')
  [ -n "$id" ] || continue
  case "$sub" in
  get_context_usage) echo '{"type":"control_response","response":{"subtype":"success","request_id":"'$id'","response":{"totalTokens":1200,"maxTokens":200000}}}' ;;
  *) echo '{"type":"control_response","response":{"subtype":"success","request_id":"'$id'","response":{"did":"'$sub'"}}}' ;;
  esac
done
"#;

const WAIT: Duration = Duration::from_secs(10);

fn next(rx: &Receiver<Said<u8>>) -> Event {
    match rx.recv_timeout(WAIT).expect("an event") {
        Said::Event(_, ev) => ev,
        Said::Ended(_, e) => panic!("ended: {e:?}"),
    }
}

fn want_in(path: &str, wants: &[&str]) {
    let b = std::fs::read_to_string(path).unwrap_or_default();
    for w in wants {
        assert!(b.contains(w), "{path} has {b}\nmissing {w}");
    }
}

/// The conn answers rush's own tools itself, keeps their traffic off the
/// tap, gives the session's commands as an event, and passes control
/// requests through.
#[test]
fn control() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, log) = fake_claude(dir.path(), CONTROL);
    let log = log.display().to_string();
    let tapped = Arc::new(Mutex::new(Vec::<String>::new()));
    let (handled_tx, handled) = channel::<String>();
    let handled_tx = Mutex::new(handled_tx);
    let tmp = dir.path().join("tmp").display().to_string();
    let t = tapped.clone();
    let o = StartOptions {
        dir: dir.path().display().to_string(),
        profile: crate::adapter::profile(&Account::at("x", dir.path().join("cfg"))),
        binary: bin.display().to_string(),
        temp_dir: tmp.clone(),
        lean: true,
        tools: vec![ToolServer {
            name: "t".into(),
            trusted: vec!["draw".into()],
            handle: Some(Arc::new(move |msg: &RawValue| {
                let _ = lock(&handled_tx).send(msg.get().to_string());
                RawJson::new(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#).unwrap()
            })),
            args: Vec::new(),
        }],
        agents: [(
            "p:helper".to_string(),
            RawJson::new(r#"{"description":"helps"}"#).unwrap(),
        )]
        .into(),
        prompt: "Be kind.".into(),
        tap: Some(Arc::new(move |l: &[u8]| {
            lock(&t).push(String::from_utf8_lossy(l).into_owned())
        })),
        ..StartOptions::default()
    };
    let (tx, rx) = channel();
    let conn = Claude::new()
        .start(o, Arc::new(Tagged { key: 1u8, tx }))
        .unwrap();
    let Event::Commands(c) = next(&rx) else {
        panic!("the first event isn't the commands")
    };
    assert_eq!(c.list.len(), 1);
    assert_eq!(
        (c.list[0].name.as_str(), c.list[0].argument_hint.as_str()),
        ("review", "<pr>")
    );
    let msg = handled
        .recv_timeout(WAIT)
        .expect("the tool server was asked");
    assert!(msg.contains("tools/list"), "{msg}");
    // Someone else's tool asks.
    let Event::Approval(a) = next(&rx) else {
        panic!("Bash's approval")
    };
    assert_eq!(a.id, "p-2");

    let body = conn
        .asker()
        .unwrap()
        .ask(&RawJson::new(r#"{"subtype":"get_settings"}"#).unwrap().0)
        .unwrap();
    assert!(
        body.get().contains(r#""did":"get_settings""#),
        "{}",
        body.get()
    );
    let u = conn.context_reader().unwrap().context_usage().unwrap();
    assert_eq!(u.total, 1200);
    assert_ne!(u.at, 0);
    conn.task_stopper().unwrap().stop_task("b1").unwrap();
    conn.backgrounder().unwrap().background("tu-1").unwrap();
    conn.responder()
        .unwrap()
        .deny("p-2", "not now", true)
        .unwrap();
    assert!(
        conn.answer("p-2", "allow").is_err(),
        "an answered request is forgotten"
    );
    // Whatever was sent is answered before it closes: the last asked.
    conn.asker()
        .unwrap()
        .ask(&RawJson::new(r#"{"subtype":"last"}"#).unwrap().0)
        .unwrap();
    conn.close().unwrap();
    assert!(matches!(rx.recv_timeout(WAIT), Ok(Said::Ended(1, _))));
    want_in(
        &log,
        &[
            r#""sdkMcpServers":["t"]"#,
            r#""mcp_response""#,
            r#""request_id":"p-1""#,
            r#""behavior":"allow""#,
            r#""subtype":"stop_task""#,
            r#""task_id":"b1""#,
            r#""tool_use_id":"tu-1""#,
            r#""message":"not now""#,
            r#""interrupt":true"#,
        ],
    );
    want_in(
        &format!("{log}.args"),
        &[
            "--allowedTools mcp__t__draw",
            r#"--agents {"p:helper":{"description":"helps"}}"#,
            "--append-system-prompt Be kind.",
        ],
    );
    want_in(
        &format!("{log}.env"),
        &[
            &format!("CLAUDE_CODE_TMPDIR={tmp}"),
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1",
            "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING=1",
        ],
    );
    let tapped = lock(&tapped);
    assert!(!tapped.is_empty(), "nothing reached the tap");
    for l in tapped.iter() {
        assert!(
            !l.contains(r#""m-1""#) && !l.contains(r#""p-1""#),
            "own traffic reached the tap: {l}"
        );
    }
}

/// A question is answered with its answers in its own input.
#[test]
fn answers_a_question() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, log) = fake_claude(
        dir.path(),
        r#"read -r init
echo '{"type":"control_request","request_id":"q-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Go?","options":[{"label":"yes","preview":"ok"},{"label":"no"}]}],"metadata":{"source":"x"}},"tool_use_id":"tu-q"}}'
read -r answer; printf '%s\n' "$answer" >> "$LOG"
"#,
    );
    let (tx, rx) = channel();
    let conn = Claude::new()
        .start(
            StartOptions {
                profile: crate::adapter::profile(&Account::at("x", dir.path().join("cfg"))),
                binary: bin.display().to_string(),
                ..StartOptions::default()
            },
            Arc::new(Tagged { key: 2u8, tx }),
        )
        .unwrap();
    let Event::Question(q) = next(&rx) else {
        panic!("not a question")
    };
    assert_eq!(q.asks[0].text, "Go?");
    let answers = [("Go?".to_string(), vec!["yes".to_string()])].into();
    conn.answerer()
        .unwrap()
        .answer_question("q-1", &answers)
        .unwrap();
    assert!(matches!(rx.recv_timeout(WAIT), Ok(Said::Ended(2, None))));
    let line = std::fs::read_to_string(&log).unwrap();
    let v: Value = serde_json::from_str(line.trim()).unwrap();
    let input = &v["response"]["response"]["updatedInput"];
    assert_eq!(input["answers"]["Go?"], "yes");
    assert_eq!(input["annotations"]["Go?"]["preview"], "ok");
    assert_eq!(input["metadata"]["source"], "x");
}

/// Answers go into the question's own input: what else Claude put there
/// stays, and an answer's own keys win.
#[test]
fn answers_keep_the_question() {
    let input = RawJson::new(
        r#"{"questions":[{"question":"Go?"}],"metadata":{"source":"x"},"answers":{}}"#,
    )
    .unwrap();
    let top = RawJson::new(r#"{"answers":{"Go?":"yes"}}"#).unwrap();
    let got = over(Some(&input), &top.0);
    for want in [
        r#""questions":[{"question":"Go?"}]"#,
        r#""metadata":{"source":"x"}"#,
        r#""answers":{"Go?":"yes"}"#,
    ] {
        assert!(got.get().contains(want), "{} is missing {want}", got.get());
    }
    assert_eq!(over(None, &top.0).get(), top.get());
}

#[test]
fn own_traffic_stays_off_the_tap() {
    let tools = vec![ToolServer {
        name: "rush".into(),
        trusted: vec!["draw".into()],
        ..ToolServer::default()
    }];
    for (line, want) in [
        (
            r#"{"type":"control_request","request_id":"a","request":{"subtype":"mcp_message","server_name":"rush"}}"#,
            true,
        ),
        (
            r#"{"type":"control_request","request_id":"a","request":{"subtype":"mcp_message","server_name":"other"}}"#,
            false,
        ),
        (
            r#"{"type":"control_request","request_id":"a","request":{"subtype":"can_use_tool","tool_name":"mcp__rush__draw"}}"#,
            true,
        ),
        (
            r#"{"type":"control_request","request_id":"a","request":{"subtype":"can_use_tool","tool_name":"mcp__rush__erase"}}"#,
            false,
        ),
        (r#"{"type":"user"}"#, false),
    ] {
        assert_eq!(own_traffic(&tools, line.as_bytes()), want, "{line}");
    }
    assert!(relay_only(br#"{"type":"stream_event","event":{}}"#));
    assert!(!relay_only(br#"{"type":"assistant"}"#));
}

#[test]
fn carries_another_agents_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let acct = Account::at("x", dir.path());
    let lines = [
        PreviewLine {
            role: "user".into(),
            text: "hi".into(),
            at: 1_790_000_000_000,
            ..PreviewLine::default()
        },
        PreviewLine {
            role: "assistant".into(),
            text: "hello".into(),
            ..PreviewLine::default()
        },
    ];
    write_carried(&acct, "/w", "s-9", &lines).unwrap();
    let b = std::fs::read_to_string(acct.transcript_path("/w", "s-9")).unwrap();
    let ls: Vec<Value> = b
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(ls.len(), 2);
    assert_eq!(ls[0]["parentUuid"], Value::Null);
    assert_eq!(ls[0]["message"]["content"], "hi");
    assert_eq!(ls[1]["parentUuid"], ls[0]["uuid"]);
    assert_eq!(ls[1]["message"]["model"], "<synthetic>");
    assert_eq!(ls[1]["message"]["content"][0]["text"], "hello");
    assert_eq!(ls[0]["uuid"].as_str().unwrap().len(), 36);
}
