//! The input box: an edge carrying labels round wrapped text, its cursor
//! and selection, chips drawn as one thing. It sits on the brightest
//! ground on screen, so where you type is never in doubt; its edge turns
//! orange while your keys go there. After Go's internal/ui/inputbox.go
//! (box :25, lines :63, rows :177, wrapSegs :198, window :248, scrolled
//! :266, content :273, chipMask :383, at :437, near :450, col :467), and
//! a dictation drawn live at the cursor (withLive :128, the edge's light
//! in lines :74-96, the inked words and cursor in content :300-345). Not
//! ported: images shown by name (named :402): theirs to add as they come.

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
    /// Cells of an edge under the pointer, lit as a click there does
    /// something: the bottom edge or the top, from and to, from the box's
    /// left edge.
    pub lit: Option<(bool, usize, usize)>,
    /// A dictation under way, drawn at the cursor (see rush-ui-social's
    /// dictation.rs).
    pub live: Option<LiveText>,
    /// What drawing live sets: where its words are, the frame its light
    /// is at, the cursor's ground. Leave it unset: lines() fills it in.
    pub look: Option<Look>,
    /// The edge's colour whether or not it has the keys: a box in a mode
    /// of its own (a message box running a shell command after `!`).
    pub edge: Option<Color>,
}

/// Where an edge's labels landed, cells from the box's left edge: the
/// left one's first cell and how many of its cells show whole (fewer when
/// it's cut short with …), and the right one's first cell when it had
/// room.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeAt {
    pub left: (usize, usize),
    pub right: Option<usize>,
}

impl EdgeAt {
    /// The parts of the left label (right false) or the right one, cells
    /// into the label, placed where it landed: from and to on the edge,
    /// each cut to what shows, those that don't show at all gone.
    pub fn place<A: Clone>(
        &self,
        right: bool,
        parts: &[(usize, usize, A)],
    ) -> Vec<(usize, usize, A)> {
        let (x, shown) = if right {
            match self.right {
                Some(x) => (x, usize::MAX),
                None => return Vec::new(),
            }
        } else {
            self.left
        };
        parts
            .iter()
            .filter(|p| p.0 < shown && p.1 > p.0)
            .map(|p| (x + p.0, x + p.1.min(shown), p.2.clone()))
            .collect()
    }
}

/// A dictation as the box draws it (withLive's ink, caret, glowing and
/// frame): its words at [from, to), the guessed ones from guess_from, the
/// text's width for the light crossing it, and the cursor's ground.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    from: usize,
    guess_from: usize,
    to: usize,
    frame: usize,
    w: usize,
    caret: Color,
}

impl InputBox {
    /// The box: a top edge carrying its labels, the text wrapped with the
    /// cursor in place, and a bottom edge (lines).
    pub fn lines(&self) -> Vec<Line> {
        if self.live.is_some() && self.focused && !self.idle {
            return self.with_live().lines();
        }
        let w = 20.max(self.w);
        let inner = w - 4;
        let rows = self.content(inner);
        let h = rows.len() + 2;
        let mut out = vec![self.edge(false, h).0];
        for (i, row) in rows.into_iter().enumerate() {
            let mut l = self.run(0, i + 1, h, "│");
            let mut body = line([plain(" ")]);
            join(&mut body, row);
            join(&mut l, on_ground(body, inner + 2, INPUT_BG));
            join(&mut l, self.run(w - 1, i + 1, h, "│"));
            out.push(l);
        }
        out.push(self.edge(true, h).0);
        out
    }

    /// Edge glyphs s from cell x of row y of a box h rows tall: in the
    /// edge's colour, or cell by cell with a dictation's light running
    /// round it.
    fn run(&self, x: usize, y: usize, h: usize, s: &str) -> Line {
        let w = 20.max(self.w);
        let edge = match self.edge {
            Some(c) => c,
            None if self.focused => ORANGE,
            None => EDGE,
        };
        let Some(frame) = self.look.as_ref().map(|k| k.frame).filter(|_| h > 0) else {
            return line([sp(s.to_string(), edge)]);
        };
        let n = 2 * (w - 1) + 2 * (h - 1);
        let mut out = Line::new();
        let mut last: Option<Color> = None;
        let mut buf = String::new();
        for (i, c) in s.chars().enumerate() {
            let x = x + i;
            let around = if y == 0 {
                x
            } else if x == w - 1 {
                w - 1 + y
            } else if y == h - 1 {
                (2 * (w - 1) + h - 1).saturating_sub(x)
            } else {
                n - y // up the left
            };
            let col = yap_ink(yap_step(yap_glint(frame, around, n))).2;
            if last.is_some_and(|l| l != col) {
                out.spans
                    .push(sp(std::mem::take(&mut buf), last.unwrap_or(edge)));
            }
            last = Some(col);
            buf.push(c);
        }
        if !buf.is_empty() {
            out.spans.push(sp(buf, last.unwrap_or(edge)));
        }
        out
    }

    /// The top edge (foot false) or the bottom one of a box h rows tall
    /// carrying their labels, and where the labels landed (border). The
    /// part under the pointer (lit) is on the pointer's ground.
    fn edge(&self, foot: bool, h: usize) -> (Line, EdgeAt) {
        let w = 20.max(self.w);
        let y = if foot { h.saturating_sub(1) } else { 0 };
        let (l, r, left, right) = if foot {
            ("╰", "╯", &self.foot_l, &self.foot_r)
        } else {
            ("╭", "╮", &self.top_l, &self.top_r)
        };
        let mut at = EdgeAt::default();
        let mut head = self.run(0, y, h, &format!("{l}─"));
        if !left.spans.is_empty() {
            head.spans.push(plain(" "));
            join(&mut head, left.clone());
            head.spans.push(plain(" "));
            at.left = (3, left.width());
        }
        let bare = self.run(w - 2, y, h, &format!("─{r}"));
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
        } else if !right.spans.is_empty() {
            at.right = Some(w - tail.width() + 1);
        }
        let over = (head.width() + tail.width()) as isize - w as isize;
        if over > 0 {
            let keep = head.width() - over as usize - 1;
            head = rush_draw::truncate_line(&head, keep, "…");
            // What's left of the left label whole, before the …
            at.left.1 = at.left.1.min(keep.saturating_sub(at.left.0 + 1));
        }
        let fill = w.saturating_sub(head.width() + tail.width());
        let hw = head.width();
        let mut out = head;
        join(&mut out, self.run(hw, y, h, &"─".repeat(fill)));
        join(&mut out, tail);
        if let Some((f, x0, x1)) = self.lit
            && f == foot
        {
            out = light_cells(&out, x0, x1);
        }
        (out, at)
    }

    /// Where the labels on the top edge (foot false) or the bottom one
    /// landed, from the box's left edge, for clicks on their parts.
    pub fn edge_at(&self, foot: bool) -> EdgeAt {
        self.edge(foot, 0).1
    }

    /// The box drawing its dictation: the words put in at the cursor,
    /// coloured as they're coloured, the cursor after them (withLive).
    fn with_live(&self) -> InputBox {
        let mut b = self.clone();
        let Some(l) = b.live.take() else {
            return b;
        };
        let cur = self.cursor.min(self.text.len());
        let (pre, post) = self.text.split_at(cur);
        let settled: Vec<char> = l.settled.chars().collect();
        let guessing: Vec<char> = l.guessing.chars().collect();
        let mut said = Vec::new();
        if pre.last().is_some_and(|c| !c.is_whitespace()) && settled.len() + guessing.len() > 0 {
            said.push(' ');
        }
        let from = pre.len() + said.len();
        said.extend_from_slice(&settled);
        if !settled.is_empty() && !guessing.is_empty() {
            said.push(' ');
        }
        let guess_from = pre.len() + said.len();
        said.extend_from_slice(&guessing);
        let to = pre.len() + said.len();
        b.text = pre.iter().chain(&said).chain(post).copied().collect();
        b.cursor = to;
        b.anchor = None;
        b.hot = Seg::default();
        b.top = 0;
        b.holder = "listening…".into();
        if !l.hint.is_empty() {
            b.foot_r = line([sp(l.hint.clone(), DIM)]);
        }
        // The cursor brightens as you speak, and breathes while you don't.
        let k = l.level.max(0.3 + 0.2 * (l.frame as f64 * 0.3).sin());
        b.look = Some(Look {
            from,
            guess_from,
            to,
            frame: l.frame,
            w: b.text_w(),
            caret: yap_ink(yap_step(k)).3,
        });
        b
    }

    /// The colour the dictation gives the character at p, x cells into
    /// row y of those shown; None for the box's own text.
    fn ink(&self, p: usize, x: usize, y: usize) -> Option<Color> {
        let k = self.look.as_ref()?;
        if p < k.from || p >= k.to {
            return None;
        }
        if p >= k.guess_from {
            let sweep = yap_sweep(k.frame, p - k.guess_from, 0, k.to - k.guess_from);
            return Some(yap_ink(yap_step(sweep)).1);
        }
        Some(yap_ink(yap_step(0.5 * yap_sweep(k.frame, x, y, k.w))).0)
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
        // A dictation's cursor is its own ground, lit as you speak.
        let caret = match &self.look {
            Some(k) => Style::fg(TEXT).on(k.caret),
            None => reversed(Style::default()),
        };
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
            let mut x = 0;
            for p in sg.from..sg.to {
                let c = self.text[p];
                let ink = self.ink(p, x, i - start);
                x += rune_w(c);
                let st = if self.focused && !self.idle && p == self.cursor {
                    if self.look.is_some() {
                        caret
                    } else {
                        reversed(Style::fg(TEXT))
                    }
                } else if sel.is_some_and(|(f, t)| p >= f && p < t) {
                    Style::fg(TEXT).on(MARK_BG)
                } else if chips.as_ref().is_some_and(|m| m[p]) {
                    let lit = p >= self.hot.from && p < self.hot.to;
                    Style::fg(if lit { ORANGE } else { BLUE }).on(CHIP_BG)
                } else if let Some(col) = ink {
                    Style::fg(col)
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
    pub fn at(&self, row: usize, col: isize) -> usize {
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
    pub fn near(&self, row: isize, col: isize) -> usize {
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
    fn col(&self, sg: Seg, col: isize) -> usize {
        let x = col - 2 - self.lead.width() as isize; // "│ " then the lead
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

// --- a dictation's look (yap.go:130-215), which the box draws ---

/// The look's colours, in steps from each's resting colour to its lit one.
pub const YAP_STEPS: usize = 16;

/// A dictation as a box draws it (liveText).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveText {
    pub settled: String,
    pub guessing: String,
    /// How loud, 0 to 1.
    pub level: f64,
    pub frame: usize,
    /// What's going on, in the box's bottom edge.
    pub hint: String,
}

/// The step of the look's colours k (0 to 1) is (yapStep).
pub fn yap_step(k: f64) -> usize {
    (k.clamp(0.0, 1.0) * YAP_STEPS as f64 + 0.5) as usize
}

/// How lit the text at column x of row y is: a band of light leaning
/// like yap's, crossing w columns every 1.6s, as wide as a third of them
/// (yapSweep).
pub fn yap_sweep(frame: usize, x: usize, y: usize, w: usize) -> f64 {
    const PERIOD: usize = 32; // frames a crossing
    let band = (w as f64 / 3.0).clamp(3.0, 8.0);
    let at = -band + (frame % PERIOD) as f64 / PERIOD as f64 * (w as f64 + 2.0 * band);
    let d = (x as f64 + 2.0 * y as f64 - at).abs();
    if d >= band {
        return 0.0;
    }
    let k = 1.0 - d / band;
    k * k
}

/// How lit a cell of the box's edge is, `around` cells clockwise from its
/// top left out of n: a light with a fading tail, round the box every
/// 2.4s (yapGlint).
pub fn yap_glint(frame: usize, around: usize, n: usize) -> f64 {
    const LAP: usize = 48;
    const TAIL: f64 = 28.0;
    if n == 0 {
        return 0.0;
    }
    let at = (frame % LAP) * n / LAP;
    let d = ((at as isize - around as isize) % n as isize + n as isize) % n as isize;
    let d = d as f64;
    if d >= TAIL {
        return 0.0;
    }
    let k = 1.0 - d / TAIL;
    k * k
}

/// The look's inks at step i of YAP_STEPS: the settled words, the guessed
/// ones, the box's edge, and the cursor's ground (yapColours).
pub fn yap_ink(i: usize) -> (Color, Color, Color, Color) {
    let t = i.min(YAP_STEPS) as f64 / YAP_STEPS as f64;
    let text = Color::Rgb(226, 221, 211);
    let lit = Color::Rgb(255, 250, 240);
    let ground = Color::Rgb(40, 36, 32);
    let (orange, glint) = (Color::Rgb(217, 119, 87), Color::Rgb(255, 232, 205));
    (
        mix(text, lit, t),
        mix(mix(text, ground, 0.5), lit, t),
        mix(orange, glint, t),
        mix(mix(orange, ground, 0.55), mix(orange, glint, 0.25), t),
    )
}
