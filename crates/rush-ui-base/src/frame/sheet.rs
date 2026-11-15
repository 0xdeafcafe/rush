//! Sheets: a panel over the screen that takes every key until it closes,
//! and the pieces sheets are drawn with. After Go's internal/ui/sheet.go
//! (sheet :16, sheetWidth :38, mouseSheet :47, sheetView :62, sheetTitle
//! :79, sheetTabs :84, sheetRow :97, window :105, textField :114, center
//! :134).

use std::any::Any;
use std::time::Instant;

use rush_core::text::Line;

use super::Out;
use super::overlayclick::MouseEv;
use super::{Mouse, Press};
use crate::model::Model;
use crate::style::*;

/// A panel over the screen: /fork's options, /rewind, /plugins, a
/// picker. It has every key until it closes (`Out::Close`, or another
/// sheet opened in its place). It's taken out of the model while one of
/// its methods runs.
pub trait Sheet: Any {
    /// The inside, w wide and at most h tall.
    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line>;
    fn key(&mut self, m: &mut Model, k: &Press) -> Out;
    /// The panel's width it wants; 0 for the usual (112), and never more
    /// than the screen less 6.
    fn width(&self, _m: &Model) -> usize {
        0
    }
    /// The mouse inside the panel, x and y within its body as drawn.
    /// Pass leaves it to the frame, which presses the keys a click stands
    /// for (overlayclick.rs): a row is ↑ or ↓ then enter, a key in a row
    /// of keys that key, outside the box esc.
    fn mouse(&mut self, _m: &mut Model, _ev: MouseEv, _x: i32, _y: i32) -> Out {
        Out::Pass
    }
    fn paste(&mut self, _m: &mut Model, _text: &str) -> Out {
        Out::Pass
    }
    /// When to be woken with nothing else happening; None sleeps.
    fn wake_at(&self, _m: &Model) -> Option<Instant> {
        None
    }
    /// A small box by what opened it rather than the panel in the middle:
    /// its title in its top edge, no blank rows, nothing behind it faded
    /// while it sits by a click (Rush-only; a GitHub link's popover).
    fn popover(&self, _m: &Model) -> Option<Popover> {
        None
    }
}

/// How a sheet drawn as a popover sits (Sheet::popover).
#[derive(Clone, Debug, Default)]
pub struct Popover {
    /// In the top edge, on the left: what it's for.
    pub title: Line,
    /// In the top edge, on the right: where it stands.
    pub state: Line,
    /// Its width, edges and all; never more than the screen.
    pub width: usize,
    /// The screen cell it was opened from: the box hangs under it, or
    /// sits above it when there's no room below. None centres it.
    pub at: Option<(usize, usize)>,
}

/// A popover's box: the title and state in its top edge, its rows each
/// between the edges with a space either side, bw wide (Rust-only).
pub fn popover_box(p: &Popover, body: &[Line], bw: usize) -> Vec<Line> {
    let edge = |s: String| sp(s, DIM);
    let inner = bw.saturating_sub(4);
    // ╭ title ─── state ╮: the state kept whole, the title cut first.
    let state_w = p.state.width();
    let state_room = if state_w > 0 { state_w + 2 } else { 0 };
    let title_room = bw.saturating_sub(6 + state_room);
    let title = rush_draw::truncate_line(&p.title, title_room, "…");
    let mut top = line([edge("╭ ".into())]);
    let tw = title.width();
    join(&mut top, title);
    let dashes = bw.saturating_sub(3 + tw + state_room + usize::from(tw > 0));
    if tw > 0 {
        top.spans.push(plain(" "));
    }
    top.spans.push(edge("─".repeat(dashes)));
    if state_w > 0 && state_room + 6 <= bw {
        top.spans.push(plain(" "));
        join(&mut top, p.state.clone());
        top.spans.push(plain(" "));
    } else {
        top.spans.push(edge("─".repeat(state_room)));
    }
    top.spans.push(edge("╮".into()));
    let mut out = vec![top];
    for l in body {
        let mut row = line([edge("│".into())]);
        let mut inside = line([plain(" ")]);
        join(&mut inside, fit(l.clone(), inner));
        inside.spans.push(plain(" "));
        join(&mut row, on_ground(inside, inner + 2, PANEL_BG));
        row.spans.push(edge("│".into()));
        out.push(row);
    }
    out.push(line([edge(format!(
        "╰{}╯",
        "─".repeat(bw.saturating_sub(2))
    ))]));
    out
}

/// Where a popover's box of bw by bh goes on a screen w by h: under the
/// cell it was opened from, above it when there's no room below, kept on
/// screen; centred when it wasn't opened from a cell (Rust-only).
pub fn popover_at(
    at: Option<(usize, usize)>,
    bw: usize,
    bh: usize,
    w: usize,
    h: usize,
) -> (usize, usize) {
    let centre = (w.saturating_sub(bw) / 2, h.saturating_sub(bh) / 2);
    let Some((x, y)) = at else {
        return centre;
    };
    let left = x.saturating_sub(2).min(w.saturating_sub(bw));
    let top = if y + 1 + bh <= h {
        y + 1
    } else if y >= bh {
        y - bh
    } else {
        h.saturating_sub(bh)
    };
    (left, top)
}

impl Model {
    /// Opens s over the screen, closing any sheet that was.
    pub fn open_sheet(&mut self, s: Box<dyn Sheet>) {
        self.sheet = Some(s);
    }

    /// The open sheet, when it's a T: to change it from outside (more
    /// rows found off the UI thread).
    pub fn sheet_as<T: Sheet>(&mut self) -> Option<&mut T> {
        let s: &mut dyn Any = self.sheet.as_mut()?.as_mut();
        s.downcast_mut::<T>()
    }

    /// The sheet's panel's width on this screen (sheetWidth).
    pub(crate) fn sheet_width(&self, want: usize) -> usize {
        let want = if want > 0 { want } else { 112 };
        40.max(self.w.saturating_sub(6).min(want))
    }

    /// Hands a key to the open sheet: closed when it says so.
    #[doc(hidden)]
    pub fn sheet_key(&mut self, k: &Press) -> Out {
        let Some(mut s) = self.sheet.take() else {
            return Out::Pass;
        };
        let out = s.key(self, k);
        self.put_back(s, out)
    }

    // The sheet goes back unless it closed, or opened another.
    fn put_back(&mut self, s: Box<dyn Sheet>, out: Out) -> Out {
        if out == Out::Close || self.sheet.is_some() {
            return if out == Out::Pass { Out::Idle } else { out };
        }
        self.sheet = Some(s);
        out
    }

    pub(crate) fn sheet_paste(&mut self, text: &str) -> Out {
        let Some(mut s) = self.sheet.take() else {
            return Out::Pass;
        };
        let out = s.paste(self, text);
        self.put_back(s, out)
    }

    /// The mouse for the open sheet, at x, y on screen (sheetMouse).
    pub(crate) fn sheet_mouse(&mut self, ev: MouseEv, e: &Mouse) -> Out {
        let Some(mut s) = self.sheet.take() else {
            return Out::Pass;
        };
        let (x, y) = (
            e.x as i32 - self.sheet_at.0 as i32,
            e.y as i32 - self.sheet_at.1 as i32,
        );
        let out = s.mouse(self, ev, x, y);
        if out == Out::Pass {
            self.sheet = Some(s);
            return Out::Pass;
        }
        self.put_back(s, out)
    }

    /// Draws the open sheet over the frame, and keeps where its body
    /// landed for the mouse (sheetView).
    pub(crate) fn sheet_view(&mut self, base: &mut [Line]) {
        let Some(mut s) = self.sheet.take() else {
            return;
        };
        if let Some(p) = s.popover(self) {
            let bw = p.width.min(self.w).max(12.min(self.w));
            // Its rows, and the edges above and below them.
            let body = s.body(
                self,
                bw.saturating_sub(4),
                base.len().saturating_sub(2).max(1),
            );
            self.sheet = Some(s);
            let b = popover_box(&p, &body, bw);
            let (left, top) = popover_at(p.at, bw, b.len(), self.w, base.len());
            if p.at.is_none() {
                super::dialog::fade(base, self.w);
            }
            self.keep_overlay(left, top, bw, b.len(), left + 2, top + 1);
            self.sheet_at = (left + 2, top + 1);
            super::dialog::paste_at(base, &b, top, left);
            return;
        }
        let bw = self.sheet_width(s.width(self));
        let body = s.body(self, bw - 4, 8.max(self.h.saturating_sub(6)));
        self.sheet = Some(s);
        // As overlay_box places it: the edge and a blank line above the
        // body, the edge and a space left of it.
        let bx = 1.max((base.len() as isize - (body.len() as isize + 4)) / 2) as usize;
        self.sheet_at = ((self.w.saturating_sub(bw)) / 2 + 2, bx + 2);
        self.overlay_box(base, &body, bw);
    }
}

/// A sheet's first line: its name, and what it's for (sheetTitle).
pub fn sheet_title(name: &str, about: &str, w: usize) -> Line {
    line([
        spb(name, TEXT),
        plain("  "),
        sp(
            rush_draw::truncate(about, w.saturating_sub(width_of(name) + 2), "…").into_owned(),
            DIM,
        ),
    ])
}

fn width_of(s: &str) -> usize {
    rush_core::text::width(s)
}

/// A row of tabs, the current one bright (sheetTabs).
pub fn sheet_tabs(names: &[&str], cur: usize) -> Line {
    let mut l = Line::new();
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            l.spans.push(sp("  ·  ", FAINT));
        }
        if i == cur {
            l.spans.push(spb(*n, ORANGE));
        } else {
            l.spans.push(sp(*n, DIM));
        }
    }
    l.spans.push(sp("   [ ]", FAINT));
    l
}

/// One choosable row: on the selection's ground with ▍ when it's the
/// cursor's (sheetRow).
pub fn sheet_row(l: Line, on: bool, w: usize) -> Line {
    if on {
        let mut r = line([sp("▍", ORANGE), plain(" ")]);
        join(&mut r, l);
        return highlight(r, w);
    }
    let mut r = line([plain("  ")]);
    join(&mut r, l);
    r
}

/// Which of n rows to show in h lines, keeping cur in view (window).
pub fn window(n: usize, cur: usize, h: usize) -> (usize, usize) {
    if n <= h {
        return (0, n);
    }
    let from = (cur as isize - h as isize / 2).clamp(0, (n - h) as isize) as usize;
    (from, from + h)
}

/// An editable line with a cursor at pos when focused (textField).
pub fn text_field(buf: &[char], pos: usize, focused: bool, placeholder: &str, w: usize) -> Line {
    if buf.is_empty() && !focused {
        return line([sp(placeholder, FAINT)]);
    }
    let all: String = buf.iter().collect();
    if !focused {
        return line([sp(rush_draw::truncate(&all, w, "…").into_owned(), TEXT)]);
    }
    if buf.is_empty() {
        return line([sp("▏", ORANGE), sp(placeholder, FAINT)]);
    }
    let pos = pos.min(buf.len());
    let mut before: String = buf[..pos].iter().collect();
    let after: String = buf[pos..].iter().collect();
    let room = w.saturating_sub(2);
    if width_of(&before) > room {
        before = truncate_left(&before, width_of(&before) - room, "…");
    }
    let rest = w.saturating_sub(width_of(&before) + 1);
    line([
        sp(before, TEXT),
        sp("▏", ORANGE),
        sp(rush_draw::truncate(&after, rest, "…").into_owned(), TEXT),
    ])
}

/// s with its first n cells cut, starting with tail (ansi.TruncateLeft).
pub fn truncate_left(s: &str, n: usize, tail: &str) -> String {
    let mut cut = 0;
    let mut at = s.len();
    for (i, c) in s.char_indices() {
        if cut >= n {
            at = i;
            break;
        }
        cut += width_of(c.encode_utf8(&mut [0; 4]));
    }
    if cut < n {
        return tail.to_string();
    }
    format!("{tail}{}", &s[at..])
}

/// s in the middle of w cells (center).
pub fn center(s: &str, w: usize) -> String {
    let pad = w.saturating_sub(width_of(s));
    format!("{}{s}{}", blanks(pad / 2), blanks(pad - pad / 2))
}
