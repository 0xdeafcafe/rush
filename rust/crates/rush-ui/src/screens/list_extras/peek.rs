//! The peek: in the "agent" layout, where the Session has the screen,
//! leaving it shows Agents for a look round; enter (or { }) opens the one
//! picked, esc goes back to the one you left. After Go's
//! internal/ui/rushmode.go:5217-5237 (leaveChat's peekFrom, peeking,
//! openPeeked), keys.go:368-388 (esc and enter while peeking) and
//! view.go:2803-2808 (the peek's row of keys).

use rush_core::text::Line;

use crate::model::Model;
use crate::style::*;

/// The agent whose Session alone you left to peek at Agents (peekFrom).
#[derive(Default)]
struct PeekFrom(String);

/// The Session is closing: from the Session alone, kept that way, it's a
/// peek; anything else ends one (leaveChat). alone is whether the Session
/// had the screen.
pub fn leave_chat(m: &mut Model, alone: bool) {
    let from = if m.cfg.view == "agent" && alone && m.zen().is_none() {
        m.sel.clone()
    } else {
        String::new()
    };
    if from.is_empty() && m.kept.peek::<PeekFrom>().is_none() {
        return;
    }
    m.kept::<PeekFrom>().0 = from;
}

/// Ends a peek without opening anything (toggleList's peekFrom = "").
pub fn clear(m: &mut Model) {
    if m.kept.peek::<PeekFrom>().is_some() {
        m.kept::<PeekFrom>().0.clear();
    }
}

/// The agent the peek came from, while Agents are on screen for one
/// (peeking).
pub fn from(m: &Model) -> Option<&str> {
    let p = m.kept.peek::<PeekFrom>()?;
    let on = !p.0.is_empty()
        && m.cfg.view == "agent"
        && m.session.is_none()
        && super::split::widths(m).1 == 0
        && m.zen().is_none();
    on.then_some(p.0.as_str())
}

/// Whether Agents are on screen for a peek from the Session alone
/// (peeking).
pub fn peeking(m: &Model) -> bool {
    from(m).is_some()
}

/// esc while peeking: back to the agent you came from, its Session alone
/// again; false when there's no peek, or that agent has gone
/// (keys.go:368).
pub fn back(m: &mut Model) -> bool {
    let Some(k) = from(m).map(str::to_string) else {
        return false;
    };
    let Some(i) = m.agents.iter().position(|a| a.key == k) else {
        return false;
    };
    m.select(k);
    open_peeked(m, i);
    true
}

/// Ends a peek on agent i, its Session alone again (openPeeked).
pub fn open_peeked(m: &mut Model, i: usize) {
    clear(m);
    m.open(i);
}

/// The peek's row of keys, w wide (view.go:2803).
pub fn hint(m: &Model, w: usize) -> Line {
    let back = from(m)
        .and_then(|k| m.agents.iter().find(|a| a.key == k))
        .map_or("esc".to_string(), |a| {
            format!("back to {}", one_line(&a.display_name))
        });
    keys_fit(
        w,
        &[
            ("↑↓", "pick"),
            ("enter · { }", "open it"),
            ("esc", &back),
            ("shift+← · #view split", "side by side"),
            ("?", "guide"),
        ],
    )
}
