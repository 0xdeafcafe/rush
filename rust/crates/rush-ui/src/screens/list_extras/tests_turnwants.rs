//! What a finished turn wants of you: Go's turnwants_test.go as of
//! 3c9343b1 (TestParseTurnWants, TestFinishedTurnsThatWantYouNeedYou,
//! TestTurnWantsInConversationAndWall).

use rush_core::agent::{Agent, State};
use rush_state::Time;

use super::tests::{MIN, NOW, agent, model_of};
use super::turnwants::*;
use super::wall::Wall;
use crate::list::{FINISHED, NEEDS};
use crate::model::Model;

/// A model's one line becomes what the turn wants; anything else is no
/// answer, so the agent stays where it was.
#[test]
fn parse() {
    for (input, wants, line) in [
        ("OFFER: check it in rush", WANTS_OFFER, "check it in rush"),
        ("**YOU:** approve PR #41.", WANTS_YOU, "approve PR #41"),
        (
            "DONE: merged into main\nmore text",
            WANTS_DONE,
            "merged into main",
        ),
        (
            "problem: tests fail on main",
            WANTS_PROBLEM,
            "tests fail on main",
        ),
    ] {
        let got = parse_turn_wants(input).unwrap_or_else(|| panic!("{input:?}"));
        assert_eq!(
            (got.wants.as_str(), got.line.as_str()),
            (wants, line),
            "{input:?}"
        );
    }
    for input in ["", "I think it's done", "MAYBE: who knows"] {
        assert!(
            parse_turn_wants(input).is_none(),
            "{input:?} taken as an answer"
        );
    }
}

fn done_agent(key: &str, ago: i64) -> Agent {
    let mut a = agent(key, key, State::Done, 7);
    a.interactive = false;
    a.root = "/src/rush".into();
    a.branch = format!("rush/{key}");
    a.job.updated_at = NOW - ago;
    a
}

fn wants(at: i64, w: &str, line: &str) -> TurnWants {
    TurnWants {
        at: Time::from_ms(at),
        wants: w.into(),
        line: line.into(),
    }
}

fn set(m: &mut Model, got: &[(&str, TurnWants)]) {
    let t = m.kept::<Turns>();
    for (k, w) in got {
        t.got.insert(format!("default/{k}"), w.clone());
    }
    m.rebuild();
}

fn where_of(m: &Model, key: &str) -> String {
    let i = m.agents.iter().position(|a| a.job.id == key).unwrap();
    m.agent_where(&m.agents[i], i, 80).plain()
}

/// A finished turn whose last message offers, asks, hands you a step or
/// says it couldn't goes under Needs you, the reason on its second line;
/// one that's done, not read yet, or read for an older turn, stays under
/// Finished.
#[test]
fn finished_turns_that_want_you_need_you() {
    let ago = 5 * MIN;
    let at = NOW - ago;
    let names = ["offer", "done", "unread", "older"];
    let mut m = model_of(names.iter().map(|k| done_agent(k, ago)).collect(), 120, 40);
    set(
        &mut m,
        &[
            ("offer", wants(at, WANTS_OFFER, "check it in rush")),
            ("done", wants(at, WANTS_DONE, "merged into main")),
            ("older", wants(at - 60 * MIN, WANTS_ASK, "which theme")),
        ],
    );
    for (k, g) in [
        ("offer", NEEDS),
        ("done", FINISHED),
        ("unread", FINISHED),
        ("older", FINISHED),
    ] {
        assert_eq!(m.group_of[&format!("default/{k}")], g, "{k}");
    }
    let got = where_of(&m, "offer");
    assert!(got.contains("offers: check it in rush"), "{got:?}");
    let got = where_of(&m, "done");
    assert!(got.contains("✓ merged into main"), "{got:?}");
    // Looked at: it asks nothing more.
    m.agents[0].seen = true;
    m.rebuild();
    assert_ne!(m.group_of["default/offer"], NEEDS);
    assert!(turn_label(&m, &m.agents[0]).is_none());
}

/// Opened, a turn that wants you says so above the box till you answer,
/// even once looked at; one that's done says nothing there. Its Wall tile
/// says it too, until it's looked at.
#[test]
fn turn_wants_in_conversation_and_wall() {
    let at = NOW - MIN;
    let mut m = model_of(vec![done_agent("k", MIN)], 120, 40);
    set(&mut m, &[("k", wants(at, WANTS_YOU, "approve PR #52"))]);
    let tile: Vec<String> = Wall::default()
        .tile(&m, 0, 60, 8, false)
        .iter()
        .map(|l| l.plain())
        .collect();
    let tile = tile.join("\n");
    assert!(
        tile.contains("needs you · wants you to: approve PR #52"),
        "{tile}"
    );
    m.agents[0].seen = true;
    let l = turn_wants_line(&m, &m.agents[0], 80).map(|l| l.plain());
    assert!(
        l.as_deref()
            .is_some_and(|l| l.contains("! wants you to: approve PR #52")),
        "{l:?}"
    );
    set(&mut m, &[("k", wants(at, WANTS_DONE, "merged into main"))]);
    assert!(
        turn_wants_line(&m, &m.agents[0], 80).is_none(),
        "a done turn"
    );
    // You answered: a new turn, not read yet.
    m.agents[0].job.updated_at = NOW;
    set(&mut m, &[("k", wants(at, WANTS_YOU, "approve PR #52"))]);
    assert!(
        turn_wants_line(&m, &m.agents[0], 80).is_none(),
        "an answered turn"
    );
}

/// turns.json is Go's: what another rush wrote reads back the same.
#[test]
fn turns_file_is_gos() {
    let go = r#"{"k":{"at":"2026-10-28T22:10:31.123456789+01:00","wants":"you","line":"approve PR #41"},"d":{"at":"2026-10-28T22:10:31Z","wants":"done"}}"#;
    let got: std::collections::HashMap<String, TurnWants> = serde_json::from_str(go).unwrap();
    assert_eq!(got["k"].wants, WANTS_YOU);
    assert_eq!(got["k"].line, "approve PR #41");
    let back = serde_json::to_string(&got["d"]).unwrap();
    assert_eq!(back, r#"{"at":"2026-10-28T22:10:31Z","wants":"done"}"#);
}
