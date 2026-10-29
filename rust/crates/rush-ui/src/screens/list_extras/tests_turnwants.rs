//! What a finished turn wants of you, on the list's side: Go's
//! turnwants_test.go (TestFinishedTurnsThatWantYouNeedYou,
//! TestTurnWantsOnWall). Parsing, the model and the cache are tested
//! with session_core's turnwants.rs.

use rush_core::agent::{Agent, State};

use super::tests::{MIN, NOW, agent, model_of};
use super::wall::Wall;
use crate::list::{FINISHED, NEEDS};
use crate::model::Model;
use crate::screens::session_core::turnwants::*;

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
        at,
        wants: w.into(),
        line: line.into(),
        ..TurnWants::default()
    }
}

fn set(m: &mut Model, got: &[(&str, TurnWants)]) {
    let t = m.kept::<TurnState>();
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
    assert!(got.contains("check it in rush"), "{got:?}");
    let got = where_of(&m, "done");
    assert!(got.contains("✓ merged into main"), "{got:?}");
    // Looked at: it asks nothing more.
    m.agents[0].seen = true;
    m.rebuild();
    assert_ne!(m.group_of["default/offer"], NEEDS);
    assert!(turn_label(&m, &m.agents[0]).is_empty());
}

/// A turn that wants you says what on its Wall tile, in plain words.
#[test]
fn turn_wants_on_wall() {
    let at = NOW - MIN;
    let mut m = model_of(vec![done_agent("k", MIN)], 120, 40);
    set(&mut m, &[("k", wants(at, WANTS_YOU, "approve PR #52"))]);
    let tile: Vec<String> = Wall::default()
        .tile(&m, 0, 60, 8, false)
        .iter()
        .map(|l| l.plain())
        .collect();
    let tile = tile.join("\n");
    assert!(tile.contains("needs you · approve PR #52"), "{tile}");
}
