//! The Session's keys: typing in its box, sending (enter, now, the ways
//! it sends while it works), stopping a turn, the queue, and the paste and
//! clicks that land in the box. After Go's internal/ui/rushmode.go
//! (paneKey :3792, the box's parts of it; answerHost :4424, sendPane
//! :4440, sendOffline :5001, moveToRushWith :5218), localqueue.go (reply
//! :63), sending.go (sendingVia :150, sendFailed :162), hosted.go
//! (canInterrupt :196, stopTurn :202), stash.go (wipeBox :173, keepSent
//! :199, boxVert :225) and sessionviews.go (endQueueEdit :349). Plugins
//! asked first (sendPane :4627, :4637) is plugin_ui::intercept.
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
use crate::screens::plugin_ui::{hooks, intercept};
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

/// esc twice within a second with nothing typed asks whether to close the
/// agent (askClose): it hides, stops, restarts, switches or deletes it.
/// Each esc that meets the empty box arms it, whatever else it does (drop
/// dragged-over text, close a watched run, stop the turn, go back), so it
/// goes before every other Session key; one that drops a picked row
/// doesn't (rushmode.go:4033-4036, :4145-4152).
pub fn esc_twice(m: &mut Model, k: &Press) -> Out {
    if k.s != "esc" || !showing(m) {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let now = m.now;
    let c = conn(m, &key);
    if !c.input.is_empty() || c.edit_q > 0 {
        return Out::Pass;
    }
    // A picked row, the conversation's or a watched run's, is dropped.
    let sub_picked = conn(m, &key)
        .runs
        .detail
        .as_ref()
        .is_some_and(|d| !d.view.sel.is_empty());
    if !sel(m).is_empty() || sub_picked && super::subview::watching(m, &key) {
        core(m).esc_at = 0;
        return Out::Pass;
    }
    let co = core(m);
    if co.esc_key == key && co.esc_at != 0 && now - co.esc_at < 1000 {
        co.esc_at = 0;
        let a = agent(m, &key).cloned();
        return crate::screens::cmdbar::stops::ask_close(m, a);
    }
    // Hosted alone, esc on the empty box takes the keys off it, and esc
    // again there only stops a running turn (hosted.rs away_key): it's
    // enter, giving the box the keys back, then esc that asks.
    let co = core(m);
    co.esc_at = now;
    co.esc_key = key;
    Out::Pass
}

/// A key in the open Session, before the conversation's own (paneKey):
/// Pass leaves it to them.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let s = k.s.as_str();
    // A room's own keys first (paneKey's roomPaneKey).
    let o = crate::screens::social::rooms::pane_key(m, &key, s);
    if o.used() {
        return o;
    }
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
    // o on a picked step asks what to do with its file, as a click on it
    // (rushmode.go:4126).
    if s == "o"
        && empty
        && !picked.is_empty()
        && let Some(u) = super::conversation::picked_link(m)
        && crate::screens::cmdbar::linkmenu::open_link_menu(m, &u)
    {
        return Out::Draw;
    }
    // v on a picked step shows its output another way (text, pretty); w
    // keeps each line of it to one row, or wraps it again
    // (rushmode.go:4101-4117).
    if (s == "v" || s == "w")
        && empty
        && picked.contains(":s:")
        && let Some(v) = m.session.as_mut()
    {
        if s == "w" {
            let k = rush_convo::render::no_wrap_ref(&picked);
            let on = v.o.open.get(&k).copied().unwrap_or(false);
            v.o.open.insert(k, !on);
            v.o.open.insert(picked, true);
            return Out::Draw;
        }
        let cur = v.o.view.get(&picked).cloned().unwrap_or_default();
        let next = rush_convo::render::next_view(&v.sess, &picked, &cur);
        if !next.is_empty() {
            v.o.view.insert(picked.clone(), next);
            v.o.open.insert(picked, true);
            return Out::Draw;
        }
    }
    if s == "?" && empty {
        return Out::Pass; // the keys' help
    }
    if !picked.is_empty() && (s == "ctrl+enter" || s == "ctrl+s") {
        let rk = super::row_key(m);
        m.flash(&format!(
            "{rk} opens or closes the row · esc returns to your message"
        ));
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
    let o = answer_key(m, &key, s, empty && picked.is_empty());
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
                // What's typed is the room's topic (discuss.go:192
                // discussTyped).
                let c = conn(m, &key);
                let topic = c.input.sent(true);
                c.input.clear();
                return crate::screens::session_cards::discuss::discuss(m, &key, &topic, true);
            }
            return send(m, &key, false);
        }
        "ctrl+t" => {
            cycle_mode(m, &key);
            return Out::Draw;
        }
        // Done with this agent: put away in Earlier, its idle process
        // stopped (rushmode.go:4370).
        "ctrl+d" | "alt+d" => {
            let a = agent(m, &key).cloned();
            return crate::screens::cmdbar::stops::mark_done(m, a);
        }
        // The agent that has waited longest for you, picked in the list
        // (rushmode.go:4444, keys.go:595 nextNeedingYou).
        "ctrl+n" => return next_needing_you(m),
        // The stash: set what's typed aside or bring it back, and what was
        // set aside, sent, cleared and replaced (rushmode.go:4337-4340).
        "ctrl+p" => return crate::screens::list_extras::stash::command(m, "stash"),
        "ctrl+r" => return crate::screens::list_extras::stash::command(m, "history"),
        // Gone for good: stopped with all it set going (rushmode.go:4440).
        "ctrl+x" => {
            let a = agent(m, &key).cloned();
            return crate::screens::cmdbar::stops::dismiss(m, a);
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
                    // Back to Agents, as esc goes (leavePane).
                    m.close();
                    super::live::sync(m);
                }
                return Out::Draw;
            }
        }
        "pgup" | "pgdown" | "ctrl+o" => return Out::Pass,
        // { } and tab go back to the list, as they came (switchFocus);
        // tab completes a / command being typed.
        "{" | "}" if empty => {
            m.close();
            super::live::sync(m);
            return Out::Draw;
        }
        "tab" if !conn(m, &key).input.string().starts_with('/') => {
            m.close();
            super::live::sync(m);
            return Out::Draw;
        }
        // shift+← → move the list's edge, as in the list (stepSplit).
        "shift+left" | "shift+right" | "alt+left" | "alt+right" if empty => {
            let o = crate::screens::list_extras::split::key(m, k);
            if o.used() {
                return o;
            }
        }
        "home" | "end" | "[" | "]" if empty => return Out::Pass,
        _ => {}
    }
    if k.text().is_some() && !picked.is_empty() {
        set_sel(m, String::new()); // typing returns to the box
    }
    let c = conn(m, &key);
    match c.input.key(k) {
        Some(copied) => {
            if k.s == "space" || k.s == "enter" {
                // A path you typed to an image becomes the image once it's
                // done, where it was typed (rushmode.go:4483-4489).
                inline_typed(&mut c.input, &mut c.imgs);
            }
            if !copied.is_empty() {
                m.copy_text(copied);
            }
            Out::Draw
        }
        None => Out::Pass,
    }
}

/// ctrl+n from a Session: the keys go to the list, on the agent that has
/// waited longest for you, so a stack of questions is cleared with ctrl+n
/// and an answer each; "nothing needs you" when none does
/// (keys.go:595 nextNeedingYou).
pub fn next_needing_you(m: &mut Model) -> Out {
    use rush_fleet::Predicates;
    let best = m
        .order
        .iter()
        .copied()
        .filter(|&i| m.agents[i].needs_you())
        .min_by_key(|&i| m.agents[i].job.updated_at);
    let Some(i) = best else {
        m.flash("nothing needs you");
        return Out::Draw;
    };
    let key = m.agents[i].key.clone();
    m.close();
    super::live::sync(m);
    m.select(key);
    m.rebuild();
    Out::Draw
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
                // The dock's runs above it, else the conversation's last
                // row in view.
                if let Some(last) = super::subview::dock_refs(m, key).pop() {
                    set_sel(m, last);
                    return Out::Draw;
                }
                set_sel(m, String::new());
                return Out::Pass;
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

/// y · a · n on an approval waiting, with the box empty and no row
/// picked (alt+y · alt+a · alt+n always); typed text and enter denies
/// with it as why (the box's part of cardKey :5811-5831: the card itself
/// is the cards').
fn answer_key(m: &mut Model, key: &str, s: &str, direct: bool) -> Out {
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
    let req = *req;
    if req.question.is_some() {
        return Out::Pass; // a question is answered on its card
    }
    let typed = !conn(m, key).input.is_empty();
    let picked = !sel(m).is_empty();
    let op = match s {
        "alt+y" => Op::Allow {
            id: req.id,
            always: false,
        },
        "y" if direct => Op::Allow {
            id: req.id,
            always: false,
        },
        "alt+a" => Op::Allow {
            id: req.id,
            always: true,
        },
        "a" if direct => Op::Allow {
            id: req.id,
            always: true,
        },
        "alt+n" => Op::Deny {
            id: req.id,
            message: String::new(),
        },
        "n" if direct => Op::Deny {
            id: req.id,
            message: String::new(),
        },
        "enter" if typed && !picked => {
            // What's typed is why not: it's denied, and the agent's told.
            let why = conn(m, key).input.sent(false).trim().to_string();
            conn(m, key).input.clear_sent();
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
    // esc again soon after asks to close, restart or switch it.
    let (now, c) = (m.now, core(m));
    c.esc_at = now;
    c.esc_key = key.to_string();
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
    // The stash told, for its history's Cleared (stash.go:178).
    let text: String = text.iter().collect();
    hooks::cleared(m, key, &text);
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
    // A room's box says it in the room (sendPane's isRoomKey).
    if let Some(o) = crate::screens::social::rooms::send_from(m, key) {
        return o;
    }
    // Plugins still looking at it, or a wake taking it: it goes once
    // that's done (rushmode.go:4627).
    if conn(m, key).intercepting || conn(m, key).waking {
        return Out::Idle;
    }
    let working = m.session.as_ref().is_some_and(|v| v.sess.live().is_some());
    let mode = mode_of(m, key);
    let asked_now = now;
    let now = now || working && mode == SendMode::Stop;
    let guide = !now && working && mode == SendMode::Guide;
    // Plugins with "intercept" have their say first (rushmode.go:4637).
    if intercept::wants(m, key, now) {
        return intercept::ask(m, key, now);
    }
    let c = conn(m, key);
    let raw = c.input.string();
    if raw.ends_with('\\') && !now {
        c.input.text.pop();
        c.input.text.push('\n');
        c.input.pos = c.input.text.len();
        return Out::Draw;
    }
    // Each paste's chip its text again. Claude Code sessions are typed
    // into, where tags would show as typed (rushmode.go:4646-4647).
    let tagged = c.hosted() || agent(m, key).is_none_or(|a| a.rush);
    let c = conn(m, key);
    let text = c.input.sent(tagged).trim().to_string();
    if c.edit_q > 0 {
        // Saved back in place, before the queue's let go, so the old text
        // never goes.
        let (i, was) = (c.edit_q - 1, std::mem::take(&mut c.edit_was));
        c.input.clear_sent();
        save_edit(m, key, i, was, text);
        return Out::Draw;
    }
    // # and a letter; # Plan or #123 is sent as it is (rushmode.go:4705).
    if crate::screens::cmdbar::fleetslash::is_hash_cmd(&text) {
        conn(m, key).input.clear_sent();
        return m.command(&text);
    }
    // The / commands rush answers itself, not the agent; one neither it
    // nor the session knows asks first (rushmode.go:4709-4720,
    // sessionviews.go:847 runRushCommand, :1015 askUnknown).
    if super::host::is_command(&text) {
        use crate::screens::session_cards::claudescreen;
        let o = super::rushcmd::run(m, key, &text);
        if o.used() {
            let c = conn(m, key);
            c.input.clear_sent();
            c.slash_sel = 0;
            return o;
        }
        if claudescreen::ask_unknown(m, key, &text, now) {
            return Out::Draw;
        }
    }
    // A message that looks meant for another agent asks where it goes
    // (rushmode.go:4722 askMisdirected).
    let k = key.to_string();
    let again: crate::frame::confirm::Then = Box::new(move |m| send(m, &k, asked_now));
    if crate::screens::session_cards::misdirect::ask_misdirected(m, key, &text, again) {
        return Out::Draw;
    }
    // A message waking a cold cache asks first; yes sends it as it was
    // (rushmode.go:4725 askCold).
    let k = key.to_string();
    let again: crate::frame::confirm::Then = Box::new(move |m| send(m, &k, asked_now));
    if crate::screens::session_cards::compact::ask_cold(m, key, &text, again) {
        return Out::Draw;
    }
    // The box's [Image #N] are its images, numbered as they go
    // (rushmode.go:4731 resolve); paths typed or dropped become images
    // too, at the end.
    let c = conn(m, key);
    let (text, mut images) = c.imgs.resolve(&text);
    let (text, typed) = extract_images(&text, &look_disk);
    images.extend(typed);
    let t = m.now;
    let c = conn(m, key);
    c.input.clear_sent();
    c.imgs = Default::default();
    c.last_send = t;
    if let Some(v) = m.session.as_mut() {
        v.pos = Pos::End;
    }
    // The stash told, for its history's Sent (rushmode.go:4743 keepSent).
    hooks::keep_sent(m, key, &text);
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

/// Sends text to the session rush hosts as if typed, the box left as it
/// is: a view's button (asksheets.go:51 sendAs).
pub fn send_as(m: &mut Model, key: &str, text: &str) {
    mark_sending(m, key, text);
    sending_via(
        m,
        key,
        Op::Send {
            text: text.to_string(),
            images: Vec::new(),
            now: false,
        },
    );
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
    super::later(
        m,
        move || link.op(op),
        move |m, r| match r {
            Ok(()) => Out::Idle,
            Err(e) => {
                conn(m, &key).out.failed();
                m.fail(&e);
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
        if !can_resume(m, &a) {
            return Out::Draw;
        }
        let c = conn(m, key);
        if c.waking {
            return Out::Draw;
        }
        // Asleep, its host wakes with what was sent going over its new
        // socket once (wakeHost); stopped, a new host resumes the
        // conversation with it (sendOffline's host.Spawn). Whether it's
        // asleep is read off disk when the link didn't say.
        let asleep = c.sleeping;
        c.waking = true;
        let word = if asleep { "waking" } else { "resuming" };
        m.flash(&format!("{word} {name}…"));
        mark_sending(m, key, &text);
        let key = key.to_string();
        super::later(
            m,
            move || {
                if asleep || h.sleeping(&a) {
                    Ok(h.wake(&a, &text, &images, now).map_err(|e| (e, text)))
                } else {
                    Err(h.resume(&a, &text, &images))
                }
            },
            move |m, r| match r {
                Ok(r) => {
                    let (r, text) = match r {
                        Ok(l) => (Ok(l), String::new()),
                        Err((e, t)) => (Err(e), t),
                    };
                    super::live::woke(m, &key, r, text)
                }
                Err(r) => {
                    conn(m, &key).waking = false;
                    match r {
                        Ok(()) => m.flash(&format!("resumed {name}")),
                        Err(e) => {
                            conn(m, &key).out.failed();
                            m.fail(&e);
                        }
                    }
                    Out::Draw
                }
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

/// Whether a's agent can pick its conversation up again, saying so when
/// it can't (canResume).
pub fn can_resume(m: &mut Model, a: &Agent) -> bool {
    if m.reg
        .supports(a.kind.as_str(), rush_core::adapter::Feature::Resume)
    {
        return true;
    }
    let h = crate::session::harness_name(&m.reg, &a.kind);
    m.flash_err(&format!("{h} can't pick a conversation up again"));
    false
}

/// Sends text to a, a session rush doesn't run, through its agent's
/// daemon (reply): one the daemon let go carries on in rush mode.
pub fn reply(m: &mut Model, a: &Agent, text: String) {
    send_reply(m, a, text, false);
}

fn send_reply(m: &mut Model, a: &Agent, text: String, sending: bool) {
    let h = hosts(m);
    let a = a.clone();
    super::later(
        m,
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
                    m.fail(&format!("couldn't send to {}: {e}", a.display_name));
                }
            }
            Out::Draw
        },
    );
}

/// Moves a, a session rush doesn't run, to rush mode, text its next
/// message (moveToRushWith).
pub fn adopt(m: &mut Model, a: &Agent, text: &str) {
    super::rushmove::move_to_rush_with(m, a, text);
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
    // Image files dropped onto the terminal arrive as a paste of their
    // paths: each becomes [Image #N] where it was dropped (model.go:1420).
    // Newlines kept, so a pasted log or snippet arrives whole; a long one
    // shows as a chip and goes out whole (model.go:1435-1446).
    let c = conn(m, &key);
    let mut text = clean_paste(text);
    if let Some(t) = c.imgs.inline(&text, &look_disk) {
        text = t;
    }
    c.input.paste(&text, true);
    Out::Draw
}

/// Image files named in a box's text, typed or dropped in as keys, as
/// [Image #N] where they were; the cursor kept as far from the end
/// (rushmode.go:4486-4489).
pub(crate) fn inline_typed(
    input: &mut crate::frame::Editor,
    imgs: &mut crate::screens::session_cards::imagerefs::ImageRefs,
) {
    let text = input.string();
    if let Some(t) = imgs.inline(&text, &look_disk) {
        let back = input.text.len() - input.pos.min(input.text.len());
        input.text = t.chars().collect();
        input.pos = input.text.len() - back.min(input.text.len());
        input.anchor = None;
    }
}

/// A click in the box puts the cursor there and gives it the keys, or
/// opens the paste chip under it; a click on a queued message picks it
/// (clickBox, queueHover). The pointer over a chip lights it (hoverChip).
pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if showing(m) && e.kind == MouseKind::Moved {
        return hover_chip(m, e);
    }
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
    // The task rows on the box: the head opens or shuts them, the rest
    // (and "all tasks ›") opens the Tasks view (clickTasks).
    if c.task_rows > 0 && y + c.task_rows >= top && y < top {
        let all = rush_core::text::width(super::tasks::TASK_ALL);
        if y + c.task_rows == top && x + 2 + all < dock_x() + c.task_w {
            c.tasks_open = !c.tasks_open;
            return Out::Draw;
        }
        super::views::show_view(m, &key, "tasks");
        return Out::Draw;
    }
    if let Some(Some(i)) = y
        .checked_sub(c.q_top)
        .filter(|_| y >= c.q_shown.0 && y < c.q_shown.1)
        .and_then(|r| c.q_rows.get(r))
    {
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
    let p = b.at(y - top - 1, x as isize - dock_x() as isize);
    if !c.input.open_chip_at(p) {
        c.input.pos = p;
        c.input.anchor = None;
    }
    set_sel(m, String::new());
    Out::Draw
}

/// The paste chip under the pointer lit, in the box as drawn last; Pass
/// when what's lit didn't change (hoverChip).
fn hover_chip(m: &mut Model, e: &Mouse) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    let c = conn(m, &key);
    let at = c.box_y.and_then(|top| {
        let rows = c.drawn.rows();
        let x0 = dock_x() + 2;
        (y > top && y <= top + rows && x >= x0 && x < dock_x() + c.drawn.w).then(|| {
            let mut b = c.drawn.clone();
            b.text = c.input.text.clone();
            b.cursor = c.input.pos;
            b.at(y - top - 1, x as isize - dock_x() as isize)
        })
    });
    if c.input.hover(at) {
        Out::Draw
    } else {
        Out::Pass
    }
}

/// The column the dock's box starts at.
pub fn dock_x() -> usize {
    super::dock::INDENT
}
