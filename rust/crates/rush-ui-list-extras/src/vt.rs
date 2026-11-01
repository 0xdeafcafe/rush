//! A terminal screen in memory: what an agent's own program draws, written
//! in as the bytes it sent, read back as rows of styled text. Only the
//! visible screen is kept (no scrollback), as Go's preview keeps it
//! (live.go:113 vt.NewEmulator, SetScrollbackSize(0); :210 lines, the
//! session's cursor drawn in reverse). Go uses charmbracelet's x/vt; this
//! is the part of a terminal a session's screen uses: text and wide
//! characters, cursor moves, erasing, inserting and deleting, scrolling
//! within a region, the alternate screen, colours and attributes, and the
//! answers to the queries it asks (cursor position, device status and
//! attributes), which go back up the stream.

use rush_core::text::{Color, Line, Mods, Span, Style};

/// One cell: the text drawn there (a character and any marks on it), how
/// wide it is (0 for the right half of a wide one), and how it looks.
#[derive(Clone, Debug, PartialEq)]
struct Cell {
    text: String,
    width: u8,
    style: Style,
}

impl Cell {
    fn blank(style: Style) -> Cell {
        Cell {
            text: " ".into(),
            width: 1,
            style: Style {
                bg: style.bg,
                ..Style::PLAIN
            },
        }
    }
}

/// Where the parser is in an escape sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    /// ESC followed by an intermediate byte ( ) # % …: one more byte ends it.
    EscInter,
    Csi,
    /// A string (OSC, DCS, APC, PM, SOS) read to its end and let go.
    Str,
    /// ESC inside a string: \ ends it.
    StrEsc,
}

/// The saved cursor (DECSC).
#[derive(Clone, Copy, Debug, Default)]
struct Saved {
    x: usize,
    y: usize,
    style: Style,
}

/// A terminal screen w by h.
pub struct Vt {
    w: usize,
    h: usize,
    grid: Vec<Vec<Cell>>,
    /// The main screen while the alternate one shows.
    main: Option<Vec<Vec<Cell>>>,
    x: usize,
    y: usize,
    /// The last column was written: the next character wraps first.
    wrap_next: bool,
    style: Style,
    saved: Saved,
    /// The scroll region's top and bottom rows, inclusive.
    top: usize,
    bottom: usize,
    autowrap: bool,
    /// The session shows its cursor.
    pub cursor: bool,
    state: State,
    params: String,
    private: bool,
    /// A UTF-8 character cut by the end of a write.
    utf8: Vec<u8>,
    /// What the session asked of its terminal, answered.
    answers: Vec<u8>,
    /// Written to since last read.
    dirty: bool,
}

impl Vt {
    pub fn new(w: usize, h: usize) -> Vt {
        let (w, h) = (w.max(1), h.max(1));
        Vt {
            w,
            h,
            grid: vec![vec![Cell::blank(Style::PLAIN); w]; h],
            main: None,
            x: 0,
            y: 0,
            wrap_next: false,
            style: Style::PLAIN,
            saved: Saved::default(),
            top: 0,
            bottom: h - 1,
            autowrap: true,
            cursor: true,
            state: State::Ground,
            params: String::new(),
            private: false,
            utf8: Vec::new(),
            answers: Vec::new(),
            dirty: true,
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// Whether it was written to since the last `take_dirty`.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The answers to the session's queries since last taken: they go
    /// back up the stream (live.go:132).
    pub fn take_answers(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.answers)
    }

    /// Makes it w by h: rows cut or added at the bottom, columns at the
    /// right, the cursor kept inside.
    pub fn resize(&mut self, w: usize, h: usize) {
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) == (self.w, self.h) {
            return;
        }
        let fit = |g: &mut Vec<Vec<Cell>>| {
            g.resize(h, vec![Cell::blank(Style::PLAIN); w]);
            for r in g.iter_mut() {
                r.resize(w, Cell::blank(Style::PLAIN));
            }
        };
        fit(&mut self.grid);
        if let Some(g) = self.main.as_mut() {
            fit(g);
        }
        (self.w, self.h) = (w, h);
        (self.top, self.bottom) = (0, h - 1);
        self.x = self.x.min(w - 1);
        self.y = self.y.min(h - 1);
        self.wrap_next = false;
        self.dirty = true;
    }

    /// Takes what the session wrote.
    pub fn write(&mut self, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        self.dirty = true;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if !self.utf8.is_empty() || c >= 0x80 && self.state == State::Ground {
                self.utf8.push(c);
                i += 1;
                match std::str::from_utf8(&self.utf8) {
                    Ok(s) => {
                        let s = s.to_string();
                        self.utf8.clear();
                        for ch in s.chars() {
                            self.print(ch);
                        }
                    }
                    Err(e) if e.error_len().is_some() || self.utf8.len() >= 4 => {
                        self.utf8.clear();
                        self.print('\u{fffd}');
                    }
                    Err(_) => {} // more to come
                }
                continue;
            }
            self.byte(c);
            i += 1;
        }
    }

    fn byte(&mut self, c: u8) {
        match self.state {
            State::Ground => match c {
                0x1b => self.state = State::Esc,
                b'\r' => self.carriage(),
                b'\n' | 0x0b | 0x0c => self.linefeed(),
                0x08 => {
                    self.x = self.x.saturating_sub(1);
                    self.wrap_next = false;
                }
                b'\t' => {
                    self.x = ((self.x / 8 + 1) * 8).min(self.w - 1);
                    self.wrap_next = false;
                }
                0x20..=0x7e => self.print(c as char),
                _ => {} // BEL, SO, SI and the rest: nothing to draw
            },
            State::Esc => self.escape(c),
            State::EscInter => self.state = State::Ground,
            State::Csi => match c {
                0x30..=0x3f => {
                    if c == b'?' || c == b'>' || c == b'<' || c == b'=' {
                        self.private |= c == b'?';
                        if c != b'?' {
                            self.params.push(c as char);
                        }
                    } else {
                        self.params.push(c as char);
                    }
                }
                0x20..=0x2f => self.params.push(c as char),
                0x40..=0x7e => {
                    self.state = State::Ground;
                    self.csi(c);
                }
                0x1b => self.state = State::Esc,
                _ => {}
            },
            State::Str => match c {
                0x07 => self.state = State::Ground,
                0x1b => self.state = State::StrEsc,
                _ => {}
            },
            State::StrEsc => {
                self.state = if c == b'\\' {
                    State::Ground
                } else {
                    State::Str
                }
            }
        }
    }

    fn escape(&mut self, c: u8) {
        self.state = State::Ground;
        match c {
            b'[' => {
                self.state = State::Csi;
                self.params.clear();
                self.private = false;
            }
            b']' | b'P' | b'_' | b'^' | b'X' => self.state = State::Str,
            b'(' | b')' | b'*' | b'+' | b'#' | b'%' | b' ' => self.state = State::EscInter,
            b'7' => {
                self.saved = Saved {
                    x: self.x,
                    y: self.y,
                    style: self.style,
                }
            }
            b'8' => self.restore(),
            b'D' => self.linefeed(),
            b'E' => {
                self.carriage();
                self.linefeed();
            }
            b'M' => self.reverse_index(),
            b'c' => *self = Vt::new(self.w, self.h),
            _ => {}
        }
    }

    fn restore(&mut self) {
        let s = self.saved;
        (self.x, self.y) = (s.x.min(self.w - 1), s.y.min(self.h - 1));
        self.style = s.style;
        self.wrap_next = false;
    }

    fn carriage(&mut self) {
        self.x = 0;
        self.wrap_next = false;
    }

    fn linefeed(&mut self) {
        self.wrap_next = false;
        if self.y == self.bottom {
            self.scroll_up(1);
        } else if self.y + 1 < self.h {
            self.y += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.wrap_next = false;
        if self.y == self.top {
            self.scroll_down(1);
        } else {
            self.y = self.y.saturating_sub(1);
        }
    }

    fn blank_row(&self) -> Vec<Cell> {
        vec![Cell::blank(self.style); self.w]
    }

    /// The region's rows move up n, blanks coming in at its bottom.
    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom + 1 - self.top);
        for _ in 0..n {
            self.grid.remove(self.top);
            let b = self.blank_row();
            self.grid.insert(self.bottom, b);
        }
    }

    /// The region's rows move down n, blanks coming in at its top.
    fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.bottom + 1 - self.top);
        for _ in 0..n {
            self.grid.remove(self.bottom);
            let b = self.blank_row();
            self.grid.insert(self.top, b);
        }
    }

    fn print(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        let w = rush_core::text::width(ch.encode_utf8(&mut buf));
        if w == 0 {
            // A mark on the character before.
            let x = if self.wrap_next {
                self.x
            } else {
                self.x.saturating_sub(1)
            };
            let row = &mut self.grid[self.y];
            let mut x = x;
            while x > 0 && row[x].width == 0 {
                x -= 1;
            }
            row[x].text.push(ch);
            return;
        }
        if self.wrap_next || self.x + w > self.w {
            if self.autowrap {
                self.carriage();
                self.linefeed();
            } else {
                self.x = self.w.saturating_sub(w);
            }
        }
        let (x, y) = (self.x, self.y);
        self.clear_wide(x, y);
        if w == 2 && x + 1 < self.w {
            self.clear_wide(x + 1, y);
        }
        let style = self.style;
        let row = &mut self.grid[y];
        row[x] = Cell {
            text: ch.to_string(),
            width: w as u8,
            style,
        };
        if w == 2 && x + 1 < self.w {
            row[x + 1] = Cell {
                text: String::new(),
                width: 0,
                style,
            };
        }
        if x + w >= self.w {
            self.x = self.w - 1;
            self.wrap_next = true;
        } else {
            self.x = x + w;
        }
    }

    /// Writing over half a wide character blanks its other half.
    fn clear_wide(&mut self, x: usize, y: usize) {
        let row = &mut self.grid[y];
        if row[x].width == 0 && x > 0 {
            row[x - 1] = Cell::blank(row[x - 1].style);
        }
        if row[x].width == 2 && x + 1 < row.len() {
            row[x + 1] = Cell::blank(row[x + 1].style);
        }
    }

    fn nums(&self) -> Vec<usize> {
        self.params
            .split(';')
            .map(|p| {
                let p = p.split(':').next().unwrap_or("");
                p.trim_start_matches(['>', '<', '=']).parse().unwrap_or(0)
            })
            .collect()
    }

    fn csi(&mut self, f: u8) {
        let ps = self.nums();
        let n = |i: usize| ps.get(i).copied().filter(|&v| v > 0).unwrap_or(1);
        let p0 = ps.first().copied().unwrap_or(0);
        if self.params.contains(['>', '<', '=']) || self.params.ends_with([' ', '$', '"', '!']) {
            // Answers about the terminal's kind and settings, cursor shapes
            // and the like: nothing a drawn screen shows.
            if f == b'c' && self.params.starts_with('>') {
                self.answers.extend_from_slice(b"\x1b[>0;0;0c");
            }
            return;
        }
        if self.private {
            if f == b'h' || f == b'l' {
                let on = f == b'h';
                for p in ps {
                    self.mode(p, on);
                }
            }
            return;
        }
        let (w, h) = (self.w, self.h);
        match f {
            b'A' => self.y = self.y.saturating_sub(n(0)).max(self.within_top()),
            b'B' | b'e' => self.y = (self.y + n(0)).min(self.within_bottom()),
            b'C' | b'a' => self.x = (self.x + n(0)).min(w - 1),
            b'D' => self.x = self.x.saturating_sub(n(0)),
            b'E' => {
                self.y = (self.y + n(0)).min(self.within_bottom());
                self.x = 0;
            }
            b'F' => {
                self.y = self.y.saturating_sub(n(0)).max(self.within_top());
                self.x = 0;
            }
            b'G' | b'`' => self.x = (n(0) - 1).min(w - 1),
            b'd' => self.y = (n(0) - 1).min(h - 1),
            b'H' | b'f' => {
                self.y = (n(0) - 1).min(h - 1);
                self.x = (n(1) - 1).min(w - 1);
            }
            b'J' => self.erase_display(p0),
            b'K' => self.erase_line(p0),
            b'X' => {
                let (x, y, st) = (self.x, self.y, self.style);
                for c in self.grid[y].iter_mut().skip(x).take(n(0)) {
                    *c = Cell::blank(st);
                }
            }
            b'@' => {
                let (x, y, st) = (self.x, self.y, self.style);
                let row = &mut self.grid[y];
                for _ in 0..n(0).min(w - x) {
                    row.pop();
                    row.insert(x, Cell::blank(st));
                }
            }
            b'P' => {
                let (x, y, st) = (self.x, self.y, self.style);
                let row = &mut self.grid[y];
                for _ in 0..n(0).min(w - x) {
                    row.remove(x);
                    row.push(Cell::blank(st));
                }
            }
            b'L' if (self.top..=self.bottom).contains(&self.y) => {
                let top = std::mem::replace(&mut self.top, self.y);
                self.scroll_down(n(0));
                self.top = top;
                self.x = 0;
            }
            b'M' if (self.top..=self.bottom).contains(&self.y) => {
                let top = std::mem::replace(&mut self.top, self.y);
                self.scroll_up(n(0));
                self.top = top;
                self.x = 0;
            }
            b'S' => self.scroll_up(n(0)),
            b'T' => self.scroll_down(n(0)),
            b'b' => {
                if let Some(c) = self.grid[self.y]
                    .get(self.x.saturating_sub(1))
                    .and_then(|c| c.text.chars().next())
                {
                    for _ in 0..n(0).min(w * h) {
                        self.print(c);
                    }
                }
            }
            b'm' => self.sgr(),
            b'r' => {
                let top = n(0) - 1;
                let bottom = ps.get(1).copied().filter(|&v| v > 0).unwrap_or(h) - 1;
                if top < bottom && bottom < h {
                    (self.top, self.bottom) = (top, bottom);
                    (self.x, self.y) = (0, 0);
                }
            }
            b's' => {
                self.saved = Saved {
                    x: self.x,
                    y: self.y,
                    style: self.style,
                }
            }
            b'u' => self.restore(),
            b'n' => match p0 {
                5 => self.answers.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let a = format!("\x1b[{};{}R", self.y + 1, self.x + 1);
                    self.answers.extend_from_slice(a.as_bytes());
                }
                _ => {}
            },
            b'c' => self.answers.extend_from_slice(b"\x1b[?62;22c"),
            _ => {}
        }
        if f != b'm' && f != b'n' && f != b'c' {
            self.wrap_next = false;
        }
    }

    fn within_top(&self) -> usize {
        if self.y >= self.top { self.top } else { 0 }
    }

    fn within_bottom(&self) -> usize {
        if self.y <= self.bottom {
            self.bottom
        } else {
            self.h - 1
        }
    }

    fn mode(&mut self, p: usize, on: bool) {
        match p {
            7 => self.autowrap = on,
            25 => self.cursor = on,
            47 | 1047 | 1049 => {
                if on == self.main.is_some() {
                    return;
                }
                if on {
                    if p == 1049 {
                        self.escape(b'7');
                    }
                    let alt = vec![vec![Cell::blank(Style::PLAIN); self.w]; self.h];
                    self.main = Some(std::mem::replace(&mut self.grid, alt));
                } else {
                    if let Some(g) = self.main.take() {
                        self.grid = g;
                    }
                    if p == 1049 {
                        self.restore();
                    }
                }
            }
            _ => {}
        }
    }

    fn erase_display(&mut self, p: usize) {
        let st = self.style;
        let (x, y) = (self.x, self.y);
        match p {
            0 => {
                self.erase_line(0);
                for r in &mut self.grid[y + 1..] {
                    r.fill(Cell::blank(st));
                }
            }
            1 => {
                self.erase_line(1);
                for r in &mut self.grid[..y] {
                    r.fill(Cell::blank(st));
                }
            }
            2 | 3 => {
                for r in &mut self.grid {
                    r.fill(Cell::blank(st));
                }
            }
            _ => {}
        }
        (self.x, self.y) = (x, y);
    }

    fn erase_line(&mut self, p: usize) {
        let st = self.style;
        let (x, y) = (self.x, self.y);
        let row = &mut self.grid[y];
        let (from, to) = match p {
            0 => (x, row.len()),
            1 => (0, (x + 1).min(row.len())),
            2 => (0, row.len()),
            _ => return,
        };
        row[from..to].fill(Cell::blank(st));
    }

    fn sgr(&mut self) {
        let raw = self.params.clone();
        if raw.is_empty() {
            self.style = Style::PLAIN;
            return;
        }
        // Each parameter, its colon-joined parts as one group.
        let groups: Vec<Vec<usize>> = raw
            .split(';')
            .map(|g| g.split(':').map(|p| p.parse().unwrap_or(0)).collect())
            .collect();
        let mut i = 0;
        while i < groups.len() {
            let g = &groups[i];
            let p = g[0];
            let s = &mut self.style;
            match p {
                0 => *s = Style::PLAIN,
                1 => s.mods = s.mods.with(Mods::BOLD),
                2 => s.mods = s.mods.with(Mods::DIM),
                3 => s.mods = s.mods.with(Mods::ITALIC),
                4 => s.mods = s.mods.with(Mods::UNDERLINE),
                7 => s.mods = s.mods.with(Mods::REVERSE),
                9 => s.mods = s.mods.with(Mods::STRIKE),
                22 => s.mods = without(s.mods, Mods(Mods::BOLD.0 | Mods::DIM.0)),
                23 => s.mods = without(s.mods, Mods::ITALIC),
                24 => s.mods = without(s.mods, Mods::UNDERLINE),
                27 => s.mods = without(s.mods, Mods::REVERSE),
                29 => s.mods = without(s.mods, Mods::STRIKE),
                30..=37 => s.fg = Color::Indexed((p - 30) as u8),
                39 => s.fg = Color::Default,
                40..=47 => s.bg = Color::Indexed((p - 40) as u8),
                49 => s.bg = Color::Default,
                90..=97 => s.fg = Color::Indexed((p - 90 + 8) as u8),
                100..=107 => s.bg = Color::Indexed((p - 100 + 8) as u8),
                38 | 48 | 58 => {
                    // 38;5;n or 38;2;r;g;b, or the same joined by colons.
                    let (c, used) = if g.len() > 1 {
                        (extended(&g[1..]), 0)
                    } else {
                        let rest: Vec<usize> = groups[i + 1..].iter().map(|g| g[0]).collect();
                        let used = match rest.first() {
                            Some(5) => 2,
                            Some(2) => 4,
                            _ => rest.len(),
                        };
                        (extended(&rest), used.min(rest.len()))
                    };
                    i += used;
                    match (p, c) {
                        (38, Some(c)) => s.fg = c,
                        (48, Some(c)) => s.bg = c,
                        _ => {}
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// The screen as rows of styled text, the session's cursor drawn in
    /// reverse where it shows one (live.go:210 lines).
    pub fn lines(&self) -> Vec<Line> {
        let mut out = Vec::with_capacity(self.h);
        for (y, row) in self.grid.iter().enumerate() {
            let mut l = Line::new();
            for (x, c) in row.iter().enumerate() {
                if c.width == 0 {
                    continue;
                }
                let mut st = c.style;
                if self.cursor && (x, y) == (self.x, self.y) {
                    st.mods = st.mods.with(Mods::REVERSE);
                }
                match l.spans.last_mut() {
                    Some(sp) if sp.style == st => sp.text.push_str(&c.text),
                    _ => l.spans.push(Span::new(c.text.clone(), st)),
                }
            }
            out.push(l);
        }
        out
    }
}

fn without(m: Mods, o: Mods) -> Mods {
    Mods(m.0 & !o.0)
}

/// An extended colour's parameters after 38 or 48: 5;n or 2;r;g;b (a
/// colour space id may come before r when joined by colons).
fn extended(p: &[usize]) -> Option<Color> {
    match p.first()? {
        5 => Some(Color::Indexed(*p.get(1)? as u8)),
        2 => {
            let rgb = if p.len() >= 5 { &p[2..5] } else { p.get(1..4)? };
            Some(Color::Rgb(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &Vt) -> Vec<String> {
        v.lines()
            .iter()
            .map(|l| l.plain().trim_end().to_string())
            .collect()
    }

    #[test]
    fn text_moves_and_erases() {
        let mut v = Vt::new(10, 3);
        v.write(b"hello\r\nworld");
        assert_eq!(text(&v), ["hello", "world", ""]);
        v.write(b"\x1b[1;3HX\x1b[2;1H\x1b[K\x1b[3;2Hok");
        assert_eq!(text(&v), ["heXlo", "", " ok"]);
        v.write(b"\x1b[2J");
        assert_eq!(text(&v), ["", "", ""]);
    }

    #[test]
    fn wraps_and_scrolls() {
        let mut v = Vt::new(4, 2);
        v.write(b"abcdefgh");
        assert_eq!(text(&v), ["abcd", "efgh"]);
        v.write(b"\r\nij");
        assert_eq!(text(&v), ["efgh", "ij"]);
        // Wide characters take two cells; a cut UTF-8 character waits.
        let mut v = Vt::new(6, 1);
        let s = "日本".as_bytes();
        v.write(&s[..4]);
        v.write(&s[4..]);
        assert_eq!(text(&v), ["日本"]);
    }

    #[test]
    fn colours_and_the_cursor() {
        let mut v = Vt::new(8, 1);
        v.write(b"\x1b[1;38;2;217;119;87mhi\x1b[0m \x1b[38;5;3mx\x1b[?25l");
        let l = &v.lines()[0];
        assert_eq!(l.spans[0].text, "hi");
        assert_eq!(l.spans[0].style.fg, Color::Rgb(217, 119, 87));
        assert!(l.spans[0].style.mods.contains(Mods::BOLD));
        assert_eq!(l.spans[2].style.fg, Color::Indexed(3));
        v.write(b"\x1b[?25h");
        let l = &v.lines()[0];
        assert!(
            l.spans
                .iter()
                .any(|s| s.text == " " && s.style.mods.contains(Mods::REVERSE))
        );
    }

    #[test]
    fn answers_its_queries() {
        let mut v = Vt::new(8, 4);
        v.write(b"\x1b[2;3H\x1b[6n\x1b[c");
        assert_eq!(v.take_answers(), b"\x1b[2;3R\x1b[?62;22c");
        assert!(v.take_answers().is_empty());
    }

    #[test]
    fn the_alternate_screen_and_strings() {
        let mut v = Vt::new(6, 2);
        v.write(b"main\x1b[?1049h\x1b[Halt\x1b]0;title\x07");
        assert_eq!(text(&v), ["alt", ""]);
        v.write(b"\x1b[?1049l");
        assert_eq!(text(&v), ["main", ""]);
        v.write(b"\x1b_cc-daemon-hint\x1b\\!");
        assert_eq!(text(&v), ["main!", ""]);
    }

    #[test]
    fn a_scroll_region() {
        let mut v = Vt::new(3, 4);
        v.write(b"a\r\nb\r\nc\r\nd\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(text(&v), ["a", "c", "", "d"]);
        v.write(b"\x1b[r\x1b[1;1H\x1b[L");
        assert_eq!(text(&v), ["", "a", "c", ""]);
    }
}
