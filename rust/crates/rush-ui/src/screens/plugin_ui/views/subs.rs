//! While the session's panel is open, what it runs shows at its top
//! rather than under the conversation: the one place to look for what's
//! running. rush draws these rows, not the agent. After Go's
//! internal/ui/panelsubs.go (subsFold/jobsFold :24, subsInPanel :31,
//! panelSubs :44, jobs :84, job :104, panelRuns.row :140) and jobs.go
//! (dockJobs :33, jobsWord :410), as of d0df4ab4 and b0cca013.
//!
//! The subagents and workflows come first, under their heading, two
//! lines each (panelSubs.subs :52, name :152, doing :166, ran :187,
//! running :194, flow :204, finished :221): what the Session read of each
//! run (session_core::subs) and what the dock worked out it shows this
//! frame (`shown`). A task's latest line of output shows once its output
//! has been read (`out`).

use rush_convo::{Job, Session, Subagent};
use rush_core::adapter::Registry;
use rush_core::text::{Color, Line, width};
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
    /// The session's runs as read, and what the dock shows of them; None
    /// with none read.
    pub runs: Option<&'a crate::screens::session_core::subs::Runs>,
    pub reg: &'a Registry,
    /// The session's own agent.
    pub kind: &'a str,
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
pub fn subs_in_panel(any: bool, st: &ViewsState, side: usize, width: usize) -> bool {
    if width <= super::panel::VIEWS_STRIP || side == 0 || st.shut || st.past {
        return false;
    }
    any
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
    if let Some(runs) = r.runs {
        subs(&mut out, s, st, w, r, runs);
    }
    let jobs: Vec<&Job> = match r.runs {
        Some(runs) => runs.shown.loose.iter().filter_map(|id| s.job(id)).collect(),
        None => loose_jobs(s),
    };
    if jobs.is_empty() {
        return out;
    }
    if !out.is_empty() {
        out.push(ViewLine::default());
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

/// The workflows and subagents, under their heading: those working
/// first, those just finished after (panelRuns.subs).
fn subs(
    out: &mut Vec<ViewLine>,
    s: &Session,
    st: &ViewsState,
    w: usize,
    r: &Runs,
    runs: &crate::screens::session_core::subs::Runs,
) {
    let sh = &runs.shown;
    if !sh.runs() {
        return;
    }
    let working = sh.dock.len() + sh.flows.iter().map(|f| f.agents.len()).sum::<usize>();
    let how = if working == 0 {
        "all done".to_string()
    } else {
        format!("{working} working")
    };
    let folded = st.fold.get(SUBS_FOLD).copied().unwrap_or(false);
    out.push(ViewLine::with(
        view_heading("Subagents", &how, "", folded, w),
        ViewAct {
            fold: SUBS_FOLD.into(),
            folded,
            ..Default::default()
        },
    ));
    if folded {
        return;
    }
    let p = Panel { s, w, r, runs };
    for (i, f) in sh.flows.iter().enumerate() {
        p.flow(out, f, i);
    }
    for (i, sa) in sh.dock.iter().enumerate() {
        p.running(out, sa, "", i);
    }
    for d in &sh.done {
        p.finished(out, d);
    }
}

/// What the panel's runs are drawn from (panelRuns).
struct Panel<'a, 'b> {
    s: &'a Session,
    w: usize,
    r: &'a Runs<'b>,
    runs: &'a crate::screens::session_core::subs::Runs,
}

impl Panel<'_, '_> {
    /// The agent run sa is: a spawned one's own, else the session's.
    fn kind(&self, sa: &Subagent) -> String {
        sa.id
            .strip_prefix(crate::screens::list_extras::spawns::SPAWN_PREFIX)
            .and_then(|h| self.runs.spawns.get(h))
            .map(|r| r.kind.clone())
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| self.r.kind.to_string())
    }

    /// A run's first line: its mark, its name, and on the right how long
    /// it has run or how it ended (name).
    fn name(
        &self,
        lead: &str,
        mark: rush_core::text::Span,
        ink: Color,
        right: &str,
        sa: &Subagent,
    ) -> Line {
        use crate::screens::session_core::subname::{sub_name, sub_tag, sub_task};
        let room = (self.w as isize - width(lead) as isize - 2 - width(right) as isize - 2).max(8)
            as usize;
        // What runs it only when the task shows whole beside it.
        let task = sub_task(sa);
        let tag = sub_tag(sa, self.runs.stats.get(&sa.id), &self.kind(sa), self.r.kind);
        let name = if !tag.is_empty() && width(&task) + 3 + width(&tag) <= room {
            sub_name(sa, &tag, room, ink)
        } else {
            line([sp(rush_draw::truncate(&task, room, "…").into_owned(), ink)])
        };
        let mut l = line([plain(lead), mark, plain(" ")]);
        join(&mut l, name);
        super::spread(l, line([sp(right, DIM)]), self.w)
    }

    /// A running run's second line: what it's doing now, else what it
    /// last did (doing).
    fn doing(&self, lead: &str, sa: &Subagent) -> Line {
        let room = (self.w as isize - width(lead) as isize - 2).max(4) as usize;
        let text = match self.runs.stats.get(&sa.id) {
            None => line([sp("starting…", DIM)]),
            Some(t) if !t.doing.is_empty() => line([sp("› ", ORANGE), sp(t.doing.clone(), TEXT)]),
            Some(t) if !t.did.is_empty() => line([sp(format!("‹ {}", t.did[0]), DIM)]),
            Some(t) if !t.last_words.is_empty() => line([sp(format!("‹ {}", t.last_words), DIM)]),
            Some(_) => line([sp("thinking…", DIM)]),
        };
        let mut l = line([plain(lead), plain("  ")]);
        join(&mut l, rush_draw::truncate_line(&text, room, "…"));
        l
    }

    /// How long a run has gone, to end (ran).
    fn ran(&self, sa: &Subagent) -> String {
        match self.runs.stats.get(&sa.id) {
            Some(t) if t.first > 0 => dur(crate::screens::session_core::workflow::round_s(
                self.r.now - t.first,
            )),
            _ => String::new(),
        }
    }

    fn running(&self, out: &mut Vec<ViewLine>, sa: &Subagent, lead: &str, i: usize) {
        let mark =
            crate::screens::accounts::look::spin_of(self.r.reg, &self.kind(sa), self.r.tick + i);
        let top = self.name(lead, mark, TEXT, &self.ran(sa), sa);
        let under = self.doing(lead, sa);
        row(out, self.r, &format!("run:{}", sa.id), self.w, [top, under]);
        // The shells it runs, under it.
        for (k, j) in crate::screens::session_core::subs::jobs_of(self.runs, self.s, &sa.id)
            .iter()
            .enumerate()
        {
            job(out, self.s, j, &format!("{lead}  "), i + k, self.w, self.r);
        }
    }

    /// A workflow: what it's for and the phase it's in, then its agents
    /// working under it (flow).
    fn flow(
        &self,
        out: &mut Vec<ViewLine>,
        f: &crate::screens::session_core::workflow::DockFlow,
        i: usize,
    ) {
        use crate::screens::session_core::workflow::{round_s, workflow_now};
        let right = if f.g.start > 0 {
            dur(round_s(self.r.now - f.g.start))
        } else {
            String::new()
        };
        let title = rush_draw::truncate(
            &f.g.title,
            (self.w as isize - width(&right) as isize - 6).max(8) as usize,
            "…",
        )
        .into_owned();
        let top = super::spread(
            line([
                sp(SPINNER[(self.r.tick + i) % SPINNER.len()], ORANGE),
                plain(" "),
                sp("⋔", QUEUE),
                plain(" "),
                sp(title, TEXT),
            ]),
            line([sp(right, DIM)]),
            self.w,
        );
        let at = workflow_now(&f.g.phases, self.r.tick + i);
        let under = if at.spans.is_empty() {
            Line::new()
        } else {
            let mut l = line([plain("  ")]);
            join(
                &mut l,
                rush_draw::truncate_line(&at, self.w.saturating_sub(2), "…"),
            );
            l
        };
        row(out, self.r, &f.g.r#ref(), self.w, [top, under]);
        for (k, sa) in f.agents.iter().enumerate() {
            self.running(out, sa, "  ", i + k);
        }
    }

    /// A run that ended lately: ticked off, how it ended and when, and
    /// the last thing it said (finished).
    fn finished(&self, out: &mut Vec<ViewLine>, d: &crate::screens::session_core::subs::DoneRun) {
        use crate::screens::session_core::subname::sub_mark;
        use crate::screens::session_core::workflow::round_s;
        let jobs = self.s.subagent_jobs();
        let (status, _) = self.runs.state(self.s, &d.sa, &jobs, self.r.now);
        let word = if status == "failed" || status == "stopped" {
            status.as_str()
        } else {
            "done"
        };
        let right = format!("{word} {} ago", dur(round_s(self.r.now - d.at)));
        let mark = sub_mark(self.r.reg, &self.kind(&d.sa), &status, false, 0);
        let top = self.name("", mark, SUB, &right, &d.sa);
        let under = match self.runs.stats.get(&d.sa.id) {
            Some(t) if !t.last_words.is_empty() => line([
                plain("  "),
                sp(
                    rush_draw::truncate(
                        &format!("‹ {}", t.last_words),
                        self.w.saturating_sub(2),
                        "…",
                    )
                    .into_owned(),
                    DIM,
                ),
            ]),
            _ => Line::new(),
        };
        row(
            out,
            self.r,
            &format!("run:{}", d.sa.id),
            self.w,
            [top, under],
        );
    }
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
