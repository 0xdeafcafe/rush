//! Text dragged over in the conversation: it stays inside the pane, as a
//! terminal's own selection can't, scrolls the conversation when the drag
//! goes past its top or bottom, and goes to the clipboard on release; a
//! press that never moved is a click on the row. A double-click takes the
//! word under it. After Go's internal/ui/textsel.go (textSel :19,
//! textCell :45, startTextSel :68, selectWord :82, wordCells :90, wordAt
//! :107, dragTextSel :133, endTextSel :151, span :180, cols :190,
//! paintSel :206, selectedText :226), model.go:1605 (a double-click) and
//! rushmode.go (clickRef :4842; esc, ctrl+c and cmd+c on the dragged-over
//! text in paneKey :3960-4013).
//!
//! Go's rows drawn from text they don't show as written (a shell command
//! laid out) give that text; rush-convo's rows don't carry it yet, so the
//! rows are taken as drawn.

use rush_convo::RenderedLine;
use rush_core::text::{Color, Line, Span, Style};
use rush_draw::{Button, MouseKind};

use super::paint::cut;
use super::{conn, open_key};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::style::*;

/// The dragged-over text's ground: the colour a terminal selects with.
pub const SEL_BLUE: Color = Color::Rgb(58, 78, 122);

/// A place in the conversation's text: a row of it (not of the screen, so
/// scrolling doesn't move it) and a column within that row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
}

/// Text dragged over in the pane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextSel {
    /// There's a selection to show.
    pub on: bool,
    /// The mouse button is still down.
    pub drag: bool,
    /// The drag has left the cell it started in.
    pub moved: bool,
    /// Where the drag began and where it is now.
    pub a: Cell,
    pub b: Cell,
    /// Where it was pressed, for a click that never moved, and when.
    pub press: (usize, usize),
    pub at: i64,
}

impl TextSel {
    /// Its ends in order.
    pub fn span(&self) -> (Cell, Cell) {
        let (a, b) = (self.a, self.b);
        if b.row < a.row || b.row == a.row && b.col < a.col {
            (b, a)
        } else {
            (a, b)
        }
    }

    /// The part of row r it covers, [from, to), w wide.
    pub fn cols(&self, r: usize, w: usize) -> Option<(usize, usize)> {
        let (a, b) = self.span();
        if !self.on || r < a.row || r > b.row {
            return None;
        }
        let from = if r == a.row { a.col } else { 0 };
        let to = if r == b.row { (b.col + 1).min(w) } else { w };
        (from < to).then_some((from, to))
    }

    /// A double-click's selection: the word under it.
    fn select_word(&mut self, line: &str) {
        if let Some((from, to)) = word_cells(line, self.a.col) {
            self.a.col = from;
            self.b.col = to - 1;
            self.on = true;
            self.moved = true;
        }
    }
}

/// Drops the selection of session key (another view, esc).
pub fn drop(m: &mut Model, key: &str) {
    if !conn(m, key).txt.drag {
        conn(m, key).txt = TextSel::default();
    }
}

/// Whether a char ends a word: blanks, brackets, quotes, commas and the
/// conversation's rails.
fn word_break(c: char) -> bool {
    c.is_whitespace() || "()[]{}<>\"'`,;|▏┊▍│".contains(c)
}

/// The word around cs[i], [from, to): out to the nearest break either
/// side, less a full stop or colon ending it, so a path or URL is taken
/// whole.
pub fn word_at(cs: &[char], i: usize) -> (usize, usize) {
    if i >= cs.len() || word_break(cs[i]) {
        return (0, 0);
    }
    let (mut from, mut to) = (i, i + 1);
    while from > 0 && !word_break(cs[from - 1]) {
        from -= 1;
    }
    while to < cs.len() && !word_break(cs[to]) {
        to += 1;
    }
    while to > i + 1 && ".:!?".contains(cs[to - 1]) {
        to -= 1;
    }
    (from, to)
}

/// The word in a drawn row around cell col, [from, to) in cells.
pub fn word_cells(line: &str, col: usize) -> Option<(usize, usize)> {
    let cs: Vec<char> = line.chars().collect();
    let mut at = vec![0; cs.len() + 1];
    let mut i = None;
    for (j, c) in cs.iter().enumerate() {
        at[j + 1] = at[j] + rush_core::text::width(c.encode_utf8(&mut [0; 4]));
        if i.is_none() && at[j + 1] > col {
            i = Some(j);
        }
    }
    let (from, to) = word_at(&cs, i?);
    (from < to).then(|| (at[from], at[to]))
}

/// The body cell under the pointer, clamped to what's shown: above or
/// below the body it's the first or last row in view (textCell).
fn text_cell(m: &Model, x: usize, y: usize) -> Option<Cell> {
    let v = m.session.as_ref()?;
    let n = v.shown.len();
    if n == 0 {
        return None;
    }
    let i = y.saturating_sub(v.top).min(n - 1);
    let w = v.o.width;
    Some(Cell {
        row: v.start + i,
        col: x.saturating_sub(2).min(w),
    })
}

/// The rows of the conversation drawn about the view, by row number.
fn drawn_row(m: &Model, r: usize) -> Option<&RenderedLine> {
    let v = m.session.as_ref()?;
    let rows = v.r.rows();
    rows.get(r.checked_sub(v.base)?).copied()
}

/// What's dragged over, as written (selectedText).
pub fn text_of(m: &Model, s: &TextSel) -> String {
    let Some(v) = m.session.as_ref() else {
        return String::new();
    };
    let (a, b) = s.span();
    let base = v.base;
    let rows = v.r.rows();
    let lines: Vec<(&Line, bool)> = rows.iter().map(|l| (&l.line, l.wrap)).collect();
    // Rows above the window drawn are gone: it starts at its first.
    let rel = |c: Cell| match c.row.checked_sub(base) {
        Some(row) => Cell { row, col: c.col },
        None => Cell::default(),
    };
    selected_text(&lines, rel(a), rel(b), v.o.width)
}

/// The text between two cells of the drawn lines, as it was written
/// rather than laid out: rows the pane wrapped joined again, the
/// conversation's spine and the indent every row shares dropped, and
/// trailing blanks gone (selectedText).
pub fn selected_text(lines: &[(&Line, bool)], a: Cell, b: Cell, w: usize) -> String {
    let (a, b) = if b.row < a.row || b.row == a.row && b.col < a.col {
        (b, a)
    } else {
        (a, b)
    };
    let s = TextSel {
        on: true,
        a,
        b,
        ..TextSel::default()
    };
    let mut out: Vec<String> = Vec::new();
    for r in a.row..=b.row {
        let Some(&(l, wrap)) = lines.get(r) else {
            break;
        };
        let cols = s.cols(r, w);
        let mut seg = cols.map_or(String::new(), |(f, t)| cut(l, f, t).plain());
        if cols.is_some_and(|(f, _)| f == 0) {
            // The spine and selection marker sit in column 0.
            let mut cs = seg.chars();
            if let Some(c) = cs.next()
                && "▏▍│".contains(c)
            {
                seg = format!(" {}", cs.as_str());
            }
        }
        // An output's rail sits after its indent: layout, not text.
        let t = seg.trim_start_matches(' ');
        if let Some(rest) = t.strip_prefix('▏').or_else(|| t.strip_prefix('┊')) {
            seg = format!("{}{rest}", &seg[..seg.len() - t.len()]);
        }
        let seg = seg.trim_end_matches(' ').to_string();
        if wrap && r > a.row && !out.is_empty() {
            let prev = out.last_mut().expect("a row before");
            let sep = if prev.ends_with('-') || prev.is_empty() {
                ""
            } else {
                " "
            };
            prev.push_str(sep);
            prev.push_str(seg.trim_start_matches(' '));
            continue;
        }
        out.push(seg);
    }
    // The indent every row shares is the layout's, not the text's. A first
    // row that starts mid-line has none to judge by.
    let mut indent: Option<usize> = None;
    for (i, l) in out.iter().enumerate() {
        if l.trim().is_empty() || i == 0 && a.col > 0 {
            continue;
        }
        let n = l.len() - l.trim_start_matches(' ').len();
        indent = Some(indent.map_or(n, |c| c.min(n)));
    }
    let cut_at = indent.unwrap_or(0);
    for (i, l) in out.iter_mut().enumerate() {
        if i == 0 && a.col > 0 {
            *l = l.trim_start_matches(' ').to_string();
        } else if l.len() >= cut_at && cut_at > 0 {
            *l = l[cut_at..].to_string();
        }
    }
    out.join("\n").trim_matches('\n').to_string()
}

/// Paints the selection over the conversation's rows on screen, the first
/// of them row start, w wide (paintSel, paintCols).
pub fn paint(rows: &mut [Line], start: usize, s: &TextSel, w: usize) {
    if !s.on {
        return;
    }
    for (i, l) in rows.iter_mut().enumerate() {
        let Some((from, to)) = s.cols(start + i, w) else {
            continue;
        };
        // A short row is selected out to where the drag reached.
        let full = pad(l.clone(), to);
        let mut out = cut(&full, 0, from);
        let mid = cut(&full, from, to).plain();
        out.spans
            .push(Span::new(mid, Style::fg(BRIGHT).on(SEL_BLUE)));
        let rest = cut(&full, to, full.width());
        out.spans.extend(rest.spans);
        out.bg = l.bg;
        *l = out;
    }
}

/// The conversation's rows a press, drag or release is over, and what
/// they do: start, move and end the selection (startTextSel,
/// dragTextSel, endTextSel).
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if m.session.is_none() || m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    match e.kind {
        MouseKind::Down(Button::Left) => {
            if super::views::view_name(m, &key) != "conversation" {
                return Out::Pass;
            }
            let Some(v) = m.session.as_ref() else {
                return Out::Pass;
            };
            let n = v.shown.len();
            let cw = v.o.width;
            if y < v.top || y >= v.top + n || x >= 2 + cw {
                return Out::Pass; // the header, the dock, the map
            }
            let Some(at) = text_cell(m, x, y) else {
                return Out::Pass;
            };
            let now = m.now;
            let was = conn(m, &key).txt;
            let dbl = was.press == (x, y) && now - was.at < 400;
            let mut t = TextSel {
                drag: true,
                a: at,
                b: at,
                press: (x, y),
                at: now,
                ..TextSel::default()
            };
            if dbl && let Some(l) = drawn_row(m, at.row) {
                t.select_word(&l.line.plain());
            }
            conn(m, &key).txt = t;
            if t.on { Out::Draw } else { Out::Idle }
        }
        MouseKind::Drag(Button::Left) => {
            if !conn(m, &key).txt.drag {
                return Out::Pass;
            }
            let rows = conn(m, &key).vs.rows.max(1);
            if let Some(v) = m.session.as_mut() {
                // Past the top or bottom the conversation scrolls a row.
                if y < v.top {
                    v.scroll(1, rows);
                } else if y >= v.top + v.shown.len() {
                    v.scroll(-1, rows);
                }
            }
            if let Some(at) = text_cell(m, x, y) {
                let t = &mut conn(m, &key).txt;
                if at != t.b {
                    t.b = at;
                    t.moved = true;
                    t.on = true;
                }
            }
            Out::Draw
        }
        MouseKind::Up(Button::Left) => {
            let t = conn(m, &key).txt;
            if !t.drag {
                return Out::Pass;
            }
            conn(m, &key).txt.drag = false;
            if !t.moved {
                conn(m, &key).txt = TextSel {
                    press: t.press,
                    at: t.at,
                    ..TextSel::default()
                };
                return click(m, &key, t.press.1);
            }
            let text = text_of(m, &t);
            if !text.is_empty() {
                m.copy_text(text);
            }
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// A click on the conversation's row at screen row y (clickRef): a turn's
/// heading, or a row already picked, opens or closes; another row is
/// picked; "show all" shows all.
fn click(m: &mut Model, key: &str, y: usize) -> Out {
    let rows = conn(m, key).vs.rows.max(1);
    let Some(v) = m.session.as_mut() else {
        return Out::Pass;
    };
    let Some(r) = v.ref_at(y).map(str::to_string) else {
        return Out::Idle;
    };
    if r == rush_convo::render::SHOW_ALL_REF {
        v.verbose = !v.verbose;
        return Out::Draw;
    }
    let turn =
        r.len() > 1 && r.starts_with('t') && r.as_bytes()[1].is_ascii_digit() && !r.contains(':');
    if r == v.sel || r.starts_with("job:") || turn {
        v.sel = r;
        v.toggle(rows);
        return Out::Draw;
    }
    v.sel = r;
    Out::Draw
}

/// esc drops the dragged-over text; ctrl+c and cmd+c copy it again, as a
/// terminal would, with nothing typed (or selected) in the box.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let t = conn(m, &key).txt;
    if !t.on {
        return Out::Pass;
    }
    let empty = conn(m, &key).input.is_empty();
    let boxed = conn(m, &key).input.selection().is_some();
    match k.s.as_str() {
        "esc" if empty => {
            conn(m, &key).txt = TextSel::default();
            Out::Draw
        }
        "ctrl+c" if empty && !boxed => {
            let text = text_of(m, &t);
            m.copy_text(text);
            Out::Draw
        }
        "super+c" if !boxed => {
            let text = text_of(m, &t);
            m.copy_text(text);
            Out::Draw
        }
        _ => Out::Pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_take_paths_whole() {
        let line = "  see ~/src/rush/main.go: it (calls) foo.";
        let col = line.find("rush").unwrap();
        let (f, t) = word_cells(line, col).unwrap();
        assert_eq!(&line[f..t], "~/src/rush/main.go");
        let col = line.find("calls").unwrap();
        let (f, t) = word_cells(line, col).unwrap();
        assert_eq!(&line[f..t], "calls");
        let col = line.find("foo").unwrap();
        let (f, t) = word_cells(line, col).unwrap();
        assert_eq!(&line[f..t], "foo");
        assert_eq!(word_cells(line, 0), None);
    }

    #[test]
    fn cols_over_rows() {
        let s = TextSel {
            on: true,
            a: Cell { row: 3, col: 5 },
            b: Cell { row: 1, col: 2 },
            ..TextSel::default()
        };
        assert_eq!(s.cols(1, 10), Some((2, 10)));
        assert_eq!(s.cols(2, 10), Some((0, 10)));
        assert_eq!(s.cols(3, 10), Some((0, 6)));
        assert_eq!(s.cols(4, 10), None);
    }

    #[test]
    fn selected_text_joins_wraps_and_drops_layout() {
        let rows = [
            line([plain("▍ "), plain("  hello there")]),
            line([plain("    world")]),
            line([plain("    ▏output line")]),
        ];
        let lines: Vec<(&Line, bool)> =
            vec![(&rows[0], false), (&rows[1], true), (&rows[2], false)];
        let t = selected_text(
            &lines,
            Cell { row: 0, col: 0 },
            Cell { row: 2, col: 40 },
            40,
        );
        assert_eq!(t, "hello there world\noutput line");
    }

    #[test]
    fn paint_marks_cells() {
        let mut rows = vec![line([plain("abcdef")])];
        let s = TextSel {
            on: true,
            a: Cell { row: 7, col: 1 },
            b: Cell { row: 7, col: 3 },
            ..TextSel::default()
        };
        paint(&mut rows, 7, &s, 10);
        assert_eq!(rows[0].plain(), "abcdef");
        let sel: Vec<&Span> = rows[0]
            .spans
            .iter()
            .filter(|s| s.style.bg == SEL_BLUE)
            .collect();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].text, "bcd");
    }
}
