//! The conversation's views: Normal, Chat, Code, Catch-up and Trail
//! (rush_convo::Preset), one for every session, kept in rush's config
//! (convoView) until changed. The key that goes through them, the picker
//! the header's label opens, and enter or a click on a turn or a step in
//! any of them (a chat bubble's foot, a code heading or check, the
//! catch-up card or a turn before it, a trail's row) opening it in
//! Normal. Changing view keeps the view on the message it was on. New in
//! the Rust port: Go rush had no such views.

use rush_convo::Preset;
use rush_core::text::{Color, Line, Span, Style};

use super::paint::spread;
use super::{conn, open_key, views};
use crate::frame::{Out, Pick, Picker, Press};
use crate::model::Model;
use crate::style::*;

/// The key that goes through the views (session.preset's own).
pub const KEY: &str = "alt+s";

/// The fewest rows the conversation has for its tabs to be drawn above it.
pub const TABS_MIN_ROWS: usize = 6;

/// The views' tab row and the rule under it, `w` wide: the view showing
/// lit with a heavy rule under it (lit: its colour), the others dim, the
/// key right-aligned and faint. A view a small model suggests (suggested)
/// has a `✦` in orange before its name. Names shorten, then the key goes,
/// when `w` is narrow; never wraps. With the cells each tab takes (from and
/// to, in the row) and the view it is.
pub fn tabs(
    cur: Preset,
    suggested: Option<Preset>,
    key: &str,
    w: usize,
    lit: Color,
) -> (Vec<Line>, Vec<(usize, usize, Preset)>) {
    let name = |p: Preset, short: bool| -> &'static str {
        match (p, short) {
            (Preset::CatchUp, true) => "Catch",
            (Preset::Normal, true) => "Norm",
            _ => p.title(),
        }
    };
    let width_of = |short: bool| -> usize {
        Preset::ALL
            .iter()
            .map(|p| {
                let star = if suggested == Some(*p) { 2 } else { 0 };
                name(*p, short).chars().count() + 2 + star
            })
            .sum::<usize>()
            + 1
    };
    let short = width_of(false) > w;
    let mut top = Line::new();
    let mut rule = Line::new();
    let mut at = Vec::new();
    // One cell in, as the names' own gap.
    top.spans.push(plain(" "));
    rule.spans.push(sp("─", FAINT));
    for p in Preset::ALL {
        let star = suggested == Some(p);
        let label = name(p, short);
        let on = p == cur;
        let x = top.width();
        let tw = label.chars().count() + 2 + if star { 2 } else { 0 };
        // ` ✦ Name ` (the mark before the name) or ` Name `.
        let style = if on {
            bold(BRIGHT).on(TAB_ON_BG)
        } else {
            Style::fg(SUB)
        };
        top.spans.push(Span::new(" ", style));
        if star {
            top.spans.push(Span::new(
                "✦ ",
                if on {
                    Style::fg(ORANGE).on(TAB_ON_BG)
                } else {
                    Style::fg(ORANGE)
                },
            ));
        }
        top.spans.push(Span::new(label, style));
        top.spans.push(Span::new(" ", style));
        at.push((x, x + tw, p));
        let (glyph, c) = if on { ("━", lit) } else { ("─", FAINT) };
        rule.spans.push(sp(glyph.repeat(tw), c));
    }
    let used = top.width();
    // The key, when there's room for it a cell apart.
    let k = line([sp(key, FAINT)]);
    let top = if used + 2 + k.width() < w {
        spread(top, k, w - 1)
    } else {
        top
    };
    let fill = w.saturating_sub(rule.width());
    rule.spans.push(sp("─".repeat(fill), FAINT));
    (vec![fit(top, w), fit(rule, w)], at)
}

/// The view every session's conversation shows.
pub fn current(m: &Model) -> Preset {
    let s = crate::accounts::loaded_config(m)
        .map_or(m.cfg.convo_view.as_str(), |c| c.convo_view.as_str());
    Preset::from_name(s)
}

/// The key for the views as the hint says it: yours if you moved it.
pub fn key_hint(m: &mut Model) -> String {
    let km = crate::keybind::key_map(m);
    let keys: Vec<String> = km
        .keys("session.preset")
        .iter()
        .map(|k| k.to_string())
        .collect();
    keys.iter()
        .find(|k| !k.contains(' '))
        .or(keys.first())
        .cloned()
        .unwrap_or_else(|| KEY.to_string())
}

/// Keeps p as the view, in the config and on disk.
fn keep(m: &mut Model, p: Preset) {
    let name = if p == Preset::Normal {
        String::new()
    } else {
        p.name().to_string()
    };
    m.cfg.convo_view = name.clone();
    rush_ui_base::config::save(m, "couldn't keep the conversation's view", move |c| {
        c.convo_view = name.clone()
    });
}

/// The rows the conversation had as last drawn.
fn rows(m: &mut Model) -> usize {
    match open_key(m) {
        Some(k) => conn(m, &k).vs.rows.max(1),
        None => 1,
    }
}

/// You chose p: shown (see show), and a suggested view taken or turned
/// down by it (viewsuggest.rs).
pub fn set(m: &mut Model, p: Preset) {
    show(m, p);
    super::viewsuggest::chose(m, p);
}

/// Shows p in every session's conversation, the one open kept on the
/// message it was on: the picked row's, else the turn at its top.
pub fn show(m: &mut Model, p: Preset) {
    keep(m, p);
    let rows = rows(m);
    if let Some(v) = m.session.as_mut() {
        v.o.preset = p;
        // A catch-up card is a turn: a step picked picks its turn.
        if p == Preset::CatchUp
            && let Some((t, _)) = v.sel.split_once(':')
            && t.starts_with('t')
        {
            v.sel = t.to_string();
        }
        let sel = v.sel.clone();
        if sel.starts_with('t') && sel[1..].starts_with(|c: char| c.is_ascii_digit()) {
            if !v.reveal(&sel, rows) {
                v.sel.clear();
            }
        } else if let crate::session::Pos::At { n, .. } = v.pos {
            v.pos = crate::session::Pos::At { n, off: 0 };
        }
    }
    m.flash(&format!("{} · {}", p.title(), p.about()));
}

/// Row r (a catch-up card's turn, a trail's step) opened in Normal, in
/// view and picked.
pub fn open_in_normal(m: &mut Model, r: &str) -> Out {
    keep(m, Preset::Normal);
    super::viewsuggest::chose(m, Preset::Normal);
    let rows = rows(m);
    if let Some(v) = m.session.as_mut() {
        v.o.preset = Preset::Normal;
        let turn = r.split(':').next().unwrap_or("").to_string();
        v.o.open.insert(turn.clone(), true);
        if let Some((_, call)) = r.split_once(":s:") {
            v.o.open.insert(r.to_string(), true);
            // A finished turn folds its steps into runs: the run it's in
            // opens too. It starts at one of the steps just before it.
            for at in run_starts(&v.sess, &turn, call) {
                v.o.open.insert(format!("{turn}:run:{at}"), true);
            }
        }
        v.sel = r.to_string();
        v.reveal(r, rows);
    }
    m.flash("Normal · everything, as usual");
    Out::Draw
}

/// Where in turn's items a run holding step call may start: the step's
/// own place and each step or thought just before it.
fn run_starts(s: &rush_convo::Session, turn: &str, call: &str) -> Vec<usize> {
    use rush_convo::Item;
    let Some(n) = turn.strip_prefix('t').and_then(|n| n.parse::<u32>().ok()) else {
        return Vec::new();
    };
    let Some(t) = s.turns.iter().find(|t| t.n == n) else {
        return Vec::new();
    };
    let Some(i) = t
        .items
        .iter()
        .position(|it| matches!(it, Item::Step(id) if &*s.step(*id).id == call))
    else {
        return Vec::new();
    };
    let mut out = vec![i];
    let mut j = i;
    while j > 0 && matches!(t.items[j - 1], Item::Step(_) | Item::Thinking(_)) {
        j -= 1;
        out.push(j);
    }
    out
}

/// Whether r is a turn's ref or a step's in one: what a catch-up card or
/// a trail's row opens.
pub fn turn_or_step(r: &str) -> bool {
    let t = r.split(':').next().unwrap_or("");
    t.len() > 1
        && t.starts_with('t')
        && t[1..].bytes().all(|b| b.is_ascii_digit())
        && (t.len() == r.len() || r[t.len()..].starts_with(":s:"))
}

/// The views' keys in the conversation: the key through them; enter, space
/// or → on a turn or a step in another view opens it in Normal.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || views::view_name(m, &key) != "conversation" {
        return Out::Pass;
    }
    let s = k.s.as_str();
    if s == KEY {
        let p = current(m).next();
        set(m, p);
        return Out::Draw;
    }
    if current(m) == Preset::Normal || !matches!(s, "enter" | "space" | " " | "right") {
        return Out::Pass;
    }
    let sel = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    if !turn_or_step(&sel) {
        return Out::Pass;
    }
    open_in_normal(m, &sel)
}

/// A click on row r of the conversation in a view other than Normal: a
/// turn's opens it at once, a step's once picked.
pub fn click(m: &mut Model, r: &str) -> Option<Out> {
    if current(m) == Preset::Normal || !turn_or_step(r) {
        return None;
    }
    let picked = m.session.as_ref().is_some_and(|v| v.sel == r);
    if !picked && r.contains(":s:") {
        return None; // picked first, as any step
    }
    Some(open_in_normal(m, r))
}

/// The views to pick from, the one showing first under the cursor.
pub fn open_picker(m: &mut Model) {
    let cur = current(m);
    let rows: Vec<Pick> = Preset::ALL
        .iter()
        .map(|p| {
            let mark = if *p == cur { "● " } else { "  " };
            let mut pick = Pick::new(p.title());
            pick.label = line([
                sp(mark, ORANGE),
                sp(format!("{:<10}", p.title()), TEXT),
                sp(p.about(), DIM),
            ]);
            pick.value = p.name().to_string();
            pick
        })
        .collect();
    let mut picker = Picker::new(
        "Conversation view",
        rows,
        Box::new(|m, pick, _| {
            let Some(p) = pick else {
                return Out::Idle;
            };
            let p = Preset::from_name(&p.value);
            set(m, p);
            Out::Draw
        }),
    );
    picker.cursor = Preset::ALL.iter().position(|p| *p == cur).unwrap_or(0);
    picker.keys = vec![
        ("↑↓".into(), "choose".into()),
        ("enter".into(), "show it".into()),
        ("esc".into(), "close".into()),
    ];
    m.open_sheet(Box::new(picker));
}
