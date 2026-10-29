//! Every place that lists a subagent run names it and marks how it stands
//! the same way: what it was asked, then what runs it; its harness's
//! spinner while it runs, then ✓ done, ✗ failed, ⏹ stopped. After Go's
//! internal/ui/subname.go (subTask :15, subTag :21, subName :43, subMark
//! :55, subWord :68).

use rush_convo::{Subagent, run_tag};
use rush_core::adapter::Registry;
use rush_core::text::{Color, Line, Span, width};

use super::paint::dim;
use super::subs::RunStat;
use crate::screens::accounts::look::spin_of;
use crate::style::*;

/// What run sa was asked, in a line (subTask).
pub fn sub_task(sa: &Subagent) -> String {
    let d = one_line(&sa.description);
    if !d.is_empty() {
        d
    } else if !sa.typ.is_empty() {
        sa.typ.clone()
    } else {
        "subagent".into()
    }
}

/// What runs sa: its model, and its harness's mark and name when that
/// isn't the session's own, home (subTag). kind is the run's agent.
pub fn sub_tag(sa: &Subagent, st: Option<&RunStat>, kind: &str, home: &str) -> String {
    let model = st
        .map(|s| s.model.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or(&sa.model);
    // A workflow's agent: its workflow's row says what it is.
    let typ = if sa.workflow.is_empty() {
        sa.typ.as_str()
    } else {
        ""
    };
    let tag = run_tag(kind, home, model, typ);
    if tag == sub_task(sa) {
        String::new()
    } else {
        tag
    }
}

/// sa's name in at most w cells: its task in ink, the tag after it
/// quieter. The task gives way first, down to half the room; then the
/// tag does (subName).
pub fn sub_name(sa: &Subagent, tag: &str, w: usize, ink: Color) -> Line {
    let task = sub_task(sa);
    if tag.is_empty() {
        return line([sp(rush_draw::truncate(&task, w, "…").into_owned(), ink)]);
    }
    let tw = width(tag).min((w / 2).saturating_sub(3).max(4));
    line([
        sp(
            rush_draw::truncate(&task, w.saturating_sub(tw + 3).max(4), "…").into_owned(),
            ink,
        ),
        dim(" · "),
        dim(rush_draw::truncate(tag, tw, "…").into_owned()),
    ])
}

/// How a run stands, as a mark: its harness's spinner while it works,
/// then ✓, ✗ or ⏹ (subMark).
pub fn sub_mark(reg: &Registry, kind: &str, status: &str, live: bool, tick: usize) -> Span {
    if live {
        return spin_of(reg, kind, tick);
    }
    match status {
        "failed" => sp("✗", RED),
        "stopped" => dim("⏹"),
        _ => sp("✓", GREEN),
    }
}

/// How a run stands, in a word (subWord).
pub fn sub_word(status: &str, live: bool) -> Span {
    if live {
        return sp("running", ORANGE);
    }
    match status {
        "failed" => sp("failed", RED),
        "" | "completed" | "ended" => dim("done"),
        s => dim(s.to_string()),
    }
}
