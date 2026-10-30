//! The mouse in a box over the screen (a sheet, a question, a picker):
//! one way for them all, which presses the keys a click stands for, so
//! each does with it just what its keys do. Outside the box is esc; on a
//! tab, ] until that tab's up; on a key in a row of keys, that key; on a
//! row, ↑ or ↓ until the cursor's on it, then enter. Sheets that take the
//! mouse themselves still do, inside the box. After Go's
//! internal/ui/overlayclick.go (overlayHit :23, overlayBody :37,
//! overlayClick :59, overlayWheel :88, press :102, cursorRow :113,
//! rowClick :131, tabClick :186, hintKey :229) and sheet.go:52 (mouseEv).

use std::sync::LazyLock;

use regex::Regex;
use rush_core::text::{Line, width};

use super::{Mouse, Out, Press};
use crate::model::Model;
use crate::style::*;

/// What the mouse did in a sheet that takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEv {
    Press,
    /// Moved with the left button down.
    Drag,
    Release,
    WheelUp,
    WheelDown,
}

/// The box over the screen as last drawn: its edges, and where its body's
/// first cell landed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayHit {
    pub x0: usize,
    pub y0: usize,
    /// Past the box.
    pub x1: usize,
    pub y1: usize,
    /// The body's top left cell.
    pub bx: usize,
    pub by: usize,
    pub on: bool,
}

impl Model {
    /// Notes a box drawn bw wide and h tall at (left, top), its body
    /// starting at (bx, by) (keepOverlay).
    pub(crate) fn keep_overlay(
        &mut self,
        left: usize,
        top: usize,
        bw: usize,
        h: usize,
        bx: usize,
        by: usize,
    ) {
        self.over = OverlayHit {
            x0: left,
            y0: top,
            x1: left + bw,
            y1: top + h,
            bx,
            by,
            on: true,
        };
    }

    /// The body of the box that has the keys, as it draws now
    /// (overlayBody).
    pub fn overlay_body(&mut self) -> Option<Vec<Line>> {
        let o = self.over;
        if !o.on {
            return None;
        }
        let bw = o.x1 - o.x0;
        if let Some(c) = &self.confirm {
            return Some(c.body(bw));
        }
        let mut s = self.sheet.take()?;
        let b = s.body(self, bw.saturating_sub(4), 8.max(self.h.saturating_sub(6)));
        self.sheet = Some(s);
        Some(b)
    }

    /// A left click while a box has the keys; Pass when no box took it
    /// (overlayClick).
    pub fn overlay_click(&mut self, e: &Mouse) -> Out {
        let (x, y) = (e.x as usize, e.y as usize);
        let o = self.over;
        if !o.on {
            return Out::Pass;
        }
        if x < o.x0 || x >= o.x1 || y < o.y0 || y >= o.y1 {
            return self.press("esc");
        }
        // A sheet that takes the mouse has it inside its box.
        if self.confirm.is_none() {
            let r = self.sheet_mouse(MouseEv::Press, e);
            if r.used() {
                return r;
            }
        }
        let Some(body) = self.overlay_body() else {
            return Out::Pass;
        };
        let (Some(i), Some(col)) = (y.checked_sub(o.by), x.checked_sub(o.bx)) else {
            return Out::Idle;
        };
        if i >= body.len() {
            return Out::Idle;
        }
        if let Some(out) = self.tab_click(&body, i, col) {
            return out;
        }
        if let Some(out) = self.row_click(&body, i) {
            return out;
        }
        if let Some(k) = hint_key(&body[i], col) {
            return self.press(&k);
        }
        Out::Idle
    }

    /// The wheel over a box that has the keys: ↑ and ↓ (overlayWheel).
    pub fn overlay_wheel(&mut self, up: bool) -> Out {
        if !self.over.on {
            return Out::Pass;
        }
        self.press(if up { "up" } else { "down" })
    }

    /// A key pressed as the keyboard would, through every binding.
    pub fn press(&mut self, s: &str) -> Out {
        match Press::of(s) {
            Some(p) => self.press_key(&p),
            None => Out::Idle,
        }
    }

    /// Moves the cursor to row i clicked, then presses enter; None when
    /// line i isn't a row (rowClick). Only an indented line can be one; a
    /// line the cursor never reaches leaves it where it was.
    fn row_click(&mut self, body: &[Line], i: usize) -> Option<Out> {
        let plain = body[i].plain();
        let want = row_text(&body[i]);
        let cur = cursor_row(body);
        if cur == Some(i) {
            return Some(self.press("enter"));
        }
        let cur = cur?;
        if want.is_empty() || !plain.starts_with(' ') {
            return None;
        }
        let (dir, back) = if i < cur {
            ("up", "down")
        } else {
            ("down", "up")
        };
        let from = row_text(&body[cur]);
        let mut at = from.clone();
        let mut moved = 0;
        for _ in 0..256 {
            let o = self.press(dir);
            let Some(b) = self.overlay_body() else {
                return Some(o);
            };
            let Some(c) = cursor_row(&b) else {
                break;
            };
            let now = row_text(&b[c]);
            if now == want {
                return Some(self.press("enter"));
            }
            if now == at || now == from {
                break; // the end of the list, or round to where it began
            }
            at = now;
            moved += 1;
        }
        for _ in 0..moved {
            match self.overlay_body() {
                Some(b) if cursor_row(&b).is_some_and(|c| row_text(&b[c]) == from) => break,
                None => break,
                _ => {}
            }
            self.press(back);
        }
        None
    }

    /// Switches to the tab clicked, pressing ] until it's the one up; None
    /// when line i isn't a row of tabs (tabClick).
    fn tab_click(&mut self, body: &[Line], i: usize, col: usize) -> Option<Out> {
        if !tab_row(&body[i]) {
            return None;
        }
        let plain = body[i].plain();
        let lead = width(&plain) - width(plain.trim_start_matches(' '));
        let trimmed = plain.trim();
        let names: Vec<&str> = trimmed
            .strip_suffix("[ ]")
            .unwrap_or(trimmed)
            .split("  ·  ")
            .map(str::trim)
            .collect();
        let mut want = "";
        let mut x = lead;
        for n in &names {
            let w = width(n);
            if col >= x && col < x + w {
                want = n;
            }
            x += w + 5;
        }
        if want.is_empty() {
            return Some(Out::Idle);
        }
        let want = want.to_string();
        let mut out = Out::Idle;
        for _ in 0..names.len() {
            let Some(b) = self.overlay_body() else { break };
            let Some(at) = b.iter().position(tab_row) else {
                break;
            };
            let up = b[at]
                .spans
                .iter()
                .any(|s| s.text == want && s.style.fg == ORANGE && s.style.mods.contains(BOLD_MOD));
            if up {
                break;
            }
            out = self.press("]");
        }
        Some(out)
    }
}

const BOLD_MOD: rush_core::text::Mods = rush_core::text::Mods::BOLD;

/// The body line the cursor's on, drawn with ▍ (cursorRow).
pub fn cursor_row(body: &[Line]) -> Option<usize> {
    body.iter()
        .position(|l| l.plain().trim_start_matches(' ').starts_with('▍'))
}

/// A row's words, without the cursor's mark or the padding (rowText).
pub fn row_text(l: &Line) -> String {
    l.plain().trim().trim_start_matches('▍').trim().to_string()
}

/// A row of sheet_tabs: its names, then [ ] (tabRow).
fn tab_row(l: &Line) -> bool {
    l.plain().trim().ends_with("[ ]")
}

/// What parts the pairs in a row of keys: keys' "  ·  ", or a
/// question's three spaces (hintGap).
static HINT_GAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+·\s+|\s{3,}").unwrap());

/// The key named at col in a row of keys (keys, a question's), None when
/// col isn't on one or the line isn't such a row: the key has to be
/// drawn as a key, bright before its dim label (hintKey).
pub fn hint_key(l: &Line, col: usize) -> Option<String> {
    let plain = l.plain();
    let mut x = 0;
    let mut rest = plain.as_str();
    while !rest.is_empty() {
        let (seg, gap, next) = match HINT_GAP.find(rest) {
            Some(m) => (&rest[..m.start()], m.as_str(), &rest[m.end()..]),
            None => (rest, "", ""),
        };
        let w = width(seg);
        if col >= x && col < x + w {
            let k = seg.trim().split(' ').next().unwrap_or("");
            if k.is_empty() || k.contains(['↑', '↓', '←', '→']) {
                return None;
            }
            let drawn = l.spans.iter().any(|s| {
                s.text == k
                    && ((s.style.fg == TEXT && s.style.mods.contains(BOLD_MOD))
                        || s.style.fg == ORANGE)
            });
            if !drawn {
                return None;
            }
            let k = k.replace('⌥', "alt+");
            return Press::of(&k).map(|_| k);
        }
        x += w + width(gap);
        rest = next;
    }
    None
}
