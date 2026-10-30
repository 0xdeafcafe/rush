//! Boxes drawn over a screen, shared by the sheets, the pickers, the
//! questions and Settings. After Go's internal/ui/dialog.go (overlayBox
//! :19, boxLines :32, edgedBox :35, pasteAt :47, panel :62) and view.go
//! (modalOver :791).

use rush_core::text::{Color, Line, Span, width};

use crate::model::Model;
use crate::style::*;

impl Model {
    /// Draws body in a panel bw wide centred over base, everything behind
    /// it faded (overlayBox).
    pub fn overlay_box(&mut self, base: &mut [Line], body: &[Line], bw: usize) {
        self.modal_over(base, body, bw, DIM);
    }

    /// overlay_box with the box's edge in col (modalOver).
    pub fn modal_over(&mut self, base: &mut [Line], body: &[Line], bw: usize, col: Color) {
        fade(base, self.w);
        let b = edged_box(body, bw, col);
        let top = 1.max((base.len() as isize - b.len() as isize) / 2) as usize;
        let left = self.w.saturating_sub(bw) / 2;
        self.keep_overlay(left, top, bw, b.len(), left + 2, top + 2);
        paste_at(base, &b, top, left);
    }
}

/// Every row faded to its words in the faint ink, as what's behind a box
/// over the screen.
pub fn fade(rows: &mut [Line], w: usize) {
    for l in rows.iter_mut() {
        let text = fit(std::mem::take(l), w).plain();
        *l = line([sp(text, FAINT)]);
    }
}

/// body in a rounded box bw wide, a blank line inside top and bottom
/// (boxLines).
pub fn box_lines(body: &[Line], bw: usize) -> Vec<Line> {
    edged_box(body, bw, DIM)
}

/// box_lines with its edge in col (edgedBox).
pub fn edged_box(body: &[Line], bw: usize, col: Color) -> Vec<Line> {
    let inner = bw.saturating_sub(4);
    let edge = |s: String| sp(s, col);
    let mut out = vec![line([edge(format!(
        "╭{}╮",
        "─".repeat(bw.saturating_sub(2))
    ))])];
    let blank = Line::new();
    for l in std::iter::once(&blank)
        .chain(body.iter())
        .chain(std::iter::once(&blank))
    {
        let mut row = line([edge("│".into())]);
        let mut inside = line([plain(" ")]);
        join(&mut inside, fit(l.clone(), inner));
        inside.spans.push(plain(" "));
        join(&mut row, panel(inside, inner + 2));
        row.spans.push(edge("│".into()));
        out.push(row);
    }
    out.push(line([edge(format!(
        "╰{}╯",
        "─".repeat(bw.saturating_sub(2))
    ))]));
    out
}

/// l on the panel's ground, w wide (panel).
pub fn panel(l: Line, w: usize) -> Line {
    on_ground(l, w, PANEL_BG)
}

/// Lays b over lines with its top left corner at (left, top) (pasteAt).
pub fn paste_at(lines: &mut [Line], b: &[Line], top: usize, left: usize) {
    for (i, r) in b.iter().enumerate() {
        let Some(l) = lines.get_mut(top + i) else {
            continue;
        };
        let rw = r.width();
        let mut out = cells(l, 0, left);
        let lw = out.width();
        if lw < left {
            out.spans.push(plain(blanks(left - lw)));
        }
        join(&mut out, r.clone());
        let tail = cells(l, left + rw, usize::MAX);
        join(&mut out, tail);
        *l = out;
    }
}

/// The cells [from, to) of l; a wide char cut by either end becomes a
/// blank.
pub fn cells(l: &Line, from: usize, to: usize) -> Line {
    let mut out = Line {
        spans: Vec::new(),
        bg: l.bg,
    };
    let mut col = 0;
    for s in &l.spans {
        let sw = s.width();
        if col + sw <= from || col >= to {
            col += sw;
            continue;
        }
        if col >= from && col + sw <= to {
            out.spans.push(s.clone());
            col += sw;
            continue;
        }
        let mut text = String::new();
        for ch in s.text.chars() {
            let cw = width(ch.encode_utf8(&mut [0; 4]));
            if col >= from && col + cw <= to {
                text.push(ch);
            } else if col < to && col + cw > from {
                // Half in: a blank for each cell inside.
                let a = col.max(from);
                let b = (col + cw).min(to);
                text.push_str(&blanks(b.saturating_sub(a)));
            }
            col += cw;
        }
        if !text.is_empty() {
            out.spans.push(Span::new(text, s.style));
        }
    }
    out
}
