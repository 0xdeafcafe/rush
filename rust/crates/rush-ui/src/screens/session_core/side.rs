//! The column right of a wide conversation: the task list, all of it in
//! sight, and what the session changed, rather than a row on the box,
//! each part opening with the same heading. Clicking a row opens its
//! view. After Go's internal/ui/rushmode.go (sideWidth :697, taskBar
//! :3247, the side refs :3261, sideHead :3270, sideTasks :3277,
//! sideChanges :3321, paneDock's column :3460-3504, rushPane's paste
//! :2694-2720, clickRef's side refs :5042).
//!
//! The session's views, folded, are its last part (paneDock :3500-3507;
//! their rows are plugin_ui's views::panel::side_views), and the column's
//! room is what's left of them (paneDock :3449-3452). The changes are the
//! project's files only (sideChanges' ProjectChanges, :3325). What rush
//! did on its own isn't drawn here: Go drew it here for a while
//! (2dcf46c6) and took it out again (8f2c1c99); the header's count and
//! #log have it. Not here yet: the
//! subagents and background jobs Go also stacks there (their runs aren't
//! read in Rust yet).

use std::path::Path;

use rush_convo::{Session, Task};
use rush_core::text::{Line, width};

use super::changes::FileChange;
use super::dock::{dock_card, spread};
use super::paint::{cut, dim, faint};
use super::{conn, open_key};
use crate::model::Model;
use crate::style::*;

/// A click on a task row in the column opens the Tasks view.
pub const TASKS_REF: &str = "side:tasks";
/// A click on the changes' head opens Changes; on a file, it picked.
pub const CHANGES_REF: &str = "side:changes";
/// A click on the folded views opens them; on a view's row, at that view.
pub const VIEWS_REF: &str = "side:views";

/// The first row of a part of the dock or the column beside the
/// conversation, the same for each: its name, then what it says about
/// itself. Its colour is the edge's (sideHead).
pub fn side_head(name: &str, rest: Line) -> Line {
    let mut l = line([plain(" "), spb(name, SUB), plain("  ")]);
    join(&mut l, rest);
    l
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

/// The column's width right of text w wide: what's left past the
/// conversation's widest row, from 44 cells up to 80; 0 when there's no
/// room (sideWidth).
pub fn side_width(w: usize) -> usize {
    const TEXT: usize = 124; // convo's capRow: its rows never run further right
    match w.checked_sub(TEXT + 3) {
        Some(sw) if sw >= 44 => sw.min(80),
        _ => 0,
    }
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
    /// Its first screen row and column as drawn, and how many rows showed.
    pub y: usize,
    pub x: usize,
    pub shown: usize,
}

/// The column's width beside session key's conversation, text w wide:
/// the room right of the text, left of any views beside it; 0 for none.
pub fn column_width(m: &mut Model, key: &str, w: usize) -> usize {
    if super::views::view_name(m, key) != "conversation" || m.zen().is_some() {
        return 0;
    }
    let views = crate::screens::plugin_ui::views::beside::views_beside(m, key, w).2;
    side_width(w - views)
}

/// The column for session key, text w wide: the tasks (orange) and the
/// changes (green), a row apart, then what runs (side_runs). Empty unless the conversation shows and
/// there's room. Returns whether the tasks went in it.
pub fn make(m: &mut Model, key: &str, w: usize, conversation: bool) -> bool {
    // The room is right of the text, left of any views beside it.
    let views = crate::screens::plugin_ui::views::beside::views_beside(m, key, w).2;
    let sw = side_width(w - views);
    let mut side = Side {
        w: sw,
        ..Side::default()
    };
    let mut beside = false;
    if sw > 0 && conversation {
        let rows = m
            .session
            .as_ref()
            .map(|v| tasks(&v.sess, sw - 1))
            .unwrap_or_default();
        if !rows.is_empty() {
            beside = true;
            side.refs
                .extend(std::iter::repeat_n(TASKS_REF.to_string(), rows.len()));
            side.rows.extend(dock_card(ORANGE, rows, sw));
        }
        let partial = m.session.as_ref().is_none_or(|v| v.sess.partial);
        if !partial {
            // The project's files only: its memory and plans have their own
            // tabs (rushmode.go:3325 ProjectChanges).
            let files = super::changes::project_changes(m).0;
            let stat = super::views::tab_stat(m, key, "changes");
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
            let (rows, refs) = changes(&files, stat, &dir, sw - 1);
            if !rows.is_empty() {
                if !side.rows.is_empty() {
                    side.rows.push(Line::new());
                    side.refs.push(String::new());
                }
                side.refs.extend(refs);
                side.rows.extend(dock_card(GREEN, rows, sw));
            }
        }
    }
    // What runs, each a part of its own.
    if sw > 0 && conversation {
        for b in std::mem::take(&mut conn(m, key).side_runs) {
            if !side.rows.is_empty() {
                side.rows.push(Line::new());
                side.refs.push(String::new());
            }
            side.refs.extend(b.refs);
            side.rows.extend(dock_card(b.edge, b.rows, sw));
        }
    }
    // Folded views are the column's last part, not a strip of their own.
    if sw > 0
        && conversation
        && let Some((edge, rows, refs)) =
            crate::screens::plugin_ui::views::beside::folded_in_side(m, key, w, sw)
    {
        if !side.rows.is_empty() {
            side.rows.push(Line::new());
            side.refs.push(String::new());
        }
        side.refs.extend(refs);
        side.rows.extend(dock_card(edge, rows, sw));
    }
    conn(m, key).side = side;
    beside
}

/// Lays the column over the conversation's rows, out (cw wide): at the
/// foot, beside the newest turn where you're reading, and under the row a
/// pinned prompt takes when it's taller than that. foot is the first row
/// that isn't the transcript's (a card or the queue under it), else the
/// body's height; every row above it stops a cell short of the column,
/// so the column reads as its own and no row's ground runs behind it
/// (rushPane's paste).
pub fn paste(m: &mut Model, key: &str, out: &mut Vec<Line>, foot: usize, cw: usize, top: usize) {
    let side = &mut conn(m, key).side;
    side.shown = 0;
    if side.rows.is_empty() || side.w == 0 || cw < side.w + 2 {
        return;
    }
    // Go fits the text a cell short of x and pastes after it: the
    // column starts at x-1 (pasteAt over fit(row, x-1)).
    let (first, x) = (1, cw - side.w - 2);
    while out.len() < foot {
        out.push(Line::new());
    }
    let n = side.rows.len().min(foot.saturating_sub(first)); // and clicks only what shows
    let y = first.max(foot - n);
    for (i, l) in out.iter_mut().enumerate().take(foot).skip(first) {
        let mut r = fit(cut(l, 0, x), x);
        if (y..y + n).contains(&i) {
            join(&mut r, side.rows[i - y].clone());
        }
        *l = fit(r, cw);
    }
    side.shown = n;
    side.y = top + y;
    side.x = 2 + x;
}

/// A click on the column: a task opens the Tasks view, the changes' head
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
    if r == TASKS_REF {
        super::views::show_view(m, &key, "tasks");
        return true;
    }
    if let Some(rest) = r.strip_prefix(VIEWS_REF) {
        crate::screens::plugin_ui::views::beside::open_from_side(m, &key, rest);
        return true;
    }
    if super::subview::click_ref(m, &key, &r) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            .map(|s| (s.text.clone(), s.style.fg))
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
        for (w, want) in [(120, 0), (170, 0), (171, 44), (200, 73), (260, 80)] {
            assert_eq!(side_width(w), want, "{w}");
        }
    }
}
