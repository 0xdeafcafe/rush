//! Conversations a usage limit stopped, handed on to the next provider
//! with room once none of their own provider's accounts has any, under a
//! profile that hands on. Only the rush that leads does it, once per
//! session. After Go's internal/ui/profiles.go (handOffStopped :136, and
//! handOff's rush branch :170-182).
//!
//! The move itself is a switch, as one you'd pick: the sessions' screens
//! do it through [`super::state::Accounts::switcher`] (Go's
//! handOverMessage), so the stopped one goes once the new one runs.
//! Until they set one, nothing is handed on.

use rush_core::adapter::Feature;
use rush_core::agent::Agent;
use rush_state::profiles::{LIMIT_HANDOFF, Pick, Profile, Room};
use serde_json::json;

use super::look::agent_name;
use super::rows::room;
use super::setup::{account_name, profile_setup};
use super::state::{Switcher, accounts, cfg, peek};
use crate::frame::Out;
use crate::model::Model;

/// Hands each conversation a usage limit stopped, under a profile that
/// hands on, to the next provider with room that can start from
/// another's conversation, once none of its own provider's accounts has
/// any (handOffStopped). Only where rush leads, and once the sessions'
/// screens can move one.
pub fn hand_off_stopped(m: &mut Model) -> Out {
    if accounts(m).switcher.is_none() || !m.leads() {
        return Out::Idle;
    }
    let r = room(m);
    hand_off_stopped_in(m, &r, |m, a| cfg(m).profile_for(&a.job.cwd, &a.profile))
}

/// hand_off_stopped given what room each provider has, and the profile
/// each session runs under (sessionProfile).
pub(crate) fn hand_off_stopped_in(
    m: &mut Model,
    r: &Room,
    profile: impl Fn(&Model, &Agent) -> Profile,
) -> Out {
    let Some(switch) = accounts(m).switcher else {
        return Out::Idle;
    };
    if !m.leads() {
        return Out::Idle;
    }
    let mut picks: Vec<(Agent, Pick, Profile)> = Vec::new();
    for a in &m.agents {
        if !a.rush
            || !a.job.detail.starts_with("usage limit")
            || peek(m).handed_off.contains(&a.key)
        {
            continue;
        }
        let p = profile(m, a);
        if p.limit() != LIMIT_HANDOFF {
            continue;
        }
        let supports = |k: &str| m.reg.supports(k, Feature::HandoffIn);
        if let Some(to) = hand_off_to(&p, a.kind.as_str(), r, supports) {
            picks.push((a.clone(), to, p));
        }
    }
    let mut out = Out::Idle;
    for (a, to, p) in picks {
        accounts(m).handed_off.insert(a.key.clone());
        if hand_off(m, switch, &a, &to, &p) == Out::Draw {
            out = Out::Draw;
        }
    }
    out
}

/// Where a conversation of kind a limit stopped goes under profile p:
/// nowhere while another of its own provider's accounts has room
/// (switching comes first), else the next provider with room that can
/// start from another's conversation (handOffStopped's takers).
pub fn hand_off_to(
    p: &Profile,
    kind: &str,
    r: &Room,
    supports: impl Fn(&str) -> bool,
) -> Option<Pick> {
    if p.pick_for(kind, r).1 {
        return None;
    }
    let mut takers = p.clone();
    takers.providers.retain(|pr| {
        let k = p.kind_of(pr);
        k == kind || supports(&k)
    });
    takers.next(kind, r)
}

/// Moves rush session a on to provider to, under profile p, as a switch
/// you'd pick: the stopped one goes once the new one runs, rather than
/// two of it on the list (handOff's rush branch).
fn hand_off(m: &mut Model, switch: Switcher, a: &Agent, to: &Pick, p: &Profile) -> Out {
    rush_host::events::log_event(
        &a.job.id,
        "handoff",
        json!({
            "key": a.key,
            "name": a.display_name,
            "from": a.kind.as_str(),
            "to": to.kind,
            "account": to.account.id,
            "profile": p.name,
            "why": "usage limit, and none of its provider's accounts has room",
        }),
    );
    let mut o = profile_setup(m, p, &to.kind);
    o.account = account_name(m, &to.kind, &to.account.id);
    o.profile = p.name.clone();
    m.flash(&format!(
        "{} is out of {} · moving it to {}",
        a.display_name,
        agent_name(&m.reg, a.kind.as_str()),
        agent_name(&m.reg, &to.kind)
    ));
    switch(m, &a.key, o, "")
}
