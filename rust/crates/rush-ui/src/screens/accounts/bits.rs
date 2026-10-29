//! Small pieces the sheets draw with. After Go's internal/ui
//! settings_keys.go:118 (keysControls), question.go (keycap :31, chip
//! :40, qCapOn :24, qInk :26), style.go:84 (qCap) and onboard.go:155
//! (keyRows).

use rush_core::text::{Color, Line};

use crate::style::*;

/// A lit keycap's ground (qCapOn).
pub const CAP_ON: Color = Color::Rgb(217, 119, 87);
/// An unlit keycap's ground (qCap).
pub const CAP: Color = Color::Rgb(48, 44, 40);
/// Dark text on a lit chip or keycap (qInk).
pub const INK: Color = Color::Rgb(33, 28, 22);

/// "key label" pairs, the key bold, wrapped to w rather than dropped
/// (keysControls).
pub fn keys_controls(w: usize, pairs: &[(&str, &str)]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut l = Line::new();
    for (k, what) in pairs {
        let item = line([spb(*k, TEXT), plain(" "), sp(*what, TEXT)]);
        if !l.spans.is_empty() && l.width() + 3 + item.width() > w {
            out.push(std::mem::take(&mut l));
        }
        if !l.spans.is_empty() {
            l.spans.push(plain(" · "));
        }
        join(&mut l, item);
    }
    if !l.spans.is_empty() {
        out.push(l);
    }
    out
}

/// A key drawn as a key: " 1 " on its own ground, lit when on (keycap).
pub fn keycap(k: &str, on: bool) -> rush_core::text::Span {
    let k = rush_convo::render::key_word(k);
    if on {
        let mut s = spb(format!(" {k} "), INK);
        s.style.bg = CAP_ON;
        return s;
    }
    let mut s = sp(format!(" {k} "), SUB);
    s.style.bg = CAP;
    s
}

/// "key  what" rows, each key a lit keycap, the keys in a column
/// (keyRows).
pub fn key_rows(rows: &[(&str, &str)]) -> Vec<Line> {
    let kw = rows
        .iter()
        .map(|r| rush_core::text::width(&rush_convo::render::key_word(r.0)))
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|(k, what)| {
            let pad = kw - rush_core::text::width(&rush_convo::render::key_word(k)) + 2;
            line([keycap(k, true), plain(blanks(pad)), sp(*what, TEXT)])
        })
        .collect()
}

/// A line cut or padded to exactly w cells.
pub fn fit_w(l: Line, w: usize) -> Line {
    pad(fit(l, w), w)
}
