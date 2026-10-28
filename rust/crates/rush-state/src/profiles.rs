//! Profiles: a list of providers (where the model comes from: Claude,
//! Codex, Ollama…) plus what to do when they run out. Every installed
//! provider is a profile of its own, built in and never stored, made from
//! Settings › Config: its harness, and what happens at a limit (AtLimit).
//! Accounts within a provider rotate as they always have. Each session gets
//! one: the one picked for it, else the default. The profiles you could
//! once make are gone (Config::old_profiles).
//!
//! A provider runs in a harness: Ollama in Claude Code, Pi or Codex.
//! Config::runs_in picks it for each provider (harness_for). Anthropic and
//! OpenAI are two providers each (agents::split): "claude" is Anthropic's
//! subscription, "claude-key" its API key, and each has its own profile.
//!
//! Ported from internal/state/profiles.go.

use crate::agents;
use crate::config::{Config, ON_LIMIT_ACCOUNT, ON_LIMIT_AGENT, ON_LIMIT_OFF};
use crate::json::lenient;
use crate::setup::{AT_LIMIT_NEXT, AT_LIMIT_WAIT};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A named list of providers and a policy.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    #[serde(rename = "name", deserialize_with = "lenient")]
    pub name: String,
    /// The providers, in the order new sessions try them. Before
    /// harnesses these were agents' kinds, and still read as such.
    #[serde(
        rename = "providers",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub providers: Vec<String>,
    /// The harness a provider runs in under this profile, by provider,
    /// where it isn't the one Config::runs_in gives it.
    #[serde(
        rename = "runsIn",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub runs_in: BTreeMap<String, String>,
    /// Where new sessions go once every account of the first provider is
    /// nearly out: "" or "stay" waits on it, "mix" moves on.
    #[serde(
        rename = "mix",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub mix: String,
    /// What a running conversation does when a usage limit stops it:
    /// "wait", "" or "account", or "handoff".
    #[serde(
        rename = "onLimit",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub on_limit: String,
    /// How the first provider is paid for: "key" with its API key, "" as
    /// it's signed in.
    #[serde(
        rename = "billing",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub billing: String,
    /// The first provider's account new sessions start on, by ID, while it
    /// has room; empty is whichever has the most.
    #[serde(
        rename = "account",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub account: String,
    /// What its sessions start with, over what Settings gives the provider
    /// in its harness.
    #[serde(
        rename = "model",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub model: String,
    #[serde(
        rename = "effort",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub effort: String,
    /// A provider's own profile, made here rather than stored.
    #[serde(skip)]
    pub builtin: bool,
    /// Config::runs_in, for the providers runs_in leaves out.
    #[serde(skip)]
    config_runs_in: BTreeMap<String, String>,
}

/// What Profile.mix and Profile.on_limit can be.
pub const MIX_STAY: &str = "stay";
pub const MIX_MIX: &str = "mix";
pub const LIMIT_WAIT: &str = "wait";
pub const LIMIT_ACCOUNT: &str = "account";
pub const LIMIT_HANDOFF: &str = "handoff";
pub const BILLING_KEY: &str = "key";

/// What the profile made from an older config is called.
pub const DEFAULT_PROFILE_NAME: &str = "Default"; // migration: built-in profiles replaced it

/// The agent whose accounts are Config::logins, and whose Start is kept in
/// Dispatch's own fields, where older rushes read it.
pub const LOGINS_KIND: &str = agents::LEGACY_KIND;

/// Gives every session started in Path, or a folder inside it, the profile
/// named Profile. Path may start with ~.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderRule {
    #[serde(rename = "path", deserialize_with = "lenient")]
    pub path: String,
    #[serde(rename = "profile", deserialize_with = "lenient")]
    pub profile: String,
}

impl Profile {
    /// Whether new sessions move on to the next provider once the first
    /// is out.
    pub fn mixes(&self) -> bool {
        self.mix == MIX_MIX
    }

    /// What a running conversation does at a usage limit.
    pub fn limit(&self) -> &str {
        if self.on_limit.is_empty() {
            LIMIT_ACCOUNT
        } else {
            &self.on_limit
        }
    }

    /// The harness provider runs in under this profile: its own choice,
    /// else the config's; empty for the provider's default.
    pub fn harness(&self, provider: &str) -> String {
        if let Some(h) = self.runs_in.get(provider).filter(|h| !h.is_empty()) {
            return h.clone();
        }
        self.config_runs_in
            .get(&self.id(provider))
            .cloned()
            .unwrap_or_default()
    }

    /// provider as this profile pays for it: its key one when that's the
    /// first provider and the profile pays with the key.
    pub fn id(&self, provider: &str) -> String {
        if self.billing == BILLING_KEY
            && self.providers.first().is_some_and(|p| p == provider)
            && agents::split(provider)
        {
            return agents::key_of(provider);
        }
        provider.into()
    }

    /// The agent that runs provider under this profile.
    pub fn kind_of(&self, provider: &str) -> String {
        match agents::kind_for(&self.id(provider), &self.harness(provider)) {
            Some(k) => k,
            None => provider.into(), // no adapter for it: stays as named, and isn't installed
        }
    }

    /// The agents that run the profile's providers, in order.
    pub fn kinds(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for pr in &self.providers {
            let k = self.kind_of(pr);
            if !out.contains(&k) {
                out.push(k);
            }
        }
        out
    }

    /// Whether the profile runs agent kind, or lists it as a provider.
    pub fn has(&self, kind: &str) -> bool {
        self.providers.iter().any(|p| p == kind) || self.kinds().iter().any(|k| k == kind)
    }

    /// The agents of the profile's providers rush can run sessions of
    /// here, in order.
    pub fn installed(&self) -> Vec<String> {
        self.kinds()
            .into_iter()
            .filter(|k| agents::runs(k))
            .collect()
    }

    /// Whether p does no more than its first provider's own profile: it
    /// never moves on to another, and switches account at a limit.
    fn plain(&self) -> bool {
        !self.mixes() && self.limit() == LIMIT_ACCOUNT && self.runs_in.is_empty()
    }

    /// The provider and account a new session starts on, given what room
    /// each has: the first installed provider while any of its accounts
    /// has room; then, when the profile mixes, the next that has. None when
    /// none of its providers is installed.
    pub fn pick(&self, room: &Room) -> Option<Pick> {
        let inst = self.installed();
        let first_kind = inst.first()?;
        let (mut first, mut ok) = seat(room, first_kind);
        if !self.account.is_empty()
            && let Some(s) = room
                .get(first_kind)
                .and_then(|seats| seats.iter().find(|s| s.id == self.account && !s.out))
        {
            first = s.clone();
            ok = true;
        }
        if ok {
            return Some(Pick {
                kind: first_kind.clone(),
                account: first,
                wait: false,
            });
        }
        if self.mixes()
            && let Some(next) = self.next(first_kind, room)
        {
            return Some(next);
        }
        Some(Pick {
            kind: first_kind.clone(),
            account: first,
            wait: true,
        })
    }

    /// The first installed provider after kind with room, for a
    /// conversation handed on from it. Providers before kind aren't tried.
    pub fn next(&self, kind: &str, room: &Room) -> Option<Pick> {
        let inst = self.installed();
        let at = inst.iter().position(|k| k == kind);
        let from = at.map(|i| i + 1).unwrap_or(0);
        for k in &inst[from..] {
            let (s, ok) = seat(room, k);
            if ok {
                return Some(Pick {
                    kind: k.clone(),
                    account: s,
                    wait: false,
                });
            }
        }
        None
    }

    /// The account of provider kind to use under this profile, for work
    /// that has to run on that provider. The bool is false when the profile
    /// doesn't list it, it isn't installed, or every account of it is
    /// nearly out (then the pick waits).
    pub fn pick_for(&self, kind: &str, room: &Room) -> (Pick, bool) {
        if !self.has(kind) || !agents::runs(kind) {
            return (Pick::default(), false);
        }
        let (s, ok) = seat(room, kind);
        (
            Pick {
                kind: kind.into(),
                account: s,
                wait: !ok,
            },
            ok,
        )
    }
}

/// One account of a provider as the picker sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seat {
    pub id: String,
    pub name: String,
    /// The one the provider is signed in as.
    pub current: bool,
    /// A fresh reading says it's nearly out.
    pub out: bool,
    /// Its tightest window, in percent.
    pub used: f64,
}

/// What rush knows of each provider's accounts, by kind. A provider with
/// none listed is taken to have room.
pub type Room = BTreeMap<String, Vec<Seat>>;

/// Where a session starts: a provider, and the account of it with room.
/// Wait is set when every account of every provider the profile allows is
/// nearly out: the session starts there anyway and waits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pick {
    pub kind: String,
    pub account: Seat,
    pub wait: bool,
}

/// The account of a provider to start on: the one in use while it has
/// room, else the one with the most room. false when all are out.
fn seat(room: &Room, kind: &str) -> (Seat, bool) {
    let seats = match room.get(kind) {
        Some(s) if !s.is_empty() => s,
        _ => return (Seat::default(), true),
    };
    let mut best: Option<&Seat> = None;
    for s in seats {
        if s.out {
            continue;
        }
        if s.current {
            return (s.clone(), true);
        }
        if best.is_none_or(|b| s.used < b.used) {
            best = Some(s);
        }
    }
    match best {
        Some(b) => (b.clone(), true),
        None => match seats.iter().find(|s| s.current) {
            Some(s) => (s.clone(), false),
            None => (seats[0].clone(), false),
        },
    }
}

/// The installed providers, each a split one's key after it.
pub fn ids() -> Vec<String> {
    let mut out = Vec::new();
    for pr in agents::providers() {
        if !agents::provider_installed(&pr) {
            continue;
        }
        let s = agents::split(&pr);
        out.push(pr.clone());
        if s {
            out.push(agents::key_of(&pr));
        }
    }
    out
}

impl Config {
    /// The profile called name, ignoring case: a provider's own ("ollama"),
    /// else an agent's, which is its provider in that harness ("ollama-pi"
    /// is Ollama in Pi).
    pub fn profile_named(&self, name: &str) -> Option<Profile> {
        if name.is_empty() {
            return None;
        }
        let k = name.to_lowercase();
        if agents::is_provider(&k) {
            return Some(self.builtin(&k));
        }
        if agents::billed(&k).1 {
            return Some(self.builtin(&k));
        }
        if agents::get(&k).is_some() {
            let mut p = self.builtin(&agents::provider_of(&k));
            p.name = k.clone();
            p.runs_in = BTreeMap::from([(p.providers[0].clone(), agents::harness_of(&k))]);
            if agents::key_only(&k) {
                p.billing = BILLING_KEY.into();
            }
            return Some(p);
        }
        None
    }

    /// Provider id's own profile: it, in the harness the config gives it,
    /// doing at a limit what AtLimit says; moving on, the other providers
    /// set up here come after it.
    pub(crate) fn builtin(&self, id: &str) -> Profile {
        let mut p = Profile {
            name: id.into(),
            providers: vec![id.into()],
            builtin: true,
            config_runs_in: self.harnesses_now(),
            ..Profile::default()
        };
        let (pr, key) = agents::billed(id);
        if key {
            p.providers = vec![pr];
            p.billing = BILLING_KEY.into();
        }
        match self.at_limit.as_str() {
            AT_LIMIT_WAIT => p.on_limit = LIMIT_WAIT.into(),
            AT_LIMIT_NEXT => {
                p.mix = MIX_MIX.into();
                p.on_limit = LIMIT_HANDOFF.into();
                for other in ids() {
                    let (pr, key) = agents::billed(&other);
                    if !key && other != id && !p.providers.contains(&pr) && self.ready(&other) {
                        p.providers.push(pr);
                    }
                }
            }
            _ => {}
        }
        p
    }

    /// The installed providers' own profiles, by name.
    pub fn builtins(&self) -> Vec<Profile> {
        ids().iter().map(|id| self.builtin(id)).collect()
    }

    /// The profile a session gets when nothing else says: the default
    /// provider's own, else the default agent's provider's.
    pub fn default_profile(&self) -> Profile {
        if let Some(p) = self.profile_named(&self.default_profile) {
            return p;
        }
        if let Some(p) = self.profile_named(&agents::provider_of(&self.default_agent())) {
            return p;
        }
        self.builtin(LOGINS_KIND)
    }

    /// The profile a session in cwd gets: explicit when it names one, else
    /// the default. Folders no longer pick one.
    pub fn profile_for(&self, _cwd: &str, explicit: &str) -> Profile {
        if !explicit.is_empty()
            && let Some(p) = self.profile_named(explicit)
        {
            return p;
        }
        self.default_profile()
    }

    /// The profile an older config meant: its default agent, then its
    /// order, with its choice of what to do when nearly out.
    fn legacy_profile(&self) -> Profile {
        let mut p = Profile {
            name: DEFAULT_PROFILE_NAME.into(),
            providers: vec![self.default_agent()],
            ..Profile::default()
        };
        for k in &self.agent_order {
            if !p.providers.contains(k) {
                p.providers.push(k.clone());
            }
        }
        match self.switch_on_limit.as_str() {
            ON_LIMIT_AGENT => {
                p.mix = MIX_MIX.into();
                // An older rush moved on through every installed agent,
                // those not in its order after, by name.
                for k in agents::providers() {
                    if !p.providers.contains(&k) {
                        p.providers.push(k);
                    }
                }
            }
            ON_LIMIT_OFF => p.on_limit = LIMIT_WAIT.into(),
            _ => {}
        }
        p
    }

    /// Keeps what an older config meant, once. A config from before
    /// profiles, or with the Default profile rush made from one, needs no
    /// profile of its own when that did only what its first provider's
    /// built-in one does. Otherwise the Default profile stays (or is made)
    /// for what it does.
    pub(crate) fn migrate_profiles(&mut self) {
        // migration: built-in profiles replaced Default
        if self.builtin_profiles {
            return;
        }
        self.builtin_profiles = true;
        if self.old_profiles.is_empty() {
            let p = self.legacy_profile();
            if !p.plain() {
                self.default_profile = p.name.clone();
                self.old_profiles = vec![p];
                return;
            }
            self.default_profile = agents::provider_of(&self.default_agent());
            return;
        }
        let only = &self.old_profiles[0];
        if self.old_profiles.len() != 1
            || only.name != DEFAULT_PROFILE_NAME
            || !only.plain()
            || only.providers.is_empty()
        {
            return;
        }
        let to = agents::provider_of(&only.providers[0]);
        for r in &mut self.old_folder_rules {
            if r.profile.eq_ignore_ascii_case(DEFAULT_PROFILE_NAME) {
                r.profile = to.clone();
            }
        }
        self.old_profiles.clear();
        self.default_profile = to;
    }

    /// Writes the default profile back into the fields older rushes read:
    /// the default agent, the order, and what happens when nearly out.
    /// Call it after changing profiles.
    pub fn sync_legacy(&mut self) {
        let p = self.default_profile();
        let ks = p.kinds();
        if !ks.is_empty() {
            self.dispatch.kind = ks[0].clone();
            self.agent_order = ks;
        }
        if p.limit() == LIMIT_WAIT {
            self.set_switch_on_limit(ON_LIMIT_OFF);
        } else if p.mixes() {
            self.set_switch_on_limit(ON_LIMIT_AGENT);
        } else {
            self.set_switch_on_limit(ON_LIMIT_ACCOUNT);
        }
    }

    /// Makes the profile called name the default.
    pub fn set_default_profile(&mut self, name: &str) -> bool {
        match self.profile_named(name) {
            Some(p) => {
                self.default_profile = p.name;
                self.sync_legacy();
                true
            }
            None => false,
        }
    }

    /// Makes agent kind's own profile the default, so new sessions run it:
    /// "ollama-pi" is Ollama's, in Pi.
    pub fn set_default_provider(&mut self, kind: &str) {
        self.set_default_profile(kind);
    }

    /// Gives provider id the harness it runs in wherever a profile doesn't
    /// pick one; empty is its default.
    pub fn set_runs_in(&mut self, id: &str, harness: &str) {
        let mut harness = harness.to_string();
        if let Some(def) = agents::runs_for(id).first()
            && agents::harness_of(def) == harness
        {
            harness.clear();
        }
        if harness.is_empty() {
            self.runs_in.remove(id);
        } else {
            self.runs_in.insert(id.into(), harness);
        }
        self.sync_legacy();
    }

    /// Whether you use provider id in harness, besides its default.
    pub fn uses(&self, id: &str, harness: &str) -> bool {
        self.harnesses
            .get(id)
            .is_some_and(|hs| hs.iter().any(|h| h == harness))
    }

    /// Says whether you use provider id in harness.
    pub fn set_uses(&mut self, id: &str, harness: &str, on: bool) {
        let mut hs: Vec<String> = self
            .harnesses
            .get(id)
            .map(|hs| hs.iter().filter(|h| *h != harness).cloned().collect())
            .unwrap_or_default();
        if on {
            hs.push(harness.into());
        }
        if hs.is_empty() {
            self.harnesses.remove(id);
            return;
        }
        self.harnesses.insert(id.into(), hs);
    }

    /// Keeps a split provider set to run in a harness only its API key can
    /// (Anthropic in Pi) doing so: that's its key's harness now, and the
    /// key what the default and folders that named it get.
    pub(crate) fn migrate_key_harness(&mut self) {
        // migration: providers split by billing
        let set: Vec<(String, String)> = self
            .runs_in
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (pr, h) in set {
            if !agents::split(&pr) || h == pr {
                continue;
            }
            let key = agents::key_of(&pr);
            self.runs_in.insert(key.clone(), h);
            self.runs_in.remove(&pr);
            if self.default_profile.to_lowercase() == pr.to_lowercase() {
                self.default_profile = key.clone();
            }
            for r in &mut self.old_folder_rules {
                if r.profile.to_lowercase() == pr.to_lowercase() {
                    r.profile = key.clone();
                }
            }
        }
    }

    /// Moves the default profile you made into what replaced profiles,
    /// once: its first provider the default, its harness that provider's ★,
    /// its model that route's, its effort its harness's, and what it did
    /// at a limit AtLimit. OldProfiles and OldFolderRules stay, unread, for
    /// older rushes.
    pub(crate) fn move_profiles_in(&mut self) {
        // migration: profiles replaced by Settings › Config
        if self.profiles_moved_in {
            return;
        }
        self.profiles_moved_in = true;
        let want = self.default_profile.to_lowercase();
        let Some(i) = self
            .old_profiles
            .iter()
            .position(|p| p.name.to_lowercase() == want)
        else {
            if self.profile_named(&self.default_profile).is_none() {
                self.default_profile.clear();
            }
            return;
        };
        let mut p = self.old_profiles[i].clone();
        if p.providers.is_empty() {
            self.default_profile.clear();
            return;
        }
        let first = p.providers[0].clone();
        let id = p.id(&first);
        self.default_profile = id.clone();
        if let Some(h) = p.runs_in.get(&first).filter(|h| !h.is_empty()) {
            self.runs_in.insert(id.clone(), h.clone());
        }
        p.config_runs_in = self.runs_in.clone();
        let k = p.kind_of(&first);
        if !p.model.is_empty() {
            let mut st = self.dispatch.start_on(&id, &k);
            st.model = p.model.clone();
            self.dispatch.set_start_on(&id, &k, st);
        }
        if !p.effort.is_empty() {
            let h = agents::harness_of(&k);
            let mut hs = self.dispatch.harness_start(&h);
            if hs.effort.is_empty() {
                hs.effort = p.effort.clone();
                self.dispatch.set_harness_start(&h, hs);
            }
        }
        if p.limit() == LIMIT_WAIT {
            self.at_limit = AT_LIMIT_WAIT.into();
        } else if p.mixes() || p.limit() == LIMIT_HANDOFF {
            self.at_limit = AT_LIMIT_NEXT.into();
        }
        self.sync_legacy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::fake::register_fakes;

    fn room(kind: &str, seats: &[(&str, bool, bool, f64)]) -> Room {
        let seats = seats
            .iter()
            .map(|(id, current, out, used)| Seat {
                id: id.to_string(),
                current: *current,
                out: *out,
                used: *used,
                ..Seat::default()
            })
            .collect();
        Room::from([(kind.to_string(), seats)])
    }

    // A session's profile is the one picked for it, else the default;
    // folders no longer pick one.
    #[test]
    fn profile_for() {
        register_fakes();
        let c = Config {
            default_profile: "pa".into(),
            ..Config::default()
        };
        for (cwd, explicit, want) in [
            ("/elsewhere", "", "pa"),
            ("/src", "PB", "pb"),
            ("/src", "nope", "pa"),
        ] {
            assert_eq!(c.profile_for(cwd, explicit).name, want, "{cwd} {explicit}");
        }
    }

    // The picker starts on the first installed provider while an account
    // of it has room, and moves on only when the profile mixes.
    #[test]
    fn pick() {
        register_fakes();
        let out = room("pa", &[("a1", true, true, 99.0), ("a2", false, true, 97.0)]);
        let stay = Profile {
            name: "s".into(),
            providers: vec!["gone".into(), "pa".into(), "pb".into()],
            ..Profile::default()
        };
        let p = stay
            .pick(&room(
                "pa",
                &[("a1", true, false, 50.0), ("a2", false, false, 10.0)],
            ))
            .unwrap();
        assert!(
            p.kind == "pa" && p.account.id == "a1" && !p.wait,
            "with room: {p:?}"
        );
        let p = stay
            .pick(&room(
                "pa",
                &[
                    ("a1", true, true, 0.0),
                    ("a2", false, false, 40.0),
                    ("a3", false, false, 20.0),
                ],
            ))
            .unwrap();
        assert!(
            p.kind == "pa" && p.account.id == "a3",
            "the roomiest: {p:?}"
        );
        let p = stay.pick(&out).unwrap();
        assert!(
            p.kind == "pa" && p.wait && p.account.id == "a1",
            "staying, all out: {p:?}"
        );
        let mut mix = stay.clone();
        mix.mix = MIX_MIX.into();
        let p = mix.pick(&out).unwrap();
        assert!(p.kind == "pb" && !p.wait, "mixing, all out: {p:?}");
        let gone = Profile {
            providers: vec!["gone".into()],
            ..Profile::default()
        };
        assert!(
            gone.pick(&Room::new()).is_none(),
            "picked one not installed"
        );
        assert_eq!(mix.next("pa", &out).unwrap().kind, "pb");
        assert!(
            mix.next("pb", &out).is_none(),
            "found a provider after the last"
        );
    }

    // PickFor gives an account of one provider, or says there's none.
    #[test]
    fn pick_for() {
        register_fakes();
        let p = Profile {
            providers: vec!["pa".into(), "claude".into(), "gone".into()],
            ..Profile::default()
        };
        let (got, ok) = p.pick_for("claude", &Room::new());
        assert!(ok && got.kind == "claude");
        assert!(
            !p.pick_for("pb", &Room::new()).1,
            "one the profile doesn't list"
        );
        assert!(
            !p.pick_for("gone", &Room::new()).1,
            "one that isn't installed"
        );
        let (got, ok) = p.pick_for("pa", &room("pa", &[("x", true, true, 0.0)]));
        assert!(!ok && got.wait, "every account out");
    }

    // A provider runs in the harness the config gives it, else its own.
    #[test]
    fn runs_in() {
        register_fakes();
        let mut c = Config::default();
        assert_eq!(c.profile_named("pa").unwrap().kinds(), ["pa"]);
        c.set_runs_in("pa", "pb");
        let p = c.profile_named("pa").unwrap();
        assert!(
            p.builtin && p.kinds() == ["pa-pb"] && p.has("pa-pb") && p.has("pa"),
            "{p:?}"
        );
        let p = c.default_profile().pick(&Room::new()).unwrap();
        assert_eq!(p.kind, "claude");
        c.set_runs_in("pa", "pa"); // its own harness: the default again
        assert!(
            !c.runs_in.contains_key("pa"),
            "kept its default: {:?}",
            c.runs_in
        );
        // A rider's kind names its provider's profile in that harness.
        assert_eq!(c.profile_named("pa-pb").unwrap().kinds(), ["pa-pb"]);
    }

    // A split provider is two; an older config that ran it in another
    // harness now runs its key there.
    #[test]
    fn split_by_billing() {
        register_fakes();
        let mut c = Config {
            default_profile: "ps".into(),
            runs_in: BTreeMap::from([("ps".into(), "pb".into())]),
            old_folder_rules: vec![FolderRule {
                path: "/w".into(),
                profile: "ps".into(),
            }],
            ..Config::default()
        };
        c.migrate_key_harness();
        assert_eq!(c.default_profile, "ps-key");
        assert_eq!(c.runs_in.get("ps-key").map(String::as_str), Some("pb"));
        assert!(!c.runs_in.contains_key("ps"));
        assert_eq!(c.old_folder_rules[0].profile, "ps-key");
        let key = c.profile_named("ps-key").unwrap();
        assert!(
            key.billing == BILLING_KEY && key.kinds() == ["ps-pb"],
            "{key:?}"
        );
        assert_eq!(c.profile_named("ps").unwrap().kinds(), ["ps"]);
        c.set_uses("ps-key", "pb", true);
        c.set_uses("ps-key", "ps", true);
        c.set_uses("ps-key", "ps", false);
        assert!(c.uses("ps-key", "pb") && !c.uses("ps-key", "ps") && !c.uses("ps", "pb"));
    }

    // Every installed provider is a profile.
    #[test]
    fn builtins() {
        register_fakes();
        let c = Config::default();
        let names: Vec<String> = c.builtins().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ids());
        assert_eq!(c.default_profile().name, "claude");
    }

    // A provider is a profile of its own, so one session can run on it
    // beside the default's.
    #[test]
    fn provider_is_a_profile() {
        register_fakes(); // stubprovider among them
        let c = Config::default();
        let p = c.profile_named("StubProvider").unwrap();
        assert!(
            p.name == "stubprovider" && p.providers == ["stubprovider"],
            "{p:?}"
        );
        assert_eq!(
            c.profile_for("/tmp", "stubprovider").providers[0],
            "stubprovider"
        );
        assert!(c.profile_named("nobody").is_none());
    }
}
