//! clanker, the RUSH bottle in the header's corner. After Go's
//! internal/ui/clanker.go (frames :74-96, clkInk :185, clkLabelBG :205,
//! bottle :213, sprite :244, lines :125, drops :361, clkPaints :378,
//! clkHash :421) on a dark ground: the drops that drift off him while he
//! works, and the coins off him on a dear day. And his reactions
//! (sprite :246, shimmer :163, clkBand :301, tint :364, clkMix :404,
//! clkFX.tint :463, draw :477, fxTick :457, onFXTick :555, clkBeat :565,
//! clkState :574): what finish.rs keeps (Finish.fx) washes
//! him and comes off him, a frame every 70 ms only while one runs. On a
//! light ground he's grey in its shades but where a light passes over
//! him (mono :307, darkGround :326, boldOf :329, clkGrey :339); on a
//! dark one he wears his own colours, so a shimmer shows nothing there.
//! In zen no light passes over him (clkBeat :566).

use std::time::{Duration, Instant};

use rush_core::text::{Color, Line, Mods, Span, Style};

use crate::frame::ground::{self, Ground, Rgb};
use crate::model::Model;
use crate::screens::session_core::finish::{FX_EVERY, Finish, Fx, Reaction, fx_tick, react};
use crate::style::{BLUE, DIM, FAINT, GREEN, ORANGE, RED, TEXT, YELLOW};

/// Seconds between the idle bottle's shimmers (clkCycle).
const CYCLE: usize = 45;

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

/// Him at second `tick`, as H lines W cells wide, reacting to fx, on the
/// ground rush's colours are made for (clanker, sprite).
pub fn draw_fx(md: Mood, tick: usize, rich: bool, fx: Reaction) -> Vec<Line> {
    draw_on(ground::painted(), md, tick, rich, fx)
}

/// Him on the ground gr (sprite): rich, today's spend is high and now and
/// then a coin drops off him; a reaction washes him in its colour and
/// draws what comes off him in place of the drops and the coin.
pub fn draw_on(gr: Ground, md: Mood, tick: usize, rich: bool, fx: Reaction) -> Vec<Line> {
    let mut g = [[Cell::default(); W]; H];
    let mut f = CAPPED;
    match md {
        Mood::Working if tick / 2 % 2 == 1 => f = POPPED,
        Mood::NeedsYou => {
            f = OPEN;
            let c = if tick.is_multiple_of(2) {
                ORANGE
            } else {
                clk_mix(ORANGE, FAINT, 0.55)
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
    // On a dark ground the bottle wears its own colours; on a light one
    // he's grey but where the shimmer's band is.
    if !gr.bg.dark() {
        let at = if fx.kind == Fx::Shimmer {
            band(fx.frame, Fx::Shimmer.frames())
        } else {
            -99.0 // off him
        };
        mono(&mut g, gr, at, md == Mood::NeedsYou);
    }
    if md == Mood::Sleepy {
        tint(&mut g, FAINT, 0.5);
    }
    if let Some((c, k)) = fx_tint(fx) {
        tint(&mut g, c, k);
    }
    if fx.kind != Fx::None {
        fx_draw(&mut g, fx);
    } else {
        if md == Mood::Working {
            drops(&mut g, tick);
        }
        if rich && tick % 30 < 3 {
            let t = tick % 30;
            let c = clk_mix(YELLOW, FAINT, t as f64 * 0.3);
            dot(&mut g, 1 + t, W - 1, ['$', '¢', '.'][t], c);
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

/// Blends two colours, t of the way from a to b (clkMix). Go's compiler
/// fuses its x + (y-x)*t into one multiply-add on arm64, which rounds a
/// half differently now and then: mul_add draws his colours as Go's.
fn clk_mix(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let l = |x: u8, y: u8| (y as f64 - x as f64).mul_add(t, x as f64).round() as u8;
            Color::Rgb(l(ar, br), l(ag, bg), l(ab, bb))
        }
        _ if t < 0.5 => a,
        _ => b,
    }
}

/// Mixes one cell k of the way to c, as clkMix does: bold is dropped.
fn wash(cell: &mut Cell, c: Color, k: f64) {
    cell.st.fg = clk_mix(cell.st.fg, c, k);
    if cell.st.bg != Color::Default {
        cell.st.bg = clk_mix(cell.st.bg, c, k);
    }
    cell.st.mods = Mods(cell.st.mods.0 & !Mods::BOLD.0);
}

/// Mixes the bottle's colours k of the way to c (clkGrid.tint).
fn tint(g: &mut [[Cell; W]; H], c: Color, k: f64) {
    for cell in g.iter_mut().flatten().filter(|c| c.body) {
        wash(cell, c, k);
    }
}

/// Greys his body but within reach of the band, where his own colours
/// show through; needing you, the label keeps its orange (mono).
fn mono(g: &mut [[Cell; W]; H], gr: Ground, at: f64, keep_label: bool) {
    for (y, row) in g.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            if !cell.body {
                continue;
            }
            let d = ((y as f64).mul_add(0.7, x as f64) - at).abs();
            let k = if d < 2.5 { d / 2.5 } else { 1.0 };
            // clkMix drops bold; mono puts it back (boldOf).
            cell.st.fg = clk_mix(cell.st.fg, grey(cell.st.fg, gr), k);
            if cell.st.bg != Color::Default && (!keep_label || y != LABEL) {
                cell.st.bg = clk_mix(cell.st.bg, grey(cell.st.bg, gr), k);
            }
        }
    }
}

/// A colour's grey in the ground's shades, between its background and
/// its text: its lightness, pushed apart so the black cap, the glass and
/// the white label stay distinct; dark to light whichever the ground is,
/// so the cap stays black (clkGrey). Go's compiler fuses its multiply-adds
/// on arm64: mul_add rounds as it does.
fn grey(c: Color, gr: Ground) -> Color {
    let Color::Rgb(r, g, b) = c else {
        return c;
    };
    let l = 0.11f64.mul_add(b as f64, 0.59f64.mul_add(g as f64, 0.3 * r as f64)) / 255.0;
    let l = (l - 0.5).mul_add(1.6, 0.5).clamp(0.0, 1.0);
    let (mut lo, mut hi) = (gr.bg, gr.fg);
    if !lo.dark() {
        (lo, hi) = (hi, lo);
    }
    let t = l.mul_add(0.9, 0.2);
    let m = |x: u8, y: u8| {
        (y as f64 - x as f64)
            .mul_add(t, x as f64)
            .clamp(0.0, 255.0)
            .round() as u8
    };
    Rgb(m(lo.0, hi.0), m(lo.1, hi.1), m(lo.2, hi.2)).color()
}

/// Where a band of light passing over him is, f of n frames in: low left
/// to high right, starting and ending off him (clkBand).
fn band(f: usize, n: usize) -> f64 {
    -3.0 + (W + 6) as f64 * f as f64 / n.saturating_sub(1).max(1) as f64
}

/// Passes a band of light over his body, low left to high right; f of n
/// is how far across it is (clkGrid.shimmer).
fn shimmer(g: &mut [[Cell; W]; H], f: usize, n: usize, c: Color, strength: f64) {
    let p = band(f, n);
    for (y, row) in g.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            if !cell.body {
                continue;
            }
            let d = ((y as f64).mul_add(0.7, x as f64) - p).abs();
            if d < 2.0 {
                wash(cell, c, strength * (1.0 - d / 2.0));
            }
        }
    }
}

/// The colour a reaction washes him in, and how far, easing back as it
/// goes (clkFX.tint).
fn fx_tint(fx: Reaction) -> Option<(Color, f64)> {
    let f = fx.frame as f64;
    let (c, k) = match fx.kind {
        Fx::Ask => (YELLOW, 0.5 * (1.0 - f / 10.0).max(0.0)),
        Fx::Answered => (GREEN, 0.5 * (1.0 - f / 14.0).max(0.0)),
        Fx::Error => (RED, 0.6 * (1.0 - f / 12.0).max(0.0)),
        _ => return None,
    };
    (k > 0.0).then_some((c, k))
}

/// Drops one cell at (y, x), which may be off the grid (clkGrid.dot).
fn dot_at(g: &mut [[Cell; W]; H], y: isize, x: isize, ch: char, c: Color) {
    if y >= 0 && x >= 0 {
        dot(g, y as usize, x as usize, ch, c);
    }
}

/// What comes off him as he reacts (clkFX.draw).
fn fx_draw(g: &mut [[Cell; W]; H], fx: Reaction) {
    let f = fx.frame;
    // The columns just off each side of him.
    let (l, r) = (X as isize - 1, (X + BODY_W) as isize);
    match fx.kind {
        // sprite lets the bottle's colour through where it passes: mono's.
        Fx::None | Fx::Shimmer => {}
        Fx::Ask => {
            // Streaks out from him, a tip with a short tail, fading as
            // they go; then a yellow light passes over him.
            let rays: [(isize, isize, isize, isize, char); 6] = [
                (2, l, 0, -1, '─'),
                (2, r, 0, 1, '─'),
                (1, l, -1, -1, '╲'),
                (1, r, -1, 1, '╱'),
                (3, l, 1, -1, '╱'),
                (3, r, 1, 1, '╲'),
            ];
            if f < 8 {
                let step = f / 2 + 1;
                let c = clk_mix(YELLOW, FAINT, f as f64 / 8.0);
                for (y, x, dy, dx, ch) in rays {
                    for s in step.saturating_sub(2)..step.min(2) {
                        let (ch, cc) = if s == step - 1 {
                            (ch, c)
                        } else {
                            ('·', clk_mix(c, FAINT, 0.5))
                        };
                        let s = s as isize;
                        dot_at(g, y + dy * s, x + dx * s, ch, cc);
                    }
                }
            }
            if f >= 4 {
                shimmer(g, f - 4, Fx::Shimmer.frames(), YELLOW, 0.8);
            }
        }
        Fx::Answered => {
            // Confetti drifting down past him, dimming as it falls.
            const PAINTS: [Color; 5] = [ORANGE, YELLOW, BLUE, GREEN, TEXT];
            const XS: [usize; 7] = [0, 1, W - 2, W - 1, X + 1, X + 5, X + 9];
            for (i, x) in XS.into_iter().enumerate() {
                let h = hash(1000 + i as i64);
                let y = (f / 2) as isize - (h % 5) as isize;
                if !(0..H as isize).contains(&y) {
                    continue;
                }
                let c = clk_mix(PAINTS[h / 13 % PAINTS.len()], FAINT, y as f64 * 0.25);
                dot_at(g, y, x as isize, ['·', '•', '°', '+'][h / 9 % 4], c);
            }
            if f >= 6 {
                shimmer(g, f - 6, Fx::Shimmer.frames(), GREEN, 0.5);
            }
        }
        Fx::Done => {
            // A glint across him, and a drop off each side.
            shimmer(g, f, Fx::Done.frames(), TEXT, 0.5);
            for (i, x) in [l, r].into_iter().enumerate() {
                let y = ((f + 2 * i) / 4 + 1) as isize;
                dot_at(g, y, x, '·', clk_mix(ORANGE, FAINT, f as f64 / 14.0));
            }
        }
        Fx::Error => {
            // A few red sparks, then nothing.
            if f < 9 && f.is_multiple_of(3) {
                for i in 0..2 {
                    let h = hash(5000 + (f * 3 + i) as i64);
                    let x = [0, 1, W - 2, W - 1][h / 5 % 4];
                    let c = clk_mix(RED, FAINT, f as f64 / 10.0);
                    dot(g, h % H, x, ['×', '·'][h / 7 % 2], c);
                }
            }
        }
    }
}

/// Where his reaction's frames are counted from: the last frame moved
/// on, and the reaction as it was then, so one a stronger one replaced
/// starts its own count.
#[derive(Default)]
struct FxClock {
    last: Option<Instant>,
    seen: Reaction,
}

/// What clanker is reacting to now (m.fx).
pub fn reaction(m: &Model) -> Reaction {
    m.kept.peek::<Finish>().map(|f| f.fx).unwrap_or_default()
}

/// Moves his reaction on a frame for every 70 ms gone since the last, and
/// asks to be woken for the next while one runs and he's shown: nothing
/// wakes for him otherwise (fxTick, onFXTick). Called as a frame is drawn.
pub fn step(m: &mut Model) {
    let fx = reaction(m);
    let now = Instant::now();
    let every = Duration::from_millis(FX_EVERY as u64);
    let ck = m.kept::<FxClock>();
    if fx.kind == Fx::None {
        *ck = FxClock::default();
        return;
    }
    let mut fx = fx;
    match ck.last {
        Some(last) if fx == ck.seen => {
            let n = (now.duration_since(last).as_millis() / every.as_millis()) as u32;
            if n > 0 {
                ck.last = Some(last + every * n);
                for _ in 0..n {
                    if !fx_tick(&mut fx) {
                        break;
                    }
                }
                m.kept::<Finish>().fx = fx;
            }
        }
        // Just started, or a stronger one took over: frame 0 is now.
        _ => ck.last = Some(now),
    }
    let ck = m.kept::<FxClock>();
    ck.seen = fx;
    if fx.kind == Fx::None {
        *ck = FxClock::default();
        return;
    }
    let next = ck.last.map(|l| l + every);
    if !m.cfg.hide_logo
        && let Some(t) = next
    {
        m.ask_wake(t);
    }
}

/// clanker's second: now and then a light passes over him, never in zen
/// (clkBeat). Called as the tick moves on; it never wakes rush by itself.
pub fn beat(m: &mut Model) {
    if reaction(m).kind != Fx::None || m.zen().is_some() {
        return;
    }
    let md = m.mood(m.tally());
    let t = m.tick;
    if md == Mood::Working && t % 15 == 8 || md == Mood::Idle && t % CYCLE == 25 {
        react(&mut m.kept::<Finish>().fx, Fx::Shimmer);
    }
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
        let c = clk_mix(PAINTS[h / 11 % PAINTS.len()], FAINT, 0.3 + 0.4 * age as f64);
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

    fn draw(md: Mood, tick: usize) -> Vec<Line> {
        draw_fx(md, tick, false, Reaction::default())
    }

    fn draw_rich(md: Mood, tick: usize, rich: bool) -> Vec<Line> {
        draw_fx(md, tick, rich, Reaction::default())
    }

    /// Every cell of him in every mood, reaction and frame, rich or not, at
    /// second 2: one line a cell, as the dump of Go's clkGrid after sprite
    /// prints it (mood kind frame tick rich y x rune fg bg; colours r,g,b,
    /// b for bold).
    fn dump() -> String {
        dump_on(crate::frame::ground::DARK)
    }

    fn dump_on(gr: Ground) -> String {
        let col = |c: Color, b: bool| match c {
            Color::Rgb(r, g, bb) => format!("{r},{g},{bb}{}", if b { "b" } else { "" }),
            _ => "-".into(),
        };
        let mut s = String::new();
        let kinds = [
            Fx::None,
            Fx::Shimmer,
            Fx::Done,
            Fx::Answered,
            Fx::Ask,
            Fx::Error,
        ];
        for (mi, md) in [Mood::Idle, Mood::Working, Mood::NeedsYou, Mood::Sleepy]
            .into_iter()
            .enumerate()
        {
            for (ki, k) in kinds.into_iter().enumerate() {
                for f in 0..k.frames().max(1) {
                    for rich in [false, true] {
                        let rows = draw_on(gr, md, 2, rich, Reaction { kind: k, frame: f });
                        for (y, l) in rows.iter().enumerate() {
                            let mut x = 0;
                            for sp in &l.spans {
                                for ch in sp.text.chars() {
                                    if !(ch == ' ' && sp.style == Style::default()) {
                                        let b = sp.style.mods.contains(Mods::BOLD);
                                        s.push_str(&format!(
                                            "{mi} {ki} {f} 2 {rich} {y} {x} {ch} {} {}\n",
                                            col(sp.style.fg, b),
                                            col(sp.style.bg, false)
                                        ));
                                    }
                                    x += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        s
    }

    fn fnv(s: &str) -> u64 {
        s.bytes().fold(0xcbf29ce484222325, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        })
    }

    /// The same dump of Go's clkGrid after sprite (clanker.go:244-575, dark
    /// ground), every reaction's every frame: 19686 cells, hashed.
    const GO_CELLS: usize = 19686;
    const GO_FNV: u64 = 0xd7e09e5a6a1002d4;

    #[test]
    fn reactions_draw_as_gos() {
        let d = dump();
        if let Some(p) = std::env::var_os("CLK_DUMP").filter(|p| !p.is_empty()) {
            std::fs::write(p, &d).unwrap();
        }
        assert_eq!(d.lines().count(), GO_CELLS);
        assert_eq!(fnv(&d), GO_FNV, "{:#x}", fnv(&d));
    }

    /// A reaction draws what comes off him in place of the drops, and
    /// ends after its frames (clkFX.draw, onFXTick).
    #[test]
    fn reactions_take_the_drops_place() {
        let ask = draw_fx(
            Mood::Working,
            0,
            false,
            Reaction {
                kind: Fx::Ask,
                frame: 0,
            },
        );
        assert_eq!(ask[2].plain().chars().next(), Some('─'));
        assert_eq!(ask[2].plain().chars().nth(X + BODY_W), Some('─'));
        assert_eq!(ask[3].plain().chars().nth(W - 1), Some(' '), "no drop");
        let err = draw_fx(
            Mood::Idle,
            0,
            false,
            Reaction {
                kind: Fx::Error,
                frame: 1,
            },
        );
        let plain = |f| {
            draw_fx(
                Mood::Idle,
                0,
                false,
                Reaction {
                    kind: Fx::Error,
                    frame: f,
                },
            )
            .iter()
            .map(Line::plain)
            .collect::<Vec<_>>()
        };
        assert_ne!(plain(0), plain(1), "sparks at 0");
        assert_eq!(plain(1), plain(2), "none between");
        assert!(
            err.iter()
                .all(|l| l.spans.iter().all(|s| !s.style.mods.contains(Mods::BOLD)))
        );
        let mut r = Reaction {
            kind: Fx::Done,
            frame: 0,
        };
        let mut n = 1;
        while fx_tick(&mut r) {
            n += 1;
        }
        assert_eq!((n, r.kind), (14, Fx::None));
    }

    /// Working, a light passes over him at second 8 of every 15; idle, at
    /// 25 of every 45; never over a reaction running (clkBeat).
    #[test]
    fn beat_shimmers_now_and_then() {
        let mut m = Model::new("t");
        m.clock = Some(1_790_000_000_000);
        m.tick = 24;
        let idle = m.mood(m.tally());
        beat(&mut m);
        assert_eq!(reaction(&m).kind, Fx::None);
        m.tick = 25;
        beat(&mut m);
        let want = if idle == Mood::Idle {
            Fx::Shimmer
        } else {
            Fx::None
        };
        assert_eq!(reaction(&m).kind, want, "{idle:?}");
        m.kept::<Finish>().fx = Reaction {
            kind: Fx::Error,
            frame: 3,
        };
        m.tick = 25 + CYCLE;
        beat(&mut m);
        assert_eq!(
            reaction(&m),
            Reaction {
                kind: Fx::Error,
                frame: 3
            }
        );
    }

    /// In zen no light passes over him, at any second (clkBeat's m.zen).
    #[test]
    fn no_shimmer_in_zen() {
        let mut m = Model::new("t");
        m.clock = Some(1_790_000_000_000);
        m.kept::<crate::screens::social::zen::Zen>().on = true;
        for t in 0..2 * CYCLE {
            m.tick = t;
            beat(&mut m);
            assert_eq!(reaction(&m).kind, Fx::None, "second {t}");
        }
        m.kept::<crate::screens::social::zen::Zen>().on = false;
        m.tick = 8;
        let md = m.mood(m.tally());
        m.tick = if md == Mood::Working { 8 } else { 25 };
        beat(&mut m);
        assert_eq!(reaction(&m).kind, Fx::Shimmer, "{md:?}");
    }

    /// The same dump of Go's clkGrid after sprite with painted set to
    /// theme.Light and the palette left as on the dark ground: mono and
    /// clkGrey grey him but where the shimmer's band is.
    const GO_LIGHT_CELLS: usize = 19686;
    const GO_LIGHT_FNV: u64 = 0x5dc07b9cfb0b4d62;

    #[test]
    fn a_light_ground_greys_him_as_gos() {
        let d = dump_on(crate::frame::ground::LIGHT);
        if let Some(p) = std::env::var_os("CLK_DUMP").filter(|p| !p.is_empty()) {
            std::fs::write(format!("{}.light", p.to_string_lossy()), &d).unwrap();
        }
        assert_eq!(d.lines().count(), GO_LIGHT_CELLS);
        assert_eq!(fnv(&d), GO_LIGHT_FNV, "{:#x}", fnv(&d));
        // Needing you, the label keeps its orange on any ground.
        let rows = draw_on(
            crate::frame::ground::LIGHT,
            Mood::NeedsYou,
            0,
            false,
            Reaction::default(),
        );
        let label = rows[LABEL]
            .spans
            .iter()
            .find(|s| s.text.contains('R'))
            .unwrap();
        assert_eq!(label.style.bg, ORANGE);
        assert!(label.style.mods.contains(Mods::BOLD), "bold kept");
    }

    /// With nothing to react to, drawing him asks for no wake-up: idle
    /// costs nothing. One running asks for its next frame.
    #[test]
    fn only_a_reaction_wakes() {
        let mut m = Model::new("t");
        step(&mut m);
        assert!(m.kept.peek::<FxClock>().is_none_or(|c| c.last.is_none()));
        m.kept::<Finish>().fx = Reaction {
            kind: Fx::Done,
            frame: 0,
        };
        step(&mut m);
        assert!(m.kept::<FxClock>().last.is_some());
        assert_eq!(reaction(&m).kind, Fx::Done);
    }

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
