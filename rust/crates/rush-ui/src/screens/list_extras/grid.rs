//! The grid: agents pinned beside the open Session, or under it, each a
//! tile drawn as the Wall draws one; a click on a tile swaps that agent
//! in, and the one it replaces takes the tile; the edge between them is
//! a handle that resizes the tiles. After Go's internal/ui/grid.go
//! (gridState :16, gridLayout :31, gridMinW/H :37, gridAgents :43,
//! gridPin :59, gridUnpin :83, gridSplit :98, gridJoin :136, gridClick
//! :201, gridDrag :347). Kept in `Model::kept::<Grid>()`. Not here yet:
//! a tile's own kept Session (gridSession, gridAdopt, gridPark,
//! gridKey) and saving the grid (gridSave), which wait on rush hosting
//! sessions and on rush's own state; and the ctrl+] chords, which wait
//! on the keymap reaching the UI. The frame has no seam yet for rows
//! beside the open Session, so `join` is what that seam will call.

use std::collections::HashMap;

use rush_core::text::Line;

use crate::model::Model;
use crate::style::*;

/// Tiles narrower or shorter than these aren't drawn: the Session has it
/// all (gridMinW, gridMinH).
pub const MIN_W: usize = 36;
pub const MIN_H: usize = 7;

/// Where a tile was drawn on screen, for clicks (wallTile).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tile {
    pub key: String,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// The grid as it stands (gridState).
#[derive(Clone, Debug, Default)]
pub struct Grid {
    /// The pinned agents, in order.
    pub keys: Vec<String>,
    /// Tiles in the row under the Session; the rest are beside it.
    pub under: HashMap<String, bool>,
    /// The tile whose box has the keys.
    pub focus: String,
    /// Where each tile was drawn.
    pub at: Vec<Tile>,
    /// How much of the pane the column takes (the row, with no column);
    /// 0 is an equal share.
    pub share: f64,
    /// The pane the grid was laid in: x, y, w, h on screen.
    pub pane: [usize; 4],
    /// The column's edge on screen, 0 with no column; the row's likewise.
    pub col_x: usize,
    pub row_y: usize,
    /// The edge between the Session and the tiles is held.
    pub drag: bool,
}

/// How the pane divides: the Session top-left, the column beside it as
/// tall as it, the row under both as wide as the pane (gridLayout). col
/// and row are agents' indexes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub sw: usize,
    pub sh: usize,
    pub cw: usize,
    pub ch: usize,
    pub rw: usize,
    pub rh: usize,
    pub col: Vec<usize>,
    pub row: Vec<usize>,
}

/// The pinned agents still about, the open one left out (gridAgents).
pub fn agents(m: &Model, g: &Grid) -> Vec<usize> {
    let open = m.focused().map(|i| m.agents[i].key.as_str());
    g.keys
        .iter()
        .filter(|k| Some(k.as_str()) != open)
        .filter_map(|k| m.agents.iter().position(|a| a.key == *k))
        .collect()
}

/// Pins the open agent as a tile, beside the Session or under it
/// (gridPin).
pub fn pin(m: &mut Model, below: bool) {
    let Some(i) = m.focused() else {
        m.flash("open an agent first");
        return;
    };
    let key = m.agents[i].key.clone();
    let name = one_line(&m.agents[i].display_name);
    let g = m.kept::<Grid>();
    g.under.insert(key.clone(), below);
    if !g.keys.contains(&key) {
        g.keys.push(key);
    }
    let where_ = if below { "under" } else { "beside" };
    m.flash(&format!(
        "{name} pinned {where_} the Session · pick the agent to open beside it"
    ));
}

/// Takes the newest tile off, or every tile with all (gridUnpin).
pub fn unpin(m: &mut Model, all: bool) {
    let g = m.kept::<Grid>();
    if g.keys.is_empty() {
        m.flash("no tiles pinned");
    } else if all {
        g.keys.clear();
    } else {
        g.keys.pop();
    }
}

/// How the pane's w×h divides; no tiles, or no room for them, leaves the
/// Session all of it (gridSplit).
pub fn split(m: &Model, g: &Grid, w: usize, h: usize) -> Layout {
    let mut l = Layout {
        sw: w,
        sh: h,
        ..Layout::default()
    };
    let (mut col, mut row) = (Vec::new(), Vec::new());
    for i in agents(m, g) {
        if g.under.get(&m.agents[i].key).copied().unwrap_or(false) {
            row.push(i);
        } else {
            col.push(i);
        }
    }
    if !row.is_empty() {
        // Alone, the row and the Session halve the height (or as
        // dragged); with a column too the row is a third.
        let rs = if !col.is_empty() {
            1.0 / 3.0
        } else if g.share != 0.0 {
            g.share
        } else {
            0.5
        };
        let rh = MIN_H.max((h as f64 * rs) as usize);
        let rw = w / row.len();
        if rw >= MIN_W && h >= rh + 12 {
            l.sh = h - rh;
            l.rw = rw;
            l.rh = rh;
            l.row = row;
        }
    }
    if !col.is_empty() {
        // Side by side: the Session and each tile an equal width, or the
        // tiles share what the edge was dragged to.
        let n = col.len();
        let share = if g.share != 0.0 {
            g.share
        } else {
            n as f64 / (n + 1) as f64
        };
        let cw = (w as f64 * share) as usize / n;
        if cw >= MIN_W && w >= cw * n + MIN_W {
            l.sw = w - cw * n;
            l.cw = cw;
            l.ch = l.sh;
            l.col = col;
        }
    }
    l
}

/// Lays the tiles round the Session's rows, the pane at (x, y) on
/// screen, and notes where each went for clicks (gridJoin). tile draws
/// agent i's tile w×h, as the Wall does.
pub fn join(
    m: &Model,
    g: &mut Grid,
    pane: &[Line],
    x: usize,
    y: usize,
    l: &Layout,
    tile: &dyn Fn(usize, usize, usize) -> Vec<Line>,
) -> Vec<Line> {
    g.at.clear();
    g.pane = [x, y, l.sw + l.cw, l.sh + l.rh];
    g.col_x = 0;
    g.row_y = 0;
    let draw = |tiles: &[usize], tw: usize, th: usize| -> Vec<Vec<Line>> {
        tiles.iter().map(|&i| tile(i, tw, th)).collect()
    };
    let col = draw(&l.col, l.cw, l.ch);
    let mut out = Vec::with_capacity(l.sh + l.rh);
    for r in 0..l.sh {
        let mut s = pane.get(r).cloned().unwrap_or_default();
        if !col.is_empty() {
            s = fit(s, l.sw);
            for d in &col {
                join_line(&mut s, fit(d.get(r).cloned().unwrap_or_default(), l.cw));
            }
        }
        out.push(s);
    }
    for (n, &i) in l.col.iter().enumerate() {
        g.at.push(Tile {
            key: m.agents[i].key.clone(),
            x: x + l.sw + n * l.cw,
            y,
            w: l.cw,
            h: l.ch,
        });
        g.col_x = x + l.sw;
    }
    let row = draw(&l.row, l.rw, l.rh);
    for r in 0..l.rh {
        let mut s = Line::new();
        for d in &row {
            join_line(&mut s, fit(d.get(r).cloned().unwrap_or_default(), l.rw));
        }
        out.push(s);
    }
    for (n, &i) in l.row.iter().enumerate() {
        g.at.push(Tile {
            key: m.agents[i].key.clone(),
            x: x + n * l.rw,
            y: y + l.sh,
            w: l.rw,
            h: l.rh,
        });
        g.row_y = y + l.sh;
    }
    out
}

fn join_line(l: &mut Line, o: Line) {
    l.spans.extend(o.spans);
}

/// What a click at (x, y) on screen does to the grid (gridClick).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    /// Not on the grid.
    Miss,
    /// On the edge: held, it resizes the tiles.
    Edge,
    /// On a tile: that agent is opened, the open one takes its place.
    Swap(String),
}

/// Which tile, or the edge, a click at (x, y) is on, and swaps the
/// tile's agent for the open one in the grid (gridClick). The caller
/// opens the agent Swap names.
pub fn click(m: &Model, g: &mut Grid, x: usize, y: usize) -> Click {
    let [bx, by, bw, bh] = g.pane;
    let row_end = if g.row_y > 0 { g.row_y } else { by + bh };
    if g.col_x > 0 && x == g.col_x && y >= by && y < row_end
        || g.col_x == 0 && g.row_y > 0 && y == g.row_y && x >= bx && x < bx + bw
    {
        g.drag = true;
        return Click::Edge;
    }
    g.focus.clear();
    let Some(t) =
        g.at.iter()
            .find(|t| x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h)
            .cloned()
    else {
        return Click::Miss;
    };
    if let Some(f) = m.focused() {
        let open = m.agents[f].key.clone();
        for k in &mut g.keys {
            if *k == t.key {
                *k = open.clone();
            }
        }
        let under = g.under.get(&t.key).copied().unwrap_or(false);
        g.under.insert(open, under); // it takes the tile's place
    }
    Click::Swap(t.key)
}

/// Moves the held edge to the pointer at (x, y): the tiles take what's
/// past it, the Session and the tiles each keeping the room they need
/// (gridDrag).
pub fn drag(g: &mut Grid, x: usize, y: usize) {
    let [bx, by, bw, bh] = g.pane.map(|v| v as f64);
    let (w, h) = (bw.max(1.0), bh.max(1.0));
    let row = g.col_x == 0;
    let (share, lo, hi) = if row {
        ((by + bh - y as f64) / h, MIN_H as f64 / h, (bh - 12.0) / h)
    } else {
        ((bx + bw - x as f64) / w, MIN_W as f64 / w, (bw - 60.0) / w)
    };
    g.share = share.max(lo).min(hi);
}
