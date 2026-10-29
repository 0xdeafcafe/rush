//! What rush does to a session running outside rush mode goes through its
//! agent's adapter (Stopper, Remover, Replier): rush knows no agent's
//! program itself. Port of internal/ui/outside.go:17-47 (stopOutside,
//! removeOutside, replyOutside). A rush-mode session stops through its
//! host (host.StopAll), which is the host's port; these are blocking
//! calls, for Model::later.

use rush_core::adapter::{Kind, Profile, Registry};
use rush_core::agent::Agent;

use crate::session::harness_name;

/// The profile a's session runs under, as its adapter knows it.
fn profile_of(reg: &Registry, a: &Agent) -> Profile {
    reg.get(a.kind.as_str())
        .and_then(|ad| {
            reg.profiles_of(&ad)
                .into_iter()
                .find(|p| p.name == a.job.account)
        })
        .unwrap_or_else(|| Profile {
            kind: a.kind.clone(),
            name: a.job.account.clone(),
            dir: String::new(),
        })
}

/// Ends a's process and keeps its conversation, the agent's own way; one
/// whose agent has none gets a SIGTERM (stopOutside).
pub fn stop_outside(reg: &Registry, a: &Agent) -> Result<(), String> {
    if let Some(ad) = reg.get(a.kind.as_str())
        && let Some(s) = ad.stopper()
    {
        return s
            .stop(&profile_of(reg, a), &a.job.id, a.pid)
            .map_err(|e| e.to_string());
    }
    if a.pid != 0 {
        return rush_sys::actions::terminate(a.pid).map_err(|e| e.to_string());
    }
    Err(format!(
        "{} can't stop {}",
        harness_name(reg, &a.kind),
        a.display_name
    ))
}

/// Deletes a session its agent keeps (removeOutside).
pub fn remove_outside(reg: &Registry, a: &Agent) -> Result<(), String> {
    if let Some(ad) = reg.get(a.kind.as_str())
        && let Some(r) = ad.remover()
    {
        return r
            .remove(&profile_of(reg, a), &a.job.id)
            .map_err(|e| e.to_string());
    }
    Err(format!(
        "{} keeps its sessions itself: rush can't delete them",
        harness_name(reg, &a.kind)
    ))
}

/// Sends text to a session of agent k's background service; one it has
/// let go of fails with adapter::Gone (replyOutside).
pub fn reply_outside(
    reg: &Registry,
    k: &Kind,
    p: &Profile,
    id: &str,
    text: &str,
) -> Result<(), rush_core::adapter::Error> {
    if let Some(ad) = reg.get(k.as_str())
        && let Some(r) = ad.replier()
    {
        return r.reply(p, id, text);
    }
    Err(format!("{} takes no messages from outside it", harness_name(reg, k)).into())
}
