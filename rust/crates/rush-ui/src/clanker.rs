//! clanker, the RUSH bottle in the header's corner. After Go's
//! internal/ui/clanker.go (frames :74-96, clkInk :185, clkLabelBG :205,
//! bottle :213, sprite :244, lines :125, drops :361, clkPaints :378,
//! clkHash :421) on a dark ground: the drops that drift off him while he
//! works, and the coins off him on a dear day. Not ported: the shimmer
//! and reactions (clkFX).

use rush_core::text::{Color, Line, Span, Style};

use crate::style::{BLUE, DIM, FAINT, GREEN, ORANGE, YELLOW, mix};

pub const W: usize = 9;
pub const H: usize = 5;
const X: usize = 1;
const BODY_W: usize = 6;
const LABEL: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Idle,
    Working,
    NeedsYou,
    Sleepy,
}

type Frame = [[&'static str; 2]; H];

const CAPPED: Frame = [
    ["GKGKGK", "KGKGKG"],
    ["kkkkkk", ".dAAa."],
    ["dAAAad", "dAAAad"],
    ["", ""],
    ["dAAAad", "dDDDDd"],
];
const POPPED: Frame = [
    [".GKGKG", ".kkkkk"],
    ["......", ".dAAa."],
    ["dAAAad", "dAAAad"],
    ["", ""],
    ["dAAAad", "dDDDDd"],
];
const OPEN: Frame = [
    ["......", "......"],
    ["......", ".dAAa."],
    ["dAAAad", "dAAAad"],
    ["", ""],
    ["dAAAad", "dDDDDd"],
];

fn ink(r: u8) -> Option<Color> {
    Some(match r {
        b'K' => Color::Rgb(64, 64, 68),
        b'G' => Color::Rgb(112, 112, 116),
        b'k' => Color::Rgb(88, 88, 92),
        b'A' => Color::Rgb(154, 76, 20),
        b'a' => Color::Rgb(212, 128, 44),
        b'd' => Color::Rgb(104, 46, 8),
        b'D' => Color::Rgb(84, 38, 6),
        _ => return None,
    })
}

#[derive(Clone, Copy, Default)]
struct Cell {
    ch: char,
    st: Style,
    body: bool,
}

/// Him at second `tick`, as H lines W cells wide.
pub fn draw(md: Mood, tick: usize) -> Vec<Line> {
    draw_rich(md, tick, false)
}

/// Him at second `tick`; rich, today's spend is high and now and then a
/// coin drops off him (clkState.rich).
pub fn draw_rich(md: Mood, tick: usize, rich: bool) -> Vec<Line> {
    let mut g = [[Cell::default(); W]; H];
    let mut f = CAPPED;
    match md {
        Mood::Working if tick / 2 % 2 == 1 => f = POPPED,
        Mood::NeedsYou => {
            f = OPEN;
            let c = if tick.is_multiple_of(2) {
                ORANGE
            } else {
                mix(ORANGE, FAINT, 0.55)
            };
            g[0][X + BODY_W / 2 - 1] = Cell {
                ch: '!',
                st: Style::fg(c),
                body: false,
            };
        }
        Mood::Sleepy => {
            let z = |g: &mut [[Cell; W]; H], y: usize, x: usize, ch: char, c: Color| {
                g[y][x] = Cell {
                    ch,
                    st: Style::fg(c),
                    body: false,
                }
            };
            match tick % 3 {
                0 => z(&mut g, 1, X + BODY_W + 1, 'z', FAINT),
                1 => {
                    z(&mut g, 0, X + BODY_W, 'Z', DIM);
                    z(&mut g, 1, X + BODY_W + 1, 'z', FAINT);
                }
                _ => z(&mut g, 0, X + BODY_W, 'Z', DIM),
            }
        }
        _ => {}
    }
    // The label's yellow; needing you, orange, its ink dark.
    let (label_bg, label_fg) = if md == Mood::NeedsYou {
        (ORANGE, Color::Rgb(40, 16, 4))
    } else {
        (Color::Rgb(250, 214, 30), Color::Rgb(214, 28, 24))
    };
    for (y, row) in f.iter().enumerate() {
        if y == LABEL {
            for (i, ch) in " RUSH ".chars().enumerate() {
                g[y][X + i] = Cell {
                    ch,
                    st: Style::fg(label_fg).on(label_bg).bold(),
                    body: true,
                };
            }
            continue;
        }
        let (top, bot) = (row[0].as_bytes(), row[1].as_bytes());
        for i in 0..top.len() {
            let (t, b) = (ink(top[i]), ink(bot[i]));
            let (ch, st) = match (t, b) {
                (Some(t), Some(b)) => ('▀', Style::fg(t).on(b)),
                (Some(t), None) => ('▀', Style::fg(t)),
                (None, Some(b)) => ('▄', Style::fg(b)),
                (None, None) => continue,
            };
            g[y][X + i] = Cell { ch, st, body: true };
        }
    }
    if md == Mood::Sleepy {
        for row in &mut g {
            for c in row.iter_mut().filter(|c| c.body) {
                c.st.fg = mix(c.st.fg, FAINT, 0.5);
                if c.st.bg != Color::Default {
                    c.st.bg = mix(c.st.bg, FAINT, 0.5);
                }
            }
        }
    }
    if md == Mood::Working {
        drops(&mut g, tick);
    }
    if rich && tick % 30 < 3 {
        let t = tick % 30;
        let c = mix(YELLOW, FAINT, t as f64 * 0.3);
        dot(&mut g, 1 + t, W - 1, ['$', '¢', '.'][t], c);
    }
    g.iter()
        .map(|row| {
            let mut l = Line::new();
            for c in row {
                let (ch, st) = if c.ch == '\0' {
                    (' ', Style::default())
                } else {
                    (c.ch, c.st)
                };
                match l.spans.last_mut() {
                    Some(s) if s.style == st => s.text.push(ch),
                    _ => l.spans.push(Span::new(ch.to_string(), st)),
                }
            }
            l
        })
        .collect()
}

/// Drops one cell off him, only where it's empty and off his body
/// (clkGrid.dot).
fn dot(g: &mut [[Cell; W]; H], y: usize, x: usize, ch: char, c: Color) {
    if y >= H || x >= W || g[y][x].ch != '\0' {
        return;
    }
    if y > 0 && (X..X + BODY_W).contains(&x) {
        return;
    }
    g[y][x] = Cell {
        ch,
        st: Style::fg(c),
        body: false,
    };
}

/// Drops drift off him a second at a time: one falls a row a second down
/// a side and fades. One in every five seconds starts one (drops).
fn drops(g: &mut [[Cell; W]; H], tick: usize) {
    const LOOK: [char; 2] = ['·', '.'];
    const PAINTS: [Color; 4] = [ORANGE, YELLOW, BLUE, GREEN];
    for age in (0..LOOK.len()).rev() {
        let h = hash(tick as i64 - age as i64);
        if !h.is_multiple_of(5) {
            continue;
        }
        let mut x = h / 7 % 2;
        if (h / 3).is_multiple_of(2) {
            x = W - 1 - x;
        }
        let c = mix(PAINTS[h / 11 % PAINTS.len()], FAINT, 0.3 + 0.4 * age as f64);
        dot(g, 2 + age, x, LOOK[age], c);
    }
}

/// Scatters n so drops land somewhere new each time but the same second
/// always draws the same (clkHash).
fn hash(n: i64) -> usize {
    let mut x = (n as u32).wrapping_mul(2654435761);
    x ^= x >> 15;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    (x % 100003) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_is_his_width() {
        for md in [Mood::Idle, Mood::Working, Mood::NeedsYou, Mood::Sleepy] {
            for tick in 0..4 {
                let rows = draw(md, tick);
                assert_eq!(rows.len(), H);
                for r in &rows {
                    assert_eq!(r.width(), W, "{md:?} {tick}");
                }
                let label: String = rows[LABEL].plain().chars().take(X + BODY_W).collect();
                assert_eq!(label, "  RUSH ");
            }
        }
        assert_eq!(draw(Mood::NeedsYou, 0)[0].plain().trim(), "!");
    }

    /// Working, a drop drifts off his side now and then, never over him;
    /// Go's frame at second 0 has one on each of rows 2 and 3.
    #[test]
    fn drops_drift_off_him() {
        let rows = draw(Mood::Working, 0);
        assert_eq!(
            rows[2].plain().chars().nth(W - 1),
            Some('·'),
            "{:?}",
            rows[2].plain()
        );
        assert_eq!(
            rows[3].plain().chars().nth(W - 1),
            Some('.'),
            "{:?}",
            rows[3].plain()
        );
        assert_eq!(
            draw(Mood::Idle, 0)[2].plain().trim_end().chars().count(),
            X + BODY_W
        );
        // A dear day: a coin off him, top right.
        assert_eq!(
            draw_rich(Mood::Idle, 0, true)[1].plain().chars().nth(W - 1),
            Some('$')
        );
    }
}
