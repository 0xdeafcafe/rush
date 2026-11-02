//! How the screen splits between Agents and the Session beside it: the
//! widths, the screen narrowed to the pane while it's drawn, and the
//! Session put beside the list for now. The frame lays itself out by
//! these; the keys, the mouse and the drawing of the split are
//! screens/list_extras/split.rs's. After Go's internal/ui/view.go
//! (widths :881, sideWidth :917, chatOpen :981, chatAlone :985, canSplit
//! :1017, minPane :1078), model.go (wide :1920, autoSplit :1924) and
//! keys.go:86 (a Session filling a narrow screen has the keys).
//!
//! Not as Go, on purpose: the list goes down to 30 columns on any screen
//! (Go's least is a quarter of it), and a wide screen doesn't push it
//! wider so the Session stays under 128 columns (Go's maxPane).

use crate::frame::ext::Act;
use crate::model::Model;

/// The narrowest pane worth splitting the screen for (minPane).
pub const MIN_PANE: usize = 84;
/// The narrowest the list gets beside a Session, on any screen.
pub const MIN_LIST: usize = 30;

/// The screen's width: the model's, but while the opened Session beside
/// the list is drawn or takes a key, the model is narrowed to the pane
/// and the screen is kept aside (Model::screen_w).
pub fn sw(m: &Model) -> usize {
    if m.in_pane { m.screen_w } else { m.w }
}

/// Runs f with the model narrowed to the pane's width, pw, the screen's
/// kept aside: so the opened Session beside the list lays itself out,
/// takes keys and the mouse as it was drawn.
pub fn in_pane<T>(m: &mut Model, pw: usize, f: impl FnOnce(&mut Model) -> T) -> T {
    if m.in_pane {
        return f(m);
    }
    let full = m.w;
    m.screen_w = full;
    m.w = pw;
    m.in_pane = true;
    let out = f(m);
    m.in_pane = false;
    m.w = full;
    out
}

/// A screen wide enough to split without being asked (wide).
pub fn wide(w: usize) -> bool {
    w >= 170
}

/// Whether the Session shows beside the list without being asked: a wide
/// screen you haven't pushed to the list alone (autoSplit).
pub fn auto_split(m: &Model) -> bool {
    wide(sw(m)) && !m.cfg.list_only
}

/// Whether the screen is wide enough for Agents and the Session side by
/// side (canSplit).
pub fn can_split(w: usize) -> bool {
    w.saturating_sub(MIN_LIST + 1) >= MIN_PANE
}

/// The list's width in a split: your own share when you've set one,
/// else rush's, 28% of the screen up to 64 columns; never under 30
/// columns nor over three quarters (sideWidth, without maxPane's push).
pub fn side_width(m: &Model) -> usize {
    let w = sw(m);
    let floor = MIN_LIST;
    let f = m.cfg.side_width;
    if f > 0.0 {
        // A width you dragged to wins.
        let want = (w as f64 * f + 0.5) as usize;
        return floor.max(want.min(w * 3 / 4));
    }
    floor.max((w * 28 / 100).min(64))
}

/// Go's preview: a Session on screen, the opened one or the one ctrl+6
/// put beside the list for now.
pub fn preview(m: &Model) -> bool {
    m.session.is_some() || previewing(m)
}

/// Whether a Session is on screen, beside the list or alone (chatOpen).
pub fn chat_open(m: &Model) -> bool {
    m.full || preview(m) || auto_split(m)
}

/// Whether the Session has the whole screen: pushed there, or opened
/// with the Session alone as your layout (chatAlone).
pub fn chat_alone(m: &Model) -> bool {
    m.full || preview(m) && m.cfg.list_only && m.cfg.chat_full
}

/// How Go splits the screen (widths): (list, pane), the list 0 when the
/// Session has the screen, the pane 0 with the list alone. It may give
/// the Session the screen with none opened yet: `settle` opens it.
pub fn go_widths(m: &Model) -> (usize, usize) {
    let w = sw(m);
    if m.ext.calls.hosted_alone()(m) {
        return (0, w); // the one session, full width
    }
    if m.ext.calls.hosted_id()(m).is_some() {
        // Hosted's list, beside the session when there's room for both.
        let side = side_width(m);
        if w.saturating_sub(side + 1) >= MIN_PANE {
            return (side, w - side - 1);
        }
        return (w, 0);
    }
    if !chat_open(m) {
        return (w, 0);
    }
    // The list keeps at least 30 columns; if that leaves the pane too narrow to read, there's no split.
    let side = side_width(m);
    if chat_alone(m) {
        (0, w)
    } else if w.saturating_sub(side + 1) >= MIN_PANE {
        (side, w - side - 1)
    } else if preview(m) {
        (0, w)
    } else {
        (w, 0)
    }
}

/// How the screen splits between the list and the Session beside it:
/// (list, pane). The list 0 is the opened Session alone; the pane 0 the
/// list alone (widths). The Session has the screen only once one is
/// opened: till then, the list (`settle` opens it).
pub fn widths(m: &Model) -> (usize, usize) {
    match go_widths(m) {
        (0, _) if m.session.is_none() => (sw(m), 0),
        lp => lp,
    }
}

/// Opens the Session that is to fill the screen: one beside the list on
/// a screen too narrow for both, or with the Session alone as your
/// layout. "A Session filling a narrow screen has the only box there
/// is, so the keys are its" (keys.go:86).
pub fn settle(m: &mut Model) {
    if m.session.is_some() || !previewing(m) || go_widths(m).0 > 0 {
        return;
    }
    let key = m.sel.clone();
    if crate::rooms::is_room_key(&key) {
        m.act(Act::OpenRoom(key));
    } else if let Some(i) = m.focused() {
        m.open(i);
    }
    if m.session.is_none() {
        end_preview(m);
    }
}

/// Whether the list and the Session are side by side now.
pub fn side_by_side(m: &Model) -> bool {
    let (l, p) = widths(m);
    l > 0 && p > 0
}

/// The Session beside the list for now, though your layout is the list
/// alone: ctrl+6 brought it, or the opened one gave the list the keys
/// back (Go's preview, with no Session opened).
#[derive(Default)]
pub struct Preview(pub bool);

/// Shows the Session beside the list, where it isn't already (v on an
/// agent's screen, Go's m.preview = true).
pub fn show(m: &mut Model) {
    if widths(m).1 == 0 {
        m.kept::<Preview>().0 = true;
    }
}

/// A Session is beside the list for now (Preview).
pub fn previewing(m: &Model) -> bool {
    m.kept.peek::<Preview>().is_some_and(|p| p.0)
}

/// Lets go of the Session ctrl+6 put beside the list (leaveChat's
/// preview).
pub fn end_preview(m: &mut Model) {
    if previewing(m) {
        m.kept::<Preview>().0 = false;
    }
}
