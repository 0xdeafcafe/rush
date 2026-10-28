//! The adapters rush knows, and what it asks of them by kind, never by an
//! agent's name. After Go's internal/agent: adapter.go (Register, Get,
//! All), installed.go (what's installed, looked for at most every three
//! minutes), harness.go and route.go (providers, harnesses and routes),
//! kinds.go, feature.go (Supports), media.go (Unreadable), pricing.go
//! (Price), window.go (ContextWindow), branch.go, choices.go, actions.go
//! (ProjectFolders), attribution.go (ApplyAttribution), agent.go
//! (ReadPreview), subagents.go (FollowRuns) and native.go.
//!
//! Go keeps one registry in package state; here a [`Registry`] is a value
//! (cheap to clone, shared inside), and [`global`] is rush's own, which
//! adapters register with as rush starts. Tests make their own.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

use super::installed::{Env, find_in};
use super::*;
use crate::agent::{Preview, TokenUsage};
use crate::tool;

/// A registered adapter.
pub type AdapterRef = Arc<dyn Adapter>;

/// How long a look for an agent's program holds: installing one while rush
/// runs shows it within this, or at once after recheck.
pub const INSTALLED_FOR: Duration = Duration::from_secs(3 * 60);

/// The adapters rush knows.
#[derive(Clone, Default)]
pub struct Registry(Arc<Inner>);

#[derive(Default)]
struct Inner {
    reg: RwLock<Reg>,
    found: Mutex<Found>,
    /// Told each time a look in the background lands.
    landed: Condvar,
    /// The registered agents by program name, made again when one is
    /// registered: program_kind is asked of every shell step a session ran.
    programs: Mutex<Option<Arc<HashMap<String, Kind>>>>,
    env: Option<Env>,
}

#[derive(Default)]
struct Reg {
    by_kind: HashMap<Kind, AdapterRef>,
    /// Adapters by kind, made again on register.
    sorted: Arc<[AdapterRef]>,
    native: Option<AdapterRef>,
}

#[derive(Default)]
struct Found {
    at: Option<Instant>,
    /// "" when it isn't installed, "~…" when only its Lesser program is,
    /// "-" when it has nothing to look for.
    paths: Option<Arc<HashMap<Kind, String>>>,
    /// Each agent's profiles, as last read (only when never waiting).
    homes: HashMap<Kind, Vec<Profile>>,
    no_wait: bool,
    looking: bool,
}

impl Borrow<str> for Kind {
    fn borrow(&self) -> &str {
        &self.0
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// rush's own registry, which adapters register with as rush starts.
pub fn global() -> &'static Registry {
    static G: OnceLock<Registry> = OnceLock::new();
    G.get_or_init(Registry::new)
}

/// The provider that is p paid for with its API key.
pub fn key_of(p: &str) -> String {
    format!("{p}{KEY_SUFFIX}")
}

const KEY_SUFFIX: &str = "-key";

/// A name as one word to type: "Claude Code" is claude-code.
fn word(s: &str) -> String {
    let s: String = s
        .chars()
        .filter(|c| !matches!(c, '(' | ')' | '.'))
        .collect();
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// A registry that looks for programs on this PATH, with relative
    /// install places under home, rather than the process's own: for tests.
    pub fn with_env(path: &str, home: &str) -> Registry {
        Registry(Arc::new(Inner {
            env: Some(Env {
                path: path.to_string(),
                home: home.to_string(),
            }),
            ..Inner::default()
        }))
    }

    fn env(&self) -> Env {
        self.0.env.clone().unwrap_or_else(Env::from_process)
    }

    /// Makes an adapter known. Adapters register as rush starts.
    pub fn register(&self, a: impl Adapter + 'static) {
        self.register_ref(Arc::new(a));
    }

    pub fn register_ref(&self, a: AdapterRef) {
        let mut r = self.0.reg.write().unwrap_or_else(|e| e.into_inner());
        if a.native().is_some() {
            r.native = Some(a.clone());
        }
        r.by_kind.insert(a.kind(), a);
        let mut sorted: Vec<AdapterRef> = r.by_kind.values().cloned().collect();
        sorted.sort_by_key(|a| a.kind());
        r.sorted = sorted.into();
        drop(r);
        *lock(&self.0.programs) = None;
    }

    /// Forgets the adapter for kind k.
    pub fn unregister(&self, k: &str) {
        let mut r = self.0.reg.write().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = r.by_kind.remove(k)
            && r.native.as_ref().is_some_and(|n| Arc::ptr_eq(n, &a))
        {
            r.native = None;
        }
        let sorted: Vec<AdapterRef> = r
            .sorted
            .iter()
            .filter(|a| a.kind().0 != k)
            .cloned()
            .collect();
        r.sorted = sorted.into();
        drop(r);
        *lock(&self.0.programs) = None;
    }

    /// The adapter for kind k, if one is registered.
    pub fn get(&self, k: &str) -> Option<AdapterRef> {
        let r = self.0.reg.read().unwrap_or_else(|e| e.into_inner());
        r.by_kind.get(k).cloned()
    }

    /// Every registered adapter, by kind. It's asked for every process on
    /// every load, so it's kept sorted rather than sorted each time.
    pub fn all(&self) -> Arc<[AdapterRef]> {
        let r = self.0.reg.read().unwrap_or_else(|e| e.into_inner());
        r.sorted.clone()
    }

    /// The registered Native adapter, if one is.
    pub fn native_agent(&self) -> Option<AdapterRef> {
        let r = self.0.reg.read().unwrap_or_else(|e| e.into_inner());
        r.native.clone()
    }

    // ---- Features (Go's feature.go) ----

    /// What agent k declares it can do; None when it isn't registered.
    pub fn features(&self, k: &str) -> Option<Features> {
        self.get(k).map(|a| a.features())
    }

    /// How agent k supports f. A feature it doesn't list, or an agent that
    /// isn't registered, is No.
    pub fn feature_of(&self, k: &str, f: Feature) -> Support {
        self.features(k)
            .and_then(|mut fs| fs.remove(&f))
            .unwrap_or_default()
    }

    /// Whether agent k can do f.
    pub fn supports(&self, k: &str, f: Feature) -> bool {
        self.feature_of(k, f).is == Has::Yes
    }

    /// How far agent k has been tried; preview when it isn't registered.
    pub fn level_of(&self, k: &str) -> Level {
        self.get(k).map_or(Level::Preview, |a| a.level())
    }

    // ---- What rush asks by kind ----

    /// Agent k's Preview of the transcript at path; empty when k can't read
    /// one. It reads the disk, so never on the UI.
    pub fn read_preview(&self, k: &str, path: &str, window: i64) -> Preview {
        self.get(k)
            .and_then(|a| a.previewer().map(|p| p.preview(path, window)))
            .unwrap_or_default()
    }

    /// What agent k says a new session can start with.
    pub fn choices_of(&self, k: &str) -> Option<Choices> {
        self.get(k).and_then(|a| a.chooser().map(|c| c.choices()))
    }

    /// Where agent k keeps session sid's conversation in cwd, in profile p;
    /// empty when k can't say.
    pub fn transcript_path(&self, k: &str, p: &Profile, cwd: &str, sid: &str) -> String {
        self.get(k)
            .and_then(|a| {
                a.transcript_locator()
                    .map(|t| t.transcript_path(p, cwd, sid))
            })
            .unwrap_or_default()
    }

    /// Where session sid, last in cwd, keeps its conversation now: looked
    /// for, as a session that moved to a worktree keeps it where it
    /// started. transcript_path when k doesn't look. It reads the disk.
    pub fn find_transcript(&self, k: &str, p: &Profile, cwd: &str, sid: &str) -> String {
        if let Some(a) = self.get(k)
            && let Some(t) = a.transcripts()
        {
            return t.find_transcript(p, cwd, sid);
        }
        self.transcript_path(k, p, cwd, sid)
    }

    /// id as agent k says it, or id as it is.
    pub fn model_name(&self, k: &str, id: &str) -> String {
        if !id.is_empty()
            && let Some(a) = self.get(k)
            && let Some(n) = a.model_namer()
        {
            return n.model_name(id);
        }
        id.to_string()
    }

    /// A call in a few words, as agent k says it where it has words of its
    /// own for its tools; None when it hasn't, for the caller to word it as
    /// tool::doing does.
    pub fn describe(&self, k: &str, c: &tool::Call) -> Option<String> {
        self.get(k).and_then(|a| a.describer().map(|d| d.doing(c)))
    }

    /// A call in a few words, as agent k says it: its own words where it
    /// has them (a Describer), else tool::doing's (Go's agent.Doing).
    pub fn doing(&self, k: &str, c: &tool::Call) -> String {
        self.describe(k, c).unwrap_or_else(|| tool::doing(c))
    }

    /// The folders the agents rush knows keep in a project, each once.
    pub fn project_folders(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in self.all().iter() {
            if let Some(f) = a.project_folderer() {
                let f = f.project_folder();
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        out
    }

    /// Sets on in every profile of every agent that can say, where it
    /// isn't already: what changed, and what couldn't be.
    pub fn apply_attribution(&self, on: bool) -> Vec<AttributionChange> {
        let mut out = Vec::new();
        for a in self.all().iter() {
            let Some(at) = a.attributor() else { continue };
            for p in self.profiles_of(a) {
                if let (now, true) = at.attribution(&p)
                    && now == on
                {
                    continue;
                }
                out.push(AttributionChange {
                    kind: a.kind(),
                    profile: if p.name.is_empty() {
                        p.dir.clone()
                    } else {
                        p.name.clone()
                    },
                    err: at.set_attribution(&p, on).err(),
                });
            }
        }
        out
    }

    /// Whether agent k's transcripts are Claude Code's.
    pub fn reads_as_claude(&self, k: &str) -> bool {
        self.get(k).is_some_and(|a| a.claude_transcripts())
    }

    /// Whether agent k's models stay warm in the cloud.
    pub fn stays_warm(&self, k: &str) -> bool {
        self.get(k).is_some_and(|a| a.warm())
    }

    /// A new follower of a session of agent k's subagent runs; None when k
    /// can't follow them.
    pub fn follow_runs(&self, k: &str) -> Option<Box<dyn SubagentRuns>> {
        self.get(k)
            .and_then(|a| a.run_follower().and_then(|f| f.subagent_runs()))
    }

    /// The kind agent k's sessions start as now: a legacy harness's
    /// replacement, else k.
    pub fn current_kind(&self, k: &str) -> Kind {
        if let Some(a) = self.get(k)
            && let Some(r) = a.replaced()
        {
            return r.replacement();
        }
        Kind::from(k)
    }

    /// What agent k's program is called; empty when it has none, isn't
    /// registered, or rides another's.
    pub fn program_of(&self, k: &str) -> String {
        let Some(a) = self.get(k) else {
            return String::new();
        };
        if a.rider().is_some() {
            return String::new();
        }
        a.programmer().map(|p| p.program().0).unwrap_or_default()
    }

    /// Whether a process called comm (a name, or a path) is one of the
    /// registered agents' programs.
    pub fn is_program(&self, comm: &str) -> bool {
        let comm = base(comm);
        if comm.is_empty() || comm == "." || comm == "/" {
            return false;
        }
        self.all()
            .iter()
            .any(|a| self.program_of(a.kind().as_str()) == comm)
    }

    /// Why agent k's model can't be sent files: "Codex can't take images",
    /// "gemma3:4b can't read images". An empty model is the agent's
    /// default. It's "" when every file can go, and when the agent doesn't
    /// know the model: a gap in an adapter's table never blocks a send; the
    /// agent says no itself if it must.
    pub fn unreadable(&self, k: &str, model: &str, files: &[String]) -> String {
        let mut need = Media::default();
        for f in files {
            need |= media_of(f);
        }
        if need.is_empty() {
            return String::new();
        }
        if need.has(Media::IMAGE) && !self.supports(k, Feature::Images) {
            let name = self.get(k).map_or_else(|| k.to_string(), |a| a.name());
            return format!("{name} can't take images");
        }
        let Some(a) = self.get(k) else {
            return String::new();
        };
        let Some(r) = a.media_reader() else {
            return String::new();
        };
        let mut model = model.to_string();
        if model.is_empty()
            && let Some(d) = a.default_modeler()
        {
            model = d.default_model();
        }
        match r.reads(&model) {
            Some(has) if !need.without(has).is_empty() => format!(
                "{} can't read {}",
                self.model_name(k, &model),
                need.without(has)
            ),
            _ => String::new(),
        }
    }

    /// What agent k's tokens u cost on model, at its default model's prices
    /// when model has none; None when k can't say.
    pub fn price(&self, k: &str, model: &str, u: &TokenUsage) -> Option<f64> {
        let a = self.get(k)?;
        let pr = a.pricer()?;
        if let Some(c) = pr.cost(model, u) {
            return Some(c);
        }
        pr.cost(&a.default_modeler()?.default_model(), u)
    }

    /// model's context window as agent k knows it, else DEFAULT_WINDOW. An
    /// empty k is the one an older record meant.
    pub fn context_window(&self, k: &str, model: &str) -> i64 {
        let k = if k.is_empty() { LEGACY_KIND } else { k };
        self.get(k)
            .and_then(|a| a.context_windower().map(|w| w.context_window(model)))
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_WINDOW)
    }

    /// Compaction as agent k tells it; none for an agent that doesn't.
    pub fn compaction_of(&self, k: &str, p: &Profile, cwd: &str, env: &[String]) -> Compaction {
        let k = if k.is_empty() { LEGACY_KIND } else { k };
        self.get(k)
            .and_then(|a| a.compacter().map(|c| c.compaction(p, cwd, env)))
            .unwrap_or_default()
    }

    /// The first message of a new session that carries c on (handoff),
    /// naming the agent it ran on as rush does.
    pub fn handoff(&self, c: &Conversation) -> Input {
        handoff(c, &self.name_of(c.from.as_str()))
    }

    /// What a session started with c as its own history is told beside it.
    pub fn carried(&self, c: &Conversation) -> String {
        carried(&self.name_of(c.from.as_str()))
    }

    fn name_of(&self, k: &str) -> String {
        self.get(k).map_or_else(|| k.to_string(), |a| a.name())
    }

    // ---- Providers and harnesses (Go's harness.go) ----
    //
    // A provider is where a session's model comes from (Anthropic, OpenAI,
    // Ollama on this machine); a harness is the program that runs the
    // session around it (Claude Code, Codex, Pi). Most agents are both at
    // once: Codex runs OpenAI's models. A Rider is a provider running in
    // another agent's harness, and one provider can ride several: Ollama in
    // Claude Code, in Pi and in Codex are each an adapter of their own, all
    // of provider "ollama".

    /// Provider p by the company behind its models, else empty: the caller
    /// names it as the agent.
    pub fn provider_name(&self, p: &str) -> String {
        self.get(p)
            .and_then(|a| a.made().map(|m| m.maker()))
            .unwrap_or_default()
    }

    /// The provider agent k runs: a Rider's provider, else k.
    pub fn provider_of(&self, k: &str) -> String {
        self.get(k)
            .and_then(|a| a.provided().map(|p| p.provider()))
            .unwrap_or_else(|| k.to_string())
    }

    /// The program agent k runs in: the agent a Rider rides, else k itself.
    pub fn harness_of(&self, k: &str) -> Kind {
        self.get(k)
            .and_then(|a| a.rider().map(|r| r.rides()))
            .unwrap_or_else(|| Kind::from(k))
    }

    fn is_current(&self, k: &Kind) -> bool {
        self.current_kind(k.as_str()) == *k
    }

    /// Every provider an adapter runs, by name, each once.
    pub fn providers(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in self.all().iter() {
            let k = a.kind();
            if !self.is_current(&k) {
                continue;
            }
            let p = self.provider_of(k.as_str());
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out.sort();
        out
    }

    /// Whether p is one of providers, without building them: a profile's
    /// name is looked up this way every frame.
    pub fn is_provider(&self, p: &str) -> bool {
        self.all().iter().any(|a| {
            let k = a.kind();
            self.is_current(&k) && self.provider_of(k.as_str()) == p
        })
    }

    /// The agents that run provider p, whether installed or not: the one of
    /// p's own kind first (its default), then the rest by kind.
    pub fn harnesses(&self, p: &str) -> Vec<Kind> {
        let mut out: Vec<Kind> = self
            .all()
            .iter()
            .map(|a| a.kind())
            .filter(|k| self.provider_of(k.as_str()) == p && self.is_current(k))
            .collect();
        out.sort_by_key(|k| k.as_str() != p);
        out
    }

    /// The agent that runs provider id p (as billed reads it) in harness h:
    /// p's own kind when h is empty or p's own harness. When that one isn't
    /// installed, the first of p's that is. None when p has no adapter at
    /// all.
    pub fn kind_for(&self, p: &str, h: &str) -> Option<Kind> {
        let all = self.runs_for(p);
        if all.is_empty() {
            return None;
        }
        if !h.is_empty()
            && let Some(k) = all
                .iter()
                .find(|k| self.harness_of(k.as_str()).as_str() == h && self.runs(k.as_str()))
        {
            return Some(k.clone());
        }
        if let Some(k) = all.iter().find(|k| self.runs(k.as_str())) {
            return Some(k.clone());
        }
        all.into_iter().next()
    }

    /// Whether any of provider p's agents runs here.
    pub fn provider_installed(&self, p: &str) -> bool {
        self.harnesses(p).iter().any(|k| self.runs(k.as_str()))
    }

    /// Provider p as you'd call it: the company behind its models
    /// (Anthropic, OpenAI, GitHub), else its agent's name (Ollama).
    pub fn provider_label(&self, p: &str) -> String {
        let n = self.provider_name(p);
        if !n.is_empty() {
            return n;
        }
        self.get(p).map_or_else(|| p.to_string(), |a| a.name())
    }

    /// Agent k as rush names it everywhere: its provider, then the harness
    /// it runs in, OpenAI (Codex), Ollama (Pi); the one name alone when
    /// they're the same, OpenCode.
    pub fn label(&self, k: &str) -> String {
        let p = self.provider_label(&self.provider_of(k));
        let h = self.harness_label(k);
        if p == h || h.is_empty() {
            return p;
        }
        format!("{p} ({h})")
    }

    /// The program agent k runs in, by name: Claude Code for Ollama in
    /// Claude Code.
    pub fn harness_label(&self, k: &str) -> String {
        let h = self.harness_of(k);
        self.get(h.as_str()).map_or(h.0, |a| a.name())
    }

    /// Where provider p's programs read its API key from; "" when it takes
    /// none.
    pub fn key_env(&self, p: &str) -> String {
        self.get(p)
            .and_then(|a| a.keyed().map(|k| k.key_env()))
            .unwrap_or_default()
    }

    /// Whether agent k is paid for only with its provider's API key: a
    /// provider that takes one, in another's harness.
    pub fn key_only(&self, k: &str) -> bool {
        self.harness_of(k).as_str() != k
            && !self.key_env(&self.provider_of(k)).is_empty()
            && !self.on_plan(k)
    }

    // A provider paid for two ways is two providers in rush: its
    // subscription, which only its own program signs in to, and its API
    // key, which any harness that speaks its API can use. The key one is
    // named key_of the provider: "claude-key" is Anthropic by API key.

    /// Whether provider p is paid for both ways: a subscription its own
    /// program signs in to, and an API key apart from it.
    pub fn split(&self, p: &str) -> bool {
        !self.key_env(p).is_empty() && self.supports(p, Feature::SignIn)
    }

    /// The provider id names, and whether it's that one's API key:
    /// "claude-key" is claude by key, "claude" its subscription.
    pub fn billed(&self, id: &str) -> (String, bool) {
        if let Some(p) = id.strip_suffix(KEY_SUFFIX)
            && self.split(p)
        {
            return (p.to_string(), true);
        }
        (id.to_string(), false)
    }

    /// The agents that run provider id: a split provider's subscription in
    /// its own program and in those that sign in to its plan (PlanRider),
    /// its key in any other that speaks its API; any other provider in all
    /// of harnesses.
    pub fn runs_for(&self, id: &str) -> Vec<Kind> {
        let (p, key) = self.billed(id);
        let mut all = self.harnesses(&p);
        if !self.split(&p) {
            return all;
        }
        all.retain(|k| {
            if key {
                !self.on_plan(k.as_str())
            } else {
                k.as_str() == p || self.on_plan(k.as_str())
            }
        });
        all
    }

    // ---- Routes (Go's route.go) ----
    //
    // A route is a provider (by id, as billed reads it: "claude" is
    // Anthropic's plan, "claude-key" its API key) in a harness: what a
    // session runs on. The agent that runs it is an adapter, found by
    // compat; nothing a user sees or keeps names the adapter itself.

    /// Whether agent k runs its provider's plan in another's harness.
    fn on_plan(&self, k: &str) -> bool {
        self.get(k)
            .and_then(|a| a.plan_rider().map(|p| p.on_plan()))
            .unwrap_or(false)
    }

    /// Every harness, each once, in the registry's order.
    pub fn harness_kinds(&self) -> Vec<Kind> {
        let mut out: Vec<Kind> = Vec::new();
        for a in self.all().iter() {
            let k = a.kind();
            if !self.is_current(&k) {
                continue;
            }
            let h = self.harness_of(k.as_str());
            if !out.contains(&h) && self.get(h.as_str()).is_some() {
                out.push(h);
            }
        }
        out
    }

    /// Every provider as billed reads it, each split one's key after its
    /// plan, whether installed or not.
    pub fn provider_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        for p in self.providers() {
            let split = self.split(&p);
            if split {
                out.push(p.clone());
                out.push(key_of(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    /// How provider id runs in harness h: (the agent that runs it, a
    /// warning when it does but you'd want to know, why when it can't).
    /// The agent is set whenever a route exists, installed or not; empty
    /// when none does.
    pub fn compat(&self, id: &str, h: &str) -> (Kind, String, String) {
        let (p, key) = self.billed(id);
        let k = self
            .runs_for(id)
            .into_iter()
            .find(|c| self.harness_of(c.as_str()).as_str() == h)
            .unwrap_or_default();
        let (pn, hn) = (self.provider_label(&p), self.harness_label(h));
        let mut why = String::new();
        if k.is_empty() && !key && self.split(&p) {
            let others: String = self
                .runs_for(&p)
                .iter()
                .filter(|c| c.as_str() != p)
                .map(|c| format!(" or {}", self.harness_label(c.as_str())))
                .collect();
            why = format!(
                "{pn}'s plan runs in {}{others}, not {hn}",
                self.harness_label(&p)
            );
        } else if k.is_empty() {
            why = format!("rush can't run {pn} in {hn}");
            if let Some(a) = self.get(h)
                && let Some(s) = a.speaker()
            {
                why = format!("{hn} speaks only {}", s.speaks());
            }
        } else if !self.runs(k.as_str()) {
            why = format!("{hn} isn't installed");
            let hint = self.hint(k.as_str());
            if !hint.is_empty() {
                why += &format!(": {hint}");
            }
        }
        let mut warn = String::new();
        if !k.is_empty() && self.on_plan(k.as_str()) {
            warn = format!("runs your {pn} plan through {hn}");
        }
        (k, warn, why)
    }

    /// The provider id agent k is paid through and the harness it runs in:
    /// a plan rider's plan, a key rider's key, else as key says.
    pub fn route_of(&self, k: &str, key: bool) -> (String, Kind) {
        let p = self.provider_of(k);
        if self.split(&p) && (key || self.key_only(k)) {
            return (key_of(&p), self.harness_of(k));
        }
        (p, self.harness_of(k))
    }

    /// Provider id as one word to type: anthropic-sub, openai-api, ollama.
    pub fn provider_word(&self, id: &str) -> String {
        let (p, key) = self.billed(id);
        let w = word(&self.provider_label(&p));
        if key {
            w + "-api"
        } else if self.split(&p) {
            w + "-sub"
        } else {
            w
        }
    }

    /// Harness h as one word to type: claude-code, codex, pi.
    pub fn harness_word(&self, h: &str) -> String {
        word(&self.harness_label(h))
    }

    /// The provider id a typed word names: its word, its id, its company or
    /// agent's name; "" when none does.
    pub fn provider_by_word(&self, w: &str) -> String {
        let w = w.to_lowercase();
        for id in self.provider_ids() {
            let (p, key) = self.billed(&id);
            if w == self.provider_word(&id)
                || w == id
                || !key && !self.split(&p) && w == word(&self.label(&p))
            {
                return id;
            }
        }
        String::new()
    }

    /// The harness a typed word names: its word, its kind, its program's
    /// name or one of its words; empty when none does.
    pub fn harness_by_word(&self, w: &str) -> Kind {
        let w = w.to_lowercase();
        for h in self.harness_kinds() {
            let mut words = vec![
                self.harness_word(h.as_str()),
                h.0.clone(),
                self.program_of(h.as_str()),
            ];
            if let Some(a) = self.get(h.as_str())
                && let Some(wd) = a.worded()
            {
                words.extend(wd.words());
            }
            if words.contains(&w) {
                return h;
            }
        }
        Kind::default()
    }

    // ---- What's installed (Go's installed.go) ----

    /// Has every ask answered from what was last found, for rush's view,
    /// whose UI thread never waits on the disk: a stale answer (or none
    /// yet) has the programs looked for again in the background, and the
    /// next ask has them. It starts the first look at once.
    pub fn never_wait(&self) {
        lock(&self.0.found).no_wait = true;
        self.look();
    }

    /// Every registered agent's program, found at most every INSTALLED_FOR.
    fn look(&self) -> Arc<HashMap<Kind, String>> {
        let mut f = lock(&self.0.found);
        if let Some(p) = &f.paths
            && f.at.is_some_and(|at| at.elapsed() < INSTALLED_FOR)
        {
            return p.clone();
        }
        if f.no_wait {
            if !f.looking {
                f.looking = true;
                let r = self.clone();
                std::thread::spawn(move || {
                    let paths = Arc::new(r.look_all());
                    {
                        let mut f = lock(&r.0.found);
                        f.paths = Some(paths);
                        f.at = Some(Instant::now());
                    }
                    // An agent's profiles ask whether it's installed: read
                    // them with what was just found.
                    let homes = r.all().iter().map(|a| (a.kind(), a.profiles())).collect();
                    let mut f = lock(&r.0.found);
                    f.homes = homes;
                    f.looking = false;
                    r.0.landed.notify_all();
                });
            }
            // None reads as nothing installed, for a moment.
            return f.paths.clone().unwrap_or_default();
        }
        let paths = Arc::new(self.look_all());
        f.paths = Some(paths.clone());
        f.at = Some(Instant::now());
        paths
    }

    fn look_all(&self) -> HashMap<Kind, String> {
        let env = self.env();
        let all = self.all();
        let mut paths: HashMap<Kind, String> = HashMap::new();
        for a in all.iter() {
            let Some(p) = a.programmer() else {
                paths.insert(a.kind(), "-".into()); // nothing to look for: taken as there
                continue;
            };
            let (name, dirs) = p.program();
            let mut found = find_in(&env, &name, &dirs).unwrap_or_default();
            if found.is_empty()
                && let Some(l) = a.lesser()
            {
                let (name, dirs, _) = l.lesser();
                if let Some(p) = find_in(&env, &name, &dirs) {
                    found = format!("~{p}");
                }
            }
            paths.insert(a.kind(), found);
        }
        for a in all.iter() {
            if let Some(r) = a.rider() {
                let rides = paths.get(&r.rides()).cloned().unwrap_or_default();
                if rides.is_empty() || rides.starts_with('~') {
                    paths.insert(a.kind(), String::new());
                }
            }
        }
        paths
    }

    /// Forgets what was found, so the next ask looks again. Never waiting,
    /// what was found stands until the new look lands.
    pub fn recheck(&self) {
        let mut f = lock(&self.0.found);
        if f.no_wait {
            f.at = None;
        } else {
            f.paths = None;
        }
    }

    /// a's profiles. Never waiting, they're as last read, with the
    /// programs, in the background.
    pub fn profiles_of(&self, a: &AdapterRef) -> Vec<Profile> {
        if !lock(&self.0.found).no_wait {
            return a.profiles();
        }
        self.look(); // looks again when it's time
        lock(&self.0.found)
            .homes
            .get(&a.kind())
            .cloned()
            .unwrap_or_default()
    }

    fn path_of(&self, k: &str) -> String {
        self.look().get(k).cloned().unwrap_or_default()
    }

    /// Whether agent k is on this machine: its program, or the lesser one
    /// it can do something with.
    pub fn installed(&self, k: &str) -> bool {
        !self.path_of(k).is_empty()
    }

    /// Whether agent k's own program is on this machine, so rush can run
    /// its sessions.
    pub fn runs(&self, k: &str) -> bool {
        let p = self.path_of(k);
        !p.is_empty() && !p.starts_with('~')
    }

    /// What installing agent k's own program would add, when only its
    /// lesser one is here; empty otherwise.
    pub fn hint(&self, k: &str) -> String {
        if !self.path_of(k).starts_with('~') {
            return String::new();
        }
        self.get(k)
            .and_then(|a| a.lesser().map(|l| l.lesser().2))
            .unwrap_or_default()
    }

    /// Where agent k's program is; empty when it isn't installed, or has no
    /// program to find.
    pub fn path(&self, k: &str) -> String {
        let p = self.path_of(k);
        if p == "-" || p.starts_with('~') {
            return String::new();
        }
        p
    }

    /// Every registered agent whose program is here, by kind.
    pub fn installed_all(&self) -> Vec<AdapterRef> {
        let paths = self.look();
        self.all()
            .iter()
            .filter(|a| paths.get(&a.kind()).is_some_and(|p| !p.is_empty()))
            .cloned()
            .collect()
    }

    /// The agent whose program a shell runs as name (a bare name or a path
    /// to it), when one is registered.
    pub fn program_kind(&self, name: &str) -> Option<Kind> {
        self.program_kinds().get(base(name)).cloned()
    }

    fn program_kinds(&self) -> Arc<HashMap<String, Kind>> {
        let mut cache = lock(&self.0.programs);
        if let Some(m) = &*cache {
            return m.clone();
        }
        let mut m = HashMap::new();
        for a in self.all().iter().rev() {
            // the first by kind wins, as it did
            if a.rider().is_some() {
                continue;
            }
            if let Some(p) = a.programmer() {
                let (n, _) = p.program();
                if !n.is_empty() {
                    m.insert(n, a.kind());
                }
            }
        }
        let m = Arc::new(m);
        *cache = Some(m.clone());
        m
    }
}

/// The last element of a path, as Go's filepath.Base.
fn base(p: &str) -> &str {
    if p.is_empty() {
        return ".";
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/";
    }
    t.rsplit('/').next().unwrap_or(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[derive(Clone, Default)]
    struct Prog {
        kind: &'static str,
        name: &'static str,
        dirs: Vec<String>,
    }

    impl Adapter for Prog {
        fn kind(&self) -> Kind {
            Kind::from(self.kind)
        }
        fn name(&self) -> String {
            self.kind.to_string()
        }
        fn features(&self) -> Features {
            Features::new()
        }
        fn level(&self) -> Level {
            Level::Preview
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        fn programmer(&self) -> Option<&dyn Programmer> {
            Some(self)
        }
    }

    impl Programmer for Prog {
        fn program(&self) -> (String, Vec<String>) {
            (self.name.to_string(), self.dirs.clone())
        }
    }

    fn install(dir: &std::path::Path, name: &str) -> String {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        p.to_string_lossy().into_owned()
    }

    // An agent shows as installed when its program is on PATH or where its
    // installer puts it, and a program installed later shows after recheck.
    #[test]
    fn installed() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let r = Registry::with_env(
            &bin.path().to_string_lossy(),
            &home.path().to_string_lossy(),
        );
        r.register(Prog {
            kind: "test-here",
            name: "rush-test-here",
            dirs: vec![".here/bin".into()],
        });
        r.register(Prog {
            kind: "test-later",
            name: "rush-test-later",
            ..Prog::default()
        });
        let here = install(&home.path().join(".here/bin"), "rush-test-here");
        r.recheck();
        assert!(r.installed("test-here"));
        assert_eq!(r.path("test-here"), here);
        assert!(!r.installed("test-later"), "installed before it is");
        install(&home.path().join(".local/bin"), "rush-test-later");
        assert!(
            !r.installed("test-later"),
            "a look is kept for a while, not made on every ask"
        );
        r.recheck();
        assert!(r.installed("test-later"), "not found after recheck");
        assert!(
            r.installed_all()
                .iter()
                .all(|a| a.kind().as_str() != "test-nothing")
        );
        assert_eq!(
            r.program_kind("/usr/bin/rush-test-here"),
            Some(Kind::from("test-here"))
        );
        assert!(r.is_program("rush-test-later") && !r.is_program("/"));
    }

    #[derive(Clone, Default)]
    struct WithLesser(Prog);

    impl Adapter for WithLesser {
        fn kind(&self) -> Kind {
            self.0.kind()
        }
        fn name(&self) -> String {
            self.0.name()
        }
        fn features(&self) -> Features {
            Features::new()
        }
        fn level(&self) -> Level {
            Level::Preview
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        fn programmer(&self) -> Option<&dyn Programmer> {
            Some(&self.0)
        }
        fn lesser(&self) -> Option<&dyn Lesser> {
            Some(self)
        }
    }

    impl Lesser for WithLesser {
        fn lesser(&self) -> (String, Vec<String>, String) {
            ("rush-test-gh".into(), Vec::new(), "install it".into())
        }
    }

    // An agent whose own program is missing but whose lesser one is there
    // is installed, can't run sessions, and says what installing it adds.
    #[test]
    fn lesser() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let r = Registry::with_env(
            &bin.path().to_string_lossy(),
            &home.path().to_string_lossy(),
        );
        r.register(WithLesser(Prog {
            kind: "test-lesser",
            name: "rush-test-cli",
            ..Prog::default()
        }));
        r.recheck();
        assert!(
            !r.installed("test-lesser"),
            "installed with neither program"
        );
        install(bin.path(), "rush-test-gh");
        r.recheck();
        assert!(r.installed("test-lesser") && !r.runs("test-lesser"));
        assert_eq!(r.hint("test-lesser"), "install it");
        assert_eq!(r.path("test-lesser"), "");
        install(bin.path(), "rush-test-cli");
        r.recheck();
        assert!(r.runs("test-lesser") && r.hint("test-lesser").is_empty());
    }

    // Never waiting, the first ask has nothing, and a look in the
    // background brings it.
    #[test]
    fn never_wait_looks_in_the_background() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        install(bin.path(), "rush-test-bg");
        let r = Registry::with_env(
            &bin.path().to_string_lossy(),
            &home.path().to_string_lossy(),
        );
        r.register(Prog {
            kind: "test-bg",
            name: "rush-test-bg",
            ..Prog::default()
        });
        r.never_wait();
        // Wait for the look to land.
        let mut f = lock(&r.0.found);
        while f.looking {
            f = r.0.landed.wait(f).unwrap();
        }
        drop(f);
        assert!(r.runs("test-bg"));
    }

    struct Reader {
        kind: &'static str,
        images: bool,
    }

    impl Adapter for Reader {
        fn kind(&self) -> Kind {
            Kind::from(self.kind)
        }
        fn name(&self) -> String {
            self.kind.to_string()
        }
        fn features(&self) -> Features {
            if self.images {
                Features::from([(Feature::Images, Support::YES)])
            } else {
                Features::new()
            }
        }
        fn level(&self) -> Level {
            Level::Preview
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        fn default_modeler(&self) -> Option<&dyn DefaultModeler> {
            Some(self)
        }
        fn media_reader(&self) -> Option<&dyn MediaReader> {
            Some(self)
        }
    }

    impl DefaultModeler for Reader {
        fn default_model(&self) -> String {
            "seer".into()
        }
    }

    /// Knows "seer", which reads images, and "blind", which reads text.
    impl MediaReader for Reader {
        fn reads(&self, model: &str) -> Option<Media> {
            match model {
                "seer" => Some(Media::IMAGE | Media::PDF),
                "blind" => Some(Media::default()),
                _ => None,
            }
        }
    }

    // A file goes only to a model that reads its kind; an agent without
    // images, or a model that can't, says so, and a model no one knows lets
    // it through.
    #[test]
    fn unreadable() {
        let r = Registry::new();
        r.register(Reader {
            kind: "test-reader",
            images: true,
        });
        r.register(Reader {
            kind: "test-noimages",
            images: false,
        });
        for (k, model, file, want) in [
            ("test-reader", "seer", "a.PNG", ""),
            ("test-reader", "", "a.png", ""), // its default, seer
            ("test-reader", "seer", "a.mp4", "seer can't read video"),
            ("test-reader", "blind", "a.png", "blind can't read images"),
            ("test-reader", "blind", "notes.txt", ""),
            ("test-reader", "new-model", "a.wav", ""),
            (
                "test-noimages",
                "seer",
                "a.png",
                "test-noimages can't take images",
            ),
        ] {
            assert_eq!(
                r.unreadable(k, model, &[file.to_string()]),
                want,
                "unreadable({k}, {model:?}, {file})"
            );
        }
    }

    /// An agent that is a provider, a rider on it, and a harness.
    #[derive(Default)]
    struct Agent {
        kind: &'static str,
        name: &'static str,
        maker: Option<&'static str>,
        key: Option<&'static str>,
        rides: Option<&'static str>,
        provider: Option<&'static str>,
        on_plan: bool,
        sign_in: bool,
    }

    impl Adapter for Agent {
        fn kind(&self) -> Kind {
            Kind::from(self.kind)
        }
        fn name(&self) -> String {
            self.name.to_string()
        }
        fn features(&self) -> Features {
            if self.sign_in {
                Features::from([(Feature::SignIn, Support::YES)])
            } else {
                Features::new()
            }
        }
        fn level(&self) -> Level {
            Level::Full
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        fn made(&self) -> Option<&dyn Made> {
            self.maker.map(|_| self as &dyn Made)
        }
        fn keyed(&self) -> Option<&dyn Keyed> {
            self.key.map(|_| self as &dyn Keyed)
        }
        fn rider(&self) -> Option<&dyn Rider> {
            self.rides.map(|_| self as &dyn Rider)
        }
        fn provided(&self) -> Option<&dyn Provided> {
            self.provider.map(|_| self as &dyn Provided)
        }
        fn plan_rider(&self) -> Option<&dyn PlanRider> {
            self.rides.map(|_| self as &dyn PlanRider)
        }
    }

    impl Made for Agent {
        fn maker(&self) -> String {
            self.maker.unwrap_or_default().into()
        }
    }
    impl Keyed for Agent {
        fn key_env(&self) -> String {
            self.key.unwrap_or_default().into()
        }
    }
    impl Rider for Agent {
        fn rides(&self) -> Kind {
            Kind::from(self.rides.unwrap_or_default())
        }
    }
    impl Provided for Agent {
        fn provider(&self) -> String {
            self.provider.unwrap_or_default().into()
        }
    }
    impl PlanRider for Agent {
        fn on_plan(&self) -> bool {
            self.on_plan
        }
    }

    /// Anthropic in Claude Code (both ways paid), in Pi on its plan and by
    /// key; Pi itself; Ollama in Pi.
    fn fleet() -> Registry {
        let r = Registry::with_env("", "");
        r.register(Agent {
            kind: "claude",
            name: "Claude Code",
            maker: Some("Anthropic"),
            key: Some("ANTHROPIC_API_KEY"),
            sign_in: true,
            ..Agent::default()
        });
        r.register(Agent {
            kind: "pi",
            name: "Pi",
            ..Agent::default()
        });
        r.register(Agent {
            kind: "claude-pi",
            name: "Claude in Pi",
            rides: Some("pi"),
            provider: Some("claude"),
            on_plan: true,
            ..Agent::default()
        });
        r.register(Agent {
            kind: "claude-key-pi",
            name: "Claude key in Pi",
            rides: Some("pi"),
            provider: Some("claude"),
            ..Agent::default()
        });
        r.register(Agent {
            kind: "ollama-pi",
            name: "Ollama",
            rides: Some("pi"),
            provider: Some("ollama"),
            ..Agent::default()
        });
        r
    }

    #[test]
    fn providers_harnesses_and_routes() {
        let r = fleet();
        assert_eq!(r.providers(), ["claude", "ollama", "pi"]);
        assert_eq!(r.provider_ids(), ["claude", "claude-key", "ollama", "pi"]);
        assert_eq!(r.label("claude"), "Anthropic (Claude Code)");
        assert_eq!(r.label("ollama-pi"), "ollama (Pi)");
        assert_eq!(r.label("pi"), "Pi");
        assert_eq!(r.billed("claude-key"), ("claude".to_string(), true));
        assert_eq!(r.billed("pi-key"), ("pi-key".to_string(), false));
        let kinds = |v: Vec<Kind>| v.into_iter().map(|k| k.0).collect::<Vec<_>>();
        assert_eq!(
            kinds(r.harnesses("claude")),
            ["claude", "claude-key-pi", "claude-pi"]
        );
        assert_eq!(kinds(r.runs_for("claude")), ["claude", "claude-pi"]);
        assert_eq!(kinds(r.runs_for("claude-key")), ["claude", "claude-key-pi"]);
        assert_eq!(kinds(r.harness_kinds()), ["claude", "pi"]);
        assert!(r.key_only("claude-key-pi") && !r.key_only("claude-pi"));
        assert_eq!(
            r.route_of("claude-key-pi", false),
            ("claude-key".into(), Kind::from("pi"))
        );
        assert_eq!(
            r.route_of("claude-pi", false),
            ("claude".into(), Kind::from("pi"))
        );
        assert_eq!(r.provider_word("claude"), "anthropic-sub");
        assert_eq!(r.provider_word("claude-key"), "anthropic-api");
        assert_eq!(r.provider_by_word("Anthropic-API"), "claude-key");
        assert_eq!(r.provider_by_word("ollama"), "ollama");
        assert_eq!(r.harness_by_word("claude-code"), Kind::from("claude"));

        // An agent with no program to look for is taken as there.
        let (k, warn, why) = r.compat("claude", "pi");
        assert_eq!(k, Kind::from("claude-pi"));
        assert_eq!(warn, "runs your Anthropic plan through Pi");
        assert_eq!(why, "");
        let (k, _, why) = r.compat("ollama", "claude");
        assert!(k.is_empty());
        assert_eq!(why, "rush can't run ollama in Claude Code");
        assert_eq!(r.kind_for("claude", "pi"), Some(Kind::from("claude-pi")));
        assert_eq!(r.kind_for("claude", ""), Some(Kind::from("claude")));
        assert_eq!(r.kind_for("nosuch", ""), None);
    }

    #[test]
    fn features_of_an_unknown_agent_are_no() {
        let r = fleet();
        assert!(r.supports("claude", Feature::SignIn));
        assert!(!r.supports("claude", Feature::Run));
        assert_eq!(r.feature_of("nosuch", Feature::Run), Support::NO);
        assert_eq!(r.level_of("nosuch"), Level::Preview);
        assert_eq!(r.level_of("pi"), Level::Full);
        assert_eq!(r.context_window("nosuch", "m"), DEFAULT_WINDOW);
        assert_eq!(r.model_name("claude", "m"), "m");
        r.unregister("pi");
        assert!(r.get("pi").is_none() && r.all().len() == 4);
    }

    #[test]
    fn migrated_kinds() {
        assert_eq!(migrated(""), Kind::from(LEGACY_KIND));
        assert_eq!(migrated("codex"), Kind::from("codex"));
        assert_eq!(base("/a/b/"), "b");
        assert_eq!(word("OpenAI (Codex) 5.5"), "openai-codex-55");
    }
}
