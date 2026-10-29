//! A view's blocks: its parts in styles of their own, tabular (a table,
//! keys and values, a checklist, a log, before and after), visual (bars,
//! a chart, a heatmap), flow (steps, a timeline, a board, a tree) and
//! expressive (a hero number, a callout, code, words), each under its
//! title, a rule a click folds. After Go's internal/ui/viewblocks.go.

use rush_core::text::Color;

use super::acts::{ViewAct, ViewLine, open_row};
use super::panel::{
    ViewsState, meta_under, view_bars, view_head, view_row, view_rule_head, view_tiles,
};
use super::*;

/// What a block is drawn with (blockCtx): the view's accent, for what has
/// no tone, and the panel's state, for what's folded.
pub struct BlockCtx<'a> {
    pub accent: Color,
    pub st: &'a ViewsState,
    /// The block's, for its folds: view/b<index>.
    pub key: String,
    /// The view's name.
    pub view: String,
    /// Its rows' keys start so (block_key).
    pub prefix: String,
}

/// A block w wide: its title as a group's rule, a click folding it, then
/// its body in its style (viewBlock).
pub fn view_block(v: &View, bi: usize, w: usize, st: &ViewsState, accent: Color) -> Vec<ViewLine> {
    let b = &v.blocks[bi];
    let key = format!("{}/b{bi}", v.name);
    let mut out = Vec::new();
    let folded = st.fold.get(&key).copied().unwrap_or(b.folded);
    if !b.title.is_empty() {
        let n = if matches!(
            b.kind.as_str(),
            "code" | "callout" | "text" | "hero" | "chart"
        ) {
            -1 // no count
        } else {
            b.items.len() as i64
        };
        out.push(ViewLine::with(
            view_rule_head(&b.title, &b.tone, n, folded, w),
            ViewAct {
                fold: key.clone(),
                folded,
                ..Default::default()
            },
        ));
        if folded {
            return out;
        }
    }
    let cx = BlockCtx {
        accent,
        st,
        key,
        view: v.name.clone(),
        prefix: block_key(bi),
    };
    match b.kind.as_str() {
        "list" => {
            let rows: Vec<Row> = b.items.iter().map(item_row).collect();
            let under = meta_under(&rows, w);
            for (it, r) in b.items.iter().zip(&rows) {
                out.extend(cx.row_lines(open_row(&cx.prefix, it), w, view_row(r, w, under)));
            }
        }
        "bars" => {
            let rows: Vec<Row> = b.items.iter().map(item_row).collect();
            let bars = view_bars(&rows, w, accent);
            for (i, r) in rows.iter().enumerate() {
                if r.value.is_some() {
                    out.extend(cx.row_lines(open_row(&cx.prefix, &b.items[i]), w, vec![bars(r)]));
                }
            }
        }
        "stats" => out.extend(view_tiles(&b.stats, w).into_iter().map(ViewLine::plain)),
        "progress" => {
            let v = View {
                progress: b.progress.clone(),
                ..Default::default()
            };
            out.extend(view_head(&v, w, accent).into_iter().map(ViewLine::plain));
        }
        "steps" => out.extend(flow::steps(b, w, &cx)),
        "timeline" => out.extend(flow::timeline(b, w, &cx)),
        "board" => out.extend(flow::board(b, w, &cx)),
        "tree" => out.extend(flow::tree(b, w, &cx)),
        "table" => out.extend(tabular::table(b, w, &cx)),
        "kv" => out.extend(tabular::kv(b, w, &cx)),
        "checklist" => out.extend(tabular::checklist(b, w, &cx)),
        "log" => out.extend(tabular::log(b, w, &cx)),
        "compare" => out.extend(tabular::compare(b, w, &cx)),
        "chart" => out.extend(visual::chart(b, w, &cx)),
        "heatmap" => out.extend(visual::heatmap(b, w, &cx)),
        "hero" => out.extend(visual::hero(b, w, &cx)),
        "callout" => out.extend(visual::callout(b, w, &cx)),
        "code" => out.extend(visual::code(b, w)),
        "text" => out.extend(visual::text(b, w)),
        _ => {}
    }
    out
}

/// An item as a group's row (itemRow).
pub fn item_row(it: &Item) -> Row {
    Row {
        text: it.text.clone(),
        meta: it.meta.clone(),
        tone: it.tone.clone(),
        link: it.link.clone(),
        progress: it.progress,
        value: it.value,
        ..Default::default()
    }
}
