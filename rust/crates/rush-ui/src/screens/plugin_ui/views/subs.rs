//! While the session's panel is open, what it runs shows at its top
//! rather than under the conversation: the one place to look for what's
//! running. rush draws these rows, not the agent. After Go's
//! internal/ui/panelsubs.go (subsFold/jobsFold :24, subsInPanel :31,
//! panelSubs :44, jobs :84, job :104, panelRuns.row :140) and jobs.go
//! (dockJobs :33, jobsWord :410), as of d0df4ab4 and b0cca013.
//!
//! The subagents and workflows Go puts first, under their heading, wait
//! on the Session reading each run (Go's c.subs and subTails); until it
//! does, the shells, monitors and other tasks the session runs come
//! alone. A task's latest line of output shows when its output has been
//! read (`out`).

use rush_convo::{Job, Session};
use rush_core::text::{Line, width};
use rush_core::time::Ms;

use super::acts::{ViewAct, ViewLine};
use super::panel::{ViewsState, view_heading};
use crate::screens::list_extras::jobs::{Tailed, dock_jobs, job_icon, job_label, job_right};
use crate::style::*;

/// The key the subagents' heading folds by.
pub const SUBS_FOLD: &str = "§subagents";
/// The key the tasks' heading folds by.
pub const JOBS_FOLD: &str = "§tasks";

/// What a frame of the panel's runs is drawn from: the run picked (its
/// ref), the spinner's step, now, and each task's output as last read.
pub struct Runs<'a> {
    pub sel: &'a str,
    pub tick: usize,
    pub now: Ms,
    pub out: &'a dyn Fn(&Job) -> Option<&'a Tailed>,
}

/// The tasks not a subagent's, for the top of the panel: everything
/// running in the background but subagents and workflows (looseJobs,
/// dockJobs).
pub fn loose_jobs(s: &Session) -> Vec<&Job> {
    dock_jobs(s)
}

/// Whether what's running shows at the top of the session's panel: it's
/// open on one of the agent's views, and there are tasks to show; the
/// dock leaves them out then (subsInPanel). width is the panel's.
pub fn subs_in_panel(s: &Session, st: &ViewsState, side: usize, width: usize) -> bool {
    if width <= super::panel::VIEWS_STRIP || side == 0 || st.shut || st.past {
        return false;
    }
    !loose_jobs(s).is_empty()
}

/// The tasks' heading: Shells, Monitors, or Tasks when they're of kinds
/// apart (jobsWord).
pub fn jobs_word(s: &Session, jobs: &[&Job]) -> String {
    let mut word = "";
    for j in jobs {
        let k = s.job_kind(j);
        if word.is_empty() {
            word = k;
        } else if k != word {
            return "tasks".into();
        }
    }
    if word.is_empty() {
        "shell".into()
    } else {
        word.into()
    }
}

/// What the session has running, for the top of its panel, w wide: its
/// shells, monitors and other tasks under their heading, two lines each
/// (panelSubs).
pub fn panel_subs(s: &Session, st: &ViewsState, w: usize, r: &Runs) -> Vec<ViewLine> {
    let mut out = Vec::new();
    let jobs = loose_jobs(s);
    if jobs.is_empty() {
        return out;
    }
    let mut word = jobs_word(s, &jobs);
    if word != "tasks" {
        word.push('s');
    }
    let title = format!("{}{}", word[..1].to_uppercase(), &word[1..]);
    let folded = st.fold.get(JOBS_FOLD).copied().unwrap_or(false);
    out.push(ViewLine::with(
        view_heading(&title, &format!("{} running", jobs.len()), "", folded, w),
        ViewAct {
            fold: JOBS_FOLD.into(),
            folded,
            ..Default::default()
        },
    ));
    if folded {
        return out;
    }
    for (i, j) in jobs.iter().enumerate() {
        job(&mut out, s, j, "", i, w, r);
    }
    out
}

/// A task's two lines: its mark, kind and command, and how long it has
/// run, then its latest line of output; picked, what its keys do (job).
fn job(out: &mut Vec<ViewLine>, s: &Session, j: &Job, lead: &str, i: usize, w: usize, r: &Runs) {
    let kind = s.job_kind(j);
    let (icon, col) = job_icon(kind);
    let mark = if j.background {
        sp(icon, col)
    } else {
        sp(SPINNER[(r.tick + i) % SPINNER.len()], ORANGE)
    };
    let tail = (r.out)(j);
    // Go trims the spaces round its state; their colours stay.
    let right = trimmed(job_right(j, tail, r.now)).unwrap_or_default();
    let room = w as isize
        - width(lead) as isize
        - 2
        - width(kind) as isize
        - 1
        - right.width() as isize
        - 2;
    let label = rush_draw::truncate(&job_label(s, j), room.max(8) as usize, "…").into_owned();
    let top = super::spread(
        line([
            plain(lead),
            mark,
            plain(" "),
            sp(kind, SUB),
            plain(" "),
            sp(label, TEXT),
        ]),
        right,
        w,
    );
    let mut under = Line::new();
    if let Some(last) = tail.and_then(|t| t.lines.last()) {
        let l = line([
            plain(lead),
            plain("  "),
            sp("› ", ORANGE),
            sp(last.clone(), DIM),
        ]);
        under = rush_draw::truncate_line(&l, w, "…");
    }
    let id = format!("job:{}", j.id);
    let mut keys = Line::new();
    if r.sel == id {
        let workflow = kind == "workflow";
        let mut l = line([plain(lead), plain("  ")]);
        join(
            &mut l,
            crate::screens::list_extras::jobs::job_hint(j, workflow, None),
        );
        keys = rush_draw::truncate_line(&l, w, "…");
    }
    row(out, r, &id, w, [top, under, keys]);
}

/// A task's state as jobRight gives it, the spaces round it trimmed as
/// Go's strings.TrimSpace does.
fn trimmed(mut l: Line) -> Option<Line> {
    while l.spans.last().is_some_and(|s| s.text.trim().is_empty()) {
        l.spans.pop();
    }
    let last = l.spans.last_mut()?;
    last.text = last.text.trim_end().to_string();
    let first = l.spans.first_mut()?;
    first.text = first.text.trim_start().to_string();
    Some(l)
}

/// Adds a run's lines, lit when it's picked: a click watches it, or picks
/// it (panelRuns.row).
fn row<const N: usize>(out: &mut Vec<ViewLine>, r: &Runs, id: &str, w: usize, lines: [Line; N]) {
    for l in lines {
        if l.spans.iter().all(|s| s.text.is_empty()) {
            continue;
        }
        let l = if r.sel == id {
            on_ground(fit(l, w), w, SEL_BG)
        } else {
            l
        };
        out.push(ViewLine::with(
            l,
            ViewAct {
                run: id.to_string(),
                ..Default::default()
            },
        ));
    }
}
