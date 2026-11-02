//! A session's own views beside its conversation: the panel, its tabs,
//! its edges, the rows of the view shown, and what a click, the wheel and
//! the pointer do on it. After Go's internal/ui/agentviews.go (viewsWidth
//! :117, viewsState :255, viewsPanel :358 with the box as tall as what's
//! in it, viewsTop :442, pastTop :479, pastBody :488, viewsBottom :508,
//! viewBody :543 with rush's rows over it and greyed or folded when quiet,
//! viewHead :653, viewTiles :680, viewRuleHead :715, viewHeading :726,
//! viewBars :745, viewRow :785, metaUnder :813, metaFits :821, metaLines
//! :831, clickViews :862, wheelViews :975, hoverViews :989, paneViewsSide
//! :1030, sideViews :1041) and rushmode.go:693 (viewsFolded).

use std::collections::HashMap;

use rush_core::text::{Color, Line};

use super::acts::{Btn, RowOpen, ViewAct, ViewLine, btn_lines};
use super::blocks::{BlockCtx, view_block};
use super::*;

/// How wide the panel is beside a conversation w wide, 0 when the
/// conversation would be left too narrow (viewsWidth). Where rush's own
/// column stands beside the conversation (side_width), the panel is
/// exactly as wide, so the text doesn't move when one gives way to the
/// other; narrower, where there's no column, it is 3/8 of the room.
pub fn views_width(w: usize) -> usize {
    views_width_with(w, 0)
}

/// views_width, the column at the width you set (pref; 0 is rush's own).
pub fn views_width_with(w: usize, pref: i64) -> usize {
    let col = crate::sidecol::side_width_with(w, pref);
    if col > 0 {
        return col;
    }
    let side = 64.min(42.max(w * 3 / 8));
    if w < side + 72 {
        return 0;
    }
    side
}

/// How wide the panel is folded: a slim box with the mark that opens it.
pub const VIEWS_STRIP: usize = 3;

/// A tab along the panel's top edge: its columns and view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewTab {
    pub from: usize,
    pub to: usize,
    pub name: String,
}

/// How you've left a session's views, and where they were last drawn,
/// for the mouse (viewsState).
#[derive(Clone, Debug, Default)]
pub struct ViewsState {
    /// Folded to a strip.
    pub shut: bool,
    /// Showing the views put away, to bring one back.
    pub past: bool,
    /// The view shown, by name: "" or one gone is the newest.
    pub tab: String,
    pub scroll: usize,
    /// Groups you folded or opened, by view/index.
    pub fold: HashMap<String, bool>,
    /// Tables you sorted: by column +1, less for down.
    pub sorts: HashMap<String, i64>,
    /// Rows you opened, by view|row.
    pub open: HashMap<String, bool>,
    /// The body row under the pointer, +1; 0 for none.
    pub hover: usize,
    /// And its column in the panel, for a button.
    pub hover_x: i64,
    /// The pointer is on something a click does.
    pub hot: bool,
    // As last drawn.
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub tabs: Vec<ViewTab>,
    pub acts: Vec<ViewAct>,
    pub most: usize,
    pub shown: String,
    /// No view shows now: the strip opens the past ones.
    pub empty: bool,
    /// ⟲ along the top edge opens them.
    pub has_past: bool,
    /// Rows rush draws at the top of the view shown, over what the agent
    /// drew: the session's subagents (panelsubs.go).
    pub live: Vec<ViewLine>,
    /// Why the view shown is greyed, "" for not (viewquiet.go), and what
    /// it was last worked out from.
    pub quiet: String,
    pub quiet_key: super::quiet::QuietKey,
    pub quiet_memo: String,
    /// The panel has the keys (→ from the box): ↑↓ move the cursor over
    /// what a click does, enter does it, ← or esc gives them back
    /// (Rust-only, as the column beside the conversation takes them).
    pub focus: bool,
    /// The first body row of the thing the cursor is on.
    pub cursor: usize,
    /// The button picked, on a row of buttons under the cursor.
    pub btn: usize,
    /// What a click does in the body as last drawn: each thing's first
    /// and last row (a row drawn on two lines is one thing).
    pub items: Vec<(usize, usize)>,
}

/// The view the tab names, else the newest (shownView).
pub(crate) fn shown_view<'a>(vs: &'a [View], tab: &str) -> &'a View {
    vs.iter().find(|v| v.name == tab).unwrap_or(&vs[0])
}

impl ViewsState {
    /// Whether a view's group is folded: as you left it, else as the
    /// agent said (isFolded).
    fn is_folded(&self, v: &View, gi: usize) -> (String, bool) {
        let key = format!("{}/{gi}", v.name);
        let f = self.fold.get(&key).copied().unwrap_or(v.groups[gi].folded);
        (key, f)
    }
}

/// A session's views in a box w wide and h tall: the views as tabs along
/// its top edge with the marks that fold and dismiss it, the one shown
/// inside, scrolled as far as st says, and along the bottom edge who made
/// it and how much more there is. Folded, a slim box with the mark that
/// opens it (viewsPanel). now and each view's times are Unix ms.
pub fn views_panel(
    vs: &[View],
    past: &[View],
    w: usize,
    h: usize,
    now: i64,
    st: &mut ViewsState,
) -> Vec<Line> {
    st.w = w;
    st.h = h;
    st.tabs.clear();
    st.acts.clear();
    st.empty = vs.is_empty();
    st.has_past = !past.is_empty();
    if h == 0 || vs.is_empty() && past.is_empty() {
        st.focus = false;
        return Vec::new();
    }
    if st.shut || st.empty && !st.past {
        st.focus = false; // a strip has nothing to move through
        let mark = if st.empty {
            paint(SUB, "⟲") // only past ones: it opens them
        } else {
            paint(ORANGE, "✦")
        };
        return (0..h)
            .map(|y| match y {
                0 => ln([faint("╭─╮")]),
                y if y == h - 1 => ln([faint("╰─╯")]),
                1 => ln([faint("│"), paint(SUB, "‹"), faint("│")]),
                2 => ln([faint("│"), mark.clone(), faint("│")]),
                _ => ln([faint("│ │")]),
            })
            .collect();
    }
    let inner = w.saturating_sub(4); // its edges, and a space inside each
    let (body, top) = if st.past {
        (past_body(past, inner, now), past_top(w))
    } else {
        let v = shown_view(vs, &st.tab);
        st.shown = v.name.clone();
        let top = views_top(vs, v, w, st);
        (view_body(v, inner, st), top)
    };
    // The box is as tall as what's in it, up to h: ground under it, not
    // an empty box.
    let full = h;
    let h = h.min(body.len() + 2);
    st.h = h;
    let bh = h.saturating_sub(2);
    st.most = body.len().saturating_sub(bh);
    // Set again by the agent, the view keeps where you scrolled it; one
    // that shrank keeps as much as it still has, its end at the bottom.
    st.scroll = st.scroll.min(st.most);
    st.items = items_of(&body);
    if st.focus {
        // On a thing a click does still; the wheel may scroll it out of
        // sight, and the next key brings it back.
        st.snap_cursor();
    }
    let cur = st.cursor_rows();
    let mut out = Vec::with_capacity(h);
    out.push(top);
    for i in 0..bh {
        let l = body.get(st.scroll + i).cloned().unwrap_or_default();
        let mid = if cur.contains(&(st.scroll + i)) {
            // The cursor: a button picked lit, else the row on the
            // selection's ground, as a picked row is.
            let btn = l.act.btns.get(st.btn).map(|b| b.from as i64);
            match btn.and_then(|x| l.act.lit_line(x)) {
                Some(lit) => cat([ln([plain(" ")]), fit(lit, inner), ln([plain(" ")])]),
                None => on_ground(
                    cat([ln([plain(" ")]), fit(l.s, inner), ln([plain(" ")])]),
                    inner + 2,
                    SEL_BG,
                ),
            }
        } else if st.hover != i + 1 || !l.act.ok() {
            cat([ln([plain(" ")]), fit(l.s, inner), ln([plain(" ")])])
        } else if let Some(lit) = l.act.lit_line(st.hover_x) {
            cat([ln([plain(" ")]), fit(lit, inner), ln([plain(" ")])])
        } else {
            on_ground(
                cat([ln([plain(" ")]), fit(l.s, inner), ln([plain(" ")])]),
                inner + 2,
                HOVER_BG,
            )
        };
        out.push(cat([ln([faint("│")]), mid, ln([faint("│")])]));
        st.acts.push(l.act);
    }
    if h >= 2 {
        let bottom = if st.past {
            views_bottom(w, "", "", false, 0, 0)
        } else {
            let v = shown_view(vs, &st.tab);
            let below = body.len() as i64 - st.scroll as i64 - bh as i64;
            let mut when = ago(now - v.updated.to_ms());
            if !st.quiet.is_empty() {
                // Stale: what's wrong, and for how long, in the warning tone.
                when = format!("{} · {}", st.quiet, when.trim_end_matches(" ago"));
            }
            views_bottom(
                w,
                &v.by,
                &when,
                !st.quiet.is_empty(),
                st.scroll,
                below.max(0) as usize,
            )
        };
        out.push(bottom);
    }
    while out.len() < full {
        out.push(ln([plain(blanks(w))]));
    }
    out
}

/// The things a click does in body, each its first and last row: the
/// lines in a run with the same click are one.
fn items_of(body: &[ViewLine]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (i, l) in body.iter().enumerate() {
        if !l.act.ok() {
            continue;
        }
        match out.last_mut() {
            Some((_, to)) if *to + 1 == i && body[*to].act == l.act && l.act.btns.is_empty() => {
                *to = i;
            }
            _ => out.push((i, i)),
        }
    }
    out
}

/// The panel's top edge: rush's mark, the views' titles as tabs, the one
/// shown bright, and at its end › to fold it and × to dismiss it
/// (viewsTop).
fn views_top(vs: &[View], v: &View, w: usize, st: &mut ViewsState) -> Line {
    let mut right = ln([
        plain(" "),
        paint(SUB, "›"),
        plain(" "),
        paint(SUB, "×"),
        plain(" "),
    ]);
    if st.has_past {
        right = cat([ln([plain(" "), paint(SUB, "⟲")]), right]);
    }
    let rw = right.width();
    let room = w as isize - 9 - rw as isize; // "╭─ ✦ ", a space and a rule, the marks, "─╮"
    let n = vs.len() as isize;
    let per = if n == 1 {
        room
    } else {
        4.max((room - 3 * (n - 1)) / n)
    };
    let mut b = Line::new();
    let mut col: isize = 5;
    for (i, x) in vs.iter().enumerate() {
        if i > 0 {
            if col + 3 + 4 > 5 + room {
                break;
            }
            b.spans.push(faint(" · "));
            col += 3;
        }
        let name = trunc(&x.title, per.min(5 + room - col).max(0) as usize);
        let style = if std::ptr::eq(x, v) {
            bolded(BRIGHT, name.clone())
        } else {
            paint(SUB, name.clone())
        };
        b.spans.push(style);
        let to = col + cw(&name) as isize;
        st.tabs.push(ViewTab {
            from: col.max(0) as usize,
            to: to.max(0) as usize,
            name: x.name.clone(),
        });
        col = to;
    }
    let left = cat([
        ln([faint("╭─ "), paint(view_accent(v), "✦"), plain(" ")]),
        line(b.spans),
        ln([plain(" ")]),
    ]);
    let fill = (w as isize - (col + 1) - rw as isize - 2).max(0) as usize;
    cat([
        left,
        ln([faint("─".repeat(fill))]),
        right,
        ln([faint("─╮")]),
    ])
}

/// The panel's top edge over the past views: a click on it goes back
/// (pastTop).
fn past_top(w: usize) -> Line {
    let left = ln([
        faint("╭─ "),
        paint(SUB, "⟲"),
        plain(" "),
        bolded(BRIGHT, "Past views"),
        plain(" "),
    ]);
    let right = ln([plain(" "), paint(SUB, "‹ back"), plain(" ")]);
    let fill = w.saturating_sub(left.width() + right.width() + 2);
    cat([
        left,
        ln([faint("─".repeat(fill))]),
        right,
        ln([faint("─╮")]),
    ])
}

/// The views put away, the one gone last first, each its title and when
/// it went, its summary under it: a click brings it back (pastBody).
fn past_body(past: &[View], w: usize, now: i64) -> Vec<ViewLine> {
    let mut out = vec![ViewLine::default()];
    if past.is_empty() {
        out.push(ViewLine::plain(ln([dim("Nothing put away yet.")])));
        return out;
    }
    for v in past {
        let act = ViewAct {
            back: v.file.clone(),
            name: v.name.clone(),
            ..Default::default()
        };
        let when = ln([dim(ago(now - v.gone.to_ms()))]);
        let left = ln([
            paint(view_accent(v), "◆"),
            plain(" "),
            paint(TEXT, trunc(&v.title, w.saturating_sub(12))),
        ]);
        out.push(ViewLine::with(spread(left, when, w), act.clone()));
        if !v.summary.is_empty() {
            out.push(ViewLine::with(
                ln([plain("  "), dim(trunc(&v.summary, w.saturating_sub(2)))]),
                act,
            ));
        }
    }
    out.push(ViewLine::default());
    out.push(ViewLine::plain(ln([faint(
        "a click brings one back, kept until you close it",
    )])));
    out
}

/// The panel's bottom edge: who made it and when it last changed it, and
/// how many rows are scrolled past above and below (viewsBottom).
fn views_bottom(w: usize, by: &str, when: &str, stale: bool, above: usize, below: usize) -> Line {
    let mut r = Vec::new();
    if above > 0 {
        r.push(format!("↑ {above}"));
    }
    if below > 0 {
        r.push(format!("↓ {below} more"));
    }
    let right = if r.is_empty() {
        Line::new()
    } else {
        ln([plain(" "), paint(SUB, r.join(" · ")), plain(" ")])
    };
    let mut left = ln([faint("╰─ ")]);
    let who = if by.is_empty() {
        "the agent".to_string()
    } else {
        format!("plugin {by}")
    };
    let labels: Vec<String> = if when.is_empty() {
        vec![String::new(), String::new()]
    } else {
        vec![
            format!("made by {who} · {when}"),
            format!("by {who} · {when}"),
            when.to_string(),
            String::new(),
        ]
    };
    for label in labels {
        if (cw(&label) as isize) < w as isize - 3 - right.width() as isize - 3 {
            if !label.is_empty() {
                // The stale part (after the first dot) in the warning tone.
                match label.split_once(" · ").filter(|_| stale) {
                    Some((who, rest))
                        if label.starts_with("made by") || label.starts_with("by ") =>
                    {
                        left.spans.push(dim(format!("{who} · ")));
                        left.spans.push(paint(YELLOW, rest.to_string()));
                    }
                    _ if stale => left.spans.push(paint(YELLOW, label)),
                    _ => left.spans.push(dim(label)),
                }
                left.spans.push(plain(" "));
            }
            break;
        }
    }
    let fill = w.saturating_sub(left.width() + right.width() + 2);
    cat([
        left,
        ln([faint("─".repeat(fill))]),
        right,
        ln([faint("─╯")]),
    ])
}

/// A view's rows w wide: rush's own rows (st.live) over it, its summary,
/// a bar of how far along, its numbers as tiles, its groups of rows under
/// a heading, folded or not, its blocks, its note and its buttons, a
/// blank row between parts. Greyed (st.quiet) when the agent has stopped
/// keeping it, or folded to its heading under rush's rows (viewBody).
pub fn view_body(v: &View, w: usize, st: &ViewsState) -> Vec<ViewLine> {
    let accent = view_accent(v);
    let mut out = vec![ViewLine::default()];
    // A blank row between parts: one, never two.
    fn room(out: &mut Vec<ViewLine>) {
        if out.last().is_some_and(|l| !l.s.plain().is_empty()) {
            out.push(ViewLine::default());
        }
    }
    if !st.live.is_empty() {
        out.extend(st.live.iter().cloned());
        room(&mut out);
    }
    let ours = out.len(); // what the agent drew starts here
    let mut fade = !st.quiet.is_empty();
    if fade && !st.live.is_empty() {
        // Greyed under rush's bright rows it looks broken: folded to its
        // heading, it reads as put away instead. A click opens it, bright.
        let key = format!("{}/§quiet", v.name);
        let folded = st.fold.get(&key).copied().unwrap_or(true);
        let title = if v.title.is_empty() {
            &v.name
        } else {
            &v.title
        };
        out.push(ViewLine::with(
            view_heading(title, &st.quiet, "", folded, w),
            ViewAct {
                fold: key,
                folded,
                ..Default::default()
            },
        ));
        if folded {
            return out;
        }
        out.push(ViewLine::default());
        fade = false; // opened: bright as it was
    }
    let hd = view_head(v, w, accent);
    if !hd.is_empty() {
        out.extend(hd.into_iter().map(ViewLine::plain));
        room(&mut out);
    }
    let t = view_tiles(&v.stats, w);
    if !t.is_empty() {
        out.extend(t.into_iter().map(ViewLine::plain));
        room(&mut out);
    }
    for (gi, g) in v.groups.iter().enumerate() {
        room(&mut out);
        let (key, mut folded) = st.is_folded(v, gi);
        if !g.title.is_empty() {
            out.push(ViewLine::with(
                view_rule_head(&g.title, &g.tone, g.rows.len() as i64, folded, w),
                ViewAct {
                    fold: key,
                    folded,
                    ..Default::default()
                },
            ));
        } else {
            folded = false; // nothing to open it by
        }
        if folded {
            continue;
        }
        if !g.text.is_empty() {
            out.extend(
                wrap(&g.text, w)
                    .into_iter()
                    .map(|l| ViewLine::plain(ln([dim(l)]))),
            );
        }
        let bars = view_bars(&g.rows, w, accent);
        let under = meta_under(&g.rows, w);
        let cx = BlockCtx {
            accent,
            st,
            key: String::new(),
            view: v.name.clone(),
            prefix: String::new(),
        };
        for r in &g.rows {
            let o = RowOpen {
                key: item_key(&group_key(gi), &r.id, &r.text, &[]),
                text: r.text.clone(),
                meta: r.meta.clone(),
                detail: r.detail.clone(),
                link: r.link.clone(),
                actions: r.actions.clone(),
                ..Default::default()
            };
            if r.value.is_some() {
                out.extend(cx.row_lines(o, w, vec![bars(r)]));
                continue;
            }
            out.extend(cx.row_lines(o, w, view_row(r, w, under)));
        }
    }
    for bi in 0..v.blocks.len() {
        room(&mut out);
        out.extend(view_block(v, bi, w, st, accent));
    }
    if !v.note.is_empty() {
        room(&mut out);
        for l in wrap(&v.note, w.saturating_sub(2)) {
            out.push(ViewLine::plain(ln([faint("│ "), dim(l)])));
        }
    }
    if !v.actions.is_empty() {
        // The view's own buttons: what it changes is no row of it.
        let r#as: Vec<Action> = v
            .actions
            .iter()
            .map(|a| Action {
                set: None,
                ..a.clone()
            })
            .collect();
        room(&mut out);
        out.extend(btn_lines(&r#as, w, 2, accent, ""));
    }
    room(&mut out);
    if fade {
        // Greyed: the agent has stopped keeping it.
        for l in &mut out[ours..] {
            // Nothing in it looks live: a busy mark is a quiet dot, the
            // step under way an empty one, everything in the faint grey.
            l.s = ln([faint(l.s.plain().replace('◐', "·").replace('◉', "○"))]);
            l.act.lit = None;
        }
    }
    out
}

/// The view's summary when it has no numbers or bar to say it, then the
/// bar of how far along it is, w wide (viewHead).
pub fn view_head(v: &View, w: usize, accent: Color) -> Vec<Line> {
    let mut out = Vec::new();
    if !v.summary.is_empty() && v.stats.is_empty() && v.progress.is_none() {
        for (i, l) in wrap(&v.summary, w.saturating_sub(2))
            .into_iter()
            .enumerate()
        {
            let lead = if i == 0 {
                paint(accent_or(&v.tone, accent), "◆")
            } else {
                plain(" ")
            };
            out.push(ln([lead, plain(" "), paint(TEXT, l)]));
        }
    }
    let Some(p) = &v.progress else {
        return out;
    };
    let frac = (p.done / p.total).clamp(0.0, 1.0);
    let frac = if frac.is_nan() { 0.0 } else { frac };
    let mut words = format!("{} of {}", num(p.done), num(p.total));
    if !p.label.is_empty() {
        words += &format!(" {}", p.label);
    }
    let pct = format!("{:>3}%", (frac * 100.0 + 0.5) as i64);
    let mut bar = progress_bar(
        frac,
        w.saturating_sub(pct.len() + 1),
        accent_or(&p.tone, accent),
    );
    bar.spans.push(plain(" "));
    bar.spans.push(bolded(BRIGHT, pct));
    out.push(bar);
    out.push(ln([dim(words)]));
    out
}

/// A view's numbers side by side, w wide, with a quiet rule between them:
/// each in its tone over its label and its trend if any has one, then the
/// bar of their shares (viewTiles).
pub fn view_tiles(ss: &[Stat], w: usize) -> Vec<Line> {
    let n = ss.len();
    if n == 0 {
        return Vec::new();
    }
    let cell = (w.saturating_sub(3 * (n - 1))) / n; // " │ " between tiles
    let mut rows: [Line; 3] = Default::default();
    let mut trends = false;
    for (i, s) in ss.iter().enumerate() {
        let cwid = if i == n - 1 {
            w.saturating_sub((cell + 3) * (n - 1)) // the last takes what's left
        } else {
            cell
        };
        if i > 0 {
            for r in rows.iter_mut() {
                r.spans.push(faint(" │ "));
            }
        }
        rows[0].spans.extend(
            fit(
                ln([bolded(view_tone_colour(&s.tone), s.value.clone())]),
                cwid,
            )
            .spans,
        );
        rows[1]
            .spans
            .extend(fit(ln([paint(SUB, s.label.clone())]), cwid).spans);
        rows[2]
            .spans
            .extend(fit(spark(&s.trend, cwid, tone_edge(&s.tone)), cwid).spans);
        trends = trends || !s.trend.is_empty();
    }
    let [r0, r1, r2] = rows;
    let mut out = vec![line(r0.spans), line(r1.spans)];
    if trends {
        out.push(line(r2.spans));
    }
    let bar = share_bar(ss, w);
    if !bar.spans.is_empty() {
        out.push(bar);
    }
    out
}

/// A group's or block's heading with its title and count (n, none below
/// 0), ▾ open or ▸ folded, w wide (viewRuleHead).
pub fn view_rule_head(title: &str, tone: &str, n: i64, folded: bool, w: usize) -> Line {
    let count = if n >= 0 { n.to_string() } else { String::new() };
    view_heading(title, &count, tone, folded, w)
}

/// A part's heading, w wide: ▾ open or ▸ folded, its title bright (in its
/// tone if it has one), and a few quiet words on it (a count). No rule:
/// the room above it sets it apart (viewHeading).
pub fn view_heading(title: &str, count: &str, tone: &str, folded: bool, w: usize) -> Line {
    let col = if tone.is_empty() {
        BRIGHT
    } else {
        tone_edge(tone)
    };
    let caret = if folded { "▸ " } else { "▾ " };
    let count = if count.is_empty() {
        String::new()
    } else {
        format!("  {count}")
    };
    let title = trunc(title, 4.max(w as isize - 2 - cw(&count) as isize) as usize);
    ln([paint(SUB, caret), bolded(col, title), dim(count)])
}

/// Draws a group's rows that have a value as a chart, w wide: each its
/// mark and text in a column, a bar as long as its value against the
/// largest, and the number, or its meta, at the end (viewBars).
pub fn view_bars(rows: &[Row], w: usize, accent: Color) -> impl Fn(&Row) -> Line + use<> {
    let words = |r: &Row| {
        if r.meta.is_empty() {
            num(r.value.unwrap_or(0.0))
        } else {
            r.meta.clone()
        }
    };
    let (mut most, mut text_w, mut num_w) = (0.0f64, 0, 0);
    for r in rows {
        if let Some(v) = r.value {
            most = most.max(v);
            text_w = text_w.max(cw(&r.text));
            num_w = num_w.max(cw(&words(r)));
        }
    }
    let num_w = num_w.min(w / 4);
    let text_w = text_w.min(w * 2 / 5);
    let bar_w = 4.max(w as isize - 2 - text_w as isize - 1 - 1 - num_w as isize) as usize;
    move |r: &Row| {
        let (mark, col) = tone_mark(&r.tone);
        let v = r.value.unwrap_or(0.0);
        let frac = if most > 0.0 { v / most } else { 0.0 };
        let bar = accent_or(&r.tone, accent);
        let mut n = ((frac * bar_w as f64 + 0.5) as isize).clamp(0, bar_w as isize) as usize;
        if v > 0.0 && n == 0 {
            n = 1; // above nought always shows
        }
        let words = if r.meta.is_empty() {
            num(v)
        } else {
            r.meta.clone()
        };
        ln([
            paint(col, mark),
            plain(" "),
            paint(TEXT, fit_str(&trunc(&r.text, text_w), text_w)),
            plain(" "),
            paint(bar, "━".repeat(n)),
            faint("─".repeat(bar_w - n)),
            plain(" "),
            paint(SUB, right_str(&trunc(&words, num_w), num_w)),
        ])
    }
}

/// s right-aligned in at least w cells (fmt's %*s).
fn right_str(s: &str, w: usize) -> String {
    format!("{}{s}", blanks(w.saturating_sub(cw(s))))
}

/// A row w wide: its mark, its text and link, and at its end its bar or
/// the word on it. The word goes on the lines under the text, lined up
/// with it, when under says so (meta_under) (viewRow).
pub fn view_row(r: &Row, w: usize, under: bool) -> Vec<Line> {
    let (mark, col) = tone_mark(&r.tone);
    let (mut meta, mut below) = (Line::new(), r.meta.as_str());
    if let Some(p) = r.progress {
        let mut m = progress_bar(p, 6, col);
        m.spans
            .push(paint(SUB, format!(" {:>3}%", (p * 100.0 + 0.5) as i64)));
        meta = m;
    } else if !r.meta.is_empty() && !under {
        meta = ln([dim(r.meta.clone())]);
        below = "";
    }
    let mut room = w as isize - 2;
    if meta.width() > 0 {
        room -= meta.width() as isize + 2;
    }
    if !r.link.is_empty() {
        room -= 2; // its ↗
    }
    let mut plain_text = trunc(&r.text, room.max(0) as usize);
    let mut text = ln([paint(TEXT, plain_text.clone())]);
    if !r.link.is_empty() {
        text.spans.push(paint(SUB, " ↗"));
        plain_text += " ↗";
    }
    let gap = 1.max(w as isize - 2 - cw(&plain_text) as isize - meta.width() as isize) as usize;
    let mut out = vec![cat([
        ln([paint(col, mark), plain(" ")]),
        text,
        ln([plain(blanks(gap))]),
        meta,
    ])];
    out.extend(meta_lines(below, w));
    out
}

/// Whether rows w wide put their words under their text: when any one's
/// doesn't fit beside it whole, all of them do, so they line up
/// (metaUnder).
pub fn meta_under(rows: &[Row], w: usize) -> bool {
    rows.iter().any(|r| {
        !r.meta.is_empty()
            && r.progress.is_none()
            && !meta_fits(&r.text, &r.meta, !r.link.is_empty(), w)
    })
}

/// Whether a row's text and its word sit side by side, w wide, with its
/// mark, a gap of two and its link's ↗ (metaFits).
pub fn meta_fits(text: &str, meta: &str, link: bool, w: usize) -> bool {
    let mut n = 2 + cw(&one_line(text)) + 2 + cw(meta);
    if link {
        n += 2;
    }
    n <= w
}

/// A row's words under its text, w wide: lined up with the text, quiet,
/// two lines at most (metaLines).
pub fn meta_lines(meta: &str, w: usize) -> Vec<Line> {
    if meta.is_empty() {
        return Vec::new();
    }
    let room = w.saturating_sub(2);
    let mut ls = wrap(meta, room);
    if ls.len() > 2 {
        ls = vec![ls[0].clone(), trunc(&format!("{} {}", ls[1], ls[2]), room)];
    }
    ls.into_iter().map(|l| ln([plain("  "), dim(l)])).collect()
}

/// What a click on the panel asks of whoever holds the session: the
/// host's part of it (Go's clickViews calls into host and m.viewAction).
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    /// The click wasn't on the panel.
    Missed,
    /// It changed the panel: draw again.
    Drew,
    /// Put the view of this name away (host.DropView).
    Drop(String),
    /// Bring the past view in this file back (host.BringBack); show it.
    BringBack { file: String, name: String },
    /// A button pressed on the row of key (viewAction): "" for the view's
    /// own.
    Act { key: String, a: Action },
    /// Open a link in the browser.
    Open(String),
    /// Watch or pick the run of this ref (a row of rush's at the top).
    Run(String),
}

impl ViewsState {
    /// Where col, row (from the panel's top left) falls on it, if it does
    /// (viewsAt).
    pub fn at(&self, col: i64, row: i64) -> bool {
        self.w > 0 && col >= 0 && (col as usize) < self.w && row >= 0 && (row as usize) < self.h
    }

    /// A click at col, row of the panel (clickViews). On the top edge a
    /// tab shows its view, × puts the shown one away and anywhere else
    /// folds them to a strip; the strip opens them again. A group's title
    /// folds or opens it, a row opens, a button does what it says, and
    /// the bottom edge scrolls a page, or back to the top from the end.
    pub fn click(&mut self, col: i64, row: i64) -> Click {
        if !self.at(col, row) {
            return Click::Missed;
        }
        let (cu, ru) = (col as usize, row as usize);
        if self.shut || self.empty && !self.past {
            self.shut = false;
            self.past = self.empty;
            return Click::Drew;
        }
        if self.past && ru == 0 {
            self.past = false;
            self.scroll = 0;
            return Click::Drew;
        }
        if ru == 0 {
            if let Some(t) = self.tabs.iter().find(|t| cu >= t.from && cu < t.to) {
                if self.tab != t.name {
                    self.tab = t.name.clone();
                    self.scroll = 0;
                }
                return Click::Drew;
            }
            if cu + 5 >= self.w && cu + 3 <= self.w {
                // " × " at the edge's end: put away
                return if self.shown.is_empty() {
                    Click::Drew
                } else {
                    Click::Drop(self.shown.clone())
                };
            }
            if self.has_past && cu + 9 >= self.w && cu + 8 <= self.w {
                // ⟲
                self.past = true;
                self.scroll = 0;
                return Click::Drew;
            }
            self.shut = true;
            return Click::Drew;
        }
        if ru == self.h - 1 {
            if self.scroll >= self.most {
                self.scroll = 0;
            } else {
                self.scroll = self.most.min(self.scroll + 1.max(self.h.saturating_sub(3)));
            }
            return Click::Drew;
        }
        let Some(a) = self.acts.get(ru - 1).cloned() else {
            return Click::Drew;
        };
        self.act(a, cu)
    }

    /// What a click at column cu does on a body row whose click is a
    /// (the rest of clickViews); enter does it at the cursor.
    fn act(&mut self, a: ViewAct, cu: usize) -> Click {
        if !a.btns.is_empty() {
            return match a.btns.iter().find(|b: &&Btn| cu >= b.from && cu < b.to) {
                Some(b) => Click::Act {
                    key: a.row.clone(),
                    a: b.a.clone(),
                },
                None => Click::Drew,
            };
        }
        if !a.run.is_empty() {
            return Click::Run(a.run.clone());
        }
        if a.tick && cu <= 3 {
            // its mark, past the edge and a space
            return Click::Act {
                key: a.row.clone(),
                a: Action {
                    set: Some(Change {
                        done: Some(!a.done),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            };
        }
        if !a.open.is_empty() {
            self.open.insert(a.open.clone(), !a.opened);
        } else if !a.fold.is_empty() {
            self.fold.insert(a.fold.clone(), !a.folded);
        } else if !a.sort.is_empty() {
            let mut c = 0i64;
            for (i, &from) in a.cols.iter().enumerate() {
                if cu >= from {
                    c = i as i64 + 1;
                }
            }
            // By it, then the other way, then as the agent had it.
            let now = self.sorts.get(&a.sort).copied().unwrap_or(0);
            if now == c {
                self.sorts.insert(a.sort.clone(), -c);
            } else if now == -c {
                self.sorts.remove(&a.sort);
            } else {
                self.sorts.insert(a.sort.clone(), c);
            }
        } else if !a.back.is_empty() {
            return Click::BringBack {
                file: a.back.clone(),
                name: a.name.clone(),
            };
        } else if !a.link.is_empty() {
            return Click::Open(a.link.clone());
        }
        Click::Drew
    }

    /// A view brought back: show it (clickViews' back once the host has
    /// it).
    pub fn brought_back(&mut self, name: &str) {
        self.past = false;
        self.tab = name.to_string();
        self.scroll = 0;
    }

    /// The wheel over the panel: whether it scrolled it (wheelViews).
    pub fn wheel(&mut self, col: i64, row: i64, up: bool) -> bool {
        if !self.at(col, row) || self.shut || self.empty && !self.past {
            return false;
        }
        let s = self.scroll as i64 + if up { -3 } else { 3 };
        self.scroll = (s.max(0) as usize).min(self.most);
        true
    }

    /// Gives the panel the keys, the cursor on the first thing in sight a
    /// click does. Whether there is one.
    pub fn take_focus(&mut self) -> bool {
        if self.w == 0 || self.shut || self.empty && !self.past {
            return false;
        }
        let Some(&(from, _)) = self
            .items
            .iter()
            .find(|(from, _)| *from >= self.scroll)
            .or(self.items.first())
        else {
            return false;
        };
        self.focus = true;
        self.cursor = from;
        self.btn = 0;
        true
    }

    /// The body rows under the cursor, none without the keys.
    fn cursor_rows(&self) -> std::ops::RangeInclusive<usize> {
        match self.items.get(self.at_item()) {
            Some(&(from, to)) if self.focus => from..=to,
            _ => 1..=0,
        }
    }

    /// The cursor's thing among items: the one it starts, else the last
    /// before it.
    fn at_item(&self) -> usize {
        self.items
            .iter()
            .rposition(|(from, _)| *from <= self.cursor)
            .unwrap_or(0)
    }

    /// The cursor kept on a thing a click does, after the view changed;
    /// none left, the keys go back.
    fn snap_cursor(&mut self) {
        match self.items.get(self.at_item()) {
            Some(&(from, _)) => self.cursor = from,
            None => self.focus = false,
        }
    }

    /// Whether the cursor's first row is in sight in bh rows.
    fn cursor_shows(&self, bh: usize) -> bool {
        (self.scroll..self.scroll + bh).contains(&self.cursor)
    }

    /// The cursor kept on a thing a click does, and in sight in bh rows.
    fn settle_cursor(&mut self, bh: usize) {
        self.snap_cursor();
        let Some(&(from, to)) = self.items.get(self.at_item()) else {
            return;
        };
        if from < self.scroll {
            self.scroll = from;
        } else if to >= self.scroll + bh {
            self.scroll = (to + 1).saturating_sub(bh).min(from).min(self.most);
        }
    }

    /// A key while the panel has the keys (k as rush names it): ↑↓ move
    /// the cursor, pgup and pgdn scroll a page, enter, space or → do what
    /// a click on it does (open a row, press a button), ← and → pick
    /// among a row of buttons, ← or esc give the keys back. None for a key
    /// that isn't the panel's: the keys go back, and it goes on.
    pub fn key(&mut self, k: &str) -> Option<Click> {
        if !self.focus {
            return None;
        }
        let i = self.at_item();
        let bh = self.h.saturating_sub(2).max(1);
        let page = 1.max(bh.saturating_sub(1));
        let row = self.cursor.saturating_sub(self.scroll);
        let a = self.acts.get(row).cloned().unwrap_or_default();
        let btns = a.btns.len();
        if matches!(k, "enter" | "space" | "right") && !self.cursor_shows(bh) {
            // Scrolled away by the wheel: back to it first.
            self.settle_cursor(bh);
            return Some(Click::Drew);
        }
        match k {
            "up" | "down" => {
                let j = if k == "up" {
                    i.saturating_sub(1)
                } else {
                    (i + 1).min(self.items.len().saturating_sub(1))
                };
                if let Some(&(from, _)) = self.items.get(j) {
                    self.cursor = from;
                }
                if k == "up" && i == 0 {
                    self.scroll = 0; // the top: what's above the first too
                }
                self.btn = 0;
                self.settle_cursor(bh);
            }
            "pgup" | "pgdown" => {
                self.scroll = if k == "pgup" {
                    self.scroll.saturating_sub(page)
                } else {
                    (self.scroll + page).min(self.most)
                };
                let sight = self.scroll..self.scroll + bh;
                let first = self
                    .items
                    .iter()
                    .map(|&(f, _)| f)
                    .find(|f| sight.contains(f));
                if let Some(from) = first
                    && !sight.contains(&self.cursor)
                {
                    self.cursor = from;
                    self.btn = 0;
                }
            }
            "left" if btns > 0 && self.btn > 0 => self.btn -= 1,
            "right" if btns > 0 && self.btn + 1 < btns => self.btn += 1,
            "right" if btns > 0 => {}
            "left" | "esc" => self.focus = false,
            "enter" | "space" | "right" => {
                let cu = match a.btns.get(self.btn) {
                    Some(b) => b.from,
                    None => 4, // past a checklist's mark: it opens, not ticks
                };
                return Some(self.act(a, cu));
            }
            _ => {
                self.focus = false;
                return None;
            }
        }
        Some(Click::Drew)
    }

    /// Lights the row under the pointer when a click on it does
    /// something; whether that changed (hoverViews).
    pub fn hover_at(&mut self, col: i64, row: i64) -> bool {
        let (mut hover, mut hot) = (0, false);
        if self.at(col, row) {
            let ru = row as usize;
            if self.shut || self.empty && !self.past || ru == 0 {
                hot = true;
            } else if ru == self.h - 1 {
                hot = self.most > 0;
            } else if let Some(a) = self.acts.get(ru - 1).filter(|a| a.ok()) {
                hover = ru;
                hot = true;
                if !a.btns.is_empty() {
                    hot = a
                        .btns
                        .iter()
                        .any(|b| col >= b.from as i64 && col < b.to as i64);
                }
            }
        }
        let changed = hover != self.hover
            || hover > 0 && self.acts[hover - 1].lit.is_some() && col != self.hover_x;
        self.hover = hover;
        self.hot = hot;
        self.hover_x = col;
        changed
    }
}

/// Puts a session's views beside the conversation's rows out[top..top+h],
/// in the last side columns of content_w+side (paneViewsSide).
#[allow(clippy::too_many_arguments)]
pub fn pane_views_side(
    out: &mut [Line],
    top: usize,
    h: usize,
    content_w: usize,
    side: usize,
    vs: &[View],
    past: &[View],
    now: i64,
    st: &mut ViewsState,
) {
    let col = views_panel(vs, past, side, h, now, st);
    for (y, c) in col.into_iter().enumerate() {
        let Some(row) = out.get_mut(top + y) else {
            break;
        };
        let mut l = fit(std::mem::take(row), content_w);
        l.spans.extend(c.spans);
        *row = l;
    }
}

impl ViewsState {
    /// Whether the session's views, side being those it has now, show
    /// folded: put away by you, or only past ones to bring back
    /// (rushmode.go:693 viewsFolded).
    pub fn folded(&self, side: &[View]) -> bool {
        self.shut || side.is_empty() && !self.past
    }
}

/// The session's views folded into the column beside the conversation, w
/// wide: how many there are, then each view's title and what it says of
/// itself, and each row's ref. A click opens them, on a view's row at
/// that view (sideViews).
pub fn side_views(vs: &[View], past: &[View], w: usize) -> (Vec<Line>, Vec<String>) {
    use crate::sidecol::{VIEWS_REF, side_head};
    let mut how = Vec::new();
    if !vs.is_empty() {
        how.push(vs.len().to_string());
    }
    if !past.is_empty() {
        how.push(format!("{} put away", past.len()));
    }
    let open = if vs.is_empty() {
        "bring one back ›"
    } else {
        "open ›"
    };
    let mut rows = vec![spread(
        side_head("Views", ln([dim(how.join(" · "))])),
        ln([paint(SUB, open), plain(" ")]),
        w,
    )];
    let mut refs = vec![VIEWS_REF.to_string()];
    const SHOWN: usize = 3;
    for v in vs.iter().take(SHOWN) {
        let title = one_line(&v.title);
        let title = if title.is_empty() {
            v.name.clone()
        } else {
            title
        };
        let mut row = ln([
            plain("   "),
            paint(view_accent(v), "▸"),
            plain(" "),
            paint(TEXT, title),
        ]);
        if !v.summary.is_empty() {
            row.spans.push(plain("  "));
            row.spans.push(dim(one_line(&v.summary)));
        }
        rows.push(fit(row, w));
        refs.push(format!("{VIEWS_REF}:{}", v.name));
    }
    if vs.len() > SHOWN {
        rows.push(ln([faint(format!("   +{} more", vs.len() - SHOWN))]));
        refs.push(VIEWS_REF.to_string());
    }
    (rows, refs)
}
