//! A decisions card that comes while you're typing waits under the box
//! (rush-ui-session-cards' hold.rs): the notice and its count, the card
//! once you stop with the box empty, alt+j with your text kept, and
//! alt+shift+j setting it aside until the card is answered or set aside.
//! testdata/hold_*.txt are the box as drawn; UPDATE_GOLDEN=1 writes them.

use super::conn;
use super::tests::{KEY, NOW, hosted, press, screen, typed};
use crate::model::Model;
use crate::screens::session_cards::ModelCards;
use crate::screens::session_cards::hold::HOLD_MS;
use crate::screens::session_cards::question::decisions_of;

const DECIDE: &str = "Decisions:\n\
1. **Scope:** (a) the full recommendation; (b) the easier alternative.\n\
2. **Answer format:** (a) `1a. Topic: label` lines; (b) keep `Q:/A:`.\n\
My picks: 1a 2b.";

/// A hosted session 80 wide, the cards' host linked.
fn model() -> Model {
    let (mut m, _fake) = hosted("steer", Vec::new());
    m.cards().host = Some(super::cards_host::host());
    m.w = 80;
    m
}

/// The agent ends its turn with a decisions block, id id.
fn decide(m: &mut Model, id: &str) {
    conn(m, KEY).info.now.decisions = decisions_of(DECIDE, id);
}

fn at(m: &mut Model, ms: rush_core::time::Ms) {
    m.clock = Some(NOW + ms);
}

fn input(m: &mut Model) -> (String, usize) {
    let c = conn(m, KEY);
    (c.input.string(), c.input.pos)
}

/// The message box's rows as drawn, from its top edge to its bottom.
fn the_box(m: &mut Model) -> String {
    let s = screen(m);
    let top = s.iter().rposition(|l| l.contains('╭')).expect("a box");
    let end = top
        + s[top..]
            .iter()
            .position(|l| l.contains('╰'))
            .expect("its foot");
    s[top..=end]
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// got against testdata/name.txt; UPDATE_GOLDEN=1 writes it.
fn golden(name: &str, got: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/screens_tests/session_core/testdata")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no {}: run with UPDATE_GOLDEN=1; got:\n{got}\n",
            path.display()
        )
    });
    assert_eq!(got, want, "{name} drawn differently; got:\n{got}\n");
}

const NOTICE: &str = "◆ 1 decision waiting · alt+j answer · alt+shift+j stash and answer";

/// Typing when the decision comes: no card, the keys stay in the box, and
/// the box's edge says it waits.
#[test]
fn a_decision_while_typing_waits_under_the_box() {
    let mut m = model();
    typed(&mut m, "hello there");
    decide(&mut m, "d1");
    let s = screen(&mut m).join("\n");
    assert!(!s.contains("? decide"), "no card while typing:\n{s}");
    assert!(!s.contains("↑ to decide above"), "{s}");
    assert!(s.contains(NOTICE), "{s}");
    assert!(!m.cards().conns[KEY].card_focus);
    golden("hold_notice", &the_box(&mut m));
    // Its letters are still the message's.
    typed(&mut m, " 1a");
    assert_eq!(input(&mut m).0, "hello there 1a");
    assert!(!screen(&mut m).join("\n").contains("? decide"));
}

/// Stopped with text still in the box: the notice stays, the card waits.
#[test]
fn a_pause_with_text_keeps_the_notice() {
    let mut m = model();
    typed(&mut m, "half done");
    decide(&mut m, "d1");
    screen(&mut m);
    at(&mut m, HOLD_MS + 500);
    let s = screen(&mut m).join("\n");
    assert!(!s.contains("? decide"), "{s}");
    assert!(s.contains(NOTICE), "{s}");
}

/// Stopped with the box empty, sent or cleared: the card shows as ever,
/// once no key has come for the hold's while.
#[test]
fn a_pause_with_an_empty_box_shows_it() {
    let mut m = model();
    typed(&mut m, "hi");
    decide(&mut m, "d1");
    screen(&mut m);
    press(&mut m, "backspace");
    press(&mut m, "backspace");
    let s = screen(&mut m).join("\n");
    assert!(!s.contains("? decide"), "still typing a moment ago:\n{s}");
    at(&mut m, HOLD_MS);
    let s = screen(&mut m).join("\n");
    assert!(s.contains("? decide"), "the card:\n{s}");
    assert!(!s.contains("decision waiting"), "{s}");
    assert!(s.contains("↑ to decide above"), "{s}");
}

/// alt+j opens it now with the keys on it, what's typed left as it was.
#[test]
fn alt_j_opens_it_keeping_the_text() {
    let mut m = model();
    typed(&mut m, "my draft");
    decide(&mut m, "d1");
    screen(&mut m);
    press(&mut m, "alt+j");
    let s = screen(&mut m).join("\n");
    assert!(s.contains("? decide"), "{s}");
    assert!(s.contains("answering the card above"), "{s}");
    assert!(m.cards().conns[KEY].card_focus);
    assert_eq!(input(&mut m), ("my draft".into(), 8));
}

/// alt+shift+j sets what's typed aside and opens the card; answered, the
/// text comes back with the cursor where it was.
#[test]
fn stash_then_answer_puts_the_text_back() {
    let mut m = model();
    typed(&mut m, "half a thought");
    for _ in 0..3 {
        press(&mut m, "left");
    }
    decide(&mut m, "d1");
    screen(&mut m);
    press(&mut m, "alt+shift+j");
    assert_eq!(input(&mut m), (String::new(), 0), "set aside");
    let s = screen(&mut m).join("\n");
    assert!(s.contains("? decide"), "{s}");
    assert!(s.contains("stashed · comes back after the decision"), "{s}");
    golden("hold_stashed", &the_box(&mut m));
    // p takes the agent's picks and sends them.
    press(&mut m, "p");
    screen(&mut m);
    assert_eq!(input(&mut m), ("half a thought".into(), 11), "back");
    let s = screen(&mut m).join("\n");
    assert!(!s.contains("stashed"), "{s}");
}

/// Set aside unanswered, the card's text comes back too.
#[test]
fn stash_then_dismiss_puts_the_text_back() {
    let mut m = model();
    typed(&mut m, "keep me");
    press(&mut m, "home");
    decide(&mut m, "d1");
    screen(&mut m);
    press(&mut m, "alt+shift+j");
    assert_eq!(input(&mut m).0, "");
    screen(&mut m);
    press(&mut m, "s");
    let s = screen(&mut m).join("\n");
    assert!(!s.contains("? decide"), "set aside:\n{s}");
    assert_eq!(input(&mut m), ("keep me".into(), 0));
}

/// Two decisions while typing: the notice counts both.
#[test]
fn two_decisions_count_up() {
    let mut m = model();
    typed(&mut m, "still going");
    decide(&mut m, "d1");
    screen(&mut m);
    typed(&mut m, " on");
    decide(&mut m, "d2");
    let s = screen(&mut m).join("\n");
    assert!(s.contains("◆ 2 decisions waiting · alt+j answer"), "{s}");
    assert!(!s.contains("? decide"), "{s}");
    golden("hold_notice_two", &the_box(&mut m));
}
