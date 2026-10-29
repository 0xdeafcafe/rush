//! Tabular styles: a table whose header a click sorts by, keys and their
//! values, a checklist, a log, and things before and after. After Go's
//! internal/ui/viewblocks_tabular.go (numeric :25, cellNum :33, blockTable
//! :48, blockKV :178, blockChecklist :202, blockLog :233, blockCompare
//! :259).

use std::cmp::Ordering;

use rush_core::text::Line;

use super::acts::{ViewAct, ViewLine, open_row};
use super::blocks::BlockCtx;
use super::*;

/// What's left of a cell once its money signs, units and spaces go.
fn bare(s: &str) -> String {
    let s = s
        .trim_start_matches(['$', '€', '£', '+', '-', '−', '~'])
        .trim();
    s.trim_end_matches(['%', 'k', 'K', 'm', 'M', 'g', 'G', 'b', 'B', 's', 'x', ' '])
        .to_string()
}

/// Whether a cell reads as a number, to sit right (numeric).
pub(super) fn numeric(s: &str) -> bool {
    let b = bare(s);
    !b.is_empty() && b.replace(',', "").parse::<f64>().is_ok()
}

/// A cell's number, to sort by; None when it isn't one (cellNum).
pub(super) fn cell_num(s: &str) -> Option<f64> {
    if !numeric(s) {
        return None;
    }
    let s = s.trim_start_matches(['$', '€', '£', '+', '~']).trim();
    let s = s.replacen('−', "-", 1);
    let s = s.trim_end_matches(['%', 'k', 'K', 'm', 'M', 'g', 'G', 'b', 'B', 's', 'x', ' ']);
    s.replace(',', "").parse().ok()
}

fn cell_at(cells: &[String], c: usize) -> &str {
    cells.get(c).map(String::as_str).unwrap_or("")
}

/// Columns over rows, each column as wide as it needs, the widest cut to
/// fit; numbers sit right. A click on the header sorts by the column
/// under it, again the other way (blockTable).
pub(super) fn table(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let n = b
        .items
        .iter()
        .map(|it| it.cells.len())
        .max()
        .unwrap_or(0)
        .max(b.columns.len());
    if n == 0 {
        return Vec::new();
    }
    let gutter = if b.items.iter().any(|it| !it.tone.is_empty()) {
        2
    } else {
        0
    };
    let mut widths = vec![0usize; n];
    let mut right = vec![false; n];
    for c in 0..n {
        if let Some(name) = b.columns.get(c) {
            widths[c] = cw(name);
        }
        right[c] = c > 0;
        for it in &b.items {
            if let Some(cell) = it.cells.get(c) {
                widths[c] = widths[c].max(cw(cell));
                if !cell.is_empty() && !numeric(cell) {
                    right[c] = false;
                }
            }
        }
    }
    // Cut the widest until the row fits, two spaces between columns.
    let room = w as isize - gutter as isize - 2 * (n as isize - 1);
    while (widths.iter().sum::<usize>() as isize) > room {
        let mut i = 0;
        for c in 0..widths.len() {
            if widths[c] > widths[i] {
                i = c;
            }
        }
        if widths[i] <= 3 {
            break;
        }
        widths[i] -= 1;
    }
    let cell = |s: &str, c: usize| -> String {
        let s = trunc(s, widths[c]);
        let pad = blanks(widths[c].saturating_sub(cw(&s)));
        if right[c] { pad + &s } else { s + &pad }
    };
    let mut starts = vec![0; n];
    let mut at = 2 + gutter; // the panel's edge and its space
    for (c, s) in starts.iter_mut().enumerate() {
        *s = at;
        at += widths[c] + 2;
    }
    let mut rows: Vec<&Item> = b.items.iter().collect();
    let sort_by = cx.st.sorts.get(&cx.key).copied().unwrap_or(0);
    if sort_by != 0 {
        let (c, desc) = (sort_by.unsigned_abs() as usize - 1, sort_by < 0);
        rows.sort_by(|x, y| {
            let (a, bb) = (cell_at(&x.cells, c), cell_at(&y.cells, c));
            let r = match cell_num(a) {
                Some(fa) => match cell_num(bb) {
                    Some(fb) => fa.partial_cmp(&fb).unwrap_or(Ordering::Equal),
                    None => Ordering::Equal,
                },
                None => a.to_lowercase().cmp(&bb.to_lowercase()),
            };
            if desc { r.reverse() } else { r }
        });
    }
    let mut out = Vec::new();
    if !b.columns.is_empty() {
        let mut h = ln([plain(blanks(gutter))]);
        #[allow(clippy::needless_range_loop)] // c names the column too
        for c in 0..n {
            if c > 0 {
                h.spans.push(plain("  "));
            }
            let mut name = b.columns.get(c).cloned().unwrap_or_default();
            let mut col = SUB;
            if sort_by != 0 && sort_by.unsigned_abs() as usize - 1 == c {
                col = BRIGHT;
                let arrow = if sort_by < 0 { "↓" } else { "↑" };
                name = trunc(&name, widths[c].saturating_sub(1)) + arrow;
            }
            h.spans.push(bolded(col, cell(&name, c)));
        }
        out.push(ViewLine::with(
            line(h.spans),
            ViewAct {
                sort: cx.key.clone(),
                cols: starts.clone(),
                ..Default::default()
            },
        ));
        out.push(ViewLine::plain(ln([faint(
            "─".repeat(w.min(at.saturating_sub(4))),
        )])));
    }
    for it in rows {
        let mut r = Line::new();
        if gutter > 0 {
            let (mut mark, col) = tone_mark(&it.tone);
            if it.tone.is_empty() {
                mark = " ";
            }
            r.spans.push(paint(col, mark));
            r.spans.push(plain(" "));
        }
        for c in 0..n {
            if c > 0 {
                r.spans.push(plain("  "));
            }
            let col = if c == 0 { TEXT } else { SUB };
            r.spans.push(paint(col, cell(cell_at(&it.cells, c), c)));
        }
        let mut o = open_row(&cx.prefix, it);
        o.cols = b.columns.clone();
        out.extend(cx.row_lines(o, w, vec![line(r.spans)]));
    }
    out
}

/// Keys down the left, quiet, and their values beside them, in their tone
/// (blockKV).
pub(super) fn kv(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let key_w = b
        .items
        .iter()
        .map(|it| cw(&it.text))
        .max()
        .unwrap_or(0)
        .min(w * 2 / 5);
    let mut out = Vec::new();
    for it in &b.items {
        let col = if it.tone.is_empty() {
            TEXT
        } else {
            tone_edge(&it.tone)
        };
        let vroom = w.saturating_sub(key_w + 2);
        let mut v = ln([paint(col, trunc(&it.meta, vroom))]);
        if !it.link.is_empty() {
            v.spans.push(paint(SUB, " ↗"));
        }
        let mut o = open_row(&cx.prefix, it);
        o.cut = cw(&it.text) > key_w || cw(&it.meta) > vroom;
        let row = cat([
            ln([
                paint(SUB, fit_str(&trunc(&it.text, key_w), key_w)),
                plain("  "),
            ]),
            v,
        ]);
        out.extend(cx.row_lines(o, w, vec![row]));
    }
    out
}

/// Things to do, ticked when done and quieter for it, with how many are
/// done along its foot (blockChecklist).
pub(super) fn checklist(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let mut out = Vec::new();
    let mut done_n = 0;
    for it in &b.items {
        let done = it.done == Some(true);
        let (mark, text) = if done {
            done_n += 1;
            (paint(GREEN, "✓"), dim(it.text.clone()))
        } else if !it.tone.is_empty() {
            let (m, col) = tone_mark(&it.tone);
            (paint(col, m), paint(TEXT, it.text.clone()))
        } else {
            (paint(SUB, "○"), paint(TEXT, it.text.clone()))
        };
        let meta = dim(it.meta.clone());
        let room = w.saturating_sub(2 + meta.width() + 1);
        let row = cat([
            ln([mark, plain(" ")]),
            fit(fit(ln([text]), room), room),
            ln([plain(" "), meta]),
        ]);
        let mut o = open_row(&cx.prefix, it);
        o.tick = Some(done);
        out.extend(cx.row_lines(o, w, vec![row]));
    }
    if b.items.len() > 2 {
        let frac = done_n as f64 / b.items.len() as f64;
        let words = format!(" {done_n} of {}", b.items.len());
        let mut l = progress_bar(frac, w.saturating_sub(cw(&words)), GREEN);
        l.spans.push(dim(words));
        out.push(ViewLine::plain(l));
    }
    out
}

/// Lines as a program wrote them, each after its time, the bad and the
/// warnings in their colour (blockLog).
pub(super) fn log(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let tw = b.items.iter().map(|it| cw(&it.time)).max().unwrap_or(0);
    let mut out = Vec::new();
    for it in &b.items {
        let col = match it.tone.as_str() {
            "bad" | "warn" | "good" | "busy" => tone_edge(&it.tone),
            "" => TEXT,
            _ => SUB,
        };
        let (mut row, room) = if tw > 0 {
            (
                ln([faint(fit_str(&it.time, tw)), plain(" ")]),
                w.saturating_sub(tw + 1),
            )
        } else {
            (Line::new(), w)
        };
        row.spans.push(paint(col, trunc(&it.text, room)));
        out.extend(cx.row_lines(open_row(&cx.prefix, it), w, vec![line(row.spans)]));
    }
    out
}

/// Things before and after: what it is, then the old quiet and the new
/// bright in its tone (blockCompare).
pub(super) fn compare(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let from_w = b.items.iter().map(|it| cw(&it.from)).max().unwrap_or(0);
    let to_w = b.items.iter().map(|it| cw(&it.to)).max().unwrap_or(0);
    let mut out = Vec::new();
    for it in &b.items {
        let col = if it.tone.is_empty() {
            BRIGHT
        } else {
            tone_edge(&it.tone)
        };
        let right = ln([
            dim(blanks(from_w - cw(&it.from)) + &it.from),
            faint(" → "),
            bolded(col, blanks(to_w - cw(&it.to)) + &it.to),
        ]);
        let room = w.saturating_sub(right.width() + 1);
        let row = cat([
            fit(ln([paint(TEXT, trunc(&it.text, room))]), room),
            ln([plain(" ")]),
            right,
        ]);
        out.extend(cx.row_lines(open_row(&cx.prefix, it), w, vec![row]));
    }
    out
}
