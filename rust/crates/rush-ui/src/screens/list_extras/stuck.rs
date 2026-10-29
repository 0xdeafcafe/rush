//! What rush says to a session gone quiet: a subagent run quiet past its
//! prompt cache is moved to the background and the session told to look
//! at what it did; a session silent at work is asked what it waits on.
//! After Go's internal/ui/stucksubs.go (nudgeStuckSub :72, askStuck
//! :104). Sending them waits on rush hosting sessions (host.Dial); these
//! are the words, the same in both.

use rush_core::time::Ms;
use rush_fleet::STUCK_SUB_AFTER;

/// What the session is told of its subagent what (its description, else
/// its type) gone quiet.
pub fn nudge_text(what: &str) -> String {
    format!(
        "Your subagent {what:?} has written nothing for {} minutes. rush moved it to the background so you can go on: \
         look at what it has done so far. If it's good, continue; if not, stop it and do the work another way.",
        STUCK_SUB_AFTER / 60_000
    )
}

/// Why the nudge was sent, for the session's log.
pub fn nudge_why() -> String {
    format!(
        "its subagent wrote nothing for {} minutes",
        STUCK_SUB_AFTER / 60_000
    )
}

/// What rush says once it has nudged name.
pub fn nudge_done(name: &str) -> String {
    format!("{name}'s subagent went quiet · moved to the background, {name} checks it")
}

/// What a session silent for quiet is asked, mid-turn, without stopping
/// it (askStuck).
pub fn ask_text(quiet: Ms) -> String {
    format!(
        "You have written nothing for {} minutes. What are you waiting on? \
         If a command or subagent hung, move it to the background or stop it, and carry on another way.",
        quiet / 60_000
    )
}

/// What rush says once it has asked name.
pub fn ask_done(name: &str) -> String {
    format!("asked {name} what it's waiting on")
}
