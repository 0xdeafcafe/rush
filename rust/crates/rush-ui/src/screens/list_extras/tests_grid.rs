//! The grid beside the open Session, after Go's internal/ui
//! grid_test.go (TestGridEqualSplit, TestGridPinsAndSwaps,
//! TestGridEdgeResizes, TestGridMixedLayout).

use rush_core::agent::State;
use rush_core::text::Line;

use super::grid::*;
use super::tests::{agent, model_of};
use crate::model::Model;
use crate::style::{line, plain};

fn three() -> Model {
    let mut m = model_of(
        (0..3)
            .map(|i| {
                agent(
                    &format!("g{i}"),
                    &format!("agent number {i} doing things"),
                    State::Working,
                    10 + i,
                )
            })
            .collect(),
        240,
        60,
    );
    m.sel = "default/g0".into();
    m
}

fn tile(m: &Model) -> impl Fn(usize, usize, usize) -> Vec<Line> + '_ {
    move |i, _w, h| {
        (0..h)
            .map(|_| line([plain(m.agents[i].display_name.clone())]))
            .collect()
    }
}

/// One tile beside the Session halves the pane; two make thirds; one
/// under it halves the height.
#[test]
fn grid_equal_split() {
    let m = three();
    let mut g = Grid {
        keys: vec!["default/g1".into()],
        ..Grid::default()
    };
    let l = split(&m, &g, 180, 50);
    assert_eq!((l.col.len(), l.sw, l.cw, l.ch), (1, 90, 90, 50));
    g.keys.push("default/g2".into());
    let l = split(&m, &g, 180, 50);
    assert_eq!((l.col.len(), l.sw, l.cw), (2, 60, 60));
    let mut g = Grid {
        keys: vec!["default/g1".into()],
        ..Grid::default()
    };
    g.under.insert("default/g1".into(), true);
    let l = split(&m, &g, 180, 50);
    assert_eq!((l.row.len(), l.sh, l.rh), (1, 25, 25));
    // No room: the Session has it all.
    assert!(split(&m, &g, 60, 20).row.is_empty());
}

/// A pinned agent is a tile beside the Session, or under it, and a click
/// on it swaps it in.
#[test]
fn grid_pins_and_swaps() {
    let mut m = three();
    super::grid::pin(&mut m, false);
    assert!(m.kept::<Grid>().keys.contains(&"default/g0".to_string()));
    m.sel = "default/g1".into();
    let pane = vec![line([plain("the session")]); 50];
    for below in [false, true] {
        let mut g = m.kept::<Grid>().clone();
        g.under.insert("default/g0".into(), below);
        let l = split(&m, &g, 200, 50);
        let rows = join(&m, &mut g, &pane, 30, 4, &l, &tile(&m));
        assert_eq!(rows.len(), 50);
        assert_eq!(g.at.len(), 1, "below={below}");
        let at = &g.at[0];
        assert!(
            if below { at.y > 4 } else { at.x > 30 },
            "below={below}: {at:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.plain().contains("agent number 0 doing things"))
        );
        *m.kept::<Grid>() = g;
    }
    let mut g = m.kept::<Grid>().clone();
    let at = g.at[0].clone();
    assert_eq!(
        click(&m, &mut g, at.x + 2, at.y + 1),
        Click::Swap("default/g0".into())
    );
    assert_eq!(g.keys, ["default/g1"], "the open one takes the tile");
    super::grid::unpin(&mut m, true);
    assert!(m.kept::<Grid>().keys.is_empty());
}

/// Beside and under at once: the column as tall as the Session, the row
/// as wide as the pane; held, the column's edge resizes the tiles.
#[test]
fn grid_mixed_layout_and_edge() {
    let m = three();
    let mut g = Grid {
        keys: vec!["default/g1".into(), "default/g2".into()],
        ..Grid::default()
    };
    g.under.insert("default/g2".into(), true);
    let pane = vec![Line::new(); 60];
    let l = split(&m, &g, 200, 60);
    join(&m, &mut g, &pane, 10, 2, &l, &tile(&m));
    let (col, row) = (g.at[0].clone(), g.at[1].clone());
    assert_eq!(
        (col.key.as_str(), row.key.as_str()),
        ("default/g1", "default/g2")
    );
    assert!(col.x > 10 && row.x == 10 && row.y == col.y + col.h && row.w > col.x - 10);
    assert_eq!(click(&m, &mut g, col.x, col.y + 2), Click::Edge);
    assert!(g.drag);
    drag(&mut g, col.x - 40, col.y + 2);
    let l2 = split(&m, &g, 200, 60);
    assert!(
        l2.cw > l.cw,
        "dragged left, the tiles widen: {} → {}",
        l.cw,
        l2.cw
    );
}
