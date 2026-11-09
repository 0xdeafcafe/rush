//! What sits under the open conversation: the messages on their way, the
//! queue, the message box with what enter does on its edge, and the keys
//! for what can be done now on the bottom row. After Go's
//! internal/ui/rushmode.go (dockCard :2162, onBg :2170, spread :2175,
//! paneDock :3209: blocks and room :3225-3250, sendingLines' block :3311,
//! the queue card :3330-3445, room before the box and auxRows :3455,
//! the box's labels :3459-3561, the hint row
//! :3563-3685), sendmode.go:95 (sendModeTop), sessionviews.go:49
//! (picked1), :3729 (slashLines, slash.rs) and view.go (keysFit :800, fewKeys :829).
//!
//! The cards (approval, question, limit, sign-in: session_cards') sit
//! last above the box, scrolling with a conversation as Go's panels do.
//!
//! Not here yet: the subagents and jobs on the box or in the column beside a
//! wide conversation (side.rs has its tasks and changes), a room's
//! verdict, images drawn as chips in queued messages and the stash's keys.

use std::time::{Duration, Instant};

use rush_core::adapter::Feature;
use rush_core::text::{Line, Span, Style};

use super::host::is_command;
use super::queue::{self, Queued, queue_of};
use super::sending::short_images;
use super::sendmode::{self, BoxState, SEND_MODE, SEND_NOW};
use super::{Core, agent, busy, conn, core, live, open_key, where_};
use crate::frame::InputBox;
use crate::model::Model;
use crate::session::harness_name;
use crate::style::*;

// Shared with the other screens: the base's (crate::paint).
pub use crate::paint::{fit_keys, pairs_line, spread};

/// The dock's rows start this far in, and stop as far from the right.
pub const INDENT: usize = 2;

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

/// The first n of pairs, then always, fitted to w: a row of keys says
/// what matters now and leaves the rest to the help (fewKeys).
pub fn few_keys(w: usize, n: usize, pairs: &[(&str, &str)], always: &[(&str, &str)]) -> Line {
    let mut all: Vec<(&str, &str)> = pairs.iter().take(n).copied().collect();
    all.extend_from_slice(always);
    fit_keys(w, &all)
}

/// The rows under the open conversation, the screen's width w: the
/// session_dock hook.
pub fn dock(m: &mut Model, w: usize) -> Vec<Line> {
    live::sync(m);
    queue::flush(m);
    let Some(key) = open_key(m) else {
        return Vec::new();
    };
    // Its shell calls run as scripts, read again as they may have moved
    // on (rushmode.go:2404's scriptViews, once a frame).
    super::scripts::sync(m, &key);
    // What its running Bash calls are doing now, from their processes
    // and the files they write.
    super::livestep::sync(m, &key);
    // The commands rush runs itself: their output so far, how they ended.
    super::shellbox::sync(m, &key);
    // Its background tasks a plugin has a card for (a rush watch): drawn
    // in the conversation as they stand.
    super::taskcards::sync(m, &key);
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
    super::agentrows::sync(m, &key);
    let in_panel = rush_ui_plugin_ui::views::beside::subs_shown_in_panel(m, &key, w);
    let sw = super::side::column_width(m, &key, w);
    let run_w = if sw > 0 {
        super::side::text_width_with(sw, m.cfg.column_width)
    } else {
        w - 1
    };
    let runs = super::running::blocks(m, &key, run_w, in_panel);
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
    // The queue and a waiting card sit last, just above the box; a room
    // running on the chat before them (paneDock :3513).
    // The views beside the conversation take their room of its width; the
    // card stays inside what's left, so they never lie over its words.
    let vw = rush_ui_plugin_ui::views::beside::views_beside(m, &key, w).2;
    let (room_rows, card_rows) =
        rush_ui_session_cards::dock_parts(m, w.saturating_sub(vw + 1).max(20));
    if !room_rows.is_empty() {
        block(&mut out, &mut blocks);
        out.extend(room_rows);
    }
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
        let send_now = super::spawned::send_now_key(m);
        let (rows, at) = queue_rows(&queued, when, &picked, typed, &send_now, w - 1);
        q_off = out.len();
        q_rows = at;
        out.extend(dock_card(QUEUE, rows, w));
    }
    // A card waiting on you, last, just above the box: the first stop
    // for ↑ (paneDock's cards :3412).
    let mut card_off = None;
    if !card_rows.is_empty() {
        block(&mut out, &mut blocks);
        card_off = Some(out.len());
        out.extend(card_rows);
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
    if let Some(at) = card_off {
        rush_ui_session_cards::card_at(m, &key, at, scrolls);
    }
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
    // The picker over the box while a command is typed (slashLines).
    out.extend(super::slash::slash_lines(m, &key, w));
    let c = conn(m, &key);
    (c.task_rows, c.task_w) = (tasks.len(), w);
    out.extend(tasks);

    let b = message_box(m, &key, w, &picked);
    // The run's box beside the list has the keys: it's the one box live,
    // the session's own not drawn under it to say otherwise (Rust-only).
    let run_box = conn(m, &key).watch.msg_focus
        && super::views::view_name(m, &key) == "subagents"
        && super::subview::box_run(m, &key).is_some();
    let c = conn(m, &key);
    c.box_off = out.len();
    c.q_off = q_off;
    c.q_rows = q_rows;
    c.q_panel = scrolls && aux > 0;
    if !run_box {
        out.extend(b.lines());
    }
    c.drawn = b;

    let hint = if m.pane_idle && !(m.ext.calls.hosted_alone())(m) {
        // Beside the list without the keys: how to give them to it
        // (paneDock's !paneFocus, rushmode.go:3848).
        fit_keys(
            w.saturating_sub(4),
            &[("enter · →", "type here"), ("ctrl+n", "next needing you")],
        )
    } else if (m.ext.calls.hosted_away())(m) {
        away_hint(m, &key, w)
    } else {
        hint(m, &key, w, &picked)
    };
    crate::sess::state(m).hint = Some(hint);
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
/// The queue card's rows, w wide: its title and when it goes, three of
/// its messages at a time, the picked one lit; and which message each
/// row is (the subagent box's card is this too).
pub fn queue_rows(
    q: &Queued,
    when: Line,
    picked: &str,
    typed: bool,
    send_now: &str,
    w: usize,
) -> (Vec<Line>, Vec<Option<usize>>) {
    let mut rows = Vec::new();
    let mut at = Vec::new();
    let pick = queue::sel(picked, q.items.len());
    let mut title = line([
        plain(" "),
        spb("queue", QUEUE),
        sp(format!(" ·{}", q.items.len()), FAINT),
    ]);
    // Held: said on the card's title, and on each row you hold.
    if q.held {
        title.spans.push(sp(" ⏸ held · alt+h lets it go", YELLOW));
    } else if q.guarded {
        title
            .spans
            .push(sp(" ⏸ held by the guard · alt+h lets it through", YELLOW));
    }
    // Hovering a row only lights it; a click picks it, which is when its
    // own keys show.
    let mut how = Line::new();
    if pick.is_none() && !typed {
        how = pairs_line(&[("↑", "edit, reorder or steer"), (send_now, "send now")]);
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
        if q.held {
            row.spans.push(sp("⏸ ", YELLOW));
        }
        join(&mut row, body.clone());
        let mut row = rush_draw::truncate_line(&row, w, "…");
        if single {
            let mut right = how.clone();
            if right.spans.is_empty() {
                right = when.clone();
                if let Some(s) = right.spans.first_mut() {
                    s.text = s.text.replacen(" · ", "", 1).into();
                }
                right.spans.push(plain("  "));
            }
            if w.saturating_sub(right.width()) < 40 {
                right = Line::new();
            }
            let mut left = title.clone();
            left.spans.push(plain("  "));
            if q.held {
                left.spans.push(sp("⏸ ", YELLOW));
            }
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
    let send_now = super::spawned::send_now_key(m);
    // The cards' word on the box: a decisions card waiting (in your own
    // words or not), the card with the keys, the side question asked or
    // a memory file edited above (rushmode.go:3687-3712).
    let (deciding, own, card_focus) = cards_now(m, key);
    let btw_on = rush_ui_session_cards::btw::btw_for(m, key).is_some_and(|t| t.focused);
    let mem_edit = super::memedit::editing(m, key)
        .then(|| super::memedit::picked_path(m, key))
        .flatten();
    // A room's pane is its row's, as Go's room rows are agents.
    let a = crate::rooms::row_agent(m, key).cloned();
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
    // Where the send mode is on the top edge's left label, while it's
    // the label's.
    let mut send_part: Option<(usize, usize)> = None;
    let mut send_top = String::new();
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
        } else if a.rush {
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
                    line([sp(format!("{send_now} sends it now"), DIM)]),
                );
            }
        } else {
            // Go says Claude Code whatever the agent (rushmode.go:3684); a
            // room has no harness of its own, so it keeps Go's words.
            let harness = if crate::rooms::is_room_key(&a.key) {
                "Claude Code".to_string()
            } else {
                harness_name(&m.reg, &a.kind)
            };
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
    } else if hosted && deciding && own {
        add(
            &mut top,
            line([sp("type your answer · enter gives it", YELLOW)]),
        );
    } else if hosted && deciding {
        add(
            &mut top,
            line([
                sp("↑ to decide above", YELLOW),
                sp(" · or type a message as ever", DIM),
            ]),
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
            // A click on it goes round as ctrl+t does.
            let at = if top.spans.is_empty() {
                0
            } else {
                top.width() + 3
            };
            send_part = Some((at, at + l.width()));
        }
        add(&mut top, l);
        send_top = top.plain();
    }
    let away = (m.ext.calls.hosted_away())(m) || m.pane_idle;
    let typing = picked.is_empty() && !away && !card_focus && !btw_on;
    if btw_on {
        top = line([sp("asking on the side, above · esc returns here", DIM)]);
    } else if card_focus {
        top = line([sp("answering the card above · esc returns here", DIM)]);
    } else if let Some(path) = &mem_edit {
        let name = std::path::Path::new(path)
            .file_name()
            .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
        top = line([sp(format!("editing {name} above · esc returns here"), DIM)]);
    } else if !picked.is_empty() {
        top = line([sp("typing returns here · ↓ past the last row or esc", DIM)]);
    }
    if edit_q > 0 {
        top = line([
            sp(format!("editing queued message {edit_q}"), ORANGE),
            sp(" · enter saves it back · esc cancels", DIM),
        ]);
    }
    // A stop waiting on why (rushmode.go:3733).
    if let Some(n) = m
        .kept
        .peek::<rush_ui_session_cards::Cards>()
        .and_then(|c| c.conns.get(key))
        .and_then(|c| c.stop_note.as_ref())
    {
        let what = if n.key == "k" {
            "kills that command"
        } else {
            "stops it"
        };
        top = line([
            sp("why? told to the agent", ORANGE),
            sp(format!(" · enter {what} and sends this · esc cancels"), DIM),
        ]);
    }
    // A question asked on this box: its top line (rushmode.go:3740).
    if let Some(q) = m.confirm.as_mut().filter(|q| q.box_key == key) {
        top = q.box_line();
        q.inlined = true;
    }
    // What plugins put on the box's edge (rushmode.go:3728 boxNote).
    let foot_r = rush_ui_plugin_ui::hooks::box_note(m, key);
    // With the stash running, the empty box names its history
    // (rushmode.go:3717, sendmode.go:154 historyHint).
    let history_hint = history_hint(m);
    // Any of the words above that took the label's place took the send
    // mode's with it.
    let send_part = send_part.filter(|_| top.plain() == send_top);
    // A decision held off the box while you type, or what you typed set
    // aside while you answer one (hold.rs).
    let foot_l = rush_ui_session_cards::hold::notice(m, key);
    let c = conn(m, key);
    let mut b = InputBox {
        w,
        foot_l,
        foot_r,
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
        holder: if away {
            // The keys off the box (paneDock, rushmode.go:3855).
            "enter or → to talk to this agent".into()
        } else {
            format!("a message for this agent{history_hint}")
        },
        lead: line([sp("❯ ", ORANGE)]),
        max_rows: 6,
        top: c.drawn.top,
        // The paste chip under the pointer (rushmode.go:3718).
        hot: c.input.hot,
        ..InputBox::default()
    };
    // A dictation under way shows at the cursor (rushmode.go:3722).
    if typing && m.sheet.is_none() && m.confirm.is_none() {
        b.live = rush_ui_social::dictation::look(m);
    }
    // After `!`, what's typed is a shell command rush runs here: the box
    // says so in orange (shellbox.rs).
    let bang = conn(m, key).bang;
    if bang {
        let dir = crate::style::tildify(&super::shellbox::folder(m, key));
        b.top_l = line([sp("! shell", ORANGE), sp(format!(" · {dir}"), TEXT)]);
        b.top_r = line([sp("enter runs · esc back", DIM)]);
        b.lead = line([sp("! ", ORANGE)]);
        b.holder = "a command to run in this folder".into();
        b.edge = Some(ORANGE);
    }
    super::emeritus::retired_box(m, key, &mut b);
    b = b.scrolled();
    // What a click takes on the top edge, where the edge drew it: the
    // permission mode on its right opens what changes it (as #perm),
    // the send mode on its left goes round (as ctrl+t).
    let at = b.edge_at(false);
    let mut hits = if bang {
        Vec::new()
    } else {
        at.place(true, &[(0, b.top_r.width(), EdgeAct::Mode)])
    };
    if let Some((f, t)) = send_part.filter(|_| !bang) {
        hits.extend(at.place(false, &[(f, t, EdgeAct::SendMode)]));
    }
    let c = conn(m, key);
    c.edge_lit = c.edge_lit.filter(|l| hits.iter().any(|h| (h.0, h.1) == *l));
    b.lit = c.edge_lit.map(|(f, t)| (false, f, t));
    c.edge_hits = hits;
    b
}

/// What a click on a part of the message box's top edge does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeAct {
    /// The permission mode: what changes it (#perm).
    Mode,
    /// How a message goes: the next way (ctrl+t).
    SendMode,
}

/// Hosted with the keys off the box: how to get them back, stop a turn
/// and quit (paneDock, rushmode.go:3848-3854).
fn away_hint(m: &mut Model, key: &str, w: usize) -> Line {
    let w = w.saturating_sub(4);
    let kind = agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let live = m.session.as_ref().is_some_and(|v| v.sess.live().is_some());
    let stops = conn(m, key).hosted() && live && m.reg.supports(&kind, Feature::Interrupt);
    if stops {
        fit_keys(
            w,
            &[
                ("enter · →", "type here"),
                ("esc", "stop the turn"),
                ("ctrl+q", "quit"),
            ],
        )
    } else {
        fit_keys(w, &[("enter · →", "type here"), ("ctrl+q", "quit")])
    }
}

/// The keys for what can be done now, the box's own first; the way back
/// goes last, and stays (paneDock's hint).
fn hint(m: &mut Model, key: &str, w: usize, picked: &str) -> Line {
    let w = w.saturating_sub(4);
    let send_now = super::spawned::send_now_key(m);
    let q = queue_of(m, key);
    if queue::sel(picked, q.items.len()).is_some() {
        return queue::hint(&q, w);
    }
    // The column beside the conversation has the keys.
    if super::side::focused(m, key) {
        return fit_keys(
            w,
            &[
                ("↑↓", "pick"),
                ("enter · →", "open"),
                ("←", "back"),
                ("esc", "back"),
            ],
        );
    }
    // The picked run's box in the subagents view has the keys.
    if conn(m, key).watch.msg_focus
        && super::views::view_name(m, key) == "subagents"
        && let Some(id) = super::subview::box_run(m, key)
    {
        return super::subbox::hint(m, key, &id, w);
    }

    let view = super::views::view_name(m, key);
    // The conversation's view: a turn or a step opens in Normal, and the
    // key to the next view is said while one other than Normal shows.
    let preset = super::presets::current(m);
    let preset_key = super::presets::key_hint(m);
    let next_view = "views";
    // The views' dropdown under the Conversation tab has the keys.
    if view == "conversation" && super::presets::menu_open(m, key) {
        return fit_keys(
            w,
            &[("↑↓", "choose"), ("enter", "show it"), ("esc", "close")],
        );
    }
    let other_view = view == "conversation" && preset != rush_convo::Preset::Normal;
    // A view rush suggests, or switched to, and the key that takes it.
    if view == "conversation"
        && picked.is_empty()
        && let Some(l) = super::viewsuggest::hint(m, key, w)
    {
        return l;
    }
    if other_view && super::presets::turn_or_step(picked) {
        return fit_keys(
            w,
            &[
                ("enter", "open in Normal"),
                ("↑↓", "pick"),
                (preset_key.as_str(), next_view),
                ("esc", "back to message"),
            ],
        );
    }
    if let Some(h) = super::shellbox::hint(m, key, w, picked) {
        return h;
    }
    if !picked.is_empty() {
        // The picked row's keys (paneDock's hint, rushmode.go:3792-3830).
        let turn = picked.split(':').next().is_some_and(|t| {
            t.len() > 1 && t.starts_with('t') && t[1..].bytes().all(|b| b.is_ascii_digit())
        });
        // A run picked: see it in Subagents, or message it there; a
        // running command picked in the conversation: x stops it
        // (rushmode.go:3800-3813).
        let sub = (picked.starts_with("run:") || picked.starts_with("sub:"))
            .then(|| super::jobkeys::picked_sub(m, key))
            .flatten();
        let shell_id = if picked.contains(":s:") {
            super::jobkeys::picked_shell(m, key)
        } else {
            String::new()
        };
        let shell = !shell_id.is_empty();
        // What k kills and p does, while rush sees which command of its
        // chain runs (rushmode.go:3802).
        let part = (shell)
            .then(|| {
                m.session
                    .as_ref()
                    .and_then(|v| v.sess.running_part(&shell_id))
            })
            .flatten()
            .map(|rp| {
                (
                    format!(
                        "kill {} only",
                        rush_ui_list_extras::jobs::first_word(&rp.command)
                    ),
                    crate::shells::pause_word(&shell_id),
                )
            });
        // Yours, or enter, opens or closes the row (rowKey).
        let rk = super::row_key(m);
        let mut bound = |id: &str| crate::keybind::bound_key(m, id);
        let remember = bound("session.remember");
        let (open_all, close_older) = (
            bound("session.history.open"),
            bound("session.history.close"),
        );
        let (rewind, fork) = (bound("session.rewind"), bound("session.fork"));
        let full_key = bound("session.full");
        let full = super::changes::full_file(m, key).is_some();
        let pairs: &[(&str, &str)] = if rush_convo::model::exchange_peer(picked).is_some() {
            // What agents said to each other (rushmode.go:3819).
            &[
                ("space", "expand message"),
                ("→", "open peer session"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if picked.starts_with("run:") && sub.as_ref().is_some_and(|s| !s.1) {
            &[
                ("space", "read what it did"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if picked.starts_with("run:") {
            &[
                ("space", "see it in Subagents"),
                ("x", "stop it"),
                ("shift+x", "…and say why"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if picked.starts_with("sub:") && sub.as_ref().is_some_and(|s| s.1) {
            &[
                ("→ · tab", "message it"),
                ("x", "stop it"),
                ("shift+x", "…and say why"),
                ("↑↓", "pick"),
                ("shift+↑↓", "next run"),
            ]
        } else if picked.starts_with("sub:") {
            &[
                ("→ · tab", "message it"),
                ("↑↓", "pick"),
                ("shift+↑↓", "next run"),
            ]
        } else if let Some((kill, pause)) = &part {
            &[
                ("x", "stop this command"),
                ("k", kill.as_str()),
                ("p", pause),
                ("shift+x · shift+k", "…and say why"),
                ("b", "background"),
                (rk.as_str(), "open or close"),
                ("esc", "back to message"),
            ]
        } else if shell {
            &[
                ("x", "stop this command"),
                ("shift+x", "…and say why"),
                ("b", "background"),
                (rk.as_str(), "open or close"),
                ("esc", "back to message"),
            ]
        } else if picked == "go:memory" {
            &[
                ("space", "open the Memory tab"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if full {
            &[("↑↓ · pgup pgdn", "scroll"), ("esc", "back to the changes")]
        } else if view == "changes" && (picked.starts_with("chg:") || picked.starts_with("tree:")) {
            &[
                (full_key.as_str(), "the file in full"),
                (rk.as_str(), "open or close"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if view == "background" && picked.starts_with("job:") {
            // A task in the background view (rushmode.go:3816).
            &[
                ("space", "show output"),
                ("d", "launch details"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if picked.starts_with("mem:") {
            &[
                ("space", "edit it here"),
                ("ctrl+g", "open in $EDITOR"),
                ("x", "delete"),
                ("+", "remember"),
                ("r", "review"),
                ("f", "tidy"),
                ("↑↓", "pick"),
                ("esc", "back to message"),
            ]
        } else if turn {
            &[
                (rk.as_str(), "open or close"),
                (open_all.as_str(), "open all"),
                (close_older.as_str(), "close older"),
                ("esc", "back to message"),
                (rewind.as_str(), "rewind"),
                (fork.as_str(), "fork"),
                (remember.as_str(), "remember"),
            ]
        } else {
            &[
                (rk.as_str(), "open or close"),
                ("↑↓", "pick a step"),
                ("esc", "back to message"),
                ("ctrl+o", "show all"),
            ]
        };
        // → opens the row picked, or moves on into the column beside.
        let mut pairs = pairs.to_vec();
        if view == "conversation" && !pairs.iter().any(|p| p.0.contains('→')) {
            let says = if super::side::has_items(m, key) {
                "open · the side"
            } else {
                "open"
            };
            pairs.insert(1.min(pairs.len()), ("→", says));
        }
        return fit_keys(w, &pairs);
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
    let (words, stash_on) = rush_ui_plugin_ui::hooks::stash(m);
    let has_stash = rush_ui_plugin_ui::hooks::has_stash(m, key);
    let (aside, history) = rush_ui_plugin_ui::hooks::stash_keys(m);
    let your_stash = format!("your {} back", words.noun);
    let history_says = format!("{} · sent · cleared", words.tab.to_lowercase());
    let side_items = view == "conversation" && super::side::has_items(m, key);
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
    // The key that sends now is yours if you moved it (sendmode.go:110-116).
    let mut pairs: Vec<(&str, &str)> = sendmode::box_keys(&st)
        .into_iter()
        .map(|(k, v)| (if k == SEND_NOW { send_now.as_str() } else { k }, v))
        .collect();
    // Where what's kept or cleared goes: the stash's keys while it runs
    // (sendmode.go:126-139).
    if st.typed && stash_on {
        pairs.retain(|(k, _)| *k != sendmode::UNDO_HINT);
        pairs.push((aside.as_str(), words.verb));
    } else if !st.cleared && has_stash {
        pairs.push((aside.as_str(), your_stash.as_str()));
    }
    if stash_on {
        pairs.push((history.as_str(), history_says.as_str()));
    }
    if side_items {
        pairs.push(("→", "the side"));
    }
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
    // Alone in a hosted view: the list is ctrl+6 away, and esc only
    // leaves the box (rushmode.go:3766).
    let mut back: Vec<(&str, &str)> = if (m.ext.calls.hosted_alone())(m) {
        vec![
            ("f1", "more"),
            ("ctrl+6", "Agents"),
            ("esc", "leave the box"),
        ]
    } else {
        vec![("f1", "more"), ("esc", "back")]
    };
    if other_view {
        back.insert(0, (preset_key.as_str(), next_view));
    } else if view == "conversation" {
        // Said last of all but the way back, so it's the first to go when
        // the row is too narrow.
        back.insert(back.len() - 1, (preset_key.as_str(), "conversation view"));
    }
    // With nothing typed, alt+←/→ make the column beside the text
    // narrower or wider (its edge drags too): said after the view's key,
    // so it goes first of all when the row is too narrow (Rust-only).
    if view == "conversation" && !st.typed && !conn(m, key).side.rows.is_empty() {
        back.insert(back.len() - 1, ("alt+←→", "the side's width"));
    }
    few_keys(w, 3, &pairs, &back)
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
    let h = crate::sess::peek_state(m)?.hint.clone()?;
    // "  "+hint, in the pane two in (paneDock's last row).
    let mut l = line([plain(blanks(INDENT + 2))]);
    join(&mut l, h);
    Some(l)
}

/// Whether `p` has `edge` starting at display column `col` (cells, not
/// bytes: the list beside it draws wide and multi-byte runes).
fn at_column(p: &str, col: usize, edge: &str) -> bool {
    let mut w = 0;
    for (i, ch) in p.char_indices() {
        if w == col {
            return p[i..].starts_with(edge);
        }
        if w > col {
            return false;
        }
        w += crate::frame::inputbox::rune_w(ch);
    }
    false
}

/// Finds where the box landed on the finished frame, for clicks (an
/// overlay that draws nothing).
#[allow(clippy::ptr_arg)] // the overlay hook's shape
pub fn locate(m: &mut Model, out: &mut Vec<Line>) {
    // The opened Session's box, or, beside the list, the picked agent's
    // too: a click there gives it the keys and places the cursor at once
    // (clickBox's boxAt reads either).
    let Some(key) = open_key(m).or_else(|| m.pane.as_ref().map(|v| v.key.clone())) else {
        return;
    };
    if m.on_place() {
        return;
    }
    // Beside the list the Session starts past the list and its divider.
    let (lw, pw) = crate::split::widths(m);
    let col = if lw > 0 && pw > 0 { lw + 1 } else { 0 } + INDENT;
    let c = conn(m, &key);
    let Some(edge) = c.drawn.lines().first().map(Line::plain) else {
        return;
    };
    let found = out.iter().rposition(|l| at_column(&l.plain(), col, &edge));
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

/// What an empty Session box says of the history, while the stash runs
/// (sendmode.go:154 historyHint): " · ctrl+r: stashed, sent, cleared".
fn history_hint(m: &Model) -> String {
    let (w, on) = rush_ui_plugin_ui::hooks::stash(m);
    if !on {
        return String::new();
    }
    let (_, history) = rush_ui_plugin_ui::hooks::stash_keys(m);
    format!(" · {history}: {}, sent, cleared", w.tab.to_lowercase())
}

/// Whether a decisions card waits on session key, whether you chose to
/// answer it in your own words, and whether a card has the keys
/// (decisionsCard, c.qOwn, c.cardFocus).
fn cards_now(m: &Model, key: &str) -> (bool, bool, bool) {
    use rush_ui_session_cards::{Cards, info_any, question};
    let Some(c) = m.kept.peek::<Cards>().and_then(|k| k.conns.get(key)) else {
        return (false, false, false);
    };
    let pending = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .map_or(0, |v| v.sess.pending().len());
    let deciding =
        info_any(m, key).is_some_and(|i| question::decisions_card(c, &i, pending).is_some());
    let focus = c.card_focus && !c.card_shown.is_empty();
    (deciding, c.q_own, focus)
}
