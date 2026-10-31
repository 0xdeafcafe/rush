//! The Agents list's own filter, opened with alt+f: it narrows the list
//! to agents whose name or transcript matches what's typed. Names match
//! at once; once typing pauses, what was said is searched in the
//! background as the command bar searches it, and a row found that way
//! shows its hit. After Go's internal/ui/listfilter.go (startListFilter
//! :28, listFilterKey :36, applyListFilter :75, searchListFilter :93,
//! listFilterMsg :124, listFilterMatch :158, hasFilterMatch :170,
//! filterSnippet :186), keys.go:510 (alt+f), model.go:2249 (the rows
//! left out), view.go:1700 (the empty list), view.go:2013
//! (listFilterHeader) and view.go:2100, :2357, :2394 (the hit shown).

use std::collections::HashMap;

use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_draw::KeyMods;
use rush_fleet::Predicates;

use super::fuzzy;
use crate::frame::{Out, Press, editor};
use crate::model::Model;
use crate::screens::cmdbar::bar::{lit_words, searchable, who_glyph};
use crate::screens::cmdbar::draw::lit_title;
use crate::screens::cmdbar::search::{BarFound, Search, conversation_of, search_all};
use crate::style::*;

/// The filter while it's open (listFilterState).
#[derive(Default)]
pub struct FilterState {
    pub open: bool,
    pub query: Vec<char>,
    /// Which query the search under way is for.
    pub r#gen: u64,
    pub search: Option<Search>,
    /// Each agent's hits, by key, as the search reports them.
    pub found: HashMap<String, BarFound>,
    pub done: bool,
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
    let r#gen = m.kept::<FilterState>().r#gen + 1;
    *m.kept::<FilterState>() = FilterState {
        open: true,
        r#gen,
        ..FilterState::default()
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
            f.search = None;
            if f.query.is_empty() {
                f.open = false;
            } else {
                f.r#gen += 1;
                f.query.clear();
                f.found.clear();
                f.done = false;
            }
            m.rebuild();
            return Out::Draw;
        }
        "backspace" | "ctrl+h" => {
            if f.query.pop().is_none() {
                f.search = None;
                f.open = false;
                m.rebuild();
                return Out::Draw;
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
    apply(m);
    Out::Draw
}

/// The list again for what's typed so far, then, once there's enough of
/// a word, every transcript searched for it in the background after a
/// pause (applyListFilter, searchListFilter).
fn apply(m: &mut Model) {
    let f = m.kept::<FilterState>();
    f.search = None;
    f.r#gen += 1;
    f.found.clear();
    f.done = false;
    let (r#gen, q) = (f.r#gen, f.query.iter().collect::<String>());
    m.rebuild();
    if !searchable(&q) {
        return;
    }
    // No loop (tests, --render): the pause never ends; search_now does it
    // here and now.
    let Some(tx) = m.sender() else { return };
    let jobs = jobs(m);
    let f = m.kept::<FilterState>();
    if jobs.is_empty() {
        f.done = true;
        return;
    }
    f.search = Some(Search::start_to(tx, r#gen, jobs, q, true, took));
}

/// Every agent with a conversation to search, the most recently active
/// first (searchListFilter's hasConversation).
fn jobs(m: &Model) -> Vec<crate::screens::cmdbar::search::Job> {
    let now = m.now;
    let mut agents: Vec<&Agent> = m.agents.iter().collect();
    agents.sort_by_key(|a| a.age(now));
    agents
        .iter()
        .filter_map(|a| {
            conversation_of(&m.reg, a).map(|s| (a.key.clone(), one_line(&a.display_name), s))
        })
        .collect()
}

/// Searches every transcript for the query here and now, with no loop to
/// hand batches back through.
pub fn search_now(m: &mut Model) {
    let Some(q) = query(m).filter(|q| searchable(q)) else {
        return;
    };
    let jobs = jobs(m);
    let r#gen = m.kept::<FilterState>().r#gen;
    m.kept::<FilterState>().search = Some(Search::idle());
    took(m, r#gen, search_all(&jobs, &q), true);
}

/// A batch of what the search found (listFilterMsg's listFilterFoundMsg).
fn took(m: &mut Model, r#gen: u64, found: Vec<BarFound>, done: bool) -> Out {
    let f = m.kept::<FilterState>();
    if !f.open || f.r#gen != r#gen || f.search.is_none() {
        return Out::Idle;
    }
    for x in found {
        if x.found.total > 0 {
            f.found.insert(x.key.clone(), x);
        }
    }
    f.done = done;
    m.rebuild();
    Out::Draw
}

/// Whether the filter found a's match in what was said, not just its name
/// (hasFilterMatch).
pub fn has_match(m: &Model, a: &Agent) -> bool {
    state(m).is_some_and(|f| {
        !f.query.is_empty()
            && f.found
                .get(&a.key)
                .is_some_and(|x| !x.found.hits.is_empty())
    })
}

/// The match the filter found in a's transcript, lit, fit to w cells;
/// None with no filter, or when a matched only by name (filterSnippet).
pub fn snippet(m: &Model, a: &Agent, w: usize) -> Option<Line> {
    if w < 8 || !has_match(m, a) {
        return None;
    }
    let f = state(m)?;
    let h = f.found.get(&a.key)?.found.hits.first()?;
    let q: String = f.query.iter().collect();
    let words = rush_convo::search::words(&q);
    let mut l = line([who_glyph(&h.who), plain(" ")]);
    join(
        &mut l,
        lit_title(&h.snippet, &lit_words(&h.snippet, &words), w - 2),
    );
    Some(l)
}

/// Whether the list shows a: with something typed, only what matches by
/// name, or (once the search reaches it) by something said in it
/// (listFilterMatch).
pub(super) fn shows(m: &Model, a: &Agent) -> bool {
    match query(m) {
        Some(q) => fuzzy::matches(&q, &a.display_name).is_some() || has_match(m, a),
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
