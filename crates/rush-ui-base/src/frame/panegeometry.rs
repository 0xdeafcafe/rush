//! The Session pane's header rows, carried into its gutters: the chrome's
//! ground left and right of the header's four rows (the right one the
//! wash's last colour), and the harness's edge beside its rule. Text and
//! transcript widths stay as they are. After Go's
//! internal/ui/panegeometry.go (paneTitleRow :6, paneFrameRow :14).

use rush_core::text::{Color, Line, Span, Style};

use crate::style::*;

pub const PANE_TITLE_ROW: usize = 1;
pub const PANE_META_ROW: usize = 2;
pub const PANE_TABS_ROW: usize = 3;

/// The left and right gutters of pane row `row` (paneFrameRow): two cells
/// left, one right. `wash_right` is the wash's colour at the row's right
/// edge, `border` the harness's colour, when a session is open.
pub fn gutters(
    header: bool,
    row: usize,
    wash_right: Option<Color>,
    border: Option<Color>,
) -> (Line, Line) {
    let blank = |n: usize, bg: Color| {
        Line::from_spans(vec![Span::new(" ".repeat(n), Style::default().on(bg))])
    };
    if header && row < 4 {
        return (
            blank(2, CHROME_BG),
            blank(1, wash_right.unwrap_or(CHROME_BG)),
        );
    }
    if header && row == 4 {
        let c = border.unwrap_or(FAINT);
        return (line([sp("──", c)]), line([sp("─", c)]));
    }
    (line([plain("  ")]), line([plain(" ")]))
}
