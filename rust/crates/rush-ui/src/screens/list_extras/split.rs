//! Agents and the Session side by side: on a wide screen the list keeps
//! its share on the left and the picked agent's Session shows beside it,
//! following the pick; shift+← → and #view move between the split, the
//! list alone and the Session alone. After Go's internal/ui/view.go
//! (widths :881, sideWidth :917, maxPane :933, stepSplit :951, chatOpen
//! :981, listOnly :991, splitAgain :1000, canSplit :1017, splitHint
//! :1021, minPane :1078, listView :1098's rows side by side, divider
//! :1469), model.go (wide :1920, autoSplit :1924), panegeometry.go:13
//! (paneFrameRow) and keys.go:1068 (#view). The Session beside the list is
//! read as an opened one is (session.rs), into a view of its own
//! (`Model::pane`), so the list keeps the keys until enter opens it.

use rush_core::text::Line;

use crate::frame::{Out, Press};
use crate::model::Model;
use crate::session;
use crate::style::*;

/// The narrowest pane worth splitting the screen for (minPane).
pub const MIN_PANE: usize = 84;
/// The widest a Session gets: its rows cap at 124 columns, plus the
/// pane's margins (maxPane).
pub const MAX_PANE: usize = 128;

/// A screen wide enough to split without being asked (wide).
pub fn wide(w: usize) -> bool {
    w >= 170
}

/// Whether the Session shows beside the list without being asked: a wide
/// screen you haven't pushed to the list alone (autoSplit).
pub fn auto_split(m: &Model) -> bool {
    wide(m.w) && !m.cfg.list_only
}

/// Whether the screen is wide enough for Agents and the Session side by
/// side (canSplit).
pub fn can_split(w: usize) -> bool {
    w.saturating_sub(floor(w) + 1) >= MIN_PANE
}

/// The list's least share: a quarter of the screen, 30 columns at least.
fn floor(w: usize) -> usize {
    w.div_ceil(4).max(30)
}

/// The list's width in a split: your own share when you've set one,
/// else rush's; never under a quarter of the screen or 30 columns
/// (sideWidth).
pub fn side_width(m: &Model) -> usize {
    let w = m.w;
    let floor = floor(w);
    let side = floor.max((w * 28 / 100).min(64));
    let f = m.cfg.side_width;
    if f > 0.0 {
        // A width you dragged to wins.
        let want = (w as f64 * f + 0.5) as usize;
        return floor.max(want.min(w * 3 / 4));
    }
    // The Session never takes more than its content can use; the spare
    // room goes to Agents.
    side.max(w.saturating_sub(1 + MAX_PANE))
}

/// How the screen splits between the list and the Session beside it:
/// (list, pane), the pane 0 with the list alone (widths). An opened
/// Session has the whole screen.
pub fn widths(m: &Model) -> (usize, usize) {
    let w = m.w;
    if m.session.is_some() || !auto_split(m) {
        return (w, 0);
    }
    let side = side_width(m);
    if w.saturating_sub(side + 1) >= MIN_PANE {
        (side, w - side - 1)
    } else {
        (w, 0)
    }
}

/// The key back to the split, for the row of keys, while one side has
/// the screen and there's room for both (splitHint).
pub fn hint(m: &Model) -> Option<(String, String)> {
    if !can_split(m.w) {
        return None;
    }
    let (l, p) = widths(m);
    if l > 0 && p > 0 {
        return None;
    }
    let back = if m.session.is_some() {
        "shift+→"
    } else {
        "shift+←"
    };
    Some((
        format!("{back} · #view split"),
        "back to side by side".into(),
    ))
}

/// Keeps the layout: in the list's config at once, on disk off the UI
/// thread (Config.SetView, SaveConfig).
fn keep_view(m: &mut Model, v: &'static str) {
    m.cfg.set_view(v);
    m.later(
        move || {
            let mut s = rush_state::Store::load();
            s.config.set_view(v);
            s.save_config().map_err(|e| e.to_string())
        },
        |m, r: Result<(), String>| {
            if let Err(e) = r {
                m.fail(&format!("couldn't keep the layout: {e}"));
            }
            Out::Draw
        },
    );
}

/// Hides the Session and gives the screen to Agents, next time too
/// (listOnly).
pub fn list_only(m: &mut Model) {
    keep_view(m, "list");
    m.pane = None;
    m.flash("just Agents, next time too · shift+← or #view split puts the Session beside them");
}

/// Agents and the Session side by side, when there's room, next time too
/// (splitAgain).
pub fn split_again(m: &mut Model) {
    if !can_split(m.w) {
        m.flash_err("too narrow for Agents and the Session side by side");
        return;
    }
    keep_view(m, "split");
    m.session = None;
    m.flash("Agents and the Session side by side, next time too");
}

/// The screen to the picked agent's Session, next time too
/// (sessionOnly).
pub fn session_only(m: &mut Model) {
    let Some(i) = m.selected() else { return };
    keep_view(m, "agent");
    m.open(i);
    m.flash("just the Session, next time too · esc for Agents · shift+→ or #view split for both side by side");
}

/// #view split|agent|list (keys.go:1068).
pub fn command(m: &mut Model, arg: &str) -> Out {
    match arg.trim() {
        "split" => split_again(m),
        "agent" | "chat" | "session" => session_only(m),
        "list" | "agents" | "orchestrator" => list_only(m),
        _ => m.flash_err("view one of: split, agent, list"),
    }
    Out::Draw
}

/// shift+← and shift+→ in the list (alt too): from the split, ← gives the
/// Session the screen and → the list; from the list alone, ← brings the
/// Session back beside it (stepSplit, without the edge's steps).
pub fn key(m: &mut Model, k: &Press) -> Out {
    let grow = match k.s.as_str() {
        "shift+right" | "alt+right" => true,
        "shift+left" | "alt+left" => false,
        _ => return Out::Pass,
    };
    if !can_split(m.w) {
        return Out::Pass;
    }
    let (_, pane) = widths(m);
    match (pane > 0, grow) {
        (true, true) => list_only(m),
        (true, false) => session_only(m),
        (false, false) => split_again(m),
        (false, true) => return Out::Pass,
    }
    Out::Draw
}

/// The agent last found with no conversation to read, so it isn't tried
/// again every frame.
#[derive(Default)]
struct Unreadable(String);

/// Keeps the Session beside the list on the agent picked: read again
/// only when the pick moves to another (loadPreview).
pub fn follow(m: &mut Model) {
    let Some(i) = m.focused() else {
        m.pane = None;
        return;
    };
    let key = m.agents[i].key.clone();
    if m.pane.as_ref().is_some_and(|v| v.key == key) {
        return;
    }
    if m.pane.is_none() && m.kept::<Unreadable>().0 == key {
        return;
    }
    m.pane = m.read_view(i, true);
    m.kept::<Unreadable>().0 = if m.pane.is_none() { key } else { String::new() };
    // A live session rush hosts is its host's: dialled now (syncHost).
    crate::screens::session_core::live::sync(m);
}

/// n cells of the header's chrome.
fn chrome(n: usize) -> rush_core::text::Span {
    let st = rush_core::text::Style {
        bg: CHROME_BG,
        ..Default::default()
    };
    rush_core::text::Span::new(blanks(n), st)
}

/// The pane of a session rush hosts while its host isn't reached yet:
/// its frame's chrome round a line saying so (rushPane's placeholder in
/// paneFrameRow's gutters); None once it's connected or read.
fn connecting(m: &mut Model, v: &session::SessionView, w: usize, rows: usize) -> Option<Vec<Line>> {
    if !matches!(v.source, session::Source::Hosted) || !v.sess.turns.is_empty() {
        return None;
    }
    let a = m.agents.iter().find(|a| a.key == v.key)?;
    let name = one_line(&a.display_name);
    let say = if a.pid == 0 {
        format!("  {name} isn't running, and has no conversation to show yet")
    } else {
        format!("  connecting to {name}…")
    };
    let c = crate::screens::session_core::conn(m, &v.key);
    if c.link.is_some() {
        return None;
    }
    let inner = w.saturating_sub(3);
    let body = [Line::new(), line([sp(say, DIM)])];
    let mut out = Vec::with_capacity(rows);
    for i in 0..rows {
        let mid = fit(body.get(i).cloned().unwrap_or_default(), inner);
        let mut r = match i {
            0..4 => line([chrome(2)]),
            4 => line([sp("──", FAINT)]),
            _ => line([plain("  ")]),
        };
        join(&mut r, mid);
        r.spans.push(match i {
            0..4 => chrome(1),
            4 => sp("─", FAINT),
            _ => plain(" "),
        });
        out.push(r);
    }
    Some(out)
}

/// The Session beside the list, w wide (the divider's column not in it)
/// and rows tall: its header on the chrome, then its conversation, each
/// row in the pane's margins (paneFrameRow).
pub fn pane_lines(m: &mut Model, w: usize, rows: usize) -> Vec<Line> {
    let (now, tick) = (m.now, m.tick);
    let Some(mut v) = m.pane.take() else {
        let say = match m.focused() {
            Some(i) => format!(
                "  {} has no conversation to show yet",
                one_line(&m.agents[i].display_name)
            ),
            None => "  select an agent to see what it is doing".into(),
        };
        return vec![Line::new(), line([sp(say, DIM)])];
    };
    if let Some(out) = connecting(m, &v, w, rows) {
        m.pane = Some(v);
        return out;
    }
    let a = m.agents.iter().find(|a| a.key == v.key);
    let mut out = session::header(a, &v, w, now, &m.reg);
    let body_rows = rows.saturating_sub(out.len());
    v.sync(w.saturating_sub(3), now, tick);
    let body = if !v.read {
        vec![line([sp("reading the conversation…", DIM)])]
    } else if !v.err.is_empty() && v.sess.turns.is_empty() {
        vec![line([sp(format!("couldn't read it: {}", v.err), RED)])]
    } else {
        v.body(body_rows)
    };
    for l in body.into_iter().take(body_rows) {
        let mut r = line([plain("  ")]);
        join(&mut r, l);
        out.push(fit(r, w));
    }
    m.pane = Some(v);
    out
}

/// The edge between Agents and the Session: a quiet line (divider).
pub fn divider() -> Line {
    line([sp("│", FAINT)])
}

/// The list's rows and the pane's side by side, each fit to its width.
pub fn join_rows(left: &[Line], lw: usize, pane: &[Line], pw: usize, n: usize) -> Vec<Line> {
    (0..n)
        .map(|i| {
            let mut l = fit(left.get(i).cloned().unwrap_or_default(), lw);
            join(&mut l, divider());
            join(&mut l, fit(pane.get(i).cloned().unwrap_or_default(), pw));
            l
        })
        .collect()
}
