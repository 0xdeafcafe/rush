//! Every model rush runs, offered to a session as its own harness's
//! subagents: the list (each model by its plain name, what it's good for,
//! its tier), the tiers an agent can name instead of a model and how rush
//! picks one, the words each harness is told, and the Delegate an adapter
//! answers a call with (spawn_agent's run: a rush-hosted child, Meta
//! spawnedBy). See docs/native-agents.md.
//!
//! Rust-only: Go's sessions reached other agents through spawn_agent and
//! their shell alone.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rush_core::adapter::others::{self, Placement, TRAITS, Tier, tier_name};
use rush_core::adapter::{Delegate, DelegateCall, DelegateRef, OtherAgent, Registry};
use rush_state::Config as Settings;

use crate::agents::{Pick, own_def, picks};

/// The harnesses whose own subagent mechanism rush registers every other
/// model in, and answers a call for one itself: Claude Code (its Agent
/// tool, through --agents and its hooks) and Codex (a tool of its own
/// rush answers). Any provider riding one of them gets the same.
pub const NATIVE: &[&str] = &["claude", "codex"];

/// Whether a session of kind k has every model as its own subagents.
pub fn native(reg: &Registry, k: &str) -> bool {
    NATIVE.contains(&reg.harness_of(k).as_str())
}

/// A model a session may start, as offered.
#[derive(Clone, Debug, Default)]
pub struct Offer {
    pub pick: Pick,
    pub place: Placement,
    /// Its provider, as you'd call it: Moonshot, OpenAI.
    pub provider: String,
    /// One of the harness's own subagent types already (opus in Claude
    /// Code): offered by the harness itself.
    pub own: bool,
}

impl Offer {
    /// What it's for, a line: "Frontier intelligence. OpenAI's; top
    /// tier, vision."
    pub fn description(&self) -> String {
        let mut d = self.pick.desc.clone();
        if !self.provider.is_empty() {
            d += &format!(" {}'s", self.provider);
            if !self.pick.model.is_empty() && self.pick.model != self.pick.name {
                d += &format!(" {}", self.pick.model);
            }
            d += ";";
        }
        d += &format!(" {} tier", self.place.tier.as_str());
        for t in &self.place.traits {
            d += &format!(", {t}");
        }
        d + "."
    }
}

/// The models offered to a session of kind: every agent its harness can
/// hand work to (spawn_agent's), placed into tiers, its harness's own
/// subagent types marked. Config is read on each call.
pub fn offers(reg: &Registry, kind: &str) -> Vec<Offer> {
    let cfg = rush_state::Store::load().config;
    let leader = reg.harness_of(kind);
    offers_of(reg, &cfg, kind, picks(reg, leader.as_str()))
}

/// offers, of picks ps.
pub fn offers_of(reg: &Registry, cfg: &Settings, kind: &str, ps: Vec<Pick>) -> Vec<Offer> {
    let claude = reg.reads_as_claude(kind);
    ps.into_iter()
        .map(|p| Offer {
            place: placement(cfg, &p),
            provider: reg.provider_label(&p.r.prov),
            own: claude && own_def(&p, kind),
            pick: p,
        })
        .collect()
}

/// Where p's model sits: where Settings › Config moved it, else where its
/// name puts it.
pub fn placement(cfg: &Settings, p: &Pick) -> Placement {
    let model = if p.model.is_empty() {
        p.r.models.first().map(|c| c.id.clone()).unwrap_or_default()
    } else {
        p.model.clone()
    };
    let context =
        p.r.models
            .iter()
            .find(|c| c.id == model)
            .map_or(0, |c| c.context);
    let local = p.r.prov == "ollama";
    let mut place = others::place(&model, context, local);
    if let Some(t) = cfg.model_tiers.get(&model).and_then(|t| Tier::parse(t)) {
        place.tier = t;
    }
    place
}

/// The OtherAgents a session of kind registers as its own subagent types:
/// each model its harness doesn't offer itself, then the tiers (with each
/// trait some but not all of a tier's models have). Word for word the
/// same for the same agents whatever their sign-ins: it's part of the
/// prompt a provider caches.
pub fn others_of(offers: &[Offer]) -> Vec<OtherAgent> {
    let mut out: Vec<OtherAgent> = offers
        .iter()
        .filter(|o| !o.own)
        .map(|o| OtherAgent {
            name: o.pick.name.clone(),
            description: o.description(),
        })
        .collect();
    out.extend(tier_agents(offers));
    out
}

/// The tiers as agent types: "top", and "quick-long-context" where a
/// trait narrows the tier.
pub fn tier_agents(offers: &[Offer]) -> Vec<OtherAgent> {
    let mut out = Vec::new();
    for t in Tier::ALL {
        let in_tier: Vec<&Offer> = offers.iter().filter(|o| o.place.tier == t).collect();
        if in_tier.is_empty() {
            continue;
        }
        out.push(OtherAgent {
            name: t.as_str().into(),
            description: format!(
                "The {} tier, for {}: rush picks one of {} that's signed in with quota left, spreading parallel calls across providers.",
                t.as_str(),
                t.what(),
                names(&in_tier)
            ),
        });
        for tr in TRAITS {
            let with: Vec<&Offer> = in_tier
                .iter()
                .copied()
                .filter(|o| o.place.has(tr))
                .collect();
            if with.is_empty() || with.len() == in_tier.len() {
                continue;
            }
            out.push(OtherAgent {
                name: tier_name(t, &[tr]),
                description: format!(
                    "The {} tier's {tr} models: rush picks one of {}.",
                    t.as_str(),
                    names(&with)
                ),
            });
        }
    }
    out
}

fn names(os: &[&Offer]) -> String {
    os.iter()
        .map(|o| o.pick.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The tiers, a line for spawn_agent's list and a session's brief.
pub fn tiers_line(offers: &[Offer]) -> String {
    let parts: Vec<String> = Tier::ALL
        .into_iter()
        .filter_map(|t| {
            let in_tier: Vec<&Offer> = offers.iter().filter(|o| o.place.tier == t).collect();
            (!in_tier.is_empty()).then(|| format!("{} ({})", t.as_str(), names(&in_tier)))
        })
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    format!(
        "Or name a tier and rush picks a model in it that's signed in with quota left, spreading parallel calls across providers: {}. Add long-context, vision or cheap to narrow it (\"quick long-context\").",
        parts.join(", ")
    )
}

/// A model a tier could be run on, as tier_pick weighs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub provider: String,
    pub place: Placement,
    /// Signed in, or has its key.
    pub ready: bool,
    /// Its plan has room, as last read (or no reading says it hasn't).
    pub quota: bool,
}

/// Which of cands a tier (with traits) runs on: one that's ready and has
/// quota, of the provider with fewest of the session's agents at work
/// now (busy), the first listed among equals. None when none fits.
pub fn tier_pick(
    cands: &[Candidate],
    tier: Tier,
    traits: &[&str],
    busy: &BTreeMap<String, usize>,
) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (i, c) in cands.iter().enumerate() {
        if c.place.tier != tier || !traits.iter().all(|t| c.place.has(t)) {
            continue;
        }
        if !c.ready || !c.quota {
            continue;
        }
        let n = busy.get(&c.provider).copied().unwrap_or(0);
        if best.is_none_or(|(_, b)| n < b) {
            best = Some((i, n));
        }
    }
    best.map(|(i, _)| i)
}

/// What a tier with no model to run says, naming why.
pub fn no_tier(tier: Tier, traits: &[&str], cands: &[Candidate]) -> String {
    let asked = tier_name(tier, traits);
    let in_tier: Vec<&Candidate> = cands
        .iter()
        .filter(|c| c.place.tier == tier && traits.iter().all(|t| c.place.has(t)))
        .collect();
    if in_tier.is_empty() {
        return format!(
            "rush has no model in the {asked} tier here; name a model, or another tier"
        );
    }
    let why: Vec<String> = in_tier
        .iter()
        .map(|c| {
            let w = if !c.ready {
                "not signed in"
            } else {
                "out of quota"
            };
            format!("{} ({w})", c.name)
        })
        .collect();
    format!(
        "no model in the {asked} tier can take work now: {}. Name another tier or model",
        why.join(", ")
    )
}

/// Every plan's limits as last read (quotas.json).
pub fn readings() -> BTreeMap<String, rush_core::usage::Quota> {
    rush_core::usage::load(&crate::kind::quotas_path())
}

/// Whether agent k of provider prov has room, as readings last said: one
/// account of its with room is enough, and none read is room.
pub fn has_quota(
    readings: &BTreeMap<String, rush_core::usage::Quota>,
    k: &str,
    prov: &str,
    now: rush_core::time::Ms,
) -> bool {
    let (a, b) = (format!("{k}:"), format!("{prov}:"));
    let mut mine = readings
        .iter()
        .filter(|(key, _)| key.starts_with(&a) || key.starts_with(&b))
        .peekable();
    mine.peek().is_none() || mine.any(|(_, q)| !q.nearly_out("", Duration::ZERO, now))
}

/// The candidates a tier could run on, of offers: ready when signed in
/// and allowed its model by Settings › Config, with quota as readings
/// say.
pub fn candidates(
    reg: &Registry,
    cfg: &Settings,
    offers: &[Offer],
    readings: &BTreeMap<String, rush_core::usage::Quota>,
) -> Vec<Candidate> {
    let now = rush_core::time::now();
    offers
        .iter()
        .map(|o| {
            let r = &o.pick.r;
            let allowed = o.pick.model.is_empty()
                || r.cell.is_empty()
                || cfg.calls(&r.cell, reg.harness_of(&r.k).as_str(), &o.pick.model);
            Candidate {
                name: o.pick.name.clone(),
                provider: r.prov.clone(),
                place: o.place.clone(),
                ready: r.ready && allowed,
                quota: has_quota(readings, &r.k, &r.prov, now),
            }
        })
        .collect()
}

/// spawn_agent's list of agents, for its description: each model the
/// harness doesn't offer itself, by its plain name, then the tiers.
pub fn catalogue(offers: &[Offer]) -> String {
    let mut lines = Vec::new();
    let mut owned = Vec::new();
    for o in offers {
        if o.own {
            owned.push(o.pick.name.as_str());
            continue;
        }
        lines.push(format!("- {}: {}", o.pick.name, o.description()));
    }
    if !owned.is_empty() {
        lines.push(format!(
            "Your own models ({}) aren't listed: they're your subagent types, for your Agent tool.",
            owned.join(", ")
        ));
    }
    let t = tiers_line(offers);
    if !t.is_empty() {
        lines.push(t);
    }
    lines.join("\n")
}

// The session's agents at work now, by provider, for spreading a tier's
// calls: each tier pick holds a place until its foreground call returns,
// or for one in the background until its first answer is in.
struct Held {
    parent: String,
    provider: String,
    /// Its rush session, once started; "" while it starts.
    child: String,
    token: u64,
}

static HELD: Mutex<Vec<Held>> = Mutex::new(Vec::new());
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn held() -> MutexGuard<'static, Vec<Held>> {
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

/// Picks one of cands for parent's tier call and holds its place, both at
/// once, so calls made in parallel spread across providers. The token
/// lets it go (release) or names its child (started).
pub fn hold_pick(
    parent: &str,
    cands: &[Candidate],
    tier: Tier,
    traits: &[&str],
) -> Option<(usize, u64)> {
    let mut h = held();
    h.retain(|x| x.child.is_empty() || !crate::subtool::answer_path(&x.child).exists());
    let mut busy = BTreeMap::new();
    for x in h.iter().filter(|x| x.parent == parent) {
        *busy.entry(x.provider.clone()).or_insert(0) += 1;
    }
    let i = tier_pick(cands, tier, traits, &busy)?;
    let token = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    h.push(Held {
        parent: parent.into(),
        provider: cands[i].provider.clone(),
        child: String::new(),
        token,
    });
    Some((i, token))
}

/// Says which child a held pick started.
pub fn started(token: u64, child: &str) {
    if let Some(x) = held().iter_mut().find(|x| x.token == token) {
        x.child = child.into();
    }
}

/// Lets a held pick go.
pub fn release(token: u64) {
    held().retain(|x| x.token != token);
}

/// Tells a session that has rush's tools but no way to take every model
/// as its own subagent types how to reach them: as its own, by spawn_agent.
pub const OTHERS_NOTE: &str = "Besides your own subagents, every model rush runs is one of yours: start any of them with rush's spawn_agent tool as you would one of your own subagents, named by its model (\"kimi\", \"astra\", \"opus\") or by a tier (top, workhorse or quick, optionally with long-context, vision or cheap, as \"quick long-context\"). It works in your folder, returns that model's final answer, and with background returns at once and sends the answer as a message when it's done. Start one when the user asks (\"use every agent\" means one of each model listed, each named) or when the work wants another model: a cheaper one for routine work, another provider for a second opinion. An agent the user wants as a session of its own in the list, not your subagent (\"start a new session to…\"), is started with rush's start_agent tool.";

/// Tells a Claude Code session its subagent types are every model.
pub const CLAUDE_NOTE: &str = "Every model rush runs is one of your subagent types: your own (opus, sonnet, haiku…), other providers' (kimi, astra, glm…), and the tiers top, workhorse and quick, where rush picks a model that's signed in with quota left. Start any with the Agent tool as you would any subagent, in the background too; the user asking for one by name (\"have kimi do it\", \"use every agent\" for one of each) means that type. rush's spawn_agent tool is for a model by a version or effort the types don't name. An agent the user wants as a session of its own in the list, not your subagent (\"start a new session to…\"), is started with rush's start_agent tool.";

/// Tells a Codex session its run_agent tool (rush-codex's others.rs) runs
/// every model.
pub const CODEX_NOTE: &str = "Every model rush runs is one of your agent types, started with your run_agent tool as spawn_agent starts your own: other providers' models (opus, kimi, glm…) and your own, by name, and the tiers top, workhorse and quick, where rush picks a model that's signed in with quota left. It works in your folder and returns that agent's final answer; with background it returns at once and the answer comes as a message. The user asking for one by name (\"have kimi do it\", \"use every agent\" for one of each) means that type. Where run_agent isn't offered (a thread resumed from before it was), rush's spawn_agent tool starts them the same way. An agent the user wants as a session of its own in the list, not your subagent (\"start a new session to…\"), is started with rush's start_agent tool.";

/// Session parent's Delegate, as StartOptions holds it.
pub fn delegate(reg: Registry, parent: &str) -> DelegateRef {
    Arc::new(SessionDelegate::new(reg, parent))
}

/// A session's Delegate: runs the call as spawn_agent does, the session's
/// child.
pub struct SessionDelegate {
    reg: Registry,
    parent: String,
    home: std::path::PathBuf,
    cache: std::path::PathBuf,
}

impl SessionDelegate {
    pub fn new(reg: Registry, parent: &str) -> SessionDelegate {
        SessionDelegate {
            reg,
            parent: parent.into(),
            home: rush_state::dir(),
            cache: rush_state::cache_dir(),
        }
    }
}

impl Delegate for SessionDelegate {
    fn run(&self, call: DelegateCall) -> Result<String, String> {
        // Called from the agent's threads, which have no rush folder of
        // their own.
        rush_state::set_thread_dir(Some(self.home.clone()));
        rush_state::set_thread_cache_dir(Some(self.cache.clone()));
        let Some(s) = crate::subtool::SubAgents::of(self.reg.clone(), &self.parent) else {
            return Err("this session can't start agents".into());
        };
        s.spawn(&crate::subtool::SpawnInput {
            agent: call.agent,
            prompt: call.prompt,
            background: call.background,
            step: call.step,
            ..crate::subtool::SpawnInput::default()
        })
    }
}

#[cfg(test)]
mod tests;
