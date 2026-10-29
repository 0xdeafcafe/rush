//! Handing a conversation to another harness: a new session on agent
//! `to`, opened with this one's conversation so far; this one is left as
//! it is. After Go's internal/ui/handoff.go (handoffTargets :47,
//! handoffTo :59) and handoff_ready.go (handoffQuiet :12, toldOf :18,
//! awaitCarried :43). Telling the conversation and starting the host
//! (conversationLater, host.Spawn, waitCarried) are the session's:
//! [`Act::Handoff`].

use std::time::{Duration, Instant};

use rush_core::adapter::{AdapterRef, Conversation, Feature, Registry};

use super::{Act, act, info};
use crate::frame::Out;
use crate::model::Model;

/// The agents a conversation on from can be handed to: installed, and
/// able to start from another's (handoffTargets).
pub fn handoff_targets(reg: &Registry, from: &str) -> Vec<AdapterRef> {
    reg.installed_all()
        .into_iter()
        .filter(|a| {
            a.kind().as_str() != from && reg.supports(a.kind().as_str(), Feature::HandoffIn)
        })
        .collect()
}

/// Starts a new session on harness to, opened with agent key's
/// conversation so far (handoffTo). With no to, says which can.
pub fn handoff_to(m: &mut Model, key: &str, to: &str) -> Out {
    let Some(inf) = info(m, key) else {
        m.flash_err("#fork to another harness works on a session rush runs: open one first");
        return Out::Draw;
    };
    let from = inf.kind.as_str();
    if to.is_empty() {
        let names: Vec<String> = handoff_targets(&m.reg, from)
            .iter()
            .map(|a| a.kind().as_str().to_string())
            .collect();
        if names.is_empty() {
            m.flash_err("no other installed harness can take this conversation on");
        } else {
            m.flash(&format!("#fork to which harness? {}", names.join(", ")));
        }
        return Out::Draw;
    }
    let k = to.to_lowercase();
    if k == from {
        m.flash_err(&format!(
            "this conversation is already on {}",
            m.reg.label(&k)
        ));
        return Out::Draw;
    }
    if !m.reg.supports(&k, Feature::HandoffIn) {
        m.flash_err(&format!(
            "{} can't take a conversation on from another harness",
            m.reg.label(&k)
        ));
        return Out::Draw;
    }
    let name = format!("{} · on {}", inf.name, m.reg.label(&k));
    m.flash(&format!("handing {} to {}…", inf.name, m.reg.label(&k)));
    act(m, key, Act::Handoff { to: k, name })
}

/// Whether a session is quiet enough to carry: idle or stopped, nothing
/// queued, waiting, running beside it or retrying (handoffQuiet).
pub fn handoff_quiet(
    state: &str,
    queued: usize,
    needs: &str,
    background: usize,
    retrying: bool,
) -> bool {
    (state == "idle" || state == "stopped")
        && queued == 0
        && needs.is_empty()
        && background == 0
        && !retrying
}

/// Whether c holds any of the conversation to carry on: none means
/// telling it failed, and the source is the only place it still exists
/// (toldOf).
pub fn told_of(c: &Conversation) -> bool {
    !c.first.is_empty() || !c.steps.is_empty() || !c.recent.is_empty() || !c.history.is_empty()
}

/// Reads until ready says the destination took the history, an error, or
/// timeout; every 50 ms (awaitCarried). Off the UI thread only.
pub fn await_carried(
    mut read: impl FnMut() -> Result<bool, String>,
    timeout: Duration,
) -> Result<(), String> {
    let until = Instant::now() + timeout;
    loop {
        if read()? {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err("initialization timed out".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
