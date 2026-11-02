//! Drawing without a terminal: for tests and `rush --render WxH`, which in
//! Go prints one frame (cmd/rush/main.go:479 `render`). Go prints the
//! view's string as it is, OSC 8 links and all, so [`Headless::ansi`]
//! writes them too.

use std::fmt::Write as _;

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect as RRect;
use ratatui::style::{Color as RColor, Modifier};
use rush_core::text::{self, Link};

use crate::canvas::{Canvas, row_text};

/// A screen in memory.
pub struct Headless {
    buf: Buffer,
    links: Vec<Link>,
    cursor: Option<(u16, u16)>,
}

impl Headless {
    pub fn new(w: u16, h: u16) -> Headless {
        Headless {
            buf: Buffer::empty(RRect::new(0, 0, w, h)),
            links: vec![Link::NONE; w as usize * h as usize],
            cursor: None,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.buf.area.width, self.buf.area.height)
    }

    /// Draws a frame on a blank screen, as [`crate::Term::draw`] does.
    pub fn draw(&mut self, f: impl FnOnce(&mut Canvas)) {
        self.buf.reset();
        self.links.fill(Link::NONE);
        let mut c = Canvas::new(&mut self.buf, &mut self.links);
        f(&mut c);
        self.cursor = c.cursor();
    }

    /// Where the frame left the cursor, if it showed it.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        self.cursor
    }

    /// The frame as text: a line per row, trailing spaces dropped.
    pub fn plain(&self) -> String {
        let mut out = String::new();
        for y in 0..self.buf.area.height {
            if y > 0 {
                out.push('\n');
            }
            out.push_str(row_text(&self.buf, y).trim_end_matches(' '));
        }
        out
    }

    /// The link drawn at (x, y); NONE where there's none.
    pub fn link_at(&self, x: u16, y: u16) -> Link {
        let (w, h) = self.size();
        if x >= w || y >= h {
            return Link::NONE;
        }
        self.links[y as usize * w as usize + x as usize]
    }

    /// The frame as ANSI-styled text: SGR codes where the style changes,
    /// reset at each row's end; trailing unstyled blanks dropped. A linked
    /// run is wrapped in OSC 8 (`ESC ]8;;url ESC \` … `ESC ]8;; ESC \`),
    /// closed by the row's end.
    pub fn ansi(&self) -> String {
        let w = self.buf.area.width as usize;
        let mut out = String::new();
        for y in 0..self.buf.area.height as usize {
            if y > 0 {
                out.push('\n');
            }
            let row = &self.buf.content[y * w..(y + 1) * w];
            let links = &self.links[y * w..(y + 1) * w];
            let end = row
                .iter()
                .zip(links)
                .rposition(|(c, l)| !(c.symbol() == " " && plain_cell(c) && l.is_none()))
                .map_or(0, |i| i + 1);
            let mut cur: Option<(RColor, RColor, Modifier)> = None;
            let mut link = Link::NONE;
            let mut skip = 0;
            for (c, &l) in row[..end].iter().zip(links) {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                if l != link {
                    osc8(&mut out, l);
                    link = l;
                }
                let st = (c.fg, c.bg, c.modifier);
                if cur != Some(st) {
                    sgr(&mut out, st);
                    cur = Some(st);
                }
                out.push_str(c.symbol());
                skip = text::width(c.symbol()).saturating_sub(1);
            }
            if cur.is_some_and(|(f, b, m)| {
                !(f == RColor::Reset && b == RColor::Reset && m.is_empty())
            }) {
                out.push_str("\x1b[0m");
            }
            if !link.is_none() {
                osc8(&mut out, Link::NONE);
            }
        }
        out
    }
}

/// Writes the OSC 8 that opens `l`, or closes the open one for NONE.
/// Control characters in a URL are left out: they would end it early.
pub(crate) fn osc8(out: &mut String, l: Link) {
    out.push_str("\x1b]8;;");
    if let Some(u) = l.url() {
        out.extend(u.chars().filter(|c| !c.is_control()));
    }
    out.push_str("\x1b\\");
}

fn plain_cell(c: &Cell) -> bool {
    c.fg == RColor::Reset && c.bg == RColor::Reset && c.modifier.is_empty()
}

// Writes the SGR sequence for a style, starting from a reset.
fn sgr(out: &mut String, (fg, bg, m): (RColor, RColor, Modifier)) {
    out.push_str("\x1b[0");
    for (bit, code) in [
        (Modifier::BOLD, 1),
        (Modifier::DIM, 2),
        (Modifier::ITALIC, 3),
        (Modifier::UNDERLINED, 4),
        (Modifier::REVERSED, 7),
        (Modifier::CROSSED_OUT, 9),
    ] {
        if m.contains(bit) {
            let _ = write!(out, ";{code}");
        }
    }
    colour(out, fg, 30, 90, 38);
    colour(out, bg, 40, 100, 48);
    out.push('m');
}

fn colour(out: &mut String, c: RColor, base: u8, bright: u8, ext: u8) {
    let _ = match c {
        RColor::Rgb(r, g, b) => write!(out, ";{ext};2;{r};{g};{b}"),
        RColor::Indexed(i) if i < 8 => write!(out, ";{}", base + i),
        RColor::Indexed(i) if i < 16 => write!(out, ";{}", bright + i - 8),
        RColor::Indexed(i) => write!(out, ";{ext};5;{i}"),
        // rush draws only default, RGB and indexed colours.
        _ => Ok(()),
    };
}
