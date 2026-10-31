//! Clicks and the pointer on Agents and the Session beside it, at exact
//! cells: a click on the Session or on Agents' box gives it the keys and
//! puts the cursor where it landed, wrapped rows and wide runes counted;
//! the pointer crossing to the other side gives that side the keys, once.
//! After Go's model.go (:1478-1489 and :1556 the pointer crossing,
//! :1664 a click on the Session, :1928 focusAt) and rushmode.go:4499
//! (clickBox).

use rush_core::agent::State;
use rush_core::text::width;
use rush_draw::{App, Button, Event, KeyMods, MouseKind, Msg};

use super::host::Fake;
use super::tests::{KEY, agent, open, press, screen};
use super::{conn, open_key};
use crate::frame::Mouse;
use crate::model::Model;
use crate::screens::list_extras::split;

/// One agent, its Session beside the list at w×h with the keys.
fn split_open(w: usize, h: usize) -> Model {
    let fake = Fake::new();
    let mut m = open(agent("plain", State::Idle), &fake, w, h);
    m.full = false;
    m.cfg.set_view("split");
    m.roots = vec![String::new()];
    m.rebuild();
    let _ = screen(&mut m);
    m
}

fn ev(m: &mut Model, kind: MouseKind, x: usize, y: usize) {
    m.mouse_event(&Mouse {
        kind,
        x: x as u16,
        y: y as u16,
        mods: KeyMods::NONE,
    });
}

/// A left click at the 0-based cell (x, y), pressed and let go, as a
/// terminal reports ESC [ < 0 ; x+1 ; y+1 M then m.
fn click(m: &mut Model, x: usize, y: usize) {
    ev(m, MouseKind::Down(Button::Left), x, y);
    ev(m, MouseKind::Up(Button::Left), x, y);
}

/// Where `needle` is drawn on the frame at or past column `from`: its
/// 0-based (column, row), in cells.
fn cell_of(m: &mut Model, needle: &str, from: usize) -> (usize, usize) {
    let rows = screen(m);
    for (y, r) in rows.iter().enumerate().rev() {
        for (i, _) in r.match_indices(needle) {
            let x = width(&r[..i]);
            if x >= from {
                return (x, y);
            }
        }
    }
    panic!("{needle:?} isn't on screen:\n{}", rows.join("\n"));
}

fn session_input(m: &mut Model, text: &str) {
    let c = conn(m, KEY);
    c.input.text = text.chars().collect();
    c.input.pos = c.input.text.len();
}

/// A click on the Session's box beside the list, the list having the
/// keys, gives the Session the keys and puts the cursor on the rune
/// clicked, at once: the second cell of a wide rune is that rune, a cell
/// past a row's end is the row's end.
#[test]
fn a_click_on_the_sessions_box_beside_the_list() {
    let text = format!("abc 世界 def {}zzz", "word ".repeat(40));
    let n = text.chars().count();
    // (what's clicked, cells right of its first, the cursor's place)
    let cases: &[(&str, usize, usize)] = &[
        ("abc", 0, 0),
        ("abc", 1, 1),
        ("世界", 0, 4),
        ("世界", 1, 4),
        ("世界", 2, 5),
        ("世界", 3, 5),
        (" def", 1, 7),
        ("zzz", 0, n - 3),
        ("zzz", 3, n),
        ("zzz", 30, n),
    ];
    for &(at, right, want) in cases {
        let mut m = split_open(200, 50);
        let (lw, pw) = split::widths(&m);
        assert!(lw > 0 && pw > 0, "side by side");
        press(&mut m, "esc");
        assert!(m.session.is_none(), "esc gives the list the keys");
        session_input(&mut m, &text);
        let (x, y) = cell_of(&mut m, at, lw + 1);
        click(&mut m, x + right, y);
        assert_eq!(
            open_key(&m).as_deref(),
            Some(KEY),
            "{at:?}+{right}: the Session has the keys"
        );
        assert_eq!(
            conn(&mut m, KEY).input.pos,
            want,
            "{at:?}+{right}: the cursor"
        );
    }
}

/// A click on Agents' box while the Session has the keys gives the box
/// the keys and the cursor goes where it landed, on its wrapped rows
/// too.
#[test]
fn a_click_on_agents_box_beside_the_session() {
    let mut m = split_open(200, 50);
    m.ext.agents_box = true;
    let text = format!("fix 世界 the {}qqq", "thing ".repeat(12));
    m.input.text = text.chars().collect();
    m.input.pos = 0;
    let _ = screen(&mut m);
    assert!(m.session.is_some());
    let (x, y) = cell_of(&mut m, "fix", 0);
    click(&mut m, x + 7, y); // the second cell of 界
    assert!(m.session.is_none(), "Agents has the keys");
    assert_eq!(m.input.pos, 5);
    let (x, y) = cell_of(&mut m, "qqq", 0);
    click(&mut m, x + 1, y);
    assert_eq!(m.input.pos, text.chars().count() - 2, "on a wrapped row");
}

/// The pointer coming onto the Session gives it the keys, and onto the
/// list gives them back, once as it crosses: tab to the other holds
/// while it stays put; back from another window, the first move only
/// says where it is (model.go:1478-1489, :1556).
#[test]
fn the_pointer_crossing_moves_the_keys() {
    let mut m = split_open(200, 50);
    let (lw, _) = split::widths(&m);
    ev(&mut m, MouseKind::Moved, lw + 20, 20);
    assert!(m.session.is_some(), "over the Session: it keeps the keys");
    ev(&mut m, MouseKind::Moved, 10, 20);
    assert!(m.session.is_none(), "onto the list: the list has them");
    ev(&mut m, MouseKind::Moved, 12, 21);
    assert!(m.session.is_none());
    ev(&mut m, MouseKind::Moved, lw + 30, 20);
    assert!(m.session.is_some(), "onto the Session again");
    // esc gives the list the keys; moving about the Session doesn't take
    // them back till the pointer has been away.
    press(&mut m, "esc");
    ev(&mut m, MouseKind::Moved, lw + 31, 22);
    assert!(m.session.is_none(), "it hasn't crossed");
    ev(&mut m, MouseKind::Moved, lw + 20, 20);
    // Away to another window and back: crossing on the way in doesn't
    // count.
    click(&mut m, lw + 20, 20);
    assert!(m.session.is_some(), "a click gives the Session the keys");
    let _ = m.update(Msg::Input(Event::FocusLost));
    let _ = m.update(Msg::Input(Event::FocusGained));
    ev(&mut m, MouseKind::Moved, 5, 20);
    assert!(m.session.is_some(), "coming back over the list");
    ev(&mut m, MouseKind::Moved, lw + 40, 20);
    ev(&mut m, MouseKind::Moved, 5, 20);
    assert!(m.session.is_none(), "a real crossing does");
}
