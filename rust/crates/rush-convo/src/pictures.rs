//! The pictures a conversation draws in place of their chips: a step's
//! image as a small thumbnail under it, and the images you sent as a row
//! of them under your message. Making one decodes an image, so the UI
//! makes them (set_maker) off its own thread and says when one's made
//! (made): every turn is drawn again then, as a link's lookup landing
//! does. Until a picture's made, or when it can't be, its chip stands in.
//! After Go's internal/convo/picture.go (PictureDrawn :58, thumbRows :186,
//! framed :208, Tile :219) and thumb.go (thumbW, thumbH :31, thumbOf :55).

use std::sync::{Arc, PoisonError, RwLock};

use rush_core::text::{Line, Span};

/// A step's thumbnail at most: 32 cells by 12 rows of half blocks, 32 by
/// 24 pixels (thumbW, thumbH).
pub const THUMB_W: usize = 32;
pub const THUMB_H: usize = 12;

/// How a picture is drawn: a step's thumbnail in half blocks, or an image
/// fitted to a box of cells at the terminal's best (Picture).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Look {
    Thumb,
    Fit,
}

/// Makes the image at path w cells by h rows as look says: its rows once
/// made (none when it can't be read), None until then. It never decodes
/// while a frame is drawn.
pub type Maker = fn(path: &str, w: usize, h: usize, look: Look) -> Option<Arc<Vec<Line>>>;

static MAKER: RwLock<Option<Maker>> = RwLock::new(None);

/// Has pictures made by f; without one every image is its chip.
pub fn set_maker(f: Maker) {
    *MAKER.write().unwrap_or_else(PoisonError::into_inner) = Some(f);
}

/// A picture was made: every turn is drawn again (lookupsGen).
pub fn made() {
    crate::linkinfo::redraw();
}

/// The image at path drawn as look says, w by h, when it's made and could
/// be read.
pub(crate) fn picture(path: &str, w: usize, h: usize, look: Look) -> Option<Arc<Vec<Line>>> {
    if path.is_empty() {
        return None;
    }
    let f = (*MAKER.read().unwrap_or_else(PoisonError::into_inner))?;
    f(path, w, h, look).filter(|r| !r.is_empty())
}

/// A message's images as thumbnails a few rows tall, side by side in w
/// cells, each opening whole when clicked as its chip does
/// (rush:message/<ref>/image/<n>); None until every one is made and
/// readable, so their chips stand in till then (thumbRows).
pub(crate) fn thumb_rows(r: &str, paths: &[String], w: usize) -> Option<Vec<Line>> {
    if paths.is_empty() {
        return None;
    }
    let mut pics = Vec::with_capacity(paths.len());
    for (i, p) in paths.iter().enumerate() {
        // The same height each, so a row of them lines up; small, as the
        // chips they stand for: opened, an image shows whole.
        let rows = picture(p, 8.max(32.min(w.saturating_sub(2))), 3, Look::Fit)?;
        let mut pic = framed(&rows);
        let link = rush_core::text::Link::to(&format!("rush:message/{r}/image/{}", i + 1));
        for l in &mut pic {
            for s in &mut l.spans {
                s.style.link = link;
            }
        }
        pics.push(pic);
    }
    Some(tile(&pics, w, 2).0)
}

/// A picture in a faint rounded edge, so it sits apart from the message
/// around it (framed).
fn framed(rows: &[Line]) -> Vec<Line> {
    let w = rows.first().map_or(0, Line::width);
    let faint = |s: String| Span::new(s, crate::render::style::C_FAINT);
    let bar = "─".repeat(w);
    let mut out = Vec::with_capacity(rows.len() + 2);
    out.push(Line::from_spans(vec![faint(format!("╭{bar}╮"))]));
    for r in rows {
        let mut l = Line::from_spans(vec![faint("│".into())]);
        l.spans.extend(r.spans.iter().cloned());
        l.spans.push(faint("│".into()));
        out.push(l);
    }
    out.push(Line::from_spans(vec![faint(format!("╰{bar}╯"))]));
    out
}

/// Pictures side by side, gap cells apart, wrapping to w cells; each row
/// of them top-aligned, a blank row between. Where each landed: x, y,
/// width and height (Tile).
pub fn tile(pics: &[Vec<Line>], w: usize, gap: usize) -> (Vec<Line>, Vec<[usize; 4]>) {
    let pw = |p: &Vec<Line>| p.first().map_or(0, Line::width);
    let mut rows: Vec<Line> = Vec::new();
    let mut at = vec![[0usize; 4]; pics.len()];
    let mut group: Vec<usize> = Vec::new();
    let mut x = 0;
    let flush = |rows: &mut Vec<Line>, group: &mut Vec<usize>| {
        if !rows.is_empty() {
            rows.push(Line::new()); // a row of them apart from the last
        }
        let h = group.iter().map(|&i| pics[i].len()).max().unwrap_or(0);
        for y in 0..h {
            let mut l = Line::new();
            for (k, &i) in group.iter().enumerate() {
                if k > 0 {
                    l.spans.push(Span::plain(" ".repeat(gap)));
                }
                match pics[i].get(y) {
                    Some(r) => l.spans.extend(r.spans.iter().cloned()),
                    None => l.spans.push(Span::plain(" ".repeat(pw(&pics[i])))),
                }
            }
            rows.push(l);
        }
        group.clear();
    };
    for (i, p) in pics.iter().enumerate() {
        if p.is_empty() {
            continue;
        }
        let n = pw(p);
        if !group.is_empty() && x + gap + n > w {
            flush(&mut rows, &mut group);
            x = 0;
        }
        if !group.is_empty() {
            x += gap;
        }
        let mut y = rows.len();
        if y > 0 {
            y += 1; // the blank row flush puts first
        }
        at[i] = [x, y, n, p.len()];
        group.push(i);
        x += n;
    }
    if !group.is_empty() {
        flush(&mut rows, &mut group);
    }
    (rows, at)
}

#[cfg(test)]
mod tests {
    use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
    use rush_core::text::{Line, Span};
    use rush_core::tool::{Call, Input, Kind, Output};

    use super::*;
    use crate::{Options, Renderer, Session};

    /// Only these tests' own files are made: set_maker stays set for the
    /// whole test process, so every other test's images stay chips.
    fn maker(path: &str, w: usize, h: usize, look: Look) -> Option<Arc<Vec<Line>>> {
        let row = |c: &str, n: usize| Line::from_spans(vec![Span::plain(c.repeat(n))]);
        match (path, look) {
            ("/pics-test/read.png", Look::Thumb) => {
                assert_eq!((w, h), (THUMB_W, THUMB_H));
                Some(Arc::new(vec![row("▀", 6), row("▄", 6)]))
            }
            ("/pics-test/sent.png", Look::Fit) => {
                assert_eq!(h, 3);
                Some(Arc::new(vec![row("█", 4); 2]))
            }
            ("/pics-test/later.png", _) => None,
            _ => Some(Arc::new(Vec::new())),
        }
    }

    fn rows(s: &Session) -> Vec<crate::RenderedLine> {
        let o = Options {
            width: 80,
            now: 10_000,
            verbose: true,
            ..Options::default()
        };
        let mut r = Renderer::new();
        let total = r.count(s, &o);
        r.window(s, &o, 0, total);
        r.rows().into_iter().cloned().collect()
    }

    fn drawn(s: &Session) -> String {
        rows(s)
            .iter()
            .map(|l| l.line.plain())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Where the cells drawn as `text` on a row link to.
    fn link_of(ls: &[crate::RenderedLine], text: &str) -> Option<String> {
        let l = ls.iter().find(|l| l.line.plain().contains(text))?;
        let col = rush_core::text::width(&l.line.plain()[..l.line.plain().find(text)?]);
        rush_core::text::link_at(&l.line, col).map(|u| u.to_string())
    }

    /// A step's image is a thumbnail under it and your message's images a
    /// framed row of them, in place of their chips; one not made yet, or
    /// that can't be, stays its chip (render.go:3176, picture.go:186).
    #[test]
    fn pictures_stand_in_for_their_chips() {
        set_maker(maker);
        let mut s = Session::new();
        s.sent(
            "look at this".into(),
            vec!["/pics-test/sent.png".into()],
            1_000,
        );
        let read = |s: &mut Session, id: &str, path: &str, t: i64| {
            let call = Call {
                id: id.into(),
                name: "Read".into(),
                kind: Kind::Read,
                input: Input {
                    path: path.into(),
                    ..Input::default()
                },
                ..Call::default()
            };
            s.apply(
                &Event::Message(Message {
                    role: "assistant".into(),
                    id: format!("m-{id}"),
                    parts: vec![Part {
                        kind: PartKind::ToolCall,
                        call: Some(Box::new(call)),
                        ..Part::default()
                    }],
                    ..Message::default()
                }),
                t,
            );
            s.apply(
                &Event::Message(Message {
                    role: "user".into(),
                    id: format!("r-{id}"),
                    parts: vec![Part {
                        kind: PartKind::ToolResult,
                        output: Some(Box::new(Output {
                            call_id: id.into(),
                            text: "[image]".into(),
                            ..Output::default()
                        })),
                        ..Part::default()
                    }],
                    ..Message::default()
                }),
                t + 1,
            );
        };
        read(&mut s, "r1", "/pics-test/read.png", 2_000);
        read(&mut s, "r2", "/pics-test/later.png", 3_000);
        s.apply(&Event::TurnEnd(TurnEnd::default()), 4_000);
        let f = drawn(&s);
        assert!(f.contains("╭────╮") && f.contains("│████│"), "sent: {f}");
        assert!(!f.contains("▣ sent.png"), "its chip gives way: {f}");
        assert!(f.contains("▀▀▀▀▀▀\n") || f.contains("▀▀▀▀▀▀ "), "read: {f}");
        assert!(!f.contains("▣ read.png"), "its chip gives way: {f}");
        assert!(f.contains("▣ later.png"), "not made yet: {f}");
        // Each opens when clicked, as its chip does: yours its message's
        // image, a step's its file (picture.go:199, render.go:3200).
        let ls = rows(&s);
        let r = ls
            .iter()
            .find(|l| l.line.plain().contains("look at this"))
            .map(|l| l.r#ref.clone())
            .unwrap();
        assert_eq!(
            link_of(&ls, "│████│"),
            Some(format!("rush:message/{r}/image/1"))
        );
        assert_eq!(
            link_of(&ls, "▀▀▀▀▀▀").as_deref(),
            Some("file:///pics-test/read.png")
        );
        assert_eq!(
            link_of(&ls, "▣ later.png").as_deref(),
            Some("file:///pics-test/later.png")
        );
    }
}
