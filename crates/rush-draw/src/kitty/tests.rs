//! Kitty pictures: the escapes as the protocol spells them, the answer
//! to the query read, the choice of sharp or blocks, the cells drawn, and
//! the bytes a frame writes with a picture on screen.

use std::io::{self, Write};
use std::sync::Arc;

use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RRect;
use rush_core::text::{Color, Link, Span, Style};

use super::*;
use crate::canvas::Canvas;
use crate::{Headless, term};

#[test]
fn transmissions_are_spelled_as_the_protocol_says() {
    // (id, PNG bytes, cols, rows, the escapes)
    let cases: &[(u32, &[u8], usize, usize, &str)] = &[
        (
            7,
            b"png",
            4,
            2,
            "\x1b_Ga=T,U=1,f=100,t=d,i=7,c=4,r=2,C=1,q=2,m=0;cG5n\x1b\\",
        ),
        (
            0xabcdef,
            b"",
            1,
            1,
            "\x1b_Ga=T,U=1,f=100,t=d,i=11259375,c=1,r=1,C=1,q=2,m=0;\x1b\\",
        ),
    ];
    for &(id, png, cols, rows, want) in cases {
        assert_eq!(transmit(id, png, cols, rows), want, "{id}");
    }
    // More than a chunk's 4096 of base64 goes in chunks, every one but
    // the last saying more comes; only the first names the image.
    let png = vec![0u8; 3075]; // 4100 base64
    let got = transmit(9, &png, 3, 1);
    let parts: Vec<&str> = got.split("\x1b\\").filter(|p| !p.is_empty()).collect();
    assert_eq!(parts.len(), 2, "{got:?}");
    assert!(
        parts[0].starts_with("\x1b_Ga=T,U=1,f=100,t=d,i=9,c=3,r=1,C=1,q=2,m=1;"),
        "{:?}",
        &parts[0][..60]
    );
    assert_eq!(parts[0].split_once(';').unwrap().1.len(), 4096);
    assert_eq!(parts[1], "\x1b_Gm=0,q=2;AAAA");
}

#[test]
fn queries_and_deletions_are_spelled_as_the_protocol_says() {
    let cases: &[(String, &str)] = &[
        (
            QUERY.to_string(),
            "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\",
        ),
        (delete(42), "\x1b_Ga=d,d=I,i=42,q=2\x1b\\"),
    ];
    for (got, want) in cases {
        assert_eq!(got, want);
    }
}

#[test]
fn reads_the_answer_about_pictures() {
    // (what the terminal sent back, what it said)
    let cases: &[(&str, Option<bool>)] = &[
        ("\x1b_Gi=31;OK\x1b\\\x1b[?62;22c", Some(true)),
        (
            "\x1b]10;rgb:e2/dd/d3\x1b\\\x1b]11;rgb:11/10/0e\x07\x1b_Gi=31;OK\x1b\\\x1b[?1;2c",
            Some(true),
        ),
        ("\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\\x1b[?62c", Some(false)),
        ("\x1b[?62;22c", None),
        ("", None),
        ("\x1b_Gi=5;OK\x1b\\\x1b[?62c", None),
        ("\x1b_Gi=5;OK\x1b\\\x1b_Gi=31;OK\x1b\\", Some(true)),
        ("\x1b_Gi=31;OK", None),
    ];
    for &(got, want) in cases {
        assert_eq!(reply(got.as_bytes()), want, "{got:?}");
    }
}

#[test]
fn sharp_only_where_the_terminal_said_so() {
    type Case = (
        &'static [(&'static str, &'static str)],
        Option<bool>,
        bool,
        bool,
    );
    // (environment, the answer, asked, sharp)
    let cases: &[Case] = &[
        (&[("TERM", "xterm-ghostty")], Some(true), true, true),
        (&[("TERM", "xterm-256color")], None, true, false),
        (&[("TERM", "xterm-256color")], Some(false), true, false),
        // tmux and screen pass none of it through.
        (&[("TERM", "tmux-256color")], Some(true), false, false),
        (&[("TERM", "screen")], Some(true), false, false),
        (
            &[("TERM", "xterm-kitty"), ("TMUX", "/tmp/tmux-1/default,1,0")],
            Some(true),
            false,
            false,
        ),
        // RUSH_GRAPHICS says outright, unasked.
        (&[("RUSH_GRAPHICS", "kitty")], None, false, true),
        (
            &[("RUSH_GRAPHICS", "KITTY"), ("TMUX", "x")],
            None,
            false,
            true,
        ),
        (&[("RUSH_GRAPHICS", "blocks")], Some(true), false, false),
        (&[("RUSH_GRAPHICS", "sixel")], Some(true), false, false),
    ];
    for &(env, said, asked, sharp) in cases {
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        };
        assert_eq!(worth_asking(get), asked, "{env:?}");
        assert_eq!(choose(get, said), sharp, "{env:?} {said:?}");
    }
}

#[test]
fn ids_are_the_pictures_own() {
    let a = id(b"one", 4, 2);
    assert_eq!(a, id(b"one", 4, 2), "the same every time");
    assert_ne!(a, id(b"one", 4, 3), "another size is another picture");
    assert_ne!(a, id(b"two", 4, 2));
    for (d, c, r) in [
        (&b""[..], 0, 0),
        (&b"x"[..], 1, 1),
        (&b"long picture"[..], 297, 297),
    ] {
        let i = id(d, c, r);
        assert!(i > 0 && i <= 0xff_ffff, "{i:#x}");
    }
}

#[test]
fn placeholders_number_each_cell() {
    let rows = placeholders(0x12_3456, 3, 2);
    assert_eq!(rows.len(), 2);
    let d = |i: usize| char::from_u32(DIACRITICS[i]).unwrap();
    for (r, l) in rows.iter().enumerate() {
        assert_eq!(l.spans.len(), 1);
        assert_eq!(l.spans[0].style.fg, Color::Rgb(0x12, 0x34, 0x56));
        let want: String = (0..3).flat_map(|c| [PLACEHOLDER, d(r), d(c)]).collect();
        assert_eq!(l.spans[0].text, want);
        assert_eq!(l.width(), 3, "a cell each");
    }
    assert_eq!(d(0), '\u{305}');
    assert_eq!(d(296), '\u{1D244}');
}

/// A frame: w×h cells drawn by f.
fn frame(w: u16, h: u16, f: impl FnOnce(&mut Canvas)) -> (Buffer, Vec<Link>) {
    let mut b = Buffer::empty(RRect::new(0, 0, w, h));
    let mut l = vec![Link::NONE; w as usize * h as usize];
    f(&mut Canvas::new(&mut b, &mut l));
    (b, l)
}

/// The bytes written to have next on screen where prev was.
fn written(prev: &(Buffer, Vec<Link>), next: &(Buffer, Vec<Link>)) -> Vec<u8> {
    struct Into(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
    impl Write for Into {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let ups = term::diff(&prev.0, &prev.1, &next.0, &next.1, true);
    let got = std::rc::Rc::default();
    let mut out = CrosstermBackend::new(Into(std::rc::Rc::clone(&got)));
    term::write_frame(&mut out, &ups).unwrap();
    drop(out);
    got.take()
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// A picture is sent the first frame it's on screen and never again:
/// a frame after with it still there writes nothing more for it than
/// the same frame without it, and none when nothing changed.
#[test]
fn a_picture_is_sent_once() {
    let png: Arc<[u8]> = Arc::from(&b"a picture sent once"[..]);
    let pid = id(&png, 6, 3);
    let rows = picture(pid, png, 6, 3);
    let draw = |clock: &str, with: bool| {
        frame(20, 4, |c| {
            c.put_str(0, 0, clock, Style::default(), 20);
            if with {
                for (i, r) in rows.iter().enumerate() {
                    c.put_line(2, 1 + i as u16, r, 18);
                }
            }
        })
    };
    let blank = frame(20, 4, |_| {});
    let first = written(&blank, &draw("12:00", true));
    assert_eq!(count(&first, b"\x1b_Ga=T"), 1, "sent with the first frame");
    assert_eq!(count(&first, "\u{10EEEE}".as_bytes()), 18, "its cells");
    // Warm: nothing changed, nothing written.
    let warm = written(&draw("12:00", true), &draw("12:00", true));
    assert_eq!(warm.len(), 0);
    // Warm, the clock moved: the bytes for the clock alone.
    let moved = written(&draw("12:00", true), &draw("12:01", true));
    let alone = written(&draw("12:00", false), &draw("12:01", false));
    assert_eq!(count(&moved, b"\x1b_G"), 0);
    assert_eq!(moved.len(), alone.len(), "0 extra bytes for the picture");
    // Scrolled a row: its cells are written again, the image isn't.
    let scrolled = frame(20, 4, |c| {
        c.put_str(0, 0, "12:00", Style::default(), 20);
        for (i, r) in rows.iter().enumerate() {
            c.put_line(2, i as u16, r, 18);
        }
    });
    let s = written(&draw("12:00", true), &scrolled);
    assert_eq!(count(&s, b"\x1b_G"), 0, "not sent again");
    // Another program had the terminal: sent again, once.
    screen_lost();
    let again = written(&blank, &draw("12:00", true));
    assert_eq!(count(&again, b"\x1b_Ga=T"), 1);
}

/// A picture's cells keep their id's colour; in another (faded since)
/// they're blank.
#[test]
fn a_faded_picture_is_blank() {
    let png: Arc<[u8]> = Arc::from(&b"a faded picture"[..]);
    let pid = id(&png, 2, 1);
    let rows = picture(pid, png, 2, 1);
    let mut s = Headless::new(4, 2);
    let mut faded = rows[0].clone();
    faded.spans[0].style.fg = Color::Rgb(1, 2, 3);
    s.draw(|c| {
        c.put_line(0, 0, &rows[0], 4);
        c.put_line(0, 1, &faded, 4);
    });
    let plain = s.plain();
    let (top, bottom) = plain.split_once('\n').unwrap_or((&plain, ""));
    assert_eq!(top.matches(PLACEHOLDER).count(), 2, "{plain:?}");
    assert_eq!(bottom.trim(), "", "{plain:?}");
}

/// A picture's cells carry rush's own link, for a click, but no OSC 8:
/// the terminal neither underlines them nor offers its menu.
#[test]
fn no_osc8_around_a_picture() {
    let png: Arc<[u8]> = Arc::from(&b"a linked picture"[..]);
    let pid = id(&png, 3, 1);
    let mut row = picture(pid, png, 3, 1).remove(0);
    let to = "rush:message/t1/image/1";
    for s in &mut row.spans {
        s.style.link = Link::quiet(to);
    }
    row.spans.push(Span::new(
        "chip",
        Style::default().linked("https://x.example"),
    ));
    let blank = frame(10, 1, |_| {});
    let next = frame(10, 1, |c| _ = c.put_line(0, 0, &row, 10));
    assert_eq!(next.1[0].url().as_deref(), Some(to), "a click finds it");
    let out = String::from_utf8_lossy(&written(&blank, &next)).into_owned();
    assert_eq!(
        count(out.as_bytes(), b"\x1b]8;;"),
        2,
        "the chip's alone: {out:?}"
    );
    assert!(!out.contains(to), "{out:?}");
    let mut h = Headless::new(10, 1);
    h.draw(|c| _ = c.put_line(0, 0, &row, 10));
    assert_eq!(h.link_at(0, 0).url().as_deref(), Some(to));
    let ansi = h.ansi();
    assert!(!ansi.contains(to), "{ansi:?}");
    assert!(ansi.contains("\x1b]8;;https://x.example"), "{ansi:?}");
}
