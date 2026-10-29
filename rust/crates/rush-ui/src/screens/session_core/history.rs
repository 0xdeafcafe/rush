//! #expand and #collapse: every turn of the open conversation shown
//! whole, or the turns gone past folded to what matters. After Go's
//! internal/ui/history_folds.go:20 (setHistoryFold), keys.go:1074 and
//! fleetslash.go:23 (the commands' words).
//!
//! Reading another agent's session from its history (Go's history.go) is
//! session.rs's Source::History and worker::spawn_history.

use super::{conn, open_key};
use crate::frame::{Ext, Out};
use crate::model::Model;

pub fn register(x: &mut Ext) {
    x.command(
        "expand",
        "open all conversation turns; tool details keep their own folds",
        |m, _| set_fold(m, true),
    );
    x.command(
        "collapse",
        "preview older turns, keeping the latest turn open",
        |m, _| set_fold(m, false),
    );
}

/// Turns shown whole (open) or folded once gone past, turns still
/// loading too; a turn opened or closed by hand gives way to it, a step's
/// own fold stays (setHistoryFold).
pub fn set_fold(m: &mut Model, open: bool) -> Out {
    let Some(key) = open_key(m) else {
        m.flash("open a session to change its history");
        return Out::Draw;
    };
    if let Some(v) = m.session.as_mut() {
        v.o.history_open = open;
        v.o.open
            .retain(|r, _| !(r.starts_with('t') && !r.contains(':')));
        // Collapsed, a picked step may go: its turn keeps the pick.
        if !open
            && v.sel.starts_with('t')
            && let Some((turn, _)) = v.sel.split_once(':')
        {
            v.sel = turn.to_string();
        }
    }
    conn(m, &key).vs.view = 0;
    if open {
        m.flash("every turn shown whole");
    } else {
        m.flash("turns gone past show what matters: what was said, what changed, what was made");
    }
    Out::Draw
}
