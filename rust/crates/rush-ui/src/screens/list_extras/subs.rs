//! An agent's running subagents as rows under it in the list, at most
//! four, the last counting the rest; ← tucks them away. After Go's
//! internal/ui/subrows.go (maxSubRows :13, subagentsShown :17,
//! subagentLine :45, toggleSubagents :72) and subpreview.go
//! (wideSubagents :20, subListWidth :24). Rust's fleet reads how many
//! subagents run, not each run (Go's fleet.SubagentTile), so the rows
//! take the runs ready made; the list draws them once its rows can have
//! rows of their own under them.

use std::collections::HashSet;

use rush_core::text::Line;
use rush_core::time::Ms;

use super::jobs::spread;
use crate::model::Model;
use crate::style::*;

/// How many rows an agent's subagents take at most (maxSubRows).
pub const MAX_SUB_ROWS: usize = 4;

/// A subagent run as its row shows it (fleet.SubagentTile).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubTile {
    pub id: String,
    pub typ: String,
    pub description: String,
    /// When it began; 0 when unknown.
    pub born: Ms,
}

/// The agents whose subagents ← tucked away (subsFolded).
#[derive(Default)]
pub struct SubsFolded(pub HashSet<String>);

/// How many of key's n subagents are drawn, and how many more a last row
/// counts: none when they're tucked away (subagentsShown).
pub fn shown(folded: bool, n: usize) -> (usize, usize) {
    if n == 0 || folded {
        (0, 0)
    } else if n > MAX_SUB_ROWS {
        (MAX_SUB_ROWS - 1, n - (MAX_SUB_ROWS - 1))
    } else {
        (n, 0)
    }
}

/// How many rows key's subagents take under it.
pub fn rows(m: &Model, key: &str, n: usize) -> usize {
    let folded = m.kept.peek::<SubsFolded>().is_some_and(|f| f.0.contains(key));
    let (s, more) = shown(folded, n);
    s + usize::from(more > 0)
}

/// The i'th subagent row, w cells: what it was asked, its kind, and how
/// long it has run in the time column, time_w wide (subagentLine).
pub fn line_of(subs: &[SubTile], folded: bool, i: usize, w: usize, time_w: usize, now: Ms, tick: usize) -> Line {
    let (shown, more) = shown(folded, subs.len());
    let branch = if i == shown.saturating_sub(1) && more == 0 || i >= shown {
        "└ "
    } else {
        "├ "
    };
    if i >= shown {
        return fit(
            line([sp(branch, FAINT), sp(format!("{more} more subagents"), DIM)]),
            w,
        );
    }
    let s = &subs[i];
    let mut right = line([plain(" ")]);
    if time_w > 0 && s.born != 0 {
        right = line([sp(crate::style::right(&dur(now - s.born), time_w), DIM), plain(" ")]);
    }
    let glyph = sp(SPINNER[(tick + s.id.len()) % SPINNER.len()], ORANGE);
    let typ = if s.typ.is_empty() { "subagent" } else { &s.typ };
    // What it was asked leads, as the run's label does everywhere else.
    let what = one_line(&s.description);
    let left = if what.is_empty() {
        line([sp(branch, FAINT), glyph, plain(" "), sp(typ, SUB)])
    } else {
        line([
            sp(branch, FAINT),
            glyph,
            plain(" "),
            sp(what, TEXT),
            sp(" · ", FAINT),
            sp(typ, DIM),
        ])
    };
    let room = (w as isize - right.width() as isize - 2).max(1) as usize;
    spread(rush_draw::truncate_line(&left, room, "…"), right, w)
}

/// ← on an agent: its subagents' rows tuck away, or come back
/// (toggleSubagents).
pub fn toggle(m: &mut Model, key: &str) {
    let f = &mut m.kept::<SubsFolded>().0;
    if f.remove(key) {
        return;
    }
    f.insert(key.to_string());
    m.flash("subagents tucked away · ← shows them");
}

/// Whether the subagents view lists runs beside a preview of the one
/// picked: wide enough, none opened (wideSubagents).
pub fn wide(w: usize, opened: bool, runs: usize) -> bool {
    !opened && w >= 150 && runs > 0
}

/// How wide that list is beside the preview (subListWidth).
pub fn list_width(w: usize) -> usize {
    72.min(w * 2 / 5)
}
