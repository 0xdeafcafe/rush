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
//! (DESIGN.md "Following"). Over SSH (RUSH_VIA=ssh) it follows while
//! another view holds the lock, as Go's does (main.go:252-260).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::model::Model;

/// Set over SSH: whether this rush is to follow while another view holds
/// the lock, and whether one does, as last read (main.go:252-260).
static SSH: AtomicBool = AtomicBool::new(false);
static LED: AtomicBool = AtomicBool::new(false);

/// Makes this rush follow while led says another view holds the lock,
/// read now and every 2 seconds after: a rush over SSH leads only while
/// no view on the Mac does (Go's Follow(&led) with instances.Led).
pub fn follow_while_led(led: fn() -> bool) {
    LED.store(led(), Ordering::Relaxed);
    SSH.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            LED.store(led(), Ordering::Relaxed);
        }
    });
}

/// Whether another view leads this one now: only ever over SSH.
pub fn others_lead() -> bool {
    SSH.load(Ordering::Relaxed) && LED.load(Ordering::Relaxed)
}

/// The variable that makes a rush follow.
pub const FOLLOW_ENV: &str = "RUSH_FOLLOW";

/// Whether RUSH_FOLLOW is set to anything.
pub fn follow_env() -> bool {
    std::env::var_os(FOLLOW_ENV).is_some_and(|v| !v.is_empty())
}

impl Model {
    /// Whether another rush leads (following).
    pub fn following(&self) -> bool {
        self.follows || others_lead()
    }

    /// Whether this rush does what rush does for every session on its own,
    /// not a hosted view nor one that follows (leads).
    pub fn leads(&self) -> bool {
        self.ext.calls.hosted_id()(self).is_none() && !self.following()
    }
}

fn never() -> bool {
    false
}

fn unless_others_lead() -> bool {
    !others_lead()
}

/// Whether a usage reading may refresh an expired sign-in on its own, for
/// as long as m runs: only while it leads, so a following or hosted rush
/// never refreshes the sign-ins of the rush it follows (one over SSH may
/// once no view on the Mac leads).
pub fn renews(m: &Model) -> fn() -> bool {
    if m.follows || m.ext.calls.hosted_id()(m).is_some() {
        never
    } else {
        unless_others_lead
    }
}

/// Holds the sign-in refreshes usage readings make on their own to
/// whether m leads (renews), before anything is read.
pub fn hold_renewals(m: &Model) {
    rush_claude::oauth::renew_while(renews(m));
}

/// Notes in the events log whether this rush leads, once as it starts
/// (model.go:1096; Go's can change, this one's can't).
pub fn log_lead(m: &Model) {
    let mut f = serde_json::Map::new();
    f.insert("leads".into(), m.leads().into());
    f.insert(
        "hosted".into(),
        m.ext.calls.hosted_id()(m).unwrap_or("").into(),
    );
    f.insert("following".into(), m.following().into());
    rush_events::log("lead", f);
}
