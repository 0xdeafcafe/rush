//! A frame being drawn: styled lines put on a grid of cells, clipped by
//! display width. Stands in for what lipgloss/ultraviolet do for Go rush
//! (internal/ui/view.go:25 builds the frame as a string; here it is cells).
//! Each cell keeps the link its text carries ([`Link`]), for the terminal
//! to be written OSC 8 around it as Go's rows carry it (ui/finish.go:400
//! link, convo/render.go:1891).

use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color as RColor, Modifier};
use rush_core::text::{self, Color, Line, Link, Mods, Span, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// A rectangle of cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, w, h }
    }
    pub const fn right(self) -> u16 {
        self.x.saturating_add(self.w)
    }
    pub const fn bottom(self) -> u16 {
        self.y.saturating_add(self.h)
    }
    pub const fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }
    /// Whether the cell at (x, y) is inside.
    pub const fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    /// The part both share; empty when they don't meet.
    pub fn intersect(self, o: Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        if r <= x || b <= y {
            return Rect::new(x, y, 0, 0);
        }
        Rect::new(x, y, r - x, b - y)
    }
}

/// A frame being drawn. Everything outside it is clipped.
pub struct Canvas<'a> {
    buf: &'a mut Buffer,
    /// The link of each cell, row by row as `buf`'s.
    links: &'a mut [Link],
    w: u16,
    h: u16,
    cursor: Option<(u16, u16)>,
    /// The last frame is on screen as it was drawn ([`Canvas::can_keep`]).
    keepable: bool,
    kept: bool,
}

impl<'a> Canvas<'a> {
    pub(crate) fn new(buf: &'a mut Buffer, links: &'a mut [Link]) -> Canvas<'a> {
        let (w, h) = (buf.area.width, buf.area.height);
        debug_assert_eq!(links.len(), buf.content.len());
        Canvas {
            buf,
            links,
            w,
            h,
            cursor: None,
            keepable: false,
            kept: false,
        }
    }

    pub(crate) fn keepable(mut self, k: bool) -> Self {
        self.keepable = k;
        self
    }

    pub(crate) fn cursor(&self) -> Option<(u16, u16)> {
        self.cursor
    }

    /// Whether the frame drawn last is on the screen as it was drawn
    /// (nothing cleared or resized since), so [`Canvas::keep`] can stand
    /// for drawing it again. Never on a screen in memory.
    pub fn can_keep(&self) -> bool {
        self.keepable
    }

    /// This frame is the last one again, cell for cell and cursor: nothing
    /// is painted and nothing written. Only when [`Canvas::can_keep`].
    pub fn keep(&mut self) {
        self.kept = self.keepable;
    }

    pub(crate) fn kept(&self) -> bool {
        self.kept
    }

    pub fn width(&self) -> u16 {
        self.w
    }
    pub fn height(&self) -> u16 {
        self.h
    }
    pub fn area(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    /// Shows the cursor at (x, y) once the frame is drawn; with none set
    /// it stays hidden.
    pub fn set_cursor(&mut self, x: u16, y: u16) {
        self.cursor = Some((x, y));
    }

    /// Paints every cell of `r` blank in `style`.
    pub fn fill(&mut self, r: Rect, style: Style) {
        let r = r.intersect(self.area());
        if r.is_empty() {
            return;
        }
        let (fg, bg, m) = (color(style.fg), color(style.bg), mods(style.mods));
        for y in r.y..r.bottom() {
            self.unsplit(r.x, y);
            for x in r.x..r.right() {
                self.set_link(x, y, Link::NONE);
                let c = self.cell(x, y);
                c.set_symbol(" ");
                c.fg = fg;
                c.bg = bg;
                c.modifier = m;
            }
        }
    }

    /// Puts `line` at (x, y), at most `max_width` cells wide and never past
    /// the right edge. A wide char that doesn't fit whole is left out. Spans
    /// without a background take the line's; the line's background, when
    /// set, fills the rest of `max_width`. Returns the cells written.
    pub fn put_line(&mut self, x: u16, y: u16, line: &Line, max_width: u16) -> u16 {
        if y >= self.h || x >= self.w {
            return 0;
        }
        let end = x.saturating_add(max_width).min(self.w);
        let mut at = x;
        for s in &line.spans {
            let mut st = s.style;
            if st.bg == Color::Default {
                st.bg = line.bg;
            }
            at = self.put(at, y, &s.text, st, end);
            if at >= end {
                break;
            }
        }
        if line.bg != Color::Default && at < end {
            self.fill(Rect::new(at, y, end - at, 1), Style::default().on(line.bg));
            at = end;
        }
        at - x
    }

    /// Puts one run of text in one style; see [`Canvas::put_line`].
    pub fn put_str(&mut self, x: u16, y: u16, s: &str, style: Style, max_width: u16) -> u16 {
        if y >= self.h || x >= self.w {
            return 0;
        }
        let end = x.saturating_add(max_width).min(self.w);
        self.put(x, y, s, style, end) - x
    }

    /// Puts spans one after another; see [`Canvas::put_line`].
    pub fn put_spans(&mut self, x: u16, y: u16, spans: &[Span], max_width: u16) -> u16 {
        if y >= self.h || x >= self.w {
            return 0;
        }
        let end = x.saturating_add(max_width).min(self.w);
        let mut at = x;
        for s in spans {
            at = self.put(at, y, &s.text, s.style, end);
            if at >= end {
                break;
            }
        }
        at - x
    }

    /// The text of row `y` without styles (wide chars once).
    pub fn row_text(&self, y: u16) -> String {
        row_text(self.buf, y)
    }

    // Writes graphemes from x up to (not past) end; returns where it stopped.
    fn put(&mut self, mut x: u16, y: u16, s: &str, style: Style, end: u16) -> u16 {
        let (fg, bg, m) = (color(style.fg), color(style.bg), mods(style.mods));
        if s.is_ascii() {
            // Each ASCII byte is a grapheme of its own (CR LF, the one
            // pair, is controls, which take no cell): no segmenting.
            for (i, b) in s.bytes().enumerate() {
                if !(0x20..0x7f).contains(&b) {
                    continue;
                }
                if x >= end {
                    break;
                }
                self.unsplit(x, y);
                self.set_link(x, y, style.link);
                let c = self.cell(x, y);
                c.set_symbol(&s[i..i + 1]);
                c.fg = fg;
                c.bg = bg;
                c.modifier = m;
                x += 1;
            }
            return x;
        }
        if s.chars().all(lone) {
            // Each char a grapheme of its own: no segmenting, and its
            // width the char's own (the same: tests.rs).
            let chars = s
                .char_indices()
                .map(|(i, c)| (&s[i..i + c.len_utf8()], c.width().unwrap_or(0)));
            return self.put_graphemes(x, y, chars, style, end);
        }
        let gs = s.graphemes(true).map(|g| {
            // Fast path for ASCII; controls (tabs, newlines) take no cell.
            let w = match g.as_bytes() {
                [b] if (0x20..0x7f).contains(b) => 1,
                _ if g.chars().any(char::is_control) => 0,
                _ => text::width(g),
            };
            (g, w)
        });
        self.put_graphemes(x, y, gs, style, end)
    }

    /// Writes graphemes, each with the cells it takes, from x up to (not
    /// past) end; returns where it stopped.
    fn put_graphemes<'s>(
        &mut self,
        mut x: u16,
        y: u16,
        gs: impl Iterator<Item = (&'s str, usize)>,
        style: Style,
        end: u16,
    ) -> u16 {
        let (fg, bg, m) = (color(style.fg), color(style.bg), mods(style.mods));
        for (g, w) in gs {
            if w == 0 {
                continue;
            }
            let w = w.min(u16::MAX as usize) as u16;
            if x.saturating_add(w) > end {
                break;
            }
            self.unsplit(x, y);
            self.set_link(x, y, style.link);
            let c = self.cell(x, y);
            c.set_symbol(g);
            c.fg = fg;
            c.bg = bg;
            c.modifier = m;
            // The cells a wide char covers are blank, in its style, so a
            // background reads as one run.
            for i in 1..w {
                self.set_link(x + i, y, style.link);
                let c = self.cell(x + i, y);
                c.set_symbol(" ");
                c.fg = fg;
                c.bg = bg;
                c.modifier = m;
            }
            x += w;
        }
        x
    }

    // Before writing at x: a wide char just left of x covers it and would
    // be cut in half, so it becomes a blank.
    fn unsplit(&mut self, x: u16, y: u16) {
        if x == 0 || x >= self.w {
            return;
        }
        let c = self.cell(x - 1, y);
        // One byte is one cell at most: only a longer symbol can be wide.
        if c.symbol().len() > 1 && text::width(c.symbol()) > 1 {
            c.set_symbol(" ");
        }
    }

    /// The link the text at (x, y) carries; NONE outside the frame.
    pub fn link_at(&self, x: u16, y: u16) -> Link {
        if x >= self.w || y >= self.h {
            return Link::NONE;
        }
        self.links[y as usize * self.w as usize + x as usize]
    }

    fn set_link(&mut self, x: u16, y: u16, l: Link) {
        self.links[y as usize * self.w as usize + x as usize] = l;
    }

    fn cell(&mut self, x: u16, y: u16) -> &mut Cell {
        let i = y as usize * self.w as usize + x as usize;
        &mut self.buf.content[i]
    }
}

pub(crate) fn row_text(buf: &Buffer, y: u16) -> String {
    let w = buf.area.width as usize;
    let row = &buf.content[y as usize * w..(y as usize + 1) * w];
    let mut out = String::with_capacity(w);
    let mut skip = 0;
    for c in row {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        out.push_str(c.symbol());
        skip = text::width(c.symbol()).saturating_sub(1);
    }
    out
}

pub(crate) const fn color(c: Color) -> RColor {
    match c {
        Color::Default => RColor::Reset,
        Color::Rgb(r, g, b) => RColor::Rgb(r, g, b),
        Color::Indexed(i) => RColor::Indexed(i),
    }
}

pub(crate) fn mods(m: Mods) -> Modifier {
    let mut o = Modifier::empty();
    for (from, to) in [
        (Mods::BOLD, Modifier::BOLD),
        (Mods::DIM, Modifier::DIM),
        (Mods::ITALIC, Modifier::ITALIC),
        (Mods::UNDERLINE, Modifier::UNDERLINED),
        (Mods::REVERSE, Modifier::REVERSED),
        (Mods::STRIKE, Modifier::CROSSED_OUT),
    ] {
        if m.contains(from) {
            o |= to;
        }
    }
    o
}

/// Whether c is a grapheme of its own wherever it stands: it joins neither
/// the char before it (marks, joiners) nor the one after (prepended
/// signs), nor pairs (flags, Hangul, CR LF). These ranges hold none of
/// those: printable ASCII; Latin-1, Latin Extended and the spacing
/// modifiers (up to the combining marks at U+0300); general punctuation
/// but the zero-width and direction marks; super- and subscripts and
/// currency (up to the combining marks at U+20D0); letterlike symbols
/// through arrows, maths, technical symbols, box drawing, blocks, shapes,
/// dingbats and more arrows. Checked against the segmenter in tests.
pub(crate) fn lone(c: char) -> bool {
    matches!(
        c as u32,
        0x20..=0x7e | 0xa0..=0x2ff | 0x2010..=0x2027 | 0x2030..=0x205e | 0x2070..=0x20cf | 0x2100..=0x2bff
    )
}
