//! The stash from the Agents Prompt: ctrl+p sets what's typed aside,
//! ctrl+r (with text typed) and #stash open what was kept. The stash is
//! the bundled drafts plugin; rush runs its stash and history commands
//! on the box with the keys, or says it isn't running. After Go's
//! internal/ui/stash.go (stash :101, stashCommand :150), keybind.go:208-211
//! (prompt.stash, prompt.history) and keys.go:985 (#stash).

use rush_bundled::drafts::{Words, words_for};

use crate::frame::Out;
use crate::model::Model;
use crate::screens::plugin_ui::STASH_PLUGIN;
use crate::screens::plugin_ui::hooks::{box_state, ui_session};

/// What the stash is called, and whether it's running: from what the
/// plugin broker last said (stash).
pub fn stash(m: &Model) -> (Words, bool) {
    if let Some(h) = m.hooks()
        && let Some(p) = h.state().plugins.iter().find(|p| p.name == STASH_PLUGIN)
    {
        let called = p.values.get("called").map_or("", String::as_str);
        return (words_for(called), true);
    }
    (words_for(""), false)
}

/// Runs the stash's command, "stash" or "history", on the box with the
/// keys, off the UI thread (stashCommand).
pub fn command(m: &mut Model, cmd: &str) -> Out {
    let (w, on) = stash(m);
    if !on {
        m.flash_err(&format!(
            "{} isn't running · Settings, Plugins has it, as {STASH_PLUGIN}",
            w.title
        ));
        return Out::Draw;
    }
    let Some(h) = m.hooks() else {
        return Out::Idle;
    };
    // In a Session, its agent's box; else the list's pick (stashCommand).
    let s = match m.session.as_ref() {
        Some(v) => m.agents.iter().find(|a| a.key == v.key).map(ui_session),
        None => m.selected_row().map(|a| ui_session(&a)),
    };
    let (input, who) = match box_state(m) {
        Some((b, who)) => (Some(b), who),
        None => (None, String::new()),
    };
    let cmd = cmd.to_string();
    let title = w.title;
    m.later(
        move || h.command(STASH_PLUGIN, &cmd, s.as_ref(), &who, input.as_ref()),
        move |m, r| match r {
            Ok(_) => Out::Idle,
            Err(e) => {
                m.fail(&format!("{title}: {e}"));
                Out::Draw
            }
        },
    );
    Out::Idle
}
