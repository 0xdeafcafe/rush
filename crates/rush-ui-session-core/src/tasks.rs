//! The Tasks view: the agent's task list as it plans its work, what it's
//! on now, what's next and what's done; and the list where it is, sat on
//! the box. After Go's internal/ui/sessionviews.go:362 (taskLines) and
//! rushmode.go (taskRow :3199, taskAll :3243, taskBar :3247, clickTasks
//! :3338).
//! On a wide conversation the list goes in the column beside it instead
//! (side.rs).

use rush_convo::Session;
use rush_core::text::Line;

use super::paint::{dim, faint};
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

/// The task row's head on the box, as what's picked: ↑ from the box lands
/// on it, and enter, space, ← or → there shut and open the list
/// (Rust-only: only a click did).
pub const TASKS_REF: &str = "tasks";

/// The task list where it is, sat on the box, w wide: how far through it
/// is, then the tasks around the one under way (the last done, it, the
/// next two). Shut, it's the one row with the task under way on it. A list
/// all done says nothing (taskRow).
pub fn task_row(s: &Session, w: usize, shut: bool, picked: bool) -> Vec<Line> {
    let (now, done, total) = s.current();
    if total == 0 || now.is_none() && done == total {
        return Vec::new();
    }
    // Picked by ↑ from the box (TASKS_REF): the bar and the ground every
    // picked row has (dock::picked1), put on below.
    let mut head = line([
        plain("  "),
        sp(if shut { "▸ " } else { "▾ " }, ORANGE),
        spb("Tasks", if picked { TEXT } else { SUB }),
        plain("  "),
        dim(format!("{done} of {total} done")),
        plain("  "),
    ]);
    if total <= 20 {
        join(&mut head, super::side::task_bar(&s.tasks));
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
    // "all tasks ›" isn't on the head (Rust-only: there it sat over the
    // box's edge words, a third thing on one line): it's the last of the
    // open list's rows, after what more there is.
    let mut out = vec![if picked {
        super::dock::picked1(head, w, true)
    } else {
        fit(head, w)
    }];
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
        if i == to - from - 1 {
            label.spans.push(plain("  "));
            if more > 0 {
                label.spans.push(faint(format!("+{more} more · ")));
            }
            label.spans.push(sp(TASK_ALL, SUB));
        }
        let mut l = line([plain("    "), mark, plain(" ")]);
        join(&mut l, label);
        out.push(fit(l, w));
    }
    out
}
