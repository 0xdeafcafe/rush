//! The column right of a wide conversation, what both the session's
//! screen and the plugins' views beside it need to lay out: its width and
//! its heading rows. The column itself is session_core/side.rs.

use rush_core::text::Line;

use crate::style::*;

/// A click on the folded views opens them; on a view's row, at that view.
pub const VIEWS_REF: &str = "side:views";

/// The first row of a part of the dock or the column beside the
/// conversation, the same for each: its name, then what it says about
/// itself. Its colour is the edge's (sideHead).
pub fn side_head(name: &str, rest: Line) -> Line {
    let mut l = line([plain(" "), spb(name, SUB), plain("  ")]);
    join(&mut l, rest);
    l
}

/// The column's width right of text w wide: three quarters of the room
/// left past the conversation's widest row, 44 cells at least; 0 when
/// that room is under 44 (sideWidth). At 200 wide it is 54 (it was the
/// whole room, 73), at 260 it is 99 (was 133); the rest of the room stays
/// blank between the text and the column, which keeps to the right edge.
/// The agent's views beside the conversation take this same width.
pub fn side_width(w: usize) -> usize {
    side_width_with(w, 0)
}

/// The column's least width, in columns.
pub const SIDE_MIN: usize = 44;

/// The column's width as side_width, but at the width you set (pref,
/// columns; 0 is rush's own): kept between 44 and all the room left of
/// the conversation's widest row, so the text stays as wide as ever.
pub fn side_width_with(w: usize, pref: i64) -> usize {
    const TEXT: usize = 124; // convo's capRow: its rows never run further right
    match w.checked_sub(TEXT + 3) {
        Some(room) if room >= SIDE_MIN => match pref {
            p if p > 0 => (p as usize).clamp(SIDE_MIN, room),
            _ => (room * 3 / 4).max(SIDE_MIN),
        },
        _ => 0,
    }
}

/// The most the column can be, w wide in all; 0 when it isn't showing.
pub fn side_room(w: usize) -> usize {
    w.checked_sub(127).filter(|&r| r >= SIDE_MIN).unwrap_or(0)
}

/// How wide the rows inside the column, sw wide, are laid out: the
/// column less its edge, 79 at most (Go's widest column, 80). A wider
/// column is still a card, its ground filling it, so its parts read
/// together rather than spread across the room.
pub fn text_width(sw: usize) -> usize {
    text_width_with(sw, 0)
}

/// text_width, the column's width being one you set (pref, as
/// side_width_with's): then its rows are as wide as it, the card no wider
/// than you asked for, so widening it gives its rows the room (Rust-only).
pub fn text_width_with(sw: usize, pref: i64) -> usize {
    if pref > 0 {
        sw.saturating_sub(1)
    } else {
        sw.saturating_sub(1).min(79)
    }
}
