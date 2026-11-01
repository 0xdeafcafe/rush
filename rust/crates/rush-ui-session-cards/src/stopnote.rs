//! shift+x (or shift+k) on a picked task, command or subagent stops it as
//! x (or k) would, with a message: the box takes what to tell the agent,
//! and enter stops it and sends that straight after, so the agent hears
//! why rather than only that it was stopped. After Go's
//! internal/ui/stopnote.go (stopNote :15, stopNoteKey :20, stoppable :45).
//! What's picked and stopping it (pickedShell, pickedJob, pickedSub,
//! stopPicked) are the session's: [`HostInfo::stops`] says what x and k
//! would stop, [`Act::Stop`] stops it.

use super::{Act, HostInfo, act, conn};
use crate::frame::{Out, Press};
use crate::model::Model;

/// A stop waiting on its message: the key it stands for and what was
/// picked when it was pressed (stopNote).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StopNote {
    pub key: String,
    pub sel: String,
}

/// Starts, sends or drops a stop with a message on agent key's session;
/// empty is whether its box is empty. Pass when the key isn't one
/// (stopNoteKey).
pub fn stop_note_key(m: &mut Model, key: &str, inf: &HostInfo, s: &str, empty: bool) -> Out {
    if let Some(n) = conn(m, key).stop_note.clone() {
        return match s {
            "esc" => {
                conn(m, key).stop_note = None;
                set_sel(m, key, n.sel);
                Out::Draw
            }
            "enter" => {
                conn(m, key).stop_note = None;
                set_sel(m, key, String::new());
                act(
                    m,
                    key,
                    Act::Stop {
                        key: n.key,
                        sel: n.sel,
                        send: !inf.input.trim().is_empty(),
                    },
                )
            }
            _ => Out::Pass,
        };
    }
    let k = match s {
        "X" | "shift+x" => "x",
        "K" | "shift+k" => "k",
        _ => return Out::Pass,
    };
    if !empty || !stoppable(inf, k) {
        return Out::Pass;
    }
    let sel = super::session_view(m, key)
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    conn(m, key).stop_note = Some(StopNote { key: k.into(), sel });
    // The box has the keys.
    set_sel(m, key, String::new());
    Out::Draw
}

// Picks row sel in agent key's open conversation.
fn set_sel(m: &mut Model, key: &str, sel: String) {
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
        v.sel = sel;
    }
}

/// Whether x (or k) would stop something picked now (stoppable).
pub fn stoppable(inf: &HostInfo, key: &str) -> bool {
    if key == "k" { inf.stops.1 } else { inf.stops.0 }
}

/// The session keys' hook: a stop with a message on the open session.
pub fn stop_note_hook(m: &mut Model, k: &Press) -> Out {
    let Some(key) = super::open_key(m) else {
        return Out::Pass;
    };
    let Some(inf) = super::info(m, &key) else {
        return Out::Pass;
    };
    let empty = inf.input.is_empty();
    stop_note_key(m, &key, &inf, &k.s, empty)
}
