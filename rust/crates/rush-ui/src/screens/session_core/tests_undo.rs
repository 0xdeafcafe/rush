//! Undo for every way text leaves a message box, the Agents box and a
//! Session's (frame/keepundo.rs): each deletion, then ctrl+/ (and its
//! other keys), brings the text back with the cursor where it was, and
//! redo takes it away again. What was sent isn't undone into the box.

use rush_core::agent::State;

use super::conn;
use super::tests::{KEY, agent, flash, hosted, open};
use crate::frame::places::PlaceId;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::worker::Note;
use rush_draw::{App, Msg};

/// A key as the terminal's loop hands it in: through Model::key, where
/// a box's changes are kept for undo.
fn key(m: &mut Model, s: &str) {
    let p = Press::of(s).unwrap_or_else(|| panic!("no key {s:?}"));
    m.key(p.code, p.mods);
}

fn typed(m: &mut Model, text: &str) {
    for c in text.chars() {
        key(
            m,
            &if c == ' ' {
                "space".into()
            } else {
                c.to_string()
            },
        );
    }
}

/// The Session box's text, and its cursor.
fn sbox(m: &mut Model) -> (String, usize) {
    let c = conn(m, KEY);
    (c.input.string(), c.input.pos)
}

/// The Agents box's.
fn pbox(m: &Model) -> (String, usize) {
    (m.input.string(), m.input.pos)
}

fn session() -> Model {
    hosted("steer", Vec::new()).0
}

/// The Agents list with its box, nothing open.
fn prompt() -> Model {
    let fake = super::host::Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    m.session = None;
    m.full = false;
    assert!(m.ext.agents_box, "the Agents box is on screen");
    m
}

/// "fix the parser" typed, the cursor moved back to after "fix the".
fn typed_and_back(m: &mut Model) {
    typed(m, "fix the parser");
    for _ in 0.."parser".len() + 1 {
        key(m, "left");
    }
}

/// Each delete key, in either box: undo puts the text back with the
/// cursor where it was; redo deletes it again.
#[test]
fn every_delete_key_undoes_in_both_boxes() {
    for del in [
        "backspace",
        "delete",
        "ctrl+h",
        "alt+backspace",
        "ctrl+w",
        "alt+delete",
        "ctrl+u",
        "ctrl+k",
        "super+backspace",
        "super+delete",
    ] {
        for (which, mut m) in [("session", session()), ("prompt", prompt())] {
            let get = |m: &mut Model| if which == "session" { sbox(m) } else { pbox(m) };
            typed_and_back(&mut m);
            let before = get(&mut m);
            assert_eq!(before, ("fix the parser".into(), 7), "{which}");
            key(&mut m, del);
            let after = get(&mut m);
            assert_ne!(after.0, before.0, "{which}: {del} deleted something");
            for undo in ["ctrl+/", "ctrl+_", "super+z"] {
                key(&mut m, undo);
                assert_eq!(get(&mut m), before, "{which}: {undo} after {del}");
                key(&mut m, "shift+super+z");
                assert_eq!(get(&mut m).0, after.0, "{which}: redo after {del}");
            }
        }
    }
}

/// A selection deleted, typed over, cut or pasted over comes back.
#[test]
fn selections_undo_in_both_boxes() {
    for which in ["session", "prompt"] {
        let fresh = || {
            if which == "session" {
                session()
            } else {
                prompt()
            }
        };
        let get = |m: &mut Model| if which == "session" { sbox(m) } else { pbox(m) };
        for how in ["backspace", "delete", "x", "super+x", "paste"] {
            let mut m = fresh();
            typed(&mut m, "keep this word");
            for _ in 0..5 {
                key(&mut m, "shift+left"); // "word" and the space before
            }
            let was = get(&mut m);
            match how {
                "paste" => {
                    m.paste("other");
                }
                k => key(&mut m, k),
            }
            assert!(get(&mut m).0.starts_with("keep this"), "{which} {how}");
            assert!(!get(&mut m).0.contains("word"), "{which} {how}");
            if how == "super+x" {
                assert_eq!(m.pending_copy.as_deref(), Some(" word"), "cut copies");
            }
            key(&mut m, "ctrl+/");
            assert_eq!(get(&mut m).0, was.0, "{which}: {how} undone");
            // ctrl+y is redo in a Session's box; in the Agents box it
            // opens the agent's pull request (keys.go:444 before :525).
            key(&mut m, if which == "session" { "ctrl+y" } else { "shift+super+z" });
            assert!(!get(&mut m).0.contains("word"), "{which}: {how} redone");
        }
    }
}

/// esc and ctrl+c clear either box; undo brings it back, as the flash
/// says.
#[test]
fn clearing_undoes_in_both_boxes() {
    for which in ["session", "prompt"] {
        for clear in ["esc", "ctrl+c"] {
            let mut m = if which == "session" {
                session()
            } else {
                prompt()
            };
            let get = |m: &mut Model| if which == "session" { sbox(m) } else { pbox(m) };
            typed_and_back(&mut m);
            key(&mut m, clear);
            assert_eq!(get(&mut m).0, "", "{which}: {clear} clears");
            assert_eq!(flash(&m), "cleared · cmd+z or ctrl+/ brings it back");
            key(&mut m, "ctrl+/");
            assert_eq!(get(&mut m), ("fix the parser".into(), 7), "{which} {clear}");
        }
    }
}

/// A paste chip taken out whole, or opened in place, comes back folded.
#[test]
fn paste_chips_undo() {
    let long = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl";
    let mut m = session();
    typed(&mut m, "see ");
    m.paste(long);
    let chipped = sbox(&mut m).0;
    assert!(chipped.contains("[pasted text #1"), "{chipped}");
    key(&mut m, "backspace");
    assert_eq!(sbox(&mut m).0, "see ", "the chip goes whole");
    key(&mut m, "ctrl+/");
    assert_eq!(sbox(&mut m).0, chipped, "and comes back");
    key(&mut m, "ctrl+/");
    assert_eq!(sbox(&mut m).0, "see ", "the paste undone");

    let mut m = prompt();
    m.paste(long);
    let chipped = pbox(&m).0;
    assert!(chipped.contains("[pasted text #1"), "{chipped}");
    key(&mut m, "backspace");
    assert_eq!(pbox(&m).0, "");
    key(&mut m, "ctrl+/");
    assert_eq!(pbox(&m).0, chipped);
}

/// Whatever else changes a box (a picker completing, the stash or the
/// history setting it, an editor's text coming back) is one step to
/// undo, though nothing in it said so.
#[test]
fn a_box_set_from_elsewhere_undoes() {
    let mut m = session();
    typed_and_back(&mut m);
    m.update(Msg::App(Note::Apply(Box::new(|m: &mut Model| {
        conn(m, KEY).input.set("from the history");
        Out::Draw
    }))));
    assert_eq!(sbox(&mut m).0, "from the history");
    key(&mut m, "ctrl+/");
    assert_eq!(sbox(&mut m), ("fix the parser".into(), 7));
    key(&mut m, "ctrl+y");
    assert_eq!(sbox(&mut m).0, "from the history");

    let mut m = prompt();
    typed_and_back(&mut m);
    m.update(Msg::App(Note::Apply(Box::new(|m: &mut Model| {
        m.input.set("#view list");
        Out::Draw
    }))));
    key(&mut m, "ctrl+/");
    assert_eq!(pbox(&m), ("fix the parser".into(), 7));
}

/// Going to another place empties the Agents box; back on Agents, undo
/// brings it back.
#[test]
fn another_place_undoes() {
    let mut m = prompt();
    typed_and_back(&mut m);
    m.guarded(|m| m.set_place(PlaceId::Settings));
    assert_eq!(pbox(&m).0, "");
    m.set_place(PlaceId::Agents);
    key(&mut m, "ctrl+/");
    assert_eq!(pbox(&m), ("fix the parser".into(), 7));
}

/// What was sent isn't undone back into the box: the history has it.
#[test]
fn sent_text_is_not_undone() {
    let mut m = session();
    typed(&mut m, "ship it");
    key(&mut m, "enter");
    assert_eq!(sbox(&mut m).0, "");
    key(&mut m, "ctrl+/");
    assert_eq!(sbox(&mut m).0, "", "sent, not undone");
}

/// A name being renamed in the Agents box isn't the message's to undo:
/// the box's undo is as it was once the rename's done.
#[test]
fn renaming_keeps_the_box_undo() {
    let mut m = prompt();
    typed(&mut m, "draft");
    key(&mut m, "esc");
    crate::screens::list_extras::rename::start_rename(&mut m, KEY);
    typed(&mut m, "new name");
    key(&mut m, "esc");
    assert_eq!(pbox(&m).0, "");
    key(&mut m, "ctrl+/");
    assert_eq!(pbox(&m).0, "draft", "the cleared draft, not the name");
}

/// The Session's draft moved to the Agents box: moved, not gone, and
/// what the Agents box held is one undo away.
#[test]
fn draft_to_prompt_keeps_what_was_there() {
    let mut m = session();
    m.input.set("old task");
    typed(&mut m, "move me");
    m.guarded(|m| super::actions::run_action(m, "session.toprompt"));
    assert_eq!(pbox(&m).0, "move me");
    assert_eq!(conn(&mut m, KEY).input.string(), "");
    assert!(m.session.is_none(), "the keys are the Agents box's");
    key(&mut m, "ctrl+/");
    assert_eq!(pbox(&m), ("old task".into(), 8));
    key(&mut m, "shift+super+z"); // ctrl+y is the list's pull request

    assert_eq!(pbox(&m).0, "move me");
}

/// Away to the list and back, the Session's box keeps its draft and its
/// undo.
#[test]
fn leaving_a_session_keeps_the_draft_and_undo() {
    let mut m = session();
    typed_and_back(&mut m);
    key(&mut m, "ctrl+w");
    let after = sbox(&mut m).0;
    let v = m.session.take();
    key(&mut m, "down");
    m.session = v;
    assert_eq!(sbox(&mut m).0, after);
    key(&mut m, "ctrl+/");
    assert_eq!(sbox(&mut m), ("fix the parser".into(), 7));
}
