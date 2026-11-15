//! Guests: other agents answering in one conversation rush hosts. Each is
//! a hosted session of its own per (conversation, agent), started the
//! first time it's named; each turn it's told what was said since its
//! last; its replies are cards in the lead's conversation, and the lead is
//! told what guests said atop its next turn; one participant changes
//! files at a time. See docs/guests.md. Rust-only: Go had no guests.

use std::collections::BTreeMap;

pub mod ask;
pub mod chat;
pub mod hook;
pub mod lease;
pub mod mention;
pub mod prompt;
pub mod roster;

pub use ask::{Ask, Asker, ask, route};
pub use lease::{Lease, Took, changes_files};
pub use roster::{Guest, Roster};

/// Meta: the lead's session id, in a guest's config.
pub const GUEST_OF: &str = "guestOf";
/// Meta: the guest's name in the chat ("Kimi").
pub const GUEST_NAME: &str = "guestName";
/// The phase of the exchanges that draw guests in the lead's conversation.
pub const PHASE: &str = "guest";
/// Asks between agents since you last spoke, past which rush holds the
/// rest until you say something.
pub const HOP_CAP: i64 = 6;

/// The conversation session id takes part in, by its config's meta: its
/// lead's id for a guest, its own for a lead that has guests; None for a
/// session with none.
pub fn lead_of(id: &str, meta: &BTreeMap<String, String>) -> Option<String> {
    if let Some(l) = meta.get(GUEST_OF).filter(|s| !s.is_empty()) {
        return Some(l.clone());
    }
    (!id.is_empty() && roster::path(id).exists()).then(|| id.to_string())
}

/// A name as the chat says it: its first letter upper case ("kimi" is
/// Kimi).
pub fn titled(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The lead's name in the chat: its harness's, its first word ("Claude
/// Code" is Claude).
pub fn lead_name(reg: &rush_core::adapter::Registry, kind: &str) -> String {
    let k = if kind.is_empty() {
        rush_core::adapter::LEGACY_KIND
    } else {
        kind
    };
    let label = reg.harness_label(&rush_core::adapter::migrated(k).0);
    let first = label.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        "Lead".into()
    } else {
        titled(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_as_the_chat_says_them() {
        for (n, want) in [
            ("kimi", "Kimi"),
            ("sol", "Sol"),
            ("", ""),
            ("éclair", "Éclair"),
        ] {
            assert_eq!(titled(n), want);
        }
    }

    #[test]
    fn who_takes_part() {
        let lead = "guests-mod-lead";
        let guest = BTreeMap::from([(GUEST_OF.to_string(), lead.to_string())]);
        assert_eq!(lead_of("g1", &guest), Some(lead.into()));
        assert_eq!(lead_of(lead, &BTreeMap::new()), None, "no guests yet");
        roster::update(lead, |r| r.lead = "Claude".into()).unwrap();
        assert_eq!(lead_of(lead, &BTreeMap::new()), Some(lead.into()));
        assert_eq!(lead_of("guests-mod-other", &BTreeMap::new()), None);
    }
}
