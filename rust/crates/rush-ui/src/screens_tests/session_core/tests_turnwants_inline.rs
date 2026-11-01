//! rush-ui-session-core's turnwants.rs tests: they make a whole UI model.

use super::turnwants::*;
use crate::model::Model;
use rush_convo::render::NeedsItem;
use rush_core::agent::Agent;
use rush_core::time::parse_rfc3339;
use std::collections::HashMap;

use crate::style::RED;

fn tw(wants: &str, line: &str, quote: &str, ui: &[(&str, &str)]) -> TurnWants {
    TurnWants {
        wants: wants.into(),
        line: line.into(),
        quote: quote.into(),
        ui: ui
            .iter()
            .map(|(k, t)| NeedsItem {
                kind: (*k).into(),
                text: (*t).into(),
            })
            .collect(),
        ..TurnWants::default()
    }
}

/// A model's one line becomes what the turn wants; anything else is no
/// answer, so the agent stays where it was (TestParseTurnWants).
#[test]
fn parse_turn_wants_reads_the_model() {
    for (input, want) in [
        (
            "OFFER: check it in rush",
            tw("offer", "check it in rush", "", &[]),
        ),
        (
            "**YOU:** approve PR #41.",
            tw("you", "approve PR #41", "", &[]),
        ),
        (
            "DONE: merged into main\nmore text",
            tw("done", "merged into main", "", &[]),
        ),
        (
            "problem: tests fail on main",
            tw("problem", "tests fail on main", "", &[]),
        ),
        (
            "YOU: push the merge\nQUOTE: I haven't pushed it",
            tw("you", "push the merge", "I haven't pushed it", &[]),
        ),
        (
            "ASK: which theme\n**QUOTE:** \"Which theme?\"",
            tw("ask", "which theme", "Which theme?", &[]),
        ),
        (
            "DONE: merged\nQUOTE: it's merged",
            tw("done", "merged", "", &[]),
        ),
        (
            "YOU: push the merge\nQUOTE: not pushed\nDO: rebuild rush\nRUN: `git push`\n- REPLY: \"yes, push it\"\nLINK: <https://x.y/1>\nNOTE: tests pass\nPICK: no",
            tw(
                "you",
                "push the merge",
                "not pushed",
                &[
                    ("do", "rebuild rush"),
                    ("run", "git push"),
                    ("reply", "yes, push it"),
                    ("link", "https://x.y/1"),
                    ("note", "tests pass"),
                ],
            ),
        ),
        ("DONE: merged\nDO: nothing", tw("done", "merged", "", &[])),
    ] {
        assert_eq!(parse_turn_wants(input), Some(want), "{input:?}");
    }
    for input in ["", "I think it's done", "MAYBE: who knows"] {
        assert_eq!(parse_turn_wants(input), None, "{input:?}");
    }
}

/// A short, plain message is read by Haiku; a long or busy one by
/// Sonnet (TestWantsModel).
#[test]
fn long_or_busy_messages_go_to_sonnet() {
    let long = "word ".repeat(400);
    for (text, want) in [
        ("Merged. I haven't pushed it.", "haiku"),
        (long.as_str(), "sonnet"),
        ("Which? This? Or that?", "sonnet"),
        ("Run:\n```\ngit push\n```", "sonnet"),
        ("Options:\n1. a\n2. b\n- c\n- d", "sonnet"),
        ("Two:\n- a\n- b", "haiku"),
    ] {
        assert_eq!(wants_model(text), want, "{text:.30}");
    }
}

/// What a turn wants in words: the line it said, a word when it said
/// nothing more, cut short for a row; done has its own mark.
#[test]
fn wants_in_words() {
    let t = tw("you", "approve PR #52", "", &[]);
    assert_eq!(wants_say(&t), "approve PR #52");
    assert_eq!(wants_whole(&tw("offer", "", "", &[])), "offers a next step");
    assert_eq!(wants_whole(&tw("done", "Done", "", &[])), "done");
    let long = tw("ask", &"x".repeat(80), "", &[]);
    assert_eq!(wants_say(&long).chars().count(), 56);
    assert!(wants_say(&long).ends_with('…'));
    assert_eq!(wants_mark(&t).text, "!");
    assert_eq!(wants_mark(&tw("problem", "", "", &[])).style.fg, RED);
    assert_eq!(wants_mark(&tw("ask", "", "", &[])).text, "?");
    let done: String = wants_label(&tw("done", "merged into main", "", &[]))
        .iter()
        .map(|s| s.text.as_str())
        .collect();
    assert_eq!(done, "✓ merged into main");
}

/// A finished turn whose last message offers, asks, hands you a step
/// or says it couldn't wants you, and its label says what; one that's
/// done, or not read yet, or read for an older turn, doesn't; looked
/// at, it asks nothing more (TestFinishedTurnsThatWantYouNeedYou's
/// session side).
#[test]
fn finished_turns_that_want_you() {
    let now = 1_800_000_000_000;
    let mut m = crate::new_model("0.1.0");
    m.now = now;
    let agent = |key: &str| {
        let mut a = Agent {
            key: key.into(),
            pid: 7,
            ..Agent::default()
        };
        a.job.state = rush_core::agent::State::Done;
        a.job.updated_at = now - 5 * 60_000;
        a
    };
    let (mut offer, done, unread, older) = (
        agent("offer"),
        agent("done"),
        agent("unread"),
        agent("older"),
    );
    let at = offer.job.updated_at;
    let s = m.kept::<TurnState>();
    s.got.insert(
        "offer".into(),
        TurnWants {
            at,
            ..tw("offer", "check it in rush", "", &[])
        },
    );
    s.got.insert(
        "done".into(),
        TurnWants {
            at,
            ..tw("done", "merged into main", "", &[])
        },
    );
    s.got.insert(
        "older".into(),
        TurnWants {
            at: at - 3_600_000,
            ..tw("ask", "which theme", "", &[])
        },
    );
    assert!(wants_you(&m, &offer));
    for a in [&done, &unread, &older] {
        assert!(!wants_you(&m, a), "{}", a.key);
    }
    let label = |m: &mut Model, a: &Agent| -> String {
        turn_label(m, a).iter().map(|s| s.text.as_str()).collect()
    };
    assert_eq!(label(&mut m, &offer), "check it in rush");
    assert_eq!(label(&mut m, &done), "✓ merged into main");
    offer.seen = true;
    assert!(!wants_you(&m, &offer));
    assert_eq!(label(&mut m, &offer), "");
}

// turnwants.go:298: only the rush that leads asks the model what a
// finished turn wants; one that follows never does, and asks nothing
// of the cache's writer either.
#[test]
fn only_the_rush_that_leads_asks_the_turn_model() {
    let now = parse_rfc3339("2026-10-28T12:00:00Z");
    for follows in [true, false] {
        let mut m = crate::new_model("0.1.0");
        m.follows = follows;
        m.now = now;
        let mut a = Agent {
            key: "k".into(),
            ..Agent::default()
        };
        a.job.state = rush_core::agent::State::Done;
        a.job.updated_at = now - 60_000;
        a.job.cwd = "/work/app".into();
        a.job.transcript_path = "/work/app/t.jsonl".into();
        m.agents = vec![a];
        m.roots = vec!["/work/app".into()];
        state(&mut m).loaded = true;
        let got = turn_to_read(&mut m).map(|a| a.key);
        if follows {
            assert_eq!(got, None, "following asked the model");
            // Nor does the whole of it start a read.
            read_turn(&mut m);
            assert!(!state(&mut m).reading);
        } else {
            assert_eq!(got.as_deref(), Some("k"));
        }
    }
}

/// The cache keeps Go's form: times as RFC 3339, empty fields left out.
#[test]
fn cache_reads_as_go_writes_it() {
    let j = r#"{"k":{"at":"2026-10-28T12:34:56.789Z","wants":"you","line":"approve PR #41","ui":[{"kind":"run","text":"git push"}]}}"#;
    let got: HashMap<String, TurnWants> = serde_json::from_str(j).unwrap();
    let t = &got["k"];
    assert_eq!(t.at, parse_rfc3339("2026-10-28T12:34:56.789Z"));
    assert_eq!(t.ui[0].text, "git push");
    let back = serde_json::to_string(t).unwrap();
    assert!(
        back.contains(r#""at":"2026-10-28T12:34:56.789Z""#),
        "{back}"
    );
    assert!(!back.contains("quote"), "{back}");
}
