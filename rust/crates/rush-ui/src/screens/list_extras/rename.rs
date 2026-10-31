//! Renaming an agent from the list, as in the Finder: ctrl+r (or enter,
//! when that's what you chose it does, or #rename) puts its name in the
//! Agents box, all of it selected so typing replaces it; enter saves, tab
//! and ↑↓ save it and go on to rename the next, esc leaves it as it was.
//! After Go's internal/ui/keys.go (the rename step in key :179-185, ctrl+r
//! :405-414, startRename :552, renameStep :566, #rename :1059-1066,
//! submit's inRename :793-805, esc :356-368) and view.go (the box's
//! labels :2726-2728, its row of keys :2787-2790).

use rush_core::text::Line;

use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// The agent being renamed, by key; empty while the box starts agents
/// (Go's inKind == inRename and promptFor).
#[derive(Default)]
pub struct Renaming(pub String);

/// The Agents box's undo while it holds a name being renamed, back once
/// the rename is done: a name's edits aren't the message's to undo.
#[derive(Default)]
struct HeldUndo(Option<crate::frame::editor::Undo>);

/// The agent being renamed, when one is.
pub fn renaming(m: &Model) -> Option<&str> {
    m.kept
        .peek::<Renaming>()
        .map(|r| r.0.as_str())
        .filter(|k| !k.is_empty())
}

/// Puts agent key's name in the box, all of it selected so typing
/// replaces it (startRename).
pub fn start_rename(m: &mut Model, key: &str) {
    let Some(a) = m.agents.iter().find(|a| a.key == key) else {
        return;
    };
    let name: Vec<char> = a.display_name.chars().collect();
    crate::screens::accounts::onboard::did_step(m, "rename");
    m.kept::<Renaming>().0 = key.to_string();
    if m.kept::<HeldUndo>().0.is_none() {
        let u = std::mem::take(&mut m.input.undo);
        m.kept::<HeldUndo>().0 = Some(u);
    }
    m.input.clear();
    m.input.pos = name.len();
    // From the start to the cursor at the end.
    m.input.anchor = (!name.is_empty()).then_some(0);
    m.input.text = name;
}

/// Leaves renaming, the box back to starting agents.
fn stop(m: &mut Model) {
    m.kept::<Renaming>().0.clear();
    if let Some(u) = m.kept::<HeldUndo>().0.take() {
        m.input.undo = u;
    }
}

/// Saves name as agent key's (submit's inRename): empty, or its own
/// name, puts its own name back. Kept in rush's state off the UI thread;
/// the list reads it again once it's written.
pub fn save(m: &mut Model, key: &str, name: &str) {
    let Some(a) = m.agents.iter_mut().find(|a| a.key == key) else {
        return;
    };
    let reset = name.is_empty() || name == a.job.name;
    if reset {
        if !a.job.name.is_empty() {
            a.display_name.clone_from(&a.job.name);
        }
    } else {
        a.display_name = name.to_string();
    }
    let (key, name) = (key.to_string(), name.to_string());
    crate::overlay::save(
        m,
        move |o| {
            if reset {
                o.names.remove(&key);
            } else {
                o.names.insert(key, name);
            }
            true
        },
        |m, r| match r {
            Ok(()) => Out::Idle,
            Err(e) => {
                m.fail(&e);
                Out::Draw
            }
        },
    );
    m.rebuild();
}

/// enter while renaming: the name typed saved, the box emptied.
fn submit(m: &mut Model) {
    let Some(key) = renaming(m).map(str::to_string) else {
        return;
    };
    let text = m.input.sent(false);
    m.input.clear();
    stop(m);
    save(m, &key, text.trim());
}

/// Saves the name being typed and renames the agent d rows on, skipping
/// headings; past either end the rename is done (renameStep).
fn step(m: &mut Model, d: isize) {
    let from = renaming(m).unwrap_or_default().to_string();
    submit(m);
    let items = m.items();
    let Some(i) = items.iter().position(|k| *k == from) else {
        return;
    };
    let mut j = i as isize + d;
    while j >= 0 && (j as usize) < items.len() {
        let k = &items[j as usize];
        if k.starts_with('§') {
            j += d;
            continue;
        }
        m.select(k.clone());
        if m.selected().is_some() {
            start_rename(m, k);
        }
        return;
    }
}

/// The list's keys for renaming: ctrl+r starts it on the agent picked;
/// while renaming, enter saves, tab and ↑↓ go on to the next, esc leaves
/// it. Pass for the rest, which edit the name in the box.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let on = renaming(m).is_some();
    match k.s.as_str() {
        "tab" | "down" if on => step(m, 1),
        "shift+tab" | "up" if on => step(m, -1),
        "enter" if on => submit(m),
        "enter" => return enter_on(m),
        "esc" if on => {
            m.input.clear();
            stop(m);
        }
        "ctrl+r" => match m.selected() {
            None => m.flash_err("select an agent first"),
            Some(_) if !on && !m.input.is_empty() => {
                m.flash_err("finish or clear what's typed first (esc)")
            }
            Some(i) => {
                let key = m.agents[i].key.clone();
                start_rename(m, &key);
            }
        },
        _ => return Out::Pass,
    }
    Out::Draw
}

/// enter on an agent with nothing typed renames it, as in the Finder, or
/// opens it, as you chose the first time you pressed it, which is asked
/// then (keys.go:388-403). A room, or one of its agents, opens the room,
/// and peeking it opens the one picked: the list's own. Until rush's
/// config is read (off the UI thread, after the first reading) enter
/// opens.
fn enter_on(m: &mut Model) -> Out {
    if !m.input.is_empty() || super::peek::peeking(m) {
        return Out::Pass;
    }
    let Some(i) = m.selected() else {
        return Out::Pass;
    };
    let key = m.agents[i].key.clone();
    if m.room_of(&key).is_some() {
        return Out::Pass;
    }
    // rush's one config, once it's read.
    let Some(cfg) = crate::screens::settings::loaded_config(m) else {
        return Out::Pass;
    };
    match cfg.enter_on.as_str() {
        "open" => return Out::Pass,
        "rename" => start_rename(m, &key),
        _ => crate::screens::session_cards::enter::ask_enter(m, &key),
    }
    Out::Draw
}

/// #rename: the agent picked renamed to what's given, or its name put in
/// the box to change (command's "rename").
pub fn command(m: &mut Model, arg: &str) -> Out {
    let Some(i) = m.selected() else {
        m.flash_err("select an agent first");
        return Out::Draw;
    };
    let key = m.agents[i].key.clone();
    if arg.is_empty() {
        start_rename(m, &key);
    } else {
        stop(m);
        save(m, &key, arg.trim());
    }
    Out::Draw
}

/// The box's edge label and what it says empty while renaming agent a
/// (promptLines' inRename).
pub fn labels(m: &Model) -> Option<(Line, String)> {
    let key = renaming(m)?;
    let a = m.agents.iter().find(|a| a.key == key)?;
    Some((
        line([
            sp("rename ", DIM),
            sp(one_line(&a.display_name), TEXT),
            sp(" · enter saves", DIM),
        ]),
        "a new name · empty resets it".into(),
    ))
}

/// The row of keys while renaming.
pub fn hint(w: usize) -> Line {
    keys_fit(
        w,
        &[
            ("enter", "save"),
            ("tab · ↑↓", "save, rename the next"),
            ("esc", "cancel"),
        ],
    )
}
