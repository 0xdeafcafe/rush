//! The Session's commands a key runs through the keymap (ctrl+] and the
//! letter, or a key you bound): its turns all open or folded, how deep its
//! steps show, the sheet to switch what it runs as, its permission mode,
//! and what's typed moved to Agents' Prompt. After Go's
//! internal/ui/keybind.go:155-189 (runAction's session cases), :221
//! (depthName), :227 (draftToPrompt) and :244-310 (carryDraft, carrying,
//! carryKey, putDraft): what's typed carried to another session. Not yet:
//! closing it (askClose).

use rush_convo::Depth;

use super::{conn, history, open_key};
use crate::frame::Out;
use crate::model::Model;

/// What depth n shows (depthName).
fn depth_name(n: usize) -> &'static str {
    [
        "m0 · only what's said",
        "m1 · every run of steps folded",
        "m2 · as usual",
        "m3 · everything open",
    ][n]
}

/// Runs the Session's action id; Pass for one that isn't the Session's.
pub fn run_action(m: &mut Model, id: &str) -> Out {
    match id {
        "session.history.open" | "session.history.close" => {
            history::set_fold(m, id == "session.history.open")
        }
        "session.depth.0" | "session.depth.1" | "session.depth.2" | "session.depth.3" => {
            let n = (id.as_bytes()[id.len() - 1] - b'0') as usize;
            let Some(v) = m.session.as_mut() else {
                return Out::Idle;
            };
            v.o.depth = [Depth::Prose, Depth::Runs, Depth::Default, Depth::Default][n];
            v.verbose = n == 3;
            m.flash(depth_name(n));
            Out::Draw
        }
        "session.setup" => {
            let Some(key) = open_key(m) else {
                return Out::Idle;
            };
            match crate::screens::bars::agent::session_start(m, &key) {
                Some(o) => crate::screens::accounts::startsheet::open_switch_sheet(m, &key, o),
                None => Out::Idle,
            }
        }
        "session.mode" => match open_key(m) {
            Some(key) => {
                crate::screens::session_cards::permissions::permission_command(m, &key, "", false)
            }
            None => Out::Idle,
        },
        "session.toprompt" => draft_to_prompt(m),
        "session.carry" => carry_draft(m),
        _ => Out::Pass,
    }
}

/// What's typed in the Session's box moved to Agents' Prompt, to start a
/// new agent with (draftToPrompt).
fn draft_to_prompt(m: &mut Model) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Idle;
    };
    let c = conn(m, &key);
    if c.input.is_empty() {
        m.flash("nothing typed to move");
        return Out::Draw;
    }
    // Moved, not gone: neither box has a step to undo for it, but what
    // the Prompt held is kept for undo.
    let (text, pastes) = (c.input.text.clone(), std::mem::take(&mut c.input.pastes));
    c.input.clear();
    c.input.undo.moved();
    crate::screens::cmdbar::bar::to_prompt(m);
    let p = &mut m.input;
    if !p.is_empty() {
        p.undo.save(&p.text.clone(), p.pos, false);
    }
    p.undo.moved();
    p.pos = text.len();
    p.text = text;
    p.anchor = None;
    p.pastes = pastes;
    m.flash("moved to the Prompt · enter starts a new agent with it");
    Out::Draw
}

/// The session a draft lifted into the Prompt came from, while it's
/// carried (Go's m.carryFrom).
#[derive(Default)]
struct CarryFrom(String);

/// What's typed in the Session's box lifted into Agents' Prompt to take to
/// another session, typed in the wrong one: ↑↓ pick it in the list, enter
/// puts it in its box and esc brings it back (carryDraft).
fn carry_draft(m: &mut Model) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Idle;
    };
    if conn(m, &key).input.is_empty() {
        m.flash("nothing typed to move");
        return Out::Draw;
    }
    if !m.input.is_empty() {
        m.flash_err("the Prompt has something typed in it already: clear it first");
        return Out::Draw;
    }
    let c = conn(m, &key);
    let (text, pastes) = (c.input.text.clone(), std::mem::take(&mut c.input.pastes));
    c.input.clear();
    c.input.undo.moved();
    crate::screens::cmdbar::bar::to_prompt(m);
    let p = &mut m.input;
    p.undo.moved();
    p.pos = 0;
    p.text = text;
    p.anchor = None;
    p.pastes = pastes;
    m.kept::<CarryFrom>().0 = key;
    m.flash("↑↓ pick the session · enter puts it in its box · esc brings it back");
    Out::Draw
}

/// Whether the Prompt holds a draft lifted from a session; one sent,
/// cleared or put down some other way isn't (carrying).
fn carrying(m: &mut Model) -> bool {
    let gone = m.input.is_empty() || m.session.is_some();
    let c = m.kept::<CarryFrom>();
    if !c.0.is_empty() && gone {
        c.0.clear();
    }
    !c.0.is_empty()
}

/// enter and esc in the list while carrying a draft: enter puts it in the
/// picked session's box, esc in the one it came from (carryKey).
pub fn carry_key(m: &mut Model, k: &crate::frame::Press) -> Out {
    if !carrying(m) {
        return Out::Pass;
    }
    let to = match k.s.as_str() {
        "enter" => match m.selected() {
            Some(i) => m.agents[i].key.clone(),
            None => {
                m.flash_err("pick a session to put it in");
                return Out::Draw;
            }
        },
        "esc" => {
            let from = m.kept::<CarryFrom>().0.clone();
            if !m.agents.iter().any(|a| a.key == from) {
                m.kept::<CarryFrom>().0.clear();
                m.flash_err("the session it came from has gone · it stays in the Prompt");
                return Out::Draw;
            }
            from
        }
        _ => return Out::Pass,
    };
    if to.starts_with(crate::screens::session_cards::mention::ROOM_KEY_PREFIX) {
        m.flash_err("a room's box takes no draft: pick a session");
        return Out::Draw;
    }
    let text = std::mem::take(&mut m.input.text);
    let pastes = std::mem::take(&mut m.input.pastes);
    m.input.clear();
    m.kept::<CarryFrom>().0.clear();
    // In place of nothing, or after what's there (putDraft).
    let c = &mut conn(m, &to).input;
    if c.is_empty() {
        c.text = text;
        c.pastes = pastes;
    } else {
        let s: String = text.iter().collect();
        let moved = c.pastes.unfold(&pastes.expand(&s, Some(0)));
        c.text.push('\n');
        c.text.extend(moved);
    }
    c.pos = c.text.len();
    c.anchor = None;
    m.sel.clone_from(&to);
    let name = match m.agents.iter().position(|a| a.key == to) {
        Some(i) => {
            let name = rush_convo::text::one_line(&m.agents[i].display_name).to_string();
            m.open(i);
            name
        }
        None => String::new(),
    };
    m.flash(&format!("moved to {name}"));
    Out::Draw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_names_are_gos() {
        assert_eq!(depth_name(0), "m0 · only what's said");
        assert_eq!(depth_name(3), "m3 · everything open");
    }
}
