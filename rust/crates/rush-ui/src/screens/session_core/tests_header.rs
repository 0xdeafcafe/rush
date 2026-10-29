//! The header's places hint with a Session open: ctrl+\ once its box
//! takes text, < > while it's still connecting or couldn't be opened
//! (view.go:240: m.paneFocus && m.host != nil, m.host set by onHostOpen,
//! rushmode.go:2136).

use rush_core::agent::State;

use super::host::Fake;
use super::tests::{KEY, agent, hosted, open, screen};
use crate::session::{SessionView, Source};

fn hint(m: &mut crate::model::Model) -> &'static str {
    let s = screen(m).join("\n");
    match (s.contains("ctrl+\\ places"), s.contains("< > places")) {
        (true, false) => "ctrl+\\",
        (false, true) => "< >",
        _ => panic!("no one places hint:\n{s}"),
    }
}

#[test]
fn places_hint_waits_for_the_box() {
    let fake = Fake::new();
    // Its conversation read: the box takes text, < > with it.
    let mut m = open(agent("plain", State::Done), &fake, 120, 40);
    assert_eq!(hint(&mut m), "ctrl+\\", "read");
    // Its transcript couldn't be read: no box (onHostOpen's error).
    m.session.as_mut().unwrap().err = "no such file".into();
    assert_eq!(hint(&mut m), "< >", "failed");
    // Not read yet.
    let mut m = open(agent("plain", State::Done), &fake, 120, 40);
    m.session.as_mut().unwrap().read = false;
    assert_eq!(hint(&mut m), "< >", "reading");

    // A hosted one is its host's: connecting, or the dial failed, there's
    // no box, read or not.
    let mut m = open(agent("steer", State::Working), &fake, 120, 40);
    let mut v = SessionView::new(KEY.into(), Source::Hosted, 1);
    v.read = true;
    m.session = Some(v);
    assert_eq!(hint(&mut m), "< >", "connecting");
    // Reached: ctrl+\, before its replay.
    let (mut m, _f) = hosted("steer", Vec::new());
    assert_eq!(hint(&mut m), "ctrl+\\", "connected");
}
