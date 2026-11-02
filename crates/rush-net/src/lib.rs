//! What rush knows about the network.
//!
//! - [`netwatch`]: this process's own view: whether the API can be reached,
//!   which network the machine is on and when that changed, how fast bytes
//!   are moving, and which of rush's jobs wait for the network.
//! - [`netproof`]: what every rush process knows together about whether the
//!   API holds up, kept in one file per API under rush's folder.
//!
//! Neither knows the events log (that module comes later, and imports
//! this one): what changed is told to the hook [`set_on_event`] installs.

pub mod netproof;
pub mod netwatch;
mod url;
mod when;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// An event's fields, as the events log writes them.
pub type Fields = serde_json::Map<String, serde_json::Value>;

type Hook = Arc<dyn Fn(&str, Fields) + Send + Sync>;

static ON_EVENT: RwLock<Option<Hook>> = RwLock::new(None);

/// Sets what is told what changed: the API going down or coming back, the
/// machine moving network, a job held back, proof the API holds. The events
/// log sets it (it can't be imported from here); it must not block, nor
/// call back into this crate.
pub fn set_on_event(f: impl Fn(&str, Fields) + Send + Sync + 'static) {
    *ON_EVENT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

pub(crate) fn event(kind: &str, fields: &[(&str, serde_json::Value)]) {
    let hook = ON_EVENT.read().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(f) = hook {
        let m = fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        f(kind, m);
    }
}

/// rush's folder (Go internal/state/state.go:27): RUSH_HOME, else
/// ~/.config/rush, or ~/.config/agtop while that's the one in use (rush's
/// has no config.json in it yet and agtop's has).
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RUSH_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let rush = home.join(".config").join("rush");
    let agtop = home.join(".config").join("agtop");
    if !rush.join("config.json").exists() && agtop.join("config.json").exists() {
        return agtop;
    }
    rush
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
