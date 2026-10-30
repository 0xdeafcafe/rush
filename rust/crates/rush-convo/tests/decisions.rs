//! A message the decisions card sent, drawn as what you chose: after
//! internal/convo/decisions_test.go.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::plain;
use rush_convo::{Options, Renderer, Session};

mod common;

use common::*;

const LOADER_ANSWER: &str = "The lane is done.\n\nDecisions:\n\
    1. **Committing the loader work:** rush's permission check stopped the lane's commit, so this needs your go-ahead. (a) yes: I commit it on perf/loader as one commit per fix, nothing pushed; (b) no: leave it uncommitted for you to look at first.\n\
    My picks: 1a.";

fn decisions_session(text: &str) -> Session {
    let mut s = Session::new();
    s.feed(sent("go"), at(0));
    s.feed(say(text), at(1));
    s.feed(done(0.0), at(2));
    s
}

/// A turn of yours starting with text, and ending.
fn answer(s: &mut Session, text: &str) {
    let n = s.turns.len() as i64;
    s.feed(sent(text), at(10 * n));
    s.feed(say("On it."), at(10 * n + 1));
    s.feed(done(0.0), at(10 * n + 2));
}

fn draw(s: &Session) -> String {
    plain(&Renderer::new().render(
        s,
        &Options {
            width: 100,
            ..Options::default()
        },
    ))
}

// What the card sends is drawn as what you chose, a decision a row: its
// topic, then a tick and the option, or a pencil and your own words, the
// topic of those found in the block it answers
// (TestDecisionAnswerDrawnAsChoices).
#[test]
fn decision_answer_drawn_as_choices() {
    let mut s = decisions_session(LOADER_ANSWER);
    answer(&mut s, "1a. Committing the loader work: yes");
    let got = draw(&s);
    for want in ["  · your decisions", "1. Committing the loader work  ✓ yes"] {
        assert!(got.contains(want), "missing {want:?} in\n{got}");
    }
    let mut s = decisions_session(LOADER_ANSWER);
    answer(&mut s, "1: only the first fix, please");
    let got = draw(&s);
    assert!(
        got.contains("1. Committing the loader work  ✎ only the first fix, please"),
        "your own words, under its topic:\n{got}"
    );
    answer(&mut s, "1a 2b please");
    let got = draw(&s);
    assert!(
        got.contains("1a 2b please") && got.matches("your decisions").count() == 1,
        "shorthand stays as written:\n{got}"
    );
}
