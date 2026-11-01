//! A paste chip lit under the pointer: space opens it (model.go:1464).
//! The pastes themselves are the frame's (crate::pastes).

use crate::frame::editor::Seg;
use crate::frame::{Out, Press};
use crate::model::Model;

// A box's pastes are the frame's (crate::pastes): each box keeps its own.
pub use rush_ui_base::pastes::{PASTE_CHIP, Pastes, chip_for, has_paste_chip, is_long_paste};

/// A key while a paste chip is lit under the pointer, in either box: it's
/// unlit, and space opens it while the boxes take keys, nothing over them
/// (model.go:1464-1469, boxesTakeKeys).
pub fn chip_key(m: &mut Model, k: &Press) -> Out {
    let boxes = m.confirm.is_none()
        && m.sheet.is_none()
        && !m.on_place()
        && !(m.ext.calls.bar_open())(m);
    let s = if boxes { k.s.as_str() } else { "" };
    if let Some(key) = crate::sess::open_key(m)
        && crate::sess::conn(m, &key).input.hot != Seg::default()
        && crate::sess::conn(m, &key).input.key_on_hot(s)
    {
        return Out::Draw;
    }
    if m.input.hot != Seg::default() && m.input.key_on_hot(s) {
        return Out::Draw;
    }
    Out::Pass
}
