//! The sheets' and panels' behaviour, after Go's internal/ui tests
//! (misdirect_test.go, mention_test.go, quickask_test.go,
//! sidekick_test.go, discuss_test.go, rewind_test.go,
//! compactsheet_test.go, messagesheet_test.go, handoff_ready_test.go)
//! and the keys each sheet answers, as tables. No sleeps: a model with
//! no loop runs handed-off work at once.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use rush_core::agent::{Agent, State, Todo};

use super::btw::{Btws, Qa};
use super::discuss::{chat_turns, ctx_words};
use super::fork::{cut_after, worktree_name};
use super::handoff::{await_carried, handoff_quiet};
use super::mention::*;
use super::message::{
    MessageSheet, UserMessage, expanded_message, hard_wrap, message_link, open_user_message,
};
use super::misdirect::*;
use super::quickask::{QuickAsk, guide_opens};
use super::rewind::turn_starts;
use super::sidekick::parse_side;
use super::stopnote::StopNote;
use super::tests::*;
use super::*;
use crate::frame::Sheet;

fn agent(key: &str, name: &str) -> Agent {
    Agent {
        key: key.into(),
        display_name: name.into(),
        ..Agent::default()
    }
}

// The two sessions of the day it went wrong: a langwatch coordinator and
// a yap session whose newest commit was its History window.
fn misdirect_fleet() -> (Agent, Agent, HashMap<String, Vec<String>>) {
    let mut lw = agent("lw", "coordinator");
    lw.repo = "langwatch".into();
    lw.branch = "feat/strict-feature-layout-v0".into();
    lw.job.cwd = "/src/langwatch".into();
    lw.job.name = "coordinator".into();
    lw.job.intent = "collect lanes and commit slices".into();
    lw.job.todos = vec![
        Todo {
            label: "Collect dashboard lane".into(),
            ..Todo::default()
        },
        Todo {
            label: "Run lint on identity module".into(),
            ..Todo::default()
        },
    ];
    lw.job.state = State::Working;
    let mut yap = agent("yap", "yap");
    yap.repo = "yap".into();
    yap.branch = "main".into();
    yap.job.cwd = "/src/yap".into();
    yap.job.name = "dictation app".into();
    yap.job.intent = "build the dictation app".into();
    yap.job.state = State::Working;
    yap.pid = 42;
    let mut subjects = HashMap::new();
    subjects.insert(
        "/src/langwatch".to_string(),
        vec![
            "fix(dashboard): an invalid widget definition is a 422 refusal".to_string(),
            "refactor(identity): auth commands go through AuthApi".to_string(),
        ],
    );
    subjects.insert(
        "/src/yap".to_string(),
        vec![
            "feat: History window lists past dictations".to_string(),
            "feat(macos): menu bar icon".to_string(),
        ],
    );
    (lw, yap, subjects)
}

/// TestMisdirectedRoutesToTheBetterMatch, …LeavesOrdinaryMessagesAlone,
/// …NeedsMoreThanAWord.
#[test]
fn misdirected_picks_only_a_clear_match() {
    let (lw, yap, mut subjects) = misdirect_fleet();
    let agents = vec![lw.clone(), yap.clone()];
    for text in [
        "this history needs to use macos liquid glass ui!!",
        "in yap, make the window wider",
    ] {
        let (to, words) = misdirected(text, &lw, &[1], &agents, &subjects).expect(text);
        assert_eq!(to, 1, "{text}");
        assert!(!words.is_empty());
    }
    for text in [
        "keep going",
        "commit",
        "yes",
        "looks good, ship it",
        "fix the failing test",
        "why did the dashboard widget refuse with a 422?",
        "use astra for all ui stuff",
        "",
        "the dashboard history window",
        "the history is wrong",
    ] {
        assert!(
            misdirected(text, &lw, &[1], &agents, &subjects).is_none(),
            "{text}"
        );
    }
    // Two sessions as likely as each other: nothing stands out.
    let mut yap2 = yap.clone();
    yap2.key = "yap2".into();
    let three = vec![lw.clone(), yap.clone(), yap2];
    assert!(
        misdirected(
            "in yap, make the window wider",
            &lw,
            &[1, 2],
            &three,
            &subjects
        )
        .is_none()
    );
    // A session in the same repo says nothing.
    let mut other = yap.clone();
    other.key = "lw2".into();
    other.repo = "langwatch".into();
    other.job.cwd = "/src/langwatch2".into();
    subjects.insert("/src/langwatch2".into(), subjects["/src/yap"].clone());
    let two = vec![lw.clone(), other];
    assert!(
        misdirected(
            "this history needs to use macos liquid glass ui!!",
            &lw,
            &[1],
            &two,
            &subjects
        )
        .is_none()
    );
}

#[derive(Default)]
struct Sent(usize);

fn misdirect_model() -> Model {
    let (lw, yap, subjects) = misdirect_fleet();
    let mut m = hosted(120, 40);
    m.agents = vec![lw, yap];
    m.order = vec![0, 1];
    m.kept::<Fake>().info.insert(
        "lw".into(),
        HostInfo {
            kind: "claude".into(),
            ..HostInfo::default()
        },
    );
    m.kept::<Misdirect>().subjects = subjects;
    m
}

fn send_here() -> crate::frame::confirm::Then {
    Box::new(|m: &mut Model| {
        m.kept::<Sent>().0 += 1;
        Out::Draw
    })
}

/// TestAskMisdirectedKeys, TestAskMisdirectedSkipsFinishedSessions.
#[test]
fn misdirected_asks_and_its_keys() {
    let text = "this history needs to use macos liquid glass ui!!";
    let mut m = misdirect_model();
    assert!(!ask_misdirected(&mut m, "lw", "keep going", send_here()) && m.confirm.is_none());
    assert!(ask_misdirected(&mut m, "lw", text, send_here()));
    let line = m.confirm.as_ref().unwrap().box_line().plain();
    for want in [
        "Looks meant for yap (yap)",
        "send there",
        "s send here anyway",
    ] {
        assert!(line.contains(want), "{line}");
    }
    // esc: nothing sent, nothing forwarded.
    press(&mut m, "esc");
    assert!(m.confirm.is_none() && m.kept::<Sent>().0 == 0 && acts(&mut m).is_empty());
    // s: sent here.
    ask_misdirected(&mut m, "lw", text, send_here());
    press(&mut m, "s");
    assert_eq!(m.kept::<Sent>().0, 1);
    assert!(
        !m.kept::<Misdirect>().skip,
        "the next send is checked again"
    );
    // enter and space: forwarded to yap, the box emptied.
    for k in ["enter", "space"] {
        let mut m = misdirect_model();
        ask_misdirected(&mut m, "lw", text, send_here());
        press(&mut m, k);
        assert_eq!(
            acts(&mut m),
            vec![Act::ReplyTo {
                to: "yap".into(),
                text: text.into(),
                tagged: text.into()
            }],
            "{k}"
        );
        assert_eq!(m.kept::<Sent>().0, 0);
    }
    // A finished session isn't offered.
    let mut m = misdirect_model();
    m.agents[1].job.state = State::Done;
    assert!(!ask_misdirected(
        &mut m,
        "lw",
        "in yap, make the window wider",
        send_here()
    ));
}

fn mention_model() -> Model {
    let mut m = hosted(120, 40);
    let mut fix = agent("a", "fix login bug");
    fix.branch = "fix/login".into();
    let docs = agent("b", "docs");
    let mut term = agent("c", "login page");
    term.interactive = true;
    m.agents = vec![docs, fix, term];
    m.order = vec![0, 1, 2];
    m.group_of.insert("a".into(), "Working".into());
    m.group_of.insert("b".into(), "Done".into());
    m
}

/// You send text in the open conversation: a turn of yours starts.
fn say(m: &mut Model, text: &str) {
    m.session
        .as_mut()
        .unwrap()
        .sess
        .sent(text.into(), vec![], 1_000);
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

/// TestMention.
#[test]
fn mentions_tag_agents() {
    let m = mention_model();
    assert_eq!(
        mention_matches(&m, &chars("@LOG"), 0),
        vec!["fix-login-bug".to_string()]
    );
    let all = mention_matches(&m, &chars("@"), 0);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], "docs");
    let rows = mention_rows(&m, &chars("@LOG"), 0, 0, 100);
    assert!(
        rows[0]
            .plain()
            .contains("fix login bug    Working · fix/login  @fix-login-bug"),
        "{}",
        rows[0].plain()
    );
    assert!(
        mention_matches(&m, &chars("@docs hi"), 0).is_empty(),
        "the cursor left the tag"
    );
    let (i, rest) = mentioned(&m, "@Fix-Login-Bug  try again\nplease").unwrap();
    assert_eq!(
        (m.agents[i].key.as_str(), rest.as_str()),
        ("a", "try again\nplease")
    );
    let (i, rest) = mentioned(&m, &format!("@{} hi", nickname("a"))).unwrap();
    assert_eq!((m.agents[i].key.as_str(), rest.as_str()), ("a", "hi"));
    assert!(nickname("a").contains('-'));
    assert_eq!(
        title_tag("Can you fix the flaky login test, please?"),
        "fix-flaky-login"
    );
    assert_eq!(title_tag(""), "");
    assert!(mentioned(&m, "@src/main.go explain").is_none());
}

/// TestMentionMidMessage.
#[test]
fn mentions_mid_message() {
    let mut m = hosted(120, 40);
    let mut docs = agent("b", "docs");
    docs.job.cwd = "/src/site".into();
    m.agents = vec![docs, agent("p", "old run")];
    m.order = vec![0, 1];
    m.kept::<Fake>().info.insert(
        "b".into(),
        HostInfo {
            kind: "claude".into(),
            id: "b1b2c3d4".into(),
            ..HostInfo::default()
        },
    );
    let input = chars("ask @do and tell me");
    let back = " and tell me".len();
    let got = mention_matches(&m, &input, back);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0], rush_room::community::name("b1b2c3d4"));
    assert!(tags(&m.agents[0], Some("b1b2c3d4"), "docs"));
    let (out, b) = complete_mention(&input, back, "docs");
    assert_eq!(out.iter().collect::<String>(), "ask @docs and tell me");
    assert_eq!(b, "and tell me".len());
    assert!(
        mention_matches(&m, &chars("mail me@do"), 0).is_empty(),
        "an email isn't a tag"
    );
    let sent = with_mentions(&m, "ask @docs and @old-run what changed.", "self");
    assert!(
        sent.starts_with("ask @docs and @old-run what changed.\n\n<system-reminder>\n"),
        "{sent}"
    );
    assert!(sent.ends_with("\n</system-reminder>"));
    assert!(
        sent.contains("session send b1b2c3d4") && sent.contains("in /src/site"),
        "{sent}"
    );
    assert!(
        sent.contains("@old-run is old run, another agent rush runs; it isn't a rush session"),
        "{sent}"
    );
    assert_eq!(with_mentions(&m, "@docs first", "self"), "@docs first");
    assert_eq!(with_mentions(&m, "me, @docs", "b"), "me, @docs");
    // Sending to a tag from the box.
    assert_eq!(send_mentioned(&mut m, "k", "hello", "hello"), Out::Pass);
    send_mentioned(&mut m, "k", "@docs look", "@docs look");
    assert_eq!(
        acts(&mut m),
        vec![Act::ReplyTo {
            to: "b".into(),
            text: "look".into(),
            tagged: "look".into()
        }]
    );
}

static ASKED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn stub_asker(p: &str) -> Result<(String, String), String> {
    let mut a = ASKED.lock().unwrap();
    a.push(p.to_string());
    if p.ends_with("what's next?") {
        return Ok(("luna".into(), "That one's waiting on you.\nopen: 2".into()));
    }
    Ok(("luna".into(), "yes: KeyTab with ModCtrl".into()))
}

fn type_in(m: &mut Model, s: &str) {
    for c in s.chars() {
        press(m, &c.to_string());
    }
}

/// TestQuickAskThread, TestGuideOpens, TestQuickAskInsert.
#[test]
fn quick_ask_thread() {
    let mut m = hosted(160, 40);
    m.agents = vec![agent("x", "first"), agent("y", "the second one")];
    m.order = vec![0, 1];
    m.kept::<QuickAsk>().asker = Some(stub_asker);
    ASKED.lock().unwrap().clear();
    press(&mut m, "alt+/");
    assert!(
        m.kept::<QuickAsk>().focused(),
        "alt+/ opens it with the keys"
    );
    type_in(&mut m, "ctrl+tab?");
    press(&mut m, "enter");
    type_in(&mut m, "kitty?");
    press(&mut m, "enter");
    {
        let asked = ASKED.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert!(
            asked[1].contains("Q: ctrl+tab?\nA: yes") && asked[1].ends_with("Now: kitty?"),
            "{asked:?}"
        );
        assert!(asked[0].contains("[2] the second one"), "{asked:?}");
    }
    assert!(acts(&mut m).is_empty(), "nothing reached the box under it");
    let screen = text(&m.frame());
    assert!(
        screen.contains("guide · luna") && screen.contains("› kitty?"),
        "{screen}"
    );
    press(&mut m, "esc");
    assert!(!m.kept::<QuickAsk>().shown && !text(&m.frame()).contains("guide · luna"));
    press(&mut m, "alt+/");
    assert!(m.kept::<QuickAsk>().focused());
    assert_eq!(
        m.kept::<QuickAsk>().thread().unwrap().qa.len(),
        2,
        "the same thread back"
    );
    press(&mut m, "alt+/");
    assert!(
        m.kept::<QuickAsk>().shown && !m.kept::<QuickAsk>().focused(),
        "again hands the keys back"
    );
    // Into the open chat's box, as one paste.
    assert_eq!(run_action(&mut m, "session.quickask.insert"), Out::Draw);
    match acts(&mut m).last() {
        Some(Act::Paste(p)) => assert!(p.contains("Q: ctrl+tab?\nA: yes"), "{p}"),
        a => panic!("no paste: {a:?}"),
    }
    assert!(!m.kept::<QuickAsk>().shown);
    // The guide's open: N opens the Nth, the line taken off.
    super::quickask::open_guide(&mut m, "what's next?");
    let qa = m.kept::<QuickAsk>().thread().unwrap().qa[0].clone();
    assert!(
        !qa.response.contains("open:") && qa.response.contains("opened the second one"),
        "{qa:?}"
    );
    assert_eq!(
        guide_opens("no.\nopen: 99", &["x".into()]),
        ("no.".into(), String::new())
    );
}

/// TestParseSide.
#[test]
fn recall_reads_its_notes() {
    let (d, g, q) = parse_side(
        "Here are the notes:\nD: hosts own sessions\n- G: a second cache beside the store\nQ:\nq: who owns retries?\nD: a\nD: b\nD: c\nD: d\nD: e\nX: noise",
    );
    assert_eq!(d.len(), 5);
    assert_eq!((d[0].as_str(), d[4].as_str()), ("hosts own sessions", "d"));
    assert_eq!(g, vec!["a second cache beside the store".to_string()]);
    assert_eq!(q, vec!["who owns retries?".to_string()]);
    assert_eq!(
        parse_side("G!: no auth on the new route").1,
        vec!["!no auth on the new route".to_string()]
    );
    let mut m = hosted(120, 40);
    run_action(&mut m, "session.recall");
    assert!(m.kept::<super::sidekick::Sidekick>().on);
}

/// ctxWords, and a chat's exchanges as a member reads them (discuss.go).
#[test]
fn discuss_reads_the_chat() {
    assert_eq!(ctx_words(0, 0), "the chat has nothing to give yet");
    assert_eq!(ctx_words(0, 4), "the topic alone");
    assert_eq!(ctx_words(1, 4), "the latest exchange of the chat");
    assert_eq!(ctx_words(3, 4), "the latest 3 of the chat's 4 exchanges");
    let mut m = hosted(120, 40);
    say(&mut m, "why is it slow");
    let s = &m.session.as_ref().unwrap().sess;
    let turns = chat_turns(s, false);
    assert_eq!(turns, vec!["User: why is it slow\n".to_string()]);
    // #discuss asks the room screens for a set-up on the chat.
    super::discuss::discuss(&mut m, "k", "", false);
    match acts(&mut m).pop() {
        Some(Act::Room(r)) => {
            assert_eq!(
                (r.topic.as_str(), r.ctx, r.fill),
                ("why is it slow", 1, false)
            );
        }
        a => panic!("no room: {a:?}"),
    }
}

/// ctrl+] t sets a room up on a chat, never on a room: a room is the
/// discussion (keybind.go:189).
#[test]
fn discuss_key_on_a_chat_not_a_room() {
    let mut m = hosted(120, 40);
    say(&mut m, "why is it slow");
    run_action(&mut m, "session.discuss");
    assert!(matches!(acts(&mut m).pop(), Some(Act::Room(_))));
    m.kept::<super::tests::Fake>().acts.clear();
    m.session.as_mut().unwrap().key = format!("{ROOM_KEY_PREFIX}r1");
    assert_eq!(run_action(&mut m, "session.discuss"), Out::Idle);
    assert!(acts(&mut m).is_empty() && m.sheet.is_none());
}

/// Where each of your messages starts in a transcript, and where a turn
/// ends for a fork of part of it (convo/turnstarts.go, cutAfter).
#[test]
fn turn_starts_and_cuts() {
    let user = |uuid: &str, t: &str, text: &str| {
        format!(
            r#"{{"type":"user","uuid":"{uuid}","timestamp":"2026-09-23T20:{t}Z","message":{{"role":"user","content":"{text}"}},"origin":{{"kind":"human"}}}}"#
        )
    };
    let said = |t: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-09-23T20:{t}Z","message":{{"id":"m{t}","role":"assistant","content":[{{"type":"text","text":"ok"}}]}}}}"#
        )
    };
    let lines = [
        user("u1", "00:00", "fix the bug"),
        said("00:05"),
        user("u2", "01:00", "add a test"),
        said("01:05"),
        user("u3", "02:00", "make it faster"),
        said("02:05"),
    ];
    let body = lines.join("\n") + "\n";
    let dir = std::env::temp_dir().join(format!("rush-turnstarts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.jsonl");
    std::fs::write(&p, &body).unwrap();
    let d = rush_convo::decode(rush_claude::messages);
    let starts = turn_starts(&p, d.clone()).unwrap();
    let got: Vec<(u32, &str, u64, &str)> = starts
        .iter()
        .map(|s| (s.n, s.prompt.as_str(), s.offset, s.uuid.as_str()))
        .collect();
    let off2 = (lines[0].len() + lines[1].len() + 2) as u64;
    let off3 = off2 + (lines[2].len() + lines[3].len() + 2) as u64;
    assert_eq!(
        got,
        vec![
            (1, "fix the bug", 0, "u1"),
            (2, "add a test", off2, "u2"),
            (3, "make it faster", off3, "u3")
        ]
    );
    assert_eq!(
        cut_after(&p, d.clone(), 2, "make it faster"),
        Ok(off3 as i64)
    );
    assert_eq!(cut_after(&p, d.clone(), 1, ""), Ok(off2 as i64));
    assert!(cut_after(&p, d, 9, "").is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// worktreeName.
#[test]
fn worktree_names() {
    for (name, want) in [
        ("fix login (fork)", "fix-login-fork"),
        ("  ", "fork"),
        ("btw: what's this?", "btw-what-s-this"),
        (&"a".repeat(50), &"a".repeat(40)),
    ] {
        assert_eq!(worktree_name(name), want, "{name}");
    }
}

/// /export: asks for the conversation, then copies it or saves it where
/// the agent works (asksheets.go).
#[test]
fn export_copies_and_saves() {
    let dir = std::env::temp_dir().join(format!("rush-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let mut m = hosted(120, 40);
    super::asksheets::open_export(&mut m, "k", &d, "");
    assert_eq!(m.kept::<Fake>().asks[0].1["subtype"], "export_conversation");
    assert!(
        m.sheet_as::<super::asksheets::ExportSheet>()
            .unwrap()
            .loading
    );
    reply(
        &mut m,
        Ok(serde_json::json!({"text": "one\ntwo", "default_filename": "c.txt"})),
    );
    press(&mut m, "enter");
    assert_eq!(m.pending_copy.as_deref(), Some("one\ntwo"));
    assert!(m.sheet.is_none());
    super::asksheets::open_export(&mut m, "k", &d, "");
    reply(
        &mut m,
        Ok(serde_json::json!({"text": "one\ntwo", "default_filename": "c.txt"})),
    );
    press(&mut m, "down");
    press(&mut m, "enter");
    assert!(m.sheet.is_none(), "saved, it closes");
    assert_eq!(
        std::fs::read_to_string(dir.join("c.txt")).unwrap(),
        "one\ntwo"
    );
    // Given a name it saves straight away; an empty conversation says so.
    super::asksheets::open_export(&mut m, "k", &d, "named.txt");
    reply(&mut m, Ok(serde_json::json!({"text": "x"})));
    assert_eq!(std::fs::read_to_string(dir.join("named.txt")).unwrap(), "x");
    super::asksheets::open_export(&mut m, "k", &d, "");
    reply(&mut m, Ok(serde_json::json!({"text": "  "})));
    assert_eq!(
        m.sheet_as::<super::asksheets::ExportSheet>().unwrap().err,
        "there's nothing in the conversation yet"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// shift+x on something picked waits on a message; enter stops it and
/// sends the box, esc gives the pick back (stopnote.go).
#[test]
fn stop_with_a_message() {
    let mut m = hosted(120, 40);
    m.session.as_mut().unwrap().sel = "job:b1".into();
    press(&mut m, "X");
    assert!(
        conn(&mut m, "k").stop_note.is_none(),
        "nothing stoppable: not ours"
    );
    // Not ours, the X went on to the box, which took the keys from the
    // pick, as Go's paneKey sends it on (jobs_test.go:414 asks the hook
    // alone): pick it again.
    m.session.as_mut().unwrap().sel = "job:b1".into();
    set_info(&mut m, |i| i.stops = (true, false));
    press(&mut m, "X");
    assert_eq!(
        conn(&mut m, "k").stop_note,
        Some(StopNote {
            key: "x".into(),
            sel: "job:b1".into()
        })
    );
    assert_eq!(m.session.as_ref().unwrap().sel, "", "the box has the keys");
    press(&mut m, "esc");
    assert!(conn(&mut m, "k").stop_note.is_none());
    assert_eq!(m.session.as_ref().unwrap().sel, "job:b1");
    press(&mut m, "X");
    set_info(&mut m, |i| i.input = "wrong approach".into());
    press(&mut m, "enter");
    assert_eq!(
        acts(&mut m),
        vec![Act::Stop {
            key: "x".into(),
            sel: "job:b1".into(),
            send: true
        }]
    );
}

/// /btw: asked with the answered ones as its history; the answer lands in
/// the thread; esc tucks it away, ctrl+d closes it (btwpanel.go).
#[test]
fn btw_side_questions() {
    let mut m = hosted(120, 40);
    press(&mut m, "ctrl+b");
    assert!(btw::btw_for(&mut m, "k").unwrap().focused);
    type_in(&mut m, "why?");
    press(&mut m, "enter");
    let req = m.kept::<Fake>().asks[0].1.clone();
    assert_eq!(req["subtype"], "side_question");
    assert_eq!(req["question"], "why?");
    assert_eq!(req["history"], serde_json::json!([]));
    type_in(&mut m, "and?");
    press(&mut m, "enter");
    assert_eq!(m.kept::<Fake>().asks.len(), 1, "one question at a time");
    reply(&mut m, Ok(serde_json::json!({"response": "because"})));
    let t = btw::btw_for(&mut m, "k").unwrap();
    assert_eq!(
        t.qa,
        vec![Qa {
            question: "why?".into(),
            response: "because".into()
        }]
    );
    assert_eq!(t.waiting, 0);
    press(&mut m, "enter");
    assert_eq!(
        m.kept::<Fake>().asks[1].1["history"],
        serde_json::json!([{"question": "why?", "response": "because"}])
    );
    reply(&mut m, Ok(serde_json::json!({"response": "more"})));
    press(&mut m, "ctrl+s");
    let input = m.kept::<Fake>().info["k"].input.clone();
    assert_eq!(
        input,
        "About my side question (\"and?\"), you said:\n\nmore\n\n"
    );
    assert!(!btw::btw_for(&mut m, "k").unwrap().focused);
    press(&mut m, "ctrl+b");
    press(&mut m, "esc");
    assert!(
        m.kept::<Btws>().threads.contains_key("k"),
        "tucked away, it stays"
    );
    press(&mut m, "ctrl+b");
    press(&mut m, "ctrl+d");
    assert!(!m.kept::<Btws>().threads.contains_key("k"));
    // Drawn over the conversation's top right.
    press(&mut m, "ctrl+b");
    let screen = text(&m.frame());
    assert!(
        screen.contains("btw  side question · not in the conversation"),
        "{screen}"
    );
}

/// The message sheet's keys (messagesheet.go:189).
#[test]
fn message_sheet_keys() {
    let mut s = MessageSheet::new(
        vec![
            UserMessage {
                r#ref: "t1".into(),
                text: "one".into(),
                images: vec![],
            },
            UserMessage {
                r#ref: "t2".into(),
                text: "two".into(),
                images: vec!["/a.png".into(), "/b.png".into()],
            },
        ],
        0,
    );
    let mut m = hosted(120, 40);
    // key, then (message, tab).
    let table: &[(&str, (usize, usize))] = &[
        ("]", (1, 0)),
        ("tab", (1, 1)),
        ("tab", (1, 2)),
        ("tab", (1, 0)),
        ("shift+tab", (1, 2)),
        ("right", (1, 1)),
        ("left", (1, 2)),
        ("[", (0, 0)),
        ("ctrl+end", (1, 0)),
        ("ctrl+home", (0, 0)),
    ];
    for (k, want) in table {
        s.key(&mut m, &crate::frame::Press::of(k).unwrap());
        assert_eq!((s.at, s.tab), *want, "after {k}");
    }
    s.key(&mut m, &crate::frame::Press::of("c").unwrap());
    assert_eq!(m.pending_copy.as_deref(), Some("one"));
    assert_eq!(
        expanded_message("look <pasted_content id=\"1\">\na\nb\n</pasted_content id=\"1\"> ok"),
        "look \na\nb\n ok"
    );
    assert_eq!(hard_wrap("abcdef\ngh", 4), vec!["abcd", "ef", "gh"]);
    assert_eq!(
        s.key(&mut m, &crate::frame::Press::of("esc").unwrap()),
        Out::Close
    );
}

/// The fork sheet's rows and what ←→ change on each (forksheet.go:160).
#[test]
fn fork_sheet_keys() {
    let mut m = hosted(120, 40);
    say(&mut m, "one");
    apply(&mut m, rush_core::event::Event::TurnEnd(Default::default()));
    say(&mut m, "two");
    set_info(&mut m, |i| {
        i.session_id = "s1".into();
        i.name = "fixer".into();
        i.cwd = "/src/app".into();
    });
    super::fork::open_fork(&mut m, "k", "");
    let f = m.sheet_as::<super::fork::ForkSheet>().unwrap();
    assert_eq!(f.name.iter().collect::<String>(), "fixer (fork)");
    assert_eq!(f.turns.len(), 1, "no brancher: only the whole of it");
    press(&mut m, "down"); // Remembers
    press(&mut m, "right");
    assert_eq!(m.sheet_as::<super::fork::ForkSheet>().unwrap().up_to, 0);
    for _ in 0..5 {
        press(&mut m, "down");
    } // First message
    type_in(&mut m, "hi");
    assert_eq!(
        m.sheet_as::<super::fork::ForkSheet>()
            .unwrap()
            .first
            .iter()
            .collect::<String>(),
        "hi"
    );
    press(&mut m, "enter");
    assert!(m.sheet.is_none());
    match acts(&mut m).pop() {
        Some(Act::Fork(f)) => {
            assert_eq!(
                (
                    f.name.as_str(),
                    f.session_id.as_str(),
                    f.fork,
                    f.prompt.as_str()
                ),
                ("fixer (fork)", "s1", true, "hi")
            );
        }
        a => panic!("no fork: {a:?}"),
    }
}

/// The rewind sheet's keys (rewindsheet.go:190).
#[test]
fn rewind_sheet_keys() {
    let m0 = hosted(120, 40);
    let mut s = super::rewind::RewindSheet {
        agent_key: "k".into(),
        agent: "fixer".into(),
        kind: "nobody".into(),
        acct: Default::default(),
        sid: String::new(),
        cwd: String::new(),
        path: String::new(),
        model: String::new(),
        turns: vec![
            super::rewind::TurnStart {
                n: 2,
                prompt: "b".into(),
                ..Default::default()
            },
            super::rewind::TurnStart {
                n: 1,
                prompt: "a".into(),
                ..Default::default()
            },
        ],
        branches: vec![super::rewind::Branch {
            session_id: "b1".into(),
            ..Default::default()
        }],
        sel: 0,
        put_back: false,
        recap: false,
        previews: HashMap::new(),
    };
    drop(m0);
    let mut m = hosted(120, 40);
    let table: &[(&str, usize, bool, bool)] = &[
        ("down", 1, false, false),
        ("c", 1, true, false),
        ("n", 1, true, true),
        ("down", 2, true, true),
        ("c", 2, true, true),
        ("down", 0, true, true),
        ("G", 2, true, true),
        ("g", 0, true, true),
        ("up", 2, true, true),
    ];
    for (k, sel, put, recap) in table {
        s.key(&mut m, &crate::frame::Press::of(k).unwrap());
        assert_eq!(
            (s.sel, s.put_back, s.recap),
            (*sel, *put, *recap),
            "after {k}"
        );
    }
    // On a branch, enter switches to it.
    assert_eq!(
        s.key(&mut m, &crate::frame::Press::of("enter").unwrap()),
        Out::Close
    );
    match acts(&mut m).pop() {
        Some(Act::Rewind(r)) => assert_eq!((r.session_id.as_str(), r.resume), ("b1", true)),
        a => panic!("no rewind: {a:?}"),
    }
}

/// handoffQuiet and awaitCarried (handoff_ready_test.go).
#[test]
fn handoff_ready() {
    assert!(handoff_quiet("idle", 0, "", 0, false));
    assert!(handoff_quiet("stopped", 0, "", 0, false));
    assert!(!handoff_quiet("working", 0, "", 0, false));
    assert!(!handoff_quiet("idle", 1, "", 0, false));
    assert!(!handoff_quiet("idle", 0, "approval", 0, false));
    assert!(!handoff_quiet("idle", 0, "", 1, false));
    assert!(!handoff_quiet("idle", 0, "", 0, true));
    assert_eq!(await_carried(|| Ok(true), Duration::from_secs(1)), Ok(()));
    assert_eq!(
        await_carried(|| Err("destination stopped".into()), Duration::from_secs(1)),
        Err("destination stopped".into())
    );
    assert_eq!(
        await_carried(|| Ok(false), Duration::ZERO),
        Err("initialization timed out".into())
    );
}

/// A verdict waiting on the chat: e puts it in the box, esc drops it,
/// enter sends it (discuss.go:272).
#[test]
fn verdict_keys() {
    for (k, want_input, want_act) in [
        ("e", "The room's verdict on which cache:\nuse redis", None),
        ("esc", "", None),
        (
            "enter",
            "",
            Some(Act::Send(
                "The room's verdict on which cache:\nuse redis".into(),
            )),
        ),
    ] {
        let mut m = hosted(120, 40);
        super::discuss::verdicts_back(
            &mut m,
            vec![rush_room::room::Summary {
                room: rush_room::room::Room {
                    id: "r".into(),
                    from: "k".into(),
                    topic: "which cache".into(),
                    ..Default::default()
                },
                verdict: true,
                final_: "use redis".into(),
                ..Default::default()
            }],
        );
        assert!(text(&m.frame()).contains("⇶ the room on this chat has a verdict"));
        press(&mut m, k);
        assert_eq!(m.kept::<Fake>().info["k"].input, want_input, "{k}");
        assert_eq!(acts(&mut m).pop(), want_act, "{k}");
        assert!(
            m.kept::<super::discuss::Verdicts>().waiting.is_empty(),
            "{k}"
        );
    }
}

/// ctrl+home and ctrl+end pick your first and latest message, alt+enter
/// opens it, and the draft stays (TestMessageNavigationKeepsDraft).
#[test]
fn message_navigation_keeps_draft() {
    let mut m = hosted(100, 30);
    say(&mut m, "first");
    say(&mut m, "steering");
    set_info(&mut m, |i| i.input = "unfinished draft".into());
    press(&mut m, "ctrl+home");
    assert_eq!(m.session.as_ref().unwrap().sel, "t1");
    press(&mut m, "ctrl+end");
    assert_eq!(m.session.as_ref().unwrap().sel, "t1:u:0");
    press(&mut m, "alt+enter");
    let s = m.sheet_as::<MessageSheet>().expect("opened the message");
    assert_eq!(s.at, 1);
    press(&mut m, "[");
    let mut s = m.sheet.take().unwrap();
    let got = super::tests::text(&s.body(&m, 80, 20));
    assert!(got.contains("first"), "{got}");
    m.sheet = Some(s);
    press(&mut m, "esc");
    assert!(m.sheet.is_none());
    assert_eq!(info(&m, "k").unwrap().input, "unfinished draft");
    // Enter on a message picked opens it too.
    m.session.as_mut().unwrap().sel = "t1:u:0".into();
    press(&mut m, "enter");
    assert_eq!(m.sheet_as::<MessageSheet>().map(|s| s.at), Some(1));
}

/// A paste shows whole, and the end of it scrolls into view
/// (TestMessageViewerFullPasteAndScroll).
#[test]
fn message_viewer_full_paste_and_scroll() {
    let mut m = hosted(100, 30);
    let text = format!(
        "before\n<pasted_content>\n{}last pasted line\n</pasted_content>\nafter",
        "line\n".repeat(100)
    );
    say(&mut m, &text);
    open_user_message(&mut m, "k", "");
    let mut s = m.sheet.take().unwrap();
    s.body(&m, 80, 20);
    s.key(&mut m, &crate::frame::Press::of("end").unwrap());
    let got = super::tests::text(&s.body(&m, 80, 20));
    assert!(
        got.contains("last pasted line") && got.contains("after"),
        "{got}"
    );
    assert!(!got.contains("pasted_content"), "the paste stayed folded");
}

/// An image that can't be read says so (TestMessageViewerImageRead), and
/// a chip's link opens the sheet on its image
/// (TestAttachmentLinkSelectsImage).
#[test]
fn message_viewer_images() {
    let mut m = hosted(100, 30);
    m.session.as_mut().unwrap().sess.sent(
        "see [Image #1]".into(),
        vec!["/no/such/image.png".into()],
        1_000,
    );
    message_link(&mut m, "k", "rush:message/t1/image/1");
    let s = m
        .sheet_as::<MessageSheet>()
        .expect("the chip opened the sheet");
    assert_eq!(s.tab, 1);
    let mut s = m.sheet.take().unwrap();
    let got = super::tests::text(&s.body(&m, 90, 24));
    assert!(got.contains("unavailable"), "{got}");
}

/// The rewind sheet lists your messages newest first, then the paths you
/// left; it says what's forgotten, and the code and note are yours to
/// pick (TestRewindSheet).
#[test]
fn rewind_sheet_words() {
    use super::rewind::{Branch, FilesPreview, RewindSheet, TurnStart};
    let mut m = hosted(140, 44);
    let ts = |n: u32, p: &str, u: &str| TurnStart {
        n,
        prompt: p.into(),
        uuid: u.into(),
        ..TurnStart::default()
    };
    let mut f = RewindSheet {
        agent_key: "k".into(),
        agent: "fixer".into(),
        kind: "claude".into(),
        acct: Default::default(),
        sid: String::new(),
        cwd: String::new(),
        path: String::new(),
        model: String::new(),
        turns: vec![
            ts(3, "try caching it", "u3"),
            ts(2, "why is it slow", "u2"),
            ts(1, "fix the bug", "u1"),
        ],
        branches: vec![Branch {
            session_id: "b1".into(),
            from: 2,
            turns: 4,
            last: "rewrite the parser".into(),
            left: now(&m) - 3_600_000,
        }],
        sel: 0,
        put_back: false,
        recap: false,
        previews: HashMap::new(),
    };
    let text = |f: &mut RewindSheet, m: &Model| super::tests::text(&f.body(m, 120, 40));
    let press = |f: &mut RewindSheet, m: &mut Model, k: &str| {
        f.key(m, &crate::frame::Press::of(k).unwrap())
    };
    let got = text(&mut f, &m);
    for want in [
        "turn 3 · the last",
        "why is it slow",
        "rewrite the parser",
        "It forgets turn 3.",
        "keep it as it is now",
        "nothing",
    ] {
        assert!(got.contains(want), "missing {want:?} in:\n{got}");
    }
    press(&mut f, &mut m, "down");
    let got = text(&mut f, &m);
    assert!(
        got.contains("It forgets turns 2–3 (2 of your messages)"),
        "{got}"
    );
    press(&mut f, &mut m, "n");
    f.previews.insert(
        "u2".into(),
        FilesPreview {
            loading: false,
            r: rush_core::adapter::FileRewind {
                can_rewind: true,
                files: vec!["/x/cache.go".into()],
                insertions: 12,
                deletions: 30,
                ..Default::default()
            },
            err: String::new(),
        },
    );
    press(&mut f, &mut m, "c");
    let got = text(&mut f, &m);
    assert!(f.put_back && f.recap, "code and note picked");
    assert!(
        got.contains("put back as they were before turn 2 · 1 file +12 −30")
            && got.contains("a note of what was learned"),
        "{got}"
    );
    // A path left earlier: only switching, the files stay.
    press(&mut f, &mut m, "end");
    assert!(text(&mut f, &m).contains("Files stay as they are now"));
    press(&mut f, &mut m, "c"); // nothing to put back on a branch
    assert_eq!(press(&mut f, &mut m, "esc"), Out::Close);
}

/// alt+r on a turn in the history rewinds to just after it, alt+f forks
/// remembering up to it; a step of a turn counts as its turn; nothing
/// comes after the last turn; and the fork says whether it starts warm
/// (TestRewindAndForkFromHistory).
#[test]
fn rewind_and_fork_from_history() {
    use super::fork::{ForkSheet, ForkTurn};
    use super::rewind::sel_turn;
    let mut m = hosted(140, 44);
    for p in ["fix the bug", "why is it slow", "try caching it"] {
        say(&mut m, p);
        apply(&mut m, rush_core::event::Event::TurnEnd(Default::default()));
    }
    let v = m.session.as_mut().unwrap();
    v.sel = "t3".into();
    assert_eq!(sel_turn(v), Some(3));
    v.sel = "t2:s:toolu_1".into();
    assert_eq!(sel_turn(v), Some(2), "a step of turn 2 counts as turn 2");
    v.sel = "t9".into();
    assert_eq!(sel_turn(v), None);
    // Typing: alt+r and alt+f are the box's.
    m.session.as_mut().unwrap().sel = "t2".into();
    set_info(&mut m, |i| i.input = "x".into());
    press(&mut m, "alt+f");
    assert!(m.sheet.is_none(), "alt+f mid-message forked");
    set_info(&mut m, |i| i.input.clear());
    // Rewind to the last turn: there's nothing after it.
    m.session.as_mut().unwrap().sel = "t3".into();
    press(&mut m, "alt+r");
    assert!(m.sheet.is_none(), "nothing to rewind after the last turn");

    // Whether the fork starts warm.
    let now = now(&m);
    let ft = |n: u32, p: &str, end: i64| ForkTurn {
        n,
        prompt: p.into(),
        end,
    };
    let mut f = ForkSheet {
        turns: vec![
            ft(3, "try caching it", now - 60_000),
            ft(2, "why is it slow", now - 120_000),
            ft(1, "fix the bug", now - 180_000),
        ],
        up_to: 1,
        models: vec!["".into(), "opus".into(), "haiku".into()],
        efforts: vec!["".into(), "low".into()],
        perms: vec!["".into(), "plan".into()],
        now_model: "claude-opus-4".into(),
        now_effort: "high".into(),
        cache_warm: now + 50 * 60_000,
        ..ForkSheet::default()
    };
    let note = |f: &ForkSheet| f.first_note(now).plain();
    assert!(note(&f).contains("added after the cache"), "{}", note(&f));
    f.model = 2; // haiku, not what it runs
    assert!(note(&f).contains("breaks the cache: another model"));
    f.model = 0;
    f.effort = 1; // low: another effort breaks it too
    assert!(note(&f).contains("breaks the cache: another effort"));
    f.effort = 0;
    f.perm = 1; // plan: permissions keep it
    assert!(note(&f).contains("added after the cache"));
}

/// Text in the /btw panel can be dragged over and copied, as in the
/// chat: on release, or with cmd+c when copy on select is off
/// (TestBtwTextSelects).
#[test]
fn btw_text_selects() {
    use crate::frame::Mouse;
    use rush_draw::{Button, KeyMods, MouseKind};
    let mut m = hosted(100, 30);
    m.kept::<super::tests::Fake>()
        .settings
        .insert("copyOnSelect".into(), "false".into());
    m.kept::<Btws>().threads.insert(
        "k".into(),
        super::btw::Thread {
            focused: true,
            qa: vec![Qa {
                question: "ssh command please".into(),
                response: "Run ssh root@example.test to get in.".into(),
            }],
            ..Default::default()
        },
    );
    let draw = |m: &mut Model| -> (u16, u16) {
        let mut rows = vec![
            rush_core::text::Line::from_spans(vec![rush_core::text::Span::plain(
                "c".repeat(100),
            )]);
            30
        ];
        super::btw::btw_overlay(m, &mut rows);
        for (i, r) in rows.iter().enumerate() {
            let p = r.plain();
            if let Some(at) = p.find("ssh root@") {
                return (rush_core::text::width(&p[..at]) as u16, i as u16);
            }
        }
        panic!("the answer isn't drawn");
    };
    let mouse = |m: &mut Model, kind: MouseKind, x: u16, y: u16| {
        super::btw::btw_mouse(
            m,
            &Mouse {
                kind,
                x,
                y,
                mods: KeyMods::default(),
            },
        )
    };
    let (x, y) = draw(&mut m);
    let end = x + "ssh root@example.test".len() as u16 - 1;
    assert_eq!(
        mouse(&mut m, MouseKind::Down(Button::Left), x, y),
        Out::Draw,
        "missed the panel"
    );
    mouse(&mut m, MouseKind::Drag(Button::Left), end, y);
    mouse(&mut m, MouseKind::Up(Button::Left), end, y);
    let sel = m.kept::<Btws>().threads["k"].sel;
    assert!(
        m.pending_copy.is_none(),
        "the release copied with copy on select off"
    );
    assert!(sel.on && !sel.drag, "the selection was dropped");
    press(&mut m, "super+c");
    assert_eq!(m.pending_copy.as_deref(), Some("ssh root@example.test"));

    // On, the release copies; a click outside the panel drops it.
    m.kept::<super::tests::Fake>()
        .settings
        .remove("copyOnSelect");
    m.pending_copy = None;
    mouse(&mut m, MouseKind::Down(Button::Left), x, y);
    mouse(&mut m, MouseKind::Drag(Button::Left), x + 2, y);
    mouse(&mut m, MouseKind::Up(Button::Left), x + 2, y);
    assert_eq!(m.pending_copy.as_deref(), Some("ssh"));
    mouse(&mut m, MouseKind::Down(Button::Left), 0, y);
    assert!(
        !m.kept::<Btws>().threads["k"].sel.on,
        "a click elsewhere kept it"
    );
}

/// The panel is told where the chat stands: idle since when, and its plan
/// (TestChatStatus).
#[test]
fn chat_status_says_where_it_stands() {
    let now: i64 = 1_790_000_000_000;
    let mut s = rush_convo::Session::new();
    s.sent("go".into(), vec![], now - 4 * 3_600_000);
    s.apply(
        &rush_core::event::Event::TurnEnd(Default::default()),
        now - 3 * 3_600_000,
    );
    let t = s.turns.last_mut().unwrap();
    t.end = now - 3 * 3_600_000;
    t.live = false;
    s.tasks.push(rush_convo::Task {
        subject: "visual diff".into(),
        status: "in_progress".into(),
        ..Default::default()
    });
    let got = super::discuss::chat_status(&s, now);
    assert!(
        got.starts_with("Status: idle since ") && got.contains("(3h0m0s ago)"),
        "{got}"
    );
    assert!(got.contains("Plan:\n- [in_progress] visual diff"), "{got}");
}

/// #intervene goes through the chat with its steps, and its verdict goes
/// in the box (TestInterveneFromChat).
#[test]
fn intervene_from_chat() {
    let mut m = hosted(200, 45);
    say(&mut m, "make the visual diff work");
    apply(&mut m, rush_core::event::Event::TurnEnd(Default::default()));
    super::discuss::intervene(&mut m, "k", "visual diff stalled");
    match acts(&mut m).pop() {
        Some(Act::Room(r)) => {
            assert!(r.fill && r.from == "k", "{r:?}");
            assert!(r.topic.contains("visual diff stalled"), "{}", r.topic);
            assert!(
                super::discuss::brief(&r).contains("[turn "),
                "the brief has no times: {}",
                super::discuss::brief(&r)
            );
        }
        a => panic!("no room: {a:?}"),
    }
}

/// A verdict comes back once, and a room on the chat shows above its box
/// while it runs, another chat's never (TestDiscussFromChat,
/// TestChatRoomPanel).
#[test]
fn verdicts_and_rooms_on_the_chat() {
    use rush_room::room::{Member, Room, Summary};
    let mut m = hosted(200, 45);
    let sum = Summary {
        room: Room {
            id: "r1".into(),
            topic: "the cache".into(),
            from: "k".into(),
            ..Default::default()
        },
        verdict: true,
        final_: "Haiku: safe\nLuna: safe".into(),
        ..Default::default()
    };
    super::discuss::verdicts_back(&mut m, vec![sum.clone()]);
    assert_eq!(info(&m, "k").unwrap().input, "", "straight in the box");
    let dock = super::tests::text(&super::discuss::verdict_rows(&m, "k", 200));
    for want in [
        "has a verdict",
        "asked: the cache",
        "Luna: safe",
        "enter send it to this agent",
        "e edit it first",
        "esc drop it",
    ] {
        assert!(dock.contains(want), "no {want:?} in the dock:\n{dock}");
    }
    press(&mut m, "esc");
    super::discuss::verdicts_back(&mut m, vec![sum]);
    assert!(
        m.kept::<super::discuss::Verdicts>().waiting.is_empty(),
        "a verdict came back twice"
    );

    let member = |n: &str| Member {
        name: n.into(),
        ..Default::default()
    };
    let mut last = HashMap::new();
    last.insert("Haiku".to_string(), "it's safe".to_string());
    m.kept::<super::discuss::Verdicts>().rooms = vec![
        Summary {
            room: Room {
                id: "other".into(),
                topic: "elsewhere".into(),
                from: "nobody".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        Summary {
            room: Room {
                id: "r2".into(),
                topic: "the cache".into(),
                from: "k".into(),
                members: vec![member("Haiku"), member("Luna")],
                ..Default::default()
            },
            round: 2,
            speaking: vec!["Luna".into()],
            last,
            ..Default::default()
        },
    ];
    let dock = super::tests::text(&super::discuss::chat_room_rows(&m, "k", 200));
    for want in [
        "room · the cache",
        "round 2/4 · 2 agents",
        "Haiku  it's safe",
        "Luna",
    ] {
        assert!(dock.contains(want), "no {want:?} in the dock:\n{dock}");
    }
    assert!(!dock.contains("elsewhere"), "another chat's room shows");
    m.kept::<super::discuss::Verdicts>().rooms[1].over = "done".into();
    assert!(super::discuss::chat_room_rows(&m, "k", 200).is_empty());
}
