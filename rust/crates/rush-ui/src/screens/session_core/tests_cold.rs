//! The cards' / commands on a session rush doesn't run now (an idle or
//! past one, its host gone): /fork and alt+f open the fork sheet, #fork
//! hands it on, and the rest say they need rush to run it, never going to
//! the agent as a message. After Go's internal/ui/sessionviews.go:897-985
//! (runRushCommand with a nil client), forksheet.go:106 (openFork reads
//! only c.sess and the agent), rewindsheet.go:74, asksheets.go:22 and :52,
//! handoff.go:58.

use super::tests::flash;
use super::tests_subview::KEY;
use super::{conn, keys};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::session_cards::fork::ForkSheet;

/// Agent KEY's conversation open, rush not running it.
fn cold(d: &tempfile::TempDir) -> Model {
    let mut m = super::tests_subview::model(d);
    m.cards().host = Some(super::cards_host::host());
    m.agents[0].job.session_id = "s1".into();
    m.agents[0].job.cwd = d.path().to_string_lossy().into_owned();
    assert!(!conn(&mut m, KEY).hosted());
    let now = m.now;
    let s = &mut m.session.as_mut().unwrap().sess;
    for (i, p) in ["fix the bug", "why is it slow"].into_iter().enumerate() {
        let at = now - 60_000 * (3 - i as i64);
        s.sent(p.into(), Vec::new(), at);
        s.apply(
            &rush_core::event::Event::TurnEnd(Default::default()),
            at + 1_000,
        );
    }
    m
}

fn send(m: &mut Model, text: &str) -> Out {
    conn(m, KEY).input.set(text);
    keys::send(m, KEY, false)
}

fn press(m: &mut Model, k: &str) -> Out {
    m.press_key(&Press::of(k).expect("a key"))
}

#[test]
fn fork_opens_its_sheet_on_a_session_rush_doesnt_run() {
    let d = tempfile::tempdir().unwrap();
    let mut m = cold(&d);
    send(&mut m, "/fork");
    let f = m.sheet_as::<ForkSheet>().expect("the fork sheet");
    assert_eq!(f.name.iter().collect::<String>(), "port it (fork)");
    assert_eq!(f.sid, "s1");
    assert!(conn(&mut m, KEY).input.string().is_empty(), "nothing sent");
}

#[test]
fn alt_f_on_a_turn_forks_from_it_and_alt_r_says_why_not() {
    let d = tempfile::tempdir().unwrap();
    let mut m = cold(&d);
    let turns = &m.session.as_ref().unwrap().sess.turns;
    let first = turns[turns.len() - 2].n;
    m.session.as_mut().unwrap().sel = format!("t{first}");
    press(&mut m, "alt+f");
    let f = m.sheet_as::<ForkSheet>().expect("alt+f forks");
    assert_eq!(f.turns[f.up_to].n, first, "up to the turn picked");
    m.sheet = None;
    m.session.as_mut().unwrap().sel = format!("t{first}");
    press(&mut m, "alt+r");
    assert!(m.sheet.is_none());
    assert_eq!(
        flash(&m),
        "/rewind works in rush-mode sessions · /rush moves this one over, or /fork from an earlier turn"
    );
}

#[test]
fn rewind_says_it_needs_rush_to_run_it() {
    let d = tempfile::tempdir().unwrap();
    let mut m = cold(&d);
    assert_eq!(send(&mut m, "/rewind"), Out::Draw);
    assert_eq!(
        flash(&m),
        "/rewind works in rush-mode sessions · /rush moves this one over, or /fork from an earlier turn"
    );
    assert!(m.sheet.is_none());
}

#[test]
fn subtask_export_and_btw_say_they_need_rush_to_run_it() {
    let d = tempfile::tempdir().unwrap();
    let mut m = cold(&d);
    let needs = "that works in rush-mode sessions · /rush moves this one over";
    send(&mut m, "/subtask look at the tests");
    assert_eq!(flash(&m), needs);
    m.flash = None;
    send(&mut m, "/export");
    assert_eq!(flash(&m), needs);
    assert!(m.sheet.is_none(), "no sheet: nothing to export from");
    m.flash = None;
    // /btw opens its panel; a question asked there says it can't be.
    send(&mut m, "/btw what's left?");
    assert_eq!(flash(&m), needs);
    let t = crate::screens::session_cards::btw::btw_for(&mut m, KEY).expect("the panel");
    assert!(t.focused);
    assert!(t.qa.is_empty(), "the question went back");
}

#[test]
fn hash_fork_hands_on_a_session_rush_doesnt_run() {
    let d = tempfile::tempdir().unwrap();
    let mut m = cold(&d);
    // Only Claude Code here: no other harness to take it.
    crate::screens::session_cards::handoff::handoff_to(&mut m, KEY, "");
    assert_eq!(
        flash(&m),
        "no other installed harness can take this conversation on"
    );
    crate::screens::session_cards::handoff::handoff_to(&mut m, KEY, "claude");
    assert_eq!(
        flash(&m),
        "this conversation is already on Anthropic (Claude Code)"
    );
}
