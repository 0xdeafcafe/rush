//! Accounts, sign-in and starting agents: each provider's look and words,
//! the accounts of every installed agent and their limits, what the next
//! session starts as (#use, the New session and Start sheets, ⌥w's
//! picker of what new sessions run), signing in, Getting started, #ask,
//! and the info sheet's tabs. After Go's internal/ui accounts.go,
//! logins.go, profiles.go, providers.go, setup.go, startsheet.go,
//! newsheet.go, recentsetups.go, signinsheet.go, signedout.go,
//! onboard.go, askrush.go, infosheet.go and infotabs.go; each module names
//! the lines it ports.
//!
//! What the others read: [`look`] (glyphs, colours, names), [`rows`]
//! (account rows, limits, what new sessions start on), [`setup`] (setups
//! and starting), [`state`] (the config store and readings), the keys on
//! an account row in [`keys`] (Settings › Providers draws the rows), and
//! the sheets' openers.

pub mod askrush;
pub mod bits;
pub mod bridge;
pub mod choices;
pub mod info;
pub mod keys;
pub mod look;
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

use crate::frame::{Ext, Out, Press};
use crate::model::Model;

/// Adds this group's keys, commands and hooks to x (frame/ext.rs).
pub fn register(x: &mut Ext) {
    x.keys.push(key);
    x.list_keys.push(list_key);
    x.on_fleet.push(state::on_fleet);
    x.on_fleet.push(onboard::note_progress);
    x.on_fleet.push(recent::on_fleet);
    x.on_fleet.push(askrush::on_fleet);
    x.list_foot.push(onboard::foot);
    x.command(
        "use",
        "what the next session starts as, or in a session switch it (same row; the old one retires), then a message to send it; alone opens the sheet (Shift+Tab)",
        use_command,
    );
    for old in ["preset", "profile", "agent"] {
        x.command(old, "what #use was called", old_use_command);
    }
    x.command("model", "choose the current session’s model", |m, arg| {
        startsheet::open_on(m, 4, arg)
    });
    x.command(
        "effort",
        "choose the current session’s reasoning effort",
        |m, arg| startsheet::open_on(m, 5, arg),
    );
    x.command(
        "with",
        "what the next session starts as, once: #new without a task",
        with_command,
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

/// #use [setup] [message] (setupCommand).
fn use_command(m: &mut Model, arg: &str) -> Out {
    let arg = arg.trim();
    let first = arg.split_whitespace().next().unwrap_or("");
    let msg = setup::after_words(arg, 1).trim().to_string();
    setup::use_setup(m, first, &msg)
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

/// #with: alone, says what the next session starts as (with.go:13).
fn with_command(m: &mut Model, arg: &str) -> Out {
    state::ensure_store(m);
    if !arg.trim().is_empty() {
        m.flash_err("#with harness@provider isn't in this rush yet · #use picks a setup");
        return Out::Draw;
    }
    let dir = rows::start_dir(m);
    let w = setup::start_with_words(m, &dir);
    m.flash(&format!(
        "the next session starts as {w} · #with harness@provider picks another, once"
    ));
    Out::Draw
}
