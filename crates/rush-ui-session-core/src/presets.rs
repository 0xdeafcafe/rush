//! The conversation's views: Normal, Chat, Code, Catch-up and Trail
//! (rush_convo::Preset), one for every session, kept in rush's config
//! (convoView) until changed. The dropdown under the Conversation tab
//! that picks one (a click on the tab while it shows, or the views' key),
//! and enter or a click on a turn or a step in any of them (a chat
//! bubble's foot, a code heading or check, the catch-up card or a turn
//! before it, a trail's row) opening it in Normal. Changing view keeps
//! the view on the message it was on. New in the Rust port: Go rush had
//! no such views.

use rush_convo::Preset;
use rush_core::text::{Line, Span};
use rush_draw::{Button, MouseKind};

use super::{conn, open_key, views};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::style::*;

/// The key that opens the views' dropdown (session.preset's own).
pub const KEY: &str = "alt+s";

/// What the dropdown says under its rows.
pub const MENU_HOW: &str = "↑↓ · enter shows it · esc closes";

/// The views' dropdown under the Conversation tab, `w` wide at most: a
/// row for each view, the one showing marked `●`, the one suggested with
/// `✦` and why, and what each is for; its keys under them. Row sel is
/// picked. Drawn as the @ picker's rows are: on the chrome's ground, the
/// one picked on the selection's with `▸`.
pub fn menu_rows(
    cur: Preset,
    suggested: Option<(Preset, String)>,
    sel: usize,
    w: usize,
) -> Vec<Line> {
    let body = |p: Preset| -> Line {
        let mark = if p == cur {
            sp("● ", ORANGE)
        } else {
            plain("  ")
        };
        let mut l = line([
            mark,
            Span::new(format!("{:<9}", p.title()), bold(BRIGHT)),
            plain("  "),
            sp(p.about(), DIM),
        ]);
        if let Some((s, why)) = &suggested
            && *s == p
        {
            l.spans.push(plain("  "));
            l.spans.push(sp("✦", ORANGE));
            if !why.is_empty() {
                l.spans.push(sp(format!(" {why}"), DIM));
            }
        }
        l
    };
    let foot = line([plain("   "), sp(MENU_HOW, DIM)]);
    let bodies: Vec<Line> = Preset::ALL.iter().map(|p| body(*p)).collect();
    let want = bodies
        .iter()
        .map(|l| l.width() + 4)
        .chain(std::iter::once(foot.width() + 1))
        .max()
        .unwrap_or(0);
    let mw = want.min(w);
    let mut out: Vec<Line> = bodies
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            if i == sel {
                let mut l = line([sp(" ▸ ", ORANGE)]);
                join(&mut l, b);
                on_ground(l, mw, SEL_BG)
            } else {
                let mut l = line([plain("   ")]);
                join(&mut l, b);
                on_ground(l, mw, CHROME_BG)
            }
        })
        .collect();
    out.push(on_ground(foot, mw, CHROME_BG));
    out
}

/// Whether session key's views dropdown is open.
pub fn menu_open(m: &mut Model, key: &str) -> bool {
    conn(m, key).vs.menu.is_some()
}

/// Opens session key's views dropdown, the view showing picked.
pub fn open_menu(m: &mut Model, key: &str) {
    let cur = current(m);
    let i = Preset::ALL.iter().position(|p| *p == cur).unwrap_or(0);
    conn(m, key).vs.menu = Some(i);
}

/// Lays the dropdown, when it's open, over the conversation's first rows
/// (out, `w` wide), under the Conversation tab; where it landed is kept
/// for the mouse.
pub fn draw_menu(m: &mut Model, key: &str, out: &mut [Line], w: usize) {
    let Some(sel) = conn(m, key).vs.menu else {
        conn(m, key).vs.menu_at = (0, 0, 0);
        return;
    };
    let cur = current(m);
    let sug = super::viewsuggest::shown(m, key)
        .filter(|s| s.from.is_none())
        .map(|s| (s.view, s.reason));
    // Under the tab: Conversation is the first, a cell in.
    let left = conn(m, key)
        .vs
        .tabs
        .first()
        .map_or(1, |t| t.0)
        .min(w.saturating_sub(1));
    let rows = menu_rows(cur, sug, sel, w.saturating_sub(left));
    let n = rows.len().min(out.len());
    let mw = rows.first().map_or(0, |l| l.width());
    crate::frame::dialog::paste_at(out, &rows[..n], 0, left);
    conn(m, key).vs.menu_at = (left, mw, n);
}

/// The dropdown's keys while it's open: ↑↓ choose, enter shows the one
/// chosen, esc or the views' key closes it. Any other key closes it and
/// goes on to do what it does.
pub fn menu_key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() {
        return Out::Pass;
    }
    let Some(sel) = conn(m, &key).vs.menu else {
        return Out::Pass;
    };
    if views::view_name(m, &key) != "conversation" {
        conn(m, &key).vs.menu = None;
        return Out::Pass;
    }
    let n = Preset::ALL.len();
    let s = k.s.as_str();
    match s {
        "up" => conn(m, &key).vs.menu = Some((sel + n - 1) % n),
        "down" => conn(m, &key).vs.menu = Some((sel + 1) % n),
        "enter" => {
            conn(m, &key).vs.menu = None;
            set(m, Preset::ALL[sel % n]);
        }
        "esc" => conn(m, &key).vs.menu = None,
        _ if s == KEY => conn(m, &key).vs.menu = None,
        _ => {
            conn(m, &key).vs.menu = None;
            return Out::Pass;
        }
    }
    Out::Draw
}

/// The pointer while the dropdown is open: over a view's row it picks
/// that row, a click on one shows that view, and a click anywhere else
/// (the Conversation tab's too) closes it.
pub fn menu_mouse(m: &mut Model, e: &Mouse) -> Out {
    if m.session.is_none() || m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let Some(sel) = conn(m, &key).vs.menu else {
        return Out::Pass;
    };
    let (left, mw, n) = conn(m, &key).vs.menu_at;
    let top = m.session.as_ref().map_or(0, |v| v.top);
    let (x, y) = (e.x as usize, e.y as usize);
    // The dropdown's row under the pointer: a view's, or its keys' last.
    let row = if y >= top && y < top + n && x >= 2 + left && x < 2 + left + mw {
        Some(y - top)
    } else {
        None
    };
    match e.kind {
        MouseKind::Moved => match row {
            Some(r) if r < Preset::ALL.len() && r != sel => {
                conn(m, &key).vs.menu = Some(r);
                Out::Draw
            }
            _ => Out::Pass,
        },
        MouseKind::Down(Button::Left) => match row {
            Some(r) if r < Preset::ALL.len() => {
                conn(m, &key).vs.menu = None;
                set(m, Preset::ALL[r]);
                Out::Draw
            }
            Some(_) => Out::Idle,
            None => {
                conn(m, &key).vs.menu = None;
                Out::Draw
            }
        },
        _ => Out::Pass,
    }
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

/// The views' keys in the conversation: the key opens the dropdown under
/// the Conversation tab (from any of the session's views, showing the
/// conversation); enter, space or → on a turn or a step in another view
/// opens it in Normal.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() {
        return Out::Pass;
    }
    let s = k.s.as_str();
    // A room's conversation is its log: no views.
    if s == KEY && m.agents.iter().any(|a| a.key == key) {
        if views::view_name(m, &key) != "conversation" {
            views::show_view(m, &key, "conversation");
        }
        open_menu(m, &key);
        return Out::Draw;
    }
    if views::view_name(m, &key) != "conversation" {
        return Out::Pass;
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
