//! Small drawing pieces the Session's views share. After Go's
//! internal/convo/style.go:205 (row), overview.go (fitTo :706, tokens :168, bar :178) and
//! internal/ui/textsel.go:216 (paintCols, the cutting part of it).

use rush_core::text::{Color, Line, Span, width};

use crate::style::*;

pub use super::dock::spread;

/// A row capw wide with right at its end, padded out to width, on ground
/// bg when there is one (convo's row): right is left out when it can't
/// fit, and left is cut short for it.
pub fn row(bg: Option<Color>, left: Line, right: Line, width_: usize, capw: usize) -> Line {
    let capw = if capw > width_ || capw == 0 {
        width_
    } else {
        capw
    };
    let (right, rw) = if right.width() + 4 > capw {
        (Line::new(), 0)
    } else {
        let w = right.width();
        (right, w)
    };
    let room = capw.saturating_sub(rw + if rw > 0 { 2 } else { 0 });
    let left = if left.width() > room {
        rush_draw::truncate_line(&left, room, "…")
    } else {
        left
    };
    let gap = capw.saturating_sub(left.width() + rw);
    let mut out = left;
    out.spans.push(plain(blanks(gap)));
    join(&mut out, right);
    out.spans.push(plain(blanks(width_.saturating_sub(capw))));
    match bg {
        Some(bg) => on_ground(out, width_, bg),
        None => out,
    }
}

/// Styled text padded or cut (with …) to exactly w cells (fitTo).
pub fn fit_to(l: Line, w: usize) -> Line {
    if l.width() > w {
        return rush_draw::truncate_line(&l, w, "…");
    }
    pad(l, w)
}

/// Cells [from, to) of l, a wide character cut in two becoming a blank.
pub fn cut(l: &Line, from: usize, to: usize) -> Line {
    let mut out = Line::new();
    let mut col = 0;
    for s in &l.spans {
        let mut text = String::new();
        for ch in s.text.chars() {
            let cw = width(ch.encode_utf8(&mut [0; 4]));
            if col >= from && col + cw <= to {
                text.push(ch);
            } else if col < to && col + cw > from {
                text.push_str(&" ".repeat((col + cw).min(to) - col.max(from)));
            }
            col += cw;
            if col >= to {
                break;
            }
        }
        if !text.is_empty() {
            out.spans.push(Span::new(text, s.style));
        }
        if col >= to {
            break;
        }
    }
    out
}

/// A token count as the overview says it: 840, 12k, 1.2M (overview.go:168).
pub fn tokens(n: i64) -> String {
    if n >= 1_000_000 {
        let s = format!("{:.1}", n as f64 / 1e6);
        return format!("{}M", s.trim_end_matches(".0"));
    }
    if n >= 1000 {
        return format!("{:.0}k", n as f64 / 1e3);
    }
    n.to_string()
}

/// "1 file", "3 files".
pub fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A bar frac full, w cells, in c (overview.go:178).
pub fn bar(frac: f64, w: usize, c: Color) -> Line {
    let frac = frac.clamp(0.0, 1.0);
    let n = ((frac * w as f64 + 0.5) as usize).min(w);
    line([sp("▰".repeat(n), c), sp("▱".repeat(w - n), FAINT)])
}

/// A span in the text colour.
pub fn text(s: impl Into<String>) -> Span {
    sp(s, TEXT)
}

pub fn dim(s: impl Into<String>) -> Span {
    sp(s, DIM)
}

pub fn faint(s: impl Into<String>) -> Span {
    sp(s, FAINT)
}

/// A section's head: ▾, its title, what it says, and a rule out to w
/// (overview.go's and changes.go's section).
pub fn section(title: &str, meta: &str, w: usize, less: usize) -> Line {
    let mut head = line([plain("  "), faint("▾"), plain(" "), spb(title, SUB)]);
    if !meta.is_empty() {
        head.spans.push(plain("  "));
        head.spans.push(dim(meta));
    }
    let n = w.saturating_sub(head.width() + less);
    head.spans.push(plain(" "));
    head.spans.push(faint("─".repeat(n)));
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_keeps_styles_and_halves_wide_chars() {
        let l = line([sp("ab", RED), sp("界c", BLUE)]);
        assert_eq!(cut(&l, 1, 3).plain(), "b ");
        assert_eq!(cut(&l, 2, 5).plain(), "界c");
        assert_eq!(cut(&l, 3, 5).plain(), " c");
        assert_eq!(cut(&l, 0, 2).spans[0].style.fg, RED);
    }

    #[test]
    fn tokens_as_go() {
        for (n, want) in [
            (840, "840"),
            (12_400, "12k"),
            (1_000_000, "1M"),
            (1_250_000, "1.2M"),
        ] {
            assert_eq!(tokens(n), want);
        }
    }

    #[test]
    fn row_drops_right_that_cant_fit() {
        let r = row(None, line([plain("abc")]), line([plain("xyz")]), 10, 6);
        assert_eq!(r.plain(), "abc       ");
        let r = row(None, line([plain("abc")]), line([plain("x")]), 10, 10);
        assert_eq!(r.plain(), "abc      x");
    }
}
