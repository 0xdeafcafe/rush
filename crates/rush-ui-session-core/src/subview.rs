//! A session's runs, opened on the conversation's page (openrun.rs says
//! where): what an open run shows (its facts, its conversation with its
//! working line while it works, and a box to message it), a workflow's
//! phases and agents, the dock's runs under the conversation, and the keys
//! and clicks that open, cycle and close them. A run's conversation is a
//! session's own while its pane has the keys, driven as one (subpane.rs).
//! Rust-only in where it shows: Go, and the port before this, listed the
//! runs in a subagents view, a tab of their own. After Go's
//! internal/ui/rushmode.go (cycleSub :734, watchRun :809, subHoverAt :990,
//! subRow.appendTo :1213, subEffort :1231, paneKey's subagents keys
//! :4255-4262, dockRefs :4900, clickRef :5046, :5102-5122, subWhere :6299,
//! subWorktree :6308), workflow.go (workflowGroupLines :400, openWorkflow
//! :586), subpreview.go (subPreviewLines :33, drawSubPreview :94),
//! spawns.go (spawnRunFor :694, subKind :721) and model.go (the preview's
//! click :1700 and wheel :1760, subHoverMsg :1192).
//!
//! The open run is read by subs.rs off the UI thread, as the Session's
//! runs are, into a SessionView of its own (Runs::detail), and drawn as a
//! conversation is; what its box sends is relay.rs's. Not here: what a
//! working run takes of the machine (subUsage: the process table).

use rush_convo::{Job, Subagent};
use rush_core::text::{Line, Span, width};
use rush_core::time::Ms;
use rush_draw::{Button, MouseKind};

use super::dock::spread;
use super::paint::{dim, faint, plural, tokens};
use super::subname::{sub_mark, sub_tag, sub_task, sub_word};
use super::subs::{Detail, DetailSrc, RunStat, Runs};
use super::workflow::{agent_row, groups, phase_mark, round_s};
use super::{agent, conn, open_key};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::session::{SessionView, Source};
use crate::spawns::SPAWN_PREFIX;
use crate::style::*;

/// What's open of session key's runs, and how it got there (Go's hostConn:
/// subBack, subFrom, subHoverAt, subPreview).
#[derive(Default)]
pub struct Watch {
    /// Opened from elsewhere (a step, the dock, the column): esc, closing
    /// it, picks that row again.
    pub back: bool,
    pub from: String,
    /// When the pointer came to rest on what it's over.
    pub hover_at: Ms,
    /// The open run's conversation, as last drawn.
    pub peek: Peek,
    /// The box under the run beside the list: what's typed for each run
    /// (subbox.rs), whether it has the keys, and where it was drawn (x,
    /// y, w, h).
    pub drafts: super::subbox::Drafts,
    pub msg_focus: bool,
    pub msg_at: [usize; 4],
    /// The run whose box has the keys, while one does: they stay its as
    /// its run is drawn again, and go only when another's shown
    /// (Rust-only).
    pub msg_for: String,
    /// The run picked from elsewhere whose box takes the keys once it's
    /// drawn, if a message can reach it (Rust-only).
    pub focus_next: String,
    /// A hosted child's breadcrumb back to its parent, screen cells
    /// (subpane::child_crumb).
    pub child_crumb: (usize, usize),
    /// The run (its id) or workflow (its ref) open on the conversation's
    /// page, beside it or framed under its step (openrun.rs); "" for none.
    pub open: String,
    /// Where it was drawn, screen cells (x, y, w, h), and its ✕.
    pub frame: [usize; 4],
    pub close_at: (usize, usize),
    /// A workflow open: the ref on each row of its pane, and the pane's
    /// cells (x, y, w, h).
    pub flow_refs: Vec<String>,
    pub flow_at: [usize; 4],
}

/// The run's conversation in its pane, as last drawn (subPreview): what
/// its view keeps that the session's would (subpane::as_run puts them in
/// its place), and where it is.
#[derive(Default)]
pub struct Peek {
    pub id: String,
    /// The text dragged over in it, and the item under the pointer.
    pub txt: super::textsel::TextSel,
    pub hover: String,
    /// Its conversation's rows, and where they were drawn, in screen
    /// cells.
    pub rows: usize,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub visible: bool,
}

/// Session key's runs, read without changing anything.
pub fn runs_of<'a>(m: &'a Model, key: &str) -> Option<&'a Runs> {
    m.kept
        .peek::<super::Core>()
        .and_then(|c| c.conns.get(key))
        .map(|c| &c.runs)
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

/// Shows run id whole, w wide: read off the UI thread, its end first
/// when it's long, and laid out there as its pane draws it, so the frame
/// that has it draws nothing new (subDetail).
pub fn show_detail(m: &mut Model, key: &str, id: &str, src: DetailSrc, w: usize) {
    let r = &mut conn(m, key).runs;
    if r.detail
        .as_ref()
        .is_some_and(|d| d.id == id && d.src == src)
    {
        return;
    }
    let live = run_live(m, key, id);
    let mut view = SessionView::new(key.to_string(), Source::Hosted, 0);
    super::subpane::prime(&mut view, w, (m.now, m.tick), live, false);
    view.note_drawn();
    let r = &mut conn(m, key).runs;
    r.detail = Some(Detail::new(id.to_string(), src, view));
    r.detail_more = false;
    // A reading under way stops short for it (Runs::hurry).
    r.hurry.store(true, std::sync::atomic::Ordering::Relaxed);
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

/// Opens run id on the conversation's page (openrun.rs), its pane with
/// the keys once it shows.
pub fn pick_run(m: &mut Model, key: &str, id: &str) {
    super::openrun::open(m, key, id, "");
}

/// Opens run id from the row from picked in the conversation (a
/// subagent's step, the dock's run, the column's): esc, closing it, picks
/// that row again (watchRun, rushmode.go:777).
pub fn pick_from(m: &mut Model, key: &str, id: &str, from: &str) {
    super::openrun::open(m, key, id, from);
}

/// Closes the run open, back to where it was opened from (closeSub):
/// whether one was open.
pub fn go_back(m: &mut Model, key: &str) -> bool {
    if !super::openrun::is_open(m, key) {
        return false;
    }
    super::openrun::close(m, key);
    true
}

/// Steps through the session's runs, running ones first, then the rest,
/// newest first, each opened in turn; the conversation alone sits at
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
    let open = super::openrun::open_id(m, key).unwrap_or_default();
    let cur = order
        .iter()
        .position(|id| *id == open)
        .map_or(0, |i| i as isize + 1);
    let next = ((cur + dir) % n + n) % n;
    if next == 0 {
        go_back(m, key);
        return;
    }
    // A sibling is still opened from where this one was.
    let from = conn(m, key).watch.from.clone();
    let from = if from.is_empty() { sel(m) } else { from };
    pick_from(m, key, &order[next as usize - 1], &from);
}

/// Opens the workflow ref picks on the conversation's page: its phases
/// and its agents, each a row that opens its run (openWorkflow).
pub fn open_workflow(m: &mut Model, key: &str, r: &str) {
    conn(m, key).runs.detail = None;
    let from = sel(m);
    super::openrun::open(m, key, r, &from);
    // A workflow's pane has no box: the keys stay where they were.
    conn(m, key).watch.focus_next.clear();
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
    // Two monitors or more are the one row, as the dock draws them.
    let loose: Vec<&Job> = sh.loose.iter().filter_map(|id| v.sess.job(id)).collect();
    let mons = rush_ui_list_extras::jobs::monitors(&v.sess, &loose);
    refs.extend(
        loose
            .iter()
            .filter(|j| !mons.iter().any(|m| m.id == j.id))
            .map(|j| format!("job:{}", j.id)),
    );
    if !mons.is_empty() {
        refs.push(rush_ui_list_extras::jobs::MONITORS_REF.to_string());
    }
    refs
}

/// How long a run has gone, or went.
pub fn took_of(st: Option<&RunStat>, live: bool, now: Ms) -> String {
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

/// The run's facts: what runs it, where it works, its steps, what it
/// cost and its effort (subagentList's second row).
pub fn facts_of(r: &Runs, sa: &Subagent, home: &str, w: usize) -> Vec<String> {
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

/// The column's top rows for run id: how it stands, then its facts.
fn peek_head(m: &mut Model, key: &str, id: &str, w: usize) -> Vec<Line> {
    let home = home_kind(m, key);
    let (now, tick) = (m.now, m.tick);
    let reg = m.reg.clone();
    // The parent's auto-compact window: its subagents run in its process,
    // under the same settings, so they compact within it too.
    let compaction = super::agent(m, key)
        .map(|a| a.compaction)
        .unwrap_or_default();
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
    let mut facts = facts_of(r, sa, &home, w);
    // How full its context is: a subagent compacts on its own, so this
    // is how near that is.
    if let Some(d) = r.detail.as_ref().filter(|d| d.id == id)
        && d.view.sess.context > 0
    {
        let used = d.view.sess.context;
        let model = st.map(|t| t.model.as_str()).unwrap_or("");
        let window = if model.is_empty() {
            0
        } else {
            reg.context_window(&kind, model)
        };
        // A subagent of the parent's own harness compacts within the
        // parent's window (an agent rush hosts beside it has its own).
        let f = rush_core::adapter::fill_of(
            used,
            window,
            if kind == home {
                compaction
            } else {
                Default::default()
            },
        );
        facts.push(if f.window > 0 {
            format!(
                "context {} of {} · {:.0}%",
                tokens(used),
                tokens(f.window),
                f.pct()
            )
        } else {
            format!("context {}", tokens(used))
        });
    }
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

/// A workflow open (openrun.rs): when it started, how it stands, each
/// phase and how far it got, its agents under it; and the ref on each
/// row, an agent's run's id where it has one, which a click opens.
pub fn flow_lines(m: &mut Model, key: &str, rf: &str, w: usize) -> (Vec<Line>, Vec<String>) {
    let (now, tick) = (m.now, m.tick);
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return (Vec::new(), Vec::new());
    };
    let s = &v.sess;
    let Some(r) = runs_of(m, key) else {
        return (Vec::new(), Vec::new());
    };
    let jobs = s.subagent_jobs();
    let live_of = |sa: &Subagent| r.state(s, sa, &jobs, now).1;
    let Some(g) = groups(r, s, &live_of).into_iter().find(|g| g.r#ref() == rf) else {
        return (Vec::new(), Vec::new());
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
    let mut refs: Vec<String> = Vec::new();
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
        refs.resize(out.len(), String::new());
        let label_w = agents.iter().map(|a| width(&a.label)).max().unwrap_or(0);
        for (i, a) in agents.into_iter().enumerate() {
            out.push(fit(agent_row(a, label_w, tick + i, w, now), w));
            let run = g.subs.get(&a.id).map(|sa| sa.id.clone());
            refs.push(run.unwrap_or_default());
        }
    }
    out.push(Line::new());
    out.push(line([faint("   click one of its agents to open it")]));
    refs.resize(out.len(), String::new());
    (out, refs)
}

/// Run id in rows pw wide, the first at (x, y) on the screen: its head,
/// its conversation (a session's own, subpane.rs), what became of what
/// was sent it, its box.
pub fn peek(
    m: &mut Model,
    key: &str,
    id: &str,
    right: &mut [Line],
    pw: usize,
    (x, y): (usize, usize),
) {
    let h = right.len();
    if let Some(src) = src_of(m, key, id) {
        show_detail(m, key, id, src, pw);
    }
    // Followed as it grows while it runs and shows (subfollow).
    super::subfollow::sync(m, key);
    // The keys stay with the pane of the run they were given for, however
    // often it's drawn; another run shown, they go back to the list. One
    // just picked takes them (Rust-only).
    let take = conn(m, key).watch.focus_next == id && can_focus(m, key, id);
    let w = &mut conn(m, key).watch;
    if w.msg_for != id {
        w.msg_focus = false;
    }
    if !w.focus_next.is_empty() {
        w.focus_next.clear();
        if take {
            w.msg_focus = true;
            w.msg_for = id.to_string();
        }
    }
    let focused = w.msg_focus;
    let head = peek_head(m, key, id, pw);
    let mut bx = super::relay::note_lines(m, key, id, pw);
    bx.extend(super::subbox::lines(m, key, id, pw));
    let live = run_live(m, key, id);
    let c = conn(m, key);
    if c.watch.peek.id != id {
        c.watch.peek.id = id.to_string();
        c.watch.peek.txt = Default::default();
        c.watch.peek.hover.clear();
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
    p.rows = rows;
    p.visible = true;
    let read = c.runs.detail.as_ref().is_some_and(|d| d.id == id);
    if read {
        if rows > 0 {
            // More was just read above what showed: it stays in place.
            if let Some(d) = conn(m, key).runs.detail.as_mut()
                && let Some(below) = d.keep_below.take()
            {
                d.view.keep_rows_below(below, rows);
            }
            let body = super::subpane::as_run(m, key, id, |m| {
                super::subpane::text(m, key, pw, rows, live, focused)
            })
            .unwrap_or_default();
            for (i, l) in body.into_iter().enumerate().take(rows) {
                right[at + i] = l;
            }
            more_above(m, key, rows);
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
    conn(m, key).watch.msg_at = [x, y + body_end, pw, box_h];
}

/// A long run shows only its end: scrolled to within a screen of the top
/// of what it has ("reading earlier…"), more of what came before is read,
/// rather than the whole of it at once when it opens (Rust-only).
fn more_above(m: &mut Model, key: &str, rows: usize) {
    let r = &mut conn(m, key).runs;
    let top = r
        .detail
        .as_ref()
        .is_some_and(|d| d.view.read && d.view.sess.partial && d.view.start <= rows);
    if !top || r.detail_more {
        return;
    }
    r.detail_more = true;
    r.hurry.store(true, std::sync::atomic::Ordering::Relaxed);
    super::subs::kick(m, key);
    super::subs::refresh(m, key);
}

/// The keys of a run open on the conversation's page before the views':
/// tab gives the run's pane the keys (and tab in it gives them back,
/// subpane.rs), whatever the session's own box holds but a / command's
/// picker; alt+o on a run rush hosts opens its own session (Rust-only).
pub fn key_first(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some(open) = super::openrun::open_id(m, &key) else {
        return Out::Pass;
    };
    let s = k.s.as_str();
    let picker = conn(m, &key).input.string().starts_with('/');
    match s {
        "tab" if !picker && box_run(m, &key).is_some() => {
            focus_pane(m, &key);
            Out::Draw
        }
        // The conversation has the keys, its box empty: esc closes what's
        // open, as its edge says, before it unpicks a row.
        "esc" if conn(m, &key).input.is_empty() => {
            go_back(m, &key);
            Out::Draw
        }
        // A run rush hosts: its own session, where what it asks is
        // answered (Rust-only).
        "alt+o" => {
            let host = conn(m, &key)
                .runs
                .hosted_of(&open)
                .map(|x| x.hosted.clone());
            match host {
                Some(h) => {
                    super::spawned::open_session(m, &h);
                    Out::Draw
                }
                None => Out::Pass,
            }
        }
        _ => Out::Pass,
    }
}

/// enter or → on a subagent's step in the conversation, or space too on a
/// run in the dock or the column, opens that run on the conversation's
/// page; on a workflow, its phases and agents (paneKey :4255-4270,
/// subOfStep). ↑↓ through the dock's runs are keys.rs's stack_nav.
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
        // On a step a subagent made: that run, opened; esc comes back to
        // the step. Not found to read yet, the step opens in place instead.
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
            // On a background task's notice: the step that started it.
            if picked.ends_with(":woke") && super::running::open_woke(m, &key, &picked) {
                return Out::Draw;
            }
            // On a run in the dock: that run, opened.
            if let Some(id) = picked.strip_prefix("run:") {
                let id = id.to_string();
                pick_from(m, &key, &id, &picked);
                return Out::Draw;
            }
            // The dock's monitors: opened under their row.
            if picked == rush_ui_list_extras::jobs::MONITORS_REF && name == "conversation" {
                super::running::open_monitors(m, &key);
                return Out::Draw;
            }
            if picked.starts_with("wf:") || is_workflow_ref(m, &picked) {
                open_workflow(m, &key, &picked);
                return Out::Draw;
            }
            Out::Pass
        }
        // Through the conversation and its runs, each opened in turn
        // (cycleSub); a picked queued message has taken shift+↑↓ before
        // this.
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
/// beside it: a workflow opens with its agents, a run opens on the
/// conversation's page, a task's step opens where it is in the
/// conversation, a task no step started is picked (clickRef :5102;
/// Rust-only: Go picked them in their views). Whether it was one.
pub fn click_ref(m: &mut Model, key: &str, r: &str) -> bool {
    // A background task's notice: the step that started the task.
    if r.ends_with(":woke") {
        return super::running::open_woke(m, key, r);
    }
    if is_workflow_ref(m, r) {
        open_workflow(m, key, r);
        return true;
    }
    if r == rush_ui_list_extras::jobs::MONITORS_REF {
        super::running::open_monitors(m, key);
        return true;
    }
    if let Some(id) = r.strip_prefix("run:") {
        let id = id.to_string();
        let from = sel(m);
        pick_from(m, key, &id, &from);
        return true;
    }
    if let Some(id) = r.strip_prefix("job:") {
        let id = id.to_string();
        if !super::running::open_task(m, key, &id) {
            set_sel(m, r.to_string());
        }
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

/// The picked run kept where it is as the dock's runs come and go
/// (keepRunPick), after ↑↓ put the cursor on one of the dock's rows
/// (keys.rs's stack_nav, which goes through them between the queue and
/// the conversation: moveSel :4932 over dockRefs).
pub(super) fn keep_pick(m: &mut Model, key: &str) {
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
}

/// The run the box beside the list is for, while it shows.
pub fn box_run(m: &Model, key: &str) -> Option<String> {
    let c = m.kept.peek::<super::Core>()?.conns.get(key)?;
    if !c.watch.peek.visible || c.watch.msg_at[2] == 0 {
        return None;
    }
    Some(c.watch.peek.id.clone())
}

/// Whether run id's pane can have the keys: it has a conversation to
/// read, or a box a message can go from.
fn can_focus(m: &mut Model, key: &str, id: &str) -> bool {
    src_of(m, key, id).is_some() || super::relay::route(m, key, id).is_ok()
}

/// Gives the pane of the run beside the list the keys: its conversation's
/// rows and its box. One no message can reach says why, and its rows
/// still take them.
fn focus_pane(m: &mut Model, key: &str) -> bool {
    let Some(id) = box_run(m, key) else {
        return false;
    };
    if let Err(e) = super::relay::route(m, key, &id) {
        m.flash(&e);
        if src_of(m, key, &id).is_none() {
            return true;
        }
    }
    let w = &mut conn(m, key).watch;
    w.msg_focus = true;
    w.msg_for = id;
    true
}

/// The pointer while a run is open on the conversation's page: ✕ on its
/// edge closes it; a click on its box, its head or its edge gives its pane
/// the keys (its conversation's own clicks are subpane.rs's), a click on a
/// workflow's agent opens that agent; the wheel anywhere in it is its own.
/// A click elsewhere gives the keys back to the conversation and goes on.
/// With nothing open, a click on one of the dock's runs opens it
/// (clickRef).
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
    if super::views::view_name(m, &key) != "conversation" {
        return Out::Pass;
    }
    let (x, y) = (e.x as usize, e.y as usize);
    if super::openrun::is_open(m, &key) {
        let inside = super::openrun::inside(m, &key, x, y);
        match e.kind {
            MouseKind::Down(Button::Left) if inside => {
                if super::openrun::on_close(m, &key, x, y) {
                    super::openrun::close(m, &key);
                    return Out::Draw;
                }
                if let Some(id) = super::openrun::flow_ref_at(m, &key, x, y) {
                    let from = conn(m, &key).watch.open.clone();
                    pick_from(m, &key, &id, &from);
                    return Out::Draw;
                }
                if !focus_pane(m, &key) {
                    return Out::Idle;
                }
                return Out::Draw;
            }
            MouseKind::Down(Button::Left) => {
                // Elsewhere: the keys go back to the conversation, and the
                // click goes on to what's under it.
                conn(m, &key).watch.msg_focus = false;
            }
            MouseKind::ScrollUp | MouseKind::ScrollDown | MouseKind::Moved if inside => {
                return Out::Idle;
            }
            _ => {}
        }
    }
    // The dock's runs on the transcript's end.
    if e.kind != MouseKind::Down(Button::Left) {
        return Out::Pass;
    }
    let r = m
        .session
        .as_ref()
        .and_then(|v| v.shown.get(y - top.min(y)).cloned())
        .filter(|_| y >= top)
        .unwrap_or_default();
    if !r.is_empty() && click_ref(m, &key, &r) {
        return Out::Draw;
    }
    Out::Pass
}
