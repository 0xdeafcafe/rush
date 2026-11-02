//! Styled text as the renderer builds it: runs of text each in a style, in
//! place of Go's strings with escape codes in them. Port of the string
//! helpers rush draws with: style.go:125 (paint), :205 (row), :305 (wrap),
//! charmbracelet/x/ansi's Wrap (wrap.go:293) which wrap follows rule for
//! rule, ansi.Truncate and TruncateLeft, and render.go:4010 (codeRows).
//! A run with no colour of its own takes the colour of whatever paints it,
//! as text between Go's resets takes the colour around it.

use rush_core::text::{Color, Line, Span, Style};
use unicode_width::UnicodeWidthChar;

/// Styled text: runs in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Txt(pub Vec<Span>);

pub(crate) fn cw(c: char) -> usize {
    if c == '\t' {
        return 1; // as ansi counts a control: rush expands tabs first
    }
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Cells s takes.
pub(crate) fn width(s: &str) -> usize {
    if s.is_ascii() {
        return s.len();
    }
    s.chars().map(cw).sum()
}

impl Txt {
    pub fn new() -> Txt {
        Txt(Vec::new())
    }

    pub fn styled(s: &str, st: Style) -> Txt {
        if s.is_empty() {
            return Txt::new();
        }
        Txt(vec![Span::new(s, st)])
    }

    /// Text with no colour of its own.
    pub fn raw(s: &str) -> Txt {
        Txt::styled(s, Style::default())
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|s| s.text.is_empty())
    }

    pub fn push(&mut self, t: Txt) {
        for s in t.0 {
            if !s.text.is_empty() {
                self.0.push(s);
            }
        }
    }

    pub fn push_str(&mut self, s: &str, st: Style) {
        if !s.is_empty() {
            self.0.push(Span::new(s, st));
        }
    }

    /// self then t.
    pub fn and(mut self, t: Txt) -> Txt {
        self.push(t);
        self
    }

    /// self then s with no colour of its own.
    pub fn and_raw(mut self, s: &str) -> Txt {
        self.push_str(s, Style::default());
        self
    }

    pub fn width(&self) -> usize {
        self.0.iter().map(|s| width(&s.text)).sum()
    }

    /// The text without styles.
    pub fn plain(&self) -> String {
        let mut out = String::with_capacity(self.0.iter().map(|s| s.text.len()).sum());
        for s in &self.0 {
            out.push_str(&s.text);
        }
        out
    }

    /// Paints the runs without a colour of their own in c, and adds c's
    /// attributes to them (Go's paint(c, s) around styled s).
    pub fn over(mut self, c: Style) -> Txt {
        for s in &mut self.0 {
            if s.style.fg == Color::Default {
                s.style.fg = c.fg;
                s.style.mods = s.style.mods.with(c.mods);
            }
            if s.style.bg == Color::Default {
                s.style.bg = c.bg;
            }
        }
        self
    }

    /// Lays a background under every run that hasn't one.
    pub fn on(mut self, bg: Color) -> Txt {
        if bg != Color::Default {
            for s in &mut self.0 {
                if s.style.bg == Color::Default {
                    s.style.bg = bg;
                }
            }
        }
        self
    }

    /// The first w cells; past them, cut so that tail fits (ansi.Truncate).
    pub fn truncate(&self, w: usize, tail: &Txt) -> Txt {
        if self.width() <= w {
            return self.clone();
        }
        let tw = tail.width();
        let keep = w.saturating_sub(tw);
        let mut out = self.take(keep);
        if w >= tw {
            out.push(tail.clone());
        }
        out
    }

    /// The first w cells, a wide character that would cross w left out.
    pub fn take(&self, w: usize) -> Txt {
        let mut out = Vec::new();
        let mut n = 0;
        for s in &self.0 {
            if n >= w {
                break;
            }
            let sw = width(&s.text);
            if n + sw <= w {
                out.push(s.clone());
                n += sw;
                continue;
            }
            let mut cut = 0;
            for (i, c) in s.text.char_indices() {
                let k = cw(c);
                if n + k > w {
                    break;
                }
                n += k;
                cut = i + c.len_utf8();
            }
            if cut > 0 {
                out.push(Span::new(&s.text[..cut], s.style));
            }
            break;
        }
        Txt(out)
    }

    /// What's left past the first w cells (ansi.TruncateLeft).
    pub fn skip(&self, w: usize) -> Txt {
        let mut out = Vec::new();
        let mut n = 0;
        for s in &self.0 {
            if n >= w {
                out.push(s.clone());
                continue;
            }
            let sw = width(&s.text);
            if n + sw <= w {
                n += sw;
                continue;
            }
            let mut from = s.text.len();
            for (i, c) in s.text.char_indices() {
                if n >= w {
                    from = i;
                    break;
                }
                n += cw(c);
            }
            if from < s.text.len() {
                out.push(Span::new(&s.text[from..], s.style));
            }
        }
        Txt(out)
    }

    /// Whether the text is nothing but spaces.
    pub fn is_blank(&self) -> bool {
        self.0.iter().all(|s| s.text.trim().is_empty())
    }

    pub fn into_line(self) -> Line {
        Line::from_spans(self.0)
    }
}

/// n spaces.
pub(crate) fn blanks(n: usize) -> Txt {
    if n == 0 {
        return Txt::new();
    }
    Txt::raw(&" ".repeat(n))
}

/// A row: left, then right ending at capw so numbers stay near their
/// labels on wide panes, on background bg (style.go:205). The rest of the
/// row past capw is the line's own fill, not spaces.
pub(crate) fn row(bg: Color, left: Txt, right: Txt, width: usize, capw: usize) -> Line {
    let capw = if capw > width || capw == 0 {
        width
    } else {
        capw
    };
    let mut right = right;
    let mut rw = right.width();
    if rw + 4 > capw {
        right = Txt::new();
        rw = 0;
    }
    let mut room = capw - rw;
    if rw > 0 {
        room = room.saturating_sub(2);
    }
    let mut left = left;
    let mut lw = left.width();
    if lw > room {
        left = left.truncate(room, &Txt::raw("…"));
        lw = left.width();
    }
    let mut t = left;
    if rw > 0 {
        t.push_str(&" ".repeat(capw.saturating_sub(lw + rw)), Style::default());
        t.push(right);
    }
    let mut l = t.on(bg).into_line();
    l.bg = bg;
    l
}

/// Wraps styled text to w cells, breaking at spaces, and words longer than
/// a row across rows; a row of nothing but spaces at the end goes on the
/// one before it (style.go:305, with w at least 10).
pub(crate) fn wrap(t: &Txt, w: usize) -> Vec<Txt> {
    wrap_at(t, w.max(10), "")
}

/// A cell of styled text, for wrapping.
#[derive(Clone, Copy)]
struct Cell {
    c: char,
    st: Style,
    w: usize,
}

/// Rows of cells, gathered into runs.
struct Rows {
    rows: Vec<Vec<Cell>>,
}

impl Rows {
    fn cur(&mut self) -> &mut Vec<Cell> {
        self.rows.last_mut().unwrap()
    }
}

fn to_txt(cells: &[Cell]) -> Txt {
    let mut out: Vec<Span> = Vec::new();
    for c in cells {
        match out.last_mut() {
            Some(s) if s.style == c.st => s.text.push(c.c),
            _ => out.push(Span::new(c.c.to_string(), c.st)),
        }
    }
    Txt(out)
}

/// ansi.Wrap: words wrap at spaces; - and breakpoints break a word too; a
/// word longer than the limit is cut. ASCII and other characters take the
/// two paths the Go code does, as their rules differ slightly.
#[allow(unused_assignments)]
pub(crate) fn wrap_at(t: &Txt, limit: usize, breakpoints: &str) -> Vec<Txt> {
    if limit < 1 || t.width() <= limit && !t.0.iter().any(|s| s.text.contains('\n')) {
        return vec![t.clone()];
    }
    let mut out = Rows {
        rows: vec![Vec::new()],
    };
    let mut word: Vec<Cell> = Vec::new();
    let mut space: Vec<Cell> = Vec::new();
    let (mut space_w, mut cur_w, mut word_len) = (0usize, 0usize, 0usize);

    macro_rules! add_space {
        () => {
            cur_w += space_w;
            out.cur().append(&mut space);
            space_w = 0;
        };
    }
    macro_rules! add_word {
        () => {
            if !word.is_empty() {
                add_space!();
                cur_w += word_len;
                out.cur().append(&mut word);
                word_len = 0;
            }
        };
    }
    macro_rules! add_newline {
        () => {
            out.rows.push(Vec::new());
            cur_w = 0;
            space.clear();
            space_w = 0;
        };
    }

    for s in &t.0 {
        for c in s.text.chars() {
            let cell = Cell {
                c,
                st: s.style,
                w: cw(c),
            };
            if !c.is_ascii() {
                if c.is_whitespace() && c != '\u{a0}' {
                    add_word!();
                    space.push(cell);
                    space_w += cell.w;
                } else if breakpoints.contains(c) {
                    add_space!();
                    if cur_w + word_len + cell.w > limit {
                        word.push(cell);
                        word_len += cell.w;
                    } else {
                        add_word!();
                        out.cur().push(cell);
                        cur_w += cell.w;
                    }
                } else {
                    if word_len + cell.w > limit {
                        add_word!();
                    }
                    word.push(cell);
                    word_len += cell.w;
                    if cur_w + word_len + space_w > limit {
                        add_newline!();
                    }
                    if word_len == limit {
                        add_word!();
                    }
                }
                continue;
            }
            match c {
                '\n' => {
                    if word_len == 0 {
                        if cur_w + space_w > limit {
                            cur_w = 0;
                        } else {
                            out.cur().append(&mut space);
                        }
                        space.clear();
                        space_w = 0;
                    }
                    add_word!();
                    add_newline!();
                }
                ' ' | '\t' | '\x0b' | '\x0c' | '\r' => {
                    add_word!();
                    space.push(Cell { w: 1, ..cell });
                    space_w += 1;
                }
                _ if c == '-' || breakpoints.contains(c) => {
                    add_space!();
                    if cur_w + word_len >= limit {
                        word.push(Cell { w: 1, ..cell });
                        word_len += 1;
                    } else {
                        add_word!();
                        out.cur().push(Cell { w: 1, ..cell });
                        cur_w += 1;
                    }
                }
                _ => {
                    if cur_w == limit {
                        add_newline!();
                    }
                    word.push(Cell { w: 1, ..cell });
                    word_len += 1;
                    if word_len == limit {
                        add_word!();
                    }
                    if cur_w + word_len + space_w > limit {
                        add_newline!();
                    }
                }
            }
        }
    }
    if word_len == 0 {
        if cur_w + space_w > limit {
            // dropped
        } else {
            out.cur().append(&mut space);
        }
        space.clear();
    }
    add_word!();
    let mut rows: Vec<Txt> = out.rows.iter().map(|r| to_txt(r)).collect();
    // A row of nothing but spaces at the end goes on the one before.
    while rows.len() > 1 && rows.last().is_some_and(Txt::is_blank) {
        let last = rows.pop().unwrap();
        rows.last_mut().unwrap().push(last);
    }
    rows
}

/// Wraps a highlighted line to rows w cells wide, at a space or after a
/// hyphen; past most rows the rest is cut unless whole (render.go:4010).
pub(crate) fn code_rows(s: &Txt, w: usize, most: usize, whole: bool) -> Vec<Txt> {
    let w = w.max(4);
    if s.width() <= w {
        return vec![s.clone()];
    }
    let mut rows = wrap_at(s, w, "-");
    if !whole && most > 0 && rows.len() > most {
        rows.truncate(most);
        let last = rows.pop().unwrap();
        let mut cut = last.take(w - 1);
        cut.push_str("›", Style::default());
        rows.push(cut);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(rows: &[Txt]) -> Vec<String> {
        rows.iter().map(Txt::plain).collect()
    }

    #[test]
    fn wraps_like_ansi_wrap() {
        let t = Txt::raw("the quick brown fox jumps over the lazy dog");
        assert_eq!(
            plain(&wrap_at(&t, 10, "")),
            ["the quick", "brown fox", "jumps over", "the lazy", "dog"]
        );
        // A word longer than a row is cut.
        let t = Txt::raw("https://example.com/a/very/long/path");
        assert_eq!(
            plain(&wrap_at(&t, 12, "")),
            ["https://exam", "ple.com/a/ve", "ry/long/path"]
        );
        // Hyphens break words.
        let t = Txt::raw("a well-known thing");
        assert_eq!(plain(&wrap_at(&t, 7, "")), ["a well-", "known", "thing"]);
    }

    #[test]
    fn truncates() {
        let t = Txt::raw("hello world");
        assert_eq!(t.truncate(8, &Txt::raw("…")).plain(), "hello w…");
        assert_eq!(t.skip(6).plain(), "world");
    }

    #[test]
    fn rows_put_right_at_the_cap() {
        let l = row(Color::Default, Txt::raw("left"), Txt::raw("right"), 40, 20);
        assert_eq!(l.plain(), "left           right");
    }
}
