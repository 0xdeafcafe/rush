//! The Tasks view: the agent's task list as it plans its work, what it's
//! on now, what's next and what's done; and the list where it is, sat on
//! the box. After Go's internal/ui/sessionviews.go:362 (taskLines) and
//! rushmode.go (taskRow :3058, taskAll :3104, clickTasks :3197).
//! On a wide conversation the list goes in the column beside it instead
//! (side.rs).

use rush_convo::Session;
use rush_core::text::Line;

use super::paint::{dim, faint, spread};
use crate::model::Model;
use crate::style::*;

/// The task list, w wide; no row has a ref.
pub fn lines(m: &Model, w: usize) -> Vec<(Line, String)> {
    let mut out = Vec::new();
    let Some(v) = m.session.as_ref() else {
        return out;
    };
    let s = &v.sess;
    let mut add = |l: Line| out.push((fit(l, w), String::new()));
    let (_, done, total) = s.current();
    add(line([
        plain("  "),
        spb("Tasks", SUB),
        plain("  "),
        dim(format!("{done} of {total} done")),
    ]));
    add(line([plain("  "), faint("─".repeat(w.saturating_sub(4)))]));
    if total == 0 {
        add(Line::new());
        add(line([
            plain("    "),
            dim("No task list yet. It appears when the agent plans its work."),
        ]));
        return out;
    }
    let groups = [
        ("Now", "in_progress", sp("■", ORANGE)),
        ("Next", "pending", dim("☐")),
        ("Done", "completed", sp("✓", GREEN)),
    ];
    for (title, status, mark) in groups {
        let ts: Vec<_> = s.tasks.iter().filter(|t| t.status == status).collect();
        if ts.is_empty() {
            continue;
        }
        add(Line::new());
        add(line([
            plain("  "),
            spb(title, SUB),
            plain("  "),
            dim(ts.len().to_string()),
        ]));
        for t in ts {
            let (label, ink) = match status {
                "in_progress" => (
                    if t.active.is_empty() {
                        &t.subject
                    } else {
                        &t.active
                    },
                    bold(TEXT),
                ),
                "completed" => (&t.subject, st(DIM)),
                _ => (&t.subject, st(SUB)),
            };
            for (j, r) in wrap(&one_line(label), 20.max(w.saturating_sub(10)))
                .into_iter()
                .enumerate()
            {
                let mut l = if j == 0 {
                    line([plain("    "), mark.clone(), plain(" ")])
                } else {
                    line([plain("      ")])
                };
                l.spans.push(rush_core::text::Span::new(r, ink));
                add(l);
            }
        }
    }
    out
}

/// The way from the task row to the Tasks view.
pub const TASK_ALL: &str = "all tasks ›";

/// The task list where it is, sat on the box, w wide: how far through it
/// is, then the tasks around the one under way (the last done, it, the
/// next two). Shut, it's the one row with the task under way on it. A list
/// all done says nothing (taskRow).
pub fn task_row(s: &Session, w: usize, shut: bool) -> Vec<Line> {
    let (now, done, total) = s.current();
    if total == 0 || now.is_none() && done == total {
        return Vec::new();
    }
    let mut head = line([
        plain("  "),
        sp(if shut { "▸ " } else { "▾ " }, ORANGE),
        spb("Tasks", SUB),
        plain("  "),
        dim(format!("{done} of {total} done")),
        plain("  "),
    ]);
    if total <= 20 {
        head.spans.push(sp("▰".repeat(done), ORANGE));
        head.spans.push(faint("▱".repeat(total - done)));
    }
    if shut && let Some(t) = now {
        let label = if t.active.is_empty() {
            &t.subject
        } else {
            &t.active
        };
        head.spans.push(plain("  "));
        head.spans.push(sp(one_line(label), TEXT));
    }
    let mut out = vec![spread(head, line([sp(TASK_ALL, SUB), plain("  ")]), w)];
    if shut {
        return out;
    }
    // Around the task under way, or the next to do when none is.
    let at = s
        .tasks
        .iter()
        .position(|t| t.status == "in_progress")
        .or_else(|| s.tasks.iter().position(|t| t.status != "completed"))
        .unwrap_or(0);
    let from = at.saturating_sub(1);
    let to = s.tasks.len().min(from + 4);
    for (i, t) in s.tasks[from..to].iter().enumerate() {
        let (mark, mut label) = match t.status.as_str() {
            "in_progress" => (
                sp("■", ORANGE),
                line([spb(
                    one_line(if t.active.is_empty() {
                        &t.subject
                    } else {
                        &t.active
                    }),
                    TEXT,
                )]),
            ),
            "completed" => (sp("✓", GREEN), line([dim(one_line(&t.subject))])),
            _ => (dim("☐"), line([sp(one_line(&t.subject), SUB)])),
        };
        let more = s.tasks.len() - to;
        if i == to - from - 1 && more > 0 {
            label.spans.push(plain("  "));
            label.spans.push(faint(format!("+{more} more")));
        }
        let mut l = line([plain("    "), mark, plain(" ")]);
        join(&mut l, label);
        out.push(fit(l, w));
    }
    out
}
