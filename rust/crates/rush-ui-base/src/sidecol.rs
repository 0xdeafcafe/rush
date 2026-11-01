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

/// The column's width right of text w wide: what's left past the
/// conversation's widest row, 44 cells at least; 0 when there's no room
/// (sideWidth). Not as Go, on purpose: Go stops it at 80, but a Session
/// beside the list can now be far wider than its text, and the column
/// takes all of that room rather than leaving it empty.
pub fn side_width(w: usize) -> usize {
    const TEXT: usize = 124; // convo's capRow: its rows never run further right
    match w.checked_sub(TEXT + 3) {
        Some(sw) if sw >= 44 => sw,
        _ => 0,
    }
}

/// How wide the rows inside the column, sw wide, are laid out: the
/// column less its edge, 79 at most (Go's widest column, 80). A wider
/// column is still a card, its ground filling it, so its parts read
/// together rather than spread across the room.
pub fn text_width(sw: usize) -> usize {
    sw.saturating_sub(1).min(79)
}
