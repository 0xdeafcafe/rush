//! A bar's line drawn from its segments: each segment painted, or nothing
//! when it has nothing to say; when the line is too wide, the segments
//! that hold on least go first, whole, and it ends ⋯, what went kept for
//! /statusline to point out. After Go's internal/ui/bars.go (barSeg :29,
//! barCtx :37, barSegs :318, findBarSeg :325, barMore :375, barLine :378,
//! barKeep :418, leastKept :432) and statuslinesheet.go:617 (sample).

use std::collections::HashMap;

use rush_core::text::Line;

use super::agent::Of;
use super::layout;
use crate::header::Drops;
use crate::model::Model;
use crate::screens::settings::statusline::{SegInfo, StatusSheet};
use crate::style::*;

/// The top bar, and the agent header (barTop, barAgent).
pub const BAR_TOP: usize = crate::screens::settings::statusline::BAR_TOP;
pub const BAR_AGENT: usize = crate::screens::settings::statusline::BAR_AGENT;

/// One thing a line can show: draw paints it, or returns an empty line
/// when it has nothing to say (barSeg).
pub struct Seg {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub draw: fn(&Ctx) -> Line,
}

/// What a line is drawn from: the fleet, and for the agent header the
/// agent and its conversation (barCtx).
pub struct Ctx<'a> {
    pub m: &'a Model,
    /// What every agent has spent today.
    pub today: f64,
    /// What's left of the line's width where a segment starts.
    pub room: usize,
    /// The agent the header is of.
    pub a: Option<&'a Of<'a>>,
}

impl<'a> Ctx<'a> {
    pub fn new(m: &'a Model) -> Ctx<'a> {
        Ctx {
            m,
            today: m.tally().today,
            room: usize::MAX / 4,
            a: None,
        }
    }
}

/// A bar's segments (barSegs).
pub fn segs(which: usize) -> &'static [Seg] {
    if which == BAR_TOP {
        super::top::SEGS
    } else {
        super::agent::SEGS
    }
}

fn find(which: usize, id: &str) -> Option<&'static Seg> {
    segs(which).iter().find(|s| s.id == id)
}

/// A bar's segments as /statusline lists them.
pub fn seg_infos(which: usize) -> Vec<SegInfo> {
    segs(which)
        .iter()
        .map(|s| SegInfo {
            id: s.id.into(),
            name: s.name.into(),
            about: s.about.into(),
        })
        .collect()
}

/// How long a segment holds on when its line is short of room: the
/// higher, the later it goes; the rest are 3 (barKeep).
fn keep(id: &str) -> u8 {
    match id {
        "clock" | "time" => 9,
        "plan" | "usage" | "context" | "system" => 8,
        "today" | "model" | "queue" => 7,
        "agents" | "effort" | "mode" | "folder" | "branch" => 6,
        "account" | "cost" | "billing" | "battery" => 5,
        "cache" => 4,
        "accounts" | "context-detail" => 2,
        "version" | "rush" | "statushelp" | "session" => 1,
        _ => 3,
    }
}

/// Which of ids goes first: the lowest keep, the last of those
/// (leastKept).
pub fn least_kept(ids: &[&str]) -> usize {
    let mut at = ids.len() - 1;
    for i in (0..ids.len().saturating_sub(1)).rev() {
        if keep(ids[i]) < keep(ids[at]) {
            at = i;
        }
    }
    at
}

fn joined(parts: &[Line], sep: &str) -> Line {
    let mut out = Line::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.spans.push(sp(sep, DIM));
        }
        join(&mut out, p.clone());
    }
    out
}

/// Line i of a bar, at most w wide. When it's too wide, the segments
/// that hold on least go, whole, and it ends ⋯; which went is put in
/// drops by which*10+i (barLine).
pub fn bar_line(
    m: &Model,
    which: usize,
    i: usize,
    x: &mut Ctx,
    w: usize,
    drops: &mut Drops,
) -> Line {
    let l = layout(m, which);
    if i >= l.lines.len() || w == 0 {
        return Line::new();
    }
    let sep = if l.sep.is_empty() { " · " } else { &l.sep };
    let sep_w = rush_core::text::width(sep);
    let mut parts: Vec<Line> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for id in &l.lines[i] {
        let Some(s) = find(which, id) else {
            continue;
        };
        let used: usize = parts.iter().map(|p| p.width() + sep_w).sum();
        x.room = w.saturating_sub(used);
        let p = (s.draw)(x);
        if p.width() > 0 {
            parts.push(p);
            ids.push(s.id);
        }
    }
    let mut out = joined(&parts, sep);
    let mut dropped = Vec::new();
    if out.width() > w {
        const MORE: &str = " ⋯";
        while !parts.is_empty() && joined(&parts, sep).width() + 2 > w {
            let k = least_kept(&ids);
            dropped.push(ids[k].to_string());
            parts.remove(k);
            ids.remove(k);
        }
        out = joined(&parts, sep);
        if parts.is_empty() {
            out = line([sp("⋯", FAINT)]);
        } else {
            out.spans.push(sp(MORE, FAINT));
        }
    }
    drops.push((which * 10 + i, dropped));
    out
}

/// The agent the builder's previews are of: the one it was opened on,
/// else the conversation open.
fn sheet_agent(m: &Model) -> String {
    m.sheet
        .as_deref()
        .and_then(|s| (s as &dyn std::any::Any).downcast_ref::<StatusSheet>())
        .map(|s| s.agent.clone())
        .filter(|k| !k.is_empty())
        .or_else(|| m.session.as_ref().map(|v| v.key.clone()))
        .unwrap_or_default()
}

/// One segment on its own, as the bar would draw it now, for agent key
/// ("" the one the builder was opened on, else the conversation open):
/// the builder hands its own agent, as it isn't m.sheet while it's drawn
/// (sample, statuslinesheet.go:737).
pub fn sample(m: &Model, which: usize, key: &str, id: &str) -> Line {
    let Some(s) = find(which, id) else {
        return Line::new();
    };
    let mut x = Ctx::new(m);
    if which == BAR_TOP {
        return (s.draw)(&x);
    }
    let key = if key.is_empty() {
        sheet_agent(m)
    } else {
        key.to_string()
    };
    let Some(of) = Of::read(m, &key) else {
        return Line::new();
    };
    x.a = Some(&of);
    (s.draw)(&x)
}

/// The bar as it's drawn now, w wide: rush's top, or agent key's header
/// rows (statuslinesheet.go:678).
pub fn preview(m: &Model, which: usize, key: &str, w: usize) -> Vec<Line> {
    if which == BAR_TOP {
        return m.head_rows(w).0;
    }
    let key = if key.is_empty() {
        sheet_agent(m)
    } else {
        key.to_string()
    };
    let Some(of) = Of::read(m, &key) else {
        return Vec::new();
    };
    let mut drops = Drops::new();
    let (r1, r2, _) = super::agent::rows(m, &of, w, &mut drops);
    vec![r1, r2]
}

/// What each line left out, kept by line (barDrops).
pub fn keep_drops(into: &mut HashMap<usize, Vec<String>>, drops: Drops) {
    for (k, v) in drops {
        into.insert(k, v);
    }
}
