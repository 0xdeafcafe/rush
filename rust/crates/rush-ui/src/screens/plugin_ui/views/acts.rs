//! A view's rows open on a click: under the row its words in full, a
//! table row's cells each by its column, its detail, and its buttons: the
//! agent's, then open ↗ for its link, tick for a checklist's item, and
//! copy. After Go's internal/ui/viewacts.go (viewBtn :19, rowOpen :25,
//! openRow :36, rowLines :43, opened :68, btnLines :172, viewChip :213)
//! and agentviews.go:264-303 (viewTab, viewAct, viewLine).

use rush_core::text::{Color, Line, Span, Style};

use super::blocks::BlockCtx;
use super::*;

/// A button on a line of the panel: the columns it takes, and what it
/// does.
#[derive(Clone, Debug, PartialEq)]
pub struct Btn {
    pub from: usize,
    pub to: usize,
    pub a: Action,
}

/// A line of buttons drawn again with the pointer at a column, its
/// button lit: where they start, their accent, and whether the line runs
/// down an open row's rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Lit {
    pub accent: Color,
    pub ruled: bool,
}

/// What a click on a row of the panel does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewAct {
    pub link: String,
    /// view/index
    pub fold: String,
    pub folded: bool,
    /// A table's header: a click sorts by the column under it; cols where
    /// each starts in the panel.
    pub sort: String,
    pub cols: Vec<usize>,
    /// A past view's file: a click brings it back as name.
    pub back: String,
    pub name: String,
    /// A row a click opens or shuts, by view|row, opened if it is; row is
    /// its key in its view, for what its buttons change.
    pub open: String,
    pub opened: bool,
    pub row: String,
    /// A checklist item's: a click on its mark ticks it.
    pub tick: bool,
    pub done: bool,
    pub btns: Vec<Btn>,
    pub lit: Option<Lit>,
}

impl ViewAct {
    /// Whether a click on it does anything.
    pub fn ok(&self) -> bool {
        !self.link.is_empty()
            || !self.fold.is_empty()
            || !self.sort.is_empty()
            || !self.back.is_empty()
            || !self.open.is_empty()
            || !self.btns.is_empty()
    }

    /// The line drawn with the pointer at col, its button lit.
    pub fn lit_line(&self, col: i64) -> Option<Line> {
        let l = self.lit.as_ref()?;
        let mut out = if l.ruled {
            ln([faint("│ ")])
        } else {
            Line::new()
        };
        out.spans.extend(draw_btns(&self.btns, l.accent, col).spans);
        Some(out)
    }
}

/// A row of a view's body and what a click on it does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewLine {
    pub s: Line,
    pub act: ViewAct,
}

impl ViewLine {
    pub fn plain(s: Line) -> ViewLine {
        ViewLine {
            s,
            act: ViewAct::default(),
        }
    }
    pub fn with(s: Line, act: ViewAct) -> ViewLine {
        ViewLine { s, act }
    }
}

/// A row a click opens, and what shows under it then.
#[derive(Clone, Debug, Default)]
pub struct RowOpen {
    pub key: String,
    pub text: String,
    pub meta: String,
    pub detail: String,
    pub link: String,
    pub cols: Vec<String>,
    pub cells: Vec<String>,
    pub actions: Vec<Action>,
    pub tick: Option<bool>,
    /// Its words didn't fit on its line.
    pub cut: bool,
}

/// The row of an item under prefix (openRow).
pub fn open_row(prefix: &str, it: &Item) -> RowOpen {
    RowOpen {
        key: item_key(prefix, &it.id, &it.text, &it.cells),
        text: it.text.clone(),
        meta: it.meta.clone(),
        detail: it.detail.clone(),
        link: it.link.clone(),
        cells: it.cells.clone(),
        actions: it.actions.clone(),
        ..Default::default()
    }
}

impl BlockCtx<'_> {
    /// A row's drawn lines with the click that opens it, and when it's
    /// open, what shows under it (rowLines).
    pub fn row_lines(&self, r: RowOpen, w: usize, lines: Vec<Line>) -> Vec<ViewLine> {
        if r.key.is_empty() {
            return lines
                .into_iter()
                .map(|s| {
                    ViewLine::with(
                        s,
                        ViewAct {
                            link: r.link.clone(),
                            ..Default::default()
                        },
                    )
                })
                .collect();
        }
        let key = format!("{}|{}", self.view, r.key);
        let open = self.st.open.get(&key).copied().unwrap_or(false);
        let mut act = ViewAct {
            open: key,
            opened: open,
            row: r.key.clone(),
            ..Default::default()
        };
        if let Some(t) = r.tick {
            act.tick = true;
            act.done = t;
        }
        let mut out: Vec<ViewLine> = lines
            .into_iter()
            .map(|s| ViewLine::with(s, act.clone()))
            .collect();
        if open {
            out.extend(self.opened(&r, w));
        }
        out
    }

    /// What shows under an open row, along a quiet rule (opened).
    fn opened(&self, r: &RowOpen, w: usize) -> Vec<ViewLine> {
        const LEAD: usize = 2; // "│ "
        let inn = 8.max(w.saturating_sub(LEAD));
        let mut out = Vec::new();
        let add = |out: &mut Vec<ViewLine>, s: Line| {
            let mut l = ln([faint("│ ")]);
            l.spans.extend(s.spans);
            out.push(ViewLine::plain(l));
        };
        if !r.cells.is_empty() {
            let mut kw = 0;
            for c in 0..r.cells.len() {
                if let Some(n) = r.cols.get(c) {
                    kw = kw.max(cw(n));
                }
            }
            kw = kw.min(inn / 3);
            for (c, cell) in r.cells.iter().enumerate() {
                if cell.is_empty() {
                    continue;
                }
                let name = r.cols.get(c).map(String::as_str).unwrap_or("");
                for (j, l) in wrap(cell, 4.max(inn.saturating_sub(kw + 2)))
                    .into_iter()
                    .enumerate()
                {
                    let k = if j == 0 {
                        fit_str(&trunc(name, kw), kw)
                    } else {
                        blanks(kw)
                    };
                    if kw == 0 {
                        add(&mut out, ln([paint(TEXT, l)]));
                    } else {
                        add(&mut out, ln([paint(SUB, k), plain("  "), paint(TEXT, l)]));
                    }
                }
            }
        } else if r.cut || cw(&r.text) > inn {
            for l in wrap(&r.text, inn) {
                add(&mut out, ln([paint(TEXT, l)]));
            }
        }
        if r.cells.is_empty() && !r.meta.is_empty() && (r.cut || cw(&r.meta) > inn / 3) {
            for l in wrap(&r.meta, inn) {
                add(&mut out, ln([dim(l)]));
            }
        }
        if !r.detail.is_empty() {
            for para in r.detail.split('\n') {
                if para.is_empty() {
                    add(&mut out, Line::new());
                    continue;
                }
                for l in wrap(para, inn) {
                    add(&mut out, ln([paint(TEXT, l)]));
                }
            }
        }
        let mut btns = r.actions.clone();
        if let Some(t) = r.tick {
            let label = if t { "untick" } else { "✓ tick off" };
            btns.push(Action {
                label: label.into(),
                set: Some(Change {
                    done: Some(!t),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        if !r.link.is_empty() && !r.actions.iter().any(|a| a.open == r.link) {
            btns.push(Action {
                label: "open ↗".into(),
                open: r.link.clone(),
                ..Default::default()
            });
        }
        let words = if r.cells.is_empty() {
            r.text.clone()
        } else {
            r.cells
                .iter()
                .filter(|c| !c.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ")
        };
        if !words.is_empty() {
            btns.push(Action {
                label: "copy".into(),
                copy: words,
                ..Default::default()
            });
        }
        for mut l in btn_lines(&btns, inn, 2 + LEAD, self.accent, &r.key) {
            let mut s = ln([faint("│ ")]);
            s.spans.extend(l.s.spans);
            l.s = s;
            if let Some(lit) = l.act.lit.as_mut() {
                lit.ruled = true;
            }
            out.push(l);
        }
        out
    }
}

/// Buttons side by side, onto another line when they don't fit w, the
/// first starting at column at of the panel (btnLines). key is the row
/// they change, "" for the view's own.
pub fn btn_lines(r#as: &[Action], w: usize, at: usize, accent: Color, key: &str) -> Vec<ViewLine> {
    let mut out = Vec::new();
    let mut row: Vec<Btn> = Vec::new();
    let mut used = 0;
    let flush = |row: &mut Vec<Btn>, used: &mut usize, out: &mut Vec<ViewLine>| {
        if row.is_empty() {
            return;
        }
        let bs = std::mem::take(row);
        out.push(ViewLine::with(
            draw_btns(&bs, accent, -1),
            ViewAct {
                btns: bs,
                row: key.to_string(),
                lit: Some(Lit {
                    accent,
                    ruled: false,
                }),
                ..Default::default()
            },
        ));
        *used = 0;
    };
    for a in r#as {
        let bw = cw(&a.label) + 2;
        if used > 0 && used + 1 + bw > w {
            flush(&mut row, &mut used, &mut out);
        }
        if used > 0 {
            used += 1;
        }
        row.push(Btn {
            from: at + used,
            to: at + used + bw,
            a: a.clone(),
        });
        used += bw;
    }
    flush(&mut row, &mut used, &mut out);
    out
}

/// Buttons drawn where they go, the one at col lit. Their columns count
/// from the panel's edge; the line starts where the first does.
fn draw_btns(bs: &[Btn], accent: Color, col: i64) -> Line {
    let mut l = Line::new();
    let mut x = bs.first().map(|b| b.from).unwrap_or(0);
    for (i, b) in bs.iter().enumerate() {
        if i > 0 {
            l.spans.push(plain(" "));
            x += 1;
        }
        l.spans.push(plain(blanks(b.from.saturating_sub(x))));
        l.spans.push(view_chip(
            &b.a,
            accent,
            col >= b.from as i64 && col < b.to as i64,
        ));
        x = b.to;
    }
    line(l.spans)
}

/// A button: its label on a raised ground, brighter under the pointer
/// (viewChip).
pub fn view_chip(a: &Action, accent: Color, lit: bool) -> Span {
    let bg = if lit { BTN_LIT_BG } else { BTN_BG };
    Span::new(
        format!(" {} ", a.label),
        Style::fg(accent_or(&a.tone, accent)).bold().on(bg),
    )
}
