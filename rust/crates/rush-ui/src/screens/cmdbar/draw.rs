//! The command bar drawn: the query, the rows that fit by section, the
//! keys under them, and its frame dropped a little below the top, glowing
//! from orange to ember with a light running round it while transcripts
//! are searched, and a shadow on the screen behind. The keys guide uses
//! the same frame (drop_box). Port of internal/ui/cmdbar.go:1411-1750
//! (barBox :1411, barHolder :1486, barHint :1502, barStatus :1528,
//! barHeading :1547, barRow :1564, litTitle :1589, overlayBar :1615,
//! dropBox :1636, barEdge :1677) and style.go:86-87 (barChip, barShadow).

use std::time::{Duration, Instant};

use rush_core::text::{Color, Line, Span, Style, width};

use super::bar::{Bar, BarState, Item, ScopeKind};
use crate::frame::dialog::{cells, panel};
use crate::model::Model;
use crate::style::*;

/// The scope's chip in the bar's box (barChip).
pub const BAR_CHIP: Color = Color::Rgb(64, 45, 37);
/// The shadow under the bar: its ground and the screen's ink there
/// (barShadow).
pub const SHADOW_BG: Color = Color::Rgb(12, 11, 10);
pub const SHADOW_FG: Color = Color::Rgb(44, 41, 38);

/// How often the glint moves while a search runs (glintTick).
const GLINT_EVERY: Duration = Duration::from_millis(40);

/// left on the left, right on the right, blanks between; left alone, cut
/// to fit, when there's no room (rushmode.go:2175 spread).
pub fn spread(left: Line, right: Line, w: usize) -> Line {
    let gap = w as isize - left.width() as isize - right.width() as isize;
    if gap < 2 {
        return fit(left, w);
    }
    let mut out = left;
    out.spans.push(plain(blanks(gap as usize)));
    join(&mut out, right);
    out
}

/// The cells [from, to) of a plain row.
fn cut(s: &str, from: usize, to: usize) -> String {
    cells(&line([plain(s)]), from, to).plain()
}

/// What the empty box says it takes, in each scope (barHolder).
pub fn holder(b: &Bar) -> String {
    let sc = b.scope();
    match sc.kind {
        ScopeKind::Chat => {
            "words in this chat · #12 for turn 12 · is:failed is:edit file:x turn:3-5".into()
        }
        ScopeKind::Group => format!("an agent in {}, or words they said", sc.group),
        ScopeKind::Agents => "an agent, or words any agent said".into(),
        ScopeKind::Place => format!("a page of {}", sc.label),
        ScopeKind::All => {
            "an agent, words said in any conversation, or a place · # for commands".into()
        }
    }
}

/// The text field: what's typed with the cursor in it, or what it takes.
pub fn field(q: &[char], pos: usize, holder: &str) -> Line {
    let cur = sp("▏", ORANGE);
    if q.is_empty() {
        return line([cur, sp(holder, DIM)]);
    }
    let a: String = q[..pos.min(q.len())].iter().collect();
    let b: String = q[pos.min(q.len())..].iter().collect();
    line([sp(a, TEXT), cur, sp(b, TEXT)])
}

/// The bar's body, w wide and at most h rows (barBox). Notes which row
/// each item landed on, counted from the body's top.
pub fn bar_box(m: &mut Model, w: usize, h: usize) -> Vec<Line> {
    let sort = m.kept::<BarState>().sort;
    let Some(b) = m.kept::<BarState>().bar.as_mut() else {
        return Vec::new();
    };
    let inner = w.saturating_sub(4);
    let sc = b.scope().clone();
    let mut head = line([spb("❯ ", ORANGE)]);
    join(&mut head, field(&b.query, b.pos, &holder(b)));
    if sc.kind != ScopeKind::All {
        let chip = |t: Span| {
            let mut t = t;
            t.style.bg = BAR_CHIP;
            t
        };
        let mut l = line([
            chip(sp(" ⌕ ", ORANGE)),
            chip(spb(format!("{} ", sc.label), TEXT)),
            plain(" "),
        ]);
        join(&mut l, head);
        head = l;
    }
    let mut out = vec![fit(head, inner), sp_line("─".repeat(inner), FAINT)];

    // Metas line up in one column, just past the longest title that has
    // one.
    let col = b
        .items
        .iter()
        .filter(|it| !it.meta.is_empty())
        .map(|it| width(&one_line(&it.title)))
        .max()
        .unwrap_or(0);
    let mut rows: Vec<(Line, Option<usize>)> = Vec::new();
    for (i, it) in b.items.iter().enumerate() {
        if i == 0 || it.section != b.items[i - 1].section {
            if i > 0 {
                rows.push((Line::new(), None));
            }
            rows.push((heading(b, &it.section, inner), None));
        }
        rows.push((bar_row(it, inner, col, i == b.cursor), Some(i)));
    }
    if b.items.is_empty() {
        rows.push((Line::new(), None));
        let t = if b.search.is_some() && !b.done {
            "  searching conversations…"
        } else {
            "  nothing matches"
        };
        rows.push((sp_line(t, DIM), None));
    }
    let avail = h.saturating_sub(out.len() + 2).max(3);
    let sel = rows
        .iter()
        .position(|(_, it)| *it == Some(b.cursor))
        .unwrap_or(0);
    // Keep the selection on screen, with its heading when it's near the
    // top.
    if sel < b.top + 1 {
        b.top = sel.saturating_sub(2);
    }
    if sel >= b.top + avail {
        b.top = sel + 1 - avail;
    }
    b.top = b.top.min(rows.len().saturating_sub(avail));
    b.rows_at.clear();
    let end = rows.len().min(b.top + avail);
    for (l, it) in rows.drain(b.top..end) {
        if let Some(i) = it {
            b.rows_at.push((out.len(), i));
        }
        out.push(l);
    }
    out.push(Line::new());
    b.hint_at = Some((out.len(), 0));
    out.push(hint(b, sort, inner));
    out
}

fn sp_line(s: impl Into<String>, c: Color) -> Line {
    line([sp(s, c)])
}

/// The row of keys under the results: what ctrl+f and ctrl+k will do from
/// here, so the scopes teach themselves (barHint).
pub fn hint(b: &Bar, sort: &str, w: usize) -> Line {
    let last = b.scopes.len() - 1;
    let wider = b
        .scopes
        .get(b.at + 1)
        .map(|s| format!("wider: {}", s.label))
        .unwrap_or_default();
    let just = format!("just {}", b.scopes[0].label);
    let by = format!("by {}", if sort.is_empty() { "status" } else { sort });
    let mut pairs: Vec<(&str, &str)> = vec![("↑↓", "choose"), ("enter", "go")];
    if b.at + 1 < last {
        pairs.extend([("ctrl+f", wider.as_str()), ("ctrl+k", "everywhere")]);
    } else if b.at + 1 == last {
        pairs.push(("ctrl+f · ctrl+k", "everywhere"));
    } else if last > 0 {
        pairs.push(("ctrl+f", just.as_str()));
    }
    if b.scope().kind == ScopeKind::All {
        pairs.extend([
            ("#", "commands"),
            ("tab", "next section"),
            ("ctrl+s", by.as_str()),
        ]);
    }
    if b.scope().transcripts() {
        pairs.push(("in:name is:failed", "narrow"));
    }
    pairs.push(("esc", "close"));
    keys_fit(w, &pairs)
}

/// How the transcript search is going, for the frame's bottom edge
/// (barStatus).
pub fn status(b: &Bar) -> Line {
    if b.search.is_none() {
        return Line::new();
    }
    if !b.done {
        return sp_line(
            format!(" searching {}/{} conversations ", b.searched, b.to_search),
            SUB,
        );
    }
    let n: usize = b.found.iter().map(|f| f.found.total).sum();
    sp_line(
        format!(
            " {n} found in {} of {} conversations ",
            b.found.len(),
            b.to_search
        ),
        DIM,
    )
}

/// A section's heading; Conversations says how many more there are than
/// it shows (barHeading).
fn heading(b: &Bar, section: &str, w: usize) -> Line {
    let mut meta = String::new();
    if section == "Conversations" {
        let shown: usize = b.found.iter().map(|f| f.found.hits.len()).sum();
        let total: usize = b.found.iter().map(|f| f.found.total).sum();
        if total > shown {
            meta = format!("{} more; open one to see all", total - shown);
        }
    }
    rule(section, &meta, w)
}

/// One result in columns: its glyph, the title lit where it matched, its
/// meta in a column of its own, and a short tail at the right edge
/// (barRow).
pub fn bar_row(it: &Item, w: usize, col: usize, sel: bool) -> Line {
    let tail = if it.tail.is_empty() {
        Line::new()
    } else {
        line([plain("  "), sp(&it.tail, DIM)])
    };
    let tl = tail.width();
    let mut tw = w as isize - 3 - tl as isize;
    if !it.meta.is_empty() {
        tw = tw.min(12.max(col.min(w * 9 / 20)) as isize);
    }
    let tw = tw.max(1) as usize;
    let mut l = line([plain(" "), it.glyph.clone(), plain(" ")]);
    join(&mut l, lit_title(&one_line(&it.title), &it.lit, tw));
    if !it.meta.is_empty() {
        let lw = l.width();
        let gap = 2.max((3 + tw + 2) as isize - lw as isize) as usize;
        l.spans.push(plain(blanks(gap)));
        let mw = w as isize - l.width() as isize - tl as isize;
        if mw > 3 {
            l.spans
                .push(sp(rush_draw::truncate(&it.meta, mw as usize, "…"), DIM));
        }
    }
    let gap = w.saturating_sub(l.width() + tl);
    l.spans.push(plain(blanks(gap)));
    join(&mut l, tail);
    if sel {
        l.spans[0] = sp("▍", ORANGE);
        return highlight(l, w);
    }
    l
}

/// title painted, its matched chars bright, cut to w cells (litTitle).
pub fn lit_title(title: &str, lit: &[usize], w: usize) -> Line {
    let t = if width(title) > w {
        rush_draw::truncate(title, w, "…").into_owned()
    } else {
        title.to_string()
    };
    if lit.is_empty() {
        return sp_line(t, TEXT);
    }
    let mut l = Line::new();
    for (i, c) in t.chars().enumerate() {
        let s = c.to_string();
        let on = lit.contains(&i);
        let span = if on { spb(s, ORANGE) } else { sp(s, TEXT) };
        // Neighbours of one look run together.
        match l.spans.last_mut() {
            Some(last) if last.style == span.style => last.text.push(c),
            _ => l.spans.push(span),
        }
    }
    l
}

/// The bar over the screen (overlayBar).
pub fn overlay(m: &mut Model, out: &mut [Line]) {
    let bw = m.w.saturating_sub(6).min(110);
    let bh = 8.max(m.h * 2 / 3);
    let body = bar_box(m, bw, bh.saturating_sub(2));
    let Some(b) = m.kept::<BarState>().bar.as_ref() else {
        return;
    };
    let searching = b.search.as_ref().filter(|_| !b.done).map(|s| s.started);
    let sc = b.scope().clone();
    let st = status(b);
    let mut e = Edge {
        w: bw,
        h: body.len() + 2,
        glint: None,
    };
    if let Some(at) = searching {
        // The glint moves 3 cells every 40 ms while a search runs.
        let n = at.elapsed().as_millis() / GLINT_EVERY.as_millis();
        e.glint = Some(n as usize * 3);
        m.ask_wake(Instant::now() + GLINT_EVERY);
    }
    let (title, key) = if sc.kind == ScopeKind::All {
        (" ✻ Go anywhere ".to_string(), " ctrl+k ")
    } else {
        (format!(" ⌕ Find in {} ", sc.label), " ctrl+f ")
    };
    drop_box(
        m,
        out,
        &body,
        e,
        line([spb(title, TEXT)]),
        line([sp(key, DIM)]),
        st,
    );
}

/// Draws body in the command bar's frame over lines, a little below the
/// top: title and key set into its top edge, status into the bottom one,
/// and a shadow on the screen behind (dropBox). Returns where the body's
/// first row landed: (column, row).
pub fn drop_box(
    m: &mut Model,
    lines: &mut [Line],
    body: &[Line],
    e: Edge,
    title: Line,
    key: Line,
    status: Line,
) -> (usize, usize) {
    let (bw, inner) = (e.w, e.w.saturating_sub(4));
    let mut b = vec![e.line(0, "╭", "╮", title, key)];
    for (i, l) in body.iter().enumerate() {
        let mut row = line([e.cell(0, i + 1, "│")]);
        let mut inside = line([plain(" ")]);
        join(&mut inside, fit(l.clone(), inner));
        inside.spans.push(plain(" "));
        join(&mut row, panel(inside, inner + 2));
        row.spans.push(e.cell(bw - 1, i + 1, "│"));
        b.push(row);
    }
    b.push(e.line(e.h - 1, "╰", "╯", Line::new(), status));
    let w = m.w;
    let top = 1.max((m.h / 8).min(lines.len().saturating_sub(b.len() + 1)));
    let left = w.saturating_sub(bw + 2) / 2;
    m.keep_overlay(left, top, bw, b.len(), left + 2, top + 1);
    let plain_rows: Vec<String> = lines.iter().map(|l| fit(l.clone(), w).plain()).collect();
    for (y, l) in lines.iter_mut().enumerate() {
        *l = sp_line(plain_rows[y].clone(), FAINT);
    }
    // The shadow: two cells right of the frame and one row under it, the
    // screen there darker still.
    let shade = |y: usize, from: usize, to: usize| {
        let mut s = Style::fg(SHADOW_FG);
        s.bg = SHADOW_BG;
        Span::new(cut(&plain_rows[y], from, to), s)
    };
    for (i, r) in b.iter().enumerate() {
        let y = top + i;
        if y >= lines.len() {
            break;
        }
        let mut l = line([sp(cut(&plain_rows[y], 0, left), FAINT)]);
        join(&mut l, r.clone());
        if i > 0 {
            l.spans.push(shade(y, left + bw, left + bw + 2));
            l.spans
                .push(sp(cut(&plain_rows[y], left + bw + 2, w), FAINT));
        } else {
            l.spans.push(sp(cut(&plain_rows[y], left + bw, w), FAINT));
        }
        lines[y] = l;
    }
    let y = top + b.len();
    if y < lines.len() {
        lines[y] = line([
            sp(cut(&plain_rows[y], 0, left + 2), FAINT),
            shade(y, left + 2, left + bw + 2),
            sp(cut(&plain_rows[y], left + bw + 2, w), FAINT),
        ]);
    }
    (left + 2, top + 1)
}

/// Colours the bar's frame cell by cell: orange at the top left cooling to
/// ember at the bottom right, and, while transcripts are being searched, a
/// glint with a fading tail running clockwise round it (barEdge).
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub w: usize,
    pub h: usize,
    /// Where the glint is along the edge.
    pub glint: Option<usize>,
}

impl Edge {
    /// How far along the edge, clockwise from the top left, a cell is.
    fn around(&self, x: usize, y: usize) -> isize {
        let (w, h, x, y) = (self.w as isize, self.h as isize, x as isize, y as isize);
        if y == 0 {
            x
        } else if x == w - 1 {
            w - 1 + y
        } else if y == h - 1 {
            2 * (w - 1) + h - 1 - x
        } else {
            2 * (w - 1) + 2 * (h - 1) - y
        }
    }

    pub fn color(&self, x: usize, y: usize) -> Color {
        let t = 0.65 * x as f64 / (self.w.max(2) - 1) as f64
            + 0.35 * y as f64 / (self.h.max(2) - 1) as f64;
        let mut c = mixf([232.0, 128.0, 92.0], [110.0, 52.0, 40.0], t);
        if let Some(g) = self.glint {
            let n = (2 * (self.w.max(1) - 1) + 2 * (self.h.max(1) - 1)).max(1) as isize;
            let d = ((g as isize - self.around(x, y)) % n + n) % n;
            if d < 14 {
                let k = 1.0 - d as f64 / 14.0;
                c = mixf(c, [255.0, 232.0, 205.0], k * k);
            }
        }
        Color::Rgb(c[0] as u8, c[1] as u8, c[2] as u8)
    }

    fn cell(&self, x: usize, y: usize, s: &str) -> Span {
        sp(s, self.color(x, y))
    }

    /// The top or bottom edge, with a label set into it at each end.
    pub fn line(&self, y: usize, l: &str, r: &str, left: Line, right: Line) -> Line {
        let mut out = line([self.cell(0, y, l)]);
        let mut x = 1;
        let (mut left, mut right) = (left, right);
        let (mut lw, mut rw) = (left.width(), right.width());
        if lw + rw + 6 > self.w {
            (left, right, lw, rw) = (Line::new(), Line::new(), 0, 0);
        }
        let dash = |out: &mut Line, x: &mut usize, n: isize| {
            for _ in 0..n.max(0) {
                out.spans.push(self.cell(*x, y, "─"));
                *x += 1;
            }
        };
        if lw > 0 {
            dash(&mut out, &mut x, 1);
            join(&mut out, left);
            x += lw;
        }
        let n = self.w as isize - 1 - x as isize - rw as isize - isize::from(rw > 0);
        dash(&mut out, &mut x, n);
        if rw > 0 {
            join(&mut out, right);
            x += rw;
            dash(&mut out, &mut x, 1);
        }
        out.spans.push(self.cell(self.w.saturating_sub(1), y, r));
        out
    }
}

fn mixf(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
