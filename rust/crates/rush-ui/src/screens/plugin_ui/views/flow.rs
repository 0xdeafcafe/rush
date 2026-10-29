//! Flow styles: steps of a pipeline left to right, a timeline down, a
//! board of columns of cards, and a tree whose branches a click folds.
//! After Go's internal/ui/viewblocks_flow.go (stepMark :22, blockSteps
//! :40, blockTimeline :108, blockBoard :146, boardHead :218, boardCard
//! :229, blockTree :252).

use rush_core::text::{Color, Line};

use super::acts::{ViewAct, ViewLine, open_row};
use super::blocks::BlockCtx;
use super::*;

/// A step's mark and colour: done, now, failed, to come.
fn step_mark(tone: &str) -> (&'static str, Color) {
    match tone {
        "good" => ("●", GREEN),
        "busy" => ("◉", BLUE),
        "bad" => ("✗", RED),
        "warn" => ("▲", YELLOW),
        _ => ("○", DIM),
    }
}

/// A pipeline: its steps left to right joined by a line, solid as far as
/// they've gone, each one's word under it; down the panel when they don't
/// fit across (blockSteps).
pub(super) fn steps(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let its = &b.items;
    if its.is_empty() {
        return Vec::new();
    }
    const JOIN: usize = 4; // " ━━ "
    let across: usize =
        its.iter().map(|it| 2 + cw(&it.text)).sum::<usize>() + JOIN * (its.len() - 1);
    if across <= w {
        let (mut top, mut under) = (Line::new(), Line::new());
        let mut used = 0;
        for (i, it) in its.iter().enumerate() {
            let (mark, col) = step_mark(&it.tone);
            if i > 0 {
                if its[i - 1].tone == "good" {
                    top.spans
                        .extend([plain(" "), paint(GREEN, "━━"), plain(" ")]);
                } else {
                    top.spans.push(faint(" ── "));
                }
                under.spans.push(plain(blanks(JOIN)));
                used += JOIN;
            }
            top.spans.push(paint(col, mark));
            top.spans.push(plain(" "));
            if it.tone.is_empty() {
                top.spans.push(dim(it.text.clone()));
            } else {
                top.spans.push(paint(TEXT, it.text.clone()));
            }
            let nw = 2 + cw(&it.text);
            // A step's word goes under it, as far as the next one starts.
            let room = if i == its.len() - 1 {
                w.saturating_sub(used)
            } else {
                nw + JOIN
            };
            under
                .spans
                .extend(fit(ln([dim(trunc(&it.meta, room.saturating_sub(1)))]), nw).spans);
            used += nw;
        }
        let mut out = vec![ViewLine::plain(line(top.spans))];
        if !under.plain().trim().is_empty() {
            out.push(ViewLine::plain(line(under.spans)));
        }
        return out;
    }
    let mut out = Vec::new();
    for (i, it) in its.iter().enumerate() {
        let (mark, col) = step_mark(&it.tone);
        let meta = dim(it.meta.clone());
        let room = w.saturating_sub(2 + meta.width() + 1);
        let row = cat([
            ln([paint(col, mark), plain(" ")]),
            fit(ln([paint(TEXT, trunc(&it.text, room))]), room),
            ln([plain(" "), meta]),
        ]);
        out.extend(cx.row_lines(open_row(&cx.prefix, it), w, vec![row]));
        if i < its.len() - 1 {
            let edge = if it.tone == "good" {
                paint(GREEN, "┃")
            } else {
                faint("│")
            };
            out.push(ViewLine::plain(ln([edge])));
        }
    }
    out
}

/// Events down a line, each at its time with its mark, its word under it
/// beside the line (blockTimeline).
pub(super) fn timeline(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let tw = b.items.iter().map(|it| cw(&it.time)).max().unwrap_or(0);
    let mut out = Vec::new();
    let n = b.items.len();
    for (i, it) in b.items.iter().enumerate() {
        let (mut mark, mut col) = tone_mark(&it.tone);
        if it.tone.is_empty() {
            (mark, col) = ("●", cx.accent);
        }
        let room = w.saturating_sub(tw + 3);
        let row = ln([
            dim(fit_str(&it.time, tw)),
            plain(" "),
            paint(col, mark),
            plain(" "),
            paint(TEXT, trunc(&it.text, room)),
        ]);
        out.extend(cx.row_lines(open_row(&cx.prefix, it), w, vec![row]));
        let last = i == n - 1;
        if !it.meta.is_empty() {
            let edge = if last { plain(" ") } else { faint("│") };
            out.push(ViewLine::plain(ln([
                plain(blanks(tw) + " "),
                edge,
                plain(" "),
                dim(trunc(&it.meta, room)),
            ])));
        }
        if !last {
            out.push(ViewLine::plain(ln([plain(blanks(tw) + " "), faint("│")])));
        }
    }
    out
}

/// Columns of cards side by side, each column's title over a rule in its
/// tone, each card its mark, its words and a quiet line; one column under
/// another when four won't fit across (blockBoard).
pub(super) fn board(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let n = b.items.len();
    if n == 0 {
        return Vec::new();
    }
    let cwid = (w as isize - (n as isize - 1)) / n as isize;
    if n > 4 || cwid < 12 {
        let mut out = Vec::new();
        for (i, col) in b.items.iter().enumerate() {
            if i > 0 {
                out.push(ViewLine::default());
            }
            out.extend(board_head(col, w).into_iter().map(ViewLine::plain));
            for card in &col.items {
                let ls: Vec<Line> = board_card(card, w).into_iter().map(|l| l.s).collect();
                let mut o = open_row(&item_key(&cx.prefix, &col.id, &col.text, &[]), card);
                o.cut = wrap(&card.text, 8.max(w.saturating_sub(2))).len() > 2;
                out.extend(cx.row_lines(o, w, ls));
            }
        }
        return out;
    }
    let cwid = cwid as usize;
    let width_of = |i: usize| {
        if i == n - 1 {
            w - (cwid + 1) * (n - 1)
        } else {
            cwid
        }
    };
    let mut cols: Vec<Vec<Line>> = vec![Vec::new(); n];
    for (i, col) in b.items.iter().enumerate() {
        let width = width_of(i);
        cols[i].extend(board_head(col, width));
        for (j, card) in col.items.iter().enumerate() {
            if j > 0 {
                cols[i].push(Line::new());
            }
            cols[i].extend(board_card(card, width).into_iter().map(|l| l.s));
        }
    }
    let h = cols.iter().map(Vec::len).max().unwrap_or(0);
    (0..h)
        .map(|y| {
            let mut l = Line::new();
            for (i, c) in cols.iter().enumerate() {
                if i > 0 {
                    l.spans.push(plain(" "));
                }
                let s = c.get(y).cloned().unwrap_or_default();
                l.spans.extend(fit(s, width_of(i)).spans);
            }
            ViewLine::plain(line(l.spans))
        })
        .collect()
}

/// A board column's title and count over a rule in its tone (boardHead).
fn board_head(col: &Item, w: usize) -> Vec<Line> {
    let c = if col.tone.is_empty() {
        SUB
    } else {
        tone_edge(&col.tone)
    };
    let count = format!(" {}", col.items.len());
    vec![
        ln([
            bolded(c, trunc(&col.text, w.saturating_sub(count.len()))),
            dim(count),
        ]),
        ln([paint(c, "▔".repeat(w))]),
    ]
}

/// A card w wide: its mark and up to two lines of its words, then its
/// quiet line (boardCard).
fn board_card(card: &Item, w: usize) -> Vec<ViewLine> {
    let (mark, col) = tone_mark(&card.tone);
    let mut lines = wrap(&card.text, 8.max(w.saturating_sub(2)));
    if lines.len() > 2 {
        let rest = lines[1..].join(" ");
        lines.truncate(1);
        lines.push(trunc(&rest, w.saturating_sub(2)));
    }
    let act = ViewAct {
        link: card.link.clone(),
        ..Default::default()
    };
    let mut out = Vec::new();
    for (i, l) in lines.into_iter().enumerate() {
        let lead = if i == 0 {
            ln([paint(col, mark), plain(" ")])
        } else {
            ln([plain("  ")])
        };
        out.push(ViewLine::with(
            cat([lead, ln([paint(TEXT, l)])]),
            act.clone(),
        ));
    }
    if !card.meta.is_empty() {
        out.push(ViewLine::with(
            ln([plain("  "), dim(trunc(&card.meta, w.saturating_sub(2)))]),
            act,
        ));
    }
    out
}

/// Nodes under their parents along guides, each with its mark and its
/// word at the end; a click on a node with children folds them
/// (blockTree).
pub(super) fn tree(b: &Block, w: usize, cx: &BlockCtx) -> Vec<ViewLine> {
    let mut out = Vec::new();
    walk(&b.items, "", "", &cx.prefix, w, cx, &mut out);
    out
}

fn walk(
    items: &[Item],
    lead: &str,
    path: &str,
    prefix: &str,
    w: usize,
    cx: &BlockCtx,
    out: &mut Vec<ViewLine>,
) {
    for (i, it) in items.iter().enumerate() {
        let last = i == items.len() - 1;
        let (mut branch, mut next) = if last {
            ("└─ ", "   ")
        } else {
            ("├─ ", "│  ")
        };
        if path.is_empty() {
            // the top: no guide
            (branch, next) = ("", "");
        }
        let key = format!("{}/t{path}{i}", cx.key);
        let folded = cx.st.fold.get(&key).copied().unwrap_or(false);
        let (mut mark, mut col) = tone_mark(&it.tone);
        let mut act = ViewAct {
            link: it.link.clone(),
            ..Default::default()
        };
        if !it.items.is_empty() {
            mark = if folded { "▸" } else { "▾" };
            col = if it.tone.is_empty() {
                SUB
            } else {
                tone_edge(&it.tone)
            };
            act = ViewAct {
                fold: key,
                folded,
                ..Default::default()
            };
        }
        let o = open_row(prefix, it);
        let okey = o.key.clone();
        let meta = dim(it.meta.clone());
        let pre = ln([
            faint(format!("{lead}{branch}")),
            paint(col, mark),
            plain(" "),
        ]);
        let room = w.saturating_sub(pre.width() + meta.width() + 1);
        let text = if it.items.is_empty() {
            paint(TEXT, trunc(&it.text, room))
        } else {
            bolded(TEXT, trunc(&it.text, room))
        };
        let row = cat([pre, fit(ln([text]), room), ln([plain(" "), meta])]);
        if !act.fold.is_empty() {
            out.push(ViewLine::with(row, act)); // a click folds it
        } else {
            out.extend(cx.row_lines(o, w, vec![row]));
        }
        if !it.items.is_empty() && !folded {
            walk(
                &it.items,
                &format!("{lead}{next}"),
                &format!("{path}{i}."),
                &okey,
                w,
                cx,
                out,
            );
        }
    }
}
