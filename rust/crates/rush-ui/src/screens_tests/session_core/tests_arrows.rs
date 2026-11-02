//! The arrows between the message box, the conversation and the column
//! beside it: ↑ on the box's first line goes up into the conversation, →
//! opens the picked item or goes on into the column, ← comes back.

use super::beside_tests::{rush_open, view};
use super::stepkeys_tests::add_fetch;
use super::*;
use crate::screens::plugin_ui::views::read;

/// Folds the fetch step r (it is drawn open by default), nothing picked.
/// Panics with the screen when it does not show closed.
fn closed_row(m: &mut Model, r: &str) -> String {
    m.session
        .as_mut()
        .unwrap()
        .o
        .open
        .insert(r.to_string(), false);
    let shown = screen(m).join("\n");
    let v = m.session.as_mut().unwrap();
    v.sel.clear();
    if !v
        .r
        .rows()
        .iter()
        .any(|l| l.r#ref == r && l.line.plain().contains('▸'))
    {
        panic!("{r} not drawn closed:\n{shown}");
    }
    r.to_string()
}

/// A session 120 wide (no column) with a step in it, nothing picked.
fn plain() -> (Model, String) {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    let r = add_fetch(&mut m);
    let r = closed_row(&mut m, &r);
    (m, r)
}

/// A session wide enough for the column, with a view folded into it so
/// the column has an item to pick, and a step in the conversation.
fn with_column(id: &str) -> (Model, String) {
    let mut m = rush_open(id, 220, 50, vec![view("tests", "Tests", "")]);
    read::state(&mut m, KEY).shut = true;
    let r = add_fetch(&mut m);
    let r = closed_row(&mut m, &r);
    assert!(
        crate::screens::session_core::side::has_items(&mut m, KEY),
        "the column has something to pick"
    );
    (m, r)
}

fn sel(m: &Model) -> String {
    m.session.as_ref().unwrap().sel.clone()
}

fn side_pick(m: &mut Model) -> Option<usize> {
    conn(m, KEY).side_pick
}

fn set_box(m: &mut Model, text: &str, pos: usize) {
    let c = conn(m, KEY);
    c.input.text = text.chars().collect();
    c.input.pos = pos;
    screen(m);
}

/// ↑ with nothing typed picks the row nearest the box.
#[test]
fn up_from_the_empty_box_picks_the_row_by_it() {
    let (mut m, _) = plain();
    press(&mut m, "up");
    assert!(!sel(&m).is_empty(), "a row picked");
}

/// ↑ on the first line of a draft leaves the box for the conversation;
/// what's typed stays.
#[test]
fn up_on_the_first_line_leaves_the_box() {
    let (mut m, _) = plain();
    typed(&mut m, "hello");
    screen(&mut m);
    press(&mut m, "up");
    assert!(!sel(&m).is_empty(), "a row picked");
    assert_eq!(input(&mut m), "hello", "the draft stays");
}

/// ↑ in a draft of several lines moves the cursor up first, and only at
/// the top line leaves.
#[test]
fn up_in_a_long_draft_moves_the_cursor_first() {
    let (mut m, _) = plain();
    set_box(&mut m, "one\ntwo", 7);
    press(&mut m, "up");
    assert!(sel(&m).is_empty(), "still in the box");
    assert!(conn(&mut m, KEY).input.pos < 4, "on the first line now");
    press(&mut m, "up");
    assert!(!sel(&m).is_empty(), "left for the conversation");
    assert_eq!(input(&mut m), "one\ntwo");
}

/// → on a picked step that can open opens it; once it is open, → goes on
/// into the column, its first item picked; ← comes back with the pick
/// kept.
#[test]
fn right_opens_then_goes_on_to_the_column() {
    let (mut m, r) = with_column("arrows-convo");
    m.session.as_mut().unwrap().sel = r.clone();
    screen(&mut m);
    press(&mut m, "right");
    assert_eq!(
        m.session.as_ref().unwrap().o.open.get(&r),
        Some(&true),
        "opened"
    );
    assert_eq!(side_pick(&mut m), None, "not yet in the column");
    press(&mut m, "right");
    assert!(side_pick(&mut m).is_some(), "in the column");
    screen(&mut m);
    press(&mut m, "left");
    assert_eq!(side_pick(&mut m), None, "back out");
    assert_eq!(sel(&m), r, "to the pick it came from");
}

/// → with nothing picked and the box empty goes into the column; ←
/// returns to the box.
#[test]
fn right_with_nothing_picked_goes_to_the_column() {
    let (mut m, _) = with_column("arrows-none");
    press(&mut m, "right");
    assert!(side_pick(&mut m).is_some(), "in the column");
    press(&mut m, "left");
    assert_eq!(side_pick(&mut m), None);
    assert!(sel(&m).is_empty(), "back to the box");
}

/// → at the end of a draft does the same; with the cursor inside the text
/// it only moves the cursor.
#[test]
fn right_in_a_draft_moves_the_cursor_until_its_end() {
    let (mut m, _) = with_column("arrows-draft");
    set_box(&mut m, "hello", 3);
    press(&mut m, "right");
    assert_eq!(conn(&mut m, KEY).input.pos, 4, "the cursor moved");
    assert_eq!(side_pick(&mut m), None);
    press(&mut m, "right");
    assert_eq!(conn(&mut m, KEY).input.pos, 5);
    assert_eq!(side_pick(&mut m), None);
    press(&mut m, "right");
    assert!(side_pick(&mut m).is_some(), "at the end: the column");
    press(&mut m, "left");
    assert_eq!(side_pick(&mut m), None);
    assert_eq!(input(&mut m), "hello", "the draft is where it was");
}

/// → at the end of a draft opens the picked step that can open.
#[test]
fn right_at_the_end_of_a_draft_opens_the_pick() {
    let (mut m, r) = plain();
    typed(&mut m, "hi");
    m.session.as_mut().unwrap().sel = r.clone();
    screen(&mut m);
    press(&mut m, "right");
    assert_eq!(m.session.as_ref().unwrap().o.open.get(&r), Some(&true));
    assert_eq!(input(&mut m), "hi");
}

/// → with nothing to open and no column does nothing.
#[test]
fn right_with_nowhere_to_go_does_nothing() {
    let (mut m, _) = plain();
    press(&mut m, "right");
    assert_eq!(side_pick(&mut m), None);
    assert!(sel(&m).is_empty());
    assert!(m.session.as_ref().unwrap().o.open.is_empty());
    typed(&mut m, "hi");
    press(&mut m, "right");
    assert_eq!(side_pick(&mut m), None);
    assert_eq!(input(&mut m), "hi");
}
