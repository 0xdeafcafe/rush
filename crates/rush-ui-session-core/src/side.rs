//! The column right of a wide conversation, the whole height of the
//! text beside it: what needs you, what runs and what changed, always in
//! sight rather than rows on the box, each part opening with the same
//! heading. Clicking a row opens it. After Go's internal/ui/rushmode.go
//! (sideWidth :697, taskBar :3247, the side refs :3261, sideHead :3270,
//! sideTasks :3277, sideChanges :3321, paneDock's column :3460-3504,
//! rushPane's paste :2694-2720, clickRef's side refs :5042).
//!
//! Rust-only, on purpose: Go laid the column at the foot of the
//! transcript, beside the newest turn; here it runs from the top down,
//! the decisions not answered first, then the session's own tools at
//! work and its subagents', workflows and background tasks (running.rs),
//! the task list, the changes, and the folded views last. Running,
//! Decisions and Changes always show, quiet when there's nothing in
//! them, so the column reads as one and its parts don't jump about.
//!
//! The session's views, open, take the room instead (views_beside): the
//! agent asked to show them, and what runs is drawn at their top then
//! (subs_in_panel). Folded, they're the column's last part (paneDock
//! :3500-3507; their rows are plugin_ui's views::panel::side_views). The
//! changes are the project's files only (sideChanges' ProjectChanges,
//! :3325). What rush did on its own isn't drawn here: Go drew it here
//! for a while (2dcf46c6) and took it out again (8f2c1c99); the header's
//! count and #log have it.

use std::path::Path;

use rush_convo::decisions::Decision;
use rush_convo::{Item, Session, Status, Step, Task};
use rush_core::text::{Line, Span, width};
use rush_core::time::Ms;
use rush_core::tool::Kind;

use super::changes::FileChange;
use super::dock::{dock_card, spread};
use super::paint::{cut, dim, faint, plural};
use super::{conn, open_key};
use crate::frame::{Out, Press};
use crate::model::Model;
pub use crate::sidecol::{VIEWS_REF, side_head, side_width, side_width_with, text_width};
use crate::style::*;

/// A click on a task row in the column opens the Tasks view.
pub const TASKS_REF: &str = "side:tasks";
/// A click on the changes' head opens Changes; on a file, it picked.
pub const CHANGES_REF: &str = "side:changes";
/// A click on a decision opens its block as the card: the block's ref
/// follows ("t4:dec").
pub const DECISIONS_REF: &str = "side:dec:";
/// A click on a tool at work picks its row in the conversation: the
/// row's ref follows ("t4:s:toolu_…").
pub const STEP_REF: &str = "side:step:";

/// A part with nothing in it, w wide: its heading and what it would say,
/// quiet, so the column keeps its shape.
pub fn quiet_part(name: &str, says: &str, w: usize) -> Vec<Line> {
    vec![fit(side_head(name, line([faint(says)])), w)]
}

/// The decisions not answered, in the column, w wide: the newest block's
/// decisions a row each, the agent's suggestion beside each (☆, as the conversation marks it), and how many
/// earlier blocks wait too; each row's ref. open is newest first
/// (open_decisions). Nothing when none waits.
pub fn decisions(open: &[(u32, Vec<Decision>)], w: usize) -> (Vec<Line>, Vec<String>) {
    let Some((n, ds)) = open.first() else {
        return (Vec::new(), Vec::new());
    };
    let r = format!(
        "{DECISIONS_REF}{}",
        rush_convo::render::decisions_ref(&format!("t{n}"))
    );
    let head = side_head("Decisions", line([dim("not answered")]));
    let mut hint = keys(&[("d", "opens the card")]);
    hint.spans.push(plain(" "));
    let mut out = vec![spread(head, hint, w)];
    let mut refs = vec![r.clone()];
    for d in ds {
        // The topic wraps under its own words; the agent's suggestion sits
        // at the far end of its last row when it fits there, else on rows
        // of its own under the topic, never cut.
        let lead = format!("{}.", d.r#ref);
        let ind = 3 + width(&lead) + 1;
        let topic = line([spb(one_line(&d.topic), TEXT)]);
        let mut rows = wrap_line(&topic, w.saturating_sub(ind + 1).max(8));
        let pick = d.options.iter().find(|o| o.recommended);
        let mut extra: Vec<Line> = Vec::new();
        let mut right = Line::new();
        if let Some(o) = pick {
            let chip = line([dim("☆ "), dim(one_line(&o.label)), plain(" ")]);
            let last = rows.last().map_or(0, |l| l.width());
            if ind + last + 2 + chip.width() <= w {
                right = chip;
            } else {
                let body = line([dim("☆ "), dim(one_line(&o.label))]);
                extra = wrap_line(&body, w.saturating_sub(ind + 3).max(8));
            }
        }
        let n = rows.len();
        for (j, t) in rows.drain(..).enumerate() {
            let mut left = if j == 0 {
                line([plain("   "), dim(lead.clone()), plain(" ")])
            } else {
                line([plain(blanks(ind))])
            };
            join(&mut left, t);
            out.push(if j + 1 == n {
                spread(left, right.clone(), w)
            } else {
                left
            });
            refs.push(r.clone());
        }
        for t in extra {
            let mut left = line([plain(blanks(ind + 2))]);
            join(&mut left, t);
            out.push(left);
            refs.push(r.clone());
        }
    }
    if open.len() > 1 {
        let more = open.len() - 1;
        out.push(line([faint(format!(
            "   +{more} earlier {} not answered",
            if more == 1 { "block" } else { "blocks" }
        ))]));
        refs.push(format!(
            "{DECISIONS_REF}{}",
            rush_convo::render::decisions_ref(&format!("t{}", open[1].0))
        ));
    }
    (out, refs)
}

/// What a tool at work does, in two parts: in its agent's words or its
/// tool's name, then what it works on, quieter (after the conversation's
/// step label, render/draw.rs label).
fn step_what(st: &Step, dir: &str) -> (String, String) {
    let x = &st.call().input;
    let rel = |p: &str| -> String {
        if !dir.is_empty()
            && let Ok(r) = Path::new(p).strip_prefix(dir)
            && let Some(r) = r.to_str()
        {
            return r.to_string();
        }
        p.to_string()
    };
    let desc = one_line(&x.description);
    match st.kind {
        Kind::Shell if !desc.is_empty() => {
            let cmd = one_line(&x.command);
            let prog = cmd.split_whitespace().next().unwrap_or("").to_string();
            (desc, prog)
        }
        Kind::Shell => (one_line(&x.command), String::new()),
        Kind::Read | Kind::Edit | Kind::Write | Kind::Notebook | Kind::Delete | Kind::Move => {
            (st.tool.to_string(), rel(&x.path))
        }
        Kind::Search | Kind::Glob => (st.tool.to_string(), one_line(&x.pattern)),
        Kind::Fetch => (st.tool.to_string(), one_line(&x.url)),
        Kind::WebSearch => (st.tool.to_string(), one_line(&x.query)),
        _ => {
            let title = one_line(&st.call().title);
            if title.is_empty() {
                (st.tool.to_string(), desc)
            } else {
                (title, String::new())
            }
        }
    }
}

/// The session's own tools at work in the turn under way, w wide: a
/// heading, then each with its spinner (spin, by its place), what it
/// does, and how long it's been at it or that it waits for you; each
/// row's ref. Its subagents and their steps, workflows and background
/// tasks are parts of their own (running.rs). Nothing when none runs.
pub fn tools(
    s: &Session,
    spin: &dyn Fn(usize) -> Span,
    dir: &str,
    now: Ms,
    w: usize,
) -> (Vec<Line>, Vec<String>) {
    let Some(t) = s.live() else {
        return (Vec::new(), Vec::new());
    };
    let at_work: Vec<&Step> = t
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Step(id) => Some(s.step(*id)),
            _ => None,
        })
        .filter(|st| {
            st.parent.is_none()
                && matches!(st.status, Status::Running | Status::Waiting)
                && st.kind != Kind::Subagent
                && !st.call().input.background
        })
        .collect();
    if at_work.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let head = side_head("Running", line([dim(plural(at_work.len(), "step"))]));
    let mut out = vec![fit(head, w)];
    let mut refs = vec![String::new()];
    for (i, st) in at_work.iter().enumerate() {
        let r = format!("{STEP_REF}t{}:s:{}", t.n, st.id);
        let right = if st.status == Status::Waiting {
            line([sp("waiting for you", YELLOW), plain(" ")])
        } else if st.start > 0 {
            line([dim(dur_since(now - st.start)), plain(" ")])
        } else {
            Line::new()
        };
        let (what, on) = step_what(st, dir);
        let mut left = line([plain("   "), spin(i), plain(" "), sp(what, TEXT)]);
        if !on.is_empty() {
            left.spans.push(plain("  "));
            left.spans.push(dim(on));
        }
        let keep = w.saturating_sub(right.width() + 2).max(12);
        out.push(spread(rush_draw::truncate_line(&left, keep, "…"), right, w));
        refs.push(r);
    }
    (out, refs)
}

/// A task list as a bar, a cell a task in the colour of its mark: done
/// green, under way orange, still to do faint (taskBar).
pub fn task_bar(ts: &[Task]) -> Line {
    let done = ts.iter().filter(|t| t.status == "completed").count();
    let now = ts.iter().filter(|t| t.status == "in_progress").count();
    line([
        sp("▰".repeat(done), GREEN),
        sp("▰".repeat(now), ORANGE),
        faint("▱".repeat(ts.len() - done - now)),
    ])
}

/// The task list in the column, w wide: how far through it is, then up to
/// eight tasks around the one under way. A list all done says nothing
/// (sideTasks).
pub fn tasks(s: &Session, w: usize) -> Vec<Line> {
    let (now, done, total) = s.current();
    if total == 0 || now.is_none() && done == total {
        return Vec::new();
    }
    let mut rest = line([dim(format!("{done} of {total}"))]);
    if total <= 20 {
        rest.spans.push(plain("  "));
        join(&mut rest, task_bar(&s.tasks));
    }
    let mut out = vec![fit(side_head("Tasks", rest), w)];
    let at = s
        .tasks
        .iter()
        .position(|t| t.status == "in_progress")
        .or_else(|| s.tasks.iter().position(|t| t.status != "completed"))
        .unwrap_or(0);
    const SHOWN: usize = 8;
    let from = at
        .saturating_sub(2)
        .min(s.tasks.len().saturating_sub(SHOWN));
    let to = s.tasks.len().min(from + SHOWN);
    if from > 0 {
        out.push(line([faint(format!("   ✓ {from} done before"))]));
    }
    for t in &s.tasks[from..to] {
        let (mark, label) = match t.status.as_str() {
            "in_progress" => (
                sp("■", ORANGE),
                spb(
                    one_line(if t.active.is_empty() {
                        &t.subject
                    } else {
                        &t.active
                    }),
                    TEXT,
                ),
            ),
            "completed" => (sp("✓", GREEN), dim(one_line(&t.subject))),
            _ => (dim("☐"), sp(one_line(&t.subject), SUB)),
        };
        out.push(fit(line([plain("   "), mark, plain(" "), label]), w));
    }
    let more = s.tasks.len() - to;
    if more > 0 {
        out.push(line([faint(format!("   +{more} more"))]));
    }
    out
}

/// What the session changed, in the column, w wide: its adds and deletes,
/// then the five files it changed most, and each row's ref. Nothing when
/// it hasn't edited a file (sideChanges).
pub fn changes(files: &[FileChange], stat: Line, dir: &str, w: usize) -> (Vec<Line>, Vec<String>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut head = side_head("Changes", stat);
    if files.len() > 1 {
        head.spans.push(dim(format!(" · {} files", files.len())));
    }
    let mut out = vec![fit(head, w)];
    let mut refs = vec![CHANGES_REF.to_string()];
    let mut most: Vec<&FileChange> = files.iter().collect();
    most.sort_by_key(|f| std::cmp::Reverse(f.add + f.del));
    const SHOWN: usize = 5;
    for fc in most.iter().take(SHOWN) {
        let mut stat = line([sp(format!("+{}", fc.add), GREEN)]);
        if fc.del > 0 {
            stat.spans.push(plain(" "));
            stat.spans.push(sp(format!("−{}", fc.del), RED));
        }
        let mut path = fc.path.as_str();
        if !dir.is_empty()
            && let Ok(rel) = Path::new(path).strip_prefix(dir)
            && let Some(r) = rel.to_str()
        {
            path = r;
        }
        let room = 8.max(w.saturating_sub(stat.width() + 6));
        let p = crate::header::short_path(path, room);
        let left = line([plain("   "), sp(fit_str(&p, room), TEXT)]);
        stat.spans.push(plain(" "));
        out.push(spread(left, stat, w));
        refs.push(format!("{CHANGES_REF}:{}", fc.path));
    }
    if files.len() > SHOWN {
        out.push(line([faint(format!("   +{} more", files.len() - SHOWN))]));
        refs.push(CHANGES_REF.to_string());
    }
    (out, refs)
}

/// s cut to w cells, with … when it's cut.
fn fit_str(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    rush_draw::truncate(s, w, "…").into_owned()
}

/// The column as the dock last made it, for the conversation to lay over
/// its rows: its rows, their refs, its width; where it was drawn.
#[derive(Default)]
pub struct Side {
    pub rows: Vec<Line>,
    pub refs: Vec<String>,
    pub w: usize,
    /// The most it could be at this width; 0 when it isn't showing.
    pub room: usize,
    /// Its first screen row and column as drawn, and how many rows showed.
    pub y: usize,
    pub x: usize,
    pub shown: usize,
}

/// The column's width beside session key's conversation, text w wide:
/// the room right of the text, left of any views beside it; 0 for none.
pub fn column_width(m: &mut Model, key: &str, w: usize) -> usize {
    if super::views::view_name(m, key) != "conversation" {
        return 0;
    }
    let views = rush_ui_plugin_ui::views::beside::views_beside(m, key, w).2;
    side_width_with(w - views, m.cfg.column_width)
}

/// alt+← and alt+→ (or ctrl, where ⌥ is no key) in a Session (the box empty) make the column beside
/// the conversation two columns narrower or wider, between its least,
/// 44, and all the room left of the text, and keep the width. Nothing
/// when the column isn't showing.
pub fn resize_key(m: &mut Model, key: &str, k: &Press) -> Out {
    let grow = match k.s.as_str() {
        "alt+right" | "ctrl+right" => true,
        "alt+left" | "ctrl+left" => false,
        _ => return Out::Pass,
    };
    let (shown, room) = {
        let s = &conn(m, key).side;
        (s.w, s.room)
    };
    if shown == 0 {
        return Out::Pass;
    }
    // From the width you set, else from the one drawn: the frame may not
    // have been drawn again since the last press.
    let cur = match m.cfg.column_width {
        p if p > 0 => (p as usize).clamp(crate::sidecol::SIDE_MIN, room),
        _ => shown,
    };
    let to = if grow {
        (cur + 2).min(room)
    } else {
        cur.saturating_sub(2).max(crate::sidecol::SIDE_MIN)
    };
    m.cfg.column_width = to as i64;
    rush_ui_base::config::save(m, "couldn't keep the side column's width", move |c| {
        c.column_width = to as i64;
    });
    m.flash(&format!("side column {to} columns"));
    Out::Draw
}

/// The column for session key, text w wide, its parts a row apart, top
/// down: the decisions not answered (yellow), the session's own tools at
/// work (orange) and what else runs (side_runs), the tasks (orange), the
/// changes (green), the folded views. Running, Decisions and Changes
/// show quiet when empty. Empty unless the conversation shows and
/// there's room. Returns whether the tasks went in it.
pub fn make(m: &mut Model, key: &str, w: usize, conversation: bool) -> bool {
    // The room is right of the text, left of any views beside it.
    let views = rush_ui_plugin_ui::views::beside::views_beside(m, key, w).2;
    let sw = side_width_with(w - views, m.cfg.column_width);
    let mut side = Side {
        w: sw,
        room: crate::sidecol::side_room(w - views),
        ..Side::default()
    };
    let runs = std::mem::take(&mut conn(m, key).side_runs);
    if sw == 0 || !conversation {
        conn(m, key).side = side;
        return false;
    }
    let tw = text_width(sw);
    let part = |side: &mut Side, edge, rows: Vec<Line>, refs: Vec<String>| {
        if rows.is_empty() {
            return;
        }
        if !side.rows.is_empty() {
            side.rows.push(Line::new());
            side.refs.push(String::new());
        }
        let n = rows.len();
        side.refs.extend(refs);
        side.refs.resize(side.rows.len() + n, String::new());
        side.rows.extend(dock_card(edge, rows, sw));
    };
    let dir = m
        .session
        .as_ref()
        .map(|v| {
            if v.sess.info.cwd.is_empty() {
                v.sess.cwd.clone()
            } else {
                v.sess.info.cwd.clone()
            }
        })
        .unwrap_or_default();

    // What waits on you: the decisions not answered, worked out again
    // only when the conversation has changed; one answered from the
    // card, its answer not landed yet, no longer waits.
    let open = open_decisions(m, key);
    let (rows, refs) = decisions(&open, tw);
    if rows.is_empty() {
        part(
            &mut side,
            FAINT,
            quiet_part("Decisions", "none open", tw),
            Vec::new(),
        );
    } else {
        part(&mut side, YELLOW, rows, refs);
    }

    // What runs: the session's own tools at work, then its subagents,
    // workflows and background tasks, each a part of its own.
    let kind = super::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let (tick, now) = (m.tick, m.now);
    let (rows, refs) = match m.session.as_ref() {
        Some(v) => {
            let reg = &m.reg;
            let spin = |i: usize| crate::look::spin_of(reg, &kind, tick + i);
            tools(&v.sess, &spin, &dir, now, tw)
        }
        None => (Vec::new(), Vec::new()),
    };
    if rows.is_empty() && runs.is_empty() {
        part(
            &mut side,
            FAINT,
            quiet_part("Running", "nothing now", tw),
            Vec::new(),
        );
    }
    part(&mut side, ORANGE, rows, refs);
    for b in runs {
        part(&mut side, b.edge, b.rows, b.refs);
    }

    // The task list.
    let rows = m
        .session
        .as_ref()
        .map(|v| tasks(&v.sess, tw))
        .unwrap_or_default();
    let beside = !rows.is_empty();
    let n = rows.len();
    part(
        &mut side,
        ORANGE,
        rows,
        std::iter::repeat_n(TASKS_REF.to_string(), n).collect(),
    );

    // What changed: the project's files only, their memory and plans
    // have their own tabs (rushmode.go:3325 ProjectChanges).
    let partial = m.session.as_ref().is_none_or(|v| v.sess.partial);
    if !partial {
        let files = super::changes::project_changes(m).0;
        let stat = super::views::tab_stat(m, key, "changes");
        let (rows, refs) = changes(&files, stat, &dir, tw);
        if rows.is_empty() {
            part(
                &mut side,
                FAINT,
                quiet_part("Changes", "none yet", tw),
                Vec::new(),
            );
        } else {
            part(&mut side, GREEN, rows, refs);
        }
    }

    // Folded views are the column's last part, not a strip of their own.
    if let Some((edge, rows, refs)) =
        rush_ui_plugin_ui::views::beside::folded_in_side(m, key, w, sw)
    {
        part(&mut side, edge, rows, refs);
    }
    conn(m, key).side = side;
    beside
}

/// The decisions blocks of session key not answered, newest first, as
/// the column shows them: one answered from the card whose answer hasn't
/// landed yet left out. Worked out again only when the conversation has
/// changed (open_decisions walks every turn).
fn open_decisions(m: &mut Model, key: &str) -> Vec<(u32, Vec<Decision>)> {
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let c = kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default();
    let at = (v.sess.serial(), v.sess.applied());
    if (c.side_dec.0, c.side_dec.1) != at {
        c.side_dec = (at.0, at.1, rush_convo::render::open_decisions(&v.sess));
    }
    c.side_dec
        .2
        .iter()
        .filter(|(n, _)| {
            let r = rush_convo::render::decisions_ref(&format!("t{n}"));
            !v.o.open
                .get(&rush_convo::render::decisions_sent(&r))
                .copied()
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Lays the column over the conversation's rows, out (cw wide), from its
/// top down to foot, wherever you're reading: foot is the first row that
/// isn't the transcript's (a card or the queue under it), else the
/// body's height. Every row above it stops a cell short of the column,
/// so the column reads as its own and no row's ground runs behind it;
/// its parts stack from the top, the conversation's ground under them,
/// as the views' panel is as tall as what's in it (rushPane's paste,
/// laid from the top rather than at the foot).
pub fn paste(m: &mut Model, key: &str, out: &mut Vec<Line>, foot: usize, cw: usize, top: usize) {
    let c = conn(m, key);
    let at = c.side_pick;
    let side = &mut c.side;
    side.shown = 0;
    if side.rows.is_empty() || side.w == 0 || cw < side.w + 2 {
        return;
    }
    // Go fits the text a cell short of x and pastes after it: the
    // column starts at x-1 (pasteAt over fit(row, x-1)). Its first row
    // is under the one a pinned prompt takes.
    let (first, x) = (1, cw - side.w - 2);
    while out.len() < foot {
        out.push(Line::new());
    }
    let n = side.rows.len().min(foot.saturating_sub(first)); // and clicks only what shows
    let y = first;
    // The item the arrow keys picked, lit as a picked row is.
    let lit = picked_rows(side, at);
    for (i, l) in out.iter_mut().enumerate().take(foot).skip(first) {
        let mut r = fit(cut(l, 0, x), x);
        if (y..y + n).contains(&i) {
            let row = side.rows[i - y].clone();
            join(
                &mut r,
                if lit.contains(&(i - y)) {
                    super::dock::picked1(row, side.w, true)
                } else {
                    row
                },
            );
        }
        *l = fit(r, cw);
    }
    side.shown = n;
    side.y = top + y;
    side.x = 2 + x;
}

/// A click on the column: a decision opens its block as the card, a tool
/// at work picks its row in the conversation, a run or task opens it
/// (subview), a task of the list opens the Tasks view, the changes' head
/// Changes, a file Changes with it picked (clickRef's side refs).
pub fn click(m: &mut Model, x: usize, y: usize) -> bool {
    let Some(key) = open_key(m) else {
        return false;
    };
    let side = &conn(m, &key).side;
    if side.shown == 0
        || y < side.y
        || y >= side.y + side.shown
        || x < side.x
        || x >= side.x + side.w
    {
        return false;
    }
    let r = side.refs.get(y - side.y).cloned().unwrap_or_default();
    act(m, &key, &r)
}

/// What a click, or enter or → on a picked row, does with the row's ref
/// r. Whether it was one that does anything.
fn act(m: &mut Model, key: &str, r: &str) -> bool {
    let key = key.to_string();
    if r == TASKS_REF {
        super::views::show_view(m, &key, "tasks");
        return true;
    }
    if let Some(block) = r.strip_prefix(DECISIONS_REF) {
        rush_ui_session_cards::question::decide_again(m, &key, block);
        return true;
    }
    if let Some(row) = r.strip_prefix(STEP_REF) {
        // The turn under way is at the conversation's end.
        if let Some(v) = m.session.as_mut() {
            v.sel = row.to_string();
            v.pos = crate::session::Pos::End;
        }
        return true;
    }
    if let Some(rest) = r.strip_prefix(VIEWS_REF) {
        rush_ui_plugin_ui::views::beside::open_from_side(m, &key, rest);
        return true;
    }
    if super::subview::click_ref(m, &key, r) {
        return true;
    }
    if let Some(rest) = r.strip_prefix(CHANGES_REF) {
        if let Some(path) = rest.strip_prefix(':')
            && let Some(v) = m.session.as_mut()
        {
            v.sel = format!("chg:{path}");
        }
        super::views::show_view(m, &key, "changes");
        return true;
    }
    false
}

/// The column's items that can be picked: the first row of each run of
/// rows with one ref, as drawn.
fn items(m: &mut Model, key: &str) -> Vec<usize> {
    let side = &conn(m, key).side;
    let n = side.shown.min(side.refs.len());
    (0..n)
        .filter(|&i| !side.refs[i].is_empty() && (i == 0 || side.refs[i - 1] != side.refs[i]))
        .collect()
}

/// Whether a column shows with something in it to pick.
pub fn has_items(m: &mut Model, key: &str) -> bool {
    !items(m, key).is_empty()
}

/// Gives the column the keys, its first item picked. Whether it has one.
pub fn focus(m: &mut Model, key: &str) -> bool {
    let first = items(m, key).first().copied();
    conn(m, key).side_pick = first;
    first.is_some()
}

/// Whether the column has the keys: the row picked is still an item.
pub fn focused(m: &mut Model, key: &str) -> bool {
    let at = conn(m, key).side_pick;
    match at {
        Some(i) if items(m, key).contains(&i) => true,
        Some(_) => {
            conn(m, key).side_pick = None;
            false
        }
        None => false,
    }
}

/// The rows of the item picked in the column.
fn picked_rows(side: &Side, at: Option<usize>) -> std::ops::Range<usize> {
    let Some(i) = at else {
        return 0..0;
    };
    let Some(r) = side.refs.get(i).filter(|r| !r.is_empty()) else {
        return 0..0;
    };
    let end = side.shown.min(side.refs.len());
    let mut to = i;
    while to < end && side.refs[to] == *r {
        to += 1;
    }
    i..to
}

/// The keys while the column has them: ↑↓ through its items, enter, space
/// or → opens one, ← or esc goes back to where the keys came from (the
/// conversation's pick, or the box). Any other key goes back too, and is
/// the box's or the conversation's.
pub fn focus_key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.session.is_none() || m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if !focused(m, &key) {
        return Out::Pass;
    }
    let all = items(m, &key);
    let at = conn(m, &key).side_pick.unwrap_or(0);
    let i = all.iter().position(|&r| r == at).unwrap_or(0);
    match k.s.as_str() {
        "up" => conn(m, &key).side_pick = Some(all[i.saturating_sub(1)]),
        "down" => conn(m, &key).side_pick = Some(all[(i + 1).min(all.len() - 1)]),
        "enter" | "space" | "right" => {
            let r = conn(m, &key).side.refs[at].clone();
            conn(m, &key).side_pick = None;
            act(m, &key, &r);
        }
        "left" | "esc" => conn(m, &key).side_pick = None,
        _ => {
            conn(m, &key).side_pick = None;
            return Out::Pass;
        }
    }
    Out::Draw
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long topic and a long suggestion wrap, the suggestion whole at
    /// the row's end or on rows of its own; never cut with an ellipsis.
    #[test]
    fn decisions_wrap_instead_of_being_cut() {
        use rush_convo::decisions::DecisionOption;
        let long = "skip the timing and merge on the passing tests and the first round's numbers (Codex: end shown at once)";
        let d = Decision {
            r#ref: "1".into(),
            topic: format!("Timing the change: {long}"),
            options: vec![DecisionOption {
                key: "a".into(),
                label: long.into(),
                recommended: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        for w in [30, 44, 60, 79] {
            let (rows, refs) = decisions(&[(1, vec![d.clone()])], w);
            let plain: Vec<String> = rows.iter().map(|l| l.plain()).collect();
            let all = plain.join("\n");
            assert!(!all.contains('…'), "w={w}:\n{all}");
            assert!(plain.iter().all(|r| width(r) <= w), "w={w}:\n{all}");
            assert_eq!(rows.len(), refs.len());
            for word in ["merge", "numbers", "Codex:", "once)", "☆"] {
                assert!(all.contains(word), "w={w}: {word:?} lost:\n{all}");
            }
        }
    }

    /// The bar has a cell a task in its mark's colour, so tasks under way
    /// show apart from those done and those to do (taskrow_test.go
    /// TestTaskBarShowsUnderWay).
    #[test]
    fn task_bar_shows_under_way() {
        let task = |status: &str| Task {
            status: status.into(),
            ..Default::default()
        };
        let bar = task_bar(&[
            task("completed"),
            task("in_progress"),
            task("in_progress"),
            task(""),
        ]);
        let runs: Vec<(String, _)> = bar
            .spans
            .iter()
            .map(|s| (s.text.to_string(), s.style.fg))
            .collect();
        assert_eq!(
            runs,
            vec![
                ("▰".to_string(), GREEN),
                ("▰▰".to_string(), ORANGE),
                ("▱".to_string(), FAINT)
            ]
        );
    }

    #[test]
    fn room_beside_the_text() {
        for (w, want) in [
            (120, 0),
            (170, 0),
            (171, 44),
            (185, 44),
            (200, 54),
            (260, 99),
        ] {
            assert_eq!(side_width(w), want, "{w}");
        }
    }

    /// However wide the column, its rows are laid out 79 wide at most.
    #[test]
    fn rows_readable_in_a_wide_column() {
        for (sw, want) in [(44, 43), (80, 79), (140, 79)] {
            assert_eq!(text_width(sw), want, "{sw}");
        }
    }
}
