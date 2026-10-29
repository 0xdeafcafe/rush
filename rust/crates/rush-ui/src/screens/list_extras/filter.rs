//! The Agents list's own filter, opened with alt+f: it narrows the list
//! to agents whose name matches what's typed. After Go's
//! internal/ui/listfilter.go (startListFilter :28, listFilterKey :36,
//! listFilterMatch :158), keys.go:510 (alt+f), model.go:2249 (the rows
//! left out), view.go:1700 (the empty list) and view.go:2013
//! (listFilterHeader). Not here yet: searching what was said in each
//! transcript (Go's barSearch, the command bar's), so a name is all it
//! matches.

use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_draw::KeyMods;

use super::fuzzy;
use crate::frame::{Out, Press, editor};
use crate::model::Model;
use crate::style::*;

/// The filter while it's open (listFilterState).
#[derive(Default)]
pub struct FilterState {
    pub open: bool,
    pub query: Vec<char>,
}

fn state(m: &Model) -> Option<&FilterState> {
    m.kept.peek::<FilterState>().filter(|f| f.open)
}

/// The filter's query, while it narrows the list.
pub fn query(m: &Model) -> Option<String> {
    state(m)
        .filter(|f| !f.query.is_empty())
        .map(|f| f.query.iter().collect())
}

/// Opens the filter, empty: nothing narrows until something's typed
/// (startListFilter).
pub fn start(m: &mut Model) {
    *m.kept::<FilterState>() = FilterState {
        open: true,
        query: Vec::new(),
    };
}

/// A key in the list: while the filter is open, typing narrows it and
/// esc and backspace step back out; anything else (the arrows, enter) is
/// the list's, which by then sees only what matches (listFilterKey).
/// alt+f with nothing typed opens it.
pub(super) fn key(m: &mut Model, k: &Press) -> Out {
    if state(m).is_none() {
        if k.s == "alt+f" && m.input.is_empty() {
            start(m);
            return Out::Draw;
        }
        return Out::Pass;
    }
    let f = m.kept::<FilterState>();
    match k.s.as_str() {
        "esc" => {
            if f.query.is_empty() {
                f.open = false;
            } else {
                f.query.clear();
            }
        }
        "backspace" | "ctrl+h" => {
            if f.query.pop().is_none() {
                f.open = false;
            }
        }
        "ctrl+w" | "alt+backspace" | "ctrl+backspace" | "ctrl+u" | "super+backspace" => {
            if let Some((q, _)) = editor::edit(&f.query, f.query.len(), k) {
                f.query = q;
            }
        }
        _ => {
            let Some(c) = k.text() else {
                return Out::Pass;
            };
            if k.mods.0 & !KeyMods::SHIFT.0 != 0 {
                return Out::Pass;
            }
            f.query.push(c);
        }
    }
    m.rebuild();
    Out::Draw
}

/// Whether the list shows a: with something typed, only what matches by
/// name (listFilterMatch).
pub(super) fn shows(m: &Model, a: &Agent) -> bool {
    match query(m) {
        Some(q) => fuzzy::matches(&q, &a.display_name).is_some(),
        None => true,
    }
}

/// The column header while the filter is open: what's typed and how many
/// agents match (listFilterHeader).
pub(super) fn header(m: &Model, w: usize) -> Option<Line> {
    let f = state(m)?;
    let label = if f.query.is_empty() {
        "  ⌕ filter".to_string()
    } else {
        format!("  ⌕ filter: {}", f.query.iter().collect::<String>())
    };
    let n = m.lines.iter().filter(|l| l.is_row()).count();
    let meta = format!("{n} agent{}", if n == 1 { "" } else { "s" });
    let mw = rush_core::text::width(&meta);
    let left = fit(
        line([spb(label, SUB)]),
        (w as isize - mw as isize - 4).max(0) as usize,
    );
    let gap = w as isize - left.width() as isize - mw as isize - 1;
    if gap < 1 {
        return Some(fit(left, w));
    }
    let mut l = left;
    l.spans.push(plain(blanks(gap as usize)));
    l.spans.push(sp(meta, DIM));
    l.spans.push(plain(" "));
    Some(l)
}

/// What the list says when nothing matches.
pub(super) fn empty(m: &Model) -> Option<String> {
    query(m).map(|q| format!("  No agents match {q:?}."))
}
