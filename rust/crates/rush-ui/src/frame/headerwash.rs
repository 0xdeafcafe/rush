//! The Session header's wash: the first 60% of a row as it is, then
//! sparse blocks of the harness's colour growing into the provider's at
//! the right edge. The pattern is fixed; a frame only lays the kept
//! colours on. After Go's internal/ui/headerwash.go (headerWash :17,
//! washWith :28, headerColours :91, headerColourAt :110, headerSmooth
//! :157, headerEdgeStart :163, makeHeaderColours :165).

use std::sync::Mutex;

use rush_core::text::{Color, Line, Span};

use super::ground::{Ground, Rgb, contrast, mix};
use crate::style::CHROME_BG;

/// What a row's colours are made from: the terminal's ground, the
/// harness's and the provider's colours, the width, and which band of
/// rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WashKey {
    pub ground: Ground,
    pub harness: Rgb,
    pub provider: Rgb,
    pub width: usize,
    pub row: usize,
}

/// The last palettes made: switching sessions and resizing never keep an
/// unbounded set (headerColourCache).
static CACHE: Mutex<Vec<(WashKey, Vec<Color>)>> = Mutex::new(Vec::new());
const KEEP: usize = 12;

/// Each cell's ground across a header row of width (headerColours).
pub fn header_colours(
    ground: Ground,
    harness: Rgb,
    provider: Rgb,
    width: usize,
    row: usize,
) -> Vec<Color> {
    let key = WashKey {
        ground,
        harness,
        provider,
        width,
        row: (row % 4) / 2,
    };
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, v)) = c.iter().find(|(k, _)| *k == key) {
        return v.clone();
    }
    let v = make(key);
    if c.len() >= KEEP {
        c.remove(0);
    }
    c.push((key, v.clone()));
    v
}

/// l over the wash: every cell without a ground of its own (or on the
/// chrome's) takes the wash's at its column (washWith).
pub fn wash(l: &Line, colours: &[Color]) -> Line {
    let width = colours.len();
    if width == 0 {
        return Line::new();
    }
    let l = crate::style::fit(l.clone(), width);
    let mut out = Line::new();
    let mut x = 0;
    for s in &l.spans {
        let own = s.style.bg != Color::Default && s.style.bg != CHROME_BG;
        if own {
            x += s.width();
            out.spans.push(s.clone());
            continue;
        }
        let mut run = String::new();
        let mut at = Color::Default;
        for ch in s.text.chars() {
            let bg = colours[x.min(width - 1)];
            if bg != at && !run.is_empty() {
                out.spans
                    .push(Span::new(std::mem::take(&mut run), s.style.on(at)));
            }
            at = bg;
            run.push(ch);
            x += rush_core::text::width(ch.encode_utf8(&mut [0; 4]));
        }
        if !run.is_empty() {
            out.spans.push(Span::new(run, s.style.on(at)));
        }
    }
    out
}

/// The Session's header rows (blank, name, meta, tabs), w wide with the
/// pane's gutters (two cells left, one right), washed for an agent of
/// kind: the rows between the gutters, the right gutter in the wash's
/// last colour; the rule under them in the harness's grey
/// (paneHeader's headerWash, paneFrameRow).
pub fn wash_head(reg: &rush_core::adapter::Registry, kind: &str, rows: &mut [Line], w: usize) {
    use crate::screens::accounts::look::look_of;
    let iw = w.saturating_sub(3);
    if iw == 0 || rows.len() < 4 {
        return;
    }
    let harness = look_of(reg, reg.harness_of(kind).as_str()).rgb;
    let provider = look_of(reg, &reg.provider_of(kind)).rgb;
    let ground = super::ground::painted();
    for (r, row) in rows.iter_mut().take(4).enumerate() {
        let colours = header_colours(ground, harness, provider, iw, r);
        let inner = crate::screens::session_core::paint::cut(row, 2, 2 + iw);
        let mut out = Line::from_spans(vec![Span::new(
            "  ",
            rush_core::text::Style::default().on(CHROME_BG),
        )]);
        out.spans.extend(wash(&inner, &colours).spans);
        let last = colours.last().copied().unwrap_or(CHROME_BG);
        out.spans
            .push(Span::new(" ", rush_core::text::Style::default().on(last)));
        *row = out;
    }
}

/// The first column the wash starts at: ceil(width × 0.6)
/// (headerEdgeStart).
pub fn edge_start(width: usize) -> usize {
    (width * 6).div_ceil(10)
}

fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

const BASE: Rgb = Rgb(30, 28, 26);

fn colour_at(key: WashKey, x: f64) -> Rgb {
    let g = key.ground;
    let ground = g.surface(BASE);
    let column = (x * key.width.saturating_sub(1) as f64).round() as usize;
    let start = edge_start(key.width);
    if column < start {
        return ground;
    }
    let block = (column - start) / 4;
    let blocks = (key.width - start).div_ceil(4);
    let fade = 8
        .min(4.max(blocks / 2))
        .min(1.max(blocks.saturating_sub(1)));
    let mut u = (block as f64 / fade as f64).min(1.0);
    // Offset alternate two-row bands within the ramp, never at its end.
    if !key.row.is_multiple_of(2) {
        u -= 0.10 * (std::f64::consts::PI * u).sin();
    }
    let env = smooth(u);
    let strength = 0.24 * env;
    let mut colour = g.surface(mix(BASE, mix(key.harness, key.provider, env), strength));
    // Dim words stay at least as readable as on the chrome (up to 3:1);
    // stronger text at least 4.5:1.
    let dim_ink = g.ink(Rgb(122, 117, 108));
    let text_ink = g.ink(Rgb(168, 162, 152));
    let dim_floor = 3.0f64.min(contrast(dim_ink, ground));
    let text_floor = 4.5f64.min(contrast(text_ink, ground));
    let mut anchor = g.bg;
    if contrast(dim_ink, anchor) < dim_floor || contrast(text_ink, anchor) < text_floor {
        anchor = ground;
    }
    for _ in 0..24 {
        if contrast(dim_ink, colour) >= dim_floor && contrast(text_ink, colour) >= text_floor {
            break;
        }
        colour = mix(colour, anchor, 0.12);
    }
    if contrast(dim_ink, colour) < dim_floor || contrast(text_ink, colour) < text_floor {
        return ground;
    }
    colour
}

fn make(key: WashKey) -> Vec<Color> {
    let w = key.width;
    let ground = key.ground.surface(BASE);
    let start = edge_start(w);
    let mut out: Vec<Color> = Vec::with_capacity(w);
    for i in 0..w {
        if i < start {
            out.push(ground.color());
            continue;
        }
        // Every four cells are one block: they share its colour.
        if !(i - start).is_multiple_of(4) {
            let prev = out[i - 1];
            out.push(prev);
            continue;
        }
        out.push(colour_at(key, i as f64 / 1.max(w - 1) as f64).color());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::ground::DARK;

    #[test]
    fn plain_left_tinted_right() {
        let c = header_colours(DARK, Rgb(217, 119, 87), Rgb(217, 119, 87), 100, 0);
        assert_eq!(c.len(), 100);
        assert_eq!(edge_start(100), 60);
        assert!(c[..60].iter().all(|&x| x == CHROME_BG));
        assert_ne!(c[99], CHROME_BG, "the right edge is tinted");
        // Four cells a block.
        assert_eq!(c[61], c[60]);
        let l = wash(&Line::from_spans(vec![Span::plain("hello")]), &c);
        assert_eq!(l.width(), 100);
    }
}
