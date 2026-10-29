//! The Session's views, one tab each under its name: Conversation,
//! Overview, Changes, Tasks and Memory, cycled with [ ] or clicked. The
//! tabs row and the rule lit under the one showing, the context used on
//! the name's row, the keys and wheel that move through a view, and a
//! view drawn whole (any but the conversation) kept to the rows in sight.
//! After Go's internal/ui/rushmode.go (paneViews :39, views :41,
//! otherViews :1340, viewName :1355, tabStat :1363, tabLabel :1418,
//! paneHeader's tabs and rule :2852-2925, readsFromTop :2705, the views'
//! keys in paneKey :4222-4246), header_context.go (headerContext :9,
//! contextAt :28), pane_pointer.go (paneTabAt :28, hoverPane :41,
//! clickPaneTab :79) and model.go:1724 (the wheel over the pane).
//!
//! Not here: the subagents, background and screen views (their runs and
//! live screens aren't read in Rust yet), the stack, and the views beside
//! the conversation (agentviews.go).

use std::collections::HashMap;

use rush_core::adapter::Feature;
use rush_core::text::{Line, Span, Style, width};
use rush_draw::{Button, MouseKind};

use super::paint::{dim, spread};
use super::{agent, conn, open_key};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::session::{HEAD_H, Pos};
use crate::style::*;

/// Every session has these; the rest only when there's something in them.
pub const PANE_VIEWS: [&str; 3] = ["conversation", "overview", "changes"];

/// Where one session's views are (Go's hostConn: view, the scroll of a
/// view drawn whole, the tabs and context label as last drawn, what the
/// pointer is over).
#[derive(Default)]
pub struct ViewState {
    /// The view showing, an index into views().
    pub view: usize,
    /// The first row in sight of a view drawn whole.
    pub top: usize,
    /// The rows of the body as last drawn: how many, and the ref on each.
    pub rows: usize,
    pub refs: Vec<String>,
    /// The ref on every row of a view drawn whole, in sight or not.
    pub all_refs: Vec<String>,
    /// What a view drawn whole has open (its files' diffs), by ref.
    pub open: HashMap<String, bool>,
    /// Files marked reviewed in the changes view (alt+r).
    pub marks: HashMap<String, bool>,
    /// The tabs as last drawn: from and to, cells into the pane, and the
    /// view each is.
    pub tabs: Vec<(usize, usize, usize)>,
    /// The context label on the name's row: from and width, in the pane.
    pub context: (usize, usize),
    /// What the pointer is over: a tab (+1), the context label, a row's
    /// item.
    pub hover_tab: usize,
    pub hover_context: bool,
    pub hover_ref: String,
    /// The view the body was last drawn for: another drops the selection
    /// of text made in it.
    pub drawn: &'static str,
    /// The memory view's first file row in sight.
    pub mem_top: usize,
}

/// The views session key has (views): its conversation, overview and
/// changes; its tasks once it has a list; its memory when its agent says
/// what it reads.
pub fn views(m: &Model, key: &str) -> Vec<&'static str> {
    let mut v = PANE_VIEWS.to_vec();
    if m.session.as_ref().is_some_and(|s| !s.sess.tasks.is_empty()) {
        v.push("tasks");
    }
    let kind = agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    if !kind.is_empty() && m.reg.supports(&kind, Feature::Memory) {
        v.push("memory");
    }
    v
}

/// The view showing in session key (viewName).
pub fn view_name(m: &mut Model, key: &str) -> &'static str {
    let v = views(m, key);
    let i = conn(m, key).vs.view;
    v[i % v.len()]
}

/// Shows the view called name, if the session has it (showView).
pub fn show_view(m: &mut Model, key: &str, name: &str) -> bool {
    let Some(i) = views(m, key).iter().position(|v| *v == name) else {
        return false;
    };
    let c = conn(m, key);
    c.vs.view = i;
    c.vs.top = 0;
    true
}

/// The views [ ] reach from this one, those only some sessions have
/// first (otherViews).
pub fn other_views(m: &mut Model, key: &str) -> String {
    let cur = view_name(m, key);
    let all = views(m, key);
    let mut names: Vec<&str> = all[PANE_VIEWS.len()..]
        .iter()
        .chain(PANE_VIEWS.iter())
        .copied()
        .filter(|v| *v != cur)
        .collect();
    if names.len() > 3 {
        names.truncate(3);
        return names.join(", ") + "…";
    }
    names.join(", ")
}

/// A view's name on its tab: capitalised (tabLabel).
pub fn tab_label(v: &str) -> String {
    let mut c = v.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A count as a tab says it: thousands as k.
fn num(n: i64) -> String {
    if n >= 1000 {
        return format!("{:.1}k", n as f64 / 1000.0);
    }
    n.to_string()
}

/// What a tab says beside its name (tabStat): Tasks how many are done,
/// Changes what this session adds and deletes, else everything
/// uncommitted.
pub fn tab_stat(m: &mut Model, key: &str, v: &str) -> Line {
    let Some(s) = m.session.as_ref() else {
        return Line::new();
    };
    let (partial, dir) = (s.sess.partial, {
        if s.sess.info.cwd.is_empty() {
            s.sess.cwd.clone()
        } else {
            s.sess.info.cwd.clone()
        }
    });
    match v {
        "tasks" => {
            let done = s
                .sess
                .tasks
                .iter()
                .filter(|t| t.status == "completed")
                .count();
            line([dim(format!("{done} of {}", s.sess.tasks.len()))])
        }
        "changes" => {
            if partial {
                return line([dim("counting…")]);
            }
            let (mut a, mut d) = super::changes::stat(m);
            if a + d == 0 && !dir.is_empty() {
                (a, d) = super::changes::tree_of(m, &dir).stat();
            }
            let _ = key;
            let mut p = Vec::new();
            if a > 0 {
                p.push(sp(format!("+{}", num(a)), GREEN));
            }
            if d > 0 {
                p.push(sp(format!("−{}", num(d)), RED));
            }
            let mut l = Line::new();
            for (i, s) in p.into_iter().enumerate() {
                if i > 0 {
                    l.spans.push(plain(" "));
                }
                l.spans.push(s);
            }
            l
        }
        _ => Line::new(),
    }
}

/// The used part of the context window, coloured as it fills, on the
/// name's row (headerContext).
pub fn header_context(m: &Model, hover: bool) -> Option<Line> {
    let v = m.session.as_ref()?;
    let s = &v.sess;
    // Session.ContextWindow (context.go:237): what the agent said, else
    // its model's.
    let win = if s.window > 0 {
        s.window
    } else {
        let model = if s.model.is_empty() {
            &s.info.model
        } else {
            &s.model
        };
        super::overview::agentry(m, &v.key).window(model)
    };
    if s.context <= 0 || win <= 0 {
        return None;
    }
    let pct = rush_core::adapter::fill_of(s.context, win, Default::default()).pct();
    let c = if pct >= 90.0 {
        RED
    } else if pct >= 80.0 {
        YELLOW
    } else {
        TEXT
    };
    let l = line([sp("context ", SUB), sp(format!("{pct:.0}%"), c)]);
    if hover {
        let w = l.width();
        return Some(hover_line(l, w));
    }
    Some(l)
}

/// The Session's header with its tabs: the tabs row and the rule under
/// it drawn again with every view the session has, the one showing lit,
/// and what of its context it has used on the name's row (paneHeader).
pub fn head(m: &mut Model, rows: &mut [Line], w: usize) {
    let Some(key) = open_key(m) else {
        return;
    };
    if rows.len() < HEAD_H || m.agents.iter().all(|a| a.key != key) {
        return; // gone: the frame says so
    }
    let hw = w.saturating_sub(3);
    let views = views(m, &key);
    let cur = conn(m, &key).vs.view % views.len();
    let hover = conn(m, &key).vs.hover_tab;
    // The tabs: " Label stat " each, one apart, from a cell in.
    let mut strip = line([plain(" ")]);
    let mut tabs = Vec::new();
    let (mut on_x, mut on_w) = (0, 0);
    for (i, v) in views.iter().enumerate() {
        let mut stat = tab_stat(m, &key, v);
        if stat.width() > 0 {
            stat.spans.push(plain(" "));
        }
        if i > 0 {
            strip.spans.push(plain(" "));
        }
        let x = strip.width();
        let label = format!(" {} ", tab_label(v));
        let tw = width(&label) + stat.width();
        tabs.push((x, (x + tw).min(hw), i));
        if i == cur {
            (on_x, on_w) = (x, tw);
            let mut t = line([Span::new(label, bold(BRIGHT).on(TAB_ON_BG))]);
            join(&mut t, stat);
            join(&mut strip, on_ground(t, tw, TAB_ON_BG));
        } else {
            let mut t = line([sp(label, SUB)]);
            join(&mut t, stat);
            if hover == i + 1 {
                t = hover_line(t, tw);
            }
            join(&mut strip, t);
        }
    }
    let v = m.session.as_ref();
    let mut chips = Line::new();
    match v.map(|v| (v.verbose, v.o.depth)) {
        Some((true, _)) => chips.spans.push(sp("m3 all shown", ORANGE)),
        Some((_, rush_convo::Depth::Prose)) => chips.spans.push(sp("m0 prose only", ORANGE)),
        Some((_, rush_convo::Depth::Runs)) => chips.spans.push(sp("m1 steps folded", ORANGE)),
        _ => {}
    }
    if !chips.spans.is_empty() {
        chips.spans.push(plain("  "));
    }
    // Filling the screen: nothing says the list is behind it but this.
    chips.spans.push(sp("esc", TEXT));
    chips.spans.push(sp(" back to the list", DIM));
    chips.spans.push(plain(" "));
    let hint = line([sp("   [ ]", DIM), sp(" views", FAINT)]);
    if strip.width() + hint.width() <= hw.saturating_sub(chips.width() + 2) {
        join(&mut strip, hint);
    }
    let sw = strip.width();
    for t in &mut tabs {
        t.1 = t.1.min(sw);
    }
    let row3 = spread(strip, chips, hw);
    rows[3] = chrome(row3, hw);
    // A rule closes the chrome off, lit under the tab showing, orange
    // while the Session has the keys.
    let on_x = on_x + 2; // the pane's two cells in
    rows[4] = fit(
        line([
            sp("─".repeat(on_x.min(w)), FAINT),
            sp("━".repeat(on_w.min(w.saturating_sub(on_x))), ORANGE),
            sp("─".repeat(w.saturating_sub(on_x + on_w)), FAINT),
        ]),
        w,
    );
    // The context used, at the right of the name's row, when there's room.
    let hc = conn(m, &key).vs.hover_context;
    let mut at = (0, 0);
    if let Some(ctx) = header_context(m, hc) {
        let body = super::paint::cut(&rows[1], 2, 2 + hw);
        let used = body.plain().trim_end().len();
        let cw = ctx.width();
        let x = hw.saturating_sub(cw + 1);
        if width(&body.plain()[..used]) + 3 <= x {
            let mut l = super::paint::cut(&body, 0, x);
            join(&mut l, ctx);
            l.spans.push(plain(" "));
            rows[1] = chrome(l, hw);
            at = (x, cw);
        }
    }
    let c = conn(m, &key);
    c.vs.tabs = tabs;
    c.vs.context = at;
}

/// A header row: two cells in, on the chrome's ground, a cell after.
fn chrome(l: Line, iw: usize) -> Line {
    let mut out = line([Span::new("  ", Style::default().on(CHROME_BG))]);
    join(&mut out, on_ground(l, iw, CHROME_BG));
    out.spans
        .push(Span::new(" ", Style::default().on(CHROME_BG)));
    out
}

/// Whether the Session has the keys: no box over it, no place.
fn showing(m: &Model) -> bool {
    m.session.is_some() && m.confirm.is_none() && m.sheet.is_none() && !m.on_place()
}

/// The views' keys, after the box's: [ ] through the views, the page
/// keys and the arrows in the view showing, enter on a view's row.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let s = k.s.as_str();
    let empty = conn(m, &key).input.is_empty();
    let picked = m
        .session
        .as_ref()
        .is_some_and(|v| !v.sel.is_empty() && !v.sel.starts_with("q:"));
    let name = view_name(m, &key);
    let rows = conn(m, &key).vs.rows.max(1);
    match s {
        "[" | "]" if empty => {
            let n = views(m, &key).len();
            let c = conn(m, &key);
            c.vs.view = if s == "[" {
                (c.vs.view % n + n - 1) % n
            } else {
                (c.vs.view % n + 1) % n
            };
            c.vs.top = 0;
            return Out::Draw;
        }
        "pgup" | "pgdown" => {
            let d: isize = if s == "pgup" { 10 } else { -10 };
            scroll(m, &key, name, d, rows);
            return Out::Draw;
        }
        "home" if empty => {
            if name == "conversation" {
                if let Some(v) = m.session.as_mut() {
                    v.home(rows);
                }
            } else {
                conn(m, &key).vs.top = 0;
            }
            return Out::Draw;
        }
        "end" if empty => {
            if name == "conversation" {
                if let Some(v) = m.session.as_mut() {
                    v.pos = Pos::End;
                    v.sel.clear();
                }
            } else {
                conn(m, &key).vs.top = usize::MAX / 2;
            }
            return Out::Draw;
        }
        "up" | "down" if empty || picked => {
            let d = if s == "up" { -1 } else { 1 };
            if name == "conversation" {
                if let Some(v) = m.session.as_mut() {
                    v.move_sel(d, rows);
                }
            } else if name == "memory" {
                return super::memory::step(m, d);
            } else {
                move_sel(m, &key, d);
            }
            return Out::Draw;
        }
        "enter" | "space" | " " | "right" if picked && name != "conversation" => {
            return act(m, &key, name, s);
        }
        "alt+r" if name == "changes" => {
            let sel = m
                .session
                .as_ref()
                .map(|v| v.sel.clone())
                .unwrap_or_default();
            if let Some(path) = sel.strip_prefix("chg:") {
                let c = conn(m, &key);
                let on = c.vs.marks.get(path).copied().unwrap_or(false);
                c.vs.marks.insert(path.to_string(), !on);
                return Out::Draw;
            }
        }
        _ => {}
    }
    Out::Pass
}

/// d rows up (down when negative) in the view showing.
fn scroll(m: &mut Model, key: &str, name: &str, d: isize, rows: usize) {
    if name == "conversation" {
        if let Some(v) = m.session.as_mut() {
            // Go counts from the end: leaving it, the pill takes a row, so
            // the top goes a row less.
            let d = if v.pos == Pos::End && d > 0 { d - 1 } else { d };
            v.scroll(d, rows);
        }
        return;
    }
    let c = conn(m, key);
    c.vs.top = (c.vs.top as isize - d).max(0) as usize;
}

/// The next row up (d < 0) or down with a ref of its own, in a view
/// drawn whole (moveSel over bodyRefs).
fn move_sel(m: &mut Model, key: &str, d: isize) {
    let refs = conn(m, key).vs.all_refs.clone();
    let Some(v) = m.session.as_mut() else {
        return;
    };
    let mut seen: Vec<&String> = Vec::new();
    for r in &refs {
        if !r.is_empty() && seen.last() != Some(&r) && !seen.contains(&r) {
            seen.push(r);
        }
    }
    if seen.is_empty() {
        return;
    }
    let at = seen.iter().position(|r| **r == v.sel);
    // moveSel (rushmode.go:4732): ↓ with nothing picked stays in the box,
    // ↑ picks the last row, the first row stays the first.
    let next = match (at, d < 0) {
        (None, true) => seen.len() - 1,
        (None, false) => return,
        (Some(i), true) => i.saturating_sub(1),
        (Some(i), false) if i + 1 < seen.len() => i + 1,
        (Some(_), false) => {
            v.sel.clear(); // past the last row: back to the box
            return;
        }
    };
    v.sel = seen[next].clone();
}

/// enter, space or → on a view's row (paneKey's enter on a pick).
fn act(m: &mut Model, key: &str, name: &str, s: &str) -> Out {
    let sel = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    if let Some(step) = sel.strip_prefix("jump:") {
        // From a hunk to the step that made it, in the conversation.
        if !step.is_empty() && s != "space" && s != " " {
            let turn = step.split(':').next().unwrap_or("").to_string();
            if let Some(v) = m.session.as_mut() {
                v.o.open.insert(turn, true);
                v.o.open.insert(step.to_string(), true);
                v.sel = step.to_string();
            }
            conn(m, key).vs.view = 0;
            return Out::Draw;
        }
    }
    if sel == "go:memory" {
        if let Some(v) = m.session.as_mut() {
            v.sel.clear();
        }
        show_view(m, key, "memory");
        return Out::Draw;
    }
    if name == "changes" && (sel.starts_with("chg:") || sel.starts_with("tree:")) {
        let c = conn(m, key);
        let open = c.vs.open.get(&sel).copied().unwrap_or(false);
        c.vs.open.insert(sel, s == "right" || !open);
        return Out::Draw;
    }
    Out::Idle
}

/// The Session's body for the view showing: the conversation (with what
/// goes over and beside it), or a view drawn whole and kept to the rows
/// in sight, the row picked among them.
pub fn body(m: &mut Model, w: usize, rows: usize) -> Option<Vec<Line>> {
    let key = open_key(m)?;
    m.agents.iter().find(|a| a.key == key)?;
    let name = view_name(m, &key);
    conn(m, &key).vs.rows = rows;
    if conn(m, &key).vs.drawn != name {
        conn(m, &key).vs.drawn = name;
        super::textsel::drop(m, &key);
    }
    if name == "conversation" {
        return Some(super::conversation::body(m, &key, w, rows));
    }
    conn(m, &key).map.visible = false;
    if let Some(v) = m.session.as_mut() {
        v.side = 0;
    }
    let (now, tick) = (m.now, m.tick);
    if let Some(v) = m.session.as_mut() {
        v.sync(w, now, tick);
    }
    let all: Vec<(Line, String)> = match name {
        "overview" => {
            let mut l = super::overview::lines(m, &key, w);
            if views(m, &key).contains(&"memory") {
                l.extend(super::overview::memory_section(m, &key, w));
            }
            l
        }
        "changes" => super::changes::lines(m, &key, w),
        "tasks" => super::tasks::lines(m, w),
        "memory" => super::memory::lines(m, &key, w, rows),
        _ => Vec::new(),
    };
    let sel = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    let c = conn(m, &key);
    c.vs.all_refs = all.iter().map(|(_, r)| r.clone()).collect();
    // The row picked stays in sight, a row of what's above it with it.
    let n = all.len();
    let most = n.saturating_sub(rows);
    if let Some(i) = all.iter().position(|(_, r)| !sel.is_empty() && *r == sel) {
        if i < c.vs.top {
            c.vs.top = i.saturating_sub(1);
        } else if i >= c.vs.top + rows {
            c.vs.top = i + 1 - rows;
        }
    }
    c.vs.top = c.vs.top.min(most);
    let top = c.vs.top;
    let shown = &all[top..(top + rows).min(n)];
    c.vs.refs = shown.iter().map(|(_, r)| r.clone()).collect();
    let hover = c.vs.hover_ref.clone();
    let refs: Vec<String> = shown.iter().map(|(_, r)| r.clone()).collect();
    let mut out: Vec<Line> = shown
        .iter()
        .map(|(l, r)| {
            if !hover.is_empty() && *r == hover {
                hover_line(l.clone(), w)
            } else {
                l.clone()
            }
        })
        .collect();
    if let Some(v) = m.session.as_mut() {
        v.shown = refs; // the frame's clicks
    }
    out.truncate(rows);
    Some(out)
}

/// The wheel over the pane scrolls the view showing, three rows a turn;
/// at either end it draws nothing. A click on a tab shows its view; the
/// pointer over a tab, the context or a view's row lights it.
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if !showing(m) {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    let Some(top) = m.session.as_ref().map(|v| v.top) else {
        return Out::Pass;
    };
    let name = view_name(m, &key);
    let rows = conn(m, &key).vs.rows.max(1);
    let tabs_y = top.checked_sub(HEAD_H - 3);
    let name_y = top.checked_sub(HEAD_H - 1);
    let tab_at = |m: &mut Model| -> Option<usize> {
        if Some(y) != tabs_y || x < 2 {
            return None;
        }
        let px = x - 2;
        conn(m, &key)
            .vs
            .tabs
            .iter()
            .find(|t| px >= t.0 && px < t.1)
            .map(|t| t.2)
    };
    match e.kind {
        MouseKind::ScrollUp | MouseKind::ScrollDown => {
            let up = e.kind == MouseKind::ScrollUp;
            if name == "conversation" {
                let Some(v) = m.session.as_mut() else {
                    return Out::Pass;
                };
                // A trackpad's momentum keeps turning at either end: those
                // turns move nothing, so they draw nothing.
                if up && v.start == 0 || !up && v.pos == Pos::End {
                    return Out::Idle;
                }
                scroll(m, &key, name, if up { 3 } else { -3 }, rows);
                return Out::Draw;
            }
            let c = conn(m, &key);
            let most = c.vs.all_refs.len().saturating_sub(rows);
            let was = c.vs.top;
            c.vs.top = if up {
                c.vs.top.saturating_sub(3)
            } else {
                (c.vs.top + 3).min(most)
            };
            if c.vs.top == was {
                return Out::Idle;
            }
            Out::Draw
        }
        MouseKind::Down(Button::Left) => {
            let Some(i) = tab_at(m) else {
                if name != "conversation" && y >= top && y < top + rows {
                    let r = conn(m, &key).vs.refs.get(y - top).cloned();
                    if let Some(r) = r.filter(|r| !r.is_empty()) {
                        if let Some(v) = m.session.as_mut() {
                            v.sel = r;
                        }
                        return act(m, &key, name, "enter").or_idle();
                    }
                }
                return Out::Pass;
            };
            let c = conn(m, &key);
            c.vs.view = i;
            c.vs.top = 0;
            Out::Draw
        }
        MouseKind::Moved => {
            let tab = tab_at(m).map_or(0, |i| i + 1);
            let (cx, cw) = conn(m, &key).vs.context;
            let ctx = Some(y) == name_y && cw > 0 && x >= 2 + cx && x < 2 + cx + cw;
            let item = if y >= top && y < top + rows && x >= 2 {
                if name == "conversation" {
                    super::conversation::owner_at(m, y - top)
                } else {
                    conn(m, &key)
                        .vs
                        .refs
                        .get(y - top)
                        .cloned()
                        .unwrap_or_default()
                }
            } else {
                String::new()
            };
            let c = conn(m, &key);
            if c.vs.hover_tab == tab && c.vs.hover_context == ctx && c.vs.hover_ref == item {
                return Out::Pass;
            }
            c.vs.hover_tab = tab;
            c.vs.hover_context = ctx;
            c.vs.hover_ref = item;
            Out::Draw
        }
        _ => Out::Pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_labels() {
        assert_eq!(tab_label("conversation"), "Conversation");
        assert_eq!(tab_label("changes"), "Changes");
        assert_eq!(tab_label(""), "");
        assert_eq!(num(1234), "1.2k");
        assert_eq!(num(999), "999");
    }
}
