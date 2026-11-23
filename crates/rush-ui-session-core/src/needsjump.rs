//! ctrl+n with a conversation open: straight to the agent that has
//! waited longest for you, as the header's "needs you" chip says
//! (rush-ui-base header.rs `waiting_elsewhere`). Rust-only: Go's ctrl+n
//! (keys.go:595 nextNeedingYou) is the list's alone and only picks the
//! row, so from a conversation it took esc, ctrl+n and enter.

use crate::frame::{Out, Press};
use crate::model::Model;

/// The key, before the box's: nothing else in a conversation takes
/// ctrl+n while another agent waits; with none waiting it passes.
pub fn key(m: &mut Model, k: &Press) -> Out {
    if k.s != "ctrl+n" || m.sheet.is_some() || m.confirm.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some((_, key, _)) = m.waiting_elsewhere() else {
        return Out::Pass;
    };
    super::menubar_jump::jump_to(m, key);
    Out::Draw
}
