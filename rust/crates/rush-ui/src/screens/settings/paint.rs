//! Small drawing helpers the settings pages share, in Go's terms
//! (ansi.Truncate, firstNonEmpty, pbox and sideBySide from projects.go:222,
//! :285).

use rush_core::text::{Color, Line};

use crate::style::*;

/// l cut to w cells with … when it's wider (ansi.Truncate(s, w, "…")).
pub fn trunc(l: Line, w: usize) -> Line {
    if l.width() <= w {
        return l;
    }
    rush_draw::truncate_line(&l, w, "…")
}

/// The first of xs that isn't empty (firstNonEmpty).
pub fn first_non_empty<'a>(xs: &[&'a str]) -> &'a str {
    xs.iter().copied().find(|x| !x.is_empty()).unwrap_or("")
}

/// on or off (onOffWord).
pub fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// a then b, as one line.
pub fn cat(a: Line, b: Line) -> Line {
    let mut a = a;
    join(&mut a, b);
    a
}

/// The edge colour of a box (cEdge).
pub const EDGE: Color = Color::Rgb(79, 73, 67);

/// A titled box w wide around body (pbox).
pub fn pbox(title: Line, meta: Line, body: &[Line], w: usize, lit: bool) -> Vec<Line> {
    let col = if lit { ORANGE } else { EDGE };
    let mut head = line([sp("╭─ ", col)]);
    join(&mut head, title);
    head.spans.push(plain(" "));
    let mut tail = Line::new();
    if meta.width() > 0 {
        tail.spans.push(plain(" "));
        join(&mut tail, meta);
        tail.spans.push(plain(" "));
    }
    let fill = (w as isize - head.width() as isize - tail.width() as isize - 2).max(1) as usize;
    let mut top = head;
    top.spans.push(sp("─".repeat(fill), col));
    join(&mut top, tail);
    top.spans.push(sp("─╮", col));
    let mut out = vec![fit(top, w)];
    for l in body {
        let mut r = line([sp("│", col), plain(" ")]);
        join(&mut r, fit(l.clone(), w.saturating_sub(4)));
        r.spans.push(plain(" "));
        r.spans.push(sp("│", col));
        out.push(r);
    }
    out.push(line([sp(
        format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
        col,
    )]));
    out
}

/// Two columns of lines, the left lw wide (sideBySide).
pub fn side_by_side(l: &[Line], r: &[Line], lw: usize) -> Vec<Line> {
    let n = l.len().max(r.len());
    (0..n)
        .map(|i| {
            let mut a = fit(l.get(i).cloned().unwrap_or_default(), lw);
            a.spans.push(plain(" "));
            join(&mut a, r.get(i).cloned().unwrap_or_default());
            a
        })
        .collect()
}

/// body filled out to n lines (pad).
pub fn pad_lines(mut body: Vec<Line>, n: usize) -> Vec<Line> {
    while body.len() < n {
        body.push(Line::new());
    }
    body
}
