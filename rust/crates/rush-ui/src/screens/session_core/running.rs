//! What the session has running, as the dock shows it under (or the
//! column beside) the conversation: its workflows and subagents at work
//! and those just done, then the shells and monitors no subagent runs,
//! each a block of its own. After Go's internal/ui/rushmode.go
//! (runningPreview :1281, keepRunPick :1251, subEffort :1231, paneDock's
//! panels :3488-3494) and jobs.go (jobWho :111, jobsPreview :358); and
//! the background view's session side: whether it's there and its tab
//! (rushmode.go views :49, tabStat :1469), its rows (jobs.go jobLines
//! :416, drawn by list_extras::jobs), its keys (jobKey :676-735: space,
//! → and d; x, b and ctrl+b wait on the host's stop_task and background
//! ops) and clicks (clickRef :5102-5122), and spawns.go (spawnJob :584,
//! jobKind :595).

use rush_convo::{Job, Session, Subagent};
use rush_core::adapter::Registry;
use rush_core::text::{Color, Line};
use rush_core::time::Ms;

use super::dock::{picked1, spread};
use super::paint::{dim, faint, plural};
use super::side::side_head;
use super::subname::{sub_mark, sub_name, sub_tag, sub_word};
use super::subs::{DOCK_RUNS_SHOWN, DoneRun, Runs, jobs_of, loose_jobs};
use super::workflow::{DockFlow, dock_flows, flow_row, round_s, workflow_now, workflow_of};
use super::{conn, core, open_key};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::accounts::look::spin_of;
use crate::screens::list_extras::jobs::{Bg, Dock, job_hint, job_lines, job_row, jobs_preview};
use crate::style::*;

/// What runs, as this frame shows it: worked out once as the dock is
/// drawn, and read by the views panel after.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    /// The session these are of, by its serial.
    pub serial: u64,
    /// Every run working, newest first.
    pub running: Vec<Subagent>,
    /// The running runs the dock shows (in the background).
    pub dock: Vec<Subagent>,
    pub flows: Vec<DockFlow>,
    pub done: Vec<DoneRun>,
    /// The dock's tasks no running run started, by id.
    pub loose: Vec<String>,
}

impl Shown {
    /// Whether there's anything to show.
    pub fn any(&self) -> bool {
        !self.dock.is_empty()
            || !self.done.is_empty()
            || !self.flows.is_empty()
            || !self.loose.is_empty()
    }
    /// Whether there are runs (not only tasks) to show.
    pub fn runs(&self) -> bool {
        !self.dock.is_empty() || !self.done.is_empty() || !self.flows.is_empty()
    }
}

/// Reads session key's runs when due, and works out what this frame
/// shows of them.
pub fn sync(m: &mut Model, key: &str) {
    super::subs::refresh(m, key);
    let now = m.now;
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return;
    };
    let s = &v.sess;
    let r = &mut kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default()
        .runs;
    let running = r.running(s, now);
    let dock = r.dock_subs(s, &running, now);
    let jobs = s.subagent_jobs();
    let live_of = |sa: &Subagent| r.state(s, sa, &jobs, now).1;
    let flows = dock_flows(r, s, &running, &live_of);
    let loose = loose_jobs(r, s, &running)
        .iter()
        .map(|j| j.id.clone())
        .collect();
    let done = r.done_shown().to_vec();
    r.shown = Shown {
        serial: s.serial(),
        running,
        dock,
        flows,
        done,
        loose,
    };
}

/// The picked run stays where it was: on the same run while it works,
/// and when it finishes, on whichever run now sits in its place
/// (keepRunPick). Returns the pick as it is now.
pub fn keep_run_pick(sel: &str, r: &mut Runs, run: &[Subagent], flows: &[DockFlow]) -> String {
    let Some(id) = sel.strip_prefix("run:") else {
        return sel.to_string();
    };
    if let Some(i) = run.iter().position(|sa| sa.id == id) {
        r.run_pick = i;
        return sel.to_string();
    }
    if r.done_shown().iter().any(|d| d.sa.id == id) {
        r.run_pick = 0; // the running rows show from the top
        return sel.to_string();
    }
    if flows.iter().any(|f| f.agents.iter().any(|sa| sa.id == id)) {
        return sel.to_string(); // under its workflow
    }
    match run.get(r.run_pick.min(run.len().saturating_sub(1))) {
        Some(sa) => format!("run:{}", sa.id),
        None => String::new(),
    }
}

/// Everything a running block is drawn from.
pub struct Ctx<'a> {
    pub s: &'a Session,
    pub r: &'a Runs,
    pub reg: &'a Registry,
    /// The session's own agent.
    pub kind: &'a str,
    pub sel: &'a str,
    pub focus: bool,
    pub tick: usize,
    pub now: Ms,
    /// Nothing typed and nothing queued: ↑ picks.
    pub idle_box: bool,
    /// How many messages wait for each run, by its id.
    pub queued: &'a dyn Fn(&str) -> usize,
}

impl Ctx<'_> {
    /// f with how the dock draws its tasks.
    fn with_dock<T>(&self, f: impl FnOnce(&Dock) -> T) -> T {
        let out = |j: &Job| self.r.out_of(j);
        let who = |j: &Job| self.who(j);
        let flow = |j: &Job, t: usize| workflow_now(&workflow_of(self.r, j), t);
        f(&Dock {
            sel: self.sel,
            focus: self.focus,
            tick: self.tick,
            now: self.now,
            out: &out,
            who: &who,
            flow: &flow,
        })
    }

    /// Whose a task is and where it runs, after its command: "→
    /// Explore(go-ports)" for a subagent's, "⎇ go-ports" for the
    /// session's own in another worktree; nothing for the session's own
    /// at home. Where is the worktree its command cds into, else its
    /// subagent's, when that isn't the session's own (jobWho).
    fn who(&self, j: &Job) -> Line {
        let owner = self.r.job_owner(j);
        let mut wt = rush_convo::text::worktree_in(&self.s.job_command(j)).to_string();
        if let Some(sa) = owner
            && wt.is_empty()
        {
            wt = self
                .r
                .stats
                .get(&sa.id)
                .map(|t| t.worktree.clone())
                .unwrap_or_default();
        }
        let cwd = if self.s.info.cwd.is_empty() {
            &self.s.cwd
        } else {
            &self.s.info.cwd
        };
        if wt == rush_convo::text::worktree_in(&format!("{cwd}/")) {
            wt.clear();
        }
        match owner {
            Some(sa) => {
                let mut l = line([plain("  "), faint("→ "), sp(sa.typ.clone(), BLUE)]);
                if !wt.is_empty() {
                    l.spans.extend([faint("("), dim(wt), faint(")")]);
                }
                l
            }
            None if !wt.is_empty() => line([plain("  "), faint("⎇ "), dim(wt)]),
            None => Line::new(),
        }
    }
}

/// The subagents still working, a heading and then a row each (a few
/// at a time, kept round the one picked): what it was asked, then what
/// it's doing now after what it just did; workflows first, each with its
/// agents under it; those just done ticked off last. With each row's
/// ref (runningPreview).
pub fn running_preview(
    c: &Ctx,
    run: &[Subagent],
    flows: &[DockFlow],
    done: &[DoneRun],
    run_pick: usize,
    w: usize,
) -> (Vec<Line>, Vec<String>) {
    let picked = c.sel.starts_with("run:");
    let start = if picked && run_pick >= DOCK_RUNS_SHOWN {
        run_pick - DOCK_RUNS_SHOWN + 1
    } else {
        0
    };
    let shown = &run[start.min(run.len())..run.len().min(start + DOCK_RUNS_SHOWN)];
    let on_run = run.iter().any(|sa| c.sel == format!("run:{}", sa.id));
    let mut hint = if picked && !on_run {
        keys(&[("space", "read what it did")])
    } else if picked {
        keys(&[("space", "watch it")])
    } else if let Some(j) = c.sel.strip_prefix("job:").and_then(|id| c.s.job(id)) {
        match c.r.job_owner(j) {
            Some(sa) if run.iter().any(|r| r.id == sa.id) => {
                job_hint(j, c.s.job_kind(j) == "workflow", None)
            }
            _ => Line::new(),
        }
    } else if c.focus && c.idle_box {
        keys(&[("↑", "pick one to watch")])
    } else {
        Line::new()
    };
    let working = run.len() + flows.iter().map(|f| f.agents.len()).sum::<usize>();
    let how = if working == 0 {
        "all done".to_string()
    } else {
        format!("{working} working")
    };
    if c.sel.starts_with("wf:") || flows.iter().any(|f| c.sel == f.g.r#ref()) {
        hint = keys(&[("space", "its agents"), ("x", "stop")]);
    }
    hint.spans.push(plain("  "));
    let mut out = vec![spread(side_head("Subagents", line([dim(how)])), hint, w)];
    let mut refs = vec![String::new()];
    if start > 0 {
        out.push(line([dim(format!("  … {start} newer"))]));
        refs.push(String::new());
    }
    let run_row =
        |out: &mut Vec<Line>, refs: &mut Vec<String>, sa: &Subagent, i: usize, lead: &str| {
            let mut doing = Line::new();
            let mut trail = Line::new();
            let mut facts: Vec<String> = Vec::new();
            if let Some(st) = c.r.stats.get(&sa.id) {
                if !st.doing.is_empty() {
                    doing.spans.push(sp(st.doing.clone(), TEXT));
                    // A history read back has no times: no start, no elapsed.
                    let el = c.now - st.since;
                    if st.since > 0 && el >= 5000 {
                        doing.spans.push(dim(format!(" {}", dur(round_s(el)))));
                    }
                } else {
                    doing.spans.push(dim("thinking…"));
                }
                for d in &st.did {
                    trail.spans.push(dim(format!("  ‹ {d}")));
                }
                if trail.spans.is_empty() && !st.last_words.is_empty() {
                    trail.spans.push(dim(format!("  ‹ {}", st.last_words)));
                }
                if st.steps > 0 {
                    facts.push(plural(st.steps, "step"));
                }
                if st.first > 0 {
                    facts.push(dur(round_s(c.now - st.first)));
                }
                if !st.effort.is_empty() {
                    facts.push(format!("{} effort", st.effort));
                }
            } else {
                doing.spans.push(dim("starting…"));
            }
            let right = line([dim(facts.join(" · ")), plain("  ")]);
            let kind = sub_kind(c, sa);
            let tag = sub_tag(sa, c.r.stats.get(&sa.id), &kind, c.kind);
            let mut left = line([plain(lead), spin_of(c.reg, &kind, c.tick + i), plain(" ")]);
            join(&mut left, sub_name(sa, &tag, (w / 2).clamp(16, 36), TEXT));
            let q = (c.queued)(&sa.id);
            if q > 0 {
                left.spans.push(plain(" "));
                left.spans.push(sp(format!("✉{q}"), QUEUE));
            }
            left.spans.push(plain("  "));
            left.spans.push(sp("›", ORANGE));
            left.spans.push(plain(" "));
            join(&mut left, doing);
            join(&mut left, trail);
            let keep = (w as isize - right.width() as isize - 2).max(20) as usize;
            let mut top = spread(rush_draw::truncate_line(&left, keep, "…"), right, w);
            if c.sel == format!("run:{}", sa.id) {
                top = picked1(top, w, c.focus);
            }
            out.push(top);
            refs.push(format!("run:{}", sa.id));
            // What it's running itself, hung under it.
            for (k, j) in jobs_of(c.r, c.s, &sa.id).iter().enumerate() {
                out.push(
                    c.with_dock(|d| job_row(c.s, j, i + k, w, &format!("{lead}  "), false, d)),
                );
                refs.push(format!("job:{}", j.id));
            }
        };
    // Workflows first, each with its agents working under it.
    for (i, f) in flows.iter().enumerate() {
        out.push(flow_row(&f.g, i, w, c.tick, c.now, c.sel, c.focus));
        refs.push(f.g.r#ref());
        for (k, sa) in f.agents.iter().enumerate() {
            run_row(&mut out, &mut refs, sa, i + k, "    ");
        }
    }
    for (i, sa) in shown.iter().enumerate() {
        run_row(&mut out, &mut refs, sa, i, "  ");
    }
    let rest = run.len().saturating_sub(start + shown.len());
    if rest > 0 {
        out.push(line([dim(format!("  + {rest} older"))]));
        refs.push(String::new());
    }
    // Those that finished lately, ticked off under the rest.
    let jobs = c.s.subagent_jobs();
    for d in done {
        let (st, _) = c.r.state(c.s, &d.sa, &jobs, c.now);
        let kind = sub_kind(c, &d.sa);
        let mut facts = Vec::new();
        if let Some(t) = c.r.stats.get(&d.sa.id) {
            if t.steps > 0 {
                facts.push(plural(t.steps, "step"));
            }
            if t.first > 0 && t.last > 0 {
                facts.push(dur(round_s(t.last - t.first)));
            }
        }
        facts.push(format!("{} ago", dur(round_s(c.now - d.at))));
        let tag = sub_tag(&d.sa, c.r.stats.get(&d.sa.id), &kind, c.kind);
        let mut left = line([
            plain("  "),
            sub_mark(c.reg, &kind, &st, false, 0),
            plain(" "),
        ]);
        join(&mut left, sub_name(&d.sa, &tag, (w / 2).clamp(16, 36), SUB));
        left.spans.push(plain("  "));
        left.spans.push(sub_word(&st, false));
        let right = line([dim(facts.join(" · ")), plain("  ")]);
        let keep = (w as isize - right.width() as isize - 2).max(20) as usize;
        let mut row = spread(rush_draw::truncate_line(&left, keep, "…"), right, w);
        if c.sel == format!("run:{}", d.sa.id) {
            row = picked1(row, w, c.focus);
        }
        out.push(row);
        refs.push(format!("run:{}", d.sa.id));
    }
    (out, refs)
}

/// The agent run sa is: a spawned one's own, else the session's
/// (subKind).
pub fn sub_kind(c: &Ctx, sa: &Subagent) -> String {
    sa.id
        .strip_prefix(crate::screens::list_extras::spawns::SPAWN_PREFIX)
        .and_then(|step| c.r.spawns.get(step))
        .map(|r| r.kind.clone())
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| c.kind.to_string())
}

/// The shells, monitors and other tasks no running subagent started, a
/// heading and a row each, with each row's ref (jobsPreview).
pub fn jobs_block(c: &Ctx, jobs: &[&Job], w: usize) -> (Vec<Line>, Vec<String>) {
    let fg = jobs.iter().any(|j| !j.background);
    let hint = if c.focus && fg {
        keys(&[("ctrl+b", "background"), ("↑", "pick")])
    } else if c.focus && c.idle_box {
        keys(&[("↑", "pick one to stop")])
    } else {
        Line::new()
    };
    c.with_dock(|d| jobs_preview(c.s, jobs, w, d, hint))
}

/// A block of the dock: its edge's colour, its rows w wide (the edge not
/// in them), and each row's ref.
pub struct Block {
    pub edge: Color,
    pub rows: Vec<Line>,
    pub refs: Vec<String>,
}

/// The dock's running blocks for session key, rows w wide: the
/// subagents' (blue-violet edge) and the loose tasks' (orange), unless
/// they show at the top of the views panel (paneDock :3488-3494).
pub fn blocks(m: &mut Model, key: &str, w: usize, in_panel: bool) -> Vec<Block> {
    if in_panel || super::views::view_name(m, key) != "conversation" {
        return Vec::new();
    }
    let (tick, now, focus) = (m.tick, m.now, true);
    let idle_box = conn(m, key).input.is_empty() && super::queue::queue_of(m, key).items.is_empty();
    let kind = super::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let reg = m.reg.clone();
    // The pick, kept on the dock's running rows.
    let shown = conn(m, key).runs.shown.clone();
    let sel = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    let sel = keep_run_pick(&sel, &mut conn(m, key).runs, &shown.dock, &shown.flows);
    if let Some(v) = m.session.as_mut()
        && v.sel != sel
        && v.sel.starts_with("run:")
    {
        v.sel = sel.clone();
    }
    let qs: std::collections::HashMap<String, usize> = core(m)
        .local_q
        .iter()
        .filter_map(|(k, q)| {
            k.strip_prefix(&format!("{key}\x00"))
                .map(|id| (id.to_string(), q.items.len()))
        })
        .collect();
    let queued = |id: &str| qs.get(id).copied().unwrap_or(0);
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let r = &kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default()
        .runs;
    let c = Ctx {
        s: &v.sess,
        r,
        reg: &reg,
        kind: &kind,
        sel: &sel,
        focus,
        tick,
        now,
        idle_box,
        queued: &queued,
    };
    let mut out = Vec::new();
    if shown.runs() {
        let (rows, refs) =
            running_preview(&c, &shown.dock, &shown.flows, &shown.done, r.run_pick, w);
        out.push(Block {
            edge: QUEUE,
            rows,
            refs,
        });
    }
    let jobs: Vec<&Job> = shown.loose.iter().filter_map(|id| v.sess.job(id)).collect();
    if !jobs.is_empty() {
        let (rows, refs) = jobs_block(&c, &jobs, w);
        out.push(Block {
            edge: ORANGE,
            rows,
            refs,
        });
    }
    out
}

/// Whether session key has runs or tasks the dock would show.
pub fn any(m: &mut Model, key: &str) -> bool {
    conn(m, key).runs.shown.any()
}

/// The open session's key, when its runs are what's on screen.
pub fn open(m: &Model) -> Option<String> {
    open_key(m)
}

/// Whether task j is a spawned agent's shell, once that agent is found:
/// it's a subagent then (spawnJob, spawns.go:584).
pub fn spawn_job(r: &Runs, j: &Job) -> bool {
    r.spawns
        .values()
        .any(|sp| sp.step == j.tool_use_id && !sp.path.is_empty())
}

/// What task j is as the pane shows it: a found spawned agent's shell is
/// a subagent (jobKind, spawns.go:595).
pub fn job_kind<'a>(r: Option<&Runs>, s: &'a Session, j: &Job) -> &'a str {
    if r.is_some_and(|r| spawn_job(r, j)) {
        return "subagent";
    }
    s.job_kind(j)
}

/// The background tasks of session key but subagents, as job_kind says:
/// with any the background view is there (views, rushmode.go:49), and
/// its tab counts those running (tabStat, rushmode.go:1469).
fn bg_tasks(m: &Model, key: &str) -> Vec<bool> {
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let r = super::subview::runs_of(m, key);
    v.sess
        .jobs()
        .iter()
        .filter(|j| j.background && job_kind(r, &v.sess, j) != "subagent")
        .map(|j| j.running())
        .collect()
}

/// Whether session key has run anything in the background but subagents:
/// the background view is there then.
pub fn has_background(m: &Model, key: &str) -> bool {
    !bg_tasks(m, key).is_empty()
}

/// What the background view's tab says: how many of its tasks run.
pub fn background_stat(m: &Model, key: &str) -> Line {
    let n = bg_tasks(m, key).into_iter().filter(|r| *r).count();
    if n == 0 {
        return Line::new();
    }
    line([sp(format!("{n} running"), ORANGE)])
}

/// The background view's rows for session key, w wide, with each one's
/// ref (jobLines).
pub fn background_lines(m: &mut Model, key: &str, w: usize) -> Vec<(Line, String)> {
    let (tick, now) = (m.tick, m.now);
    let kind = super::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let reg = m.reg.clone();
    let c = conn(m, key);
    let open = c.vs.open.clone();
    let hover = c.vs.hover_ref.clone();
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let r = &kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default()
        .runs;
    let no_queue = |_: &str| 0;
    let ctx = Ctx {
        s: &v.sess,
        r,
        reg: &reg,
        kind: &kind,
        sel: &v.sel,
        focus: true,
        tick,
        now,
        idle_box: false,
        queued: &no_queue,
    };
    let is_open = |r: &str| open.get(r).copied().unwrap_or(false);
    let spawned = |j: &Job| spawn_job(r, j);
    let out = |j: &Job| r.out_of(j);
    let who = |j: &Job| ctx.who(j);
    let flow = |j: &Job, opened: bool| {
        super::workflow::workflow_rows(
            &workflow_of(r, j),
            &format!("job:{}", j.id),
            opened,
            w,
            tick,
            now,
        )
    };
    let mut o = v.o.clone();
    o.width = w;
    let details = |j: &Job| {
        rush_convo::render::job_lines(&v.sess, j, &o, 6)
            .into_iter()
            .map(|l| l.line)
            .collect()
    };
    let b = Bg {
        w,
        now,
        sel: &v.sel,
        focused: true,
        hover: &hover,
        open: &is_open,
        spawned: &spawned,
        out: &out,
        who: &who,
        flow: &flow,
        details: &details,
    };
    job_lines(&v.sess, &b)
}

/// A key on a row picked in the background view: on a task, space or →
/// opens or closes its output (→ only opens) and d its launch script; on
/// a workflow they show its agents in the subagents view; on one of a
/// workflow's agents they watch it (jobKey, jobs.go:707-735; paneKey's
/// run: rows, rushmode.go:4084).
pub fn background_key(m: &mut Model, key: &str, s: &str) -> Out {
    let sel = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    if let Some(id) = sel.strip_prefix("run:") {
        if matches!(s, "space" | " " | "right" | "enter") {
            let id = id.to_string();
            super::subview::watch_run(m, key, &id);
            return Out::Draw;
        }
        return Out::Pass;
    }
    let Some(id) = sel.strip_prefix("job:").map(str::to_string) else {
        return Out::Pass;
    };
    let Some((kind, has_cmd)) = m.session.as_ref().and_then(|v| {
        v.sess
            .job(&id)
            .map(|j| (v.sess.job_kind(j), !v.sess.job_command(j).is_empty()))
    }) else {
        return Out::Pass;
    };
    match s {
        "d" => {
            if !has_cmd {
                m.flash("this task did not report a launch command or process details");
                return Out::Draw;
            }
            let c = conn(m, key);
            let d = format!("job-details:{id}");
            let was = c.vs.open.get(&d).copied().unwrap_or(false);
            c.vs.open.insert(d, !was);
            c.vs.open.insert(sel, true);
            Out::Draw
        }
        "space" | " " | "right" | "enter" if kind == "workflow" => {
            super::subview::open_workflow(m, key, &sel);
            Out::Draw
        }
        "space" | " " | "right" | "enter" => {
            let c = conn(m, key);
            let was = c.vs.open.get(&sel).copied().unwrap_or(false);
            c.vs.open.insert(sel, s == "right" || !was);
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// The background view's keys on a picked row, with the box empty,
/// before the box types them (paneKey's jobKey, jobs.go:676).
pub fn bg_key(m: &mut Model, k: &crate::frame::Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if super::views::view_name(m, &key) != "background" {
        return Out::Pass;
    }
    let empty = conn(m, &key).input.is_empty();
    match k.s.as_str() {
        "d" if empty => background_key(m, &key, "d"),
        s @ ("space" | " " | "right") => background_key(m, &key, s),
        _ => Out::Pass,
    }
}

/// A click on the background view's row r: a workflow shows its agents,
/// one of its agents is watched, a task opens or closes on the first
/// click: its rows are one thing (clickRef, rushmode.go:5102-5122).
pub fn background_click(m: &mut Model, key: &str, r: &str) -> Out {
    if let Some(v) = m.session.as_mut() {
        v.sel = r.to_string();
    }
    let workflow = m
        .session
        .as_ref()
        .is_some_and(|v| super::workflow::is_workflow_ref(&v.sess, r));
    if workflow {
        super::subview::open_workflow(m, key, r);
        return Out::Draw;
    }
    if let Some(id) = r.strip_prefix("run:") {
        let id = id.to_string();
        super::subview::watch_run(m, key, &id);
        return Out::Draw;
    }
    if r.starts_with("job:") {
        let c = conn(m, key);
        let was = c.vs.open.get(r).copied().unwrap_or(false);
        c.vs.open.insert(r.to_string(), !was);
        return Out::Draw;
    }
    Out::Idle
}
