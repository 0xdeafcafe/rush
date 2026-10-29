//! The subagents view: every run the session started, a two-line row
//! each, running ones first, workflows with their phases and agents; and,
//! opened, one run's own conversation under a banner that says whose it
//! is and goes back. Wide, the picked run's conversation shows beside the
//! list. After Go's internal/ui/rushmode.go (views :47, cycleSub :734,
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

use std::collections::HashMap;

use rush_convo::{Job, Session, Subagent};
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
/// How long the pointer rests on a run before the wide view shows its
/// conversation beside the list (subPeekAfter).
pub const PEEK_AFTER: Ms = 300;
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
    open.is_empty() && w >= 150 && any
}

/// The list's width when the run's conversation is beside it
/// (subListWidth).
pub fn list_width(w: usize) -> usize {
    72.min(w * 2 / 5)
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
pub fn watching(m: &mut Model, key: &str) -> bool {
    let c = conn(m, key);
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

/// Leaves the opened run for wherever it was opened from (closeSub).
pub fn close_sub(m: &mut Model, key: &str) {
    let w = std::mem::take(&mut conn(m, key).watch);
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
    conn(m, key).watch = Watch::default();
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
    s: &'a Session,
    r: &'a Runs,
    reg: &'a rush_core::adapter::Registry,
    home: &'a str,
    own_wt: String,
    sel: &'a str,
    tick: usize,
    now: Ms,
    w: usize,
    out: Vec<(Line, String)>,
}

impl List<'_> {
    /// A run's two rows: what it was asked and how it stands, then what
    /// runs it, where, what it used and its last words (subagentList's
    /// draw).
    fn draw(&mut self, r: &Row, i: usize, indent: &str) {
        let (sa, w) = (r.sa, self.w);
        let rf = format!("sub:{}", sa.id);
        let (first, last) = r.st.map_or((0, 0), |t| (t.first, t.last));
        let end = if r.live { self.now } else { last };
        let took = if first > 0 && end > 0 {
            dur(round_s(end - first))
        } else {
            String::new()
        };
        let kind = kind_of(self.r, self.home, &sa.id);
        let mark = sub_mark(self.reg, &kind, &r.status, r.live, self.tick + i);
        let mut right = line([sub_word(&r.status, r.live), plain("   ")]);
        if !took.is_empty() {
            right.spans.push(dim(took));
        }
        right.spans.push(plain("  "));
        let room = (w as isize - right.width() as isize - 6 - indent.len() as isize).max(12);
        let name = rush_draw::truncate(&sub_task(sa), room as usize, "…").into_owned();
        let top = spread(
            line([
                plain(format!("{indent}  ")),
                mark,
                plain(" "),
                Span::new(name, bold(TEXT)),
            ]),
            right,
            w,
        );
        let mut facts: Vec<String> = Vec::new();
        let tag = sub_tag(sa, r.st, &kind, self.home);
        if !tag.is_empty() {
            facts.push(tag);
        }
        let wt = self.worktree(&sa.id);
        if !wt.is_empty() {
            facts.push(format!("⎇ {wt}"));
        }
        if let Some(t) = r.st {
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
        let mut second = line([plain(format!("{indent}      ")), dim(facts.join(" · "))]);
        if let Some(lw) =
            r.st.map(|t| t.last_words.as_str())
                .filter(|l| !l.is_empty())
        {
            let keep = (w as isize - second.width() as isize - 8).max(10) as usize;
            second.spans.push(dim("  ·  "));
            second
                .spans
                .push(faint(rush_draw::truncate(lw, keep, "…").into_owned()));
        }
        if self.sel == rf {
            self.out.push((picked1(top, w, true), rf.clone()));
            self.out.push((on_ground(second, w, SEL_BG), rf));
            return;
        }
        self.out.push((fit(top, w), rf.clone()));
        self.out.push((fit(second, w), rf));
    }

    /// The worktree run id worked in, when not the session's
    /// (subWorktree).
    fn worktree(&self, id: &str) -> String {
        if let Some(wt) = self.r.wt.get(id).filter(|w| !w.is_empty()) {
            return wt.clone();
        }
        match self.r.stats.get(id) {
            Some(t) if !t.worktree.is_empty() && t.worktree != self.own_wt => t.worktree.clone(),
            _ => String::new(),
        }
    }

    /// Workflow g: its row (what it's for, its phases, how it stands),
    /// then each phase with its agents under it, each one's run drawn as
    /// any run is, a step in (workflowGroupLines).
    fn group(&mut self, g: &Group, runs: &HashMap<String, Row>) {
        let (w, now, tick) = (self.w, self.now, self.tick);
        let end = if g.live || g.end == 0 { now } else { g.end };
        let mut right = match () {
            _ if g.live => line([sp("running", ORANGE)]),
            _ if g.status == "failed" => line([sp("failed", RED)]),
            _ if g.status == "stopped" => line([dim("stopped")]),
            _ => line([dim("done")]),
        };
        if g.start > 0 {
            right.spans.push(plain("   "));
            right.spans.push(dim(dur(round_s(end - g.start))));
        }
        right.spans.push(plain("  "));
        let mark = if g.live {
            sp(SPINNER[tick % SPINNER.len()], ORANGE)
        } else {
            sp("⋔", QUEUE)
        };
        let room = w as isize - right.width() as isize - 8;
        let title_w = (room / 2).clamp(12, 60) as usize;
        let title = rush_draw::truncate(&g.title, title_w, "…").into_owned();
        let mut left = line([
            plain("  "),
            mark,
            plain(" "),
            Span::new(title, bold(TEXT)),
            plain("   "),
        ]);
        join(&mut left, workflow_strip(&g.phases, tick));
        let keep = (room + 4).max(20) as usize;
        let top = spread(rush_draw::truncate_line(&left, keep, "…"), right, w);
        let rf = g.r#ref();
        let picked = self.sel == rf;
        self.out.push((
            if picked {
                picked1(top, w, true)
            } else {
                fit(top, w)
            },
            rf.clone(),
        ));
        if !g.job.is_empty() && (g.tokens > 0 || g.tool_uses > 0) {
            let mut facts = Vec::new();
            if g.tool_uses > 0 {
                facts.push(format!("{} steps", g.tool_uses));
            }
            if g.tokens > 0 {
                facts.push(format!("{} tokens", tokens(g.tokens)));
            }
            let row = line([plain("      "), dim(facts.join(" · "))]);
            self.out.push((
                if picked {
                    picked1(row, w, true)
                } else {
                    fit(row, w)
                },
                rf,
            ));
        }
        let mut shown: Vec<&str> = Vec::new();
        let label_w = g
            .phases
            .iter()
            .flat_map(|p| &p.agents)
            .map(|a| width(&a.label).min(28))
            .fold(8, usize::max);
        for p in &g.phases {
            // A gap above each phase, and how far it's got, so the eye
            // finds where one ends and the next starts.
            let title = if p.title.is_empty() {
                "agents"
            } else {
                &p.title
            };
            let mut head = line([
                plain("    "),
                phase_mark(p, tick),
                plain(" "),
                Span::new(title.to_string(), bold(SUB)),
            ]);
            let done = p.agents.iter().filter(|a| a.state == "done").count();
            head.spans.push(if p.agents.is_empty() {
                dim(" · not started")
            } else {
                dim(format!(" · {done} of {} done", p.agents.len()))
            });
            self.out.push((Line::new(), String::new()));
            self.out.push((fit(head, w), String::new()));
            for (i, a) in p.agents.iter().enumerate() {
                if let Some(r) = runs.get(&a.id).filter(|_| !a.id.is_empty()) {
                    shown.push(&a.id);
                    self.draw(r, i, "    ");
                    continue;
                }
                // Not started, or its run's files aren't there yet.
                let row = agent_row(a, label_w, tick + i, w, now);
                self.out.push((fit(cut_lead(row, 2), w), String::new()));
            }
        }
        // Runs no phase lists: before its progress is heard of, all of
        // them; after, the tries an agent made before the one its phase
        // lists, under their own heading.
        let mut i = 0;
        let had = !shown.is_empty();
        for sa in &self.r.subs {
            let Some(r) = runs.get(&sa.id) else {
                continue;
            };
            if sa.workflow != g.run || shown.contains(&sa.id.as_str()) {
                continue;
            }
            if i == 0 && had {
                self.out.push((Line::new(), String::new()));
                self.out.push((
                    fit(
                        line([
                            plain("    "),
                            Span::new("Earlier tries", bold(SUB)),
                            dim(" · tried again above"),
                        ]),
                        w,
                    ),
                    String::new(),
                ));
            }
            self.draw(r, i, "    ");
            i += 1;
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

/// The runs, one two-line row each: a count, then those running and
/// those finished under their headings, workflows first, newest first
/// (subagentList). Each row with its ref.
pub fn list(m: &mut Model, key: &str, w: usize) -> Vec<(Line, String)> {
    let home = home_kind(m, key);
    let (now, tick) = (m.now, m.tick);
    let sel = sel(m);
    let reg = m.reg.clone();
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let s = &v.sess;
    let r = &kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default()
        .runs;
    let jobs: HashMap<String, &Job> = s.subagent_jobs();
    let mut running = 0;
    let mut rows: Vec<Row> = Vec::new();
    let mut by_id: HashMap<String, Row> = HashMap::new();
    for sa in r.subs.iter().rev() {
        let (status, live) = r.state(s, sa, &jobs, now);
        if live {
            running += 1;
        }
        let row = Row {
            sa,
            st: r.stats.get(&sa.id),
            status,
            live,
        };
        if sa.workflow.is_empty() {
            rows.push(row);
        } else {
            by_id.insert(sa.id.clone(), row);
        }
    }
    let live_of = |sa: &Subagent| r.state(s, sa, &jobs, now).1;
    let gs = groups(r, s, &live_of);
    let mut head = Line::new();
    if running > 0 {
        head.spans.push(sp(format!("{running} running"), ORANGE));
        head.spans.push(dim(" · "));
    }
    head.spans.push(dim(format!("{} runs", r.subs.len())));
    let mut first = line([plain("  ")]);
    join(&mut first, head);
    first
        .spans
        .push(dim(" · space opens one · → opens its conversation"));
    let cwd = if s.info.cwd.is_empty() {
        &s.cwd
    } else {
        &s.info.cwd
    };
    let mut l = List {
        s,
        r,
        reg: &reg,
        home: &home,
        own_wt: rush_convo::text::worktree_in(&format!("{cwd}/")).to_string(),
        sel: &sel,
        tick,
        now,
        w,
        out: vec![(fit(first, w), String::new()), (Line::new(), String::new())],
    };
    let _ = l.s;
    rows.sort_by_key(|r| !r.live);
    for (title, live) in [("Running", true), ("Finished", false)] {
        let n = l.out.len();
        for g in gs.iter().filter(|g| g.live == live) {
            l.group(g, &by_id);
        }
        for (i, r) in rows.iter().enumerate() {
            if r.live == live {
                l.draw(r, i, "");
            }
        }
        if l.out.len() > n {
            l.out.insert(
                n,
                (
                    fit(line([plain("  "), Span::new(title, bold(SUB))]), w),
                    String::new(),
                ),
            );
            l.out.push((Line::new(), String::new()));
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
/// else the one the pointer rests on, else the latest working, else the
/// latest (subPreviewLines' id).
fn peek_id(m: &mut Model, key: &str) -> Option<String> {
    let sel = sel(m);
    let now = m.now;
    let hover = conn(m, key).vs.hover_ref.clone();
    let hover_at = conn(m, key).watch.hover_at;
    let id = {
        let v = m.session.as_ref()?;
        let r = runs_of(m, key)?;
        match sel.strip_prefix("sub:") {
            Some(id) => id.to_string(),
            None => match r.running(&v.sess, now).first() {
                Some(sa) => sa.id.clone(),
                None => r.subs.last()?.id.clone(),
            },
        }
    };
    let p = &mut conn(m, key).watch.peek;
    if p.selected != sel {
        p.selected.clone_from(&sel);
        p.hover.clear();
    }
    if let Some(h) = hover.strip_prefix("sub:")
        && now - hover_at >= PEEK_AFTER
    {
        p.hover = h.to_string();
    }
    // A hovered one stays while the pointer crosses into it to read.
    Some(if p.hover.is_empty() {
        id
    } else {
        p.hover.clone()
    })
}

/// Lays the run's conversation beside the wide list, over the rows after
/// the view's first top (drawSubPreview, subPreviewLines).
pub fn beside(m: &mut Model, key: &str, out: &mut [Line], w: usize, screen_top: usize) {
    conn(m, key).watch.peek.visible = false;
    let any = has_runs(m, key);
    if !wide(&conn(m, key).watch.open, w, any) {
        return;
    }
    let Some(id) = peek_id(m, key) else {
        return;
    };
    let lw = list_width(w);
    let pw = w.saturating_sub(lw + 3);
    let h = out.len();
    let name = runs_of(m, key)
        .and_then(|r| r.subs.iter().find(|sa| sa.id == id))
        .map(|sa| {
            if sa.typ.is_empty() {
                id.clone()
            } else {
                sa.typ.clone()
            }
        })
        .unwrap_or_else(|| id.clone());
    if let Some(src) = src_of(m, key, &id) {
        show_detail(m, key, &id, src);
    }
    let (now, tick) = (m.now, m.tick);
    let c = conn(m, key);
    if c.watch.peek.id != id {
        c.watch.peek.id.clone_from(&id);
        c.watch.peek.scroll = 0;
        c.watch.peek.total = 0;
    }
    let rows = h.saturating_sub(2);
    let mut right: Vec<Line> = vec![Line::new(); h];
    if h > 0 {
        right[0] = line([dim(format!(" {name} · scroll here · click to open"))]);
    }
    let p = &mut c.watch.peek;
    if let Some(d) = c.runs.detail.as_mut().filter(|d| d.id == id) {
        let v = &mut d.view;
        v.sync(pw, now, tick);
        v.o.selected.clear();
        v.o.focused = false;
        v.o.hide_activity = true;
        if rows > 0 && v.read {
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
            let b = v.body(rows);
            for (i, l) in b.into_iter().enumerate() {
                right[i + 1] = l;
            }
            if h > 1 && total > rows {
                let at = if p.scroll > 0 {
                    format!("↓ {} more", p.scroll)
                } else {
                    "latest".into()
                };
                right[h - 1] = line([dim(format!(" {}–{end} / {total} · {at}", start + 1))]);
            }
        }
    }
    p.x = 2 + lw + 3;
    p.y = screen_top;
    p.w = pw;
    p.h = h;
    p.visible = true;
    for (i, l) in out.iter_mut().enumerate() {
        let mut row = fit(std::mem::take(l), lw);
        row.spans.push(plain(" "));
        row.spans.push(faint("│"));
        row.spans.push(plain(" "));
        join(&mut row, fit(std::mem::take(&mut right[i]), pw));
        *l = row;
    }
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
        "enter" | "space" | " " | "right" if !picked.is_empty() => {
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

/// The pointer over the subagents view: the banner goes back or to
/// another run, a click picks a run and again opens it, the wide list's
/// conversation opens on a click and scrolls under the wheel; resting on
/// a run shows its conversation beside the list (subHoverAt,
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
    let watching = watching(m, &key);
    if row.is_none() && !in_peek {
        return Out::Pass;
    }
    match e.kind {
        MouseKind::Down(Button::Left) => {
            if in_peek {
                let id = conn(m, &key).watch.peek.id.clone();
                open_sub(m, &key, &id);
                return Out::Draw;
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
            if rf.is_empty() {
                return Out::Pass;
            }
            let picked = sel(m);
            if is_workflow_ref(m, &rf) && rf == picked {
                open_workflow(m, &key, &rf);
                return Out::Draw;
            }
            if let Some(id) = rf.strip_prefix("sub:")
                && rf == picked
            {
                let id = id.to_string();
                open_sub(m, &key, &id);
                return Out::Draw;
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
            // Wide, a run's conversation sits beside the list on its rows:
            // only the list itself lights a row (subHoverAt :1007-1011).
            let pw = conn(m, &key).watch.pane_w;
            let beside = !watching && pw >= 150 && x.saturating_sub(2) >= list_width(pw);
            let over = if in_peek || beside {
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
            c.vs.hover_ref = over.clone();
            c.watch.hover_at = now;
            if over.starts_with("sub:") && !watching && m.sender().is_some() {
                // Drawn again once it has rested long enough for the run's
                // conversation to show beside the list.
                let key = key.clone();
                m.later(
                    || std::thread::sleep(std::time::Duration::from_millis(PEEK_AFTER as u64)),
                    move |m, ()| {
                        if conn(m, &key).vs.hover_ref == over {
                            Out::Draw
                        } else {
                            Out::Idle
                        }
                    },
                );
            }
            Out::Draw
        }
        _ => Out::Pass,
    }
}
