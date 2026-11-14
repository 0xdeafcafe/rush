//! A run opened from the conversation: from its step, from the column's
//! Running part, or from the dock. It opens on the conversation's own page,
//! never a view of its own. Where the column beside the conversation has
//! room, it takes the column (its whole height, all the room right of the
//! text); else, or when Settings asks for it, it's a peek: a frame under
//! its step, as an editor peeks at a definition, its own conversation
//! scrolling inside and its box at its foot, the parent's conversation
//! going on under it. A step out of sight docks the frame over the box.
//! Either way what's open is a run (subview::peek: its head, conversation
//! and box) or a workflow (its phases, each agent a row that opens it).
//! The run has the keys as it opens; tab gives them back to the
//! conversation and takes them again, esc closes it, and ✕ on its edge
//! does too. Rust-only: Go, and the port before this, showed a run in the
//! subagents view, a tab of its own, and background tasks in another.

use rush_core::text::Line;

use super::conn;
use super::paint::faint;
use crate::model::Model;
use crate::style::*;

/// Where an open run shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In the column right of the conversation.
    Beside,
    /// Framed under its step, in the conversation.
    Peek,
}

/// The setting: "" opens a run beside the conversation where the column
/// fits, "peek" always under its step.
pub const PEEK: &str = "peek";

/// What's open in session key: a run's id or a workflow's ref.
pub fn open_id(m: &Model, key: &str) -> Option<String> {
    let c = m.kept.peek::<super::Core>()?.conns.get(key)?;
    (!c.watch.open.is_empty()).then(|| c.watch.open.clone())
}

/// Whether session key has a run or workflow open.
pub fn is_open(m: &Model, key: &str) -> bool {
    open_id(m, key).is_some()
}

/// Where what's open shows, the column cols wide where it fits (0 where
/// it doesn't).
pub fn place(m: &Model, cols: usize) -> Place {
    let peek = crate::accounts::loaded_config(m).is_some_and(|c| c.runs_open == PEEK);
    if cols == 0 || peek {
        Place::Peek
    } else {
        Place::Beside
    }
}

/// Opens run (or workflow) id in session key, from the conversation's
/// pick from: esc, closing it, picks that again. A run takes the keys
/// once it's drawn, if it has a conversation or a box.
pub fn open(m: &mut Model, key: &str, id: &str, from: &str) {
    let w = &mut conn(m, key).watch;
    if w.open != id {
        w.peek.txt = Default::default();
        w.peek.hover.clear();
        w.flow_refs.clear();
    }
    w.open = id.to_string();
    w.from = from.to_string();
    w.back = !from.is_empty();
    w.msg_focus = false;
    w.focus_next = id.to_string();
    super::subs::kick(m, key);
}

/// Closes what's open in session key: the keys go back to the
/// conversation, its pick back to where the run was opened from. A run
/// opened from a workflow's goes back to the workflow.
pub fn close(m: &mut Model, key: &str) {
    let (was, from) = {
        let w = &mut conn(m, key).watch;
        if w.open.is_empty() {
            return;
        }
        let was = std::mem::take(&mut w.open);
        w.msg_focus = false;
        w.focus_next.clear();
        w.peek.visible = false;
        w.msg_at = [0; 4];
        w.frame = [0; 4];
        w.flow_refs.clear();
        w.back = false;
        (was, std::mem::take(&mut w.from))
    };
    conn(m, key).runs.detail = None;
    if !is_flow(m, &was) && is_flow(m, &from) {
        open(m, key, &from, "");
        conn(m, key).watch.focus_next.clear();
        return;
    }
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
        v.sel = from;
    }
}

/// Whether ref r is a workflow's.
pub fn is_flow(m: &Model, r: &str) -> bool {
    !r.is_empty()
        && m.session
            .as_ref()
            .is_some_and(|v| super::workflow::is_workflow_ref(&v.sess, r))
}

/// The tool call of the step run id was started by, where the peek hangs
/// under; None for a workflow, or a run whose step isn't known.
pub fn anchor(m: &Model, key: &str, id: &str) -> Option<String> {
    let r = super::subview::runs_of(m, key)?;
    let sa = r.subs.iter().find(|sa| sa.id == id)?;
    (!sa.tool_use_id.is_empty()).then(|| sa.tool_use_id.clone())
}

/// Whether a conversation row's ref r is call's step, or one of the
/// agents it ran ("t3:s:toolu_1", "t3:s:toolu_1/2").
pub fn is_step_of(r: &str, call: &str) -> bool {
    r.split_once(":s:")
        .is_some_and(|(_, s)| s == call || s.strip_prefix(call).is_some_and(|t| t.starts_with('/')))
}

/// How tall the peek is in a conversation rows tall: three fifths of it,
/// ten rows at least, never all of it.
pub fn peek_rows(rows: usize) -> usize {
    if rows < 6 {
        return 0;
    }
    (rows * 3 / 5).max(10).min(rows - 2)
}

/// What's open, rows tall and w wide, its first cell at (x, y) on the
/// screen: a run's head, conversation and box, or a workflow's phases and
/// agents.
pub fn draw(m: &mut Model, key: &str, w: usize, rows: usize, (x, y): (usize, usize)) -> Vec<Line> {
    let mut out = vec![Line::new(); rows];
    let Some(id) = open_id(m, key) else {
        return out;
    };
    if is_flow(m, &id) {
        let w_ = &mut conn(m, key).watch;
        w_.peek.visible = false;
        w_.msg_at = [0; 4];
        let (lines, refs) = super::subview::flow_lines(m, key, &id, w);
        let mut refs = refs;
        refs.resize(rows, String::new());
        refs.truncate(rows);
        for (i, l) in lines.into_iter().enumerate().take(rows) {
            out[i] = l;
        }
        let w_ = &mut conn(m, key).watch;
        w_.flow_refs = refs;
        w_.flow_at = [x, y, w, rows];
        return out;
    }
    super::subview::peek(m, key, &id, &mut out, w, (x, y));
    out
}

/// The run open, framed as a peek under its step: h rows, w wide, its top
/// edge at screen row y. The edge says how to get out of it and lights
/// while it has the keys; ✕ on it closes it.
pub fn frame(m: &mut Model, key: &str, w: usize, h: usize, y: usize) -> Vec<Line> {
    if w < 12 || h < 3 {
        return Vec::new();
    }
    let iw = w - 4;
    // Drawn first: an opened run takes the keys as it's drawn.
    let body = draw(m, key, iw, h - 2, (2 + 2, y + 1));
    let focused = conn(m, key).watch.msg_focus;
    let edge = if focused { SUB } else { FAINT };
    let says = if focused {
        " tab back · esc closes "
    } else {
        " tab to it · esc closes "
    };
    let close = " ✕ ";
    let used = 1 + rush_core::text::width(says) + rush_core::text::width(close) + 1;
    let mut out = Vec::with_capacity(h);
    let mut top = line([
        sp("╭", edge),
        sp("─".repeat(w.saturating_sub(used + 1)), edge),
    ]);
    top.spans.push(faint(says));
    top.spans.push(sp(close, edge));
    top.spans.push(sp("─╮", edge));
    out.push(fit(top, w));
    for l in body {
        let mut r = line([sp("│ ", edge)]);
        join(&mut r, fit(l, iw));
        r.spans.push(sp(" │", edge));
        out.push(r);
    }
    out.push(line([
        sp("╰", edge),
        sp("─".repeat(w - 2), edge),
        sp("╯", edge),
    ]));
    let wt = &mut conn(m, key).watch;
    wt.frame = [2, y, w, h];
    // ✕ sits left of the top edge's last two cells.
    wt.close_at = (2 + w - 2 - rush_core::text::width(close), y);
    out
}

/// The column beside the conversation with the run open in it: rows tall
/// and w wide, its first cell at (x, y), a rule down its left so it reads
/// as its own, its top row the way out.
pub fn beside(
    m: &mut Model,
    key: &str,
    w: usize,
    rows: usize,
    (x, y): (usize, usize),
) -> Vec<Line> {
    if w < 8 || rows < 2 {
        return vec![Line::new(); rows];
    }
    let iw = w - 2;
    // Drawn first: an opened run takes the keys as it's drawn.
    let body = draw(m, key, iw, rows - 1, (x + 2, y + 1));
    let focused = conn(m, key).watch.msg_focus;
    let edge = if focused { SUB } else { FAINT };
    let says = if focused {
        "tab back · esc closes"
    } else {
        "tab to it · esc closes"
    };
    let close = "✕ ";
    let bar = super::dock::spread(
        line([sp("│ ", edge), faint(says)]),
        line([sp(close, edge)]),
        w,
    );
    let mut out = Vec::with_capacity(rows);
    out.push(bar);
    for l in body {
        let mut r = line([sp("│ ", edge)]);
        join(&mut r, fit(l, iw));
        out.push(r);
    }
    let wt = &mut conn(m, key).watch;
    wt.frame = [x, y, w, rows];
    wt.close_at = (x + w - rush_core::text::width(close), y);
    out
}

/// Whether screen cell (x, y) is on what's open's ✕.
pub fn on_close(m: &mut Model, key: &str, x: usize, y: usize) -> bool {
    let (cx, cy) = conn(m, key).watch.close_at;
    cy == y && cx > 0 && (cx..cx + 2).contains(&x)
}

/// Whether screen cell (x, y) is inside what's open, as last drawn.
pub fn inside(m: &mut Model, key: &str, x: usize, y: usize) -> bool {
    let [fx, fy, fw, fh] = conn(m, key).watch.frame;
    fw > 0 && x >= fx && x < fx + fw && y >= fy && y < fy + fh
}

/// The workflow's agent on screen row y of its pane, if one is there.
pub fn flow_ref_at(m: &mut Model, key: &str, x: usize, y: usize) -> Option<String> {
    let w = &conn(m, key).watch;
    let [fx, fy, fw, fh] = w.flow_at;
    if fw == 0 || x < fx || x >= fx + fw || y < fy || y >= fy + fh {
        return None;
    }
    w.flow_refs.get(y - fy).filter(|r| !r.is_empty()).cloned()
}
