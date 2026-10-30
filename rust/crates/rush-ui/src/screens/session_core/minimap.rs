//! The conversation's map: the whole conversation in braille dots down
//! its right, coloured as its rows are, the part in view lit, its edge
//! bright where turns ended lately. A click or drag on it scrolls there.
//! After Go's internal/ui/minimap.go (miniTurn :26, prepareWindow :82,
//! turnKey :144, packMini :149, rowAt :158, miniGuess :186, layout :200,
//! miniAge :278, miniPixels :305, bounds :361, line :365, hit :373,
//! pressMinimap :377, dragMinimap :399, seekMinimap :416,
//! minimapEnabled :413, conversationWidth :418).
//!
//! It runs beside the conversation only: Go's runs on down beside the
//! message box too.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use rush_convo::{RenderedLine, Renderer, Session};
use rush_core::text::{Color, Line, Span, Style, width};
use rush_core::time::Ms;

use crate::session::{Pos, SessionView};
use crate::style::*;

pub const MINI_COLUMNS: usize = 10;
/// Its columns and the two before them: a blank and its edge.
pub const MINI_WIDTH: usize = MINI_COLUMNS + 2;
/// How many rows a dots row is averaged from at most.
const MINI_SAMPLES: usize = 4;
const PX: usize = MINI_COLUMNS * 2;

/// The terminal's own ground, which the map's dots sit on.
const GROUND: Color = Color::Rgb(17, 16, 14);

/// How much a colour is faded toward the ground (view.go:1322 fadeBy).
const FADE_BY: f64 = 0.3;

/// c faded toward the ground (fadeCode).
pub fn fade(c: Color) -> Color {
    mix(c, GROUND, FADE_BY)
}

/// The colours the map tells rows apart by; 0 is none.
fn palette() -> [Color; 7] {
    [Color::Default, SUB, BLUE, ORANGE, GREEN, RED, QUEUE]
}

/// Whether the map is drawn for a conversation w wide: from 96 wide
/// (minimapEnabled, conversationWidth).
pub fn enabled(w: usize) -> bool {
    w >= 96
}

/// What the map keeps of a turn as last drawn: a few of its rows'
/// miniatures, 3 bits a dot column, and enough to tell when it's drawn
/// differently.
#[derive(Default)]
struct MiniTurn {
    n: usize,
    sum: usize,
    ends: u64,
    rows: Vec<u64>,
}

/// One session's map.
#[derive(Default)]
pub struct Minimap {
    /// By turn ref ("t13").
    turns: HashMap<String, MiniTurn>,
    /// This frame's window, a turn at a time: its first row and its ref.
    groups: Vec<(usize, String)>,
    width: usize,
    pub height: usize,
    pub scale: usize,
    total: usize,
    normal: Vec<Line>,
    active: Vec<Line>,
    /// Where it was drawn on screen, and the view it shows.
    pub x: usize,
    pub y: usize,
    pub viewport: usize,
    pub start: usize,
    pub end: usize,
    pub visible: bool,
    pub dragging: bool,
    pub drag_y: usize,
    pub drag_start: usize,
    /// Rows decoded, for tests and regressions.
    pub parsed: u64,
}

/// "t13" for "t13:s:…".
fn turn_key(r: &str) -> &str {
    r.split(':').next().unwrap_or("")
}

fn pack(px: &[u8; PX]) -> u64 {
    px.iter()
        .enumerate()
        .fold(0, |v, (x, &c)| v | (u64::from(c & 7) << (3 * x)))
}

/// A row's miniature: which dot columns have ink, in which colour
/// (miniPixels).
fn pixels(l: &RenderedLine, w: usize) -> [u8; PX] {
    let pal = palette();
    let base = if is_turn_ref(&l.r#ref) {
        2
    } else if l.r#ref.contains(":s:") || l.r#ref.contains(":run:") {
        3
    } else {
        1
    };
    let mut px = [0u8; PX];
    let mut x = 0;
    let w = w.max(1);
    for s in &l.line.spans {
        let colour = pal[2..]
            .iter()
            .position(|c| *c == s.style.fg)
            .map_or(base, |i| i as u8 + 2);
        for ch in s.text.chars() {
            if x >= w {
                return px;
            }
            let cw = width(ch.encode_utf8(&mut [0; 4]));
            if cw == 0 {
                continue;
            }
            if !ch.is_whitespace() && !"│─┆┊".contains(ch) {
                for col in x..(x + cw).min(w) {
                    px[(col * PX / w).min(PX - 1)] = colour;
                }
            }
            x += cw;
        }
    }
    px
}

/// A turn's heading row ("t13"), not one of its parts.
fn is_turn_ref(r: &str) -> bool {
    r.len() > 1 && r.starts_with('t') && r[1..].bytes().all(|b| b.is_ascii_digit())
}

/// A turn never drawn: its heading, then prose (miniGuess).
fn guess(k: usize, off: usize) -> u64 {
    let (from, to, colour) = if off > 0 {
        let h = (k as u32).wrapping_mul(2654435761) ^ (off as u32).wrapping_mul(40503);
        (2, 8 + (h % 11) as usize, 1u64)
    } else {
        (1, 7, 2u64)
    };
    (from..to).fold(0, |v, x| v | colour << (3 * x))
}

impl Minimap {
    /// Takes in the rows drawn, [base, base+lines) of total, decoding only
    /// the turns drawn differently since, then lays the map out height
    /// rows tall (prepareWindow).
    pub fn prepare(&mut self, v: &mut SessionView, width_: usize, height: usize, now: Ms) {
        if self.width != width_ {
            self.turns.clear();
        }
        self.width = width_;
        self.height = height;
        let total = v.total();
        let scale = total.max(height * 4);
        let lines = v.r.rows();
        self.groups.clear();
        let mut a = 0;
        while a < lines.len() {
            let key = turn_key(&lines[a].r#ref).to_string();
            let mut b = a + 1;
            let mut sum = lines[a].line.plain().len();
            while b < lines.len() && (lines[b].r#ref.is_empty() || turn_key(&lines[b].r#ref) == key)
            {
                sum += lines[b].line.plain().len();
                b += 1;
            }
            let n = b - a;
            let mut h = DefaultHasher::new();
            lines[a].line.plain().hash(&mut h);
            lines[b - 1].line.plain().hash(&mut h);
            let ends = h.finish();
            let g = self.turns.entry(key.clone()).or_default();
            if g.n != n || g.sum != sum || g.ends != ends {
                let k = n.min(64).min((2 * n * height * 4 / scale + 1).max(4));
                g.n = n;
                g.sum = sum;
                g.ends = ends;
                g.rows.clear();
                for j in 0..k {
                    g.rows.push(pack(&pixels(lines[a + j * n / k], width_)));
                    self.parsed += 1;
                }
            }
            self.groups.push((a, key));
            a = b;
        }
        self.total = total;
        self.scale = scale;
        self.layout(&v.r, &v.sess, v.base, lines.len(), now);
    }

    /// Row r's miniature: as drawn in the window, else as kept of its
    /// turn, else a guess at a turn's shape (rowAt).
    fn row_at(&self, r: &Renderer, s: &Session, base: usize, n: usize, row: usize) -> u64 {
        if let Some(i) = row.checked_sub(base).filter(|&i| i < n) {
            let j = self.groups.partition_point(|g| g.0 <= i);
            let Some((at, key)) = j.checked_sub(1).and_then(|j| self.groups.get(j)) else {
                return 0;
            };
            let Some(t) = self.turns.get(key) else {
                return 0;
            };
            if t.rows.is_empty() {
                return 0;
            }
            return t.rows[((i - at) * t.rows.len() / t.n.max(1)).min(t.rows.len() - 1)];
        }
        if s.turns.is_empty() {
            return 0;
        }
        let k = r.turn_at(row).min(s.turns.len() - 1);
        let off = row.saturating_sub(r.turn_row(k));
        if let Some(t) = self.turns.get(&format!("t{}", s.turns[k].n))
            && !t.rows.is_empty()
        {
            return t.rows[(off * t.rows.len() / t.n.max(1)).min(t.rows.len() - 1)];
        }
        guess(k, off)
    }

    /// Every dots row drawn again, each averaged from a few of its rows,
    /// ordered dithering keeping the blanks at any scale (layout).
    fn layout(&mut self, r: &Renderer, s: &Session, base: usize, n: usize, now: Ms) {
        let height = self.height;
        let h4 = height * 4;
        let threshold: [[u32; 2]; 4] = [[1, 5], [7, 3], [4, 8], [6, 2]];
        let mut px = vec![[0u8; PX]; h4];
        for (y, row) in px.iter_mut().enumerate() {
            let mut counts = [[0u32; 8]; PX];
            let lo = y * self.scale / h4;
            let hi = self.total.min((y + 1) * self.scale / h4);
            let mut total = 0u32;
            let step = ((hi.saturating_sub(lo)) / MINI_SAMPLES).max(1);
            let mut at = lo;
            while at < hi {
                let p = self.row_at(r, s, base, n, at);
                total += 1;
                for (x, c) in counts.iter_mut().enumerate() {
                    c[(p >> (3 * x) & 7) as usize] += 1;
                }
                at += step;
            }
            for x in 0..PX {
                let ink = total - counts[x][0];
                if ink == 0 || ink * 8 < total * threshold[y % 4][x % 2] {
                    continue;
                }
                let mut colour = 1;
                for c in 2..7 {
                    if counts[x][c] > counts[x][colour] {
                        colour = c;
                    }
                }
                row[x] = colour as u8;
            }
        }
        let pal = palette();
        let ink: Vec<Color> = pal.iter().map(|&c| fade(fade(c))).collect();
        let dots = [[1u32, 2, 4, 64], [8, 16, 32, 128]];
        self.normal.clear();
        self.active.clear();
        for y in 0..height {
            let mut spans: Vec<Span> = Vec::new();
            for x in 0..MINI_COLUMNS {
                let (mut bits, mut colour) = (0u32, 0u8);
                for (dy, _) in dots[0].iter().enumerate() {
                    for (dx, d) in dots.iter().enumerate() {
                        let c = px[y * 4 + dy][x * 2 + dx];
                        if c != 0 {
                            bits |= d[dy];
                            colour = colour.max(c);
                        }
                    }
                }
                if bits == 0 {
                    spans.push(Span::plain(" "));
                } else {
                    let ch = char::from_u32(0x2800 + bits).unwrap_or(' ');
                    spans.push(Span::new(ch.to_string(), Style::fg(ink[colour as usize])));
                }
            }
            let text = Line::from_spans(spans);
            let r_at = ((y * 4 + 2) * self.scale / h4.max(1)).min(self.total.saturating_sub(1));
            let mut normal = line([plain(" "), sp("│", age(r, s, r_at, now))]);
            join(&mut normal, on_ground(text.clone(), MINI_COLUMNS, GROUND));
            let mut active = line([plain(" "), sp("▌", fade(BLUE))]);
            join(&mut active, on_ground(text, MINI_COLUMNS, CHROME_BG));
            self.normal.push(normal);
            self.active.push(active);
        }
    }

    /// The rows of the map the view covers, [first, last) (bounds).
    pub fn bounds(&self) -> (usize, usize) {
        let s = self.scale.max(1);
        (
            self.start * self.height / s,
            self.height.min((self.end * self.height).div_ceil(s).max(1)),
        )
    }

    /// Dots row y, lit where the view is (line).
    pub fn line(&self, y: usize) -> Line {
        let (first, last) = self.bounds();
        let rows = if y >= first && y < last {
            &self.active
        } else {
            &self.normal
        };
        rows.get(y).cloned().unwrap_or_default()
    }

    /// Whether screen cell x, y is on the map (hit).
    pub fn hit(&self, x: usize, y: usize) -> bool {
        self.visible
            && x >= self.x
            && x < self.x + MINI_WIDTH
            && y >= self.y
            && y < self.y + self.height
    }

    /// A press on the map at screen row y: off the view, the view jumps
    /// there (the top and bottom rows to the ends); then it drags
    /// (pressMinimap). The start to seek to, if it moves.
    pub fn press(&mut self, y: usize) -> Option<usize> {
        let (first, last) = self.bounds();
        let local = y - self.y;
        let to = if local < first || local >= last {
            Some(if local == 0 {
                0
            } else if local + 1 == self.height {
                usize::MAX
            } else {
                (local * self.scale / self.height.max(1)).saturating_sub(self.viewport / 2)
            })
        } else {
            None
        };
        self.dragging = true;
        self.drag_y = y;
        self.drag_start = to.map_or(self.start, |t| t.min(self.total));
        to
    }

    /// Where a drag to screen row y takes the view's start (dragMinimap).
    pub fn drag(&self, y: usize) -> usize {
        if y <= self.y {
            return 0;
        }
        if y + 1 >= self.y + self.height {
            return usize::MAX;
        }
        let d =
            (y as isize - self.drag_y as isize) * self.scale as isize / self.height.max(1) as isize;
        (self.drag_start as isize + d).max(0) as usize
    }
}

/// The map's edge beside row r: bright where the turn there ended in the
/// last few minutes, fading through the hour and the day (miniAge).
fn age(r: &Renderer, s: &Session, row: usize, now: Ms) -> Color {
    if s.turns.is_empty() {
        return FAINT;
    }
    let t = &s.turns[r.turn_at(row).min(s.turns.len() - 1)];
    let at = if t.end > 0 { t.end } else { t.start };
    if at <= 0 {
        return FAINT;
    }
    match now - at {
        a if a < 5 * 60_000 => TEXT,
        a if a < 3_600_000 => SUB,
        a if a < 86_400_000 => DIM,
        _ => FAINT,
    }
}

/// Puts the view's top at row start, rows tall: the end follows from the
/// last screenful, the top as Home (seekMinimap).
pub fn seek(v: &mut SessionView, mm: &mut Minimap, start: usize, rows: usize) {
    let total = v.total();
    if start >= total.saturating_sub(mm.viewport) {
        v.pos = Pos::End;
        mm.start = total.saturating_sub(mm.viewport);
    } else if start == 0 {
        v.home(rows);
        mm.start = 0;
    } else {
        v.scroll(v.start as isize - start as isize, rows);
        mm.start = start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_three_bits_a_column() {
        let mut px = [0u8; PX];
        px[0] = 2;
        px[19] = 5;
        let v = pack(&px);
        assert_eq!(v & 7, 2);
        assert_eq!(v >> 57 & 7, 5);
    }

    #[test]
    fn pixels_take_ink_and_colour() {
        let l = RenderedLine {
            line: line([plain("ab  "), sp("xy", ORANGE)]),
            r#ref: "t1:s:x".into(),
            wrap: false,
            owner: String::new(),
        };
        let px = pixels(&l, 20);
        // a step's row is orange (3) at base; its orange span is 3 too.
        assert_eq!(&px[..6], &[3, 3, 0, 0, 3, 3]);
        let l = RenderedLine {
            line: line([sp("ok", GREEN)]),
            r#ref: String::new(),
            wrap: false,
            owner: String::new(),
        };
        assert_eq!(&pixels(&l, 20)[..3], &[4, 4, 0]);
    }

    #[test]
    fn a_turn_never_drawn_is_guessed() {
        let head = guess(3, 0);
        assert_eq!(head >> 3 & 7, 2);
        assert_eq!(head & 7, 0);
        assert_ne!(guess(3, 2), 0);
    }

    #[test]
    fn bounds_and_drag() {
        let mm = Minimap {
            height: 10,
            scale: 100,
            start: 50,
            end: 60,
            y: 4,
            drag_y: 6,
            drag_start: 50,
            ..Minimap::default()
        };
        assert_eq!(mm.bounds(), (5, 6));
        assert_eq!(mm.drag(4), 0);
        assert_eq!(mm.drag(13), usize::MAX);
        assert_eq!(mm.drag(8), 70);
    }

    /// The edge beside a turn fades with how long ago it ended: the last
    /// five minutes bright, then the hour, the day, older
    /// (TestMinimapEdgeShadesByAge).
    #[test]
    fn the_edge_fades_with_age() {
        let mut s = Session::new();
        s.sent("q".into(), Vec::new(), 1000);
        let r = Renderer::new();
        let now: Ms = 1_000_000_000;
        for (ago, want) in [
            (60_000, TEXT),
            (30 * 60_000, SUB),
            (5 * 3_600_000, DIM),
            (72 * 3_600_000, FAINT),
        ] {
            s.turns[0].end = now - ago;
            assert_eq!(age(&r, &s, 0, now), want, "{ago}");
        }
        s.turns[0].end = 0;
        s.turns[0].start = 0;
        assert_eq!(age(&r, &s, 0, now), FAINT, "never timed");
    }
}
