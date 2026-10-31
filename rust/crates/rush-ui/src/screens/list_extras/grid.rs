//! The grid: agents pinned beside the open Session, or under it. A
//! pinned agent's Session is kept open when another is opened, and its
//! tile is that Session drawn small, its conversation and its own box; a
//! tile with no Session kept is drawn as the Wall draws one. A click on a
//! tile's box gives it the keys where it is; anywhere else on the tile
//! swaps that agent in, and the one it replaces takes the tile; the edge
//! between them is a handle that resizes the tiles. After Go's
//! internal/ui/grid.go (gridState :16, gridLayout :31, gridMinW/H :37,
//! gridAgents :43, gridPin :59, gridUnpin :83, gridSplit :98, gridJoin
//! :136, gridClick :201, gridAdopt :256, gridPark :277, gridClose :289,
//! withHost :305, gridSession :316, gridSave :334, gridDrag :347, gridKey
//! :364), the grid as it was left (model.go:485), laid round the Session
//! as view.go:1216-1244 does, and its clicks and drags (model.go:1496,
//! :1589, :1640). Kept in `Model::kept::<Grid>()`; the tiles' Sessions in
//! `Model::tiles`, whose host connections stay open with them (live.rs).

use std::collections::HashMap;

use rush_core::text::Line;
use rush_draw::{Button, MouseKind};

use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::session::SessionView;
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
    /// Taken from the config already (the grid as it was left).
    pub loaded: bool,
    /// The screen row each tile's box was drawn on, by agent key: a click
    /// from there down gives the tile the keys (gridClick's c.dockY).
    pub box_at: HashMap<String, usize>,
}

/// The grid, as it was left the first time it's asked for (model.go:485).
pub fn grid(m: &mut Model) -> &mut Grid {
    if !m.kept::<Grid>().loaded {
        let (keys, share) = (m.cfg.grid.clone(), m.cfg.grid_share);
        let under: HashMap<String, bool> = keys
            .iter()
            .map(|k| {
                let u = m.cfg.grid_below || m.cfg.grid_below_keys.contains(k);
                (k.clone(), u)
            })
            .collect();
        let g = m.kept::<Grid>();
        g.loaded = true;
        if g.keys.is_empty() {
            (g.keys, g.under, g.share) = (keys, under, share);
        }
    }
    m.kept::<Grid>()
}

/// Keeps the grid for the next start (gridSave): in rush's store too,
/// so a setting changed in Settings doesn't save the old grid back, and
/// on disk in order with every other change.
fn save(m: &mut Model) {
    let g = grid(m);
    let keys = g.keys.clone();
    let under: Vec<String> = keys
        .iter()
        .filter(|k| g.under.get(*k).copied().unwrap_or(false))
        .cloned()
        .collect();
    let share = g.share;
    (m.cfg.grid, m.cfg.grid_below_keys, m.cfg.grid_share) = (keys.clone(), under.clone(), share);
    m.cfg.grid_below = false; // GridBelowKeys says it now
    crate::config::save(m, "couldn't keep the grid", move |c| {
        (c.grid, c.grid_below_keys, c.grid_share) = (keys.clone(), under.clone(), share);
        c.grid_below = false;
    });
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

/// Pins the open agent as a tile, beside the Session or under it, and
/// gives the keys to the list to pick the agent the Session shows next
/// (gridPin).
pub fn pin(m: &mut Model, below: bool) {
    let Some(i) = m.focused() else {
        m.flash("open an agent first");
        return;
    };
    let key = m.agents[i].key.clone();
    let name = one_line(&m.agents[i].display_name);
    let g = grid(m);
    g.under.insert(key.clone(), below);
    if !g.keys.contains(&key) {
        g.keys.push(key);
    }
    if m.session.is_some() {
        super::split::leave_pane(m);
    }
    save(m);
    let where_ = if below { "under" } else { "beside" };
    m.flash(&format!(
        "{name} pinned {where_} the Session · pick the agent to open beside it"
    ));
}

/// Takes the newest tile off, or every tile with all (gridUnpin).
pub fn unpin(m: &mut Model, all: bool) {
    let g = grid(m);
    if g.keys.is_empty() {
        m.flash("no tiles pinned");
    } else if all {
        g.keys.clear();
    } else {
        g.keys.pop();
    }
    close(m);
    save(m);
}

/// Keeps v, a Session being let go, for its tile when its agent is
/// pinned; any other is let go (gridPark).
pub fn park(m: &mut Model, v: SessionView) {
    if grid(m).keys.contains(&v.key) {
        m.tiles.insert(v.key.clone(), v);
    }
}

/// The kept Session of key's tile, taken out to be the one shown; None
/// when it has none (gridAdopt).
pub fn adopt(m: &mut Model, key: &str) -> Option<SessionView> {
    let v = m.tiles.remove(key)?;
    let g = m.kept::<Grid>();
    if g.focus == key {
        g.focus.clear();
    }
    Some(v)
}

/// Lets go of the kept Sessions of tiles no longer pinned, and their
/// hosts' connections (gridClose).
pub fn close(m: &mut Model) {
    let keys = grid(m).keys.clone();
    let gone: Vec<String> = m
        .tiles
        .keys()
        .filter(|k| !keys.contains(k))
        .cloned()
        .collect();
    for k in gone {
        m.tiles.remove(&k);
        if crate::screens::session_core::live::showing(m).as_deref() != Some(k.as_str()) {
            crate::screens::session_core::live::drop_link(m, &k);
        }
    }
    let g = grid(m);
    if !g.keys.contains(&g.focus) {
        g.focus.clear();
    }
}

/// A tile's Session is being drawn or given a key as though it were the
/// open one (with_tile): which connection to keep open isn't decided
/// meanwhile (live::sync), or the open one's would be let go.
#[derive(Default)]
pub struct InTile(pub bool);

/// Runs f with key's kept Session as the open one, given the keys or not,
/// then puts it back in its tile and the one that was open back where it
/// was (withHost). None when key has none kept.
fn with_tile<T>(
    m: &mut Model,
    key: &str,
    focused: bool,
    f: impl FnOnce(&mut Model) -> T,
) -> Option<T> {
    let v = m.tiles.remove(key)?;
    let session = m.session.replace(v);
    let pane = m.pane.take();
    let sel = std::mem::replace(&mut m.sel, key.to_string());
    let shown = m.shown.clone();
    let idle = std::mem::replace(&mut m.pane_idle, !focused);
    let hint = crate::screens::session_core::core(m).hint.clone();
    let within = std::mem::replace(&mut m.kept::<InTile>().0, true);
    let out = f(m);
    m.kept::<InTile>().0 = within;
    // Whichever slot it ended in, it goes back to its tile.
    let back = m
        .session
        .take_if(|v| v.key == key)
        .or_else(|| m.pane.take_if(|v| v.key == key));
    if let Some(v) = back {
        m.tiles.insert(key.to_string(), v);
    }
    m.session = session;
    m.pane = pane;
    m.sel = sel;
    m.shown = shown;
    m.pane_idle = idle;
    crate::screens::session_core::core(m).hint = hint;
    Some(out)
}

/// Agent i's tile w×h, its top on screen row top: its kept Session drawn
/// small, a rule on its left (orange while its box has the keys), else
/// as the Wall draws it (gridJoin's draw, gridSession). The row its box
/// landed on is noted in box_at.
fn tile(
    m: &mut Model,
    box_at: &mut HashMap<String, usize>,
    focus: &str,
    i: usize,
    w: usize,
    h: usize,
    top: usize,
) -> Vec<Line> {
    let key = m.agents[i].key.clone();
    let focused = focus == key;
    // The open Session's spinner keeps going whatever the tile says.
    let spinning = m.spinning();
    let drawn = with_tile(m, &key, focused, |m| {
        let lines = m.tile_frame(top, h, w.saturating_sub(1));
        let edge = crate::screens::session_core::conn(m, &key)
            .drawn
            .lines()
            .first()
            .map(Line::plain);
        (lines, edge)
    });
    m.keep_spinning(spinning);
    box_at.remove(&key);
    let Some((mut lines, edge)) = drawn else {
        return super::wall::Wall::default().tile(m, i, w, h, false);
    };
    if let Some(e) = edge.filter(|e| !e.trim().is_empty())
        && let Some(r) = lines.iter().rposition(|l| l.plain().contains(e.trim()))
    {
        box_at.insert(key.clone(), top + r);
    }
    let rule = if focused {
        sp("┃", ORANGE)
    } else {
        sp("│", FAINT)
    };
    for l in &mut lines {
        l.spans.insert(0, rule.clone());
    }
    lines
}

/// A key while a tile's box has them: its Session takes it where it is,
/// as though it were open, the open one staying; esc in its empty box
/// hands them back (gridKey). None when no tile has them.
pub fn key(m: &mut Model, k: &Press) -> Option<Out> {
    let focus = m.kept.peek::<Grid>().map(|g| g.focus.clone())?;
    if focus.is_empty() || k.s == "tab" || m.sheet.is_some() || m.confirm.is_some() {
        return None;
    }
    if !grid(m).keys.contains(&focus) || !m.tiles.contains_key(&focus) {
        grid(m).focus.clear();
        return None;
    }
    let empty = crate::screens::session_core::conn(m, &focus)
        .input
        .is_empty()
        && m.tiles.get(&focus).is_some_and(|v| v.sel.is_empty());
    if k.s == "esc" && empty {
        grid(m).focus.clear();
        return Some(Out::Draw);
    }
    let full = m.w;
    let w = grid(m)
        .at
        .iter()
        .find(|t| t.key == focus)
        .map_or(full, |t| t.w.saturating_sub(1));
    with_tile(m, &focus, true, |m| {
        crate::screens::list_extras::split::in_pane(m, w, |m| m.session_press(k))
    })
}

/// The Session's pane, pw wide and rows tall, with the grid's tiles laid
/// round it: a column beside it, a row under it (view.go:1216-1244). The
/// pane is at (x, y) on screen; draw draws the Session w wide and h
/// tall, its margins in w. No tiles, or no room for them, leaves the
/// Session all of it.
pub fn round_pane(
    m: &mut Model,
    pw: usize,
    rows: usize,
    x: usize,
    y: usize,
    draw: impl FnOnce(&mut Model, usize, usize) -> Vec<Line>,
) -> Vec<Line> {
    let mut g = std::mem::take(grid(m));
    let mut l = split(m, &g, pw.saturating_sub(3), rows);
    let out = if l.col.is_empty() && l.row.is_empty() {
        g.at.clear();
        g.box_at.clear();
        (g.col_x, g.row_y) = (0, 0);
        draw(m, pw, rows)
    } else {
        l.sw += 3; // the Session's margins
        let pane = draw(m, l.sw, l.sh);
        // Each tile drawn first, where it goes: its Session's box is
        // found on screen for clicks.
        let mut drawn: HashMap<usize, Vec<Line>> = HashMap::new();
        let mut box_at = std::mem::take(&mut g.box_at);
        for &i in &l.col {
            drawn.insert(i, tile(m, &mut box_at, &g.focus, i, l.cw, l.ch, y));
        }
        for &i in &l.row {
            let t = tile(m, &mut box_at, &g.focus, i, l.rw, l.rh, y + l.sh);
            drawn.insert(i, t);
        }
        g.box_at = box_at;
        let get = |i: usize, _: usize, _: usize| drawn.get(&i).cloned().unwrap_or_default();
        join(m, &mut g, &pane, x, y, &l, &get)
    };
    *m.kept::<Grid>() = g;
    out
}

/// A click on a tile swaps its agent in, the open one pinned in its
/// place; on the edge, it's held, and dragging it resizes the tiles
/// (model.go:1496, :1589, :1640 gridClick, gridDrag).
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if m.on_place() || m.sheet.is_some() || m.confirm.is_some() {
        return Out::Pass;
    }
    let (x, y) = (e.x as usize, e.y as usize);
    match e.kind {
        MouseKind::Drag(Button::Left) if grid(m).drag => {
            drag(grid(m), x, y);
            Out::Draw
        }
        MouseKind::Up(Button::Left) if grid(m).drag => {
            grid(m).drag = false;
            save(m);
            Out::Draw
        }
        MouseKind::Down(Button::Left) => {
            let mut g = std::mem::take(grid(m));
            let c = click(m, &mut g, x, y);
            *m.kept::<Grid>() = g;
            match c {
                Click::Miss => Out::Pass,
                Click::Edge | Click::Focus => Out::Draw,
                Click::Swap(key) => {
                    if let Some(i) = m.agents.iter().position(|a| a.key == key) {
                        m.select(key);
                        m.rebuild();
                        save(m);
                        m.open(i);
                    }
                    Out::Draw
                }
            }
        }
        _ => Out::Pass,
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
    /// On a tile's box: the tile has the keys, where it is.
    Focus,
    /// On a tile: that agent is opened, the open one takes its place.
    Swap(String),
}

/// Which tile, or the edge, a click at (x, y) is on: on a kept Session's
/// box the tile takes the keys; elsewhere on a tile its agent is swapped
/// for the open one in the grid (gridClick). The caller opens the agent
/// Swap names.
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
    // A click on its box (its foot) gives the tile the keys, in place.
    if m.tiles.contains_key(&t.key) && g.box_at.get(&t.key).is_some_and(|&by| y >= by) {
        g.focus = t.key;
        return Click::Focus;
    }
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
