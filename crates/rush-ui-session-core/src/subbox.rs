//! The box under a run picked in the subagents view, as a Session's own:
//! a draft of its own for each run (its pastes as chips, its images, undo),
//! enter sending it the way its route goes and ctrl+enter sending now, and
//! the queue it waits in shown above it, picked with ↑ and edited, moved,
//! merged, dropped, held or sent now with the queue's keys. Rush-only: Go's
//! box under a subagent was one line (rushmode.go subPreviewLines); its
//! keys are a Session's (keys.go, sessionviews.go queueKey :68, queueEdit
//! :152, holdQueue :210), its queue subqueue.go's.

use std::collections::HashMap;

use rush_core::text::Line;

use super::host::join_queue;
use super::paint::{dim, faint};
use super::queue::{LocalQueue, Queued, with_images};
use super::subname::sub_task;
use super::subview::runs_of;
use super::{conn, core, open_key};
use crate::frame::editor::{clean_paste, extract_images, look_disk};
use crate::frame::{Editor, Out, Press};
use crate::imagerefs::ImageRefs;
use crate::model::Model;
use crate::paint::fit_keys;
use crate::style::*;

/// What's typed for one run, kept while you look at others.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub input: Editor,
    pub imgs: ImageRefs,
    /// The queued message picked, while the keys are on the queue.
    pub pick: Option<usize>,
    /// The queued message being edited in the box, 1-based; 0 for none.
    pub edit_q: usize,
    /// The edit held the queue, and lets it go when it's done.
    pub edit_held: bool,
    /// The queued message being edited as it was: saved over only where
    /// it still is (keys.rs's edit_was).
    pub edit_was: String,
}

/// The drafts, by run id.
pub type Drafts = HashMap<String, Draft>;

/// Run id's draft in session key.
pub fn draft<'a>(m: &'a mut Model, key: &str, id: &str) -> &'a mut Draft {
    conn(m, key).watch.drafts.entry(id.to_string()).or_default()
}

/// Whether run id's box has nothing typed.
pub fn empty(m: &mut Model, key: &str, id: &str) -> bool {
    conn(m, key)
        .watch
        .drafts
        .get(id)
        .is_none_or(|d| d.input.is_empty())
}

/// Run id's queue, as the dock's card shows one.
pub fn queue_of(m: &mut Model, key: &str, id: &str) -> Queued {
    let q = core(m).local_q.get(&super::subqueue::key(key, id));
    Queued {
        items: q.map(|q| q.items.clone()).unwrap_or_default(),
        held: q.is_some_and(|q| q.held),
        local: true,
        ..Queued::default()
    }
}

fn local_q<'a>(m: &'a mut Model, key: &str, id: &str) -> &'a mut LocalQueue {
    core(m)
        .local_q
        .entry(super::subqueue::key(key, id))
        .or_default()
}

/// Puts text back in run id's box after it didn't go: before what's been
/// typed since.
pub fn give_back(m: &mut Model, key: &str, id: &str, text: &str) {
    let d = draft(m, key, id);
    let mut t = text.to_string();
    if !d.input.is_empty() {
        t.push('\n');
        t.push_str(&d.input.string());
    }
    d.input.set(&t);
}

/// The box's lines, w wide: the queue card when anything waits, then
/// the box (subPreviewLines' box).
pub fn lines(m: &mut Model, key: &str, id: &str, w: usize) -> Vec<Line> {
    let focused = conn(m, key).watch.msg_focus;
    lines_for(m, key, id, w, focused)
}

/// lines, with the keys or without them: a run open whole (runview.rs)
/// has its box alone, and it has them.
pub fn lines_for(m: &mut Model, key: &str, id: &str, w: usize, focused: bool) -> Vec<Line> {
    let rt = super::relay::route(m, key, id);
    let name = runs_of(m, key)
        .and_then(|r| r.subs.iter().find(|sa| sa.id == id))
        .map(sub_task)
        .unwrap_or_default();
    let hint = match &rt {
        Ok(rt) => rt.hint(m),
        Err(e) => e.clone(),
    };
    let send_now = super::spawned::send_now_key(m);
    let q = queue_of(m, key, id);
    let focused = focused && rt.is_ok();
    let d = draft(m, key, id).clone();
    let mut out = Vec::new();
    if !q.items.is_empty() {
        let pick = d.pick.filter(|_| focused);
        let when = if q.held {
            line([sp(format!(" · {send_now} sends it now"), DIM)])
        } else {
            line([sp(" · one goes each time a step ends", DIM)])
        };
        let picked = pick.map(|i| format!("q:{i}")).unwrap_or_default();
        let (rows, _) =
            super::dock::queue_rows(&q, when, &picked, !d.input.is_empty(), &send_now, w);
        out.extend(rows);
    }
    let to = rush_draw::truncate(&name, w.saturating_sub(16).max(8), "…").into_owned();
    let top = if d.edit_q > 0 {
        line([
            sp(format!("editing queued message {}", d.edit_q), QUEUE),
            dim(format!(" · to {to}")),
        ])
    } else {
        line([dim(format!("to {to}"))])
    };
    let typing = focused && d.pick.is_none();
    let b = crate::frame::inputbox::InputBox {
        w,
        focused,
        top_l: top,
        foot_l: if rt.is_ok() {
            line([faint(
                rush_draw::truncate(&hint, w.saturating_sub(6), "…").into_owned(),
            )])
        } else {
            Line::new()
        },
        text: d.input.text.clone(),
        cursor: d.input.pos,
        anchor: d.input.anchor,
        hot: d.input.hot,
        holder: match (&rt, focused) {
            (Err(_), _) => rush_draw::truncate(&hint, w.saturating_sub(8), "…").into_owned(),
            (Ok(_), true) => {
                format!("a message for it · enter sends · {send_now} now · esc leaves")
            }
            (Ok(_), false) => "tab or click here to message it".into(),
        },
        lead: line([sp("❯ ", if typing { ORANGE } else { FAINT })]),
        max_rows: 6,
        idle: !typing,
        ..Default::default()
    };
    out.extend(b.lines());
    out
}

/// The keys the foot says while run id's box has them.
pub fn hint(m: &mut Model, key: &str, id: &str, w: usize) -> Line {
    let send_now = super::spawned::send_now_key(m);
    let d = draft(m, key, id);
    if d.pick.is_some() {
        let held = queue_of(m, key, id).held;
        return fit_keys(
            w,
            &[
                ("space", "edit"),
                ("shift+↑↓", "merge up/down"),
                ("[ ]", "move"),
                ("s", "send this now"),
                ("⌫", "drop"),
                ("h", if held { "let go" } else { "hold" }),
                ("esc", "done"),
            ],
        );
    }
    if d.edit_q > 0 {
        return fit_keys(
            w,
            &[
                ("enter", "save it in the queue"),
                ("esc", "leave it as it was"),
            ],
        );
    }
    // The run it goes to, by name: the one box on screen says whose it is.
    let name = runs_of(m, key)
        .and_then(|r| r.subs.iter().find(|sa| sa.id == id))
        .map(sub_task)
        .unwrap_or_default();
    let to = if name.is_empty() {
        "send to the subagent".to_string()
    } else {
        format!("send to {}", rush_draw::truncate(&name, 32, "…"))
    };
    let mut pairs = vec![("enter", to.as_str()), (send_now.as_str(), "send now")];
    if !queue_of(m, key, id).items.is_empty() {
        pairs.push(("↑", "the queue"));
    }
    if conn(m, key).runs.hosted_of(id).is_some() {
        pairs.push(("alt+o", "its own session"));
    }
    pairs.push(("ctrl+v", "image"));
    if super::runview::open_id(m, key).is_some() {
        pairs.push(("esc", "back to subagents"));
    } else {
        pairs.push(("tab · esc", "back to the list"));
    }
    fit_keys(w, &pairs)
}

/// What's typed, as it goes: each paste whole, each image as its path.
fn take_text(d: &mut Draft) -> String {
    let text = d.input.sent(true);
    let (text, mut images) = d.imgs.resolve(&text);
    let (text, typed) = extract_images(&text, &look_disk);
    images.extend(typed);
    with_images(text.trim(), &images).trim().to_string()
}

/// Keys while run id's box has them (keys::key's for a Session's box).
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if !conn(m, &key).watch.msg_focus {
        return Out::Pass;
    }
    let Some(id) = super::subview::box_run(m, &key)
        .filter(|_| super::views::view_name(m, &key) == "subagents")
    else {
        conn(m, &key).watch.msg_focus = false;
        return Out::Pass;
    };
    // A box no message can reach any more (its session stopped, its run
    // not found) lets the keys go rather than keep them (Rust-only).
    if super::relay::route(m, &key, &id).is_err() {
        conn(m, &key).watch.msg_focus = false;
        return Out::Pass;
    }
    run_key(m, &key, &id, k)
}

/// Key k in run id's box: typing, sending, its queue's keys. The box
/// beside the list's (key) and a run open whole's (runview.rs).
pub fn run_key(m: &mut Model, key: &str, id: &str, k: &Press) -> Out {
    let (key, id) = (key.to_string(), id.to_string());
    if super::relay::route(m, &key, &id).is_err() {
        return Out::Pass;
    }
    let s = k.s.as_str();
    if draft(m, &key, &id).pick.is_some() {
        return queue_key(m, &key, &id, s);
    }
    let empty = empty(m, &key, &id);
    let editing = draft(m, &key, &id).edit_q > 0;
    match s {
        "esc" if editing => {
            draft(m, &key, &id).input.clear();
            end_edit(m, &key, &id);
            Out::Draw
        }
        // esc on an empty box of a run picked from elsewhere goes back
        // there at once, as it would from the list (Rust-only).
        "esc" if empty && conn(m, &key).watch.back => {
            super::subview::go_back(m, &key);
            Out::Draw
        }
        // esc and tab give the keys back to the list; what's typed stays.
        "esc" | "tab" => {
            conn(m, &key).watch.msg_focus = false;
            Out::Draw
        }
        // ← from an empty box goes back to the list.
        "left" if empty => {
            conn(m, &key).watch.msg_focus = false;
            Out::Draw
        }
        // ↑ on an empty box picks the queue's last, as a Session's does.
        "up" if empty && !queue_of(m, &key, &id).items.is_empty() => {
            let n = queue_of(m, &key, &id).items.len();
            draft(m, &key, &id).pick = Some(n - 1);
            Out::Draw
        }
        // ↑↓ from an empty box with nothing queued go back through the
        // list (Rust-only).
        "up" | "down" if empty && !editing => {
            conn(m, &key).watch.msg_focus = false;
            Out::Pass
        }
        // Its own session, where what it asks is answered (Rust-only).
        "alt+o" => {
            let host = conn(m, &key).runs.hosted_of(&id).map(|x| x.hosted.clone());
            if let Some(h) = host {
                super::spawned::open_session(m, &h);
            }
            Out::Draw
        }
        // Editing a queued message, any send saves it in place, as a
        // Session's does: the old text never goes too (keys.rs send).
        "enter" | "ctrl+enter" if editing => {
            save_edit(m, &key, &id);
            Out::Draw
        }
        "enter" | "ctrl+enter" => {
            let now = s != "enter";
            let d = draft(m, &key, &id);
            // A line ending in \ goes on to a new line (keys::send).
            if !now && d.input.string().ends_with('\\') {
                let t = d.input.string();
                d.input.set(&format!("{}\n", &t[..t.len() - 1]));
                return Out::Draw;
            }
            let text = take_text(d);
            if text.is_empty() {
                if now && !queue_of(m, &key, &id).items.is_empty() {
                    return super::relay::send_to(m, &key, &id, String::new(), true);
                }
                return Out::Idle;
            }
            draft(m, &key, &id).input.clear_sent();
            draft(m, &key, &id).imgs = ImageRefs::default();
            super::relay::send_to(m, &key, &id, text, now)
        }
        "alt+h" => {
            hold(m, &key, &id);
            Out::Draw
        }
        "ctrl+v" | "super+v" => clip_image(m, &key, &id),
        _ => {
            let d = draft(m, &key, &id);
            match d.input.key(k) {
                Some(copied) => {
                    if s == "space" || s == "enter" {
                        super::keys::inline_typed(&mut d.input, &mut d.imgs);
                    }
                    if !copied.is_empty() {
                        m.copy_text(copied);
                    }
                    Out::Draw
                }
                // Not an editing key: kept from the rest while it types.
                None if s.starts_with("ctrl+c") => Out::Pass,
                None => Out::Idle,
            }
        }
    }
}

/// A key on run id's picked queued message (queue::key's).
fn queue_key(m: &mut Model, key: &str, id: &str, s: &str) -> Out {
    let n = queue_of(m, key, id).items.len();
    let Some(i) = draft(m, key, id).pick.filter(|&i| i < n) else {
        draft(m, key, id).pick = None;
        return Out::Draw;
    };
    let pick = |m: &mut Model, p: Option<usize>| draft(m, key, id).pick = p;
    match s {
        "up" => pick(m, Some(i.saturating_sub(1))),
        // Down off the last gives the box back.
        "down" => pick(m, (i + 1 < n).then_some(i + 1)),
        "esc" | "tab" | "left" => pick(m, None),
        "space" | " " | "e" | "enter" => {
            let text = local_q(m, key, id).items[i].clone();
            let held = queue_of(m, key, id).held;
            let d = draft(m, key, id);
            d.input.set(&text);
            d.edit_was = text;
            d.edit_q = i + 1;
            d.edit_held = !held;
            d.pick = None;
            if !held {
                local_q(m, key, id).held = true;
            }
        }
        "shift+up" => {
            if i == 0 {
                m.flash_err("nothing before it to merge with");
                return Out::Draw;
            }
            merge(m, key, id, i - 1);
            pick(m, Some(i - 1));
        }
        "shift+down" | "m" | "alt+m" => {
            if i + 1 >= n {
                m.flash_err("nothing after it to merge with");
                return Out::Draw;
            }
            merge(m, key, id, i);
        }
        "[" | "]" | "alt+up" | "alt+down" => {
            let down = s == "]" || s == "alt+down";
            let to = if down { i + 1 } else { i.wrapping_sub(1) };
            if to >= n {
                return Out::Idle;
            }
            local_q(m, key, id).items.swap(i, to);
            pick(m, Some(to));
        }
        "s" => {
            let text = local_q(m, key, id).items.remove(i);
            pick(m, (n > 1).then_some(i.min(n - 2)));
            super::relay::tell_now(m, key, id, text);
        }
        "backspace" | "delete" | "ctrl+x" => {
            local_q(m, key, id).items.remove(i);
            pick(m, (n > 1).then_some(i.min(n - 2)));
        }
        "h" | "alt+h" => hold(m, key, id),
        _ => return Out::Idle,
    }
    Out::Draw
}

/// Merges run id's queued message i with the one after it.
fn merge(m: &mut Model, key: &str, id: &str, i: usize) {
    let q = local_q(m, key, id);
    if i + 1 >= q.items.len() {
        return;
    }
    let joined = join_queue(&q.items[i..=i + 1]);
    q.items[i] = joined;
    q.items.remove(i + 1);
}

/// Holds run id's queue, or lets it go (holdQueue).
fn hold(m: &mut Model, key: &str, id: &str) {
    draft(m, key, id).edit_held = false; // yours now, not the edit's
    let q = local_q(m, key, id);
    q.held = !q.held;
    let held = q.held;
    m.flash(if held {
        "its queue holds · nothing in it goes until you let it go"
    } else {
        "its queue goes on"
    });
}

/// Saves the box over the queued message being edited, where it still
/// is (it may have moved, or gone); emptied, it's dropped (queueEdit's
/// save, keys.rs save_edit). One sent meanwhile stays in the box, said.
fn save_edit(m: &mut Model, key: &str, id: &str) {
    let d = draft(m, key, id);
    let i = d.edit_q - 1;
    let was = std::mem::take(&mut d.edit_was);
    let text = take_text(d);
    d.input.clear_sent();
    d.imgs = ImageRefs::default();
    let q = local_q(m, key, id);
    let at = if q.items.get(i) == Some(&was) {
        Some(i)
    } else {
        q.items.iter().position(|it| *it == was)
    };
    match at {
        Some(i) if text.is_empty() => {
            q.items.remove(i);
        }
        Some(i) => q.items[i] = text,
        None => {
            end_edit(m, key, id);
            give_back(m, key, id, &text);
            m.flash_err("that message has already been sent · your edit is in the box");
            return;
        }
    }
    end_edit(m, key, id);
}

/// Done editing a queued message: the queue goes on if the edit held it.
fn end_edit(m: &mut Model, key: &str, id: &str) {
    let d = draft(m, key, id);
    let held = d.edit_held;
    d.edit_q = 0;
    d.edit_held = false;
    d.edit_was.clear();
    if held {
        local_q(m, key, id).held = false;
    }
}

/// ctrl+v: the image on the clipboard, attached where the cursor is
/// (clipimage.rs's, into run id's box).
fn clip_image(m: &mut Model, key: &str, id: &str) -> Out {
    m.flash("reading the clipboard…");
    let (key, id) = (key.to_string(), id.to_string());
    m.later(rush_ui_session_cards::clipimage::clip_image, move |m, r| {
        match r {
            Err(e) => m.fail(&format!("couldn't read the clipboard: {e}")),
            Ok(None) => m.flash("no image on the clipboard"),
            Ok(Some(p)) => attach(m, &key, &id, &[p]),
        }
        Out::Draw
    });
    Out::Draw
}

/// Attaches images to run id's box at its cursor (cards_host's
/// attach_images).
pub fn attach(m: &mut Model, key: &str, id: &str, imgs: &[String]) {
    let d = draft(m, key, id);
    let (text, pos) = (d.input.text.clone(), d.input.pos.min(d.input.text.len()));
    d.input.undo.save(&text, pos, false);
    let marked = crate::imagerefs::with_marks(&text, pos, &mut d.imgs, imgs);
    d.input.pos = pos + (marked.len() - text.len());
    d.input.text = marked;
    d.input.anchor = None;
}

/// A paste while run id's box has the keys goes into it: image files as
/// images, a long paste as its chip.
pub fn paste(m: &mut Model, text: &str) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if !conn(m, &key).watch.msg_focus {
        return Out::Pass;
    }
    let Some(id) = super::subview::box_run(m, &key) else {
        return Out::Pass;
    };
    let d = draft(m, &key, &id);
    d.pick = None;
    let mut t = clean_paste(text);
    if let Some(x) = d.imgs.inline(&t, &look_disk) {
        t = x;
    }
    d.input.paste(&t, true);
    Out::Draw
}
