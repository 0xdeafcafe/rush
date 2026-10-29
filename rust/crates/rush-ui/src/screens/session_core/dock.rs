//! What sits under the open conversation: the messages on their way, the
//! queue, the message box with what enter does on its edge, and the keys
//! for what can be done now on the bottom row. After Go's
//! internal/ui/rushmode.go (dockCard :2162, onBg :2170, spread :2175,
//! paneDock :3209: blocks and room :3225-3250, sendingLines' block :3311,
//! the queue card :3330-3445, room before the box and auxRows :3455,
//! the box's labels :3459-3561, the hint row
//! :3563-3685), sendmode.go:95 (sendModeTop), sessionviews.go:49
//! (picked1) and view.go (keysFit :800, fewKeys :829).
//!
//! Not here yet: the cards (approval, question, limit, sign-in: the
//! cards'), the subagents and jobs on the box or in the column beside a
//! wide conversation (side.rs has its tasks and changes), a room's
//! verdict, the slash
//! picker, images drawn as chips in queued messages and the stash's keys.

use std::time::{Duration, Instant};

use rush_core::adapter::Feature;
use rush_core::text::{Line, Span, Style};

use super::host::{hosts, is_command};
use super::queue::{self, Queued, queue_of};
use super::sending::short_images;
use super::sendmode::{self, BoxState, SEND_MODE, SEND_NOW};
use super::{Core, agent, busy, conn, core, live, open_key, where_};
use crate::frame::InputBox;
use crate::model::Model;
use crate::session::harness_name;
use crate::style::*;

/// The dock's rows start this far in, and stop as far from the right.
pub const INDENT: usize = 2;

/// left and right on one row w wide, apart; left alone when they don't
/// both fit (spread).
pub fn spread(left: Line, right: Line, w: usize) -> Line {
    let gap = w as isize - left.width() as isize - right.width() as isize;
    if gap < 2 {
        return fit(left, w);
    }
    let mut out = left;
    out.spans.push(plain(blanks(gap as usize)));
    join(&mut out, right);
    out
}

/// Rows w wide on a ground of their own, edged down the left in the
/// colour that says what they are (dockCard).
pub fn dock_card(edge: rush_core::text::Color, rows: Vec<Line>, w: usize) -> Vec<Line> {
    rows.into_iter()
        .map(|r| {
            let mut l = line([sp("▌", edge)]);
            join(&mut l, r);
            on_ground(l, w, CHROME_BG)
        })
        .collect()
}

/// A row drawn as picked: the selection's ground, a bar down its left,
/// orange while the keys are there (picked1).
pub fn picked1(row: Line, w: usize, focused: bool) -> Line {
    let bar = if focused { ORANGE } else { FAINT };
    let rest = rush_draw::truncate_line(&fit(row, w), w, "");
    let mut l = line([sp("▍", bar)]);
    // The row's first cell gives way to the bar.
    let mut cut = Line::new();
    let mut skipped = false;
    for s in rest.spans {
        if !skipped && !s.text.is_empty() {
            let mut cs = s.text.chars();
            cs.next();
            skipped = true;
            cut.spans.push(Span::new(cs.as_str().to_string(), s.style));
            continue;
        }
        cut.spans.push(s);
    }
    join(&mut l, cut);
    on_ground(l, w, SEL_BG)
}

/// "key what" pairs, keys as the keyboard names them (keys).
pub fn pairs_line(pairs: &[(&str, &str)]) -> Line {
    let words: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, w)| (rush_convo::render::key_word(k), w.to_string()))
        .collect();
    let refs: Vec<(&str, &str)> = words
        .iter()
        .map(|(k, w)| (k.as_str(), w.as_str()))
        .collect();
    keys(&refs)
}

/// The first n of pairs, then always, fitted to w: a row of keys says
/// what matters now and leaves the rest to the help (fewKeys).
pub fn few_keys(w: usize, n: usize, pairs: &[(&str, &str)], always: &[(&str, &str)]) -> Line {
    let mut all: Vec<(&str, &str)> = pairs.iter().take(n).copied().collect();
    all.extend_from_slice(always);
    fit_keys(w, &all)
}

/// keysFit with the keys as the keyboard names them.
pub fn fit_keys(w: usize, pairs: &[(&str, &str)]) -> Line {
    let words: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, w)| (rush_convo::render::key_word(k), w.to_string()))
        .collect();
    let refs: Vec<(&str, &str)> = words
        .iter()
        .map(|(k, w)| (k.as_str(), w.as_str()))
        .collect();
    keys_fit(w, &refs)
}

/// The rows under the open conversation, the screen's width w: the
/// session_dock hook.
pub fn dock(m: &mut Model, w: usize) -> Vec<Line> {
    live::sync(m);
    queue::flush(m);
    let Some(key) = open_key(m) else {
        return Vec::new();
    };
    // Go's pane is the screen less 3, from column 2 (view.go:1210); the
    // box gives the map its width from 96 wide (rushmode.go:3224).
    let w = w.saturating_sub(INDENT + 1).max(20);
    let w = if super::conversation::map_on(m, &key, w) {
        w - super::minimap::MINI_WIDTH
    } else {
        w
    };
    let mut out: Vec<Line> = Vec::new();
    let mut blocks = 0;
    let mut blocks_n = 0;
    // Each part is its own block, a row of ground between one and the
    // next; in a conversation that row is the conversation's.
    let block = |out: &mut Vec<Line>, blocks: &mut usize| {
        if *blocks > 0 {
            out.push(Line::new());
        }
        *blocks += 1;
    };
    // A running block: its rows on a card edged in its colour, each row's
    // ref kept for clicks and ↑.
    fn run_block(
        out: &mut Vec<Line>,
        n: &mut usize,
        rows: &mut (usize, Vec<String>),
        b: super::running::Block,
        w: usize,
    ) {
        if *n > 0 {
            out.push(Line::new());
            rows.1.push(String::new());
        }
        *n += 1;
        if rows.1.is_empty() {
            rows.0 = out.len();
        }
        out.extend(dock_card(b.edge, b.rows, w));
        rows.1.extend(b.refs);
    }

    // What the session runs: its subagents and workflows, then the tasks
    // none of them started; beside a wide conversation, in the column
    // there, else each a block of its own (paneDock's panels :3488).
    super::running::sync(m, &key);
    let in_panel = m.zen().is_none()
        && crate::screens::plugin_ui::views::beside::subs_shown_in_panel(m, &key, w);
    let sw = super::side::column_width(m, &key, w);
    let runs = super::running::blocks(m, &key, if sw > 0 { sw - 1 } else { w - 1 }, in_panel);
    let mut run_rows = (0, Vec::new());
    if sw > 0 {
        conn(m, &key).side_runs = runs;
    } else {
        conn(m, &key).side_runs.clear();
        for b in runs {
            run_block(&mut out, &mut blocks_n, &mut run_rows, b, w);
        }
    }

    // The messages on their way.
    let (tick, now) = (m.tick, m.now);
    let queued = queue_of(m, &key);
    if let Some(v) = m.session.as_ref() {
        let c = m.kept.get::<Core>().conns.entry(key.clone()).or_default();
        c.out.settle(&v.sess, queued.items.len(), now);
    }
    blocks += blocks_n;
    conn(m, &key).run_rows = run_rows;
    let sending = conn(m, &key).out.lines(tick, w);
    if !sending.is_empty() {
        block(&mut out, &mut blocks);
        out.extend(sending.into_iter().map(|l| on_ground(l, w, CHROME_BG)));
        // The spinner turns, and a message waited on too long goes.
        m.ask_wake(Instant::now() + Duration::from_secs(1));
    }

    // The queue, last before the box.
    let picked = m
        .session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    let typed = !conn(m, &key).input.is_empty();
    let (mut q_rows, mut q_off) = (Vec::new(), 0);
    if !queued.items.is_empty() {
        block(&mut out, &mut blocks);
        let when = queue::when(m, &key, &queued);
        let (rows, at) = queue_rows(&queued, when, &picked, typed, w - 1);
        q_off = out.len();
        q_rows = at;
        out.extend(dock_card(QUEUE, rows, w));
    }
    // And room before the box. In a conversation one row scrolls with the
    // transcript, so it never ends flush against the box, and one stays
    // put, so scrolled up it never runs into what's stacked on the box.
    // Until the conversation is read, what's in its place isn't one: the
    // panels stay in the dock.
    let scrolls = super::views::view_name(m, &key) == "conversation"
        && m.session.as_ref().is_some_and(|v| v.read);
    let pad = if scrolls { 2 } else { blocks.min(1) };
    for _ in 0..pad {
        out.push(Line::new());
    }
    // In a conversation all but that last row go on the transcript's end
    // and scroll with it (paneDock's auxRows, rushmode.go:2216).
    let mut panels = Vec::new();
    if scrolls {
        let n = out.len() - 1;
        panels = out.drain(..n).collect();
    }
    let aux = panels.len();
    // The runs' rows keep their refs there, for clicks.
    let mut panel_refs = vec![String::new(); aux];
    {
        let (at, refs) = &conn(m, &key).run_rows;
        for (i, r) in refs.iter().enumerate() {
            if let Some(p) = panel_refs.get_mut(at + i) {
                p.clone_from(r);
            }
        }
    }
    if let Some(v) = m.session.as_mut() {
        v.panels = panels;
        v.panel_refs = panel_refs;
    }

    // With room right of the conversation's text the tasks go in the
    // column there; else they sit on the box, one row until opened.
    let conversation = super::views::view_name(m, &key) == "conversation";
    let beside = super::side::make(m, &key, w, conversation);
    let shut = !conn(m, &key).tasks_open;
    let tasks = m
        .session
        .as_ref()
        .filter(|_| !beside)
        .map(|v| super::tasks::task_row(&v.sess, w, shut))
        .unwrap_or_default();
    let c = conn(m, &key);
    (c.task_rows, c.task_w) = (tasks.len(), w);
    out.extend(tasks);

    let b = message_box(m, &key, w, &picked);
    let c = conn(m, &key);
    c.box_off = out.len();
    c.q_off = q_off;
    c.q_rows = q_rows;
    c.q_panel = scrolls && aux > 0;
    out.extend(b.lines());
    c.drawn = b;

    let hint = hint(m, &key, w, &picked);
    core(m).hint = Some(hint);
    conn(m, &key).dock_rows = out.len();
    out.into_iter()
        .map(|l| {
            let mut r = line([plain(blanks(INDENT))]);
            join(&mut r, l);
            r
        })
        .collect()
}

/// The queue card's rows, w wide, and the message on each (the queue's
/// part of paneDock).
fn queue_rows(
    q: &Queued,
    when: Line,
    picked: &str,
    typed: bool,
    w: usize,
) -> (Vec<Line>, Vec<Option<usize>>) {
    let mut rows = Vec::new();
    let mut at = Vec::new();
    let pick = queue::sel(picked, q.items.len());
    let title = line([
        plain(" "),
        spb("queue", QUEUE),
        sp(format!(" ·{}", q.items.len()), FAINT),
    ]);
    // Hovering a row only lights it; a click picks it, which is when its
    // own keys show.
    let mut how = Line::new();
    if pick.is_none() && !typed {
        how = pairs_line(&[("↑", "edit, reorder or steer"), (SEND_NOW, "send now")]);
        how.spans.push(plain("  "));
    }
    // One queued: it shares the title's row.
    let single = q.items.len() == 1;
    if !single {
        let mut head = title.clone();
        join(&mut head, when.clone());
        rows.push(spread(head, how.clone(), w));
        at.push(None);
    }
    // Three at a time, keeping the picked one in sight.
    let start = match pick {
        Some(p) if p >= 3 => p - 2,
        _ => 0,
    };
    if start > 0 {
        rows.push(line([sp(format!("  … {start} before"), DIM)]));
        at.push(None);
    }
    let end = q.items.len().min(start + 3);
    for i in start..end {
        let text = short_images(&one_line(&q.items[i]));
        let text = rush_draw::truncate(&text, 10.max(w.saturating_sub(8)), "…").into_owned();
        let body = if is_command(&q.items[i]) {
            // A command runs alone, as a command, not words.
            line([sp("⌘ ", QUEUE), Span::new(text, Style::fg(GREEN).bold())])
        } else if i > 0 && is_command(&q.items[i - 1]) {
            line([sp("once it's done ", FAINT), sp(text, TEXT)])
        } else {
            line([sp(text, TEXT)])
        };
        let mut row = line([plain("  "), sp((i + 1).to_string(), SUB), plain("  ")]);
        join(&mut row, body.clone());
        let mut row = rush_draw::truncate_line(&row, w, "…");
        if single {
            let mut right = how.clone();
            if right.spans.is_empty() {
                right = when.clone();
                if let Some(s) = right.spans.first_mut() {
                    s.text = s.text.replacen(" · ", "", 1);
                }
                right.spans.push(plain("  "));
            }
            if w.saturating_sub(right.width()) < 40 {
                right = Line::new();
            }
            let mut left = title.clone();
            left.spans.push(plain("  "));
            join(&mut left, body);
            let room = w.saturating_sub(right.width() + 2);
            row = spread(rush_draw::truncate_line(&left, room, "…"), right, w);
        }
        if pick == Some(i) {
            row = picked1(row, w, true);
        }
        rows.push(row);
        at.push(Some(i));
    }
    let rest = q.items.len().saturating_sub(start + 3);
    if rest > 0 {
        rows.push(line([sp(format!("  + {rest} more"), DIM)]));
        at.push(None);
    }
    (rows, at)
}

/// The message box, w wide, with who it goes to and what enter does on
/// its edge (paneDock's box).
fn message_box(m: &mut Model, key: &str, w: usize, picked: &str) -> InputBox {
    let a = agent(m, key).cloned();
    let h = hosts(m);
    let c = conn(m, key);
    let hosted = c.hosted();
    let typed = !c.input.is_empty();
    let text = c.input.string();
    let edit_q = c.edit_q;
    let mode = c.info.permission_mode.clone();
    let (sess_live, question, pending) = match m.session.as_ref() {
        Some(v) => {
            let p = v.sess.pending();
            let q = p.iter().any(|&s| {
                v.sess
                    .step(s)
                    .approval
                    .as_ref()
                    .is_some_and(|a| a.question.is_some())
            });
            (v.sess.live().is_some(), q, !p.is_empty())
        }
        None => (false, false, false),
    };
    let mut top = Line::new();
    let add = |top: &mut Line, l: Line| {
        if !top.spans.is_empty() {
            top.spans.push(sp(" · ", DIM));
        }
        join(top, l);
    };
    if text.trim_start().starts_with('#') {
        top = line([sp("rush command · enter runs it", DIM)]);
    } else if let Some(a) = a.as_ref().filter(|_| !hosted) {
        if a.interactive {
            add(
                &mut top,
                line([sp(
                    format!("{}, so it can't take messages here", where_(a)),
                    DIM,
                )]),
            );
        } else if a.past {
            add(
                &mut top,
                line([
                    sp("enter resumes it", ORANGE),
                    sp(" in rush mode with your message", DIM),
                ]),
            );
        } else if h.hosts(a) {
            add(
                &mut top,
                line([
                    sp("stopped; ", DIM),
                    sp("enter resumes it", ORANGE),
                    sp(" with your message", DIM),
                ]),
            );
        } else if busy(a) {
            add(&mut top, line([sp("queuing", DIM)]));
            if typed {
                add(
                    &mut top,
                    line([sp(format!("{SEND_NOW} sends it now"), DIM)]),
                );
            }
        } else {
            let harness = harness_name(&m.reg, &a.kind);
            add(
                &mut top,
                line([sp(format!("enter replies through {harness}"), DIM)]),
            );
        }
    } else if hosted && question {
        add(
            &mut top,
            line([sp(
                "pick above, or type your own answer · enter sends it",
                YELLOW,
            )]),
        );
    } else if hosted && pending {
        add(
            &mut top,
            line([sp(
                "y · a · n answer the card · or type why not, enter denies with it",
                YELLOW,
            )]),
        );
    } else if hosted && sess_live && typed {
        // How it sends, once there's something to (sendModeTop).
        let kind = a
            .as_ref()
            .map(|a| a.kind.as_str().to_string())
            .unwrap_or_default();
        let modes = sendmode::modes(&m.reg, &kind);
        let picked_mode = core(m).send_modes.get(key).copied();
        let cur = sendmode::mode_of(picked_mode, &modes);
        let next = sendmode::next_mode(picked_mode, &modes);
        let mut l = line([sp(cur.state(), DIM)]);
        if next != cur {
            l.spans
                .push(sp(format!(" · {SEND_MODE} {}", next.switch()), DIM));
        }
        add(&mut top, l);
    }
    let typing = picked.is_empty();
    if !typing {
        top = line([sp("typing returns here · ↓ past the last row or esc", DIM)]);
    }
    if edit_q > 0 {
        top = line([
            sp(format!("editing queued message {edit_q}"), ORANGE),
            sp(" · enter saves it back · esc cancels", DIM),
        ]);
    }
    let c = conn(m, key);
    let mut b = InputBox {
        w,
        focused: typing,
        top_l: top,
        top_r: if mode.is_empty() {
            Line::new()
        } else {
            line([sp(mode, ORANGE)])
        },
        text: c.input.text.clone(),
        cursor: c.input.pos,
        anchor: c.input.anchor,
        holder: "a message for this agent".into(),
        lead: line([sp("❯ ", ORANGE)]),
        max_rows: 6,
        top: c.drawn.top,
        ..InputBox::default()
    };
    b = b.scrolled();
    b
}

/// The keys for what can be done now, the box's own first; the way back
/// goes last, and stays (paneDock's hint).
fn hint(m: &mut Model, key: &str, w: usize, picked: &str) -> Line {
    let w = w.saturating_sub(4);
    let q = queue_of(m, key);
    if queue::sel(picked, q.items.len()).is_some() {
        return queue::hint(&q, w);
    }
    let view = super::views::view_name(m, key);
    if !picked.is_empty() {
        // The picked row's keys (paneDock's hint :3608-3660), those whose
        // keys rush has: the ctrl+] chords (open all, close older, rewind,
        // fork, the file in full) and editing memory in place aren't wired
        // yet.
        let turn = picked.split(':').next().is_some_and(|t| {
            t.len() > 1 && t.starts_with('t') && t[1..].bytes().all(|b| b.is_ascii_digit())
        });
        let remember = crate::screens::cmdbar::keybind::bound_key(m, "session.remember");
        let pairs: &[(&str, &str)] = if picked == "go:memory" {
            &[
                ("space", "open the Memory tab"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if view == "changes" && (picked.starts_with("chg:") || picked.starts_with("tree:")) {
            &[
                ("enter", "open or close"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if picked.starts_with("mem:") {
            &[
                ("+", "remember"),
                ("r", "review"),
                ("f", "tidy"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if turn {
            &[
                ("enter", "open or close"),
                ("esc", "back to message"),
                (remember.as_str(), "remember"),
            ]
        } else {
            &[
                ("enter", "open or close"),
                ("↑↓", "pick a step"),
                ("esc", "back to message"),
                ("ctrl+o", "show all"),
            ]
        };
        return fit_keys(w, pairs);
    }
    if view == "memory" && conn(m, key).input.is_empty() {
        return fit_keys(
            w,
            &[
                ("↑↓", "pick a file"),
                ("#remember", "keep something"),
                ("[ ]", "views"),
            ],
        );
    }
    let a = agent(m, key).cloned();
    let now = m.now;
    let live_turn = m.session.as_ref().is_some_and(|v| v.sess.live().is_some());
    let turns = m.session.as_ref().is_some_and(|v| !v.sess.turns.is_empty());
    let kind = a
        .as_ref()
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let modes = sendmode::modes(&m.reg, &kind);
    let picked_mode = core(m).send_modes.get(key).copied();
    let rewinds = m.reg.supports(&kind, Feature::Rewind);
    let c = conn(m, key);
    let live = c.hosted() && live_turn;
    let st = BoxState {
        live,
        working: live || !c.hosted() && a.as_ref().is_some_and(busy),
        typed: !c.input.is_empty(),
        queued: !q.items.is_empty(),
        picked: picked_mode,
        mode: sendmode::mode_of(picked_mode, &modes),
        cleared: c.cleared_at > 0 && now - c.cleared_at < 120_000,
    };
    let mut pairs = sendmode::box_keys(&st);
    if turns {
        pairs.push((
            "↑",
            if c.hosted() && rewinds {
                "rewind"
            } else {
                "steps"
            },
        ));
    }
    few_keys(w, 3, &pairs, &[("f1", "more"), ("esc", "back")])
}

/// The conversation's rows on screen as last drawn.
fn conn_rows(m: &mut Model, key: &str) -> usize {
    m.session
        .as_ref()
        .map_or(0, |v| v.shown.len())
        .min(conn(m, key).vs.rows)
}

/// The bottom row while a Session shows: its hint (the status hook).
pub fn status(m: &Model, _w: usize) -> Option<Line> {
    m.session.as_ref()?;
    let h = m.kept.peek::<Core>()?.hint.clone()?;
    // "  "+hint, in the pane two in (paneDock's last row).
    let mut l = line([plain(blanks(INDENT + 2))]);
    join(&mut l, h);
    Some(l)
}

/// Finds where the box landed on the finished frame, for clicks (an
/// overlay that draws nothing).
#[allow(clippy::ptr_arg)] // the overlay hook's shape
pub fn locate(m: &mut Model, out: &mut Vec<Line>) {
    let Some(key) = open_key(m) else {
        return;
    };
    if m.on_place() {
        return;
    }
    let c = conn(m, &key);
    let Some(edge) = c.drawn.lines().first().map(Line::plain) else {
        return;
    };
    let found = out.iter().rposition(|l| {
        l.plain()
            .get(INDENT..)
            .is_some_and(|p| p.starts_with(&edge))
    });
    c.box_y = found;
    if c.q_panel {
        // On the transcript's end: where that is on screen, if it is.
        let Some(v) = m.session.as_mut() else {
            return;
        };
        let (top, start) = (v.top, v.start);
        let t = v.r.count(&v.sess, &v.o);
        let rows = conn_rows(m, &key);
        let c = conn(m, &key);
        c.q_top = (top + t + c.q_off).wrapping_sub(start);
        c.q_shown = (top, top + rows);
        return;
    }
    if let Some(y) = found {
        c.q_top = (y + c.q_off).saturating_sub(c.box_off);
        c.q_shown = (0, usize::MAX);
    }
}
