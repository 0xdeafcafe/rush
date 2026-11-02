//! Cutting text to a width with a tail, as Go rush does with
//! `ansi.Truncate(s, w, "…")` (charmbracelet/x/ansi truncate.go:53): text
//! that fits is kept whole; otherwise graphemes are kept while they fit in
//! `w` less the tail's width, and a wide char that would cross is dropped.

use std::borrow::Cow;

use rush_core::text::{self, Line, Span};
use unicode_segmentation::UnicodeSegmentation;

/// `s` cut to `w` cells, ending in `tail` when cut.
pub fn truncate<'a>(s: &'a str, w: usize, tail: &str) -> Cow<'a, str> {
    if text::width(s) <= w {
        return Cow::Borrowed(s);
    }
    let Some(room) = w.checked_sub(text::width(tail)) else {
        return Cow::Borrowed("");
    };
    let cut = keep(s, room);
    let mut out = String::with_capacity(cut + tail.len());
    out.push_str(&s[..cut]);
    out.push_str(tail);
    Cow::Owned(out)
}

/// `line` cut to `w` cells, ending in `tail` when cut; the tail takes the
/// style of the span it cuts, as the ANSI version leaves it in.
pub fn truncate_line(line: &Line, w: usize, tail: &str) -> Line {
    if line.width() <= w {
        return line.clone();
    }
    let mut out = Line {
        spans: Vec::new(),
        bg: line.bg,
    };
    let Some(mut room) = w.checked_sub(text::width(tail)) else {
        return out;
    };
    for s in &line.spans {
        let sw = s.width();
        if sw <= room {
            out.spans.push(s.clone());
            room -= sw;
            continue;
        }
        let cut = keep(&s.text, room);
        let mut t = String::with_capacity(cut + tail.len());
        t.push_str(&s.text[..cut]);
        t.push_str(tail);
        out.spans.push(Span::new(t, s.style));
        return out;
    }
    // Only zero-width text was left past the room: nothing to cut into.
    out
}

// The byte length of the graphemes of s that fit in w cells.
fn keep(s: &str, w: usize) -> usize {
    let mut used = 0;
    for (i, g) in s.grapheme_indices(true) {
        used += text::width(g);
        if used > w {
            return i;
        }
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::text::{Color, Style};

    #[test]
    fn truncates_like_go() {
        // (input, width, tail, want)
        let cases = [
            ("hello", 5, "…", "hello"),
            ("hello", 10, "…", "hello"),
            ("hello world", 5, "…", "hell…"),
            ("hello", 0, "…", ""),
            ("hello", 1, "…", "…"),
            ("hello", 3, "", "hel"),
            // A wide char that would cross the edge is dropped, not split.
            ("日本語です", 5, "…", "日本…"),
            ("日本語です", 4, "…", "日…"),
            ("a日本", 4, "…", "a日…"),
            ("ab日本", 4, "…", "ab…"),
            // Graphemes stay whole.
            ("e\u{301}e\u{301}e\u{301}", 2, "…", "e\u{301}…"),
            ("👍🏽👍🏽👍🏽", 5, "…", "👍🏽👍🏽…"),
        ];
        for (s, w, tail, want) in cases {
            assert_eq!(truncate(s, w, tail), want, "truncate({s:?}, {w}, {tail:?})");
        }
    }

    #[test]
    fn truncate_borrows_when_it_fits() {
        assert!(matches!(truncate("abc", 3, "…"), Cow::Borrowed(_)));
    }

    #[test]
    fn truncate_line_keeps_styles() {
        let red = Style::fg(Color::Rgb(255, 0, 0));
        let blue = Style::fg(Color::Rgb(0, 0, 255));
        let line = Line::from_spans(vec![Span::new("abc", red), Span::new("defgh", blue)]);
        let got = truncate_line(&line, 6, "…");
        assert_eq!(
            got.spans,
            vec![Span::new("abc", red), Span::new("de…", blue)]
        );
        let got = truncate_line(&line, 3, "…");
        assert_eq!(got.spans, vec![Span::new("ab…", red)]);
        assert_eq!(truncate_line(&line, 8, "…"), line);
        assert!(truncate_line(&line, 0, "…").spans.is_empty());
    }
}
