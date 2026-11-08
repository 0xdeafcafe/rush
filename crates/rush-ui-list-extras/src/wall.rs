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
//! wallClick :1059; what a finished turn wants, as of 3c9343b1: :101,
//! :499, :580) and workstreams.go:277 (lanesLine). Each running
//! subagent has a tile of its own beside its agent's (wallItem :125,
//! wallItems :136, wallGroups :150, wallTile :512, wallSubTile :684).
//! Not here yet: ctrl+b's keep going, which waits on rush running
//! sessions itself.
//!
//! Beyond Go: tiles are laid out in columns, each agent's tile with its
//! subagents' small ones under it, in the shortest column, and each as
//! tall as what it has to show; a subagent's tile is one line, what it's
//! doing; a reply shows as plain words, no tables or code; a run of the
//! same step is one line with its count; and a tile whose last reply ends
//! with a decisions block shows them, and enter (or a click) on it opens
//! its session's decisions card in the tile, answered there with the
//! card's own keys and sent as the card sends it (session_cards'
//! question.rs, through the decide_* slots).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rush_convo::decisions::{Decision, DecisionOption, decisions_in};
use rush_core::adapter::fill_of;
use rush_core::agent::{Agent, Preview, PreviewLine};
use rush_core::event::Question;
use rush_core::text::{Color, Line, Span, width};
use rush_core::time::Ms;
use rush_draw::{Button, MouseKind};
use rush_fleet::Predicates;

use crate::frame::overlayclick::hint_key;
use crate::frame::places::Place;
use crate::frame::{Decided, Mouse, Out, Press};
use crate::list::{folder_key, folder_titles, project_tint};
use crate::model::Model;
use crate::style::*;
use crate::turnwants::{WANTS_PROBLEM, unlooked, wants_mark, wants_of, wants_say, wants_you};

/// Tiles are at least this big, unless they've less to show; below it
/// they scroll.
pub const MIN_W: usize = 40;
pub const MIN_H: usize = 9;
/// How wide a tile would like to be.
pub const TILE_W: usize = 72;
/// A subagent's tile: its edges and one line.
pub const SUB_H: usize = 3;
/// How far a subagent's tile sits in under its agent's.
const SUB_IN: usize = 2;
// What a wall shows besides what's open: the last day's (workSince).
use super::work::WORK_SINCE;
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
    /// The decisions card each tile was answered with, by agent: when its
    /// reply was written, and the card's id. Once the card is answered or
    /// set aside its tile stops asking, and asks again if the answers
    /// didn't go.
    pub asked: HashMap<String, (Ms, String)>,
    /// The open decisions card of each rush session the list says has
    /// one, as its host last wrote it (info.json), by agent: the
    /// headings it was read for (Agent::decisions_key), and the card.
    pub hosted: HashMap<String, (String, Option<Question>)>,
    host_reading: HashSet<String>,
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
    /// The lines scrolled past.
    top: usize,
    tiles: Vec<Tile>,
    /// Where each tile was last drawn, for the arrows.
    geo: Vec<Geo>,
    /// The tile whose decisions are being answered on it, by its agent's
    /// key, and its card as last drawn.
    answering: String,
    card: Option<CardAt>,
    above: usize,
    below: usize,
    /// A live tile was drawn: its spinner moves.
    moving: bool,
    /// When it was last drawn, which its spinner's wake counts from.
    drawn: Option<Instant>,
    last_click: Option<Instant>,
    names: HashMap<String, String>,
}

/// A decisions card as drawn on its tile: its rows, where its first is in
/// the Wall's body (its top may be scrolled off, so above 0), and what a
/// click on each row presses.
#[derive(Clone, Debug)]
struct CardAt {
    key: String,
    x: usize,
    y: isize,
    rows: Vec<Line>,
    hits: HashMap<usize, String>,
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
        if a.needs_you() || a.halted() || wants_you(m, a) || decisions_of(m, a).is_some() {
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

/// One tile on the Wall: an open agent (by its place in Model::agents),
/// or one of its subagents still working (by its place in the agent's
/// subagents); key is the agent's, and a subagent's is the agent's, NUL,
/// its id (wallItem).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub key: String,
    pub a: usize,
    pub sub: Option<usize>,
}

/// The Wall's tiles: wall_agents' agents, each followed at once by its
/// own subagents still running, so a fleet busy underneath reads top to
/// bottom as it happens rather than behind a count (wallItems).
pub fn wall_items(m: &Model, all: bool) -> Vec<Item> {
    let agents = wall_agents(m, all);
    let mut out = Vec::with_capacity(agents.len());
    for i in agents {
        let a = &m.agents[i];
        out.push(Item {
            key: a.key.clone(),
            a: i,
            sub: None,
        });
        for (j, s) in a.subagents.iter().enumerate() {
            out.push(Item {
                key: format!("{}\0{}", a.key, s.id),
                a: i,
                sub: Some(j),
            });
        }
    }
    out
}

/// The sizes of items' groups: an agent and its subagents (wallGroups).
pub fn groups(items: &[Item]) -> Vec<usize> {
    let mut gs: Vec<usize> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if i == 0 || it.a != items[i - 1].a {
            gs.push(0);
        }
        if let Some(g) = gs.last_mut() {
            *g += 1;
        }
    }
    gs
}

/// How tall a tile can be: the least it takes, and the most its content
/// fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    pub min: usize,
    pub most: usize,
}

/// A tile laid out: its column, its top in lines down the Wall, and its
/// height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    pub col: usize,
    pub y: usize,
    pub h: usize,
}

/// How many columns w holds for n groups: tiles about TILE_W wide, never
/// under MIN_W, and no more columns than groups.
pub fn columns(w: usize, n: usize) -> usize {
    let most = 1.max((w + 1) / (MIN_W + 1));
    let like = ((w + 1) as f64 / (TILE_W + 1) as f64).round() as usize;
    like.clamp(1, most).min(n.max(1))
}

/// The columns' widths and where each starts, cols across w with a gap
/// between them.
fn col_geometry(w: usize, cols: usize) -> (Vec<usize>, Vec<usize>) {
    let tile_w = w.saturating_sub(cols - 1) / cols;
    let extra = w.saturating_sub(cols - 1) - tile_w * cols;
    let (mut col_w, mut col_x) = (vec![0; cols], vec![0; cols]);
    let mut x = 0;
    for c in 0..cols {
        col_w[c] = tile_w + usize::from(c < extra);
        col_x[c] = x;
        x += col_w[c] + 1;
    }
    (col_w, col_x)
}

/// Groups of tiles in cols columns h tall: each group, an agent's tile
/// with its subagents' under it, goes in the column with least in it so
/// far, so what needs you sits top left; then each column's room left
/// goes to its tiles that want more, a share each, until they're full or
/// it's gone. A column past h scrolls.
pub fn layout(groups: &[usize], wants: &[Want], cols: usize, h: usize) -> Vec<Placed> {
    let cols = cols.max(1);
    let mut placed = vec![Placed::default(); wants.len()];
    let mut in_col: Vec<Vec<usize>> = vec![Vec::new(); cols];
    let mut used = vec![0; cols];
    let mut i = 0;
    for &g in groups {
        let c = (0..cols).min_by_key(|&c| (used[c], c)).unwrap_or(0);
        for k in i..(i + g).min(wants.len()) {
            in_col[c].push(k);
            used[c] += wants[k].min;
            placed[k].col = c;
            placed[k].h = wants[k].min;
        }
        i += g;
    }
    for (c, ks) in in_col.iter().enumerate() {
        let mut left = h.saturating_sub(used[c]);
        while left > 0 {
            let more: Vec<usize> = ks
                .iter()
                .copied()
                .filter(|&k| placed[k].h < wants[k].most)
                .collect();
            if more.is_empty() {
                break;
            }
            let share = 1.max(left / more.len());
            for k in more {
                let add = share.min(wants[k].most - placed[k].h).min(left);
                placed[k].h += add;
                left -= add;
                if left == 0 {
                    break;
                }
            }
        }
        let mut y = 0;
        for &k in ks {
            placed[k].y = y;
            y += placed[k].h;
        }
    }
    placed
}

/// Where a tile is for moving between them: its column, and its top and
/// bottom in lines (wallGeo).
#[derive(Clone, Copy, Debug, Default)]
pub struct Geo {
    col: usize,
    top: usize,
    bot: usize,
}

pub fn geos(placed: &[Placed]) -> Vec<Geo> {
    placed
        .iter()
        .map(|p| Geo {
            col: p.col,
            top: p.y,
            bot: p.y + p.h,
        })
        .collect()
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
            (cur.top - g.bot) as isize * 100 + col_d * 10_000
        } else if dy > 0 && g.top >= cur.bot {
            (g.top - cur.bot) as isize * 100 + col_d * 10_000
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

/// The tiles wholly above the lines shown from top, h of them, and wholly
/// below (wallOffscreen).
fn offscreen(placed: &[Placed], top: usize, h: usize) -> (usize, usize) {
    let above = placed.iter().filter(|p| p.y + p.h <= top).count();
    let below = placed.iter().filter(|p| p.y >= top + h).count();
    (above, below)
}

// What a busy agent still runs (lanesLine): the base's, at its old path.
pub use crate::look::lanes_line;

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

pub use crate::look::ctx_bar;

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

/// A step in words: what a tool call did, or what was said.
fn step_words(e: &PreviewLine) -> String {
    if e.role != "tool" {
        return unmark(&one_line(&e.text));
    }
    let d = one_line(&tildify(&e.doing));
    if !d.is_empty() {
        return d;
    }
    let (tool, arg) = e.text.split_once('\0').unwrap_or((&e.text, ""));
    format!("{tool} {}", one_line(&tildify(arg)))
        .trim()
        .to_string()
}

/// One of a tile's steps on a line, n times running: a tool call in
/// words, or something said or asked before the latest. back is how many
/// steps came after it: the newest three bright, the next six dim, the
/// rest faint (wallDid).
pub fn did(e: &PreviewLine, n: usize, back: usize, live: bool, w: usize) -> Line {
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
    let times = if n > 1 {
        sp(format!(" ×{n}"), ink(DIM))
    } else {
        plain("")
    };
    let cut =
        |s: &str| rush_draw::truncate(s, w.saturating_sub(2 + times.width()), "…").into_owned();
    match e.role.as_str() {
        "user" => line([sp(format!("❯ {}", cut(&one_line(&e.text))), ink(BLUE))]),
        "tool" => {
            let dot = if back == 0 && live { ORANGE } else { ink(SUB) };
            line([sp("● ", dot), sp(cut(&step_words(e)), ink(TEXT)), times])
        }
        _ => line([plain("  "), sp(cut(&step_words(e)), ink(SUB)), times]),
    }
}

/// A reply as a tile shows it, on one line: its decisions block, code
/// blocks and tables' rules left out, a table row's cells joined by " · ",
/// and no headings' or bold's marks.
pub fn plain_reply(s: &str) -> String {
    let (before, ds, after) = decisions_in(s);
    let text = if ds.is_empty() {
        s.to_string()
    } else {
        format!("{before}\n{after}")
    };
    let mut out = Vec::new();
    let mut fence = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if t.starts_with('|') {
            if t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                continue;
            }
            let cells: Vec<&str> = t
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .collect();
            out.push(cells.join(" · "));
            continue;
        }
        out.push(t.trim_start_matches('#').trim().to_string());
    }
    unmark(&one_line(&out.join("\n")))
}

/// a's last reply, while its turn is over and nothing of yours came
/// after it.
fn last_reply<'a>(m: &'a Model, a: &Agent) -> Option<&'a PreviewLine> {
    if a.job.live() || a.busy() {
        return None;
    }
    let p = preview(m, &a.key)?;
    let asked = p.recent.iter().rposition(|e| e.role == "user");
    let (at, e) = p
        .recent
        .iter()
        .enumerate()
        .rev()
        .find(|(_, e)| e.role == "assistant")?;
    if asked.is_some_and(|u| u > at) {
        return None;
    }
    Some(e)
}

/// The decisions card a rush session's host keeps open for it, as read
/// for the headings the list has now.
fn host_card<'a>(m: &'a Model, a: &Agent) -> Option<&'a Question> {
    if !a.decides {
        return None;
    }
    let (k, q) = m.kept.peek::<WallState>()?.hosted.get(&a.key)?;
    q.as_ref().filter(|_| *k == a.decisions_key)
}

/// Agent key's card id, asked for the reply written at at, was answered
/// or set aside.
fn gone(m: &Model, key: &str, at: Ms, id: &str) -> bool {
    m.kept
        .peek::<WallState>()
        .and_then(|s| s.asked.get(key))
        .is_some_and(|(a, i)| *a == at && i == id && m.ext.calls.decide_gone()(m, key, id))
}

/// A card's decisions as a block's, to draw on a tile.
fn decisions_from(q: &Question) -> Vec<Decision> {
    q.asks
        .iter()
        .map(|a| Decision {
            r#ref: a.id.clone(),
            topic: a.header.clone(),
            context: String::new(),
            options: a
                .options
                .iter()
                .map(|o| DecisionOption {
                    key: o.key.clone(),
                    label: o.label.clone(),
                    detail: o.description.clone(),
                    preview: o.preview.clone(),
                    recommended: o.picked,
                })
                .collect(),
        })
        .collect()
}

/// The decisions a asks, and when the reply asking them was written (0
/// for its host's card): its host's open card for a rush session, else
/// the block its last reply ends with while its turn is over; none once
/// their card is answered or set aside (here or in its session, the
/// card's state is the one).
pub fn decisions_of(m: &Model, a: &Agent) -> Option<(Vec<Decision>, Ms)> {
    if let Some(q) = host_card(m, a) {
        return (!gone(m, &a.key, 0, &q.id)).then(|| (decisions_from(q), 0));
    }
    let e = last_reply(m, a)?;
    let (_, ds, _) = decisions_in(&e.text);
    if ds.is_empty() {
        return None;
    }
    // The card it was answered with, if it was: its id is in asked.
    let id = m
        .kept
        .peek::<WallState>()
        .and_then(|s| s.asked.get(&a.key))
        .map(|(_, id)| id.as_str());
    if id.is_some_and(|id| gone(m, &a.key, e.at, id)) {
        return None;
    }
    Some((ds, e.at))
}

/// The decisions card a's tile answers, and when the reply it's for was
/// written: its host's open card, else its session's or its reply's
/// block (decide_question).
fn card_of(m: &Model, a: &Agent) -> Option<(Question, Ms)> {
    decisions_of(m, a)?;
    if let Some(q) = host_card(m, a) {
        return Some((q.clone(), 0));
    }
    let e = last_reply(m, a)?;
    let q = m.ext.calls.decide_question()(m, &a.key, &e.text, e.at)?;
    Some((q, e.at))
}

/// Reads, off the UI thread, the open decisions card of each rush
/// session on the Wall whose list row says it has one that isn't read
/// yet: what its tile shows and answers, the very card its session has.
fn read_hosted(m: &mut Model) {
    let all = m.kept::<WallState>().all;
    let mut wants = Vec::new();
    {
        let items = wall_items(m, all);
        let st = m.kept.get::<WallState>();
        let open: HashSet<&str> = items.iter().map(|it| it.key.as_str()).collect();
        st.hosted.retain(|k, _| open.contains(k.as_str()));
        for it in items.iter().filter(|it| it.sub.is_none()) {
            let a = &m.agents[it.a];
            let read = st.hosted.get(&a.key).map(|(k, _)| k);
            if !a.decides
                || a.job.id.is_empty()
                || read == Some(&a.decisions_key)
                || st.host_reading.contains(&a.key)
            {
                continue;
            }
            st.host_reading.insert(a.key.clone());
            wants.push((a.key.clone(), a.job.id.clone(), a.decisions_key.clone()));
        }
    }
    if wants.is_empty() {
        return;
    }
    m.later(
        move || {
            wants
                .into_iter()
                .map(|(key, id, k)| {
                    let q = rush_host::client::read_info(&id)
                        .ok()
                        .and_then(|i| i.decisions);
                    (key, k, q)
                })
                .collect::<Vec<_>>()
        },
        |m, got| {
            let st = m.kept::<WallState>();
            for (key, k, q) in got {
                st.host_reading.remove(&key);
                st.hosted.insert(key, (k, q));
            }
            Out::Draw
        },
    );
}

/// A tile's decisions, w wide: each one's number and topic, and under it
/// its options by letter, wrapped; the letter typed orange, the agent's
/// own pick green.
pub fn decision_lines(ds: &[Decision], picks: &HashMap<String, String>, w: usize) -> Vec<Line> {
    let mut out = Vec::new();
    for d in ds {
        // The topic wraps under its own words, past its number.
        let ind = width(&d.r#ref) + 1;
        let topic = line([spb(unmark(&one_line(&d.topic)), TEXT)]);
        for (j, t) in wrap_line(&topic, w.saturating_sub(ind).max(8))
            .into_iter()
            .enumerate()
        {
            let mut l = if j == 0 {
                line([spb(format!("{} ", d.r#ref), YELLOW)])
            } else {
                line([plain(blanks(ind))])
            };
            join(&mut l, t);
            out.push(l);
        }
        let picked = picks.get(&d.r#ref);
        let mut cur = line([plain("  ")]);
        for o in &d.options {
            let chosen = picked == Some(&o.key);
            let (kc, lc) = if chosen {
                (ORANGE, BRIGHT)
            } else if o.recommended {
                (GREEN, TEXT)
            } else if picked.is_some() {
                (DIM, SUB)
            } else {
                (BLUE, TEXT)
            };
            // A label too long for a row wraps under its own first letter.
            let kw = width(&o.key) + 1;
            let label = line([sp(unmark(&one_line(&o.label)), lc)]);
            let rows = wrap_line(&label, w.saturating_sub(2 + kw).max(8));
            let mut piece = line([spb(o.key.clone(), kc), plain(" ")]);
            join(&mut piece, rows[0].clone());
            if cur.width() > 2 && cur.width() + 2 + piece.width() > w {
                out.push(std::mem::replace(&mut cur, line([plain("  ")])));
            } else if cur.width() > 2 {
                cur.spans.push(plain("  "));
            }
            join(&mut cur, piece);
            for r in &rows[1..] {
                out.push(std::mem::replace(&mut cur, line([plain("  ")])));
                let mut l = line([plain(blanks(2 + kw))]);
                join(&mut l, r.clone());
                cur = l;
            }
        }
        if cur.width() > 2 {
            out.push(cur);
        }
    }
    out
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
        let text = plain_reply(&p.recent[s].text);
        for l in clip(wrap(&text, tw), say_n, tw) {
            head.push(line([plain("  "), sp(l, TEXT)]));
        }
    }
    head.truncate(room);
    // A run of the same step is one, with its count.
    let mut steps: Vec<(&PreviewLine, usize)> = Vec::new();
    for (i, e) in p.recent.iter().enumerate() {
        if Some(i) == at || Some(i) == say {
            continue;
        }
        match steps.last_mut() {
            Some((last, n))
                if e.role == "tool" && last.role == "tool" && step_words(last) == step_words(e) =>
            {
                *n += 1;
            }
            _ => steps.push((e, 1)),
        }
    }
    let mut left = room - head.len();
    if !head.is_empty() {
        left = left.saturating_sub(1); // the rule
    }
    if steps.is_empty() || left < 1 {
        return head;
    }
    let mut out = Vec::new();
    for back in 0..steps.len().min(left) {
        let (e, n) = steps[steps.len() - 1 - back];
        out.push(did(e, n, back, live, w));
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
fn frame_close(
    top: Line,
    inner: Vec<Line>,
    edge: Color,
    w: usize,
    h: usize,
    heavy: bool,
) -> Vec<Line> {
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
    fn pick(&mut self, items: &[Item]) -> usize {
        if let Some(i) = items.iter().position(|it| it.key == self.sel) {
            return i;
        }
        if let Some(it) = items.first() {
            self.sel = it.key.clone();
        }
        0
    }

    /// Keeps p, the picked tile, among the h lines shown of all; its top
    /// when it's taller than them (wallScroll).
    fn scroll(&mut self, p: Placed, all: usize, h: usize) {
        if p.y < self.top {
            self.top = p.y;
        } else if p.y + p.h > self.top + h {
            self.top = (p.y + p.h - h).min(p.y);
        }
        self.top = self.top.min(all.saturating_sub(h));
    }

    /// How a tile shows its agent's state: its edge's colour, the glyph by
    /// its name, and the word for it (wallLook).
    fn look(&self, m: &Model, a: &Agent) -> (Color, Span, Span) {
        let now = m.now;
        if a.needs_you() || decisions_of(m, a).is_some() {
            (YELLOW, spb("●", YELLOW), spb("needs you", YELLOW))
        } else if a.halted() {
            (RED, sp("✕", RED), sp("stopped", RED))
        } else if a.job.live() {
            let g = spin(a.kind.as_str(), m.tick + a.job.id.len());
            (ORANGE, sp(g, ORANGE), sp("working", ORANGE))
        } else if a.busy() {
            (QUEUE, sp("◌", QUEUE), sp(lanes_line(a), QUEUE))
        } else if wants_you(m, a) {
            let t = wants_of(m, a);
            let c = if t.wants == WANTS_PROBLEM {
                RED
            } else {
                YELLOW
            };
            (c, wants_mark(&t), spb("needs you", c))
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
    fn project(m: &Model, i: usize) -> &str {
        let a = &m.agents[i];
        let root = m.roots.get(i).map_or("", String::as_str);
        if root.is_empty() && !a.repo.is_empty() {
            return &a.repo;
        }
        folder_key(a, root)
    }

    /// Where a tile's agent works: its project in its own colour and,
    /// dim, its branch; the worktree and the folder the project's in are
    /// left to its session (wallWhere).
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
        if !a.branch.is_empty() {
            out.spans.push(sp(" · ", FAINT));
            out.spans.push(sp(a.branch.clone(), DIM));
        }
        out
    }

    /// One agent's tile, w×h, its edge heavy when it's picked
    /// (wallAgentTile).
    pub fn tile(&self, m: &Model, i: usize, w: usize, h: usize, picked: bool) -> Vec<Line> {
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
        let model_id: &str = if p.model.is_empty() {
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
        let asks = decisions_of(m, a);
        let now1 = if let Some((ds, _)) = &asks {
            let n = ds.len();
            let s = if n == 1 { "" } else { "s" };
            line([
                spb("needs you", YELLOW),
                sp(" · ", DIM),
                sp(format!("{n} decision{s} to answer"), TEXT),
            ])
        } else if a.needs_you() {
            line([
                word,
                sp(" · ", DIM),
                sp(first(&[&a.job.needs, &a.job.detail, &p.text]), TEXT),
            ])
        } else if a.halted() {
            let t = a.spend.halt.as_ref().map(|h| h.text.as_str()).unwrap_or("");
            line([word, sp(" · ", DIM), sp(one_line(t), SUB)])
        } else if unlooked(m.now, a) && !wants_say(&wants_of(m, a)).is_empty() {
            // What its last message wants, or that it's done.
            line([word, sp(" · ", DIM), sp(wants_say(&wants_of(m, a)), TEXT)])
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

        // Its decisions being answered: their card, under its place, in
        // all the room there is; the card has its own keys at its foot.
        if let Some(c) = self.card.as_ref().filter(|c| c.key == a.key) {
            let room = h.saturating_sub(3);
            inner.extend(c.rows.iter().take(room).map(|l| fit(l.clone(), iw)));
            for _ in c.rows.len()..room {
                inner.push(plain_line(iw));
            }
            return frame_close(top, inner, edge, w, h, picked);
        }

        // Between them, the decisions it asks, or the stream.
        let room = h as isize - 4;
        if room > 0 {
            let room = room as usize;
            let s = self.content(m, i, iw, room);
            let n = s.len();
            inner.extend(s.into_iter().map(|l| fit(l, iw)));
            for _ in n..room {
                inner.push(plain_line(iw));
            }
        }
        inner.push(foot);
        frame_close(top, inner, edge, w, h, picked)
    }

    /// A tile, w×h: an agent's, or one of its subagents' (wallTile).
    fn item_tile(&self, m: &Model, it: &Item, w: usize, h: usize, picked: bool) -> Vec<Line> {
        match it.sub {
            Some(j) => self.sub_tile(m, it, j, w, h, picked),
            None => self.tile(m, it.a, w, h, picked),
        }
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

    /// A subagent still working, small: its name on its top edge with what
    /// it runs on and how long it's been going, and one line, what it's
    /// doing now (wallSubTile).
    fn sub_tile(
        &self,
        m: &Model,
        it: &Item,
        j: usize,
        w: usize,
        h: usize,
        picked: bool,
    ) -> Vec<Line> {
        let a = &m.agents[it.a];
        let Some(sub) = a.subagents.get(j) else {
            return Vec::new();
        };
        if w < 8 || h < 3 {
            return Vec::new();
        }
        let now = m.now;
        let empty = Preview::default();
        let p = preview(m, &it.key).unwrap_or(&empty);
        let mut edge = ORANGE;
        let glyph = sp(SPINNER[(m.tick + sub.id.len()) % SPINNER.len()], ORANGE);
        if p.at != 0 && now - p.at < 2000 {
            edge = BRIGHT;
        }
        let (tl, tr, hz) = if picked {
            ("┏", "┓", "━")
        } else {
            ("╭", "╮", "─")
        };
        let name_c = if picked { BRIGHT } else { TEXT };
        let iw = w - 4;

        let typ = if sub.kind.is_empty() {
            "subagent"
        } else {
            sub.kind.as_str()
        };
        let name = one_line(if sub.description.is_empty() {
            typ
        } else {
            &sub.description
        });
        let runs = crate::look::runs_on(&m.reg, a.kind.as_str(), &p.model, &p.effort);
        let took = if sub.born > 0 {
            age(now - sub.born)
        } else {
            String::new()
        };
        let mut meta = [runs, took]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let mut room_name = w as isize - 12 - width(&meta) as isize;
        if room_name < 8 {
            meta.clear();
            room_name = w as isize - 8;
        }
        let mut title = line([glyph, plain(" ")]);
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

        // Its latest step, else what it last said, else that it's starting.
        let last = p.recent.iter().rev().find(|e| e.role != "user");
        let doing = match last {
            Some(e) if e.role == "tool" => did(e, 1, 0, true, iw),
            Some(e) => line([sp(plain_reply(&e.text), SUB)]),
            None if !p.doing.is_empty() => {
                line([sp("● ", ORANGE), sp(one_line(&tildify(&p.doing)), TEXT)])
            }
            None => line([sp("starting…", FAINT)]),
        };
        let mut inner = vec![fit(doing, iw)];
        for _ in 1..h.saturating_sub(2) {
            inner.push(plain_line(iw));
        }
        frame_close(top, inner, edge, w, h, picked)
    }

    /// Between an agent tile's place and foot, room lines of w: the
    /// decisions its last reply asks, or its stream.
    fn content(&self, m: &Model, i: usize, w: usize, room: usize) -> Vec<Line> {
        let a = &m.agents[i];
        if let Some((ds, _)) = decisions_of(m, a) {
            let mut l = decision_lines(&ds, &HashMap::new(), w);
            l.truncate(room);
            return l;
        }
        let empty = Preview::default();
        let p = preview(m, &a.key).unwrap_or(&empty);
        stream(p, a.job.live(), &a.job.intent, w, room)
    }

    /// How tall a tile can be, w wide on a Wall h tall: a subagent's one
    /// line; an agent's as tall as what it shows, at least MIN_H unless it
    /// shows less.
    fn want(&self, m: &Model, it: &Item, w: usize, h: usize) -> Want {
        if it.sub.is_some() {
            return Want {
                min: SUB_H,
                most: SUB_H,
            };
        }
        if self.answering == it.key
            && let Some((q, _)) = card_of(m, &m.agents[it.a])
        {
            // Its card whole, when the Wall's that tall: two edges and
            // its place besides.
            let (rows, _) =
                m.ext.calls.decide_rows()(m, &it.key, &q, w.saturating_sub(4), h.saturating_sub(3));
            let most = (rows.len() + 3).min(h.max(MIN_H));
            return Want { min: most, most };
        }
        // Two edges, its place and its foot.
        let shows = self
            .content(m, it.a, w.saturating_sub(4), h.max(MIN_H))
            .len();
        let most = (shows + 4).clamp(5, h.max(MIN_H));
        Want {
            min: most.min(MIN_H),
            most,
        }
    }

    /// Opens agent i's decisions card on its tile, the keys on it.
    fn start_answer(&mut self, m: &mut Model, i: usize) -> Out {
        let a = &m.agents[i];
        let Some((q, at)) = card_of(m, a) else {
            return Out::Idle;
        };
        let key = a.key.clone();
        m.kept::<WallState>().asked.insert(key.clone(), (at, q.id));
        self.answering = key;
        Out::Draw
    }

    /// Key s on agent i's decisions card on its tile, as its session's
    /// card takes it; what it sends goes as the card sends it, and the
    /// card closes once it's answered or set aside. Answering one in your
    /// own words opens its session, where they're typed.
    fn answer_key(&mut self, m: &mut Model, i: usize, s: &str) -> Out {
        let a = m.agents[i].clone();
        let Some((q, at)) = card_of(m, &a) else {
            self.answering.clear();
            return Out::Draw;
        };
        m.kept::<WallState>()
            .asked
            .insert(a.key.clone(), (at, q.id.clone()));
        match m.ext.calls.decide_key()(m, &a.key, &q, s) {
            Decided::Yes => Out::Draw,
            Decided::Done => {
                self.answering.clear();
                Out::Draw
            }
            Decided::Own => {
                self.answering.clear();
                Self::go(m, &a.key)
            }
            // The card's keys stay its own while it's open: the arrows
            // never move off the tile, a letter never starts a message;
            // [ and ] still turn the page.
            Decided::No if s == "[" || s == "]" => Out::Pass,
            Decided::No if s.chars().count() == 1 || is_arrow(s) => Out::Idle,
            Decided::No => Out::Pass,
        }
    }

    /// Opens agent key's conversation from the Wall (goAgent); a
    /// subagent's tile opens its agent's.
    fn go(m: &mut Model, key: &str) -> Out {
        let key = key.split('\0').next().unwrap_or(key);
        let Some(i) = m.agents.iter().position(|a| a.key == key) else {
            return Out::Idle;
        };
        m.sel = key.to_string();
        m.open(i);
        Out::Close
    }
}

fn is_arrow(s: &str) -> bool {
    matches!(
        s,
        "up" | "down" | "left" | "right" | "space" | "tab" | "shift+tab"
    )
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

/// The tiles whose transcripts are read: the Wall's while it shows, else
/// the grid's pinned agents, read the same way (wallTick :924-928, and
/// gridItems); none with neither.
fn read_items(m: &mut Model) -> Vec<Item> {
    let st = m.kept::<WallState>();
    if st.shown {
        let all = st.all;
        return wall_items(m, all);
    }
    let g = super::grid::grid(m).clone();
    super::grid::agents(m, &g)
        .into_iter()
        .map(|i| Item {
            key: m.agents[i].key.clone(),
            a: i,
            sub: None,
        })
        .collect()
}

/// Reads the tiles' transcripts that grew, off the UI thread, all in one
/// round (wallTick, onWallRead): the Wall's, or the grid's when the Wall
/// is closed.
pub fn read(m: &mut Model) {
    let items = read_items(m);
    let mut wants = Vec::new();
    {
        let st = m.kept.get::<WallState>();
        for it in &items {
            let a = &m.agents[it.a];
            // A subagent's tile reads its own transcript.
            let path = match it.sub.and_then(|j| a.subagents.get(j)) {
                Some(s) => &s.path,
                None => &a.job.transcript_path,
            };
            if path.is_empty() || st.reading.contains(&it.key) || wants.len() >= READS {
                continue;
            }
            st.reading.insert(it.key.clone());
            let had = st.reads.get(&it.key).map(|(_, n)| *n);
            wants.push((it.key.clone(), path.clone(), a.kind.to_string(), had));
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

/// ctrl+b on a tile, a row or a project's agent (keys.go:747 keepGoing).
pub fn keep_going(m: &mut Model, i: usize) -> Out {
    let a = m.agents[i].clone();
    m.ext.calls.keep_going()(m, &a)
}

/// After each reading of the list: the tiles' transcripts read again
/// while the Wall shows, or the grid's pinned ones while it doesn't.
pub fn on_fleet(m: &mut Model) {
    // Agents gone, subagents ended and tiles unpinned take their reads
    // with them.
    let keys: HashSet<String> = read_items(m).into_iter().map(|it| it.key).collect();
    m.kept::<WallState>().reads.retain(|k, _| keys.contains(k));
    read(m);
    if m.kept::<WallState>().shown {
        read_hosted(m);
    }
}

impl Place for Wall {
    fn name(&self) -> &str {
        "Wall"
    }

    fn enter(&mut self, m: &mut Model) {
        m.kept::<WallState>().shown = true;
        read(m);
        read_hosted(m);
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
        let items = wall_items(m, all);
        self.tiles.clear();
        self.geo.clear();
        self.card = None;
        (self.above, self.below) = (0, 0);
        self.moving = false;
        self.drawn = Some(Instant::now());
        if items.is_empty() || h == 0 {
            return Self::empty(m, w, h);
        }
        let keys: Vec<&str> = items.iter().map(|it| Self::project(m, it.a)).collect();
        self.names = folder_titles(keys.into_iter());
        let groups = groups(&items);
        let cols = columns(w, groups.len());
        let (col_w, col_x) = col_geometry(w, cols);
        let narrow = col_w.iter().copied().min().unwrap_or(w);
        let wants: Vec<Want> = items.iter().map(|it| self.want(m, it, narrow, h)).collect();
        let placed = layout(&groups, &wants, cols, h);
        self.geo = geos(&placed);
        let pick = self.pick(&items);
        let all_h = placed.iter().map(|p| p.y + p.h).max().unwrap_or(0);
        self.scroll(placed[pick], all_h, h);
        (self.above, self.below) = offscreen(&placed, self.top, h);

        let top = self.top;
        let mut cells: Vec<Vec<Option<Line>>> = vec![vec![None; h]; cols];
        for (k, p) in placed.iter().enumerate() {
            if p.y + p.h <= top || p.y >= top + h {
                continue;
            }
            let it = &items[k];
            let a = &m.agents[it.a];
            // A subagent's tile is one still working: its spinner moves.
            self.moving |= it.sub.is_some() || a.job.live() || a.busy();
            let inset = if it.sub.is_some() { SUB_IN } else { 0 };
            let cw = col_w[p.col];
            let tw = cw.saturating_sub(inset);
            if it.sub.is_none()
                && self.answering == it.key
                && let Some((q, _)) = card_of(m, a)
            {
                // Under its top edge and its place, inside its side edge.
                let (rows, hits) = m.ext.calls.decide_rows()(
                    m,
                    &it.key,
                    &q,
                    tw.saturating_sub(4),
                    p.h.saturating_sub(3),
                );
                self.card = Some(CardAt {
                    key: it.key.clone(),
                    x: col_x[p.col] + inset + 2,
                    y: (p.y + 2) as isize - top as isize,
                    rows,
                    hits,
                });
            }
            let t = self.item_tile(m, it, tw, p.h, k == pick);
            for (ly, l) in t.into_iter().enumerate() {
                let y = p.y + ly;
                if y < top || y >= top + h || y >= p.y + p.h {
                    continue;
                }
                let mut row = line([plain(blanks(inset))]);
                join(&mut row, l);
                cells[p.col][y - top] = Some(fit(row, cw));
            }
            let (y0, y1) = (p.y.max(top), (p.y + p.h).min(top + h));
            self.tiles.push(Tile {
                key: it.key.clone(),
                x: col_x[p.col] + inset,
                y: y0 - top,
                w: tw,
                h: y1 - y0,
            });
        }
        let mut out = Vec::with_capacity(h);
        for ly in 0..h {
            let mut l = Line::new();
            for (c, col) in cells.iter_mut().enumerate() {
                if c > 0 {
                    l.spans.push(plain(" "));
                }
                match col[ly].take() {
                    Some(t) => join(&mut l, t),
                    None => l.spans.push(plain(blanks(col_w[c]))),
                }
            }
            out.push(l);
        }
        out
    }

    /// The keys: arrows between tiles, enter opens one, a shows the
    /// day's, esc back (wallHint).
    fn hint(&self, m: &Model, w: usize) -> Line {
        let all = m.kept.peek::<WallState>().is_some_and(|s| s.all);
        let all = if all {
            "only what's open"
        } else {
            "the last day's"
        };
        let picked = m.agents.iter().find(|a| a.key == self.sel);
        let go = match picked {
            Some(a) if a.halted() => "continue",
            _ => "keep going",
        };
        let asks = picked.is_some_and(|a| decisions_of(m, a).is_some());
        let answering = asks && self.answering == self.sel;
        let pairs: Vec<(&str, &str)> = if answering {
            vec![
                ("enter", "picks"),
                ("p", "takes its picks"),
                ("s", "sets it aside"),
                ("esc", "back"),
            ]
        } else if asks {
            vec![
                ("enter", "answer"),
                ("o", "open"),
                ("←↑↓→", "move"),
                ("esc", "back"),
            ]
        } else {
            vec![
                ("←↑↓→", "move"),
                ("enter", "open"),
                ("a", all),
                ("ctrl+b", go),
                ("esc", "back"),
            ]
        };
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
        let items = wall_items(m, all);
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
        let i = self.pick(&items);
        // The picked tile's decisions: enter opens their card on it, and
        // while it's open the keys are the card's.
        let asks = items[i].sub.is_none() && decisions_of(m, &m.agents[items[i].a]).is_some();
        if !asks || self.answering != items[i].key {
            self.answering.clear();
        }
        if asks && !self.answering.is_empty() {
            return self.answer_key(m, items[i].a, s);
        }
        match s {
            "enter" if asks => return self.start_answer(m, items[i].a),
            "o" if asks => {
                let key = items[i].key.clone();
                return Self::go(m, &key);
            }
            _ => {}
        }
        let geo = if self.geo.len() == items.len() {
            self.geo.clone()
        } else {
            Vec::new()
        };
        let mut moved = |dx: isize, dy: isize| {
            let j = (!geo.is_empty())
                .then(|| step(&geo, i, dx, dy))
                .flatten()
                .or_else(|| {
                    let j = i as isize + dx;
                    (dx != 0 && j >= 0 && (j as usize) < items.len()).then_some(j as usize)
                });
            if let Some(j) = j {
                self.sel = items[j].key.clone();
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
                self.sel = items[0].key.clone();
                Out::Draw
            }
            "end" | "G" => {
                self.sel = items[items.len() - 1].key.clone();
                Out::Draw
            }
            "a" => toggle(m),
            "enter" => {
                let key = items[i].key.clone();
                Self::go(m, &key)
            }
            // A subagent's tile has none: it's its agent that goes on.
            "ctrl+b" | "alt+g" if items[i].sub.is_none() => keep_going(m, items[i].a),
            "ctrl+b" | "alt+g" => Out::Idle,
            _ => Out::Pass,
        }
    }

    /// A click picks the tile under it; a second opens it (wallClick).
    fn mouse(&mut self, m: &mut Model, e: &Mouse, x: usize, y: usize) -> Out {
        if e.kind != MouseKind::Down(Button::Left) {
            return Out::Idle;
        }
        // On a decisions card: an option picks it, a button or a key in
        // its hints presses that key.
        if let Some(c) = self.card.clone()
            && x >= c.x
            && (y as isize) >= c.y
            && ((y as isize - c.y) as usize) < c.rows.len()
            && let Some(t) = self.tiles.iter().find(|t| t.key == c.key)
            && x < t.x + t.w
        {
            let dy = (y as isize - c.y) as usize;
            let k = c
                .hits
                .get(&dy)
                .cloned()
                .or_else(|| hint_key(&c.rows[dy], x - c.x))
                .unwrap_or_default();
            let Some(i) = m.agents.iter().position(|a| a.key == c.key) else {
                return Out::Idle;
            };
            if k.is_empty() {
                return Out::Idle;
            }
            return self.answer_key(m, i, &k);
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
        if self.answering != t.key {
            self.answering.clear();
        }
        if double {
            return Self::go(m, &t.key);
        }
        // A click on a tile that asks opens its decisions card on it.
        if self.answering.is_empty()
            && let Some(i) = m.agents.iter().position(|a| a.key == t.key)
            && !t.key.contains('\0')
            && decisions_of(m, &m.agents[i]).is_some()
        {
            return self.start_answer(m, i);
        }
        Out::Draw
    }

    /// A spinner moves only while a tile at work is drawn: a second after
    /// it last was (a time fixed when drawn, so the wake comes due).
    fn wake_at(&self, _m: &Model) -> Option<Instant> {
        self.drawn
            .filter(|_| self.moving)
            .map(|d| d + Duration::from_secs(1))
    }
}
