//! Whether this rush leads: does what rush does for every session on its
//! own (ends what agents left running, offers a cleanup, switches logins,
//! compresses transcripts, indexes conversations, asks a model what a
//! finished turn wants, starts the plugin broker), or follows another rush
//! that does. After Go's internal/ui/hosted.go:70-77 (Follow, following,
//! leads) and model.go:1096 (the "lead" event): neither a hosted view
//! (hosted.rs) nor one that follows leads.
//!
//! Rust-only: Go's Follow is set by the app that embeds the view. Here
//! RUSH_FOLLOW, set to anything, makes the whole run follow, so the Rust
//! rush can run beside the Go one on the same home while the Go one leads
//! (DESIGN.md "Following").

use crate::model::Model;

/// The variable that makes a rush follow.
pub const FOLLOW_ENV: &str = "RUSH_FOLLOW";

/// Whether RUSH_FOLLOW is set to anything.
pub fn follow_env() -> bool {
    std::env::var_os(FOLLOW_ENV).is_some_and(|v| !v.is_empty())
}

impl Model {
    /// Whether another rush leads (following).
    pub fn following(&self) -> bool {
        self.follows
    }

    /// Whether this rush does what rush does for every session on its own,
    /// not a hosted view nor one that follows (leads).
    pub fn leads(&self) -> bool {
        crate::hosted::id(self).is_none() && !self.following()
    }
}

/// Notes in the events log whether this rush leads, once as it starts
/// (model.go:1096; Go's can change, this one's can't).
pub fn log_lead(m: &Model) {
    let mut f = serde_json::Map::new();
    f.insert("leads".into(), m.leads().into());
    f.insert("hosted".into(), crate::hosted::id(m).unwrap_or("").into());
    f.insert("following".into(), m.following().into());
    rush_events::log("lead", f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leads_unless_it_follows() {
        let mut m = Model::new("t");
        m.follows = false;
        assert!(m.leads() && !m.following());
        m.follows = true;
        assert!(!m.leads() && m.following());
    }
}
