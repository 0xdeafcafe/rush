//! What state needs to know of the agents rush runs: each adapter's
//! providers, harnesses and keys, and whether its program is here. The
//! adapters register themselves, as in Go; until internal/agent's
//! registry is ported, this is that registry, holding each adapter's
//! answers as plain data rather than the interfaces it implements.
//!
//! Ported from internal/agent: adapter.go:303-342 (Register, Get, All),
//! harness.go:39-229 (providers and harnesses, Split, Billed, RunsFor),
//! route.go:1-124 (onPlan, ProviderIDs, Compat), kinds.go:8-20 (LegacyKind,
//! Migrated), kinds.go CurrentKind, installed.go:126-200 (Runs, Hint),
//! choices.go:9-42 (Choices).

use std::sync::{Arc, RwLock};

/// LegacyKind is the agent an empty kind meant: rush ran only Claude Code
/// before it ran others, and wrote no kind for it.
pub const LEGACY_KIND: &str = "claude"; // migration: what older state meant by no kind

/// Migrated is a kind read from state, a host's info or config an older
/// rush wrote, as it's kept now: an empty one is LegacyKind.
pub fn migrated(kind: &str) -> String {
    if kind.is_empty() {
        return LEGACY_KIND.into(); // migration: older rushes wrote no kind for Claude Code
    }
    kind.into()
}

/// Whether an adapter's program is on this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Program {
    /// It has no program to look for: taken as there.
    #[default]
    None,
    /// Its program was found.
    Found,
    /// Its program isn't here.
    Missing,
    /// Only the lesser program it can do something with is here; hint says
    /// what installing its own would add.
    Lesser { hint: String },
}

/// One value a new session can start with, and what it means.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub note: String,
    /// A model's context window in tokens, when known; 0 when not.
    pub context: i64,
}

impl Choice {
    pub fn new(id: &str) -> Choice {
        Choice {
            id: id.into(),
            ..Choice::default()
        }
    }
}

/// What a new session can start with: each list offered after the agent's
/// own default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub models: Vec<Choice>,
    pub efforts: Vec<Choice>,
    pub modes: Vec<Choice>,
}

/// An adapter's answers, as the Go interfaces it implements give them.
#[derive(Clone, Debug, Default)]
pub struct Adapter {
    pub kind: String,
    pub name: String,
    /// Provided: the provider a rider runs, when it isn't its own kind.
    pub provider: Option<String>,
    /// Made: the company behind its models, when it isn't its name.
    pub maker: Option<String>,
    /// Rider: the agent whose program it runs on.
    pub rides: Option<String>,
    /// Keyed: the variable its programs read its API key from.
    pub key_env: Option<String>,
    /// Whether it supports FeatureSignIn: a subscription its program signs in to.
    pub sign_in: bool,
    /// PlanRider: it runs its provider on the plan its harness is signed in to.
    pub on_plan: bool,
    /// Speaker: the one provider's API a harness speaks alone.
    pub speaks: Option<String>,
    /// Replaced: a legacy harness kept for saved sessions, and what replaced it.
    pub replacement: Option<String>,
    /// BuiltinDefNamer: its own agent's name among its definitions.
    pub builtin_def: Option<String>,
    /// Homer: its own home, by name and folder.
    pub home: Option<(String, String)>,
    /// Chooser: what a new session of it can start with.
    pub choices: Option<Choices>,
    pub program: Program,
}

impl Adapter {
    /// An adapter of kind, named after it, with no program to look for.
    pub fn new(kind: &str) -> Adapter {
        Adapter {
            kind: kind.into(),
            name: kind.into(),
            ..Adapter::default()
        }
    }
}

/// Adapters by kind, kept sorted.
static ADAPTERS: RwLock<Vec<Arc<Adapter>>> = RwLock::new(Vec::new());

thread_local! {
    /// This thread's own list, over ADAPTERS: for tests, which run side
    /// by side in one process and each tell it a registry of their own.
    static THREAD_ADAPTERS: std::cell::RefCell<Option<Vec<Arc<Adapter>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Gives this thread a list of its own, empty, over the process's: for
/// tests, as set_thread_dir is.
pub fn use_thread_list() {
    THREAD_ADAPTERS.with(|t| *t.borrow_mut() = Some(Vec::new()));
}

fn with_list<T>(f: impl FnOnce(&mut Vec<Arc<Adapter>>) -> T) -> T {
    let mut f = Some(f);
    let mine = THREAD_ADAPTERS.with(|t| t.borrow_mut().as_mut().map(|l| f.take().unwrap()(l)));
    match (mine, f) {
        (Some(v), _) => v,
        (None, Some(f)) => f(&mut ADAPTERS.write().unwrap_or_else(|e| e.into_inner())),
        (None, None) => unreachable!("f runs once"),
    }
}

/// Register makes an adapter known, or replaces the one of its kind.
pub fn register(a: Adapter) {
    with_list(
        |all| match all.binary_search_by(|x| x.kind.as_str().cmp(&a.kind)) {
            Ok(i) => all[i] = Arc::new(a),
            Err(i) => all.insert(i, Arc::new(a)),
        },
    )
}

/// Get is the adapter for kind, if one is registered.
pub fn get(k: &str) -> Option<Arc<Adapter>> {
    with_list(|all| {
        all.binary_search_by(|x| x.kind.as_str().cmp(k))
            .ok()
            .map(|i| all[i].clone())
    })
}

/// All is every registered adapter, by kind.
pub fn all() -> Vec<Arc<Adapter>> {
    with_list(|all| all.clone())
}

/// CurrentKind is what replaced a legacy harness, else k.
pub fn current_kind(k: &str) -> String {
    match get(k).and_then(|a| a.replacement.clone()) {
        Some(r) => r,
        None => k.into(),
    }
}

fn is_current(a: &Adapter) -> bool {
    current_kind(&a.kind) == a.kind
}

/// ProviderOf is the provider agent k runs: a Rider's Provider, else k.
pub fn provider_of(k: &str) -> String {
    get(k)
        .and_then(|a| a.provider.clone())
        .unwrap_or_else(|| k.into())
}

/// HarnessOf is the program agent k runs in: the agent a Rider rides, else
/// k itself.
pub fn harness_of(k: &str) -> String {
    get(k)
        .and_then(|a| a.rides.clone())
        .unwrap_or_else(|| k.into())
}

/// Providers are every provider an adapter runs, by name, each once.
pub fn providers() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in all() {
        if !is_current(&a) {
            continue;
        }
        let p = provider_of(&a.kind);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// IsProvider is whether p is one of Providers.
pub fn is_provider(p: &str) -> bool {
    all()
        .iter()
        .any(|a| is_current(a) && provider_of(&a.kind) == p)
}

/// Harnesses are the agents that run provider p, whether installed or not:
/// the one of p's own kind first (its default), then the rest by kind.
pub fn harnesses(p: &str) -> Vec<String> {
    let mut out: Vec<String> = all()
        .iter()
        .filter(|a| provider_of(&a.kind) == p && is_current(a))
        .map(|a| a.kind.clone())
        .collect();
    out.sort_by_key(|k| k != p); // stable: p's own first
    out
}

/// KindFor is the agent that runs provider id p in harness h: p's own kind
/// when h is empty or p's own harness; when that one isn't installed, the
/// first of p's that is. None when p has no adapter at all.
pub fn kind_for(p: &str, h: &str) -> Option<String> {
    let all = runs_for(p);
    if all.is_empty() {
        return None;
    }
    if !h.is_empty()
        && let Some(k) = all.iter().find(|k| harness_of(k) == h && runs(k))
    {
        return Some(k.clone());
    }
    if let Some(k) = all.iter().find(|k| runs(k)) {
        return Some(k.clone());
    }
    Some(all[0].clone())
}

/// ProviderInstalled is whether any of provider p's agents runs here.
pub fn provider_installed(p: &str) -> bool {
    harnesses(p).iter().any(|k| runs(k))
}

/// ProviderLabel is provider p as you'd call it: the company behind its
/// models, else its agent's name.
pub fn provider_label(p: &str) -> String {
    match get(p) {
        Some(a) => a.maker.clone().unwrap_or_else(|| a.name.clone()),
        None => p.into(),
    }
}

/// HarnessLabel is the program agent k runs in, by name.
pub fn harness_label(k: &str) -> String {
    let h = harness_of(k);
    match get(&h) {
        Some(a) => a.name.clone(),
        None => h,
    }
}

/// KeyEnv is where provider p's programs read its API key from; "" when
/// it takes none.
pub fn key_env(p: &str) -> String {
    get(p).and_then(|a| a.key_env.clone()).unwrap_or_default()
}

/// KeyOnly is whether agent k is paid for only with its provider's API
/// key: a provider that takes one, in another's harness.
pub fn key_only(k: &str) -> bool {
    harness_of(k) != k && !key_env(&provider_of(k)).is_empty() && !on_plan(k)
}

/// Split is whether provider p is paid for both ways: a subscription its
/// own program signs in to, and an API key apart from it.
pub fn split(p: &str) -> bool {
    !key_env(p).is_empty() && get(p).is_some_and(|a| a.sign_in)
}

const KEY_SUFFIX: &str = "-key";

/// KeyOf is the provider that is p paid for with its API key.
pub fn key_of(p: &str) -> String {
    format!("{p}{KEY_SUFFIX}")
}

/// Billed is the provider id names, and whether it's that one's API key:
/// "claude-key" is claude by key, "claude" its subscription.
pub fn billed(id: &str) -> (String, bool) {
    if let Some(p) = id.strip_suffix(KEY_SUFFIX)
        && split(p)
    {
        return (p.into(), true);
    }
    (id.into(), false)
}

/// RunsFor are the agents that run provider id: a split provider's
/// subscription in its own program and in those that sign in to its plan,
/// its key in any other that speaks its API; any other provider in all of
/// Harnesses.
pub fn runs_for(id: &str) -> Vec<String> {
    let (p, key) = billed(id);
    let mut all = harnesses(&p);
    if !split(&p) {
        return all;
    }
    all.retain(|k| {
        let drop = if key {
            on_plan(k)
        } else {
            *k != p && !on_plan(k)
        };
        !drop
    });
    all
}

/// onPlan is whether agent k runs its provider's plan in another's harness.
pub fn on_plan(k: &str) -> bool {
    get(k).is_some_and(|a| a.on_plan)
}

/// ProviderIDs are every provider as Billed reads it, each split one's
/// key after its plan, whether installed or not.
pub fn provider_ids() -> Vec<String> {
    let mut out = Vec::new();
    for p in providers() {
        let s = split(&p);
        out.push(p.clone());
        if s {
            out.push(key_of(&p));
        }
    }
    out
}

/// Where agent k's program is, as installed.go's look finds it: "" when it
/// isn't installed, "~" when only its lesser one is, "-" when it has none
/// to look for. A rider is installed only while what it rides is.
fn path(k: &str) -> &'static str {
    let Some(a) = get(k) else { return "" };
    let own = match a.program {
        Program::None => "-",
        Program::Found => "/",
        Program::Missing => "",
        Program::Lesser { .. } => "~",
    };
    if let Some(r) = &a.rides {
        let rp = match get(r) {
            Some(ra) => match ra.program {
                Program::None => "-",
                Program::Found => "/",
                Program::Missing => "",
                Program::Lesser { .. } => "~",
            },
            None => "",
        };
        if rp.is_empty() || rp == "~" {
            return "";
        }
    }
    own
}

/// Runs is whether agent k's own program is on this machine, so rush can
/// run its sessions.
pub fn runs(k: &str) -> bool {
    let p = path(k);
    !p.is_empty() && p != "~"
}

/// Hint is what installing agent k's own program would add, when only its
/// lesser one is here; empty otherwise.
pub fn hint(k: &str) -> String {
    if path(k) != "~" {
        return String::new();
    }
    match get(k).map(|a| a.program.clone()) {
        Some(Program::Lesser { hint }) => hint,
        _ => String::new(),
    }
}

/// ChoicesOf are what agent k says a new session can start with.
pub fn choices_of(k: &str) -> Option<Choices> {
    get(k).and_then(|a| a.choices.clone())
}

/// Compat is how provider id runs in harness h: the agent that runs it
/// (set whenever a route exists, installed or not), a warning when it does
/// but you'd want to know, and why when it can't.
pub fn compat(id: &str, h: &str) -> (String, String, String) {
    let (p, key) = billed(id);
    let k = runs_for(id)
        .into_iter()
        .find(|c| harness_of(c) == h)
        .unwrap_or_default();
    let (pn, hn) = (provider_label(&p), harness_label(h));
    let mut why = String::new();
    let mut warn = String::new();
    if k.is_empty() && !key && split(&p) {
        let others: String = runs_for(&p)
            .iter()
            .filter(|o| **o != p)
            .map(|o| format!(" or {}", harness_label(o)))
            .collect();
        why = format!(
            "{pn}'s plan runs in {}{others}, not {hn}",
            harness_label(&p)
        );
    } else if k.is_empty() {
        why = format!("rush can't run {pn} in {hn}");
        if let Some(s) = get(h).and_then(|a| a.speaks.clone()) {
            why = format!("{hn} speaks only {s}");
        }
    } else if !runs(&k) {
        why = format!("{hn} isn't installed");
        let hint = hint(&k);
        if !hint.is_empty() {
            why = format!("{why}: {hint}");
        }
    }
    if !k.is_empty() && on_plan(&k) {
        warn = format!("runs your {pn} plan through {hn}");
    }
    (k, warn, why)
}

#[cfg(test)]
pub(crate) mod fake {
    //! The agents state's tests run against, as Go's tests register them.
    use super::*;
    use std::sync::Once;

    pub(crate) fn register_fakes() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            for k in ["claude", "pa", "pb", "pc", "stubprovider"] {
                register(Adapter::new(k));
            }
            register(Adapter {
                program: Program::Missing,
                ..Adapter::new("gone")
            });
            register(Adapter {
                provider: Some("pa".into()),
                rides: Some("pb".into()),
                ..Adapter::new("pa-pb")
            });
            register(Adapter {
                key_env: Some("PS_KEY".into()),
                sign_in: true,
                ..Adapter::new("ps")
            });
            register(Adapter {
                provider: Some("ps".into()),
                rides: Some("pb".into()),
                ..Adapter::new("ps-pb")
            });
        });
    }

    // A split provider is two: its subscription runs only in its own
    // harness, its key in any.
    #[test]
    fn split_by_billing() {
        register_fakes();
        assert_eq!(runs_for("ps"), ["ps"]);
        assert_eq!(runs_for("ps-key"), ["ps", "ps-pb"]);
        assert_eq!(billed("ps-key"), ("ps".into(), true));
        assert_eq!(billed("pa-key"), ("pa-key".into(), false));
        assert!(runs("pa-pb") && !runs("gone"));
        assert_eq!(compat("pa", "pc").2, "rush can't run pa in pc");
        assert_eq!(compat("ps", "pb").2, "ps's plan runs in ps, not pb");
        assert_eq!(compat("gone", "gone").2, "gone isn't installed");
    }
}
