//! The session's views beside its conversation: how much room they take,
//! the panel laid right of the text, and what a click, the pointer and
//! the wheel do on it, buttons' sends, links, copies and row changes
//! included. After Go's internal/ui/rushmode.go (viewsBeside :674,
//! viewsFolded :693, rushPane's panel :2361-2362 and :2725-2737, clickRef's
//! side views :5054), agentviews.go (viewsAt :853, clickViews :862,
//! refreshViews :966, wheelViews :975, hoverViews :989; its runs' rows
//! :908-916), viewacts.go
//! (viewAction :222), panelsubs.go (subsInPanel :31) and asksheets.go
//! (sendAs :51).
//!
//! What a click asks of the host (put a view away, bring one back, change
//! a row) runs off the UI thread; the view's follower (read.rs) sees the
//! folder change, and the panel is read again at once besides.

use rush_draw::{Button, MouseKind};

use super::model::{Action, View};
use super::panel::{Click, VIEWS_STRIP, ViewsState, pane_views_side, views_width_with};
use super::read::{self, id_of, past_views_of, views_of};
use crate::frame::ext::Act;
use crate::frame::{Mouse, Out};
use crate::model::Model;
use crate::panel_host;
use crate::sess::open_key;
use crate::sidecol::{side_width_with, text_width};

/// Whether what's running shows at the top of the session's panel: it's
/// open on one of the agent's views, and there are tasks to show; the
/// dock leaves them out then (subsInPanel). width is the panel's.
pub fn subs_in_panel(any: bool, st: &ViewsState, side: usize, width: usize) -> bool {
    if width <= VIEWS_STRIP || side == 0 || st.shut || st.past {
        return false;
    }
    any
}

/// The cells between the conversation's text and the column, or the panel
/// standing in its place, and past it on the right (the column's x).
const VIEWS_GAP: usize = 2;

/// The session's views and past views shown beside a conversation w
/// wide, and how much of w they take: 0 when there are none or no room,
/// a slim strip when they're folded or only past (viewsBeside). It
/// follows the session's views folder while it's open.
pub fn views_beside(m: &mut Model, key: &str, w: usize) -> (Vec<View>, Vec<View>, usize) {
    let id = crate::sess::agent(m, key)
        .map(id_of)
        .unwrap_or("")
        .to_string();
    read::follow(m, &id);
    if (m.ext.calls.view_name())(m, key) != "conversation" || id.is_empty() {
        return (Vec::new(), Vec::new(), 0);
    }
    let (side, past) = (views_of(m, &id), past_views_of(m, &id));
    if side.is_empty() && past.is_empty() {
        return (side, past, 0);
    }
    let pref = m.cfg.column_width;
    let mut width = views_width_with(w, pref);
    if width > VIEWS_STRIP && side_width_with(w, pref) > 0 {
        width += VIEWS_GAP; // the room the column leaves right of its text
    }
    if width > 0 && read::state(m, key).folded(&side) {
        width = VIEWS_STRIP;
        if side_width_with(w, pref) > 0 {
            width = 0; // a part of the column beside the conversation instead
        }
    }
    (side, past, width)
}

/// Lays the views w wide right of the conversation's rows, out (cw
/// wide), rows tall from screen row top: rush's own rows at the panel's
/// top while it's open, the view greyed when the agent has stopped
/// keeping it (rushPane's panel). w 0 leaves the panel where no click
/// finds it.
#[allow(clippy::too_many_arguments)]
pub fn paste(
    m: &mut Model,
    key: &str,
    out: &mut Vec<rush_core::text::Line>,
    rows: usize,
    cw: usize,
    w: usize,
    vs: &[View],
    past: &[View],
    top: usize,
) {
    let now = m.now;
    // Beside a wide conversation the panel stands where the column does,
    // as wide, the same gap left of it: the column's two spare cells go
    // blank on its right.
    let w = if w > VIEWS_STRIP && side_width_with(cw + w, m.cfg.column_width) > 0 {
        w - VIEWS_GAP
    } else {
        w
    };
    let mut st = std::mem::take(read::state(m, key));
    st.w = w;
    st.live.clear();
    if w > 0 {
        if let Some(v) = m.session.as_ref().filter(|v| v.key == key) {
            // What runs shows at the panel's top when the dock leaves it
            // out: session_core draws those rows (panel_host().panel_subs).
            if let Some(rows) = (panel_host().panel_subs)(m, key, &st, vs.len(), w) {
                st.live = rows;
            }
            st.quiet = String::new();
            if !vs.is_empty() {
                let shown = super::panel::shown_view(vs, &st.tab).clone();
                st.quiet = st.quiet_why(Some(&v.sess), &shown, now);
            }
        }
        while out.len() < rows {
            out.push(rush_core::text::Line::new());
        }
        st.x = 2 + cw; // the pane starts two cells in
        st.y = top;
        pane_views_side(out, 0, rows, cw, w, vs, past, now, &mut st);
    }
    *read::state(m, key) = st;
}

/// Whether the open session's views are drawn where the pointer could
/// find them, and its key.
fn open_panel(m: &mut Model) -> Option<String> {
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return None;
    }
    let key = open_key(m)?;
    if (m.ext.calls.view_name())(m, &key) != "conversation" {
        return None;
    }
    read::state_of(m, &key).filter(|st| st.w > 0)?;
    Some(key)
}

/// A click, the wheel or the pointer over the open session's views
/// (clickViews, wheelViews, hoverViews). Off them it passes.
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    let Some(key) = open_panel(m) else {
        return Out::Pass;
    };
    let st = read::state(m, &key);
    let (col, row) = (e.x as i64 - st.x as i64, e.y as i64 - st.y as i64);
    match e.kind {
        MouseKind::Moved => {
            let changed = st.hover_at(col, row);
            if !st.at(col, row) {
                // Off the panel: the rest of the pane has the pointer.
                return if changed { Out::Draw } else { Out::Pass };
            }
            // Nothing under it in the conversation is lit meanwhile.
            let lit = (m.ext.calls.clear_hover())(m, &key);
            if changed || lit { Out::Draw } else { Out::Idle }
        }
        MouseKind::ScrollUp | MouseKind::ScrollDown => {
            if !st.at(col, row) {
                return Out::Pass;
            }
            let was = st.scroll;
            if !st.wheel(col, row, e.kind == MouseKind::ScrollUp) {
                return Out::Pass;
            }
            if st.scroll == was {
                Out::Idle
            } else {
                Out::Draw
            }
        }
        MouseKind::Down(Button::Left) => {
            let c = st.click(col, row);
            on_click(m, &key, c)
        }
        _ => {
            if st.at(col, row) {
                Out::Idle
            } else {
                Out::Pass
            }
        }
    }
}

/// Does what a click on the panel asked of the session (clickViews' host
/// calls, viewAction, the runs at its top).
pub fn on_click(m: &mut Model, key: &str, c: Click) -> Out {
    let id = crate::sess::agent(m, key)
        .map(id_of)
        .unwrap_or("")
        .to_string();
    match c {
        Click::Missed => Out::Pass,
        Click::Drew => Out::Draw,
        Click::Drop(name) => {
            if !id.is_empty() {
                host_then(m, &id, "", move |id| rush_host::views::drop_view(id, &name));
            }
            Out::Draw
        }
        Click::BringBack { file, name } => {
            if id.is_empty() {
                return Out::Draw;
            }
            let k = key.to_string();
            let read_id = id.clone();
            m.later(
                move || rush_host::views::bring_back(&read_id, &file),
                move |m, r| {
                    match r {
                        Err(e) => m.fail(&format!("couldn't bring it back: {e}")),
                        Ok(()) => {
                            read::state(m, &k).brought_back(&name);
                            read::refresh(m, &id);
                        }
                    }
                    Out::Draw
                },
            );
            Out::Draw
        }
        Click::Act { key: row, a } => {
            let shown = read::state_of(m, key)
                .map(|st| st.shown.clone())
                .unwrap_or_default();
            view_action(m, key, &shown, &row, a)
        }
        Click::Open(link) => {
            open_link(m, &link);
            Out::Draw
        }
        Click::Run(r) => {
            // A workflow shows its agents, a run is picked in the
            // subagents view, shown beside its list rather than across the
            // whole view, a task is picked: its keys show under it
            // (agentviews.go:908-916; the user asked for the pick).
            m.act(Act::OpenRun {
                key: key.to_string(),
                r,
            });
            Out::Draw
        }
    }
}

/// Asks the host f off the UI thread, then reads the session's views
/// again; a failure says so, after what (refreshViews' callers).
fn host_then<T: Send + 'static>(
    m: &mut Model,
    id: &str,
    what: &'static str,
    f: impl FnOnce(&str) -> Result<T, String> + Send + 'static,
) {
    let (id, run_id) = (id.to_string(), id.to_string());
    m.later(
        move || f(&run_id),
        move |m, r| {
            match r {
                Err(e) if !what.is_empty() => m.fail(&format!("{what}{e}")),
                Err(_) => {}
                Ok(_) => read::refresh(m, &id),
            }
            Out::Draw
        },
    );
}

/// Does what a button says, on the row of key in the view of name:
/// changes the row, opens its link, copies its words, sends the agent its
/// message (viewAction).
pub fn view_action(m: &mut Model, sess: &str, name: &str, key: &str, a: Action) -> Out {
    let Some(id) = crate::sess::agent(m, sess)
        .map(id_of)
        .filter(|id| !id.is_empty())
    else {
        return Out::Idle;
    };
    let id = id.to_string();
    if let Some(set) = a.set.clone()
        && !key.is_empty()
    {
        let (name, key) = (name.to_string(), key.to_string());
        host_then(m, &id, "couldn't change it: ", move |id| {
            rush_host::viewacts::change_row(id, &name, &key, set)
        });
    }
    if !a.open.is_empty() {
        open_link(m, &a.open);
    }
    if !a.copy.is_empty() {
        m.pending_copy = Some(a.copy.clone());
        m.flash(&format!("copied {}", super::trunc(&a.copy, 40)));
    }
    if !a.send.is_empty() {
        send_as(m, sess, &a.send);
    }
    Out::Draw
}

/// Opens a link in the browser, off the UI thread, and says so.
fn open_link(m: &mut Model, link: &str) {
    let l = link.to_string();
    m.later(
        move || {
            let _ = std::process::Command::new("open").arg(&l).output();
        },
        |_, _| Out::Idle,
    );
    m.flash(&format!(
        "opened {}",
        link.strip_prefix("https://").unwrap_or(link)
    ));
}

/// Sends the agent text as if typed, in a session rush runs (sendAs).
fn send_as(m: &mut Model, key: &str, text: &str) {
    if !crate::sess::conn(m, key).hosted() {
        m.flash_err("that works in rush-mode sessions · /rush moves this one over");
        return;
    }
    m.act(Act::SendAs {
        key: key.to_string(),
        text: text.to_string(),
    });
}

/// A click on the views folded into the column: opens them, on a view's
/// row at that view (clickRef's side views). rest is what follows the
/// ref's prefix.
pub fn open_from_side(m: &mut Model, key: &str, rest: &str) {
    let id = crate::sess::agent(m, key)
        .map(id_of)
        .unwrap_or("")
        .to_string();
    let none = views_of(m, &id).is_empty();
    let st = read::state(m, key);
    st.shut = false;
    st.past = none;
    st.scroll = 0;
    if let Some(name) = rest.strip_prefix(':') {
        st.tab = name.to_string();
    }
}

/// The session's views folded into the column beside the conversation,
/// sw wide, when that's where they go: the column's last part, edged in
/// the newest view's colour (paneDock's folded views). None when they
/// show as a panel or a strip, or there are none.
pub fn folded_in_side(
    m: &mut Model,
    key: &str,
    w: usize,
    sw: usize,
) -> Option<(
    rush_core::text::Color,
    Vec<rush_core::text::Line>,
    Vec<String>,
)> {
    let (side, past, vw) = views_beside(m, key, w);
    if sw == 0 || vw != 0 || side.is_empty() && past.is_empty() {
        return None;
    }
    if !read::state(m, key).folded(&side) {
        return None;
    }
    let edge = side.first().map_or(crate::style::SUB, super::view_accent);
    let (rows, refs) = super::panel::side_views(&side, &past, text_width(sw));
    Some((edge, rows, refs))
}

/// After each reading of the fleet: the list's summaries read again where
/// they changed, and the open session's views followed, none once it's
/// closed.
pub fn on_fleet(m: &mut Model) {
    if m.session.is_none() {
        read::follow(m, "");
    }
    read::on_fleet(m);
}

/// Whether what's running shows at the top of the open session's panel
/// rather than in its dock, a pane w wide (subsInPanel).
pub fn subs_shown_in_panel(m: &mut Model, key: &str, w: usize) -> bool {
    let (side, _, width) = views_beside(m, key, w);
    let st = read::state(m, key).clone();
    let any = (m.ext.calls.running_any())(m, key);
    m.session
        .as_ref()
        .filter(|v| v.key == key)
        .is_some_and(|_| subs_in_panel(any, &st, side.len(), width))
}
