//! Where this crate's modules record what they did in rush's events log
//! (Go: `events.Log(kind, fields)`, internal/events/events.go:98) and say
//! they wait on the network (Go: `netwatch.Run` / `netwatch.Done`).
//!
//! The events log and netwatch are other modules; the program wires them
//! in once at start with [`set`]. Until it does, events go nowhere and the
//! network is taken as up.

use std::sync::OnceLock;

use serde_json::{Map, Value};

/// Records one event of a kind with its fields.
pub type LogFn = Box<dyn Fn(&str, Map<String, Value>) + Send + Sync>;

/// What the program gives this crate to record events and ask about the
/// network.
pub struct Hooks {
    /// Records one event of `kind` with its fields (Go `events.Log`).
    pub log: LogFn,
    /// Says job is starting to wait on the network; false when rush is
    /// offline and it shouldn't (Go `netwatch.Run`).
    pub net_run: Box<dyn Fn(&str) -> bool + Send + Sync>,
    /// Says job is done with the network (Go `netwatch.Done(job, nil)`).
    pub net_done: Box<dyn Fn(&str) + Send + Sync>,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Wires in the events log and netwatch; only the first call counts.
pub fn set(h: Hooks) {
    let _ = HOOKS.set(h);
}

/// Records an event (Go `events.Log`).
pub fn log(kind: &str, fields: Value) {
    if let Some(h) = HOOKS.get() {
        let m = match fields {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        (h.log)(kind, m);
    }
}

pub(crate) fn net_run(job: &str) -> bool {
    HOOKS.get().is_none_or(|h| (h.net_run)(job))
}

pub(crate) fn net_done(job: &str) {
    if let Some(h) = HOOKS.get() {
        (h.net_done)(job);
    }
}
