//! The input box: an edge carrying labels round wrapped text, its cursor
//! and selection, chips drawn as one thing. It sits on the brightest
//! ground on screen, so where you type is never in doubt; its edge turns
//! orange while your keys go there. After Go's internal/ui/inputbox.go
//! (box :25, lines :63, rows :177, wrapSegs :198, window :248, scrolled
//! :266, content :273, chipMask :383, at :437, near :450, col :467). Not
//! ported: a dictation drawn live (yap's withLive :128) and images shown
//! by name (named :402): theirs to add as they come.

use rush_core::text::{Color, Line, Span, Style, width};

use super::editor::{Seg, chip_spans};
use crate::style::*;

/// The edge of a box that hasn't the keys (cEdge).
pub const EDGE: Color = Color::Rgb(79, 73, 67);
/// The box's ground (bgInput), selected text's (bgMark), a chip's
/// (bgChip).
pub const INPUT_BG: Color = Color::Rgb(40, 36, 32);
pub const MARK_BG: Color = Color::Rgb(74, 64, 54);
pub const CHIP_BG: Color = Color::Rgb(56, 62, 72);

/// One input box: the edge's labels and what's inside.
#[derive(Clone, Debug, Default)]
pub struct InputBox {
    pub w: usize,
    pub focused: bool,
    /// Who the text goes to, and what enter will do.
    pub top_l: Line,
    pub top_r: Line,
    pub foot_l: Line,
    pub foot_r: Line,
    pub text: Vec<char>,
    pub cursor: usize,
    pub anchor: Option<usize>,
    /// Shown faint while the box is empty.
    pub holder: String,
    /// Before the text on its first line, e.g. ❯.
    pub lead: Line,
    pub max_rows: usize,
    /// The first wrapped row shown last draw, kept while the cursor stays
    /// in view.
    pub top: usize,
    /// A chip lit, under the pointer.
    pub hot: Seg,
    /// Focused but not being typed in: the edge stays, the cursor doesn't.
    pub idle: bool,
}

impl InputBox {
    /// The box: a top edge carrying its labels, the text wrapped with the
    /// cursor in place, and a bottom edge (lines).
    pub fn lines(&self) -> Vec<Line> {
        let w = 20.max(self.w);
        let edge = if self.focused { ORANGE } else { EDGE };
        let inner = w - 4;
        let rows = self.content(inner);
        let border = |l: &str, r: &str, left: &Line, right: &Line| -> Line {
            let mut head = line([sp(format!("{l}─"), edge)]);
            if !left.spans.is_empty() {
                head.spans.push(plain(" "));
                join(&mut head, left.clone());
                head.spans.push(plain(" "));
            }
            let bare = line([sp(format!("─{r}"), edge)]);
            let mut tail = bare.clone();
            if !right.spans.is_empty() {
                let mut t = line([plain(" ")]);
                join(&mut t, right.clone());
                t.spans.push(plain(" "));
                join(&mut t, bare.clone());
                tail = t;
            }
            if head.width() + tail.width() > w {
                tail = bare;
            }
            let over = (head.width() + tail.width()) as isize - w as isize;
            if over > 0 {
                head = rush_draw::truncate_line(&head, head.width() - over as usize - 1, "…");
            }
            let fill = w.saturating_sub(head.width() + tail.width());
            let mut out = head;
            out.spans.push(sp("─".repeat(fill), edge));
            join(&mut out, tail);
            out
        };
        let mut out = vec![border("╭", "╮", &self.top_l, &self.top_r)];
        for row in rows {
            let mut l = line([sp("│", edge)]);
            let mut body = line([plain(" ")]);
            join(&mut body, row);
            join(&mut l, on_ground(body, inner + 2, INPUT_BG));
            l.spans.push(sp("│", edge));
            out.push(l);
        }
        out.push(border("╰", "╯", &self.foot_l, &self.foot_r));
        out
    }

    /// How many text rows lines() draws (rows).
    pub fn rows(&self) -> usize {
        if self.text.is_empty() {
            return 1;
        }
        let (s, e) = self.window(&self.segs());
        e - s
    }

    /// The text wrapped as lines() draws it. A row leaves a cell spare, so
    /// the cursor after a full row sits inside the edge (segs).
    pub fn segs(&self) -> Vec<Seg> {
        wrap_segs(&self.text, self.text_w())
    }

    pub fn text_w(&self) -> usize {
        (20.max(self.w) - 4).saturating_sub(self.lead.width() + 1)
    }

    /// Which wrapped rows are on screen: the rows from top, moved only as
    /// far as it takes to keep the cursor's row among them (window).
    pub fn window(&self, segs: &[Seg]) -> (usize, usize) {
        let limit = 1.max(self.max_rows);
        if segs.len() <= limit {
            return (0, segs.len());
        }
        let at = segs
            .iter()
            .position(|s| self.cursor >= s.from && self.cursor <= s.to)
            .unwrap_or(segs.len() - 1);
        let start = self.top.max((at + 1).saturating_sub(limit)).min(at);
        let start = start.min(segs.len() - limit);
        (start, start + limit)
    }

    /// The box with top set to the first row it shows, for the caller to
    /// keep and hand back on the next draw (scrolled).
    pub fn scrolled(mut self) -> InputBox {
        self.top = self.window(&self.segs()).0;
        self
    }

    fn content(&self, w: usize) -> Vec<Line> {
        let lw = self.lead.width();
        let caret = reversed(Style::default());
        if self.text.is_empty() {
            let mut l = self.lead.clone();
            if self.focused && !self.idle {
                l.spans.push(Span::new(" ", caret));
            }
            l.spans.push(sp(
                rush_draw::truncate(&self.holder, 1.max(w.saturating_sub(lw + 1)), "…")
                    .into_owned(),
                FAINT,
            ));
            return vec![l];
        }
        let segs = self.segs();
        let (start, end) = self.window(&segs);
        let chips = chip_mask(&self.text);
        let sel = self
            .anchor
            .filter(|&a| a != self.cursor)
            .map(|a| (a.min(self.cursor), a.max(self.cursor)));
        let mut rows = Vec::new();
        for i in start..end {
            let sg = segs[i];
            let mut l = if i == 0 {
                self.lead.clone()
            } else {
                line([plain(blanks(lw))])
            };
            let mut run = String::new();
            let mut run_style = Style::fg(TEXT);
            let flush = |l: &mut Line, run: &mut String, st: Style| {
                if !run.is_empty() {
                    l.spans.push(Span::new(std::mem::take(run), st));
                }
            };
            for p in sg.from..sg.to {
                let c = self.text[p];
                let st = if self.focused && !self.idle && p == self.cursor {
                    reversed(Style::fg(TEXT))
                } else if sel.is_some_and(|(f, t)| p >= f && p < t) {
                    Style::fg(TEXT).on(MARK_BG)
                } else if chips.as_ref().is_some_and(|m| m[p]) {
                    let lit = p >= self.hot.from && p < self.hot.to;
                    Style::fg(if lit { ORANGE } else { BLUE }).on(CHIP_BG)
                } else {
                    Style::fg(TEXT)
                };
                if st != run_style {
                    flush(&mut l, &mut run, run_style);
                    run_style = st;
                }
                run.push(c);
            }
            flush(&mut l, &mut run, run_style);
            // The cursor at the end of a row shows as a block after it.
            let last = i == segs.len() - 1 || segs[i + 1].from > sg.to;
            if self.focused && !self.idle && self.cursor == sg.to && last {
                l.spans.push(Span::new(" ", caret));
            }
            rows.push(l);
        }
        rows
    }

    /// Where drawn position p is in the text; the box draws the text as
    /// it is, so the same.
    fn text_pos(&self, d: usize) -> usize {
        d
    }

    /// A click inside the box's text (row from the first text row, col
    /// from the box's left edge) as a position in the text (at).
    pub fn at(&self, row: usize, col: usize) -> usize {
        let segs = self.segs();
        let (start, end) = self.window(&segs);
        let i = start + row;
        if i >= end {
            return self.text_pos(self.cursor);
        }
        self.text_pos(self.col(segs[i], col))
    }

    /// at for a drag, which can leave the box: above its first row is that
    /// row's start, below its last row is that row's end (near).
    pub fn near(&self, row: isize, col: usize) -> usize {
        let segs = self.segs();
        let (start, end) = self.window(&segs);
        if segs.is_empty() {
            return 0;
        }
        if row < 0 {
            return self.text_pos(segs[start].from);
        }
        let i = start + row as usize;
        if i >= end {
            return self.text_pos(segs[end - 1].to);
        }
        self.text_pos(self.col(segs[i], col))
    }

    // The text position under screen column col on row sg.
    fn col(&self, sg: Seg, col: usize) -> usize {
        let x = col as isize - 2 - self.lead.width() as isize; // "│ " then the lead
        let mut p = sg.from;
        let mut wd = 0isize;
        while p < sg.to && wd + rune_w(self.text[p]) as isize <= x {
            wd += rune_w(self.text[p]) as isize;
            p += 1;
        }
        p
    }
}

/// A char's cells, printable ASCII answered directly (runeW).
pub fn rune_w(c: char) -> usize {
    if (' '..'\x7f').contains(&c) {
        1
    } else {
        width(c.encode_utf8(&mut [0; 4]))
    }
}

/// text word-wrapped to w cells a row, long words broken, each row's
/// offsets kept so a click or the cursor maps to an exact char
/// (wrapSegs).
pub fn wrap_segs(text: &[char], w: usize) -> Vec<Seg> {
    let w = 4.max(w);
    // at[i] is the width of text[..i], so any stretch's is a subtraction.
    let mut at = vec![0; text.len() + 1];
    for (i, &c) in text.iter().enumerate() {
        at[i + 1] = at[i] + rune_w(c);
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start <= text.len() {
        let mut end = start;
        while end < text.len() && text[end] != '\n' {
            end += 1;
        }
        let mut from = start;
        loop {
            if at[end] - at[from] <= w {
                out.push(Seg { from, to: end });
                break;
            }
            let mut cut = from;
            while cut < end && at[cut + 1] - at[from] <= w {
                cut += 1;
            }
            let mut brk = cut;
            for i in (from + 1..=cut).rev() {
                if text[i - 1] == ' ' {
                    brk = i;
                    break;
                }
            }
            if brk == from {
                brk = cut.max(from + 1); // a char wider than the row still moves on
            }
            out.push(Seg { from, to: brk });
            from = brk;
        }
        if end >= text.len() {
            break;
        }
        start = end + 1;
    }
    out
}

/// The chars of each chip in text marked, None when it has none
/// (chipMask).
pub fn chip_mask(text: &[char]) -> Option<Vec<bool>> {
    let spans = chip_spans(text);
    if spans.is_empty() {
        return None;
    }
    let mut mask = vec![false; text.len()];
    for s in spans {
        for m in &mut mask[s.from..s.to] {
            *m = true;
        }
    }
    Some(mask)
}
