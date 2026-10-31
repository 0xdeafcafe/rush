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
//! with a decisions block shows them, answered from the Wall by typing
//! 1a 2b and enter.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rush_convo::decisions::{Decision, decisions_in};
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
use crate::screens::session_core::host::SendErr;
use crate::screens::session_core::turnwants::{
    WANTS_PROBLEM, unlooked, wants_mark, wants_of, wants_say, wants_you,
};
use crate::style::*;

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
    /// The replies whose decisions were answered from the Wall, by agent,
    /// by when the reply was written: their tiles stop asking.
    pub answered: HashMap<String, Ms>,
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
    /// What's typed to answer a tile's decisions: its key, and the text.
    answer: (String, String),
    above: usize,
    below: usize,
    /// A live tile was drawn: its spinner moves.
    moving: bool,
    /// When it was last drawn, which its spinner's wake counts from.
    drawn: Option<Instant>,
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

/// What a busy agent still runs (lanesLine).
pub fn lanes_line(a: &Agent) -> String {
    let (mut agents, mut workflows, mut other) = (0, 0, 0);
    for b in &a.job.background {
        if b.starts_with("agent\0") {
            agents += 1;
        } else if b.starts_with("workflow\0") {
            workflows += 1;
        } else {
            other += 1;
        }
    }
    let s = |n: usize| if n == 1 { "" } else { "s" };
    let mut parts = Vec::new();
    if workflows > 0 {
        parts.push(format!("{workflows} workflow{} running", s(workflows)));
    }
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

/// The decisions a's last reply ends with, and when it was written:
/// while its turn is over and they're not answered from here yet.
pub fn decisions_of(m: &Model, a: &Agent) -> Option<(Vec<Decision>, Ms)> {
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
    let answered = m
        .kept
        .peek::<WallState>()
        .and_then(|s| s.answered.get(&a.key));
    if answered == Some(&e.at) {
        return None;
    }
    let (_, ds, _) = decisions_in(&e.text);
    (!ds.is_empty()).then_some((ds, e.at))
}

/// The picks typed, by decision number: "1a 2b", "1a2b" or "1 a".
pub fn typed_picks(s: &str) -> HashMap<String, String> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = HashMap::new();
    let mut i = 0;
    while i < cs.len() {
        if !cs[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let from = i;
        while i < cs.len() && cs[i].is_ascii_digit() {
            i += 1;
        }
        let n: String = cs[from..i].iter().collect();
        while i < cs.len() && cs[i] == ' ' {
            i += 1;
        }
        if i < cs.len() && cs[i].is_ascii_lowercase() {
            out.insert(n, cs[i].to_string());
            i += 1;
        }
    }
    out
}

/// The answer picks give ds, a paragraph a decision answered, "1a.
/// Topic: label" as the session's card writes it (DecisionAnswer); None
/// when none of them is an option.
pub fn answer_text(ds: &[Decision], picks: &HashMap<String, String>) -> Option<String> {
    let out: Vec<String> = ds
        .iter()
        .filter_map(|d| {
            let k = picks.get(&d.r#ref)?;
            let o = d.options.iter().find(|o| &o.key == k)?;
            Some(format!("{}{k}. {}: {}", d.r#ref, d.topic, o.label))
        })
        .collect();
    (!out.is_empty()).then(|| out.join("\n\n"))
}

/// A tile's decisions, w wide: each one's number and topic, and under it
/// its options by letter, wrapped; the letter typed orange, the agent's
/// own pick green.
pub fn decision_lines(ds: &[Decision], picks: &HashMap<String, String>, w: usize) -> Vec<Line> {
    let mut out = Vec::new();
    for d in ds {
        let topic = rush_draw::truncate(&unmark(&d.topic), w.saturating_sub(3), "…").into_owned();
        out.push(line([
            spb(format!("{} ", d.r#ref), YELLOW),
            spb(topic, TEXT),
        ]));
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
            let label = unmark(&one_line(&o.label));
            let label = rush_draw::truncate(&label, w.saturating_sub(6).min(48), "…").into_owned();
            let piece = line([spb(o.key.clone(), kc), plain(" "), sp(label, lc)]);
            if cur.width() > 2 && cur.width() + 2 + piece.width() > w {
                out.push(std::mem::replace(&mut cur, line([plain("  ")])));
            } else if cur.width() > 2 {
                cur.spans.push(plain("  "));
            }
            join(&mut cur, piece);
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
    pub(super) fn tile(&self, m: &Model, i: usize, w: usize, h: usize, picked: bool) -> Vec<Line> {
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
        let asks = decisions_of(m, a);
        let typed = self.typed(&a.key);
        let now1 = if asks.is_some() && !typed.is_empty() {
            line([
                spb("answer ", YELLOW),
                spb(typed.to_string(), BRIGHT),
                sp("▏", ORANGE),
            ])
        } else if let Some((ds, _)) = &asks {
            let n = ds.len();
            let s = if n == 1 { "" } else { "s" };
            line([
                spb("needs you", YELLOW),
                sp(" · ", DIM),
                sp(format!("{n} decision{s} · type 1a… to answer"), TEXT),
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
        if asks.is_some() && !typed.is_empty() {
            stats = line([sp("enter sends · esc clears", DIM)]);
        }
        let foot = spread(now1, stats, iw);

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
        let runs =
            crate::screens::accounts::look::runs_on(&m.reg, a.kind.as_str(), &p.model, &p.effort);
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
            let mut l = decision_lines(&ds, &typed_picks(self.typed(&a.key)), w);
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

    /// What's typed to answer agent key's decisions.
    fn typed(&self, key: &str) -> &str {
        if self.answer.0 == key {
            &self.answer.1
        } else {
            ""
        }
    }

    /// Sends what's typed as the answer to agent i's decisions, "1a.
    /// Topic: label" a paragraph each as the session's card answers
    /// (DecisionAnswer), through its host or its agent's own daemon.
    fn send_answer(&mut self, m: &mut Model, i: usize) -> Out {
        let a = m.agents[i].clone();
        let Some((ds, at)) = decisions_of(m, &a) else {
            return Out::Idle;
        };
        let Some(text) = answer_text(&ds, &typed_picks(self.typed(&a.key))) else {
            m.flash_err("type a decision's number and an option's letter, like 1a");
            return Out::Draw;
        };
        self.answer = Default::default();
        m.kept::<WallState>().answered.insert(a.key.clone(), at);
        let h = crate::screens::session_core::host::hosts(m);
        let (key, name) = (a.key.clone(), a.display_name.clone());
        m.later(
            move || h.reply(&a, &text),
            move |m, r| {
                let e = match r {
                    Ok(()) => {
                        m.flash(&format!("answered {name}"));
                        return Out::Draw;
                    }
                    Err(SendErr::Gone) => format!("{name} has gone · open it to answer"),
                    Err(SendErr::Failed(e)) => e,
                };
                // Not sent: its tile asks again.
                m.kept::<WallState>().answered.remove(&key);
                m.fail(&e);
                Out::Draw
            },
        );
        Out::Draw
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
pub(super) fn keep_going(m: &mut Model, i: usize) -> Out {
    let a = m.agents[i].clone();
    crate::screens::cmdbar::hashcmds::keep_going(m, &a)
}

/// After each reading of the list: the tiles' transcripts read again
/// while the Wall shows, or the grid's pinned ones while it doesn't.
pub(super) fn on_fleet(m: &mut Model) {
    // Agents gone, subagents ended and tiles unpinned take their reads
    // with them.
    let keys: HashSet<String> = read_items(m).into_iter().map(|it| it.key).collect();
    m.kept::<WallState>().reads.retain(|k, _| keys.contains(k));
    read(m);
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
        let items = wall_items(m, all);
        self.tiles.clear();
        self.geo.clear();
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
        let typing = asks && !self.typed(&self.sel).is_empty();
        let pairs: Vec<(&str, &str)> = if typing {
            vec![("enter", "send"), ("esc", "clear"), ("←↑↓→", "move")]
        } else if asks {
            vec![
                ("1a 2b", "answer"),
                ("←↑↓→", "move"),
                ("enter", "open"),
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
        // Typing 1a 2b answers the picked tile's decisions: a number
        // starts it, and while it's typed letters and space go to it.
        let asks = items[i].sub.is_none() && decisions_of(m, &m.agents[items[i].a]).is_some();
        let key = items[i].key.clone();
        let typing = asks && !self.typed(&key).is_empty();
        if asks {
            let c = s.chars().next().filter(|_| s.chars().count() == 1);
            let starts = c.is_some_and(|c| c.is_ascii_digit());
            let goes_on = typing
                && (s == "space"
                    || c.is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
            if starts || goes_on {
                if !typing {
                    self.answer = (key, String::new());
                }
                self.answer
                    .1
                    .push(if s == "space" { ' ' } else { c.unwrap_or(' ') });
                return Out::Draw;
            }
            match s {
                "backspace" if typing => {
                    self.answer.1.pop();
                    return Out::Draw;
                }
                "esc" if typing => {
                    self.answer = Default::default();
                    return Out::Draw;
                }
                "enter" if typing => return self.send_answer(m, items[i].a),
                _ => {}
            }
        }
        let geo = if self.geo.len() == items.len() {
            self.geo.clone()
        } else {
            Vec::new()
        };
        let mut moved = |dx: isize, dy: isize| {
            self.answer = Default::default();
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

    /// A spinner moves only while a tile at work is drawn: a second after
    /// it last was (a time fixed when drawn, so the wake comes due).
    fn wake_at(&self, _m: &Model) -> Option<Instant> {
        self.drawn
            .filter(|_| self.moving)
            .map(|d| d + Duration::from_secs(1))
    }
}
