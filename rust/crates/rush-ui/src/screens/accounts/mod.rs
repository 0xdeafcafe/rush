//! Accounts, sign-in and starting agents: each provider's look and words,
//! the accounts of every installed agent and their limits, what the next
//! session starts as (#use, the New session and Start sheets, ⌥w's
//! picker of what new sessions run), signing in, Getting started, #ask,
//! and the info sheet's tabs. After Go's internal/ui accounts.go,
//! logins.go, profiles.go, providers.go, setup.go, startsheet.go,
//! newsheet.go, recentsetups.go, signinsheet.go, signedout.go,
//! onboard.go, askrush.go, infosheet.go and infotabs.go, and
//! internal/fleet/logins.go's choice of the next login; each module names
//! the lines it ports.
//!
//! What the others read: [`look`] (glyphs, colours, names), [`rows`]
//! (account rows, limits, what new sessions start on), [`setup`] (setups
//! and starting), [`state`] (the config store and readings), the keys on
//! an account row in [`keys`] (Settings › Providers draws the rows), and
//! the sheets' openers.

pub mod askrush;
pub mod autoswitch;
pub mod bits;
pub mod bridge;
pub mod choices;
pub mod forsettings;
pub mod handoff;
pub mod info;
pub mod keys;
pub mod logins;
pub mod look;
pub mod newcmd;
pub mod newsheet;
pub mod onboard;
pub mod profiles;
pub mod recent;
pub mod rows;
pub mod setup;
pub mod signedout;
pub mod signin;
pub mod startsheet;
pub mod state;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_logins;
#[cfg(test)]
mod tests_newcmd;
#[cfg(test)]
mod tests_no5h;
#[cfg(test)]
mod tests_settings;

use crate::frame::{Ext, Out, Press};
use crate::model::Model;

/// Adds this group's keys, commands and hooks to x (frame/ext.rs).
pub fn register(x: &mut Ext) {
    forsettings::provide();
    x.keys.push(key);
    x.list_keys.push(list_key);
    x.on_fleet.push(logins::on_fleet);
    x.on_fleet.push(state::on_fleet);
    x.on_fleet.push(onboard::note_progress);
    x.on_fleet.push(recent::on_fleet);
    x.on_fleet.push(askrush::on_fleet);
    x.list_foot.push(onboard::foot);
    x.start_tag = Some(look::start_tag);
    x.command(
        "use",
        "what the next session starts as, or in a session switch it (same row; the old one retires), then a message to send it; alone opens the sheet (Shift+Tab)",
        use_command,
    );
    for old in ["preset", "profile", "agent"] {
        x.command(old, "what #use was called", old_use_command);
    }
    // In the session open, #model and #effort are its /model and /effort
    // (keys.go:956-967); else the start sheet's rows.
    x.command("model", "choose the current session’s model", |m, arg| {
        crate::screens::session_core::rushcmd::hash_arg(m, "model", arg)
            .unwrap_or_else(|| startsheet::open_on(m, 4, arg))
    });
    x.command(
        "effort",
        "choose the current session’s reasoning effort",
        |m, arg| {
            crate::screens::session_core::rushcmd::hash_arg(m, "effort", arg)
                .unwrap_or_else(|| startsheet::open_on(m, 5, arg))
        },
    );
    x.command(
        "new",
        "start an agent on any harness, provider, model and effort, once; defaults stay as they are",
        newcmd::new_command,
    );
    x.command(
        "with",
        "what the next session starts as, once: #new without a task",
        newcmd::with_command,
    );
    x.command(
        "ask",
        "ask rush about itself, or have it change a setting for you: an agent in rush's own folder, with its guide",
        askrush::ask_rush,
    );
    x.command(
        "tips",
        "Getting started and tips from the top; off puts them away",
        onboard::tips_command,
    );
}

/// Keys for every screen, before the rest: ⌥w picks what new sessions
/// run, ⌥m what the next one starts as (keys.go:105, :417). Getting
/// started notes the keys it teaches as they go by.
fn key(m: &mut Model, k: &Press) -> Out {
    onboard::saw_key(m, k);
    let free = m.sheet.is_none()
        && m.confirm.is_none()
        && !m.with_shown(|p, _| p.asking()).unwrap_or(false);
    match k.s.as_str() {
        "alt+w" if free => profiles::open_profile_picker(m),
        "alt+m" if free && !m.on_place() => newsheet::open_composer(m),
        _ => Out::Pass,
    }
}

/// The Agents list's keys: shift+tab in the box opens what the next
/// session starts as (keys.go:415).
fn list_key(m: &mut Model, k: &Press) -> Out {
    match k.s.as_str() {
        "shift+tab" if m.ext.agents_box => newsheet::open_composer(m),
        _ => Out::Pass,
    }
}

/// #use [setup] [message] (setupCommand): a switch of the session open
/// when it's the agent picked, else the next session's (keys.go:956).
fn use_command(m: &mut Model, arg: &str) -> Out {
    let arg = arg.trim();
    let first = arg.split_whitespace().next().unwrap_or("");
    let msg = setup::after_words(arg, 1).trim().to_string();
    let key = use_session(m);
    setup::use_setup(m, key.as_deref(), first, &msg)
}

/// The session #use switches: the one open, when it's the agent picked
/// and not a room (keys.go:957-960).
pub fn use_session(m: &Model) -> Option<String> {
    let key = m.session.as_ref().map(|v| v.key.clone())?;
    let a = m.selected_row()?;
    (a.key == key && !crate::screens::social::rooms::is_room_key(&key)).then_some(key)
}

/// #preset, #profile, #agent: #use, as they were, saying what it's
/// called now (setupCommand, keys.go:881).
fn old_use_command(m: &mut Model, arg: &str) -> Out {
    let out = use_command(m, arg);
    if m.flash.is_none() {
        m.flash("rush's own commands start with #: #use");
    }
    out
}
