//! Drawing through the public API, checked headless.

use crate::{Headless, Rect};
use rush_core::text::{Color, Line, Mods, Span, Style};

fn plain(w: u16, h: u16, f: impl FnOnce(&mut crate::Canvas)) -> String {
    let mut s = Headless::new(w, h);
    s.draw(f);
    s.plain()
}

#[test]
fn puts_lines_clipped_by_width() {
    // (text, x, max_width, want row) on a 10-wide screen
    let cases = [
        ("hello", 0, 10, "hello"),
        ("hello world", 0, 5, "hello"),
        ("hello world", 3, 20, "   hello w"),
        ("日本語", 0, 5, "日本"),
        ("日本語", 0, 6, "日本語"),
        ("日本語", 1, 4, " 日本"),
        ("a日本", 0, 2, "a"),
        ("e\u{301}tude", 0, 3, "e\u{301}tu"),
        ("tab\there", 0, 10, "tabhere"),
        ("a\r\nb\x1b[0mc\x7f", 0, 10, "ab[0mc"),
        ("ab日", 7, 3, "       ab"),
        ("👍🏽ok", 0, 3, "👍🏽o"),
        ("x", 10, 5, ""),
    ];
    for (t, x, max, want) in cases {
        let got = plain(10, 1, |c| {
            let n = c.put_line(x, 0, &Line::from_spans(vec![Span::plain(t)]), max);
            assert!(n <= max, "{t:?} wrote {n} > {max}");
        });
        assert_eq!(got, want, "put_line({t:?}, x={x}, max={max})");
    }
}

#[test]
fn reports_cells_written() {
    let mut s = Headless::new(10, 1);
    s.draw(|c| {
        assert_eq!(c.put_str(0, 0, "日本語", Style::default(), 5), 4);
        assert_eq!(c.put_str(8, 0, "abc", Style::default(), 5), 2);
        assert_eq!(c.put_str(0, 3, "abc", Style::default(), 5), 0);
    });
}

#[test]
fn overwriting_half_a_wide_char_blanks_it() {
    let got = plain(6, 1, |c| {
        c.put_str(0, 0, "日本語", Style::default(), 6);
        c.put_str(3, 0, "x", Style::default(), 1);
    });
    assert_eq!(got, "日 x語");
    let got = plain(6, 1, |c| {
        c.put_str(0, 0, "日本語", Style::default(), 6);
        c.fill(Rect::new(1, 0, 1, 1), Style::default());
    });
    assert_eq!(got, "  本語");
}

#[test]
fn line_background_fills_the_rest() {
    let bg = Color::Rgb(1, 2, 3);
    let mut line = Line::from_spans(vec![Span::new("ab", Style::fg(Color::Indexed(1)))]);
    line.bg = bg;
    let mut s = Headless::new(6, 1);
    s.draw(|c| {
        assert_eq!(c.put_line(0, 0, &line, 4), 4);
    });
    assert_eq!(s.plain(), "ab");
    assert_eq!(
        s.ansi(),
        "\x1b[0;31;48;2;1;2;3mab\x1b[0;48;2;1;2;3m  \x1b[0m"
    );
}

#[test]
fn ansi_marks_style_changes() {
    let bold = Style::fg(Color::Rgb(255, 0, 0)).bold();
    let line = Line::from_spans(vec![
        Span::plain("a "),
        Span::new("b", bold),
        Span::new("c", bold),
        Span::new("日", Style::fg(Color::Indexed(200))),
        Span::new(
            "d",
            Style {
                mods: Mods::DIM.with(Mods::UNDERLINE),
                ..Style::default()
            },
        ),
    ]);
    let mut s = Headless::new(12, 2);
    s.draw(|c| {
        c.put_line(0, 0, &line, 12);
        c.put_str(0, 1, "plain", Style::default(), 12);
    });
    assert_eq!(s.plain(), "a bc日d\nplain");
    assert_eq!(
        s.ansi(),
        "\x1b[0ma \x1b[0;1;38;2;255;0;0mbc\x1b[0;38;5;200m日\x1b[0;2;4md\x1b[0m\n\x1b[0mplain"
    );
}

#[test]
fn fill_and_cursor() {
    let mut s = Headless::new(4, 3);
    s.draw(|c| {
        c.fill(
            Rect::new(1, 1, 10, 10),
            Style::default().on(Color::Indexed(4)),
        );
        c.set_cursor(2, 1);
        assert_eq!(c.row_text(1), "    ");
    });
    assert_eq!(s.cursor(), Some((2, 1)));
    assert_eq!(
        s.ansi(),
        "\n\x1b[0m \x1b[0;44m   \x1b[0m\n\x1b[0m \x1b[0;44m   \x1b[0m"
    );
    // A new frame starts blank.
    s.draw(|_| {});
    assert_eq!(s.plain(), "\n\n");
    assert_eq!(s.cursor(), None);
}

#[test]
fn rect_intersect() {
    let a = Rect::new(0, 0, 10, 5);
    assert_eq!(a.intersect(Rect::new(5, 2, 10, 10)), Rect::new(5, 2, 5, 3));
    assert!(a.intersect(Rect::new(10, 0, 2, 2)).is_empty());
    assert!(a.contains(9, 4));
    assert!(!a.contains(10, 4));
}

/// The memory a screen costs: ratatui keeps two buffers (this frame and
/// the last) of one cell per position. Run with --nocapture to see it.
#[test]
fn frame_memory() {
    use ratatui::buffer::{Buffer, Cell};
    let cell = std::mem::size_of::<Cell>();
    let (w, h) = (200usize, 50usize);
    let pair = 2 * w * h * cell;
    let b = Buffer::empty(ratatui::layout::Rect::new(0, 0, w as u16, h as u16));
    assert_eq!(b.content.capacity() * cell * 2, pair);
    println!(
        "ratatui Cell: {cell} bytes; one 200x50 frame pair: {pair} bytes ({} KiB)",
        pair / 1024
    );
    assert!(cell <= 48, "Cell grew to {cell} bytes");
}

// Go's rows carry OSC 8 around a link's words (ui/finish.go:400 link);
// --render prints them as they are.
#[test]
fn ansi_wraps_links_in_osc8() {
    let u = "https://example.com/a";
    let line = Line::from_spans(vec![
        Span::plain("see "),
        Span::new("docs", Style::fg(Color::Indexed(4)).linked(u)),
        Span::plain(" ok"),
    ]);
    let mut s = Headless::new(12, 2);
    s.draw(|c| {
        c.put_line(0, 0, &line, 12);
        c.put_str(0, 1, "end", Style::default().linked(u), 12);
    });
    assert_eq!(s.plain(), "see docs ok\nend");
    assert_eq!(s.link_at(4, 0).url().as_deref(), Some(u));
    assert!(s.link_at(3, 0).is_none() && s.link_at(8, 0).is_none());
    assert_eq!(
        s.ansi(),
        "\x1b[0msee \x1b]8;;https://example.com/a\x1b\\\x1b[0;34mdocs\x1b]8;;\x1b\\\x1b[0m ok\n\
         \x1b]8;;https://example.com/a\x1b\\\x1b[0mend\x1b]8;;\x1b\\"
    );
}

#[test]
fn fill_drops_links() {
    let mut s = Headless::new(4, 1);
    s.draw(|c| {
        c.put_str(0, 0, "abcd", Style::default().linked("rush:agent/x"), 4);
        c.fill(Rect::new(1, 0, 2, 1), Style::default());
    });
    assert!(!s.link_at(0, 0).is_none());
    assert!(s.link_at(1, 0).is_none() && s.link_at(2, 0).is_none());
    assert!(!s.link_at(3, 0).is_none());
}

/// A screen in memory has no frame on screen to keep: it's drawn whole.
#[test]
fn keeping_needs_a_frame_on_screen() {
    let got = plain(4, 1, |c| {
        assert!(!c.can_keep());
        c.keep();
        c.put_str(0, 0, "ab", Style::default(), 4);
    });
    assert_eq!(got, "ab");
}
