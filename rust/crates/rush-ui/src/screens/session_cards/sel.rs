//! Text dragged over in a panel (/btw, the quick ask): it stays inside
//! the panel, as a terminal's own selection can't, and goes to the
//! clipboard on release. After Go's internal/ui/textsel.go (cell :17,
//! textSel :21, selectWord :82, wordCells :90, wordAt :106, span :175,
//! cols :184, paintCols :213, selectedText :227), for plain rows: the
//! conversation's written-text spans are the session's.

use rush_core::text::{Color, Line, Span, Style};

use crate::frame::dialog::cells;
use crate::style::*;

/// The dragged-over text: the colour a terminal selects with (selBlue).
pub const SEL_BLUE: Color = Color::Rgb(58, 78, 122);

/// A place in a panel's text: a row and a column within it (cell).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
}

/// Text dragged over (textSel).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sel {
    /// There's a selection to show.
    pub on: bool,
    /// The mouse button is still down.
    pub drag: bool,
    /// The drag has left the cell it started in.
    pub moved: bool,
    pub a: Cell,
    pub b: Cell,
}

impl Sel {
    /// A drag begun at at.
    pub fn at(at: Cell) -> Sel {
        Sel { drag: true, a: at, b: at, ..Sel::default() }
    }

    /// The ends in order (span).
    pub fn span(&self) -> (Cell, Cell) {
        let (a, b) = (self.a, self.b);
        if b.row < a.row || (b.row == a.row && b.col < a.col) { (b, a) } else { (a, b) }
    }

    /// The part of row r the selection covers, [from, to) (cols).
    pub fn cols(&self, r: usize, w: usize) -> Option<(usize, usize)> {
        let (a, b) = self.span();
        if !self.on || r < a.row || r > b.row {
            return None;
        }
        let from = if r == a.row { a.col } else { 0 };
        // The cell under the pointer is taken in.
        let to = if r == b.row { (b.col + 1).min(w) } else { w };
        (from < to).then_some((from, to))
    }

    /// A double-click's selection: the word under it (selectWord).
    pub fn select_word(&mut self, row: &str) {
        if let Some((from, to)) = word_cells(row, self.a.col) {
            self.a.col = from;
            self.b.col = to - 1;
            self.on = true;
            self.moved = true;
        }
    }

    /// Moves the drag's end to at (drag).
    pub fn drag_to(&mut self, at: Cell) {
        if at != self.b {
            self.b = at;
            self.moved = true;
            self.on = true;
        }
    }
}

fn word_break(c: char) -> bool {
    c.is_whitespace() || "()[]{}<>\"'`,".contains(c)
}

/// The word in a row around cell col, [from, to) in cells (wordCells).
pub fn word_cells(row: &str, col: usize) -> Option<(usize, usize)> {
    let rs: Vec<char> = row.chars().collect();
    let mut at = vec![0usize; rs.len() + 1];
    let mut i = None;
    for (j, c) in rs.iter().enumerate() {
        at[j + 1] = at[j] + rush_core::text::width(c.encode_utf8(&mut [0; 4]));
        if i.is_none() && at[j + 1] > col {
            i = Some(j);
        }
    }
    let i = i?;
    if word_break(rs[i]) {
        return None;
    }
    let (mut from, mut to) = (i, i + 1);
    while from > 0 && !word_break(rs[from - 1]) {
        from -= 1;
    }
    while to < rs.len() && !word_break(rs[to]) {
        to += 1;
    }
    while to > i + 1 && ".:!?".contains(rs[to - 1]) {
        to -= 1;
    }
    (from < to).then_some((at[from], at[to]))
}

/// Cells [from, to) of l painted as selected, the styles either side
/// kept; a short row is selected out to where the drag reached
/// (paintCols).
pub fn paint_cols(l: &Line, from: usize, to: usize) -> Line {
    let mut l = l.clone();
    let n = l.width();
    if n < to {
        l.spans.push(plain(blanks(to - n)));
    }
    let mut out = cells(&l, 0, from);
    let mid: String = cells(&l, from, to).plain();
    out.spans.push(Span::new(mid, Style::fg(BRIGHT).on(SEL_BLUE)));
    let w = l.width();
    join(&mut out, cells(&l, to, w));
    out.bg = l.bg;
    out
}

/// The text between two cells of rows w wide: each row's part, its
/// trailing blanks gone, the edge in column 0 dropped (selectedText).
pub fn selected_text(rows: &[Line], a: Cell, b: Cell, w: usize) -> String {
    let s = Sel { on: true, a, b, ..Sel::default() };
    let (a, b) = s.span();
    let mut out = Vec::new();
    for r in a.row..=b.row.min(rows.len().saturating_sub(1)) {
        let Some((from, to)) = s.cols(r, w) else {
            out.push(String::new());
            continue;
        };
        let mut seg = cells(&rows[r], from, to).plain();
        if from == 0
            && let Some(c) = seg.chars().next()
            && "▏▍│".contains(c)
        {
            seg = format!(" {}", &seg[c.len_utf8()..]);
        }
        out.push(seg.trim_end().to_string());
    }
    // The indent every row shares goes.
    let indent = out
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    out.iter()
        .map(|l| l.get(indent..).unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}
