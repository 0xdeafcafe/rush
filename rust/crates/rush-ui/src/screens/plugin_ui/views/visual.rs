//! Visual and expressive styles: a chart of columns or a line, a heatmap,
//! a hero number drawn big, a callout, code and words. After Go's
//! internal/ui/viewblocks_visual.go (blockChart :27, columnChart :91,
//! lineChart :128, blockHeatmap :183, bigGlyphs :245, bigText :259,
//! blockHero :293, stretch :321, blockCallout :342, blockCode :358,
//! blockText :370).

use super::acts::{ViewAct, ViewLine};
use super::blocks::BlockCtx;
use super::*;

const CHART_ROWS: usize = 6;

fn round2(f: f64) -> f64 {
    (f * 100.0).round() / 100.0
}

/// Values as columns, or a line, CHART_ROWS tall, its most at its top
/// left and its labels, first, middle and last, under it (blockChart).
pub(super) fn chart(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let vs = &b.values;
    if vs.is_empty() {
        return Vec::new();
    }
    let (mut lo, mut hi) = (0.0f64, vs[0]);
    for &v in vs {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if b.style == "line" {
        lo = vs.iter().copied().fold(vs[0], f64::min);
    }
    let col = accent_or(&b.tone, cx.accent);
    let (hs, ls) = (num(round2(hi)), num(round2(lo)));
    let axis = cw(&hs).max(cw(&ls));
    let plot_w = w.saturating_sub(axis + 2).max(1);
    let rows = if b.style == "line" {
        line_chart(vs, lo, hi, plot_w, CHART_ROWS)
    } else {
        column_chart(vs, lo, hi, plot_w, CHART_ROWS)
    };
    let mut out = Vec::new();
    let n = rows.len();
    for (y, r) in rows.into_iter().enumerate() {
        let label = if y == 0 {
            hs.clone()
        } else if y == n - 1 {
            ls.clone()
        } else {
            String::new()
        };
        out.push(ViewLine::plain(ln([
            dim(blanks(axis - cw(&label)) + &label),
            plain(" "),
            faint("┤"),
            paint(col, r),
        ])));
    }
    out.push(ViewLine::plain(ln([
        plain(blanks(axis + 1)),
        faint(format!("└{}", "─".repeat(plot_w))),
    ])));
    if let (Some(first), Some(last)) = (b.labels.first(), b.labels.last()) {
        let mid = if b.labels.len() > 2 {
            b.labels[b.labels.len() / 2].as_str()
        } else {
            ""
        };
        let mut l = spread(ln([dim(first.clone())]), ln([dim(last.clone())]), plot_w);
        if !mid.is_empty() && plot_w > cw(first) + cw(mid) + cw(last) + 4 {
            let at = plot_w / 2 - cw(mid) / 2;
            l = ln([
                dim(first.clone()),
                plain(blanks(at.saturating_sub(cw(first)))),
                dim(mid),
                plain(blanks(plot_w.saturating_sub(at + cw(mid) + cw(last)))),
                dim(last.clone()),
            ]);
        }
        let mut row = ln([plain(blanks(axis + 2))]);
        row.spans.extend(l.spans);
        out.push(ViewLine::plain(row));
    }
    out
}

/// Values as columns h rows tall in w cells, eighths of a cell at their
/// tops, spread to fill w (columnChart).
pub(super) fn column_chart(vs: &[f64], lo: f64, hi: f64, w: usize, h: usize) -> Vec<String> {
    let vs = &vs[vs.len().saturating_sub(w)..];
    let n = vs.len().max(1);
    let cwid = (w / n).max(1);
    let gap = usize::from(cwid >= 3);
    let mut cells = vec![vec![' '; w]; h];
    let eighths: Vec<char> = " ▁▂▃▄▅▆▇█".chars().collect();
    for (i, &v) in vs.iter().enumerate() {
        let frac = if hi > lo { (v - lo) / (hi - lo) } else { 0.0 };
        let mut units = (frac * (h * 8) as f64 + 0.5) as i64;
        if v > lo && units == 0 {
            units = 1;
        }
        for (y, row) in cells.iter_mut().enumerate() {
            let from = ((h - 1 - y) * 8) as i64;
            let fill = (units - from).clamp(0, 8) as usize;
            for x in i * cwid..(i * cwid + cwid - gap).min(w) {
                row[x] = eighths[fill];
            }
        }
    }
    cells.into_iter().map(|r| r.into_iter().collect()).collect()
}

/// Values as a line h rows tall in w cells, drawn in braille dots: two
/// across and four down a cell (lineChart).
pub(super) fn line_chart(vs: &[f64], lo: f64, hi: f64, w: usize, h: usize) -> Vec<String> {
    let (dw, dh) = (w * 2, h * 4);
    let mut dots = vec![vec![false; dw]; dh];
    let pt = |i: usize| -> (usize, usize) {
        let x = if vs.len() > 1 {
            i * (dw - 1) / (vs.len() - 1)
        } else {
            0
        };
        let frac = if hi > lo {
            (vs[i] - lo) / (hi - lo)
        } else {
            0.5
        };
        let y = dh as i64 - 1 - (frac * (dh - 1) as f64 + 0.5) as i64;
        (x, y.clamp(0, dh as i64 - 1) as usize)
    };
    for i in 0..vs.len() {
        let (x0, y0) = pt(i);
        dots[y0][x0] = true;
        if i == 0 {
            continue;
        }
        let (x1, y1) = pt(i - 1);
        let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
        let steps = (x0 - x1).abs().max((y0 - y1).abs());
        for s in 1..steps {
            let x = x1 + (x0 - x1) * s / steps;
            let y = y1 + (y0 - y1) * s / steps;
            dots[y as usize][x as usize] = true;
        }
    }
    const BITS: [[u32; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
    (0..h)
        .map(|cy| {
            (0..w)
                .map(|cx| {
                    let mut r = 0;
                    for (dy, bits) in BITS.iter().enumerate() {
                        for (dx, bit) in bits.iter().enumerate() {
                            if dots[cy * 4 + dy][cx * 2 + dx] {
                                r |= bit;
                            }
                        }
                    }
                    if r == 0 {
                        ' '
                    } else {
                        char::from_u32(0x2800 + r).unwrap_or(' ')
                    }
                })
                .collect()
        })
        .collect()
}

/// A grid: a row a thing, a cell a value, denser and brighter the more it
/// is, its columns' labels over it when they fit and a key under it
/// (blockHeatmap).
pub(super) fn heatmap(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let (mut lw, mut n, mut hi) = (0, 0, 0.0f64);
    for it in &b.items {
        lw = lw.max(cw(&it.text));
        n = n.max(it.values.len());
        for &v in &it.values {
            hi = hi.max(v);
        }
    }
    if n == 0 {
        return Vec::new();
    }
    lw = lw.min(w / 3);
    let cwid = if lw + 1 + n * 3 <= w { 3 } else { 2 };
    n = n.min(w.saturating_sub(lw + 1) / cwid);
    let col = accent_or(&b.tone, cx.accent);
    const SHADES: [&str; 4] = ["░", "▒", "▓", "█"];
    let mut out = Vec::new();
    let heads = &b.columns[..n.min(b.columns.len())];
    if !b.columns.is_empty() && heads.iter().all(|s| cw(s) <= cwid - 1) {
        let mut h = ln([plain(blanks(lw + 1))]);
        for c in 0..n {
            let name = b.columns.get(c).cloned().unwrap_or_default();
            h.spans.extend(fit(ln([dim(name)]), cwid).spans);
        }
        out.push(ViewLine::plain(line(h.spans)));
    }
    for it in &b.items {
        let mut r = ln([paint(SUB, fit_str(&trunc(&it.text, lw), lw)), plain(" ")]);
        for c in 0..n {
            match it.values.get(c) {
                Some(&v) if v > 0.0 && hi > 0.0 => {
                    let i = ((v / hi * 4.0) as usize).min(3);
                    r.spans.push(paint(col, SHADES[i].repeat(cwid - 1)));
                    r.spans.push(plain(" "));
                }
                _ => {
                    r.spans.push(faint("·"));
                    r.spans.push(plain(blanks(cwid - 1)));
                }
            }
        }
        out.push(ViewLine::with(
            line(r.spans),
            ViewAct {
                link: it.link.clone(),
                ..Default::default()
            },
        ));
    }
    out.push(ViewLine::plain(ln([
        plain(blanks(lw + 1)),
        dim("less "),
        paint(col, "░▒▓█"),
        dim(format!(" more · most {}", num(round2(hi)))),
    ])));
    out
}

/// Digits and a few marks three rows tall, for a hero (bigGlyphs).
fn big_glyph(r: char) -> Option<[&'static str; 3]> {
    Some(match r {
        '0' => ["█▀█", "█ █", "█▄█"],
        '1' => ["▀█ ", " █ ", "▄█▄"],
        '2' => ["▀▀█", "█▀▀", "█▄▄"],
        '3' => ["▀▀█", " ▀█", "▄▄█"],
        '4' => ["█ █", "▀▀█", "  █"],
        '5' => ["█▀▀", "▀▀█", "▄▄█"],
        '6' => ["█▀▀", "█▀█", "█▄█"],
        '7' => ["▀▀█", "  █", "  █"],
        '8' => ["█▀█", "█▀█", "█▄█"],
        '9' => ["█▀█", "▀▀█", "▄▄█"],
        '.' | ',' => [" ", " ", "▄"],
        ':' => [" ", "▀", "▀"],
        '%' => ["█ ▄", " ▄▀", "▀ █"],
        '+' => ["   ", "▄█▄", " ▀ "],
        '-' | '−' => ["   ", "▀▀▀", "   "],
        '$' => ["▄█▀", "▀█▄", "▀█▀"],
        '/' => ["  █", " █ ", "█  "],
        ' ' => [" ", " ", " "],
        _ => return None,
    })
}

/// s three rows tall: digits and marks drawn big, letters small on its
/// last row, as units are ("212ms"); false when it has nothing to draw
/// big (bigText).
pub(super) fn big_text(s: &str) -> ([String; 3], bool) {
    let mut rows: [String; 3] = Default::default();
    let (mut big, mut was_big) = (false, false);
    for (i, r) in s.chars().enumerate() {
        let g = big_glyph(r);
        // A space between big glyphs, none between a unit's letters.
        if i > 0 && (g.is_some() || was_big) {
            for row in rows.iter_mut() {
                row.push(' ');
            }
        }
        was_big = g.is_some();
        let Some(g) = g else {
            let w = cw(r.encode_utf8(&mut [0; 4]));
            rows[0].push_str(&blanks(w));
            rows[1].push_str(&blanks(w));
            rows[2].push(r);
            continue;
        };
        big = big || r.is_ascii_digit();
        for (y, row) in rows.iter_mut().enumerate() {
            row.push_str(g[y]);
        }
    }
    (rows, big)
}

/// One number drawn big in its tone, what it is beside it, and its trend
/// under both (blockHero).
pub(super) fn hero(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let col = if b.tone == "quiet" {
        BRIGHT
    } else {
        accent_or(&b.tone, cx.accent)
    };
    let (big, ok) = big_text(&b.value);
    let mut out = Vec::new();
    if ok && cw(&big[0]) <= w {
        let bw = cw(&big[0]);
        let words = wrap(&b.text, 8.max(w.saturating_sub(bw + 3)));
        for (y, g) in big.iter().enumerate() {
            let mut l = ln([bolded(col, g.clone())]);
            if let Some(word) = words.get(y)
                && bw + 3 + cw(word) <= w
            {
                l = fit(l, bw);
                l.spans.push(plain("   "));
                l.spans.push(paint(SUB, word.clone()));
            }
            out.push(ViewLine::plain(l));
        }
    } else {
        out.push(ViewLine::plain(ln([bolded(col, trunc(&b.value, w))])));
        for l in wrap(&b.text, w) {
            out.push(ViewLine::plain(ln([paint(SUB, l)])));
        }
    }
    if b.trend.len() > 1 {
        out.push(ViewLine::plain(spark(&stretch(&b.trend, w), w, col)));
    }
    out
}

/// vs spread over n points, so a short trend fills the width (stretch).
pub(super) fn stretch(vs: &[f64], n: usize) -> Vec<f64> {
    if vs.len() >= n || vs.len() < 2 {
        return vs.to_vec();
    }
    (0..n)
        .map(|i| {
            let f = i as f64 * (vs.len() - 1) as f64 / (n - 1) as f64;
            let j = f as usize;
            if j >= vs.len() - 1 {
                return vs[vs.len() - 1];
            }
            vs[j] + (vs[j + 1] - vs[j]) * (f - j as f64)
        })
        .collect()
}

/// Words to notice: a bar in its tone down their left and its mark before
/// them (blockCallout).
pub(super) fn callout(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let (mut mark, mut col) = tone_mark(&b.tone);
    if b.tone.is_empty() {
        (mark, col) = ("✦", cx.accent);
    }
    wrap(&b.text, w.saturating_sub(4))
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let lead = if i == 0 {
                ln([paint(col, mark), plain(" ")])
            } else {
                ln([plain("  ")])
            };
            ViewLine::plain(cat([
                ln([paint(col, "▌"), plain(" ")]),
                lead,
                ln([paint(TEXT, l)]),
            ]))
        })
        .collect()
}

/// Lines of code as they are, numbered quietly down a rule (blockCode).
pub(super) fn code(b: &Block, w: usize) -> Vec<ViewLine> {
    let lines: Vec<&str> = b.text.split('\n').collect();
    let nw = lines.len().to_string().len();
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let n = (i + 1).to_string();
            ViewLine::plain(ln([
                faint(format!("{}{n} │ ", blanks(nw - n.len()))),
                paint(
                    TEXT,
                    rush_draw::truncate(l, w.saturating_sub(nw + 3), "…").into_owned(),
                ),
            ]))
        })
        .collect()
}

/// Words, wrapped (blockText).
pub(super) fn text(b: &Block, w: usize) -> Vec<ViewLine> {
    wrap(&b.text, w)
        .into_iter()
        .map(|l| ViewLine::plain(ln([paint(TEXT, l)])))
        .collect()
}
