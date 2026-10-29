//! The Wall, Agents' second page: every agent at once, a tile each, what
//! it is saying and running streaming up its tile. A tile's edge is the
//! colour of its state and lights up while its transcript is written;
//! older lines fade. Only what's open is on it, unless a shows the
//! day's. After Go's internal/ui/wall.go (wallAgents :87, wallItems
//! :136, wallGroups :150, wallLayout :183, wallStacked :219, wallGeos
//! :259, wallStep :274, wallRows :300, wallGrid :308, wallBody :352,
//! wallScroll :396, wallOffscreen :407, wallEmpty :420, wallRowLines
//! :434, wallPick :472, wallLook :486, wallAgentTile :517, wallNames
//! :613, wallWhere :625, wallProject :646, wallFrameClose :665,
//! wallStream :776, wallDid :853, wallClip :885, wallSpread :901,
//! wallTick :914, onWallRead :967, wallHint :976, wallKey :1001,
//! wallClick :1059) and workstreams.go:277 (lanesLine). Not here yet:
//! subagents' own tiles (wallSubTile), which wait on the list knowing
//! each running subagent (fleet's SubagentTile), and ctrl+b's keep going,
//! which waits on rush running sessions itself.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rush_core::adapter::fill_of;
use rush_core::agent::{Agent, Preview, PreviewLine};
use rush_core::text::{Color, Line, Span, width};
use rush_core::time::Ms;
use rush_draw::{Button, MouseKind};
use rush_fleet::Predicates;

use crate::frame::places::Place;
use crate::frame::{Mouse, Out, Press};
use crate::list::{folder_key, folder_titles, project_tint};
use crate::model::Model;
use crate::style::*;

/// Tiles are at least this big; below it they page.
pub const MIN_W: usize = 40;
pub const MIN_H: usize = 9;
/// The least a subagent's tile is, stacked with others.
pub const SUB_MIN_H: usize = 5;
/// What a shows besides what's open: the last day's (workSince).
const WORK_SINCE: Ms = 24 * 3_600_000;
/// The window of transcript a tile reads (wallTick's 128 KiB).
const TILE_READ: i64 = 128 << 10;
/// How many tiles' transcripts are read at once.
const READS: usize = 24;
const EDGE: Color = crate::frame::inputbox::EDGE;

/// The Wall's state the list's readings touch: what a shows, and the
/// tiles' transcripts as last read, with their sizes.
#[derive(Default)]
pub struct WallState {
    pub all: bool,
    pub shown: bool,
    pub reads: HashMap<String, (Preview, u64)>,
    reading: HashSet<String>,
}

/// A tile as last drawn, for clicks.
#[derive(Clone, Debug)]
struct Tile {
    key: String,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

/// The Wall page: the tile picked, the rows scrolled past, and the tiles
/// as drawn.
#[derive(Default)]
pub struct Wall {
    pub sel: String,
    top: usize,
    tiles: Vec<Tile>,
    above: usize,
    below: usize,
    /// The body's size last drawn, for the arrows' geometry.
    size: (usize, usize),
    /// A live tile was drawn: its spinner moves.
    moving: bool,
    last_click: Option<Instant>,
    names: HashMap<String, String>,
}

/// The Wall's tiles, agents by their place in Model::agents, in an order
/// that doesn't shuffle as they work: what needs you first, then what's
/// at work, then the rest, each by when it started (wallAgents).
pub fn wall_agents(m: &Model, all: bool) -> Vec<usize> {
    let now = m.now;
    let mut out: Vec<usize> = m
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| !a.past)
        .filter(|(_, a)| {
            let open = a.job.live() || a.busy() || a.pid != 0 || a.halted() || a.your_turn(now);
            open || all && now - a.job.updated_at < WORK_SINCE
        })
        .map(|(i, _)| i)
        .collect();
    let rank = |a: &Agent| {
        if a.needs_you() || a.halted() {
            0
        } else if a.job.live() {
            1
        } else if a.busy() {
            2
        } else if a.your_turn(now) {
            3
        } else if a.pid != 0 {
            4
        } else {
            5
        }
    };
    out.sort_by(|&i, &j| {
        let (a, b) = (&m.agents[i], &m.agents[j]);
        rank(a)
            .cmp(&rank(b))
            .then(a.job.created_at.cmp(&b.job.created_at))
    });
    out
}

/// A tile's place in a row of the Wall: its column, and which of the n
/// stacked in that column it is (wallCell).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub item: usize,
    pub col: usize,
    pub k: usize,
    pub n: usize,
}

/// A row of tiles; joined[c] is set where the tiles either side of the
/// gap after column c are one agent's, so their top edges join (wallRow).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub joined: Vec<bool>,
}

impl Row {
    fn new(cols: usize) -> Row {
        Row {
            cells: Vec::new(),
            joined: vec![false; cols],
        }
    }
}

/// Groups of tiles in rows cols wide: a group that fits a row sits in it
/// side by side; a bigger one has rows of its own, the agent down its
/// first column and its subagents stacked up to stack high (wallLayout).
pub fn layout(groups: &[usize], cols: usize, stack: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut cur = Row::new(cols);
    let mut col = 0;
    let mut i = 0;
    for &g in groups {
        if g <= cols {
            if col + g > cols {
                if !cur.cells.is_empty() {
                    rows.push(std::mem::replace(&mut cur, Row::new(cols)));
                }
                col = 0;
            }
            for j in 0..g {
                cur.cells.push(Cell {
                    item: i + j,
                    col,
                    k: 0,
                    n: 1,
                });
                if j > 0 {
                    cur.joined[col - 1] = true;
                }
                col += 1;
            }
            i += g;
            continue;
        }
        if !cur.cells.is_empty() {
            rows.push(std::mem::replace(&mut cur, Row::new(cols)));
        }
        col = 0;
        rows.extend(stacked(i, g, cols, stack));
        i += g;
    }
    if !cur.cells.is_empty() {
        rows.push(cur);
    }
    rows
}

/// The rows of a group too big for one row side by side (wallStacked).
fn stacked(i: usize, g: usize, cols: usize, stack: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut cur = Row::new(cols);
    cur.cells.push(Cell {
        item: i,
        col: 0,
        k: 0,
        n: 1,
    });
    let mut start = 1;
    if cols == 1 {
        rows.push(std::mem::replace(&mut cur, Row::new(cols)));
        start = 0;
    }
    let sc = cols - start;
    let (mut next, mut left) = (i + 1, g - 1);
    while left > 0 {
        let take = left.min(sc * stack);
        let used = take.min(sc);
        for c in 0..used {
            let n = take / used + usize::from(c < take % used);
            for k in 0..n {
                cur.cells.push(Cell {
                    item: next,
                    col: start + c,
                    k,
                    n,
                });
                next += 1;
            }
        }
        let from = if cur.cells[0].item == i { 0 } else { start };
        for c in from..(start + used).saturating_sub(1) {
            cur.joined[c] = true;
        }
        left -= take;
        rows.push(std::mem::replace(&mut cur, Row::new(cols)));
    }
    rows
}

/// Where a tile is for moving between them: its column, and its top and
/// bottom in rows of GEO_H (wallGeo).
#[derive(Clone, Copy, Debug, Default)]
pub struct Geo {
    col: usize,
    top: usize,
    bot: usize,
}

const GEO_H: usize = 60;

pub fn geos(layout: &[Row], n: usize) -> Vec<Geo> {
    let mut g = vec![Geo::default(); n];
    for (r, row) in layout.iter().enumerate() {
        for c in &row.cells {
            if c.item < n {
                g[c.item] = Geo {
                    col: c.col,
                    top: r * GEO_H + c.k * GEO_H / c.n,
                    bot: r * GEO_H + (c.k + 1) * GEO_H / c.n,
                };
            }
        }
    }
    g
}

/// The tile next to tile i the way dx or dy goes: the nearest above or
/// below, in the nearest column; beside, overlapping it (wallStep).
pub fn step(geo: &[Geo], i: usize, dx: isize, dy: isize) -> Option<usize> {
    let cur = geo[i];
    let mut best: Option<(usize, isize)> = None;
    for (j, g) in geo.iter().enumerate() {
        let col_d = (g.col as isize - cur.col as isize).abs();
        let d = if j == i {
            continue;
        } else if dy < 0 && g.bot <= cur.top {
            (cur.top - g.bot) as isize * 100 + col_d
        } else if dy > 0 && g.top >= cur.bot {
            (g.top - cur.bot) as isize * 100 + col_d
        } else if dx != 0
            && (g.col as isize - cur.col as isize).signum() == dx
            && g.top < cur.bot
            && g.bot > cur.top
        {
            col_d * 1000 + (g.top as isize - cur.top as isize).abs()
        } else {
            continue;
        };
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((j, d));
        }
    }
    best.map(|(j, _)| j)
}

/// The rows groups take cols wide in h: fewer subagents stack in a column
/// when the rows are shorter (wallRows).
fn rows_in(groups: &[usize], cols: usize, h: usize) -> Vec<Row> {
    let l = layout(groups, cols, 1.max(h / SUB_MIN_H));
    layout(groups, cols, 1.max(h / l.len().max(1) / SUB_MIN_H))
}

/// Groups of tiles laid out in w×h: the columns, the rows that fit on
/// screen, and each row's height, tiles about three and a half times as
/// wide as tall, wasting few (wallGrid).
pub fn grid(groups: &[usize], w: usize, h: usize) -> (usize, usize, usize) {
    let n: usize = groups.iter().sum();
    if n == 0 || w == 0 || h == 0 {
        return (1, 1, h);
    }
    let max_cols = 1.max((w + 1) / (MIN_W + 1));
    let (mut best, mut best_rows, mut best_score) = (0, 0, f64::INFINITY);
    for c in 1..=max_cols.min(n) {
        let l = rows_in(groups, c, h);
        let r = l.len();
        let th = h / r;
        if th < MIN_H {
            continue;
        }
        let used: usize = l
            .iter()
            .map(|row| row.cells.iter().map(|c| c.col).collect::<HashSet<_>>().len())
            .sum();
        let tw = (w as isize - (c as isize - 1)) / c as isize;
        let score = (tw as f64 / th as f64 / 3.5).ln().abs()
            + 1.5 * (r * c - used) as f64 / n as f64;
        if score < best_score {
            (best, best_rows, best_score) = (c, r, score);
        }
    }
    if best > 0 {
        return (best, best_rows, h / best_rows);
    }
    // They don't all fit: as many columns as there's room for, and pages
    // of rows.
    let rows = 1.max(h / MIN_H);
    (max_cols.min(n), rows, h / rows)
}

/// How many subagents stack in a column in rows tile_h tall (wallStack).
pub fn stack(tile_h: usize) -> usize {
    1.max(tile_h / SUB_MIN_H)
}

/// The tiles in rows above top, and below the rows shown from it
/// (wallOffscreen).
fn offscreen(row_of: &[usize], top: usize, rows: usize) -> (usize, usize) {
    let above = row_of.iter().filter(|&&r| r < top).count();
    let below = row_of.iter().filter(|&&r| r >= top + rows).count();
    (above, below)
}

/// What a busy agent still runs (lanesLine).
pub fn lanes_line(a: &Agent) -> String {
    let (mut agents, mut other) = (0, 0);
    for b in &a.job.background {
        if b.starts_with("agent\0") {
            agents += 1;
        } else {
            other += 1;
        }
    }
    let s = |n: usize| if n == 1 { "" } else { "s" };
    let mut parts = Vec::new();
    if agents > 0 {
        parts.push(format!("{agents} subagent{} running", s(agents)));
    }
    if other > 0 {
        parts.push(format!("{other} task{} running", s(other)));
    }
    if parts.is_empty() {
        return "background work running".into();
    }
    parts.join(", ")
}

/// An agent's spinner frame: its own where its program has one
/// (convo.Spin).
pub fn spin(kind: &str, tick: usize) -> &'static str {
    const CODEX: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const OLLAMA: [&str; 4] = ["◐", "◓", "◑", "◒"];
    const PI: [&str; 4] = ["◜", "◝", "◞", "◟"];
    const GEMINI: [&str; 5] = ["✦", "✧", "✦", "✧", "·"];
    let fs: &[&str] = match kind {
        "codex" => &CODEX,
        k if k.starts_with("ollama") => &OLLAMA,
        "pi" => &PI,
        "gemini" => &GEMINI,
        _ => &SPINNER,
    };
    fs[tick % fs.len()]
}

/// The bar of how full a context is (ctxBar).
pub fn ctx_bar(pct: f64) -> Line {
    let n = ((pct / 12.5 + 0.5).max(0.0) as usize).min(8);
    let c = if pct >= 80.0 {
        RED
    } else if pct >= 50.0 {
        YELLOW
    } else {
        SUB
    };
    line([sp("━".repeat(n), c), sp("━".repeat(8 - n), FAINT)])
}

/// l on the left of w cells and r on the right, l cut first when they
/// don't both fit (wallSpread).
pub fn spread(l: Line, r: Line, w: usize) -> Line {
    let rw = r.width();
    if rw == 0 || rw + 2 > w {
        return fit(l, w);
    }
    let mut out = fit(l, w - rw - 1);
    out.spans.push(plain(" "));
    join(&mut out, r);
    out
}

/// The first n of ls, the last of them ending … when more were cut
/// (wallClip).
fn clip(mut ls: Vec<String>, n: usize, w: usize) -> Vec<String> {
    if ls.len() <= n {
        return ls;
    }
    ls.truncate(n);
    let last = ls.pop().unwrap_or_default();
    ls.push(format!(
        "{}…",
        rush_draw::truncate(&last, w.saturating_sub(1), "")
    ));
    ls
}

/// One of a tile's steps on a line: a tool call in words, or something
/// said or asked before the latest. back is how many steps came after
/// it: the newest three bright, the next six dim, the rest faint
/// (wallDid).
pub fn did(e: &PreviewLine, back: usize, live: bool, w: usize) -> Line {
    let ink = |c: Color| {
        if back >= 9 {
            FAINT
        } else if back >= 3 && c == TEXT {
            SUB
        } else if back >= 3 {
            DIM
        } else {
            c
        }
    };
    let cut = |s: &str| rush_draw::truncate(s, w.saturating_sub(2), "…").into_owned();
    match e.role.as_str() {
        "user" => line([sp(format!("❯ {}", cut(&one_line(&e.text))), ink(BLUE))]),
        "tool" => {
            let dot = if back == 0 && live { ORANGE } else { ink(SUB) };
            let mut d = one_line(&tildify(&e.doing));
            if d.is_empty() {
                let (tool, arg) = e.text.split_once('\0').unwrap_or((&e.text, ""));
                d = format!("{tool} {}", one_line(&tildify(arg)))
                    .trim()
                    .to_string();
            }
            line([sp("● ", dot), sp(cut(&d), ink(TEXT))])
        }
        _ => line([
            plain("  "),
            sp(cut(&unmark(&one_line(&e.text))), ink(SUB)),
        ]),
    }
}

/// A tile's body, room lines of w: what it was asked last, what it's
/// saying now (only when it's said something since), a rule, and under
/// it every step it took, newest at the bottom, fading as they age; live
/// colours the latest step's dot; intent is what it was asked until the
/// transcript says (wallStream).
pub fn stream(p: &Preview, live: bool, intent: &str, w: usize, room: usize) -> Vec<Line> {
    if room == 0 {
        return Vec::new();
    }
    let mut ask = intent.to_string();
    let mut at = None;
    for (i, e) in p.recent.iter().enumerate().rev() {
        if e.role == "user" {
            ask = e.text.clone();
            at = Some(i);
            break;
        }
    }
    if at.is_none() && !p.last_user.is_empty() {
        ask = p.last_user.clone();
    }
    let from = at.map_or(0, |a| a + 1);
    let say = (from..p.recent.len())
        .rev()
        .find(|&i| p.recent[i].role == "assistant");
    if ask.is_empty() && p.recent.is_empty() {
        return vec![line([sp("…", FAINT)])];
    }
    // The head: the prompt, two lines at most, and the message, up to
    // four; fewer of each when the tile is short.
    let (ask_n, say_n) = if room < 5 {
        (1, 1)
    } else if room < 10 {
        (1, 2)
    } else {
        (2, 4)
    };
    let tw = w.saturating_sub(2);
    let mut head = Vec::new();
    if !ask.is_empty() {
        for (i, l) in clip(wrap(&one_line(&ask), tw), ask_n, tw)
            .into_iter()
            .enumerate()
        {
            let lead = if i == 0 {
                sp("❯ ", BLUE)
            } else {
                plain("  ")
            };
            head.push(line([lead, sp(l, BLUE)]));
        }
    }
    if let Some(s) = say {
        let text = unmark(&one_line(&p.recent[s].text));
        for l in clip(wrap(&text, tw), say_n, tw) {
            head.push(line([plain("  "), sp(l, TEXT)]));
        }
    }
    head.truncate(room);
    let steps: Vec<&PreviewLine> = p
        .recent
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != at && Some(*i) != say)
        .map(|(_, e)| e)
        .collect();
    let mut left = room - head.len();
    if !head.is_empty() {
        left = left.saturating_sub(1); // the rule
    }
    if steps.is_empty() || left < 1 {
        return head;
    }
    let mut out = Vec::new();
    for back in 0..steps.len().min(left) {
        out.push(did(steps[steps.len() - 1 - back], back, live, w));
    }
    out.reverse();
    if head.is_empty() {
        return out;
    }
    head.push(line([sp("╌".repeat(w), FAINT)]));
    head.extend(out);
    head
}

/// A finished tile: top (drawn already), inner between the side edges,
/// and the bottom edge (wallFrameClose).
fn frame_close(top: Line, inner: Vec<Line>, edge: Color, w: usize, h: usize, heavy: bool) -> Vec<Line> {
    let (bl, br, hz, vt) = if heavy {
        ("┗", "┛", "━", "┃")
    } else {
        ("╰", "╯", "─", "│")
    };
    let mut out = Vec::with_capacity(h);
    out.push(top);
    for l in inner.into_iter().take(h.saturating_sub(2)) {
        let mut r = line([sp(vt, edge), plain(" ")]);
        join(&mut r, l);
        r.spans.push(plain(" "));
        r.spans.push(sp(vt, edge));
        out.push(r);
    }
    out.push(line([sp(
        format!("{bl}{}{br}", hz.repeat(w.saturating_sub(2))),
        edge,
    )]));
    out
}

impl Wall {
    /// The tile picked: the one by sel, else the first (wallPick).
    fn pick(&mut self, m: &Model, items: &[usize]) -> usize {
        if let Some(i) = items.iter().position(|&i| m.agents[i].key == self.sel) {
            return i;
        }
        if let Some(&i) = items.first() {
            self.sel = m.agents[i].key.clone();
        }
        0
    }

    /// Keeps row, the picked tile's, among the rows shown (wallScroll).
    fn scroll(&mut self, row: usize, all: usize, rows: usize) {
        if row < self.top {
            self.top = row;
        } else if row >= self.top + rows {
            self.top = row + 1 - rows;
        }
        self.top = self.top.min(all.saturating_sub(rows));
    }

    /// How a tile shows its agent's state: its edge's colour, the glyph by
    /// its name, and the word for it (wallLook).
    fn look(&self, m: &Model, a: &Agent) -> (Color, Span, Span) {
        let now = m.now;
        if a.needs_you() {
            (YELLOW, spb("●", YELLOW), spb("needs you", YELLOW))
        } else if a.halted() {
            (RED, sp("✕", RED), sp("stopped", RED))
        } else if a.job.live() {
            let g = spin(a.kind.as_str(), m.tick + a.job.id.len());
            (ORANGE, sp(g, ORANGE), sp("working", ORANGE))
        } else if a.busy() {
            (QUEUE, sp("◌", QUEUE), sp(lanes_line(a), QUEUE))
        } else if a.your_turn(now) {
            (GREEN, sp("●", GREEN), sp("your turn", GREEN))
        } else if a.pid != 0 {
            (EDGE, sp("○", SUB), sp("idle", DIM))
        } else {
            (
                FAINT,
                sp("·", FAINT),
                sp(format!("{} ago", age(a.age(now))), FAINT),
            )
        }
    }

    /// The folder of agent i's project: its checkout when the main one
    /// isn't known (wallProject).
    fn project<'a>(m: &'a Model, i: usize) -> &'a str {
        let a = &m.agents[i];
        let root = m.roots.get(i).map_or("", String::as_str);
        if root.is_empty() && !a.repo.is_empty() {
            return &a.repo;
        }
        folder_key(a, root)
    }

    /// Where a tile's agent works: its project in its own colour, the
    /// worktree it's in, the branch, and, faint, the folder the project's
    /// in (wallWhere).
    fn place(&self, m: &Model, i: usize) -> Line {
        let a = &m.agents[i];
        let key = Self::project(m, i);
        let base = key.rsplit('/').next().unwrap_or(key);
        let name = self
            .names
            .get(key)
            .filter(|n| !n.is_empty())
            .map_or(base, String::as_str);
        let mut out = line([spb(name.to_string(), project_tint(key))]);
        let root = m.roots.get(i).map_or("", String::as_str);
        if !root.is_empty() && a.repo != root {
            out.spans.push(sp(" › ", FAINT));
            out.spans.push(sp(
                a.repo.rsplit('/').next().unwrap_or(&a.repo).to_string(),
                SUB,
            ));
        }
        if !a.branch.is_empty() {
            out.spans.push(sp(" ⎇ ", FAINT));
            out.spans.push(sp(a.branch.clone(), DIM));
        }
        if key.starts_with('/') {
            let dir = key.rsplit_once('/').map_or("/", |(d, _)| if d.is_empty() { "/" } else { d });
            out.spans.push(sp(format!("  {}", tildify(dir)), FAINT));
        }
        out
    }

    /// One agent's tile, w×h, its edge heavy when it's picked
    /// (wallAgentTile).
    fn tile(&self, m: &Model, i: usize, w: usize, h: usize, picked: bool) -> Vec<Line> {
        if w < 8 || h < 3 {
            return Vec::new();
        }
        let a = &m.agents[i];
        let now = m.now;
        let empty = Preview::default();
        let p = preview(m, &a.key).unwrap_or(&empty);
        let (mut edge, glyph, word) = self.look(m, a);
        // Its transcript was written in the last couple of seconds: the
        // edge flashes bright.
        if p.at != 0 && now - p.at < 2000 && (a.job.live() || a.busy()) {
            edge = BRIGHT;
        }
        let (tl, tr, hz) = if picked {
            ("┏", "┓", "━")
        } else {
            ("╭", "╮", "─")
        };
        let name_c = if picked { BRIGHT } else { TEXT };
        let iw = w - 4;

        // The top edge: the name and, on the right, the model and age.
        let model_id = if p.model.is_empty() {
            &a.spend.model
        } else {
            &p.model
        };
        let mut model = m.reg.model_name(a.kind.as_str(), model_id);
        let native = m.reg.native_agent().is_some_and(|n| n.kind() == a.kind);
        if !native {
            model = format!("{} {model}", m.reg.label(a.kind.as_str()))
                .trim()
                .to_string();
        }
        let mut meta = [model, age(a.age(now))]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let mut room_name = w as isize - 12 - width(&meta) as isize;
        if room_name < 8 {
            meta.clear();
            room_name = w as isize - 10;
        }
        let name = one_line(&a.display_name);
        let mut title = line([glyph.clone(), plain(" ")]);
        title.spans.push(Span::new(
            rush_draw::truncate(&name, room_name.max(1) as usize, "…").into_owned(),
            bold(name_c),
        ));
        let right = if meta.is_empty() {
            Line::new()
        } else {
            line([plain(" "), sp(meta, DIM), plain(" ")])
        };
        let fill = w as isize - 6 - title.width() as isize - right.width() as isize;
        let mut top = line([sp(format!("{tl}{hz}"), edge), plain(" ")]);
        join(&mut top, title);
        top.spans.push(plain(" "));
        top.spans.push(sp(hz.repeat(fill.max(0) as usize), edge));
        join(&mut top, right);
        top.spans.push(sp(format!("{hz}{tr}"), edge));
        let top = fit(top, w);

        let mut inner = Vec::with_capacity(h.saturating_sub(2));
        // Where: its project, worktree and branch, and how full its
        // context is.
        let mut ctx = Line::new();
        if p.context > 0 {
            let win = m.reg.context_window(a.kind.as_str(), &p.model);
            let pct = fill_of(p.context, win, Default::default()).pct();
            ctx = ctx_bar(pct);
            ctx.spans.push(sp(format!(" {pct:2.0}%"), DIM));
        }
        inner.push(spread(self.place(m, i), ctx, iw));

        // The bottom line: what it's doing now, what it costs, its todos.
        let first = |xs: &[&str]| {
            xs.iter()
                .find(|s| !s.is_empty())
                .map(|s| one_line(s))
                .unwrap_or_default()
        };
        let now1 = if a.needs_you() {
            line([
                word,
                sp(" · ", DIM),
                sp(first(&[&a.job.needs, &a.job.detail, &p.text]), TEXT),
            ])
        } else if a.halted() {
            let t = a.spend.halt.as_ref().map(|h| h.text.as_str()).unwrap_or("");
            line([word, sp(" · ", DIM), sp(one_line(t), SUB)])
        } else if a.job.live() && !p.doing.is_empty() {
            line([glyph, plain(" "), sp(one_line(&tildify(&p.doing)), TEXT)])
        } else if a.job.live() && !a.job.detail.is_empty() {
            line([glyph, plain(" "), sp(one_line(&a.job.detail), TEXT)])
        } else {
            line([word])
        };
        let mut stats = Line::new();
        let mut stat = |s: Span| {
            if !stats.spans.is_empty() {
                stats.spans.push(plain(" "));
            }
            stats.spans.push(s);
        };
        let todos = a.job.todos.len();
        if todos > 0 {
            let done = a.job.todos.iter().filter(|t| t.done).count();
            let c = if done == todos { GREEN } else { SUB };
            stat(sp(format!("☑ {done}/{todos}"), c));
        }
        let n = a.subs.direct + a.subs.nested;
        if n > 0 {
            stat(sp(format!("⑂{n}"), QUEUE));
        }
        if a.spend.cost > 0.0 {
            stat(sp(money_short(a.spend.cost), DIM));
        }
        let foot = spread(now1, stats, iw);

        // Between them, the stream.
        let room = h as isize - 4;
        if room > 0 {
            let room = room as usize;
            let s = stream(p, a.job.live(), &a.job.intent, iw, room);
            let n = s.len();
            inner.extend(s.into_iter().map(|l| fit(l, iw)));
            for _ in n..room {
                inner.push(plain_line(iw));
            }
        }
        inner.push(foot);
        frame_close(top, inner, edge, w, h, picked)
    }

    /// A row of tiles th tall, its top y rows down the body, noting
    /// where each tile is for clicks (wallRowLines).
    #[allow(clippy::too_many_arguments)]
    fn row_lines(
        &mut self,
        m: &Model,
        row: &Row,
        items: &[usize],
        col_w: &[usize],
        col_x: &[usize],
        th: usize,
        y: usize,
        pick: usize,
    ) -> Vec<Line> {
        let cols = col_w.len();
        let mut lines: Vec<Vec<Line>> = vec![Vec::new(); cols];
        for c in &row.cells {
            let (mut ch, y0) = (th / c.n, c.k * (th / c.n));
            if c.k == c.n - 1 {
                ch = th - y0;
            }
            let mut t = self.tile(m, items[c.item], col_w[c.col], ch, c.item == pick);
            while t.len() < ch {
                t.push(plain_line(col_w[c.col]));
            }
            lines[c.col].extend(t);
            self.tiles.push(Tile {
                key: m.agents[items[c.item]].key.clone(),
                x: col_x[c.col],
                y: y + y0,
                w: col_w[c.col],
                h: ch,
            });
        }
        let mut out = Vec::with_capacity(th);
        for ly in 0..th {
            let mut l = Line::new();
            for c in 0..cols {
                if c > 0 {
                    if ly == 0 && row.joined[c - 1] {
                        l.spans.push(sp("─", SUB));
                    } else {
                        l.spans.push(plain(" "));
                    }
                }
                match lines[c].get(ly) {
                    Some(t) => join(&mut l, t.clone()),
                    None => l.spans.push(plain(blanks(col_w[c]))),
                }
            }
            out.push(l);
        }
        out
    }

    /// The body when there's nothing to show (wallEmpty).
    fn empty(m: &Model, w: usize, h: usize) -> Vec<Line> {
        let mut out = vec![Line::new(); h];
        let all = m.kept.peek::<WallState>().is_some_and(|s| s.all);
        let mut msg = line([sp("nothing open right now", DIM)]);
        if !all {
            msg.spans.push(sp("  ·  a shows the last day's", FAINT));
        }
        if h > 0 {
            let mut l = line([plain(blanks(w.saturating_sub(msg.width()) / 2))]);
            join(&mut l, msg);
            out[h / 2] = l;
        }
        out
    }

    /// The tiles' geometry at the size last drawn, for the arrows.
    fn geometry(&self, n: usize) -> Vec<Geo> {
        let groups = vec![1; n];
        let (w, h) = self.size;
        let (cols, _, tile_h) = grid(&groups, w, h);
        geos(&layout(&groups, cols, stack(tile_h)), n)
    }

    /// Opens agent key's conversation from the Wall (goAgent).
    fn go(m: &mut Model, key: &str) -> Out {
        let Some(i) = m.agents.iter().position(|a| a.key == key) else {
            return Out::Idle;
        };
        m.sel = key.to_string();
        m.open(i);
        Out::Close
    }
}

fn plain_line(w: usize) -> Line {
    line([plain(blanks(w))])
}

/// A tile's preview: the Wall's own read, else the list's.
fn preview<'a>(m: &'a Model, key: &str) -> Option<&'a Preview> {
    let mine = m
        .kept
        .peek::<WallState>()
        .and_then(|s| s.reads.get(key))
        .map(|(p, _)| p);
    match (mine, m.previews.get(key)) {
        (Some(a), Some(b)) if b.at > a.at => Some(b),
        (Some(a), _) => Some(a),
        (None, b) => b,
    }
}

/// Reads the tiles' transcripts that grew, off the UI thread, all in one
/// round (wallTick, onWallRead).
pub fn read(m: &mut Model) {
    let all = m.kept::<WallState>().all;
    let items = wall_agents(m, all);
    let mut wants = Vec::new();
    {
        let st = m.kept.get::<WallState>();
        for &i in &items {
            let a = &m.agents[i];
            if a.job.transcript_path.is_empty() || st.reading.contains(&a.key) || wants.len() >= READS {
                continue;
            }
            st.reading.insert(a.key.clone());
            let had = st.reads.get(&a.key).map(|(_, n)| *n);
            wants.push((
                a.key.clone(),
                a.job.transcript_path.clone(),
                a.kind.to_string(),
                had,
            ));
        }
    }
    if wants.is_empty() {
        return;
    }
    let reg = m.reg.clone();
    m.later(
        move || {
            wants
                .into_iter()
                .map(|(key, path, kind, had)| {
                    let size = std::fs::metadata(&path).map(|md| md.len()).ok();
                    let got = match size {
                        Some(n) if Some(n) != had => {
                            Some((reg.read_preview(&kind, &path, TILE_READ), n))
                        }
                        _ => None,
                    };
                    (key, got)
                })
                .collect::<Vec<_>>()
        },
        |m, got| {
            let st = m.kept::<WallState>();
            let mut changed = false;
            for (key, g) in got {
                st.reading.remove(&key);
                if let Some(e) = g {
                    st.reads.insert(key, e);
                    changed = true;
                }
            }
            if changed { Out::Draw } else { Out::Idle }
        },
    );
}

/// After each reading of the list: the tiles' transcripts read again
/// while the Wall shows.
pub(super) fn on_fleet(m: &mut Model) {
    if m.kept::<WallState>().shown {
        // Agents gone take their reads with them.
        let keys: HashSet<String> = m.agents.iter().map(|a| a.key.clone()).collect();
        m.kept::<WallState>().reads.retain(|k, _| keys.contains(k));
        read(m);
    }
}

impl Place for Wall {
    fn name(&self) -> &str {
        "Wall"
    }

    fn enter(&mut self, m: &mut Model) {
        m.kept::<WallState>().shown = true;
        read(m);
    }

    fn leave(&mut self, m: &mut Model) {
        let st = m.kept::<WallState>();
        st.shown = false;
        st.reads.clear();
    }

    /// The tiles in w×h (wallBody).
    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        // Go draws it m.w-4 wide (view.go:643): a margin either side.
        let w = w.saturating_sub(2);
        let all = m.kept.peek::<WallState>().is_some_and(|s| s.all);
        let items = wall_agents(m, all);
        self.tiles.clear();
        (self.above, self.below) = (0, 0);
        self.size = (w, h);
        self.moving = false;
        if items.is_empty() {
            return Self::empty(m, w, h);
        }
        let keys: Vec<&str> = items.iter().map(|&i| Self::project(m, i)).collect();
        self.names = folder_titles(keys.into_iter());
        let groups = vec![1; items.len()];
        let (cols, rows, tile_h) = grid(&groups, w, h);
        let lay = layout(&groups, cols, stack(tile_h));
        let mut row_of = vec![0; items.len()];
        for (r, row) in lay.iter().enumerate() {
            for c in &row.cells {
                row_of[c.item] = r;
            }
        }
        let pick = self.pick(m, &items);
        let rows = rows.min(lay.len());
        self.scroll(row_of[pick], lay.len(), rows);
        (self.above, self.below) = offscreen(&row_of, self.top, rows);

        let tile_w = (w - (cols - 1)) / cols;
        let extra = w - (cols - 1) - tile_w * cols;
        let (mut col_w, mut col_x) = (vec![0; cols], vec![0; cols]);
        let mut x = 0;
        for c in 0..cols {
            col_w[c] = tile_w + usize::from(c < extra);
            col_x[c] = x;
            x += col_w[c] + 1;
        }
        let mut out = Vec::with_capacity(h);
        for r in 0..rows {
            let th = if r == rows - 1 {
                h - tile_h * (rows - 1)
            } else {
                tile_h
            };
            let row = lay[self.top + r].clone();
            let y = out.len();
            for c in &row.cells {
                let a = &m.agents[items[c.item]];
                self.moving |= a.job.live() || a.busy();
            }
            out.extend(self.row_lines(m, &row, &items, &col_w, &col_x, th, y, pick));
        }
        out
    }

    /// The keys: arrows between tiles, enter opens one, a shows the
    /// day's, esc back (wallHint).
    fn hint(&self, m: &Model, w: usize) -> Line {
        let all = m.kept.peek::<WallState>().is_some_and(|s| s.all);
        let all = if all { "only what's open" } else { "the last day's" };
        let go = match m.agents.iter().find(|a| a.key == self.sel) {
            Some(a) if a.halted() => "continue",
            _ => "keep going",
        };
        let pairs = [
            ("←↑↓→", "move"),
            ("enter", "open"),
            ("a", all),
            ("ctrl+b", go),
            ("esc", "back"),
        ];
        let w = w.saturating_sub(2);
        let mut more = Line::new();
        if self.above > 0 {
            more.spans.push(sp(format!("↑ {} above", self.above), SUB));
        }
        if self.below > 0 {
            if !more.spans.is_empty() {
                more.spans.push(sp(" · ", DIM));
            }
            more.spans.push(sp(format!("↓ {} more", self.below), SUB));
        }
        if more.spans.is_empty() {
            return keys_fit(w, &pairs);
        }
        let mw = more.width();
        more.spans.push(plain("   "));
        join(&mut more, keys_fit(w.saturating_sub(mw + 3), &pairs));
        more
    }

    /// The Wall's keys (wallKey).
    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let s = k.s.as_str();
        let all = m.kept::<WallState>().all;
        let items = wall_agents(m, all);
        let toggle = |m: &mut Model| {
            let st = m.kept::<WallState>();
            st.all = !st.all;
            read(m);
            Out::Draw
        };
        if items.is_empty() {
            return match s {
                "a" => toggle(m),
                "esc" | "q" => Out::Close,
                _ => Out::Pass,
            };
        }
        let i = self.pick(m, &items);
        let geo = self.geometry(items.len());
        let mut moved = |dx: isize, dy: isize| {
            let j = step(&geo, i, dx, dy).or_else(|| {
                let j = i as isize + dx;
                (dx != 0 && j >= 0 && (j as usize) < items.len()).then_some(j as usize)
            });
            if let Some(j) = j {
                self.sel = m.agents[items[j]].key.clone();
            }
            Out::Draw
        };
        match s {
            "esc" | "q" => Out::Close,
            "left" | "h" => moved(-1, 0),
            "right" | "l" => moved(1, 0),
            "up" | "k" => moved(0, -1),
            "down" | "j" => moved(0, 1),
            "home" | "g" => {
                self.sel = m.agents[items[0]].key.clone();
                Out::Draw
            }
            "end" | "G" => {
                self.sel = m.agents[items[items.len() - 1]].key.clone();
                Out::Draw
            }
            "a" => toggle(m),
            "enter" => {
                let key = m.agents[items[i]].key.clone();
                Self::go(m, &key)
            }
            "ctrl+b" | "alt+g" => {
                m.flash_err("rush can't tell agents to keep going from here yet");
                Out::Draw
            }
            _ => Out::Pass,
        }
    }

    /// A click picks the tile under it; a second opens it (wallClick).
    fn mouse(&mut self, m: &mut Model, e: &Mouse, x: usize, y: usize) -> Out {
        if e.kind != MouseKind::Down(Button::Left) {
            return Out::Idle;
        }
        // The body starts under the header's rule.
        let Some(t) = self
            .tiles
            .iter()
            .find(|t| x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h)
            .cloned()
        else {
            return Out::Idle;
        };
        let double = t.key == self.sel
            && self
                .last_click
                .is_some_and(|c| c.elapsed() < Duration::from_millis(400));
        self.sel = t.key.clone();
        self.last_click = Some(Instant::now());
        if double {
            return Self::go(m, &t.key);
        }
        Out::Draw
    }

    /// A spinner moves only while a tile at work is drawn.
    fn wake_at(&self, _m: &Model) -> Option<Instant> {
        self.moving
            .then(|| Instant::now() + Duration::from_secs(1))
    }
}
