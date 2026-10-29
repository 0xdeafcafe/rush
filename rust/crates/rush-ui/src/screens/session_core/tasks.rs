//! The Tasks view: the agent's task list as it plans its work, what it's
//! on now, what's next and what's done. After Go's
//! internal/ui/sessionviews.go:362 (taskLines).

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
