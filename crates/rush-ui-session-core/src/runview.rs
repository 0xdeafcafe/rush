//! A run of the session's opened whole: its own screen inside the
//! Session's, as a session of its own looks. The run's conversation full
//! width, drawn and driven as the session's own (conversation::rows_of:
//! the pins, the pill, the row under the pointer, dragged-over text;
//! ↑↓ pick rows, enter → ← open and close them, ctrl+o shows all, o opens
//! a step's file, a click picks or opens a row, a link opens), under a
//! header that says where it sits (`<parent> › <run>`), how it stands, its
//! model, effort, worktree, cost and tokens, and its tabs: Conversation,
//! Overview (how it stands, what it was asked, its steps so far), Changes
//! (the files it edited) and Tasks (its task list), the last two where it
//! has them. Under it one box, the run's own (subbox.rs), routed as
//! relay.rs routes it: into its inbox while it runs, passed on by the
//! session once it's ended. esc, ← from an empty box and a click on the
//! breadcrumb go back to the Subagents list, the run still picked; one
//! opened from elsewhere (its step, the dock) goes back there on esc.
//!
//! It is a sub-state of the parent's Session (`Watch::open`), not an
//! agent of its own: the run's conversation is the SessionView subs.rs
//! already reads for the run picked (`Runs::detail`), put in m.session's
//! place only for the length of a call (`as_run`), so the Session's own
//! drawing and row keys work on it unchanged, while the parent's live
//! link, its box, queue and cards stay the parent's. DESIGN.md has why.
//! Rust-only: Go opened a run whole under a banner (rushmode.go openSub,
//! watchRun :777); the Rust port had dropped that for the list's peek.

use rush_core::text::{Line, Span, width};
use rush_draw::{Button, MouseKind};

use super::paint::{dim, faint, plural, spread};
use super::subname::{sub_mark, sub_task, sub_word};
use super::{conn, core, open_key};
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::session::{HEAD_H, Pos};
use crate::style::*;

/// The run open whole, and where its screen's parts were drawn.
#[derive(Default)]
pub struct Open {
    pub id: String,
    /// The tab showing, an index into tabs().
    pub tab: usize,
    /// The text dragged over in its conversation (the session's own is
    /// kept apart, in Conn::txt, while this one is in use).
    pub txt: super::textsel::TextSel,
    /// A tab drawn whole: its first row in sight.
    pub top: usize,
    /// The body's rows as last drawn.
    pub rows: usize,
    /// The tabs as last drawn: from and to, screen cells, and the tab.
    pub tab_at: Vec<(usize, usize, usize)>,
    /// The breadcrumb's way back on the header's name row: from and to,
    /// screen cells.
    pub crumb: (usize, usize),
}

/// The run open whole in session key, while the subagents view shows.
pub fn open_id(m: &Model, key: &str) -> Option<String> {
    let c = m.kept.peek::<super::Core>()?.conns.get(key)?;
    let id = c.watch.open.as_ref()?.id.clone();
    (super::views::view_name(m, key) == "subagents").then_some(id)
}

fn open_mut<'a>(m: &'a mut Model, key: &str) -> Option<&'a mut Open> {
    conn(m, key).watch.open.as_mut()
}

/// Opens run id of session key whole: the subagents view showing, the run
/// picked in its list, its conversation at its end. Whether it could be:
/// a run known only by its task has no conversation to open.
pub fn open(m: &mut Model, key: &str, id: &str) -> bool {
    let Some(src) = super::subview::src_of(m, key, id) else {
        return false;
    };
    if super::views::view_name(m, key) != "subagents" {
        super::subview::pick_run(m, key, id);
    }
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
        v.sel = format!("sub:{id}");
    }
    super::subview::show_detail(m, key, id, src);
    let c = conn(m, key);
    if let Some(d) = c.runs.detail.as_mut().filter(|d| d.id == id) {
        // As a session opens: at its end, following it.
        d.view.pos = Pos::End;
        d.view.sel.clear();
    }
    c.watch.msg_focus = false;
    c.watch.focus_next.clear();
    c.vs.hover_ref.clear();
    c.watch.open = Some(Open {
        id: id.to_string(),
        ..Open::default()
    });
    true
}

/// Closes the run open whole: back to the subagents list with it picked;
/// one opened from elsewhere goes back there unless to_list.
pub fn close(m: &mut Model, key: &str, to_list: bool) {
    let c = conn(m, key);
    let Some(o) = c.watch.open.take() else {
        return;
    };
    c.vs.hover_ref.clear();
    if !to_list && c.watch.back {
        super::subview::go_back(m, key);
        return;
    }
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key) {
        v.sel = format!("sub:{}", o.id);
    }
}

/// Runs f with the run open whole in m.session's place, its dragged-over
/// text in the session's: the Session's own drawing, keys and clicks then
/// work on the run's conversation. The session's own view is put back
/// after, as it was. None when no run is open or its conversation isn't
/// read.
pub fn as_run<T>(m: &mut Model, key: &str, f: impl FnOnce(&mut Model) -> T) -> Option<T> {
    let id = conn(m, key).watch.open.as_ref()?.id.clone();
    if m.session.as_ref().is_none_or(|v| v.key != key) {
        return None;
    }
    let mut d = conn(m, key).runs.detail.take()?;
    if d.id != id {
        conn(m, key).runs.detail = Some(d);
        return None;
    }
    let run_txt = conn(m, key)
        .watch
        .open
        .as_ref()
        .map(|o| o.txt)
        .unwrap_or_default();
    let own_txt = std::mem::replace(&mut conn(m, key).txt, run_txt);
    if let Some(v) = m.session.as_mut() {
        // Where the session's conversation would be on screen.
        (d.view.top, d.view.top_off) = (v.top, v.top_off);
        std::mem::swap(v, &mut d.view);
    }
    core(m).as_run = Some(key.to_string());
    let out = f(m);
    core(m).as_run = None;
    let back = m.session.as_mut().filter(|v| v.key == key);
    let put = back.is_some();
    if let Some(v) = back {
        std::mem::swap(v, &mut d.view);
    }
    let run_txt = std::mem::replace(&mut conn(m, key).txt, own_txt);
    if let Some(o) = open_mut(m, key) {
        o.txt = run_txt;
    }
    if put {
        conn(m, key).runs.detail = Some(d);
    } else {
        // f opened another session (a link to an agent): the run's
        // screen is done with.
        conn(m, key).watch.open = None;
    }
    Some(out)
}

/// The tabs run id has: its conversation and overview always; Changes
/// once it has edited a file, Tasks once it has a task list.
fn tabs(m: &Model, key: &str, id: &str) -> Vec<&'static str> {
    let mut t = vec!["conversation", "overview"];
    let Some(d) = super::subview::runs_of(m, key)
        .and_then(|r| r.detail.as_ref())
        .filter(|d| d.id == id)
    else {
        return t;
    };
    if !rush_ui_base::changes::changes(&d.view.sess).is_empty() {
        t.push("changes");
    }
    if !d.view.sess.tasks.is_empty() {
        t.push("tasks");
    }
    t
}

fn tab_name(m: &Model, key: &str, id: &str) -> &'static str {
    let t = tabs(m, key, id);
    let i = m
        .kept
        .peek::<super::Core>()
        .and_then(|c| c.conns.get(key))
        .and_then(|c| c.watch.open.as_ref())
        .map_or(0, |o| o.tab);
    t[i.min(t.len() - 1)]
}

fn pad(mut out: Vec<Line>, rows: usize) -> Vec<Line> {
    out.truncate(rows);
    while out.len() < rows {
        out.push(Line::new());
    }
    out
}

/// The run's screen under the Session's header, rows tall and w wide: the
/// tab showing.
pub fn body(m: &mut Model, key: &str, id: &str, w: usize, rows: usize) -> Vec<Line> {
    if let Some(src) = super::subview::src_of(m, key, id) {
        super::subview::show_detail(m, key, id, src);
    }
    // Followed as it grows while it runs (subfollow).
    super::subfollow::sync(m, key);
    let name = tab_name(m, key, id);
    if let Some(o) = open_mut(m, key) {
        o.rows = rows;
    }
    let read = conn(m, key)
        .runs
        .detail
        .as_ref()
        .is_some_and(|d| d.id == id);
    if !read {
        return pad(
            vec![line([faint(
                "  its conversation isn't found yet; its step is in the main conversation",
            )])],
            rows,
        );
    }
    let live = super::subview::run_live(m, key, id);
    if name == "conversation" {
        let out = as_run(m, key, |m| {
            if let Some(v) = m.session.as_mut() {
                // Its working line under its conversation while it works.
                v.o.hide_activity = !live;
            }
            super::conversation::run_text(m, key, w, rows)
        });
        return pad(out.unwrap_or_default(), rows);
    }
    let all: Vec<Line> = match name {
        "overview" => overview(m, key, id, w),
        "changes" => changes(m, key, id, w),
        "tasks" => as_run(m, key, |m| super::tasks::lines(m, w))
            .unwrap_or_default()
            .into_iter()
            .map(|(l, _)| l)
            .collect(),
        _ => Vec::new(),
    };
    let Some(o) = open_mut(m, key) else {
        return pad(all, rows);
    };
    o.top = o.top.min(all.len().saturating_sub(rows));
    let from = o.top.min(all.len());
    pad(all[from..].to_vec(), rows)
}

/// A heading of the overview.
fn heading(title: &str) -> Line {
    line([plain("  "), spb(title.to_string(), SUB)])
}

/// What it is, how it stands, what it was asked and the steps it's taken
/// so far, the latest last.
fn overview(m: &mut Model, key: &str, id: &str, w: usize) -> Vec<Line> {
    let (now, tick) = (m.now, m.tick);
    let reg = m.reg.clone();
    let home = super::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let Some(r) = super::subview::runs_of(m, key) else {
        return Vec::new();
    };
    let Some(sa) = r.subs.iter().find(|sa| sa.id == id) else {
        return Vec::new();
    };
    let (status, live) = r.state(&v.sess, sa, &v.sess.subagent_jobs(), now);
    let st = r.stats.get(id);
    let kind = super::subview::kind_of(r, &home, id);
    let mut out = vec![Line::new(), heading("Status")];
    let mut how = line([
        plain("  "),
        sub_mark(&reg, &kind, &status, live, tick),
        plain(" "),
        sub_word(&status, live),
    ]);
    let took = super::subview::took_of(st, live, now);
    if !took.is_empty() {
        how.spans.push(dim(format!(" · {took}")));
    }
    if let Some(t) = st.filter(|_| live) {
        if t.waiting {
            how.spans.push(sp(" · waiting on you", YELLOW));
        } else if !t.doing.is_empty() {
            how.spans.push(dim(format!(" · {}", one_line(&t.doing))));
        }
    }
    out.push(fit(how, w));
    let facts = super::subview::facts_of(r, sa, &home, w);
    if !facts.is_empty() {
        out.push(fit(line([plain("  "), dim(facts.join(" · "))]), w));
    }
    let Some(d) = r.detail.as_ref().filter(|d| d.id == id) else {
        return out;
    };
    let s = &d.view.sess;
    let asked = s.turns.first().map(|t| t.prompt.trim()).unwrap_or("");
    if !asked.is_empty() {
        out.push(Line::new());
        out.push(heading("Asked"));
        for l in wrap(asked, w.saturating_sub(4).max(20)) {
            out.push(fit(line([plain("  "), sp(l, TEXT)]), w));
        }
    }
    let steps: Vec<_> = s.steps().iter().filter(|st| st.parent.is_none()).collect();
    out.push(Line::new());
    out.push(line([
        plain("  "),
        spb("Steps", SUB),
        faint(format!("  {}", steps.len())),
    ]));
    if steps.is_empty() {
        out.push(line([faint("  none yet")]));
    }
    for st in steps {
        use rush_convo::Status;
        let mark = match st.status {
            Status::Running => sp(SPINNER[tick % SPINNER.len()], ORANGE),
            Status::Waiting => sp("●", YELLOW),
            Status::Failed | Status::Denied => sp("✗", RED),
            Status::Lost => dim("⏹"),
            _ => sp("✓", GREEN),
        };
        let doing = rush_core::tool::doing(st.call());
        let what = if doing.is_empty() {
            st.tool.to_string()
        } else {
            doing
        };
        out.push(fit(
            line([
                plain("  "),
                mark,
                plain(" "),
                sp(st.tool.to_string(), SUB),
                plain("  "),
                sp(what, TEXT),
            ]),
            w,
        ));
    }
    out
}

/// The files it edited, what it added and took away in each, the most
/// changed first.
fn changes(m: &mut Model, key: &str, id: &str, w: usize) -> Vec<Line> {
    let Some(d) = super::subview::runs_of(m, key)
        .and_then(|r| r.detail.as_ref())
        .filter(|d| d.id == id)
    else {
        return Vec::new();
    };
    let mut files = rush_ui_base::changes::changes(&d.view.sess);
    files.sort_by_key(|f| std::cmp::Reverse(f.add + f.del));
    let (a, del) = files
        .iter()
        .fold((0, 0), |(a, d), f| (a + f.add, d + f.del));
    let mut head = line([
        plain("  "),
        spb("Changes", SUB),
        faint(format!("  {}", plural(files.len(), "file"))),
        plain("  "),
    ]);
    join(&mut head, super::changes::add_del(a, del));
    let mut out = vec![Line::new(), head, Line::new()];
    for f in &files {
        let mut row = line([plain("  ")]);
        join(&mut row, super::changes::add_del(f.add, f.del));
        row.spans.push(plain("  "));
        row.spans.push(sp(tildify(&f.path), TEXT));
        if f.new {
            row.spans.push(faint("  new"));
        }
        out.push(fit(row, w));
    }
    out
}

/// The run's header over the Session's header rows (w wide): the
/// breadcrumb back to the session's list and how the run stands, its
/// facts, and its tabs with the rule under them, the one showing lit.
pub fn head(m: &mut Model, key: &str, id: &str, rows: &mut [Line], w: usize) {
    if rows.len() < HEAD_H {
        return;
    }
    let hw = w.saturating_sub(3);
    let (now, tick) = (m.now, m.tick);
    let reg = m.reg.clone();
    let (home, parent) = super::agent(m, key).map_or_else(Default::default, |a| {
        (a.kind.as_str().to_string(), one_line(&a.display_name))
    });
    let parent = if parent.is_empty() {
        "this session".to_string()
    } else {
        rush_draw::truncate(&parent, 32, "…").into_owned()
    };
    let (task, status, live, facts, took, kind) = {
        let (Some(v), Some(r)) = (
            m.session.as_ref().filter(|v| v.key == key),
            super::subview::runs_of(m, key),
        ) else {
            return;
        };
        let Some(sa) = r.subs.iter().find(|sa| sa.id == id) else {
            return;
        };
        let (status, live) = r.state(&v.sess, sa, &v.sess.subagent_jobs(), now);
        let st = r.stats.get(id);
        (
            sub_task(sa),
            status,
            live,
            super::subview::facts_of(r, sa, &home, w),
            super::subview::took_of(st, live, now),
            super::subview::kind_of(r, &home, id),
        )
    };
    // The name row: ‹ parent › run · how it stands.
    let mut state = line([
        sub_mark(&reg, &kind, &status, live, tick),
        plain(" "),
        sub_word(&status, live),
    ]);
    if !took.is_empty() {
        state.spans.push(dim(format!(" · {took}")));
    }
    let lead = line([plain("  "), sp("‹ ", SUB), sp(parent.clone(), SUB)]);
    // Screen cells: the chrome's two, then the row's own.
    let crumb = (2 + 2, 2 + lead.width());
    let room = hw.saturating_sub(lead.width() + state.width() + 9).max(8);
    let mut row1 = lead;
    row1.spans.push(sp(" › ", FAINT));
    row1.spans.push(spb(
        rush_draw::truncate(&task, room, "…").into_owned(),
        BRIGHT,
    ));
    row1.spans.push(plain("   "));
    row1.spans.push(sp("·", DIM));
    row1.spans.push(plain("  "));
    join(&mut row1, state);
    rows[1] = super::views::chrome(rush_draw::truncate_line(&row1, hw, "…"), hw);
    // Its facts: what runs it, where, its steps, tokens, cost, effort.
    let row2 = line([plain("  "), dim(facts.join(" · "))]);
    rows[2] = super::views::chrome(rush_draw::truncate_line(&row2, hw, "…"), hw);
    // Its tabs.
    let names = tabs(m, key, id);
    let cur = open_mut(m, key).map_or(0, |o| o.tab).min(names.len() - 1);
    let mut strip = line([plain(" ")]);
    let mut tab_at = Vec::new();
    let (mut on_x, mut on_w) = (0, 0);
    for (i, v) in names.iter().enumerate() {
        if i > 0 {
            strip.spans.push(plain(" "));
        }
        let x = strip.width();
        let label = format!(" {} ", super::views::tab_label(v));
        let tw = width(&label);
        tab_at.push((2 + x, 2 + x + tw, i));
        if i == cur {
            (on_x, on_w) = (x, tw);
            strip
                .spans
                .push(Span::new(label, bold(BRIGHT).on(TAB_ON_BG)));
        } else {
            strip.spans.push(sp(label, SUB));
        }
    }
    let hint = line([sp("   [ ]", DIM), sp(" tabs", FAINT)]);
    let chips = line([sp("esc", TEXT), sp(" back to subagents", DIM), plain(" ")]);
    if strip.width() + hint.width() + chips.width() + 2 <= hw {
        join(&mut strip, hint);
    }
    rows[3] = super::views::chrome(spread(strip, chips, hw), hw);
    let on_x = on_x + 2;
    let border = super::agent(m, key).map_or(FAINT, |a| {
        crate::look::harness_border(&m.reg, a.kind.as_str())
    });
    let lit = if m.pane_idle { SUB } else { ORANGE };
    rows[4] = fit(
        line([
            sp("─".repeat(on_x.min(w)), border),
            sp("━".repeat(on_w.min(w.saturating_sub(on_x))), lit),
            sp("─".repeat(w.saturating_sub(on_x + on_w)), border),
        ]),
        w,
    );
    if let Some(o) = open_mut(m, key) {
        o.tab_at = tab_at;
        o.crumb = crumb;
    }
}

/// The run's box and what became of what was sent it, w wide, for the
/// dock: the one box the run's screen has.
pub fn dock_rows(m: &mut Model, key: &str, id: &str, w: usize) -> Vec<Line> {
    let mut out = super::relay::note_lines(m, key, id, w);
    out.extend(super::subbox::lines_for(m, key, id, w, true));
    let hint = super::subbox::hint(m, key, id, w.saturating_sub(4));
    crate::sess::state(m).hint = Some(hint);
    out
}

fn run_sel(m: &Model, key: &str) -> String {
    super::subview::runs_of(m, key)
        .and_then(|r| r.detail.as_ref())
        .map(|d| d.view.sel.clone())
        .unwrap_or_default()
}

fn set_run_sel(m: &mut Model, key: &str, sel: String) {
    if let Some(d) = conn(m, key).runs.detail.as_mut() {
        d.view.sel = sel;
    }
}

/// The run's conversation's own key s: the Session's (Model::session_key)
/// on the run's view.
fn conv_key(m: &mut Model, key: &str, s: &str) -> Out {
    as_run(m, key, |m| Out::from(m.session_key(s))).unwrap_or(Out::Idle)
}

/// Moves a tab drawn whole d rows down (up when negative).
fn scroll_tab(m: &mut Model, key: &str, d: isize) -> Out {
    let Some(o) = open_mut(m, key) else {
        return Out::Pass;
    };
    let was = o.top;
    o.top = (o.top as isize + d).max(0) as usize;
    if o.top == was { Out::Idle } else { Out::Draw }
}

/// The keys while a run is open whole: every one is the run's screen's,
/// but the few the whole of rush answers (ctrl+n, the key guide, ctrl+c
/// with nothing typed). Its conversation's rows take ↑↓ from an empty box,
/// and enter → ← space on a row picked, as the session's own do; typing
/// goes to the run's box (subbox::run_key), which sends it the run's way.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some(id) = open_id(m, &key) else {
        return Out::Pass;
    };
    let s = k.s.as_str();
    let empty = super::subbox::empty(m, &key, &id);
    let d = super::subbox::draft(m, &key, &id);
    let (queue_pick, editing) = (d.pick.is_some(), d.edit_q > 0);
    if queue_pick {
        return super::subbox::run_key(m, &key, &id, k);
    }
    let picked = !run_sel(m, &key).is_empty();
    let conv = tab_name(m, &key, &id) == "conversation";
    let txt_on = open_mut(m, &key).is_some_and(|o| o.txt.on);
    let rows = open_mut(m, &key).map_or(1, |o| o.rows.max(1)) as isize;
    match s {
        "esc" if txt_on && empty => {
            if let Some(o) = open_mut(m, &key) {
                o.txt = Default::default();
            }
            Out::Draw
        }
        "esc" if picked => {
            set_run_sel(m, &key, String::new());
            Out::Draw
        }
        "esc" if editing => super::subbox::run_key(m, &key, &id, k),
        "esc" => {
            close(m, &key, false);
            Out::Draw
        }
        "left" if empty && !picked => {
            close(m, &key, true);
            Out::Draw
        }
        "left" if empty && conv => {
            // ← goes back a stage: what's open round the row, then off it.
            let closed = as_run(m, &key, |m| {
                let rows = conn(m, &key).vs.rows.max(1);
                m.session.as_mut().is_some_and(|v| v.fold_in(rows))
            });
            if closed != Some(true) {
                set_run_sel(m, &key, String::new());
            }
            Out::Draw
        }
        "[" | "]" if empty => {
            let n = tabs(m, &key, &id).len();
            if let Some(o) = open_mut(m, &key) {
                let cur = o.tab.min(n - 1);
                o.tab = if s == "[" {
                    (cur + n - 1) % n
                } else {
                    (cur + 1) % n
                };
                o.top = 0;
            }
            Out::Draw
        }
        "up" if empty
            && !picked
            && conv
            && !super::subbox::queue_of(m, &key, &id).items.is_empty() =>
        {
            super::subbox::run_key(m, &key, &id, k)
        }
        "up" | "down" if empty || picked => {
            if conv {
                return conv_key(m, &key, s).or_idle();
            }
            scroll_tab(m, &key, if s == "up" { -1 } else { 1 })
        }
        "enter" | "space" | " " | "right" if picked && empty && conv => conv_key(m, &key, s),
        "pgup" | "pgdown" => {
            if conv {
                return conv_key(m, &key, s);
            }
            scroll_tab(m, &key, if s == "pgup" { 1 - rows } else { rows - 1 })
        }
        "home" | "end" if empty => {
            if conv {
                return conv_key(m, &key, s);
            }
            scroll_tab(
                m,
                &key,
                if s == "home" {
                    isize::MIN / 2
                } else {
                    isize::MAX / 2
                },
            )
        }
        "ctrl+o" if conv => conv_key(m, &key, s),
        "o" if empty && picked && conv => {
            let u = as_run(m, &key, |m| super::conversation::picked_link(m)).flatten();
            match u {
                Some(u) if rush_ui_cmdbar::linkmenu::open_link_menu(m, &u) => Out::Draw,
                _ => Out::Idle,
            }
        }
        "ctrl+c" | "super+c" if txt_on && (empty || s == "super+c") => {
            let text = as_run(m, &key, |m| {
                let t = conn(m, &key).txt;
                super::textsel::text_of(m, &t)
            })
            .unwrap_or_default();
            if !text.is_empty() {
                m.copy_text(text);
            }
            Out::Draw
        }
        "ctrl+c" | "ctrl+n" | "f1" if empty => Out::Pass,
        "?" if empty => Out::Pass,
        _ => {
            // Typing returns to the box, as in a session.
            if k.text().is_some() && picked {
                set_run_sel(m, &key, String::new());
            }
            let o = super::subbox::run_key(m, &key, &id, k);
            if o.used() { o } else { Out::Idle }
        }
    }
}

/// The pointer while a run is open whole: on the breadcrumb, back to the
/// list; on a tab, that tab; over its conversation, what the session's
/// own conversation does (a link, a row picked or opened, text dragged
/// over, the wheel, the row under it lit); over a tab drawn whole, the
/// wheel. Under it (the box) the session's own clicks don't reach.
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some(id) = open_id(m, &key) else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    let Some((top, off)) = m.session.as_ref().map(|v| (v.top, v.top_off)) else {
        return Out::Pass;
    };
    let (rows, crumb, tab_at, drag) = match open_mut(m, &key) {
        Some(o) => (o.rows.max(1), o.crumb, o.tab_at.clone(), o.txt.drag),
        None => return Out::Pass,
    };
    let name_y = top.checked_sub(HEAD_H - 1 + off);
    let tabs_y = top.checked_sub(HEAD_H - 3 + off);
    if e.kind == MouseKind::Down(Button::Left) {
        if Some(y) == name_y {
            if x >= crumb.0 && x < crumb.1 {
                close(m, &key, true);
                return Out::Draw;
            }
            return Out::Idle;
        }
        if Some(y) == tabs_y {
            let Some(&(_, _, i)) = tab_at.iter().find(|t| x >= t.0 && x < t.1) else {
                return Out::Idle;
            };
            if let Some(o) = open_mut(m, &key) {
                o.tab = i;
                o.top = 0;
            }
            return Out::Draw;
        }
    }
    let in_body = y >= top && y < top + rows;
    let conv = tab_name(m, &key, &id) == "conversation";
    if conv && (in_body || drag) {
        let o = as_run(m, &key, |m| match e.kind {
            MouseKind::ScrollUp | MouseKind::ScrollDown => {
                let up = e.kind == MouseKind::ScrollUp;
                let Some(v) = m.session.as_mut() else {
                    return Out::Idle;
                };
                if up && v.start == 0 || !up && v.pos == Pos::End {
                    return Out::Idle; // a trackpad's momentum at either end
                }
                v.scroll(if up { 3 } else { -3 }, rows);
                Out::Draw
            }
            MouseKind::Moved => {
                let item = if in_body {
                    super::conversation::owner_at(m, y - top)
                } else {
                    String::new()
                };
                let c = conn(m, &key);
                if c.vs.hover_ref == item {
                    return Out::Idle;
                }
                c.vs.hover_ref = item;
                Out::Draw
            }
            _ => {
                let o = super::conversation::mouse(m, e);
                if o.used() {
                    return o;
                }
                super::textsel::mouse(m, e)
            }
        })
        .unwrap_or(Out::Idle);
        return if o.used() { o } else { Out::Idle };
    }
    if in_body {
        return match e.kind {
            MouseKind::ScrollUp => scroll_tab(m, &key, -3),
            MouseKind::ScrollDown => scroll_tab(m, &key, 3),
            _ => Out::Idle,
        };
    }
    Out::Idle
}

/// A child rush hosts, open as a session of its own (spawned::
/// open_session): the way back to the session that started it, over its
/// header's first row. Its parent's key and the run's id there, when it
/// has one open in rush's list.
pub fn parent_of(m: &Model) -> Option<(String, String, String)> {
    let v = m.session.as_ref()?;
    let by = &v.sess.info.spawned_by;
    if by.is_empty() {
        return None;
    }
    let me = m.agents.iter().find(|a| a.key == v.key)?;
    let p = m.agents.iter().find(|a| a.rush && a.job.id == *by)?;
    let name = one_line(&p.display_name);
    Some((p.key.clone(), name, me.job.id.clone()))
}

/// The breadcrumb over a hosted child's own session: `‹ parent › its
/// name`, the parent clickable (crumb_click).
pub fn child_crumb(m: &Model, rows: &mut [Line], w: usize) -> Option<(usize, usize)> {
    let (_, parent, _) = parent_of(m)?;
    let v = m.session.as_ref()?;
    let me = m.agents.iter().find(|a| a.key == v.key)?;
    let hw = w.saturating_sub(3);
    let lead = line([plain("  "), sp("‹ ", SUB), sp(parent, SUB)]);
    let crumb = (2 + 2, 2 + lead.width());
    let mut l = lead;
    l.spans.push(sp(" › ", FAINT));
    l.spans.push(dim(one_line(&me.display_name)));
    l.spans.push(faint("   · subagents"));
    let first = rows.first_mut()?;
    *first = super::views::chrome(rush_draw::truncate_line(&l, hw, "…"), hw);
    Some(crumb)
}

/// A click on a hosted child's breadcrumb: its parent's session, on its
/// Subagents list with the child picked.
pub fn crumb_click(m: &mut Model, e: &Mouse) -> Out {
    if e.kind != MouseKind::Down(Button::Left) || m.confirm.is_some() || m.sheet.is_some() {
        return Out::Pass;
    }
    let Some((top, off)) = m.session.as_ref().map(|v| (v.top, v.top_off)) else {
        return Out::Pass;
    };
    if top.checked_sub(HEAD_H + off) != Some(e.y as usize) {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let (from, to) = conn(m, &key).watch.child_crumb;
    let x = e.x as usize;
    if to == 0 || x < from || x >= to {
        return Out::Pass;
    }
    let Some((parent, _, child)) = parent_of(m) else {
        return Out::Pass;
    };
    super::menubar_jump::jump_to(m, parent.clone());
    if m.session.as_ref().is_some_and(|v| v.key == parent) {
        let run = conn(m, &parent)
            .runs
            .spawns
            .iter()
            .find(|(_, s)| s.hosted == child)
            .map(|(step, _)| format!("{}{step}", crate::spawns::SPAWN_PREFIX));
        match run {
            Some(id) => super::subview::pick_run(m, &parent, &id),
            None => {
                super::views::show_view(m, &parent, "subagents");
            }
        }
    }
    Out::Draw
}
