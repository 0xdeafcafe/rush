//! clanker, the RUSH bottle in the header's corner. After Go's
//! internal/ui/clanker.go (frames :74-96, clkInk :185, clkLabelBG :205,
//! bottle :213, sprite :244, lines :125) on a dark ground. Not ported: the
//! shimmer, the drops that drift off him, coins and reactions (clkFX).

use rush_core::text::{Color, Line, Span, Style};

use crate::style::{DIM, FAINT, ORANGE, mix};

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
                assert_eq!(rows[LABEL].plain(), "  RUSH   ");
            }
        }
        assert_eq!(draw(Mood::NeedsYou, 0)[0].plain().trim(), "!");
    }
}
