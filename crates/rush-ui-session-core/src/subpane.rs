//! The run picked in the subagents view, in its pane beside the list (or
//! under it): its conversation drawn and driven as a session's own, at
//! the pane's width. Never a screen of its own: tab, enter or → on the
//! run, or a click in its pane, gives the pane the keys, and then ↑↓ pick
//! its rows, enter → ← and ctrl+o open and close them and show all, o
//! opens a step's file, PgUp/PgDn, Home and End move it (End follows its
//! tail), a click picks or opens a row or a link, dragged-over text is
//! copied; and what's typed goes to the run's own box (subbox.rs), its
//! queue picked with ↑, routed as relay.rs routes it. esc gives the keys
//! back to the list, then to where the run was picked from.
//!
//! The run's conversation is the SessionView subs.rs reads for the run
//! picked (`Runs::detail`), laid out off the UI thread as the pane draws
//! it; `as_run` puts it in m.session's place for the length of a call (a
//! swap of the two views, nothing copied or laid out again), so the
//! Session's own drawing (conversation::rows_of: the pins, the pill, the
//! row under the pointer, dragged-over text), keys and clicks work on it
//! unchanged, while the parent's live link, box, queue and cards stay the
//! parent's. Rust-only: Go peeked at a run beside its list
//! (subpreview.go drawSubPreview :94) and opened it whole under a banner
//! (rushmode.go openSub, watchRun :777).

use rush_core::text::Line;
use rush_core::time::Ms;
use rush_draw::{Button, MouseKind};

use super::paint::{dim, faint};
use super::{conn, core, open_key};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::session::{HEAD_H, Pos, SessionView};
use crate::style::*;

/// Lays the run's view out as its pane draws it: w wide, drawn with the
/// keys or without, its working line while it works. The reader lays
/// out what it reads the same way (SessionView::note_drawn).
pub fn prime(v: &mut SessionView, w: usize, (now, tick): (Ms, usize), live: bool, focused: bool) {
    v.side = 0;
    v.idle = !focused;
    v.sync(w, now, tick);
    // Its working line under its conversation, as the session's, while
    // it works; finished or waiting, how it stands says instead.
    v.o.hide_activity = !live;
}

/// Runs f with run id's view in m.session's place, its dragged-over
/// text, rows and the row under the pointer in the session's: the
/// Session's own drawing, keys and clicks then work on the run's
/// conversation, laid where its pane is. The session's own are put back
/// after, as they were. None when the run's conversation isn't the one
/// read.
pub fn as_run<T>(m: &mut Model, key: &str, id: &str, f: impl FnOnce(&mut Model) -> T) -> Option<T> {
    if m.session.as_ref().is_none_or(|v| v.key != key) {
        return None;
    }
    let mut d = conn(m, key).runs.detail.take()?;
    if d.id != id {
        conn(m, key).runs.detail = Some(d);
        return None;
    }
    let c = conn(m, key);
    let p = &mut c.watch.peek;
    let own_txt = std::mem::replace(&mut c.txt, p.txt);
    let own_rows = std::mem::replace(&mut c.vs.rows, p.rows.max(1));
    let own_hover = std::mem::replace(&mut c.vs.hover_ref, std::mem::take(&mut p.hover));
    (d.view.top, d.view.top_off) = (p.y, 0);
    if let Some(v) = m.session.as_mut() {
        std::mem::swap(v, &mut d.view);
    }
    core(m).as_run = Some(key.to_string());
    let out = f(m);
    core(m).as_run = None;
    let back = m.session.as_mut().filter(|v| v.key == key);
    let put = back.is_some();
    if let Some(v) = back {
        std::mem::swap(v, &mut d.view);
    }
    let c = conn(m, key);
    c.watch.peek.txt = std::mem::replace(&mut c.txt, own_txt);
    c.vs.rows = own_rows;
    c.watch.peek.hover = std::mem::replace(&mut c.vs.hover_ref, own_hover);
    if put {
        c.runs.detail = Some(d);
    } else {
        // f opened another session (a link to an agent): the pane is
        // done with.
        c.watch.msg_focus = false;
    }
    Some(out)
}

/// The run's conversation, rows tall and w wide, while its view stands in
/// the session's (as_run): as the Session draws its own (rows_of),
/// without what's the session's alone beside and under it.
pub fn text(
    m: &mut Model,
    key: &str,
    w: usize,
    rows: usize,
    live: bool,
    focused: bool,
) -> Vec<Line> {
    let clock = (m.now, m.tick);
    let pref = m.cfg.column_width;
    let txt = conn(m, key).txt;
    let hover = conn(m, key).vs.hover_ref.clone();
    let Some(v) = m.session.as_mut() else {
        return Vec::new();
    };
    prime(v, w, clock, live, focused);
    let mut out = if !v.read {
        v.note_drawn();
        vec![line([dim(" reading its conversation…")])]
    } else if v.sess.turns.is_empty() {
        let first = if v.err.is_empty() {
            line([dim(" nothing in it yet")])
        } else {
            line([sp(format!(" couldn't read it: {}", v.err), RED)])
        };
        v.shown = vec![String::new()];
        vec![first]
    } else {
        super::conversation::rows_of(v, rows, w, &hover, None, &txt, pref).0
    };
    out.truncate(rows);
    out
}

fn run_sel(m: &Model, key: &str, id: &str) -> String {
    super::subview::runs_of(m, key)
        .and_then(|r| r.detail.as_ref())
        .filter(|d| d.id == id)
        .map(|d| d.view.sel.clone())
        .unwrap_or_default()
}

fn set_run_sel(m: &mut Model, key: &str, sel: String) {
    if let Some(d) = conn(m, key).runs.detail.as_mut() {
        d.view.sel = sel;
    }
}

/// Whether run id's box has a pick in its conversation: its row keys then
/// take enter, → and ←.
pub fn row_picked(m: &Model, key: &str, id: &str) -> bool {
    !run_sel(m, key, id).is_empty()
}

/// Key s for the run's conversation: the Session's own
/// (Model::session_key_at) on the run's view, at its pane's size.
fn conv_key(m: &mut Model, key: &str, id: &str, s: &str) -> Out {
    let (w, rows) = {
        let p = &conn(m, key).watch.peek;
        (p.w, p.rows.max(1))
    };
    as_run(m, key, id, |m| Out::from(m.session_key_at(s, w, rows))).unwrap_or(Out::Idle)
}

/// What /compact typed in a run's box does: a run rush hosts is a session
/// of its own and takes it as its session does (it goes on to it); a
/// subagent of another's session can't be compacted from outside it, and
/// the box says so (Some, the key used).
fn compact(m: &mut Model, key: &str, id: &str) -> Option<Out> {
    let t = super::subbox::draft(m, key, id).input.string();
    let t = t.trim();
    let named = |c: &str| t == c || t.starts_with(&format!("{c} "));
    if !named("/compact") && !named("#compact") {
        return None;
    }
    if conn(m, key).runs.hosted_of(id).is_some() {
        if t.starts_with('#') {
            // Its own session's #compact sheet is its own: /compact goes.
            let d = super::subbox::draft(m, key, id);
            let rest = t.trim_start_matches("#compact").to_string();
            d.input.set(&format!("/compact{rest}"));
        }
        return None;
    }
    let home = super::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let kind = super::subview::runs_of(m, key)
        .map(|r| super::subview::kind_of(r, &home, id))
        .unwrap_or(home);
    if m.reg.reads_as_claude(&kind) {
        m.flash_err(COMPACT_CLAUDE);
    } else {
        let name = m.reg.label(&kind);
        m.flash_err(&format!(
            "{name} can't compact a subagent from outside its session"
        ));
    }
    Some(Out::Draw)
}

/// What a /compact typed for a Claude Code subagent says: Claude Code has
/// no way to compact one (no control request for it; SendMessage carries
/// only words), and compacts each as it fills.
pub const COMPACT_CLAUDE: &str =
    "Claude Code can't compact a subagent; it compacts on its own as it fills";

/// The keys while the run's pane has them: every one is the pane's but
/// the few the whole of rush answers (ctrl+n, ctrl+c with nothing typed
/// or dragged over). Its conversation's rows take ↑↓ from an empty box
/// (its queue's last first, when anything waits), and enter → ← space on
/// a row picked, as a session's do; typing goes to the run's box
/// (subbox::run_key), never the session's, and returns to it from a row.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if !conn(m, &key).watch.msg_focus || super::views::view_name(m, &key) != "subagents" {
        return Out::Pass;
    }
    let Some(id) = super::subview::box_run(m, &key) else {
        conn(m, &key).watch.msg_focus = false;
        return Out::Pass;
    };
    let s = k.s.as_str();
    let routed = super::relay::route(m, &key, &id);
    // A run gone (its host no longer lists it, nothing to read): the
    // keys go back to the list, what was typed with them too.
    if routed.is_err() && super::subview::src_of(m, &key, &id).is_none() {
        conn(m, &key).watch.msg_focus = false;
        return if k.text().is_some() {
            Out::Draw
        } else {
            Out::Pass
        };
    }
    let empty = super::subbox::empty(m, &key, &id);
    let d = super::subbox::draft(m, &key, &id);
    let (queue_pick, editing) = (d.pick.is_some(), d.edit_q > 0);
    if queue_pick && routed.is_ok() {
        return super::subbox::run_key(m, &key, &id, k);
    }
    let picked = row_picked(m, &key, &id);
    let txt_on = conn(m, &key).watch.peek.txt.on;
    let queued = !super::subbox::queue_of(m, &key, &id).items.is_empty();
    match s {
        "esc" if txt_on && empty => {
            conn(m, &key).watch.peek.txt = Default::default();
            Out::Draw
        }
        "esc" if picked => {
            set_run_sel(m, &key, String::new());
            Out::Draw
        }
        "esc" if editing => super::subbox::run_key(m, &key, &id, k),
        // Picked from elsewhere: esc goes straight back there.
        "esc" if empty && conn(m, &key).watch.back => {
            super::subview::go_back(m, &key);
            Out::Draw
        }
        // esc and tab give the keys back to the list; what's typed stays.
        "esc" | "tab" => {
            conn(m, &key).watch.msg_focus = false;
            Out::Draw
        }
        "left" if empty && !picked => {
            conn(m, &key).watch.msg_focus = false;
            Out::Draw
        }
        "left" if empty => {
            // ← goes back a stage: what's open round the row, then off it.
            let rows = conn(m, &key).watch.peek.rows.max(1);
            let closed = as_run(m, &key, &id, |m| {
                m.session.as_mut().is_some_and(|v| v.fold_in(rows))
            });
            if closed != Some(true) {
                set_run_sel(m, &key, String::new());
            }
            Out::Draw
        }
        "up" if empty && !picked && queued && routed.is_ok() => {
            super::subbox::run_key(m, &key, &id, k)
        }
        "up" | "down" if empty || picked => conv_key(m, &key, &id, s),
        "enter" | "space" | " " | "right" if picked && empty => conv_key(m, &key, &id, s),
        "pgup" | "pgdown" | "ctrl+o" => conv_key(m, &key, &id, s),
        "home" | "end" if empty => conv_key(m, &key, &id, s),
        "o" if empty && picked => {
            let u = as_run(m, &key, &id, |m| super::conversation::picked_link(m)).flatten();
            match u {
                Some(u) if rush_ui_cmdbar::linkmenu::open_link_menu(m, &u) => Out::Draw,
                _ => Out::Idle,
            }
        }
        "ctrl+c" | "super+c" if txt_on && (empty || s == "super+c") => {
            let text = as_run(m, &key, &id, |m| {
                let t = conn(m, &key).txt;
                super::textsel::text_of(m, &t)
            })
            .unwrap_or_default();
            if !text.is_empty() {
                m.copy_text(text);
            }
            Out::Draw
        }
        "ctrl+c" if !empty => {
            super::subbox::draft(m, &key, &id).input.clear();
            Out::Draw
        }
        "ctrl+c" | "ctrl+n" => Out::Pass,
        "?" | "f1" if empty => {
            m.act(crate::frame::ext::Act::OpenKeyHelp);
            Out::Draw
        }
        "f1" => {
            m.act(crate::frame::ext::Act::OpenKeyHelp);
            Out::Draw
        }
        "enter" | "ctrl+enter" if !editing && let Some(o) = compact(m, &key, &id) => o,
        _ => {
            if let Err(e) = &routed {
                // Nothing can reach it: what's typed doesn't go anywhere
                // else either.
                if k.text().is_some() || s == "enter" || s == "ctrl+enter" {
                    m.flash(e);
                    return Out::Draw;
                }
                return Out::Idle;
            }
            // Typing returns to the box, as in a session.
            if k.text().is_some() && picked {
                set_run_sel(m, &key, String::new());
            }
            let o = super::subbox::run_key(m, &key, &id, k);
            if o.used() { o } else { Out::Idle }
        }
    }
}

/// The keys the guide lists while the run's pane has them.
pub fn here_keys(m: &mut Model) -> Option<(&'static str, Vec<(&'static str, &'static str)>)> {
    let key = open_key(m)?;
    if !conn(m, &key).watch.msg_focus || super::views::view_name(m, &key) != "subagents" {
        return None;
    }
    super::subview::box_run(m, &key)?;
    Some((
        "Subagent's pane",
        vec![
            ("↑ ↓", "pick its rows (↑ from the box: its queue first)"),
            ("enter · →", "open the row picked, ← close it"),
            ("ctrl+o", "show all of it"),
            ("o", "open the picked step's file or diff"),
            ("pgup · pgdn", "scroll its conversation"),
            ("home · end", "its start, its end (end follows it)"),
            ("enter", "send what's typed to the run"),
            ("ctrl+enter", "send it now"),
            (
                "↑ then space · [ ] · ⌫ · s · h",
                "edit, move, drop, send now or hold its queue",
            ),
            ("/compact", "compact a run rush hosts"),
            ("alt+o", "a run rush hosts: its own session"),
            ("esc · tab", "back to the list"),
        ],
    ))
}

/// The pointer over the run's conversation in its pane (or dragging text
/// in it): what the session's own conversation does (a link, a row picked
/// or opened, text dragged over, the wheel, the row under it lit), and a
/// click gives the pane the keys. Elsewhere it's the subagents view's.
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if super::views::view_name(m, &key) != "subagents" {
        return Out::Pass;
    }
    let (x, y) = (e.x as usize, e.y as usize);
    let p = &conn(m, &key).watch.peek;
    if !p.visible || p.w == 0 {
        return Out::Pass;
    }
    let inside = x >= p.x && x < p.x + p.w && y >= p.y && y < p.y + p.h;
    if !inside && !p.txt.drag {
        return Out::Pass;
    }
    let (id, px, py, rows) = (p.id.clone(), p.x, p.y, p.rows.max(1));
    let mut used = Out::Idle;
    if e.kind == MouseKind::Down(Button::Left) && inside {
        let w = &mut conn(m, &key).watch;
        if !w.msg_focus || w.msg_for != id {
            w.msg_focus = true;
            w.msg_for.clone_from(&id);
            used = Out::Draw;
        }
    }
    // The conversation's own code reads its text from the screen's third
    // cell: the pane's cells, moved there.
    let at = Mouse {
        x: (x + 2).saturating_sub(px) as u16,
        ..*e
    };
    let o = as_run(m, &key, &id, |m| match e.kind {
        MouseKind::ScrollUp | MouseKind::ScrollDown => {
            let up = e.kind == MouseKind::ScrollUp;
            let Some(v) = m.session.as_mut() else {
                return Out::Idle;
            };
            if up && v.start == 0 || !up && v.pos == Pos::End {
                return Out::Idle; // a trackpad's momentum at either end
            }
            v.scroll(if up { 3 } else { -3 }, rows);
            Out::Draw
        }
        MouseKind::Moved => {
            let item = if inside {
                super::conversation::owner_at(m, y - py)
            } else {
                String::new()
            };
            let c = conn(m, &key);
            if c.vs.hover_ref == item {
                return Out::Idle;
            }
            c.vs.hover_ref = item;
            Out::Draw
        }
        _ => {
            let o = super::conversation::mouse(m, &at);
            if o.used() {
                return o;
            }
            super::textsel::mouse(m, &at)
        }
    })
    .unwrap_or(Out::Idle);
    if o.used() { o } else { used }
}

/// A child rush hosts, open as a session of its own (spawned::
/// open_session): the way back to the session that started it, over its
/// header's first row. Its parent's key and the run's id there, when it
/// has one open in rush's list.
pub fn parent_of(m: &Model) -> Option<(String, String, String)> {
    let v = m.session.as_ref()?;
    let by = &v.sess.info.spawned_by;
    if by.is_empty() {
        return None;
    }
    let me = m.agents.iter().find(|a| a.key == v.key)?;
    let p = m.agents.iter().find(|a| a.rush && a.job.id == *by)?;
    let name = one_line(&p.display_name);
    Some((p.key.clone(), name, me.job.id.clone()))
}

/// The breadcrumb over a hosted child's own session: `‹ parent › its
/// name`, the parent clickable (crumb_click).
pub fn child_crumb(m: &Model, rows: &mut [Line], w: usize) -> Option<(usize, usize)> {
    let (_, parent, _) = parent_of(m)?;
    let v = m.session.as_ref()?;
    let me = m.agents.iter().find(|a| a.key == v.key)?;
    let hw = w.saturating_sub(3);
    let lead = line([plain("  "), sp("‹ ", SUB), sp(parent, SUB)]);
    let crumb = (2 + 2, 2 + lead.width());
    let mut l = lead;
    l.spans.push(sp(" › ", FAINT));
    l.spans.push(dim(one_line(&me.display_name)));
    l.spans.push(faint("   · subagents"));
    let first = rows.first_mut()?;
    *first = super::views::chrome(rush_draw::truncate_line(&l, hw, "…"), hw);
    Some(crumb)
}

/// A click on a hosted child's breadcrumb: its parent's session, on its
/// Subagents list with the child picked.
pub fn crumb_click(m: &mut Model, e: &Mouse) -> Out {
    if e.kind != MouseKind::Down(Button::Left) || m.confirm.is_some() || m.sheet.is_some() {
        return Out::Pass;
    }
    let Some((top, off)) = m.session.as_ref().map(|v| (v.top, v.top_off)) else {
        return Out::Pass;
    };
    if top.checked_sub(HEAD_H + off) != Some(e.y as usize) {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let (from, to) = conn(m, &key).watch.child_crumb;
    let x = e.x as usize;
    if to == 0 || x < from || x >= to {
        return Out::Pass;
    }
    let Some((parent, _, child)) = parent_of(m) else {
        return Out::Pass;
    };
    super::menubar_jump::jump_to(m, parent.clone());
    if m.session.as_ref().is_some_and(|v| v.key == parent) {
        let run = conn(m, &parent)
            .runs
            .spawns
            .iter()
            .find(|(_, s)| s.hosted == child)
            .map(|(step, _)| format!("{}{step}", crate::spawns::SPAWN_PREFIX));
        match run {
            Some(id) => super::subview::pick_run(m, &parent, &id),
            None => {
                super::views::show_view(m, &parent, "subagents");
            }
        }
    }
    Out::Draw
}
