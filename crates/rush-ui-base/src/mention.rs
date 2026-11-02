//! An agent's handle, how it's tagged with @: its title now, kebab-cased
//! (@fix-the-login-bug), with a short id suffix when another session rush
//! runs has the same title; with no title yet, session-<id>. What a
//! message keeps for a session rush runs is its id, @{id}; the names
//! themselves are rush_room::handle's, so the feed, its CLI and the list
//! agree. Tagging and sending to the one tagged are
//! screens/session_cards/mention.rs's. After Go's internal/ui/mention.go
//! (mentionName :29, mentionable :72) and stream.go (handleTint :232).

use rush_core::agent::Agent;
use rush_core::text::Color;

pub use crate::rooms::ROOM_KEY_PREFIX;
use crate::style::*;
pub use rush_room::handle::{WIDTH, kebab, plain, stored, stored_id, trim};

fn fnv32a(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// An agent's host id, when rush runs it: what a mention of it keeps.
pub fn host_id(a: &Agent) -> Option<&str> {
    a.rush.then_some(a.job.id.as_str())
}

/// The session id a mention of a keeps, when rush runs it (not a room).
fn kept_id<'a>(a: &'a Agent, host_id: Option<&'a str>) -> Option<&'a str> {
    host_id.filter(|id| !id.is_empty() && !a.key.starts_with(ROOM_KEY_PREFIX))
}

/// An agent's handle, how it's tagged and drawn, without its @: its
/// title kebab-cased, else plain from its id. A session rush runs (its
/// host's id given) gets a short id suffix when another the list has
/// shares its title (mentionName).
pub fn mention_name(a: &Agent, host_id: Option<&str>) -> String {
    let title = one_line(&a.display_name);
    if let Some(id) = kept_id(a, host_id) {
        return rush_room::handle::known().name(id, &title);
    }
    let k = kebab(&title);
    if !k.is_empty() {
        return k;
    }
    plain(if a.job.id.is_empty() {
        &a.key
    } else {
        &a.job.id
    })
}

/// What an agent's colour is worked out from: what a mention of it
/// keeps, so it keeps its colour when its title changes.
pub fn handle_key(a: &Agent) -> String {
    match kept_id(a, host_id(a)) {
        Some(id) => stored(id),
        None => format!("@{}", mention_name(a, None)),
    }
}

/// An agent's handle as it's drawn: @ and its name, cut to WIDTH.
pub fn drawn_handle(a: &Agent) -> String {
    trim(&format!("@{}", mention_name(a, host_id(a))))
}

/// Whether tag (with or without its @, any case) names agent a: its
/// handle, its title kebab-cased, a kept @{id}, the animal handle rush
/// used to give it, or the start of its handle when no other session's
/// starts so (tags).
pub fn tags(a: &Agent, host_id: Option<&str>, tag: &str) -> bool {
    let t = tag.strip_prefix('@').unwrap_or(tag);
    if t.is_empty() {
        return false;
    }
    if let Some(id) = kept_id(a, host_id)
        && (stored_id(t) == Some(id)
            || rush_room::handle::legacy_name(id).eq_ignore_ascii_case(t)
            || rush_room::handle::known().id_of(t).as_deref() == Some(id))
    {
        return true;
    }
    let eq = |s: &str| !s.is_empty() && s.eq_ignore_ascii_case(t);
    eq(&mention_name(a, host_id)) || eq(&kebab(&one_line(&a.display_name)))
}

/// text as it's kept: each @handle in it that names a session rush runs
/// made @{id}, so it keeps pointing at that session.
pub fn keep(text: &str) -> String {
    rush_room::handle::known().store(text)
}

/// The colour a handle is painted (handleTint).
pub fn handle_tint(h: &str) -> Color {
    let palette = [BLUE, GREEN, YELLOW, QUEUE, ORANGE, RED];
    palette[(fnv32a(h) % palette.len() as u32) as usize]
}

/// Whether a message can reach it from here (mentionable).
pub fn mentionable(a: &Agent) -> bool {
    !a.interactive && !a.headless
}
