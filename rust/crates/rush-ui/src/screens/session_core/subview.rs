//! The subagents view: every run the session started, a line each, under
//! Running and Finished, workflows with their phases and agents, a finished
//! workflow folded to its row and named by when it started, an agent's
//! earlier tries folded under the try that replaced them; and, opened
//! (enter), one run's own conversation under a banner that says whose it
//! is and goes back. A click picks a run: wide, it shows beside the list
//! with its facts, its conversation and a box to message it; narrow, its
//! facts show under its row. Where the user asked, this parts from Go: one
//! line a run rather than two, no conversation beside the list until a
//! click, no click that opens a run whole, the branch from where the run
//! worked. After Go's internal/ui/rushmode.go (views :47, cycleSub :734,
//! openSub :789, watchRun :809, watchingSub :823, closeSub :825,
//! subBanner :846, stripClick :962, subHoverAt :990, subagentLines
//! :1039, subagentList :1059, subRow.appendTo :1213, subEffort :1231,
//! tabStat's subagents :1469, readsFromTop :2841, paneKey's subagents
//! keys :4162, :4255-4262, :4335, :4359, :4399, :4452, moveSel :4932,
//! dockRefs :4900, clickRef :5046, :5102-5122, subWhere :6299,
//! subWorktree :6308), workflow.go (workflowGroupLines :400,
//! openWorkflow :586), subpreview.go (wideSubagents :20, subListWidth
//! :24, subPreviewLines :33, drawSubPreview :94), spawns.go (watchedHost
//! :683, spawnRunFor :694, subKind :721) and model.go (the preview's
//! click :1700 and wheel :1760, subHoverMsg :1192).
//!
//! The opened run is read by subs.rs off the UI thread, as the Session's
//! runs are, into a SessionView of its own (Runs::detail), and drawn as a
//! conversation is; what's typed while it's watched is relay.rs's. Not
//! here: what a working run takes of the machine (subUsage: the process
//! table). A run watched from the background view goes back there
//! (watchRun :777); wide, the pointer right of the list lights no row
//! (subHoverAt :1007-1011).

use std::collections::{HashMap, HashSet};

use rush_convo::{Job, Subagent};
use rush_core::text::{Color, Line, Span, Style, width};
use rush_core::time::Ms;
use rush_draw::{Button, MouseKind};

use super::dock::{picked1, spread};
use super::paint::{dim, faint, plural, tokens};
use super::subname::{sub_mark, sub_name, sub_tag, sub_task, sub_word};
use super::subs::{Detail, DetailSrc, JOB_SUB_PREFIX, RunStat, Runs};
use super::workflow::{Group, agent_row, groups, phase_mark, round_s, workflow_strip};
use super::{agent, conn, open_key};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::screens::list_extras::spawns::SPAWN_PREFIX;
use crate::session::{Pos, SessionView, Source};
use crate::style::*;

/// Watching a subagent: its own, cooler ground (bgSub).
pub const SUB_BG: Color = Color::Rgb(24, 31, 42);
/// What the banner's rows are, for clicks.
pub const BACK_REF: &str = "subback";
pub const STRIP_REF: &str = "substrip";

/// Which run the Session watches and how it got there (Go's hostConn:
/// subOpen, subBack, subFrom, subFromView, subHoverAt, stripHits,
/// subPreview).
#[derive(Default)]
pub struct Watch {
    /// The run opened in the subagents view; "" for its list.
    pub open: String,
    /// Opened from the conversation (the dock, a step): esc goes back
    /// there, to the row it was opened from.
    pub back: bool,
    pub from: String,
    pub from_view: String,
    /// When the pointer came to rest on what it's over.
    pub hover_at: Ms,
    /// The banner's chips: from and to, cells into the pane, and the run
    /// each opens.
    pub strip: Vec<(usize, usize, String)>,
    /// The run's conversation beside the wide list.
    pub peek: Peek,
    /// The pane's width as the view was last drawn (paneW).
    pub pane_w: usize,
    /// The folds opened in the list: a run's earlier tries
    /// ("tries:<id>"), a finished workflow run (its ref).
    pub open_folds: HashSet<String>,
    /// The box under the run beside the list: what's typed, the cursor,
    /// whether it has the keys, and where it was drawn (x, y, w, h).
    pub msg: Vec<char>,
    pub msg_pos: usize,
    pub msg_focus: bool,
    pub msg_at: [usize; 4],
}

/// The run's conversation beside the wide list, as last drawn
/// (subPreview).
#[derive(Default)]
pub struct Peek {
    pub id: String,
    pub selected: String,
    pub hover: String,
    /// Rows up from its end.
    pub scroll: usize,
    pub total: usize,
    pub rows: usize,
    pub width: usize,
    /// Where it was drawn, in screen cells.
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub visible: bool,
}

/// Whether the list is wide enough for a run's conversation beside it
/// (wideSubagents).
pub fn wide(open: &str, w: usize, any: bool) -> bool {
    open.is_empty() && w >= WIDE_AT && any
}

/// How wide the pane is before the picked run shows beside the list: room
/// for the list's one-line rows and a column that reads.
pub const WIDE_AT: usize = 110;

/// The list's width when the run's conversation is beside it
/// (subListWidth).
pub fn list_width(w: usize) -> usize {
    (w / 2).clamp(48, 80)
}

/// Session key's runs, read without changing anything.
pub fn runs_of<'a>(m: &'a Model, key: &str) -> Option<&'a Runs> {
    m.kept
        .peek::<super::Core>()
        .and_then(|c| c.conns.get(key))
        .map(|c| &c.runs)
}

/// Whether session key has runs, so a subagents view.
pub fn has_runs(m: &Model, key: &str) -> bool {
    runs_of(m, key).is_some_and(|r| !r.subs.is_empty())
}

/// The agent run id is: a spawned one's own, else home, the session's
/// (subKind).
pub fn kind_of(r: &Runs, home: &str, id: &str) -> String {
    id.strip_prefix(SPAWN_PREFIX)
        .and_then(|step| r.spawns.get(step))
        .map(|s| s.kind.clone())
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| home.to_string())
}

fn home_kind(m: &Model, key: &str) -> String {
    agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default()
}

/// Whether the Session shows one run's own conversation (watchingSub).
pub fn watching(m: &Model, key: &str) -> bool {
    let Some(c) = m.kept.peek::<super::Core>().and_then(|c| c.conns.get(key)) else {
        return false;
    };
    if c.watch.open.is_empty() || !c.runs.detail.as_ref().is_some_and(|d| d.id == c.watch.open) {
        return false;
    }
    super::views::view_name(m, key) == "subagents"
}

/// The rush session of the spawned agent the Session shows, when rush
/// hosts it: what's sent goes to it rather than the session
/// (watchedHost), with its kind.
pub fn watched_host(m: &mut Model, key: &str) -> Option<(String, String, String)> {
    if !watching(m, key) {
        return None;
    }
    let c = conn(m, key);
    let r = c.runs.hosted_of(&c.watch.open)?;
    Some((c.watch.open.clone(), r.hosted.clone(), r.kind.clone()))
}

/// Where run id's conversation is read from; None for one known only by
/// its task (spawnRunFor, openSub).
fn src_of(m: &Model, key: &str, id: &str) -> Option<DetailSrc> {
    let r = runs_of(m, key)?;
    if let Some(step) = id.strip_prefix(SPAWN_PREFIX) {
        let s = r.spawns.get(step).filter(|s| !s.path.is_empty())?;
        let kind = if s.kind.is_empty() {
            home_kind(m, key)
        } else {
            s.kind.clone()
        };
        if m.reg.reads_as_claude(&kind) {
            return Some(DetailSrc::Transcript {
                path: s.path.clone(),
                whole: true,
            });
        }
        return Some(DetailSrc::History {
            kind,
            path: s.path.clone(),
        });
    }
    let sa = r.subs.iter().find(|sa| sa.id == id)?;
    if sa.path.is_empty() {
        return None;
    }
    Some(DetailSrc::Transcript {
        path: sa.path.clone(),
        whole: false,
    })
}

/// Shows run id whole: read from the start, at once (subDetail).
fn show_detail(m: &mut Model, key: &str, id: &str, src: DetailSrc) {
    let r = &mut conn(m, key).runs;
    if r.detail
        .as_ref()
        .is_some_and(|d| d.id == id && d.src == src)
    {
        return;
    }
    r.detail = Some(Detail {
        id: id.to_string(),
        src,
        view: SessionView::new(key.to_string(), Source::Hosted, 0),
    });
    super::subs::kick(m, key);
    super::subs::refresh(m, key);
}

fn set_sel(m: &mut Model, sel: String) {
    if let Some(v) = m.session.as_mut() {
        v.sel = sel;
    }
}

fn sel(m: &Model) -> String {
    m.session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default()
}

/// Drills into one run's own conversation (openSub).
pub fn open_sub(m: &mut Model, key: &str, id: &str) {
    let Some(src) = src_of(m, key, id) else {
        if id.starts_with(JOB_SUB_PREFIX) {
            let kind = home_kind(m, key);
            if m.reg.reads_as_claude(&kind) {
                m.flash(
                    "that subagent's conversation isn't found yet; its step is in the conversation",
                );
            } else {
                m.flash(
                    "this agent keeps that subagent's conversation to itself; its step is in the conversation",
                );
            }
        }
        return;
    };
    let w = &mut conn(m, key).watch;
    w.open = id.to_string();
    w.back = false;
    w.from.clear();
    set_sel(m, String::new());
    show_detail(m, key, id, src);
}

/// Watches run id from the dock, or from its workflow's row in the
/// background view, which esc goes back to (watchRun, rushmode.go:777).
pub fn watch_run(m: &mut Model, key: &str, id: &str) {
    let from_bg = super::views::view_name(m, key) == "background";
    super::views::show_view(m, key, "subagents");
    open_sub(m, key, id);
    let w = &mut conn(m, key).watch;
    w.back = true;
    if from_bg {
        w.from = format!("run:{id}");
        w.from_view = "background".into();
    }
}

/// Shows the subagents view with run id picked: beside the list when
/// there's room, its facts under its row when not. Nothing opens across
/// the whole view; enter does that.
pub fn pick_run(m: &mut Model, key: &str, id: &str) {
    super::views::show_view(m, key, "subagents");
    let w = &mut conn(m, key).watch;
    w.open.clear();
    w.back = false;
    w.msg_focus = false;
    conn(m, key).runs.detail = None;
    set_sel(m, format!("sub:{id}"));
}

/// Leaves the opened run for wherever it was opened from (closeSub).
pub fn close_sub(m: &mut Model, key: &str) {
    let w = {
        let wt = &mut conn(m, key).watch;
        let keep = Watch {
            open_folds: std::mem::take(&mut wt.open_folds),
            msg: std::mem::take(&mut wt.msg),
            msg_pos: wt.msg_pos,
            ..Watch::default()
        };
        std::mem::replace(wt, keep)
    };
    conn(m, key).runs.detail = None;
    if w.back {
        // Opened from the conversation: back to it, on the row it was
        // opened from (its step, or its row on the dock).
        conn(m, key).vs.view = 0;
        let from = if w.from.is_empty() {
            format!("run:{}", w.open)
        } else {
            w.from
        };
        set_sel(m, from);
        if !w.from_view.is_empty() {
            super::views::show_view(m, key, &w.from_view);
        }
    } else {
        set_sel(m, String::new());
    }
}

/// Steps through the conversation and its runs, running ones first, then
/// the rest, newest first, each shown in place; the conversation sits at
/// either end (cycleSub).
pub fn cycle_sub(m: &mut Model, key: &str, dir: isize) {
    let now = m.now;
    let order: Vec<String> = {
        let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
            return;
        };
        let Some(r) = runs_of(m, key) else {
            return;
        };
        let mut order: Vec<String> = r
            .running(&v.sess, now)
            .into_iter()
            .map(|sa| sa.id)
            .collect();
        for sa in r.subs.iter().rev() {
            if !order.contains(&sa.id) {
                order.push(sa.id.clone());
            }
        }
        order
    };
    if order.is_empty() {
        return;
    }
    let n = order.len() as isize + 1;
    let mut cur = 0;
    if super::views::view_name(m, key) == "subagents" {
        let open = conn(m, key).watch.open.clone();
        if let Some(i) = order.iter().position(|id| *id == open) {
            cur = i as isize + 1;
        }
    }
    let next = ((cur + dir) % n + n) % n;
    if next == 0 {
        conn(m, key).watch.open.clear();
        conn(m, key).runs.detail = None;
        let c = conn(m, key);
        c.vs.view = 0;
        set_sel(m, String::new());
        return;
    }
    super::views::show_view(m, key, "subagents");
    let (back, from) = {
        let w = &conn(m, key).watch;
        (w.back, w.from.clone())
    };
    open_sub(m, key, &order[next as usize - 1]);
    // A sibling is still watched from where this one was.
    let w = &mut conn(m, key).watch;
    w.back = back;
    w.from = from;
}

/// Shows the subagents view on the workflow ref picks: its phases and
/// its agents' runs (openWorkflow).
pub fn open_workflow(m: &mut Model, key: &str, r: &str) {
    super::views::show_view(m, key, "subagents");
    let w = &mut conn(m, key).watch;
    w.open.clear();
    w.back = false;
    w.from.clear();
    w.from_view.clear();
    w.msg_focus = false;
    conn(m, key).runs.detail = None;
    set_sel(m, r.to_string());
}

/// What the subagents view's tab says: how many run (tabStat).
pub fn tab_stat(m: &Model, key: &str) -> Line {
    let (Some(v), Some(r)) = (m.session.as_ref(), runs_of(m, key)) else {
        return Line::new();
    };
    let n = r.running(&v.sess, m.now).len();
    if n == 0 {
        return Line::new();
    }
    line([sp(format!("{n} running"), ORANGE)])
}

/// The rows ↑↓ go through between the conversation and the box, top to
/// bottom as drawn: the dock's workflows, runs and their tasks, those
/// just done, then the loose tasks (dockRefs, without the queue's).
pub fn dock_refs(m: &mut Model, key: &str) -> Vec<String> {
    if super::views::view_name(m, key) != "conversation" {
        return Vec::new();
    }
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
    let sh = &r.shown;
    let mut refs = Vec::new();
    for f in &sh.flows {
        refs.push(f.g.r#ref());
        refs.extend(f.agents.iter().map(|sa| format!("run:{}", sa.id)));
    }
    for sa in &sh.dock {
        refs.push(format!("run:{}", sa.id));
        for j in super::subs::jobs_of(r, &v.sess, &sa.id) {
            refs.push(format!("job:{}", j.id));
        }
    }
    refs.extend(sh.done.iter().map(|d| format!("run:{}", d.sa.id)));
    refs.extend(sh.loose.iter().map(|id| format!("job:{id}")));
    refs
}

/// A run as the list shows it (subListRow).
struct Row<'a> {
    sa: &'a Subagent,
    st: Option<&'a RunStat>,
    status: String,
    live: bool,
}

/// What the list is drawn from.
struct List<'a> {
    r: &'a Runs,
    reg: &'a rush_core::adapter::Registry,
    home: &'a str,
    sel: &'a str,
    /// The folds opened: earlier tries, finished workflow runs.
    open: &'a HashSet<String>,
    /// The picked run shows beside the list, so its row needn't say more.
    wide: bool,
    tick: usize,
    now: Ms,
    w: usize,
    out: Vec<(Line, String)>,
}

/// Picks a row: its ref.
pub const TRIES_REF: &str = "tries:";
/// Opens run id whole, from the line of facts under it when picked.
pub const OPEN_REF: &str = "open:";

/// How long a run has gone, or went.
fn took_of(st: Option<&RunStat>, live: bool, now: Ms) -> String {
    let (first, last) = st.map_or((0, 0), |t| (t.first, t.last));
    let end = if live { now } else { last };
    if first > 0 && end > 0 {
        dur(round_s(end - first))
    } else {
        String::new()
    }
}

/// When a workflow run started, to tell runs of one name apart: "14:02",
/// or "Mon 14:02" when it wasn't today.
pub fn started(at: Ms, now: Ms) -> String {
    if at <= 0 {
        return String::new();
    }
    let c = crate::screens::settings::paint::civil(at);
    let hm = format!("{:02}:{:02}", c.hour, c.minute);
    if now - at > 20 * 3_600_000 {
        return format!("{} {hm}", c.weekday_name());
    }
    hm
}

/// What a run is doing or how it ended, short: its step under way while it
/// works, else done, failed or stopped.
fn short_status(r: &Row, w: usize) -> Span {
    if r.live {
        let doing =
            r.st.map(|t| one_line(&t.doing))
                .filter(|d| !d.is_empty())
                .unwrap_or_default();
        if doing.is_empty() {
            return sp("running", ORANGE);
        }
        return sp(rush_draw::truncate(&doing, w, "…").into_owned(), SUB);
    }
    sub_word(&r.status, false)
}

/// The run's facts: what runs it, where it works, its steps, what it
/// cost and its effort (subagentList's second row).
fn facts_of(r: &Runs, sa: &Subagent, home: &str, w: usize) -> Vec<String> {
    let kind = kind_of(r, home, &sa.id);
    let st = r.stats.get(&sa.id);
    let mut facts: Vec<String> = Vec::new();
    let tag = sub_tag(sa, st, &kind, home);
    if !tag.is_empty() {
        facts.push(tag);
    }
    let wt = worktree_of(r, &sa.id);
    if !wt.is_empty() {
        facts.push(format!("⎇ {wt}"));
    }
    if let Some(t) = st {
        if t.steps > 0 {
            facts.push(plural(t.steps, "step"));
        }
        if t.requests > 0 {
            if w >= 100 {
                facts.push(format!("in {}", tokens(t.tokens_in)));
                facts.push(format!("out {}", tokens(t.tokens_out)));
            }
            if t.cost > 0.0 {
                facts.push(money(t.cost));
            }
        }
        if !t.effort.is_empty() {
            facts.push(format!("{} effort", t.effort));
        }
    }
    facts
}

/// Where run id works: the worktree its own calls reached into most;
/// for an agent rush hosts, else its session's folder when that isn't
/// the parent's. Never the folder the parent was in when it started it,
/// which a subagent's transcript says: "" when nothing says.
pub fn worktree_of(r: &Runs, id: &str) -> String {
    if let Some(t) = r.stats.get(id).filter(|t| !t.worktree.is_empty()) {
        return t.worktree.clone();
    }
    r.wt.get(id).cloned().unwrap_or_default()
}

impl List<'_> {
    /// A run's row: how it stands, what it was asked, what it's doing and
    /// how long it went. Picked where nothing shows beside the list, a
    /// line of its facts under it, a click on which opens it.
    fn draw(&mut self, r: &Row, i: usize, indent: &str, quiet: bool) {
        let (sa, w) = (r.sa, self.w);
        let rf = format!("sub:{}", sa.id);
        let took = took_of(r.st, r.live, self.now);
        let kind = kind_of(self.r, self.home, &sa.id);
        let mark = sub_mark(self.reg, &kind, &r.status, r.live, self.tick + i);
        let status_w = (w / 5).clamp(10, 20);
        let status = short_status(r, status_w);
        let pad = status_w.saturating_sub(status.text.chars().count());
        let mut right = line([status, plain(blanks(pad)), plain("  ")]);
        right.spans.push(dim(format!("{took:>6}")));
        right.spans.push(plain("  "));
        let room = (w as isize - right.width() as isize - 6 - indent.len() as isize).max(12);
        let name = rush_draw::truncate(&sub_task(sa), room as usize, "…").into_owned();
        let ink = if r.live {
            bold(TEXT)
        } else if quiet {
            Style::fg(DIM)
        } else {
            Style::fg(TEXT)
        };
        let top = spread(
            line([
                plain(format!("{indent}  ")),
                mark,
                plain(" "),
                Span::new(name, ink),
            ]),
            right,
            w,
        );
        if self.sel != rf {
            self.out.push((fit(top, w), rf));
            return;
        }
        self.out.push((picked1(top, w, true), rf.clone()));
        if self.wide {
            return;
        }
        // How to open it always shows; the facts give way to it.
        let open = "enter opens it ›";
        let lead = indent.len() + 6;
        let room = w.saturating_sub(lead + width(open) + 6);
        let facts = facts_of(self.r, sa, self.home, w).join(" · ");
        let mut second = line([
            plain(blanks(lead)),
            dim(rush_draw::truncate(&facts, room, "…").into_owned()),
        ]);
        if !facts.is_empty() {
            second.spans.push(dim("  ·  "));
        }
        second.spans.push(sp(open, BLUE));
        self.out.push((
            on_ground(rush_draw::truncate_line(&second, w, "…"), w, SEL_BG),
            format!("{OPEN_REF}{}", sa.id),
        ));
    }

    /// A fold of earlier tries, under the row of the try that replaced
    /// them: how many, and, opened, each a row of its own, quieter.
    fn tries(&mut self, rf: String, tries: &[&Row], indent: &str) {
        if tries.is_empty() {
            return;
        }
        let open = self.open.contains(&rf);
        let n = tries.len();
        let words = format!("{} earlier {}", n, if n == 1 { "try" } else { "tries" });
        let row = line([
            plain(format!("{indent}    ")),
            faint(if open { "▾ " } else { "▸ " }),
            dim(words),
            faint(if open { "" } else { " · click to show" }),
        ]);
        let picked = self.sel == rf;
        self.out.push((
            if picked {
                picked1(row, self.w, true)
            } else {
                fit(row, self.w)
            },
            rf,
        ));
        if !open {
            return;
        }
        for (i, r) in tries.iter().enumerate() {
            self.draw(r, i, &format!("{indent}  "), true);
        }
    }

    /// Workflow g: its row (what it's for, when it started, its phases,
    /// how it stands); while it runs, or opened, each phase with its
    /// agents under it, a try that replaced earlier ones with those
    /// folded under it (workflowGroupLines).
    fn group(&mut self, g: &Group, runs: &HashMap<String, Row>) {
        let (w, now, tick) = (self.w, self.now, self.tick);
        let rf = g.r#ref();
        let open = g.live || self.open.contains(&rf);
        let end = if g.live || g.end == 0 { now } else { g.end };
        let mut right = match () {
            _ if g.live => line([sp("running", ORANGE)]),
            _ if g.status == "failed" => line([sp("failed", RED)]),
            _ if g.status == "stopped" => line([dim("stopped")]),
            _ => line([dim("done")]),
        };
        let status_w = (w / 5).clamp(10, 20);
        right
            .spans
            .push(plain(blanks(status_w.saturating_sub(right.width()))));
        right.spans.push(plain("  "));
        let took = if g.start > 0 {
            dur(round_s(end - g.start))
        } else {
            String::new()
        };
        right.spans.push(dim(format!("{took:>6}")));
        right.spans.push(plain("  "));
        let mark = if g.live {
            sp(SPINNER[tick % SPINNER.len()], ORANGE)
        } else if open {
            faint("▾")
        } else {
            faint("▸")
        };
        // The title gives way first, then what follows it: when it
        // started always shows, to tell runs of one name apart.
        let at = started(g.start, now);
        let at = if at.is_empty() {
            Line::new()
        } else {
            line([faint(format!(" · {at}"))])
        };
        let tail = if g.live {
            workflow_strip(&g.phases, tick)
        } else {
            let n: usize = g.phases.iter().map(|p| p.current().len()).sum();
            if n > 0 {
                line([dim(plural(n, "agent"))])
            } else {
                Line::new()
            }
        };
        let room = w as isize - right.width() as isize - 9 - at.width() as isize;
        let title_w = (room - 3 - tail.width().min(18) as isize).clamp(10, 48) as usize;
        let title = rush_draw::truncate(&g.title, title_w, "…").into_owned();
        let mut left = line([
            plain("  "),
            mark,
            plain(" "),
            sp("⋔ ", QUEUE),
            Span::new(title, if g.live { bold(TEXT) } else { Style::fg(SUB) }),
        ]);
        join(&mut left, at);
        if !tail.spans.is_empty() {
            left.spans.push(plain("   "));
            join(&mut left, tail);
        }
        let keep = w.saturating_sub(right.width() + 2).max(20);
        let top = spread(rush_draw::truncate_line(&left, keep, "…"), right, w);
        self.out.push((
            if self.sel == rf {
                picked1(top, w, true)
            } else {
                fit(top, w)
            },
            rf.clone(),
        ));
        if !open {
            return;
        }
        // Its runs no phase lists: the tries an agent made before the one
        // its phase lists, once its progress is heard of; before, all of
        // them.
        let listed: HashSet<&str> = g
            .phases
            .iter()
            .flat_map(|p| &p.agents)
            .map(|a| a.id.as_str())
            .filter(|id| runs.contains_key(*id))
            .collect();
        let mut left_over: Vec<&Row> = self
            .r
            .subs
            .iter()
            .rev()
            .filter(|sa| sa.workflow == g.run && !listed.contains(sa.id.as_str()))
            .filter_map(|sa| runs.get(&sa.id))
            .collect();
        let label_w = g
            .phases
            .iter()
            .flat_map(|p| &p.agents)
            .map(|a| width(&a.label).min(28))
            .fold(8, usize::max);
        for p in &g.phases {
            let title = if p.title.is_empty() {
                "agents"
            } else {
                &p.title
            };
            let agents = p.current();
            let mut head = line([
                plain("    "),
                phase_mark(p, tick),
                plain(" "),
                Span::new(title.to_string(), bold(SUB)),
            ]);
            let done = agents.iter().filter(|a| a.state == "done").count();
            head.spans.push(if agents.is_empty() {
                faint("  not started")
            } else {
                dim(format!("  {done} of {} done", agents.len()))
            });
            self.out.push((fit(head, w), String::new()));
            for (i, a) in agents.into_iter().enumerate() {
                // Its earlier tries: those its phase lists before it, and
                // the runs of its label no phase lists.
                let mut mine: Vec<&Row> = p
                    .earlier(a)
                    .iter()
                    .filter_map(|b| runs.get(&b.id))
                    .collect();
                let (same, rest): (Vec<&Row>, Vec<&Row>) = left_over.into_iter().partition(|t| {
                    !a.label.is_empty()
                        && (t.sa.description == a.label || sub_task(t.sa) == a.label)
                });
                left_over = rest;
                mine.extend(same);
                if let Some(r) = runs.get(&a.id).filter(|_| !a.id.is_empty()) {
                    self.draw(r, i, "    ", false);
                } else {
                    // Not started, or its run's files aren't there yet.
                    let row = agent_row(a, label_w, tick + i, w, now);
                    self.out.push((fit(cut_lead(row, 2), w), String::new()));
                }
                self.tries(format!("{TRIES_REF}{}", a.id), &mine, "    ");
            }
        }
        if listed.is_empty() {
            for (i, r) in left_over.iter().enumerate() {
                self.draw(r, i, "    ", false);
            }
        } else {
            self.tries(format!("{TRIES_REF}wf:{}", g.run), &left_over, "  ");
        }
    }
}

/// row without its first n cells of blank (TrimPrefix of spaces).
fn cut_lead(mut l: Line, mut n: usize) -> Line {
    for s in &mut l.spans {
        if n == 0 {
            break;
        }
        let blank = s.text.len() - s.text.trim_start_matches(' ').len();
        let k = blank.min(n);
        s.text.drain(..k);
        n -= k;
        if !s.text.is_empty() {
            break;
        }
    }
    l
}

/// A section's heading: its name, how many are in it, and a gap above
/// the rows.
fn heading(title: &str, n: usize, w: usize) -> (Line, String) {
    (
        fit(
            line([
                plain("  "),
                Span::new(title.to_string(), bold(SUB)),
                faint(format!("  {n}")),
            ]),
            w,
        ),
        String::new(),
    )
}

/// The runs, a row each: a count, then those running and those finished
/// under their headings, workflows first, newest first; a finished
/// workflow folded to its row. wide when the picked run shows beside the
/// list (subagentList). Each row with its ref.
pub fn list(m: &mut Model, key: &str, w: usize, wide: bool) -> Vec<(Line, String)> {
    let home = home_kind(m, key);
    let (now, tick) = (m.now, m.tick);
    let sel = sel(m);
    let reg = m.reg.clone();
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let s = &v.sess;
    let c = &*kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default();
    let r = &c.runs;
    let jobs: HashMap<String, &Job> = s.subagent_jobs();
    let mut running = 0;
    let mut rows: Vec<Row> = Vec::new();
    let mut by_id: HashMap<String, Row> = HashMap::new();
    for sa in r.subs.iter().rev() {
        let (status, live) = r.state(s, sa, &jobs, now);
        let row = Row {
            sa,
            st: r.stats.get(&sa.id),
            status,
            live,
        };
        if sa.workflow.is_empty() {
            if live {
                running += 1;
            }
            rows.push(row);
        } else {
            by_id.insert(sa.id.clone(), row);
        }
    }
    let live_of = |sa: &Subagent| r.state(s, sa, &jobs, now).1;
    let gs = groups(r, s, &live_of);
    let flows_live = gs.iter().filter(|g| g.live).count();
    let mut head = line([plain("  ")]);
    if running + flows_live > 0 {
        let mut said = Vec::new();
        if running > 0 {
            said.push(plural(running, "agent"));
        }
        if flows_live > 0 {
            said.push(plural(flows_live, "workflow"));
        }
        head.spans
            .push(sp(format!("{} running", said.join(" and ")), ORANGE));
        head.spans.push(dim(" · "));
    }
    head.spans.push(dim(format!(
        "{} in all",
        plural(rows.len() + gs.len(), "run")
    )));
    head.spans.push(faint(if wide {
        "  ·  click one to see it here"
    } else {
        "  ·  click one for more · enter opens it"
    }));
    let mut l = List {
        r,
        reg: &reg,
        home: &home,
        sel: &sel,
        open: &c.watch.open_folds,
        wide,
        tick,
        now,
        w,
        out: vec![(fit(head, w), String::new())],
    };
    rows.sort_by_key(|r| !r.live);
    for (title, live) in [("Running", true), ("Finished", false)] {
        let n = gs.iter().filter(|g| g.live == live).count()
            + rows.iter().filter(|r| r.live == live).count();
        if n == 0 {
            continue;
        }
        l.out.push((Line::new(), String::new()));
        l.out.push(heading(title, n, w));
        // Workflows and runs together, newest first.
        let mut items: Vec<(Ms, Option<&Group>, usize)> = gs
            .iter()
            .filter(|g| g.live == live)
            .map(|g| (g.start, Some(g), 0))
            .collect();
        for (i, r) in rows.iter().enumerate() {
            if r.live == live {
                let at = if r.sa.born > 0 {
                    r.sa.born
                } else {
                    r.st.map_or(0, |t| t.first)
                };
                items.push((at, None, i));
            }
        }
        items.sort_by_key(|it| std::cmp::Reverse(it.0));
        for (_, g, i) in items {
            match g {
                Some(g) => l.group(g, &by_id),
                None => l.draw(&rows[i], i, "", false),
            }
        }
    }
    l.out
}

/// What the banner pinned under the header says while a run is watched:
/// where esc goes back to, which run this is and how it stands; then,
/// when the session ran others, a line of them all, each a click away,
/// the one watched lit (subBanner).
pub fn banner(m: &mut Model, key: &str, w: usize) -> Vec<Line> {
    conn(m, key).watch.strip.clear();
    if !watching(m, key) {
        return Vec::new();
    }
    let home = home_kind(m, key);
    let (now, tick) = (m.now, m.tick);
    let hover = conn(m, key).vs.hover_ref.clone();
    let reg = m.reg.clone();
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let s = &v.sess;
    let c = kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default();
    let (r, watch) = (&c.runs, &mut c.watch);
    let Some(d) = r.detail.as_ref() else {
        return Vec::new();
    };
    let t = &d.view.sess;
    let jobs = s.subagent_jobs();
    let cur = r
        .subs
        .iter()
        .find(|sa| sa.id == watch.open)
        .cloned()
        .unwrap_or_default();
    let (status, live) = r.state(s, &cur, &jobs, now);
    let mut state = line([
        sub_mark(&reg, &kind_of(r, &home, &cur.id), &status, live, tick),
        plain(" "),
        sub_word(&status, live),
    ]);
    let end = if live { now } else { t.last };
    if t.first > 0 && end > 0 {
        state
            .spans
            .push(dim(format!(" {}", dur(round_s(end - t.first)))));
    }
    let steps = t.tool_calls();
    if steps > 0 {
        state
            .spans
            .push(dim(format!(" · {}", plural(steps, "step"))));
    }
    let effort = t
        .turns
        .iter()
        .rev()
        .find(|t| !t.effort.is_empty())
        .map(|t| t.effort.to_string())
        .unwrap_or_default();
    if !effort.is_empty() {
        state.spans.push(dim(format!(" · {effort} effort")));
    }
    let back = if watch.back {
        "‹ conversation"
    } else {
        "‹ subagents"
    };
    let crumb = if hover == BACK_REF {
        line([sp("▍", BLUE), Span::new(back, bold(BRIGHT))])
    } else {
        line([sp("▍", BLUE), sp(back, TEXT)])
    };
    let room = (w as isize - crumb.width() as isize - state.width() as isize - 5).max(8) as usize;
    let tag = sub_tag(
        &cur,
        r.stats.get(&cur.id),
        &kind_of(r, &home, &cur.id),
        &home,
    );
    let mut left = crumb;
    left.spans.push(plain("   "));
    join(&mut left, bold_line(sub_name(&cur, &tag, room, BRIGHT)));
    state.spans.push(plain(" "));
    let top = on_ground(spread(left, state, w), w, SUB_BG);
    if r.subs.len() < 2 {
        return vec![top];
    }
    // The runs in the order they started, as many as fit around the one
    // watched; each a click target.
    let chip_w = ((w as isize - 10) / r.subs.len() as isize - 5).clamp(22, 40) as usize;
    let mut all: Vec<(Line, String, usize)> = Vec::new();
    let mut at = 0;
    for sa in &r.subs {
        let (st, lv) = r.state(s, sa, &jobs, now);
        let kind = kind_of(r, &home, &sa.id);
        let mark = sub_mark(&reg, &kind, &st, lv, tick);
        let tag = sub_tag(sa, r.stats.get(&sa.id), &kind, &home);
        let chip = if sa.id == watch.open {
            at = all.len();
            let mut c = line([sp("▍", ORANGE), mark, plain(" ")]);
            join(&mut c, bold_line(sub_name(sa, &tag, chip_w + 6, BRIGHT)));
            c.spans.push(plain(" "));
            c
        } else {
            let mut c = line([plain(" "), mark, plain(" ")]);
            join(&mut c, sub_name(sa, &tag, chip_w, SUB));
            c.spans.push(plain(" "));
            c
        };
        let cw = chip.width() + 1;
        all.push((chip, sa.id.clone(), cw));
    }
    // The keys go first when the runs don't all fit beside them.
    let mut hint = keys_fit(w, &[("shift+↑↓", "next run"), ("esc", "back")]);
    let total: usize = all.iter().map(|c| c.2).sum();
    if total as isize > w as isize - hint.width() as isize - 3 {
        hint = Line::new();
    }
    let avail = w as isize - hint.width() as isize - 3;
    let (mut lo, mut hi, mut used) = (at, at, all[at].2 as isize);
    let mut grew = true;
    while grew {
        grew = false;
        if hi + 1 < all.len() && used + all[hi + 1].2 as isize <= avail {
            hi += 1;
            used += all[hi].2 as isize;
            grew = true;
        }
        if lo > 0 && used + all[lo - 1].2 as isize <= avail {
            lo -= 1;
            used += all[lo].2 as isize;
            grew = true;
        }
    }
    let mut x = 1;
    let mut chips = if lo > 0 {
        line([dim("‹")])
    } else {
        line([plain(" ")])
    };
    for (chip, id, cw) in &all[lo..=hi] {
        join(&mut chips, chip.clone());
        chips.spans.push(plain(" "));
        watch.strip.push((x, x + cw - 1, id.clone()));
        x += cw;
    }
    if hi < all.len() - 1 {
        chips.spans.push(dim("›"));
    }
    let mut right = Line::new();
    if !hint.spans.is_empty() {
        right = hint;
        right.spans.push(plain(" "));
    }
    vec![top, spread(chips, right, w)]
}

/// l with every span bold (cBright+bold, cText+bold).
fn bold_line(mut l: Line) -> Line {
    for s in &mut l.spans {
        if s.style.fg == BRIGHT || s.style.fg == TEXT {
            s.style = s.style.bold();
        }
    }
    l
}

/// The subagents view's body, rows tall: the opened run's conversation
/// under its banner, else None for the list, which views.rs draws.
pub fn body(m: &mut Model, key: &str, w: usize, rows: usize) -> Option<Vec<Line>> {
    conn(m, key).watch.pane_w = w;
    if !watching(m, key) {
        return None;
    }
    let head = banner(m, key, w);
    let mut refs: Vec<String> = Vec::new();
    match head.len() {
        2 => refs.extend([BACK_REF.to_string(), STRIP_REF.to_string()]),
        1 => refs.push(BACK_REF.to_string()),
        _ => {}
    }
    let mut out = head;
    out.truncate(rows);
    let (now, tick) = (m.now, m.tick);
    let n = rows.saturating_sub(out.len() + 1);
    let r = &mut conn(m, key).runs;
    let Some(d) = r.detail.as_mut() else {
        return Some(out);
    };
    let v = &mut d.view;
    if out.len() < rows {
        out.push(Line::new());
        refs.push(String::new());
    }
    v.sync(w, now, tick);
    if !v.read {
        out.push(fit(line([sp("reading its conversation…", DIM)]), w));
        refs.push(String::new());
    } else if !v.err.is_empty() && v.sess.turns.is_empty() {
        out.push(fit(
            line([sp(format!("couldn't read it: {}", v.err), RED)]),
            w,
        ));
        refs.push(String::new());
    } else if n > 0 {
        let k = if v.pos != Pos::End && n > 1 { n - 1 } else { n };
        let mut b = v.body(k);
        refs.extend(v.shown.iter().cloned());
        if v.pos != Pos::End {
            let total = v.total();
            let below = total.saturating_sub(v.start + b.len());
            while b.len() < k {
                b.push(Line::new());
                refs.push(String::new());
            }
            let pill = line([
                Span::new(" ", Style::default().on(SEL_BG)),
                Span::new(
                    format!("↓ {below} more · end follows"),
                    Style::fg(TEXT).on(SEL_BG),
                ),
                Span::new(" ", Style::default().on(SEL_BG)),
            ]);
            b.push(spread(Line::new(), pill, w));
            refs.push(String::new());
        }
        out.extend(b);
    }
    out.truncate(rows);
    refs.truncate(out.len());
    let hover = conn(m, key).vs.hover_ref.clone();
    if !hover.is_empty() {
        for (l, r) in out.iter_mut().zip(&refs).skip(2) {
            if *r == hover {
                *l = hover_line(std::mem::take(l), w);
            }
        }
    }
    let c = conn(m, key);
    c.vs.refs = refs.clone();
    c.vs.all_refs = refs.clone();
    if let Some(v) = m.session.as_mut() {
        v.shown = refs;
    }
    Some(out)
}

/// The run whose conversation shows beside the wide list: the one picked,
/// and only once it's picked; the pointer passing over the list moves
/// nothing (subPreviewLines' id).
fn peek_id(m: &Model) -> Option<String> {
    sel(m).strip_prefix("sub:").map(str::to_string)
}

/// The column's top rows for run id: how it stands, then its facts.
fn peek_head(m: &mut Model, key: &str, id: &str, w: usize) -> Vec<Line> {
    let home = home_kind(m, key);
    let (now, tick) = (m.now, m.tick);
    let reg = m.reg.clone();
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let s = &v.sess;
    let Some(r) = runs_of(m, key) else {
        return Vec::new();
    };
    let Some(sa) = r.subs.iter().find(|sa| sa.id == id) else {
        return Vec::new();
    };
    let (status, live) = r.state(s, sa, &s.subagent_jobs(), now);
    let st = r.stats.get(id);
    let kind = kind_of(r, &home, id);
    let mut right = line([sub_word(&status, live)]);
    let took = took_of(st, live, now);
    if !took.is_empty() {
        right.spans.push(dim(format!("  {took}")));
    }
    let name =
        rush_draw::truncate(&sub_task(sa), w.saturating_sub(right.width() + 6), "…").into_owned();
    let top = spread(
        line([
            plain(" "),
            sub_mark(&reg, &kind, &status, live, tick),
            plain(" "),
            Span::new(name, bold(BRIGHT)),
        ]),
        right,
        w,
    );
    let facts = facts_of(r, sa, &home, w);
    let mut out = vec![top];
    out.push(rush_draw::truncate_line(
        &line([plain("   "), dim(facts.join(" · "))]),
        w,
        "…",
    ));
    if live && let Some(d) = st.map(|t| one_line(&t.doing)).filter(|d| !d.is_empty()) {
        out.push(rush_draw::truncate_line(
            &line([plain("   "), sp("› ", ORANGE), sp(d, SUB)]),
            w,
            "…",
        ));
    }
    out
}

/// The column beside the list for a workflow picked: when it started,
/// how it stands, each phase and how far it got.
fn group_lines(m: &mut Model, key: &str, rf: &str, w: usize) -> Vec<Line> {
    let (now, tick) = (m.now, m.tick);
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let s = &v.sess;
    let Some(r) = runs_of(m, key) else {
        return Vec::new();
    };
    let jobs = s.subagent_jobs();
    let live_of = |sa: &Subagent| r.state(s, sa, &jobs, now).1;
    let Some(g) = groups(r, s, &live_of).into_iter().find(|g| g.r#ref() == rf) else {
        return Vec::new();
    };
    let mut out = vec![line([
        plain(" "),
        sp("⋔ ", QUEUE),
        Span::new(
            rush_draw::truncate(&g.title, w.saturating_sub(4), "…").into_owned(),
            bold(BRIGHT),
        ),
    ])];
    let mut facts = Vec::new();
    let at = started(g.start, now);
    if !at.is_empty() {
        facts.push(format!("started {at}"));
    }
    facts.push(if g.live {
        "running".to_string()
    } else if g.status.is_empty() {
        "done".to_string()
    } else {
        g.status.clone()
    });
    if g.tool_uses > 0 {
        facts.push(plural(g.tool_uses as usize, "step"));
    }
    if g.tokens > 0 {
        facts.push(format!("{} tokens", tokens(g.tokens)));
    }
    out.push(line([plain("   "), dim(facts.join(" · "))]));
    out.push(Line::new());
    for p in &g.phases {
        let agents = p.current();
        let done = agents.iter().filter(|a| a.state == "done").count();
        let title = if p.title.is_empty() {
            "agents"
        } else {
            &p.title
        };
        out.push(line([
            plain("   "),
            phase_mark(p, tick),
            plain(" "),
            sp(title.to_string(), TEXT),
            dim(if agents.is_empty() {
                "  not started".to_string()
            } else {
                format!("  {done} of {} done", agents.len())
            }),
        ]));
    }
    out.push(Line::new());
    out.push(line([faint("   click one of its agents to see it here")]));
    out
}

/// The box under the run beside the list: where a message to it goes,
/// typed into it once it's clicked or → gives it the keys.
fn msg_box(m: &mut Model, key: &str, id: &str, w: usize) -> Vec<Line> {
    let rt = super::relay::route(m, key, id);
    let name = runs_of(m, key)
        .and_then(|r| r.subs.iter().find(|sa| sa.id == id))
        .map(sub_task)
        .unwrap_or_default();
    let hint = match &rt {
        Ok(rt) => rt.hint(m),
        Err(e) => e.clone(),
    };
    let wt = &conn(m, key).watch;
    let focused = wt.msg_focus && rt.is_ok();
    let b = crate::frame::inputbox::InputBox {
        w,
        focused,
        top_l: line([dim(format!(
            "to {}",
            rush_draw::truncate(&name, w.saturating_sub(16).max(8), "…")
        ))]),
        foot_l: if rt.is_ok() {
            line([faint(
                rush_draw::truncate(&hint, w.saturating_sub(6), "…").into_owned(),
            )])
        } else {
            Line::new()
        },
        text: wt.msg.clone(),
        cursor: wt.msg_pos,
        holder: match (&rt, focused) {
            (Err(_), _) => rush_draw::truncate(&hint, w.saturating_sub(8), "…").into_owned(),
            (Ok(_), true) => "a message for it · enter sends · esc leaves the box".into(),
            (Ok(_), false) => "click here or press → to message it".into(),
        },
        lead: line([sp("❯ ", if focused { ORANGE } else { FAINT })]),
        max_rows: 3,
        idle: !focused,
        ..Default::default()
    };
    b.lines()
}

/// Lays the picked run beside the wide list, over the rows after the
/// view's first top: how it stands and its facts, its conversation's
/// latest, and a box to message it; a workflow picked, its phases;
/// nothing picked, how to pick one (drawSubPreview, subPreviewLines).
pub fn beside(m: &mut Model, key: &str, out: &mut [Line], w: usize, screen_top: usize) {
    conn(m, key).watch.peek.visible = false;
    conn(m, key).watch.msg_at = [0; 4];
    let any = has_runs(m, key);
    if !wide(&conn(m, key).watch.open, w, any) {
        return;
    }
    let lw = list_width(w);
    let pw = w.saturating_sub(lw + 3);
    let h = out.len();
    let mut right: Vec<Line> = vec![Line::new(); h];
    let picked = sel(m);
    match peek_id(m) {
        Some(id) => peek(m, key, &id, &mut right, pw, screen_top, lw),
        None => {
            let rows = if is_workflow_ref(m, &picked) {
                group_lines(m, key, &picked, pw)
            } else {
                vec![
                    Line::new(),
                    line([faint(" Click an agent to see it here: how it stands,")]),
                    line([faint(" what it's doing, and a box to send it a message.")]),
                    line([faint(" Enter opens it across the whole view.")]),
                ]
            };
            for (i, l) in rows.into_iter().enumerate().take(h) {
                right[i] = l;
            }
        }
    }
    for (i, l) in out.iter_mut().enumerate() {
        let mut row = fit(std::mem::take(l), lw);
        row.spans.push(plain(" "));
        row.spans.push(faint("│"));
        row.spans.push(plain(" "));
        join(&mut row, fit(std::mem::take(&mut right[i]), pw));
        *l = row;
    }
}

/// Run id in the column, pw wide: its head, its conversation's latest,
/// its box.
#[allow(clippy::too_many_arguments)]
fn peek(
    m: &mut Model,
    key: &str,
    id: &str,
    right: &mut [Line],
    pw: usize,
    screen_top: usize,
    lw: usize,
) {
    let h = right.len();
    if let Some(src) = src_of(m, key, id) {
        show_detail(m, key, id, src);
    }
    let head = peek_head(m, key, id, pw);
    let bx = msg_box(m, key, id, pw);
    let (now, tick) = (m.now, m.tick);
    let c = conn(m, key);
    if c.watch.peek.id != id {
        c.watch.peek.id = id.to_string();
        c.watch.peek.scroll = 0;
        c.watch.peek.total = 0;
        c.watch.msg_focus = false;
    }
    let mut at = 0;
    for l in head {
        if at < h {
            right[at] = l;
            at += 1;
        }
    }
    if at < h {
        right[at] = line([faint("─".repeat(pw))]);
        at += 1;
    }
    // The box and its foot take the column's last rows.
    let box_h = bx.len().min(h.saturating_sub(at));
    let body_end = h - box_h;
    let rows = body_end.saturating_sub(at + 1);
    let p = &mut c.watch.peek;
    p.x = 2 + lw + 3;
    p.y = screen_top + at;
    p.w = pw;
    p.h = rows;
    p.visible = true;
    if let Some(d) = c.runs.detail.as_mut().filter(|d| d.id == id) {
        let v = &mut d.view;
        v.sync(pw, now, tick);
        v.o.selected.clear();
        v.o.focused = false;
        v.o.hide_activity = true;
        if !v.read {
            right[at] = line([dim(" reading its conversation…")]);
        } else if rows > 0 {
            let total = v.total();
            if p.scroll > 0 && p.width == pw {
                // Streaming adds below what's read: follow only at the end.
                p.scroll = (p.scroll + total).saturating_sub(p.total);
            }
            p.total = total;
            p.rows = rows;
            p.width = pw;
            p.scroll = p.scroll.min(total.saturating_sub(rows));
            let end = total - p.scroll;
            let start = end.saturating_sub(rows);
            v.pos = Pos::End;
            if p.scroll > 0 {
                v.scroll(p.scroll as isize, rows);
            }
            for (i, l) in v.body(rows).into_iter().enumerate() {
                right[at + i] = l;
            }
            if total > rows {
                let more = if p.scroll > 0 {
                    format!("↓ {} more · scroll here", p.scroll)
                } else {
                    "latest · scroll here for more".into()
                };
                right[body_end - 1] =
                    line([faint(format!(" {}–{end} of {total} · {more}", start + 1))]);
            }
        }
    } else {
        right[at] = line([faint(
            " its conversation isn't found yet; its step is in the main conversation",
        )]);
    }
    for (i, l) in bx.into_iter().enumerate() {
        if body_end + i < h {
            right[body_end + i] = l;
        }
    }
    c.watch.msg_at = [2 + lw + 3, screen_top + body_end, pw, box_h];
}

/// The keys of a watched run, before the box's: esc and ← back out of
/// it, shift or alt with the arrows go to the next run, and the arrows,
/// page keys, enter, space and → move through and open its
/// conversation's rows; what's typed goes to a spawned agent rush hosts
/// (paneKey :4162, :4335, :4359, :4399, :4452; sendPane's watchedHost).
/// shift or alt with ↑↓ step into the runs from the conversation too.
pub fn key_first(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let s = k.s.as_str();
    let empty = conn(m, &key).input.is_empty();
    if !watching(m, &key) {
        return Out::Pass;
    }
    let picked = conn(m, &key)
        .runs
        .detail
        .as_ref()
        .is_some_and(|d| !d.view.sel.is_empty());
    match s {
        "esc" | "left" if empty => {
            if picked {
                if let Some(d) = conn(m, &key).runs.detail.as_mut() {
                    d.view.sel.clear(); // the first drops the pick
                }
            } else {
                close_sub(m, &key);
            }
            Out::Draw
        }
        "shift+up" | "shift+down" | "alt+up" | "alt+down" if empty || s.starts_with("alt") => {
            cycle_sub(m, &key, if s.ends_with("up") { -1 } else { 1 });
            Out::Draw
        }
        "shift+left" | "shift+right" | "alt+left" | "alt+right" if empty => {
            cycle_sub(m, &key, if s.ends_with("right") { 1 } else { -1 });
            Out::Draw
        }
        "enter" | "ctrl+enter" | "ctrl+s" if !empty => {
            let Some((sub, id, kind)) = watched_host(m, &key) else {
                return super::relay::send(m, &key, s != "enter");
            };
            super::spawned::send(m, &key, &sub, &id, &kind, s != "enter")
        }
        "up" | "down" | "home" | "end" | "enter" | "space" | " " | "right" if empty => {
            detail_key(m, &key, s)
        }
        "pgup" | "pgdown" | "ctrl+o" => detail_key(m, &key, s),
        _ => Out::Pass,
    }
}

/// enter, space or → on a run picked in the list or the dock opens or
/// watches it, on a workflow shows its agents; ↑↓ go through the dock's
/// runs between the conversation and the queue (paneKey :4255-4268,
/// moveSel :4932).
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let s = k.s.as_str();
    let empty = conn(m, &key).input.is_empty();
    let picked = sel(m);
    let name = super::views::view_name(m, &key);
    match s {
        // → goes into the column beside the list: its box takes the keys.
        "right"
            if empty
                && name == "subagents"
                && picked.starts_with("sub:")
                && box_run(m, &key).is_some_and(|id| picked[4..] == id) =>
        {
            focus_box(m, &key);
            Out::Draw
        }
        // → on a step a subagent made opens that run in the subagents
        // view; esc comes back to the step (rushmode.go:4270, subOfStep).
        "right"
            if name == "conversation"
                && picked.contains(":s:")
                && let Some(id) = run_of_step(m, &key, &picked) =>
        {
            super::views::show_view(m, &key, "subagents");
            open_sub(m, &key, &id);
            let w = &mut conn(m, &key).watch;
            if w.open != id {
                // Not found to read yet: the step opens in place instead.
                super::views::show_view(m, &key, "conversation");
                set_sel(m, picked);
                return Out::Pass;
            }
            w.back = true;
            w.from = picked;
            Out::Draw
        }
        "enter" | "space" | " " | "right" if !picked.is_empty() => {
            if name == "subagents" && is_fold(m, &key, &picked) {
                if s != "right" || !conn(m, &key).watch.open_folds.contains(&picked) {
                    toggle_fold(m, &key, &picked);
                }
                return Out::Draw;
            }
            if let Some(id) = picked.strip_prefix("sub:")
                && name == "subagents"
            {
                let id = id.to_string();
                open_sub(m, &key, &id);
                return Out::Draw;
            }
            // On a run in the dock: watch it.
            if let Some(id) = picked.strip_prefix("run:") {
                let id = id.to_string();
                watch_run(m, &key, &id);
                return Out::Draw;
            }
            if picked.starts_with("wf:") || is_workflow_ref(m, &picked) && name != "subagents" {
                open_workflow(m, &key, &picked);
                return Out::Draw;
            }
            Out::Pass
        }
        "up" | "down" if name == "conversation" && (empty || !picked.is_empty()) => {
            dock_nav(m, &key, s == "up", &picked)
        }
        // Through the conversation and its runs, in place (cycleSub); a
        // picked queued message has taken shift+↑↓ before this.
        "shift+up" | "shift+down" | "alt+up" | "alt+down" if empty || s.starts_with("alt") => {
            cycle_sub(m, &key, if s.ends_with("up") { -1 } else { 1 });
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// The run the conversation's step picked draws: a subagent's call, or
/// one of the agents a command ran ("step/run" when it ran several, each
/// a row of its own) (subOfStep).
fn run_of_step(m: &mut Model, key: &str, picked: &str) -> Option<String> {
    let (_, id) = picked.split_once(":s:")?;
    let id = id.to_string();
    let kids = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .and_then(|v| v.o.kids.get(id.split('/').next().unwrap_or("")))
        .map_or(0, Vec::len);
    let prefix = crate::screens::list_extras::spawns::SPAWN_PREFIX;
    conn(m, key)
        .runs
        .subs
        .iter()
        .find(|sa| {
            let run = sa.id.strip_prefix(prefix);
            match (id.split_once('/'), run) {
                (Some((step, kid)), Some(run)) => sa.tool_use_id == step && run == kid,
                _ => sa.tool_use_id == id && (run.is_none() || kids <= 1),
            }
        })
        .map(|sa| sa.id.clone())
}

/// A click on one of the dock's runs, in the conversation or the column
/// beside it: a workflow shows its agents, a run or task is picked
/// (clickRef :5102). Whether it was one.
pub fn click_ref(m: &mut Model, key: &str, r: &str) -> bool {
    if is_workflow_ref(m, r) {
        open_workflow(m, key, r);
        return true;
    }
    if r.starts_with("run:") || r.starts_with("job:") {
        set_sel(m, r.to_string());
        return true;
    }
    false
}

/// Whether ref picks a workflow: its task, or a run of one rush didn't
/// hear start (isWorkflowRef).
fn is_workflow_ref(m: &Model, r: &str) -> bool {
    m.session
        .as_ref()
        .is_some_and(|v| super::workflow::is_workflow_ref(&v.sess, r))
}

/// ↑↓ through the dock's runs, between the conversation and the queue
/// (moveSel over dockRefs). Pass leaves the rest to the conversation and
/// the queue.
fn dock_nav(m: &mut Model, key: &str, up: bool, picked: &str) -> Out {
    let refs = dock_refs(m, key);
    if refs.is_empty() {
        return Out::Pass;
    }
    let at = refs.iter().position(|r| r == picked);
    let queued = !super::queue::queue_of(m, key).items.is_empty();
    match (at, up) {
        // ↑ from the box, nothing queued: the dock's last (from the
        // queue's first, keys.rs's queue_nav).
        (None, true) if picked.is_empty() && !queued => set_sel(m, refs[refs.len() - 1].clone()),
        // ↓ past the conversation's last row: the dock's first.
        (None, false) if !picked.is_empty() && !picked.starts_with("q:") => {
            let rows = conn(m, key).vs.rows.max(1);
            let Some(v) = m.session.as_mut() else {
                return Out::Pass;
            };
            v.move_sel(1, rows);
            if v.sel.is_empty() {
                v.sel.clone_from(&refs[0]);
            }
            return Out::Draw;
        }
        (None, _) => return Out::Pass,
        (Some(0), true) => {
            // Past the dock's first: the conversation's last row in view.
            set_sel(m, String::new());
            let rows = conn(m, key).vs.rows.max(1);
            if let Some(v) = m.session.as_mut() {
                v.move_sel(-1, rows);
            }
        }
        (Some(i), true) => set_sel(m, refs[i - 1].clone()),
        (Some(i), false) if i + 1 < refs.len() => set_sel(m, refs[i + 1].clone()),
        (Some(_), false) => {
            // Past the dock's last: the queue's first, else the box.
            set_sel(m, if queued { "q:0".into() } else { String::new() });
        }
    }
    let Model { session, kept, .. } = m;
    if let Some(v) = session.as_mut() {
        let r = &mut kept
            .get::<super::Core>()
            .conns
            .entry(key.to_string())
            .or_default()
            .runs;
        let (run, flows) = (r.shown.dock.clone(), r.shown.flows.clone());
        v.sel = super::running::keep_run_pick(&v.sel, r, &run, &flows);
    }
    Out::Draw
}

/// A key in the watched run's conversation: its rows picked, opened and
/// scrolled as the conversation's are.
fn detail_key(m: &mut Model, key: &str, s: &str) -> Out {
    let rows = conn(m, key).vs.rows.max(1);
    let Some(d) = conn(m, key).runs.detail.as_mut() else {
        return Out::Pass;
    };
    let v = &mut d.view;
    match s {
        "up" => v.move_sel(-1, rows),
        "down" => {
            if v.sel.is_empty() {
                return Out::Idle;
            }
            v.move_sel(1, rows);
        }
        "pgup" => v.scroll(10, rows),
        "pgdown" => v.scroll(-10, rows),
        "home" => v.home(rows),
        "end" => {
            v.pos = Pos::End;
            v.sel.clear();
        }
        "ctrl+o" => v.verbose = !v.verbose,
        "enter" | "space" | " " | "right" => {
            if v.sel.is_empty() {
                return Out::Pass;
            }
            if s == "right" {
                v.o.open.insert(v.sel.clone(), true);
            } else {
                v.toggle(rows);
            }
        }
        _ => return Out::Pass,
    }
    Out::Draw
}

/// Opens or closes the fold rf picks: a run's earlier tries, or a
/// finished workflow run.
fn toggle_fold(m: &mut Model, key: &str, rf: &str) {
    let f = &mut conn(m, key).watch.open_folds;
    if !f.remove(rf) {
        f.insert(rf.to_string());
    }
}

/// Whether ref is a fold the list opens and closes in place: earlier
/// tries, or a workflow run that has finished.
fn is_fold(m: &Model, key: &str, rf: &str) -> bool {
    if rf.starts_with(TRIES_REF) {
        return true;
    }
    if !is_workflow_ref(m, rf) {
        return false;
    }
    let now = m.now;
    let (Some(v), Some(r)) = (m.session.as_ref(), runs_of(m, key)) else {
        return false;
    };
    let s = &v.sess;
    let jobs = s.subagent_jobs();
    let live_of = |sa: &Subagent| r.state(s, sa, &jobs, now).1;
    groups(r, s, &live_of)
        .iter()
        .any(|g| g.r#ref() == rf && !g.live)
}

/// What's typed in the box under the run beside the list, given back
/// when it couldn't be sent, ahead of anything typed since.
pub fn give_back(m: &mut Model, key: &str, text: &str) {
    let w = &mut conn(m, key).watch;
    let mut t: Vec<char> = text.chars().collect();
    if !w.msg.is_empty() {
        t.push('\n');
        t.extend(w.msg.iter());
    }
    w.msg_pos = t.len();
    w.msg = t;
}

/// The run the box beside the list is for, while it shows.
fn box_run(m: &Model, key: &str) -> Option<String> {
    let c = m.kept.peek::<super::Core>()?.conns.get(key)?;
    if !c.watch.peek.visible || c.watch.msg_at[2] == 0 {
        return None;
    }
    Some(c.watch.peek.id.clone())
}

/// Gives the box beside the list the keys, if a message can go from it.
fn focus_box(m: &mut Model, key: &str) -> bool {
    let Some(id) = box_run(m, key) else {
        return false;
    };
    if let Err(e) = super::relay::route(m, key, &id) {
        m.flash(&e);
        return true;
    }
    conn(m, key).watch.msg_focus = true;
    true
}

/// Keys while the box under the run beside the list has them: enter
/// sends (ctrl+enter at once), esc gives the keys back, the rest edit.
pub fn box_key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if !conn(m, &key).watch.msg_focus {
        return Out::Pass;
    }
    let Some(id) = box_run(m, &key).filter(|_| super::views::view_name(m, &key) == "subagents")
    else {
        conn(m, &key).watch.msg_focus = false;
        return Out::Pass;
    };
    let s = k.s.as_str();
    match s {
        "esc" => {
            conn(m, &key).watch.msg_focus = false;
            Out::Draw
        }
        // ← from an empty box goes back to the list.
        "left" if conn(m, &key).watch.msg.is_empty() => {
            conn(m, &key).watch.msg_focus = false;
            Out::Draw
        }
        "enter" | "ctrl+enter" | "ctrl+s" => {
            let w = &mut conn(m, &key).watch;
            let text: String = w.msg.iter().collect();
            let text = text.trim().to_string();
            if text.is_empty() {
                return Out::Idle;
            }
            w.msg.clear();
            w.msg_pos = 0;
            super::relay::send_to(m, &key, &id, text, s != "enter")
        }
        _ => {
            let w = &mut conn(m, &key).watch;
            match crate::frame::editor::edit(&w.msg, w.msg_pos, k) {
                Some((buf, pos)) => {
                    w.msg = buf;
                    w.msg_pos = pos;
                    Out::Draw
                }
                // Not an editing key: kept from the rest while it types.
                None if s.starts_with("ctrl+c") => Out::Pass,
                None => Out::Idle,
            }
        }
    }
}

/// A paste while the box beside the list has the keys goes into it.
pub fn paste(m: &mut Model, text: &str) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if !conn(m, &key).watch.msg_focus || box_run(m, &key).is_none() {
        return Out::Pass;
    }
    let t = crate::frame::editor::clean_paste(text);
    let w = &mut conn(m, &key).watch;
    let r: Vec<char> = t.chars().collect();
    w.msg = crate::frame::editor::insert(&w.msg, w.msg_pos, &r);
    w.msg_pos += r.len();
    Out::Draw
}

/// The pointer over the subagents view: the banner goes back or to
/// another run; in the list a click picks a run, which then shows in the
/// column beside it (never across the whole view: enter does that), and
/// opens or closes a fold; in the column, a click on the box gives it the
/// keys and the wheel scrolls the run's conversation (subHoverAt,
/// subMouseMove, stripClick, clickRef).
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some(top) = m.session.as_ref().map(|v| v.top) else {
        return Out::Pass;
    };
    let name = super::views::view_name(m, &key);
    if name != "subagents" {
        // The dock's runs on the transcript's end; another view's rows
        // are its own (the background view's: views.rs).
        if e.kind != MouseKind::Down(Button::Left) || name != "conversation" {
            return Out::Pass;
        }
        let r = m
            .session
            .as_ref()
            .and_then(|v| v.shown.get(e.y as usize - top.min(e.y as usize)).cloned())
            .filter(|_| e.y as usize >= top)
            .unwrap_or_default();
        if !r.is_empty() && click_ref(m, &key, &r) {
            return Out::Draw;
        }
        return Out::Pass;
    }
    let (x, y) = (e.x as usize, e.y as usize);
    let rows = conn(m, &key).vs.rows.max(1);
    let row = y.checked_sub(top).filter(|&i| i < rows);
    let rf = row
        .and_then(|i| conn(m, &key).vs.refs.get(i).cloned())
        .unwrap_or_default();
    let p = &conn(m, &key).watch.peek;
    let in_peek = p.visible && x >= p.x && x < p.x + p.w && y >= p.y && y < p.y + p.h;
    let b = conn(m, &key).watch.msg_at;
    let in_box = b[2] > 0 && x >= b[0] && x < b[0] + b[2] && y >= b[1] && y < b[1] + b[3];
    let pw = conn(m, &key).watch.pane_w;
    let watching = watching(m, &key);
    // Wide, the column beside the list is the run's, not the list's.
    let beside = !watching && pw >= WIDE_AT && x.saturating_sub(2) >= list_width(pw);
    if row.is_none() && !in_peek && !in_box {
        return Out::Pass;
    }
    match e.kind {
        MouseKind::Down(Button::Left) => {
            if in_box {
                if !focus_box(m, &key) {
                    return Out::Idle;
                }
                return Out::Draw;
            }
            if beside {
                // Elsewhere in the column: the keys go back to the list.
                let w = &mut conn(m, &key).watch;
                if w.msg_focus {
                    w.msg_focus = false;
                    return Out::Draw;
                }
                return Out::Idle;
            }
            if watching && rf == BACK_REF {
                close_sub(m, &key);
                return Out::Draw;
            }
            if watching && rf == STRIP_REF {
                let col = x.saturating_sub(2);
                let open = conn(m, &key).watch.open.clone();
                let hit = conn(m, &key)
                    .watch
                    .strip
                    .iter()
                    .find(|h| col >= h.0 && col < h.1 && h.2 != open)
                    .map(|h| h.2.clone());
                if let Some(id) = hit {
                    let (back, from) = {
                        let w = &conn(m, &key).watch;
                        (w.back, w.from.clone())
                    };
                    open_sub(m, &key, &id);
                    let w = &mut conn(m, &key).watch;
                    w.back = back;
                    w.from = from;
                    return Out::Draw;
                }
                return Out::Idle;
            }
            if watching {
                // A row of the run's conversation: picked, then opened.
                if rf.is_empty() {
                    return Out::Idle;
                }
                let Some(d) = conn(m, &key).runs.detail.as_mut() else {
                    return Out::Idle;
                };
                d.view.sel = rf;
                d.view.toggle(rows);
                return Out::Draw;
            }
            conn(m, &key).watch.msg_focus = false;
            if rf.is_empty() {
                return Out::Pass;
            }
            if let Some(id) = rf.strip_prefix(OPEN_REF) {
                let id = id.to_string();
                open_sub(m, &key, &id);
                return Out::Draw;
            }
            if is_fold(m, &key, &rf) {
                toggle_fold(m, &key, &rf);
            }
            set_sel(m, rf);
            Out::Draw
        }
        MouseKind::ScrollUp | MouseKind::ScrollDown if in_peek => {
            let up = e.kind == MouseKind::ScrollUp;
            let p = &mut conn(m, &key).watch.peek;
            let was = p.scroll;
            let most = p.total.saturating_sub(p.rows);
            p.scroll = if up {
                (p.scroll + 3).min(most)
            } else {
                p.scroll.saturating_sub(3)
            };
            if p.scroll == was {
                return Out::Idle;
            }
            Out::Draw
        }
        MouseKind::ScrollUp | MouseKind::ScrollDown if watching => {
            let up = e.kind == MouseKind::ScrollUp;
            let Some(d) = conn(m, &key).runs.detail.as_mut() else {
                return Out::Pass;
            };
            let v = &mut d.view;
            if up && v.start == 0 || !up && v.pos == Pos::End {
                return Out::Idle;
            }
            v.scroll(if up { 3 } else { -3 }, rows);
            Out::Draw
        }
        MouseKind::Moved => {
            // The pointer lights the list's row under it, and nothing
            // else: what's beside the list changes on a click only.
            let over = if in_peek || in_box || beside {
                String::new()
            } else if watching {
                if rf == BACK_REF { rf } else { String::new() }
            } else {
                rf
            };
            let now = m.now;
            let c = conn(m, &key);
            if c.vs.hover_ref == over {
                return Out::Pass;
            }
            c.vs.hover_ref = over;
            c.watch.hover_at = now;
            Out::Draw
        }
        _ => Out::Pass,
    }
}
