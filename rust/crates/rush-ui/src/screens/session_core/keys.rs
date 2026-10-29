//! The Session's keys: typing in its box, sending (enter, now, the ways
//! it sends while it works), stopping a turn, the queue, and the paste and
//! clicks that land in the box. After Go's internal/ui/rushmode.go
//! (paneKey :3792, the box's parts of it; answerHost :4424, sendPane
//! :4440, sendOffline :5001, moveToRushWith :5218), localqueue.go (reply
//! :63), sending.go (sendingVia :150, sendFailed :162), hosted.go
//! (canInterrupt :196, stopTurn :202), stash.go (wipeBox :173, boxVert
//! :225) and sessionviews.go (endQueueEdit :349).
//!
//! The rest of paneKey (a picked row's keys, views, subagents, cards'
//! own keys, rewind and fork, the stash, slash commands) is the session
//! views', cards' and command bar's.

use rush_core::adapter::Feature;
use rush_core::agent::Agent;
use rush_draw::{Button, MouseKind};

use super::host::{Op, SendErr, hosts};
use super::queue::{self, queue_of, with_images};
use super::sendmode::{self, SendMode};
use super::{agent, busy, conn, core, open_key, where_};
use crate::frame::editor::{clean_paste, extract_images, look_disk};
use crate::frame::inputbox::rune_w;
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::session::Pos;

/// Whether the Session showing has the keys: no box over it, no place.
fn showing(m: &Model) -> bool {
    m.session.is_some() && m.confirm.is_none() && m.sheet.is_none() && !m.on_place()
}

/// The conversation's pick ("" for none).
fn sel(m: &Model) -> String {
    m.session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default()
}

fn set_sel(m: &mut Model, s: String) {
    if let Some(v) = m.session.as_mut() {
        v.sel = s;
    }
}

/// A key in the open Session, before the conversation's own (paneKey):
/// Pass leaves it to them.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let s = k.s.as_str();
    let picked = sel(m);
    let empty = conn(m, &key).input.is_empty();
    // A picked queued message takes plain keys.
    let o = queue::key(m, &key, s);
    if o.used() {
        return o;
    }
    let o = queue_nav(m, &key, s, &picked, empty);
    if o.used() {
        return o;
    }
    if s == "?" && empty {
        return Out::Pass; // the keys' help
    }
    if !picked.is_empty() && (s == "ctrl+enter" || s == "ctrl+s") {
        m.flash("enter opens or closes the row · esc returns to your message");
        return Out::Draw;
    }
    if !picked.is_empty() && s == "esc" {
        return Out::Pass; // the first esc drops the pick
    }
    if s == "esc" && conn(m, &key).edit_q > 0 {
        conn(m, &key).input.clear();
        end_queue_edit(m, &key);
        return Out::Draw;
    }
    let o = answer_key(m, &key, s, empty);
    if o.used() {
        return o;
    }
    match s {
        "esc" => {
            if !empty {
                wipe_box(m, &key);
                return Out::Draw;
            }
            if can_interrupt(m, &key) {
                stop_turn(m, &key);
                return Out::Draw;
            }
            return Out::Pass; // back to the list
        }
        "ctrl+c" => {
            if conn(m, &key).input.selection().is_none() {
                if empty {
                    return Out::Pass;
                }
                wipe_box(m, &key);
                return Out::Draw;
            }
            // A selection: the editor copies it.
        }
        "enter" | "space" | "right" if !picked.is_empty() => return Out::Pass,
        "enter" => {
            if empty {
                return Out::Idle;
            }
            if mode_of(m, &key) == SendMode::Discuss {
                m.flash_err("rush can't open rooms yet · ctrl+t for another way to send");
                return Out::Draw;
            }
            return send(m, &key, false);
        }
        "ctrl+t" => {
            cycle_mode(m, &key);
            return Out::Draw;
        }
        "ctrl+enter" | "ctrl+s" => {
            if !empty {
                return send(m, &key, true);
            }
            if !queue_of(m, &key).items.is_empty() {
                queue::send_now(m, &key, "");
                return Out::Draw;
            }
            return Out::Idle;
        }
        "up" | "down" => {
            if empty || !picked.is_empty() {
                return Out::Pass; // the conversation's rows
            }
            box_vert(m, &key, if s == "up" { -1 } else { 1 }, false);
            return Out::Draw;
        }
        "shift+up" | "shift+down" if !empty => {
            box_vert(m, &key, if s == "shift+up" { -1 } else { 1 }, true);
            return Out::Draw;
        }
        "alt+h" => {
            conn(m, &key).edit_held = false; // yours now, not the edit's
            let held = queue_of(m, &key).held;
            queue::hold(m, &key, !held);
            return Out::Draw;
        }
        "left" => {
            if !picked.is_empty() && !empty {
                set_sel(m, String::new());
                return Out::Draw;
            }
            if empty {
                // ← only ever means back: off the picked row, then to the
                // list. It never folds anything.
                if !picked.is_empty() {
                    set_sel(m, String::new());
                } else {
                    m.session = None;
                    super::live::sync(m);
                }
                return Out::Draw;
            }
        }
        "pgup" | "pgdown" | "ctrl+o" => return Out::Pass,
        "home" | "end" | "[" | "]" | "{" | "}" if empty => return Out::Pass,
        _ => {}
    }
    if k.text().is_some() && !picked.is_empty() {
        set_sel(m, String::new()); // typing returns to the box
    }
    let c = conn(m, &key);
    match c.input.key(k) {
        Some(copied) => {
            if !copied.is_empty() {
                m.copy_text(copied);
            }
            Out::Draw
        }
        None => Out::Pass,
    }
}

/// ↑ from the empty box picks the last queued message; ↑↓ go through the
/// queue, ↑ past its first into the conversation, ↓ past its last back
/// to the box (dockRefs and moveSel, for the queue).
fn queue_nav(m: &mut Model, key: &str, s: &str, picked: &str, empty: bool) -> Out {
    let n = queue_of(m, key).items.len();
    if n == 0 || s != "up" && s != "down" {
        return Out::Pass;
    }
    match queue::sel(picked, n) {
        Some(i) if s == "up" => {
            if i == 0 {
                set_sel(m, String::new());
                return Out::Pass; // the conversation's last row in view
            }
            set_sel(m, format!("q:{}", i - 1));
        }
        Some(i) => set_sel(
            m,
            if i + 1 < n {
                format!("q:{}", i + 1)
            } else {
                String::new()
            },
        ),
        None if s == "up" && empty && picked.is_empty() => set_sel(m, format!("q:{}", n - 1)),
        None => return Out::Pass,
    }
    Out::Draw
}

/// y · a · n on an approval waiting, with the box empty; typed text and
/// enter denies with it as why (the box's part of cardKey: the card
/// itself is the cards').
fn answer_key(m: &mut Model, key: &str, s: &str, empty: bool) -> Out {
    if !conn(m, key).hosted() {
        return Out::Pass;
    }
    let Some(v) = m.session.as_ref() else {
        return Out::Pass;
    };
    let Some(&sid) = v.sess.pending().first() else {
        return Out::Pass;
    };
    let Some(req) = v.sess.step(sid).approval.clone() else {
        return Out::Pass;
    };
    if req.question.is_some() {
        return Out::Pass; // a question is answered on its card
    }
    let op = match s {
        "y" if empty => Op::Allow {
            id: req.id,
            always: false,
        },
        "a" if empty => Op::Allow {
            id: req.id,
            always: true,
        },
        "n" if empty => Op::Deny {
            id: req.id,
            message: String::new(),
        },
        "enter" if !empty => {
            let why = conn(m, key).input.string().trim().to_string();
            conn(m, key).input.clear();
            Op::Deny {
                id: req.id,
                message: why,
            }
        }
        _ => return Out::Pass,
    };
    queue::run(m, key, op);
    Out::Draw
}

/// How enter sends to session key now.
pub fn mode_of(m: &mut Model, key: &str) -> SendMode {
    let kind = agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let modes = sendmode::modes(&m.reg, &kind);
    let picked = core(m).send_modes.get(key).copied();
    sendmode::mode_of(picked, &modes)
}

/// ctrl+t: the next way enter sends (cycleSendMode).
pub fn cycle_mode(m: &mut Model, key: &str) {
    let kind = agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let modes = sendmode::modes(&m.reg, &kind);
    let picked = core(m).send_modes.get(key).copied();
    let next = sendmode::next_mode(picked, &modes);
    core(m).send_modes.insert(key.to_string(), next);
    m.flash(&sendmode::cycled(next));
}

/// Whether session key has a turn running that can be stopped, and wasn't
/// just asked to stop (canInterrupt).
pub fn can_interrupt(m: &mut Model, key: &str) -> bool {
    let now = m.now;
    let kind = agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default();
    let live = m.session.as_ref().is_some_and(|v| v.sess.live().is_some());
    let supports = m.reg.supports(&kind, Feature::Interrupt);
    let c = conn(m, key);
    c.hosted() && live && now - c.stop_armed > 2000 && supports
}

/// esc on a running turn: it stops at once (stopTurn).
pub fn stop_turn(m: &mut Model, key: &str) {
    conn(m, key).stop_armed = m.now;
    m.flash("stopping the turn · esc again to close, restart or switch it");
    queue::run(m, key, Op::Interrupt);
}

/// Clears the box, kept to undo (wipeBox).
pub fn wipe_box(m: &mut Model, key: &str) {
    let now = m.now;
    let c = conn(m, key);
    if c.input.is_empty() {
        return;
    }
    let (text, pos) = (c.input.text.clone(), c.input.pos);
    c.input.undo.save(&text, pos, false);
    c.input.text.clear();
    c.input.pos = 0;
    c.input.anchor = None;
    c.cleared_at = now;
    m.flash(&format!("cleared · {} brings it back", sendmode::UNDO_HINT));
}

/// The queue let go again if editing held it (endQueueEdit).
fn end_queue_edit(m: &mut Model, key: &str) {
    let c = conn(m, key);
    c.edit_q = 0;
    if !c.edit_held {
        return;
    }
    c.edit_held = false;
    queue::hold(m, key, false);
}

/// Moves the box's cursor a row up or down through its text as wrapped,
/// keeping its column; past the first row to the start, past the last to
/// the end. shift extends the selection (boxVert).
pub fn box_vert(m: &mut Model, key: &str, d: isize, shift: bool) {
    let c = conn(m, key);
    let mut b = c.drawn.clone();
    b.text = c.input.text.clone();
    let segs = b.segs();
    let text = &c.input.text;
    let mut pos = c.input.pos.min(text.len());
    let row = segs
        .iter()
        .position(|sg| pos >= sg.from && pos <= sg.to)
        .unwrap_or(segs.len().saturating_sub(1));
    if shift && c.input.anchor.is_none() {
        c.input.anchor = Some(pos);
    } else if !shift {
        c.input.anchor = None;
    }
    let to = row as isize + d;
    if to < 0 {
        pos = 0;
    } else if to as usize >= segs.len() {
        pos = text.len();
    } else {
        let col: usize = text[segs[row].from..pos].iter().map(|&r| rune_w(r)).sum();
        let sg = segs[to as usize];
        let mut w = 0;
        pos = sg.from;
        while pos < sg.to && w + rune_w(text[pos]) <= col {
            w += rune_w(text[pos]);
            pos += 1;
        }
    }
    c.input.pos = pos;
}

/// Sends what's in the box (sendPane): now, or as enter sends while it
/// works (queued, guiding, stopping it first). A trailing backslash goes
/// on to a new line instead.
pub fn send(m: &mut Model, key: &str, now: bool) -> Out {
    let working = m.session.as_ref().is_some_and(|v| v.sess.live().is_some());
    let mode = mode_of(m, key);
    let now = now || working && mode == SendMode::Stop;
    let guide = !now && working && mode == SendMode::Guide;
    let c = conn(m, key);
    let raw = c.input.string();
    if raw.ends_with('\\') && !now {
        c.input.text.pop();
        c.input.text.push('\n');
        c.input.pos = c.input.text.len();
        return Out::Draw;
    }
    let text = raw.trim().to_string();
    if c.edit_q > 0 {
        // Saved back in place, before the queue's let go, so the old text
        // never goes.
        let (i, was) = (c.edit_q - 1, std::mem::take(&mut c.edit_was));
        c.input.clear();
        save_edit(m, key, i, was, text);
        return Out::Draw;
    }
    if text.starts_with('#') {
        conn(m, key).input.clear();
        return m.command(&text);
    }
    // Paths typed or dropped become images, at the end.
    let (text, images) = extract_images(&text, &look_disk);
    let t = m.now;
    let c = conn(m, key);
    c.input.clear();
    c.input.undo = Default::default();
    c.last_send = t;
    if let Some(v) = m.session.as_mut() {
        v.pos = Pos::End;
    }
    m.flash("sent");
    if now && images.is_empty() && !queue_of(m, key).items.is_empty() {
        queue::send_now(m, key, &text);
        return Out::Draw;
    }
    if !conn(m, key).hosted() {
        return send_offline(m, key, text, images, now);
    }
    mark_sending(m, key, &text);
    let op = if guide {
        Op::Guide { text, images }
    } else {
        Op::Send { text, images, now }
    };
    sending_via(m, key, op);
    Out::Draw
}

/// Saves an edited queued message back in place (editLocal, and the
/// host's EditQueued), then lets the queue go if editing held it.
fn save_edit(m: &mut Model, key: &str, i: usize, was: String, text: String) {
    let c = conn(m, key);
    c.edit_q = 0;
    let release = std::mem::take(&mut c.edit_held);
    if !c.hosted() {
        let q = core(m).local_q.entry(key.to_string()).or_default();
        let at = if q.items.get(i) == Some(&was) {
            Some(i)
        } else {
            q.items.iter().position(|it| *it == was)
        };
        match at {
            Some(i) => q.items[i] = text,
            None => m.flash_err("that message has already been sent"),
        }
        if release {
            queue::hold(m, key, false);
        }
        return;
    }
    if c.info.queue.get(i) == Some(&was) {
        c.info.queue[i] = text.clone();
    }
    let mut ops = vec![Op::EditQueued { i, was, text }];
    if release {
        c.info.queue_held = false;
        ops.push(Op::HoldQueue(false));
    }
    queue::run_all(m, key, ops);
}

/// Shows text as sending until the agent has it (markSending).
fn mark_sending(m: &mut Model, key: &str, text: &str) {
    let now = m.now;
    let queued = queue_of(m, key).items.len();
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return;
    };
    let c = m
        .kept
        .get::<super::Core>()
        .conns
        .entry(key.to_string())
        .or_default();
    c.out.mark(&v.sess, queued, text, now);
}

/// Asks the host for op; a failure takes the message off as sending and
/// says why (sendingVia, sendFailed).
fn sending_via(m: &mut Model, key: &str, op: Op) {
    let Some(link) = conn(m, key).link.clone() else {
        return;
    };
    let key = key.to_string();
    m.later(
        move || link.op(op),
        move |m, r| match r {
            Ok(()) => Out::Idle,
            Err(e) => {
                conn(m, &key).out.failed();
                m.flash_err(&e);
                Out::Draw
            }
        },
    );
}

/// Sends from the box of a session rush isn't hosting (sendOffline): a
/// past one moves to rush mode with it, a stopped rush session resumes
/// with it, and one its agent's daemon runs gets it as a reply, or queued
/// while it works.
fn send_offline(m: &mut Model, key: &str, text: String, images: Vec<String>, now: bool) -> Out {
    let Some(a) = agent(m, key).cloned() else {
        return Out::Idle;
    };
    let name = a.display_name.clone();
    if a.interactive {
        m.flash_err(&format!("{name} is {}; rush can't send to it", where_(&a)));
        return Out::Draw;
    }
    if a.past {
        adopt(m, &a, &with_images(&text, &images));
        return Out::Draw;
    }
    let h = hosts(m);
    if h.hosts(&a) {
        m.flash(&format!("resuming {name}…"));
        mark_sending(m, key, &text);
        let key = key.to_string();
        m.later(
            move || h.resume(&a, &text, &images),
            move |m, r| {
                match r {
                    Ok(()) => m.flash(&format!("resumed {name}")),
                    Err(e) => {
                        conn(m, &key).out.failed();
                        m.flash_err(&e);
                    }
                }
                Out::Draw
            },
        );
        return Out::Draw;
    }
    let text = with_images(&text, &images);
    if !now && (busy(&a) || !queue_of(m, key).items.is_empty()) {
        // It's working: the message waits in the queue and goes when it's
        // idle, with anything else waiting.
        let n = queue::queue_local(m, key, &text, m.now);
        m.flash(&format!("queued · {n} waiting · goes to {name} within 15s"));
        return Out::Draw;
    }
    mark_sending(m, key, &text);
    send_reply(m, &a, text, true);
    Out::Draw
}

/// Sends text to a, a session rush doesn't run, through its agent's
/// daemon (reply): one the daemon let go carries on in rush mode.
pub fn reply(m: &mut Model, a: &Agent, text: String) {
    send_reply(m, a, text, false);
}

fn send_reply(m: &mut Model, a: &Agent, text: String, sending: bool) {
    let h = hosts(m);
    let a = a.clone();
    m.later(
        move || {
            let r = h.reply(&a, &text);
            (a, text, r)
        },
        move |m, (a, text, r)| {
            match r {
                Ok(()) => m.flash(&format!("sent to {}", a.display_name)),
                Err(SendErr::Gone) => adopt(m, &a, &text),
                Err(SendErr::Failed(e)) => {
                    if sending {
                        conn(m, &a.key).out.failed();
                    }
                    m.flash_err(&format!("couldn't send to {}: {e}", a.display_name));
                }
            }
            Out::Draw
        },
    );
}

/// Moves a, a session rush doesn't run, to rush mode, text its next
/// message (moveToRushWith).
pub fn adopt(m: &mut Model, a: &Agent, text: &str) {
    let h = hosts(m);
    if a.headless {
        m.flash_err(&format!(
            "{} is driven by another program; rush can't take it over",
            a.display_name
        ));
        return;
    }
    m.flash(&format!("moving {} to rush mode…", a.display_name));
    let (a, text) = (a.clone(), text.to_string());
    m.later(
        move || h.adopt(&a, &text),
        |m, r| {
            if let Err(e) = r {
                m.flash_err(&e);
            }
            Out::Draw
        },
    );
}

/// A paste into the Session's box, made clean (the frame's Agents box
/// takes the rest).
pub fn paste(m: &mut Model, text: &str) -> Out {
    if !showing(m) {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    set_sel(m, String::new());
    conn(m, &key).input.insert_str(&clean_paste(text));
    Out::Draw
}

/// A click in the box puts the cursor there and gives it the keys; a
/// click on a queued message picks it (clickBox, queueHover).
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if !showing(m) || e.kind != MouseKind::Down(Button::Left) {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    let c = conn(m, &key);
    let Some(top) = c.box_y else {
        return Out::Pass;
    };
    if let Some(Some(i)) = y.checked_sub(c.q_top).and_then(|r| c.q_rows.get(r)) {
        let i = *i;
        set_sel(m, format!("q:{i}"));
        return Out::Draw;
    }
    let rows = c.drawn.rows();
    if y <= top || y > top + rows {
        return Out::Pass;
    }
    let mut b = c.drawn.clone();
    b.text = c.input.text.clone();
    b.cursor = c.input.pos;
    c.input.pos = b.at(y - top - 1, x as isize - dock_x() as isize);
    c.input.anchor = None;
    set_sel(m, String::new());
    Out::Draw
}

/// The column the dock's box starts at.
pub fn dock_x() -> usize {
    super::dock::INDENT
}
