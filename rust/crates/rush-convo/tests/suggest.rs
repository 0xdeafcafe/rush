//! Which view fits right now (suggest.rs): the counts read off a session's
//! last turn, the rules that pick a view from them, and the question put
//! to a quick model, which never carries the conversation's words.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::Preset;
use rush_convo::Session;
use rush_convo::suggest::{Digest, Moment, answer, digest, prompt, rules};
use serde_json::json;

mod common;

use common::*;

/// One finished turn: you ask, it edits two files, runs the tests (which
/// fail once, then pass) and answers. Every word in it is one a digest
/// must not carry.
fn coded() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/zebra".into();
    let evs = vec![
        (0, sent("please fix the flamingo parser")),
        (2, say("Looking at the flamingo parser now.")),
        (
            3,
            tool_use(
                "e1",
                "Edit",
                json!({"file_path": "/work/zebra/src/flamingo.rs"}),
            ),
        ),
        (5, tool_result("e1", "ok", false, None)),
        (
            6,
            tool_use(
                "e2",
                "Write",
                json!({"file_path": "/work/zebra/src/pelican.rs"}),
            ),
        ),
        (8, tool_result("e2", "ok", false, None)),
        (
            9,
            tool_use(
                "b1",
                "Bash",
                json!({"command": "cargo test -p walrus-secret"}),
            ),
        ),
        (
            40,
            tool_result("b1", "Exit code 101\ntest walrus failed", true, None),
        ),
        (
            41,
            tool_use(
                "b2",
                "Bash",
                json!({"command": "cargo test -p walrus-secret"}),
            ),
        ),
        (
            70,
            tool_result("b2", "test result: ok. 3 passed", false, None),
        ),
        (72, say("Fixed the flamingo parser; the walrus tests pass.")),
        (73, done(0.1)),
    ];
    for (sec, e) in evs {
        s.feed(e, at(sec));
    }
    s
}

/// The digest counts the last finished turn: edits, files, steps, checks
/// with their last outcome, words, and its time split.
#[test]
fn the_digest_counts_the_last_turn() {
    let s = coded();
    let d = digest(&s, 0, at(100)).unwrap();
    assert_eq!(d.edits, 2);
    assert_eq!(d.files, 2);
    assert_eq!(d.steps, 4);
    assert_eq!(d.checks, vec![("tests", true)]);
    assert!(d.words > 5, "{d:?}");
    assert_eq!(d.turn, 73_000);
    assert_eq!(d.tools, (2 + 2 + 31 + 29) * 1000);
    assert_eq!(d.thinking, d.turn - d.tools);
    assert_eq!((d.away, d.unseen), (0, 0), "never looked");
    let d = digest(&s, at(1), at(61)).unwrap();
    assert_eq!((d.away, d.unseen), (60_000, 1));
    assert!(digest(&Session::new(), 0, at(1)).is_none());
}

/// What the quick model is asked holds counts and view names only: no
/// word you or the agent said, no path, no command.
#[test]
fn the_question_has_no_conversation_text() {
    let s = coded();
    let d = digest(&s, at(1), at(700)).unwrap();
    let q = prompt(
        &d,
        Moment::Back,
        Preset::Chat,
        &[Preset::Code, Preset::Trail],
    );
    for w in [
        "flamingo", "pelican", "walrus", "zebra", "please", "Fixed", "cargo", "/work",
    ] {
        assert!(!q.contains(w), "{w} in {q}");
    }
    assert!(q.contains(r#""viewInUse":"chat""#), "{q}");
    assert!(q.contains(r#""recentViews":["code","trail"]"#), "{q}");
    assert!(q.contains(r#""filesChanged":2"#), "{q}");
    assert!(q.contains(r#""kind":"tests""#), "{q}");
}

fn dg(f: impl FnOnce(&mut Digest)) -> Digest {
    let mut d = Digest::default();
    f(&mut d);
    d
}

/// Each rule, in order, and when none can tell.
#[test]
fn the_rules_table() {
    let cases: Vec<(&str, Digest, Moment, Option<(Preset, &str)>)> = vec![
        (
            "back to finished turns",
            dg(|d| {
                d.unseen = 3;
                d.files = 2;
                d.checks = vec![("tests", true)];
            }),
            Moment::Back,
            Some((Preset::CatchUp, "3 turns finished while you were away")),
        ),
        (
            "unseen turns don't count when a turn just ended",
            dg(|d| {
                d.unseen = 3;
                d.files = 4;
                d.checks = vec![("tests", true)];
            }),
            Moment::Ended,
            Some((Preset::Code, "changed 4 files, ran tests")),
        ),
        (
            "changed and checked",
            dg(|d| {
                d.files = 1;
                d.checks = vec![("clippy", false), ("tests", true)];
            }),
            Moment::Ended,
            Some((
                Preset::Code,
                "changed 1 file, ran clippy, tests (clippy failed)",
            )),
        ),
        (
            "a long turn mostly in tools",
            dg(|d| {
                d.turn = 6 * 60_000;
                d.tools = 4 * 60_000;
                d.steps = 20;
            }),
            Moment::Ended,
            Some((Preset::Trail, "took 6 min, 4 of them in tools")),
        ),
        (
            "mostly talk",
            dg(|d| {
                d.words = 300;
                d.steps = 1;
                d.turn = 20_000;
            }),
            Moment::Back,
            Some((Preset::Chat, "mostly talk, 1 step")),
        ),
        (
            "talk with no steps",
            dg(|d| {
                d.words = 40;
            }),
            Moment::Ended,
            Some((Preset::Chat, "mostly talk, no steps")),
        ),
        (
            "a short look around",
            dg(|d| {
                d.words = 40;
                d.steps = 6;
                d.turn = 30_000;
            }),
            Moment::Ended,
            Some((Preset::Normal, "6 steps, nothing changed")),
        ),
        (
            "changed without checks: unclear",
            dg(|d| {
                d.files = 2;
                d.steps = 5;
                d.words = 50;
            }),
            Moment::Ended,
            None,
        ),
        (
            "a long turn mostly thinking: unclear",
            dg(|d| {
                d.turn = 5 * 60_000;
                d.tools = 60_000;
                d.steps = 8;
            }),
            Moment::Ended,
            None,
        ),
    ];
    for (name, d, moment, want) in cases {
        let got = rules(&d, moment);
        let want = want.map(|(p, w)| (p, w.to_string()));
        assert_eq!(got, want, "{name}");
    }
}

/// The quick model's answer: the view by name or title, its reason one
/// short line; anything else is no answer.
#[test]
fn the_answer_is_read_or_dropped() {
    assert_eq!(
        answer(br#"{"view":"trail","reason":"long\nturn"}"#),
        Some((Preset::Trail, "long turn".into()))
    );
    assert_eq!(
        answer(br#"{"view":"Catch-up","reason":""}"#),
        Some((Preset::CatchUp, String::new()))
    );
    assert_eq!(answer(br#"{"view":"diff","reason":"x"}"#), None);
    assert_eq!(answer(b"not json"), None);
    let long = format!(r#"{{"view":"code","reason":"{}"}}"#, "a".repeat(100));
    let (_, r) = answer(long.as_bytes()).unwrap();
    assert_eq!(r.chars().count(), 61);
}
