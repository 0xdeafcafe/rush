//! The grid's tiles as their agents' own Sessions, kept open while they
//! are pinned: drawn with their conversation, their output still taken
//! in, taken back in at once, and their box given the keys where it is.
//! After Go's internal/ui/grid_test.go (TestGridKeepsTileSessions,
//! TestGridTileFocusTakesKeys).

use std::sync::Arc;

use rush_core::agent::State;
use rush_draw::{Button, KeyMods, MouseKind};

use super::host::Fake;
use super::tests::{KEY, agent, hosted, press, screen, typed};
use super::{conn, live};
use crate::model::Model;
use crate::screens::list_extras::grid::{self, Grid};

const OTHER: &str = "default/other";

/// A session rush hosts, its Session beside the list with a message in
/// it, pinned to the grid, and another agent picked: the pinned one's
/// Session kept for its tile.
fn pinned() -> (Model, Arc<Fake>) {
    let (mut m, fake) = hosted("steer", Vec::new());
    m.w = 240;
    m.h = 60;
    m.full = false;
    m.cfg.set_view("split");
    let now = m.now;
    let v = m.session.as_mut().unwrap();
    v.read = true;
    v.sess
        .sent("now make streaming quicker".into(), Vec::new(), now);
    // The other one rush hosts too: its Session beside the list has its
    // own connection.
    fake.host(OTHER);
    let mut other = agent("steer", State::Idle);
    other.key = OTHER.into();
    other.display_name = "the other one".into();
    other.job.id = "o1".into();
    m.agents.push(other);
    m.roots = vec![String::new(); 2];
    m.rebuild();
    let _ = screen(&mut m);
    grid::pin(&mut m, false);
    m.sel = OTHER.into();
    (m, fake)
}

fn click(m: &mut Model, x: usize, y: usize) {
    m.mouse_event(&crate::frame::Mouse {
        kind: MouseKind::Down(Button::Left),
        x: x as u16,
        y: y as u16,
        mods: KeyMods::default(),
    });
}

/// Another agent opened, the pinned one's Session stays open for its
/// tile, connection and all, and the tile is drawn as that Session; back
/// on it, the kept one is taken in at once, not read or dialled again.
#[test]
fn a_pinned_session_is_kept_for_its_tile() {
    let (mut m, fake) = pinned();
    let f = screen(&mut m).join("\n");
    assert!(m.tiles.contains_key(KEY), "not kept");
    assert!(conn(&mut m, KEY).hosted(), "its connection was let go");
    assert_eq!(m.kept::<Grid>().at.len(), 1);
    assert!(f.contains("now make streaming quicker"), "{f}");
    live::sync(&mut m);
    assert!(
        conn(&mut m, KEY).hosted(),
        "its connection was let go once synced"
    );
    // Drawing the tile leaves the open one's connection be.
    let _ = screen(&mut m);
    let _ = screen(&mut m);
    assert!(
        conn(&mut m, OTHER).hosted(),
        "the open one's connection was let go"
    );
    assert_eq!(*fake.dials.lock().unwrap(), 2, "dialled again");
    m.sel = KEY.into();
    let _ = screen(&mut m);
    assert!(m.tiles.is_empty(), "the kept one wasn't taken back in");
    assert_eq!(m.pane.as_ref().map(|v| v.key.as_str()), Some(KEY));
    assert_eq!(*fake.dials.lock().unwrap(), 2, "dialled again");
    // Unpinned, a kept Session and its connection are let go.
    m.sel = OTHER.into();
    let _ = screen(&mut m);
    assert!(m.tiles.contains_key(KEY));
    grid::unpin(&mut m, true);
    assert!(m.tiles.is_empty());
    assert!(!conn(&mut m, KEY).hosted());
}

/// A click on a tile's box gives it the keys without swapping it in:
/// what's typed lands in its box, the open one stays picked; esc in its
/// empty box hands them back; elsewhere on the tile still swaps it in.
#[test]
fn a_tiles_box_takes_the_keys_where_it_is() {
    let (mut m, _fake) = pinned();
    let _ = screen(&mut m);
    let at = m.kept::<Grid>().at[0].clone();
    let by = *m.kept::<Grid>().box_at.get(KEY).expect("its box found");
    assert!(by > at.y && by < at.y + at.h, "box at {by}, tile {at:?}");
    click(&mut m, at.x + 3, by + 1);
    assert_eq!(m.kept::<Grid>().focus, KEY);
    assert_eq!(m.sel, OTHER);
    let f = screen(&mut m);
    assert!(
        f[at.y..at.y + at.h]
            .iter()
            .all(|l| l.chars().nth(at.x) == Some('┃')),
        "the tile with the keys isn't marked"
    );
    typed(&mut m, "x");
    assert_eq!(conn(&mut m, KEY).input.string(), "x");
    assert_eq!(m.sel, OTHER, "the open one moved");
    assert!(
        m.tiles.contains_key(KEY),
        "the tile's Session was swapped in"
    );
    conn(&mut m, KEY).input.set("");
    press(&mut m, "esc");
    assert_eq!(m.kept::<Grid>().focus, "", "esc didn't hand the keys back");
    let _ = screen(&mut m);
    let at = m.kept::<Grid>().at[0].clone();
    click(&mut m, at.x + 3, at.y + 1);
    assert_eq!(m.sel, KEY, "a click on the tile didn't swap it in");
}
