//! The Session's commands a key runs through the keymap (ctrl+] and the
//! letter, or a key you bound): its turns all open or folded, how deep its
//! steps show, the sheet to switch what it runs as, its permission mode,
//! and what's typed moved to Agents' Prompt. After Go's
//! internal/ui/keybind.go:155-189 (runAction's session cases), :221
//! (depthName) and :227 (draftToPrompt). Not yet: carrying what's typed
//! to another session (carryDraft) and closing it (askClose).

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
    let text = std::mem::take(&mut c.input);
    crate::screens::cmdbar::bar::to_prompt(m);
    m.input = text;
    m.input.anchor = None;
    m.flash("moved to the Prompt · enter starts a new agent with it");
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
