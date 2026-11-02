//! The subagents view: every run the session started, a line each, under
//! Running and Finished, workflows with their phases and agents, a finished
//! workflow folded to its row and named by when it started, an agent's
//! earlier tries folded under the try that replaced them. The run picked
//! shows with the list: its facts, its conversation (with its working line
//! while it works) and a box to message it; beside the list when there's
//! room, under a short list of the runs when there isn't. There is no view
//! of one run alone: everything that opens a run (a subagent's step, the
//! dock, a card's link) shows this view with it picked, and esc goes back
//! where it came from. Where the user asked, this parts from Go: one line
//! a run rather than two, the run picked never opened whole, the branch
//! from where the run worked. After Go's internal/ui/rushmode.go (views
//! :47, cycleSub :734, watchRun :809, subHoverAt :990, subagentLines
//! :1039, subagentList :1059, subRow.appendTo :1213, subEffort :1231,
//! tabStat's subagents :1469, paneKey's subagents keys :4255-4262, moveSel
//! :4932, dockRefs :4900, clickRef :5046, :5102-5122, subWhere :6299,
//! subWorktree :6308), workflow.go (workflowGroupLines :400,
//! openWorkflow :586), subpreview.go (wideSubagents :20, subListWidth
//! :24, subPreviewLines :33, drawSubPreview :94), spawns.go (spawnRunFor
//! :694, subKind :721) and model.go (the preview's click :1700 and wheel
//! :1760, subHoverMsg :1192).
//!
//! The picked run is read by subs.rs off the UI thread, as the Session's
//! runs are, into a SessionView of its own (Runs::detail), and drawn as a
//! conversation is; what its box sends is relay.rs's. Not here: what a
//! working run takes of the machine (subUsage: the process table). Wide,
//! the pointer right of the list lights no row (subHoverAt :1007-1011).

use std::collections::{HashMap, HashSet};

use rush_convo::{Job, Subagent};
use rush_core::text::{Line, Span, Style, width};
use rush_core::time::Ms;
use rush_draw::{Button, MouseKind};

use super::dock::{picked1, spread};
use super::paint::{dim, faint, plural, tokens};
use super::subname::{sub_mark, sub_tag, sub_task, sub_word};
use super::subs::{Detail, DetailSrc, RunStat, Runs};
use super::workflow::{Group, agent_row, groups, phase_mark, round_s, workflow_strip};
use super::{agent, conn, open_key};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::session::{Pos, SessionView, Source};
use crate::spawns::SPAWN_PREFIX;
use crate::style::*;

/// How the subagents view got its run picked (Go's hostConn: subBack,
/// subFrom, subFromView, subHoverAt, subPreview).
#[derive(Default)]
pub struct Watch {
    /// Picked from elsewhere (a step, the dock, the background view): esc
    /// goes back to that view, to the row it was picked from.
    pub back: bool,
    pub from: String,
    pub from_view: String,
    /// When the pointer came to rest on what it's over.
    pub hover_at: Ms,
    /// The picked run's conversation, beside the wide list or under the
    /// narrow one.
    pub peek: Peek,
    /// Narrow, the rows the list takes over the picked run; 0 when wide.
    pub split: usize,
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
/// (wideSubagents); narrower, the run goes under a short list.
pub fn wide(w: usize, any: bool) -> bool {
    w >= WIDE_AT && any
}

/// Narrow, the rows the list takes over the picked run, rows tall in all,
/// the list n rows long: a short list, scrolled, the rest the run's.
pub fn split_rows(n: usize, rows: usize) -> usize {
    let most = (rows * 2 / 5).max(6);
    if rows < most + 8 {
        return rows; // too short for the run under it
    }
    n.min(most)
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

/// Where run id's conversation is read from; None for one known only by
/// its task (spawnRunFor, openSub).
pub fn src_of(m: &Model, key: &str, id: &str) -> Option<DetailSrc> {
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
    let seed = super::subs::seed_detail(m, key, id, &src);
    let mut view = SessionView::new(key.to_string(), Source::Hosted, 0);
    if let Some(s) = seed {
        view.seed(s);
    }
    conn(m, key).runs.detail = Some(Detail {
        id: id.to_string(),
        src,
        view,
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

/// Shows the subagents view with run id picked: beside the list when
/// there's room, under a short list when not (the one place a run shows;
/// Go's openSub opened it whole).
pub fn pick_run(m: &mut Model, key: &str, id: &str) {
    super::views::show_view(m, key, "subagents");
    let w = &mut conn(m, key).watch;
    w.back = false;
    w.from.clear();
    w.from_view.clear();
    w.msg_focus = false;
    set_sel(m, format!("sub:{id}"));
}

/// Shows run id picked in the subagents view from elsewhere, the row from
/// picked in the view showing (a subagent's step, the dock's run, the
/// background view's): esc goes back there (watchRun, rushmode.go:777).
pub fn pick_from(m: &mut Model, key: &str, id: &str, from: &str) {
    let view = super::views::view_name(m, key).to_string();
    pick_run(m, key, id);
    let w = &mut conn(m, key).watch;
    w.back = true;
    w.from = from.to_string();
    w.from_view = view;
}

/// Back to where the run picked was picked from, when it was picked from
/// elsewhere (closeSub): whether it was.
pub fn go_back(m: &mut Model, key: &str) -> bool {
    let w = &mut conn(m, key).watch;
    if !w.back {
        return false;
    }
    w.back = false;
    w.msg_focus = false;
    let from = std::mem::take(&mut w.from);
    let view = std::mem::take(&mut w.from_view);
    conn(m, key).runs.detail = None;
    if view.is_empty() || view == "conversation" {
        conn(m, key).vs.view = 0;
    } else {
        super::views::show_view(m, key, &view);
    }
    set_sel(m, from);
    true
}

/// Steps through the conversation and its runs, running ones first, then
/// the rest, newest first, each picked in the subagents view; the
/// conversation sits at either end (cycleSub).
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
        let picked = sel(m);
        if let Some(i) = order
            .iter()
            .position(|id| picked.strip_prefix("sub:") == Some(id.as_str()))
        {
            cur = i as isize + 1;
        }
    }
    let next = ((cur + dir) % n + n) % n;
    if next == 0 {
        conn(m, key).runs.detail = None;
        conn(m, key).watch.back = false;
        conn(m, key).vs.view = 0;
        set_sel(m, String::new());
        return;
    }
    // A sibling is still picked from where this one was.
    let (back, from, view) = {
        let w = &conn(m, key).watch;
        (w.back, w.from.clone(), w.from_view.clone())
    };
    pick_run(m, key, &order[next as usize - 1]);
    let w = &mut conn(m, key).watch;
    w.back = back;
    w.from = from;
    w.from_view = view;
}

/// Shows the subagents view on the workflow ref picks: its phases and
/// its agents' runs (openWorkflow).
pub fn open_workflow(m: &mut Model, key: &str, r: &str) {
    super::views::show_view(m, key, "subagents");
    let w = &mut conn(m, key).watch;
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
    tick: usize,
    now: Ms,
    w: usize,
    out: Vec<(Line, String)>,
}

/// Picks a row: its ref.
pub const TRIES_REF: &str = "tries:";

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
    let c = rush_ui_settings::paint::civil(at);
    let hm = format!("{:02}:{:02}", c.hour, c.minute);
    if now - at > 20 * 3_600_000 {
        return format!("{} {hm}", c.weekday_name());
    }
    hm
}

/// What a run is doing or how it ended, short, w wide: while it works,
/// the session's working line with its step under way (the spinner at
/// tick); waiting on an answer, that; else done, failed or stopped.
fn short_status(r: &Row, w: usize, tick: usize) -> Line {
    if r.live && r.st.is_some_and(|t| t.waiting) {
        let s = rush_draw::truncate("waiting on you", w, "…").into_owned();
        return line([sp(s, YELLOW)]);
    }
    if r.live {
        let doing = r.st.map(|t| one_line(&t.doing)).unwrap_or_default();
        let verb = if doing.is_empty() {
            "working"
        } else {
            doing.as_str()
        };
        let head = rush_convo::render::live_head(tick, verb, None);
        return rush_draw::truncate_line(&head, w, "…");
    }
    line([sub_word(&r.status, false)])
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
    /// how long it went.
    fn draw(&mut self, r: &Row, i: usize, indent: &str, quiet: bool) {
        let (sa, w) = (r.sa, self.w);
        let rf = format!("sub:{}", sa.id);
        let took = took_of(r.st, r.live, self.now);
        let kind = kind_of(self.r, self.home, &sa.id);
        let mark = sub_mark(self.reg, &kind, &r.status, r.live, self.tick + i);
        let status_w = (w / 5).clamp(10, 20);
        let status = short_status(r, status_w, self.tick);
        let pad = status_w.saturating_sub(status.width());
        let mut right = status;
        right.spans.extend([plain(blanks(pad)), plain("  ")]);
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
        // Picked, its facts and conversation show with the list.
        self.out.push((picked1(top, w, true), rf));
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
/// list, else under it (subagentList). Each row with its ref.
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
        "  ·  click one to see it below"
    }));
    let mut l = List {
        r,
        reg: &reg,
        home: &home,
        sel: &sel,
        open: &c.watch.open_folds,
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
    out
}

/// Whether run id works now, as the list says: its conversation's working
/// line shows under it only then, and goes once it's finished.
pub fn run_live(m: &Model, key: &str, id: &str) -> bool {
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return false;
    };
    let s = &v.sess;
    let Some(r) = runs_of(m, key) else {
        return false;
    };
    let jobs = s.subagent_jobs();
    r.subs
        .iter()
        .find(|sa| sa.id == id)
        .is_some_and(|sa| r.state(s, sa, &jobs, m.now).1)
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
    if !wide(w, any) {
        return;
    }
    let lw = list_width(w);
    let pw = w.saturating_sub(lw + 3);
    let h = out.len();
    let mut right: Vec<Line> = vec![Line::new(); h];
    column(m, key, &mut right, pw, (2 + lw + 3, screen_top));
    for (i, l) in out.iter_mut().enumerate() {
        let mut row = fit(std::mem::take(l), lw);
        row.spans.push(plain(" "));
        row.spans.push(faint("│"));
        row.spans.push(plain(" "));
        join(&mut row, fit(std::mem::take(&mut right[i]), pw));
        *l = row;
    }
}

/// Narrow, the picked run under the list's first split rows, out rows
/// tall in all: a rule, then what the wide view shows beside the list.
pub fn below(
    m: &mut Model,
    key: &str,
    out: &mut [Line],
    w: usize,
    split: usize,
    screen_top: usize,
) {
    conn(m, key).watch.peek.visible = false;
    conn(m, key).watch.msg_at = [0; 4];
    let h = out.len();
    if split + 1 >= h {
        return;
    }
    out[split] = line([faint("─".repeat(w))]);
    let mut under: Vec<Line> = vec![Line::new(); h - split - 1];
    column(m, key, &mut under, w, (2, screen_top + split + 1));
    for (i, l) in under.into_iter().enumerate() {
        out[split + 1 + i] = fit(l, w);
    }
}

/// What shows with the list, into rows pw wide whose first cell is at
/// (x, y) on the screen: the run picked, a workflow's phases, or how to
/// pick one.
fn column(m: &mut Model, key: &str, rows: &mut [Line], pw: usize, at: (usize, usize)) {
    let picked = sel(m);
    if let Some(id) = peek_id(m) {
        peek(m, key, &id, rows, pw, at);
        return;
    }
    let lines = if is_workflow_ref(m, &picked) {
        group_lines(m, key, &picked, pw)
    } else {
        vec![
            Line::new(),
            line([faint(" Click an agent to see it here: how it stands,")]),
            line([faint(" what it's doing, and a box to send it a message.")]),
        ]
    };
    for (i, l) in lines.into_iter().enumerate().take(rows.len()) {
        rows[i] = l;
    }
}

/// Run id in rows pw wide, the first at (x, y) on the screen: its head,
/// its conversation's latest, what became of what was sent it, its box.
fn peek(m: &mut Model, key: &str, id: &str, right: &mut [Line], pw: usize, (x, y): (usize, usize)) {
    let h = right.len();
    if let Some(src) = src_of(m, key, id) {
        show_detail(m, key, id, src);
    }
    // Followed as it grows while it runs and shows (subfollow).
    super::subfollow::sync(m, key);
    let head = peek_head(m, key, id, pw);
    let mut bx = super::relay::note_lines(m, key, id, pw);
    bx.extend(msg_box(m, key, id, pw));
    let live = run_live(m, key, id);
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
    p.x = x;
    p.y = y + at;
    p.w = pw;
    p.h = rows;
    p.visible = true;
    if let Some(d) = c.runs.detail.as_mut().filter(|d| d.id == id) {
        let v = &mut d.view;
        v.sync(pw, now, tick);
        v.o.selected.clear();
        v.o.focused = false;
        // Its working line under its conversation, as the session's, while
        // it works; finished or waiting, how it stands says instead.
        v.o.hide_activity = !live;
        if !v.read {
            if at < h {
                right[at] = line([dim(" reading its conversation…")]);
            }
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
    } else if at < h {
        right[at] = line([faint(
            " its conversation isn't found yet; its step is in the main conversation",
        )]);
    }
    for (i, l) in bx.into_iter().enumerate() {
        if body_end + i < h {
            right[body_end + i] = l;
        }
    }
    c.watch.msg_at = [x, y + body_end, pw, box_h];
}

/// The subagents view's keys before the views': esc goes back where the
/// run picked was picked from (closeSub); tab gives the picked run's box
/// the keys, as → does, and tab in the box gives them back.
pub fn key_first(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if super::views::view_name(m, &key) != "subagents" {
        return Out::Pass;
    }
    let s = k.s.as_str();
    let empty = conn(m, &key).input.is_empty();
    match s {
        "esc" if empty && conn(m, &key).watch.back => {
            go_back(m, &key);
            Out::Draw
        }
        "tab" if empty && box_run(m, &key).is_some_and(|id| sel(m) == format!("sub:{id}")) => {
            focus_box(m, &key);
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// enter, space or → on a run picked in the list gives its box the keys;
/// on a subagent's step in the conversation (enter or →) or a run in the
/// dock, shows it picked in the subagents view; on a workflow, its
/// agents. ↑↓ go through the dock's runs between the conversation and the
/// queue (paneKey :4255-4270, moveSel :4932, subOfStep).
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
        // On a step a subagent made: that run, picked in the subagents
        // view; esc comes back to the step. Not found to read yet, the
        // step opens in place instead.
        "enter" | "right"
            if empty
                && name == "conversation"
                && picked.contains(":s:")
                && let Some(id) = run_of_step(m, &key, &picked)
                && src_of(m, &key, &id).is_some() =>
        {
            pick_from(m, &key, &id, &picked);
            Out::Draw
        }
        "enter" | "space" | " " | "right" if !picked.is_empty() => {
            if name == "subagents" && is_fold(m, &key, &picked) {
                if s != "right" || !conn(m, &key).watch.open_folds.contains(&picked) {
                    toggle_fold(m, &key, &picked);
                }
                return Out::Draw;
            }
            // A run picked in the list: into its box, when a message can
            // reach it (focus_box says why when none can).
            if picked.starts_with("sub:") && name == "subagents" {
                if box_run(m, &key).is_some_and(|id| picked[4..] == id) {
                    focus_box(m, &key);
                }
                return Out::Draw;
            }
            // On a run in the dock: that run, picked in the subagents view.
            if let Some(id) = picked.strip_prefix("run:") {
                let id = id.to_string();
                pick_from(m, &key, &id, &picked);
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
        // Through the conversation and its runs, each picked in the
        // subagents view (cycleSub); a picked queued message has taken
        // shift+↑↓ before this.
        "shift+up" | "shift+down" | "alt+up" | "alt+down" if empty || s.starts_with("alt") => {
            cycle_sub(m, &key, if s.ends_with("up") { -1 } else { 1 });
            Out::Draw
        }
        _ => Out::Pass,
    }
}

/// The run the conversation's step picked draws: a subagent's call, or
/// one of the agents a command ran ("step/run" when it ran several, each
/// a row of its own), or the agent an agent_result or agent_send call is
/// about, by the id it names (subOfStep).
fn run_of_step(m: &mut Model, key: &str, picked: &str) -> Option<String> {
    let (_, id) = picked.split_once(":s:")?;
    let id = id.to_string();
    let v = m.session.as_ref().filter(|v| v.key == key);
    let kids = v
        .and_then(|v| v.o.kids.get(id.split('/').next().unwrap_or("")))
        .map_or(0, Vec::len);
    let about = v
        .and_then(|v| v.sess.step_by_call(&id).map(|at| v.sess.step(at)))
        .and_then(rush_convo::model::runs::agent_id);
    let prefix = crate::spawns::SPAWN_PREFIX;
    let subs = &conn(m, key).runs.subs;
    if let Some(about) = about {
        let want = format!("{prefix}{about}");
        return subs.iter().find(|sa| sa.id == want).map(|sa| sa.id.clone());
    }
    subs.iter()
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
        // esc and tab give the keys back to the list.
        "esc" | "tab" => {
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

/// The pointer over the subagents view: in the list a click picks a run,
/// which then shows with the list (beside it or under it), and opens or
/// closes a fold; with the run, a click on the box gives it the keys,
/// elsewhere gives them back, and the wheel scrolls the run's
/// conversation (subHoverAt, subMouseMove, clickRef).
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
    let split = conn(m, &key).watch.split;
    // Wide, the column beside the list is the run's, not the list's;
    // narrow, the rows under it.
    let beside = if split > 0 {
        y >= top + split
    } else {
        pw >= WIDE_AT && x.saturating_sub(2) >= list_width(pw)
    };
    if row.is_none() && !in_peek && !in_box && !beside {
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
                // Elsewhere with the run: the keys go back to the list.
                let w = &mut conn(m, &key).watch;
                if w.msg_focus {
                    w.msg_focus = false;
                    return Out::Draw;
                }
                return Out::Idle;
            }
            conn(m, &key).watch.msg_focus = false;
            if rf.is_empty() {
                return Out::Pass;
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
        MouseKind::Moved => {
            // The pointer lights the list's row under it, and nothing
            // else: what shows with the list changes on a click only.
            let over = if in_peek || in_box || beside {
                String::new()
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
