//! One provider's models in another's harness, where the provider serves
//! the API that harness speaks: DeepSeek and GLM in Claude Code, on their
//! Anthropic-compatible APIs, and Anthropic, OpenAI, DeepSeek and GLM in
//! Pi. Each is paid for per token with the provider's API key, which
//! Settings › Providers keeps.
//!
//! Pairs that would need a translator aren't here: Claude Code speaks only
//! Anthropic's API, which OpenAI doesn't serve, and Codex only OpenAI's
//! Responses API, which no other provider but Ollama does. Ollama's own
//! pairs are in rush-ollama, fitted to a model on this machine.
//!
//! Ported from Go's internal/adapters/cross/cross.go:1-232. A pair starts
//! its session through its harness's Driver, found in the registry, as Go
//! calls the claude and pi adapters' Start.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use rush_core::adapter::{
    Adapter, Conn, Driver, Feature, Features, Kind, Level, PlanRider, Profile, Programmer,
    Provided, Registry, Result, Rider, Sink, StartOptions, Support,
};

use crate::{Getenv, process_env};

/// Claude Code's kind.
const CLAUDE: &str = "claude";
/// Pi's kind.
const PI: &str = "pi";

/// A provider's models in another's harness.
#[derive(Clone)]
pub struct Pair {
    def: Pair0,
    reg: Registry,
    getenv: Getenv,
    /// rush's folder; None is rush_state's.
    state_dir: Option<PathBuf>,
}

const NONE: Pair0 = Pair0 {
    kind: "",
    name: "",
    provider: "",
    harness: "",
    url: "",
    pi_provider: "",
    pi_api: "",
    models: &[],
    plan: false,
};

impl std::ops::Deref for Pair {
    type Target = Pair0;
    fn deref(&self) -> &Pair0 {
        &self.def
    }
}

/// A pair as written down, before it knows the registry.
#[derive(Clone, Copy)]
pub struct Pair0 {
    kind: &'static str,
    name: &'static str,
    /// Whose models, as its agent's kind.
    provider: &'static str,
    /// The program that runs them: claude or pi.
    harness: &'static str,
    /// The provider's API, in the harness's words.
    url: &'static str,
    /// Pi: the provider it knows by name (anthropic, openai), or one of
    /// rush's own in models.json, speaking pi_api, with models to offer.
    pi_provider: &'static str,
    pi_api: &'static str,
    models: &'static [&'static str],
    /// Runs the provider on the plan the harness is signed in to (Pi's
    /// own /login), in the harness's own folder, with no key at all.
    plan: bool,
}

// ponytail: DeepSeek's and GLM's model ids are as they were named in 2026;
// read the provider's /models when they go stale.
const PAIRS: [Pair0; 8] = [
    Pair0 {
        kind: "deepseek-claude",
        name: "DeepSeek in Claude Code",
        provider: "deepseek",
        harness: CLAUDE,
        url: "https://api.deepseek.com/anthropic",
        ..NONE
    },
    Pair0 {
        kind: "glm-claude",
        name: "GLM in Claude Code",
        provider: "glm",
        harness: CLAUDE,
        url: "https://api.z.ai/api/anthropic",
        ..NONE
    },
    Pair0 {
        kind: "anthropic-pi",
        name: "Anthropic in Pi",
        provider: CLAUDE,
        harness: PI,
        pi_provider: "anthropic",
        ..NONE
    },
    Pair0 {
        kind: "openai-pi",
        name: "OpenAI in Pi",
        provider: "codex",
        harness: PI,
        pi_provider: "openai",
        ..NONE
    },
    Pair0 {
        kind: "anthropic-plan-pi",
        name: "Anthropic plan in Pi",
        provider: CLAUDE,
        harness: PI,
        pi_provider: "anthropic",
        plan: true,
        ..NONE
    },
    Pair0 {
        kind: "openai-plan-pi",
        name: "OpenAI plan in Pi",
        provider: "codex",
        harness: PI,
        pi_provider: "openai-codex",
        plan: true,
        ..NONE
    },
    Pair0 {
        kind: "deepseek-pi",
        name: "DeepSeek in Pi",
        provider: "deepseek",
        harness: PI,
        url: "https://api.deepseek.com/v1",
        pi_provider: "rush-deepseek",
        pi_api: "openai-completions",
        models: &["deepseek-chat", "deepseek-reasoner"],
        ..NONE
    },
    Pair0 {
        kind: "glm-pi",
        name: "GLM in Pi",
        provider: "glm",
        harness: PI,
        url: "https://api.z.ai/api/coding/paas/v4",
        pi_provider: "rush-glm",
        pi_api: "openai-completions",
        models: &["glm-4.6", "glm-4.5-air"],
        ..NONE
    },
];

/// Every provider in a harness it speaks to, knowing what's installed
/// and what runs each harness from reg.
pub fn pairs(reg: &Registry) -> Vec<Pair> {
    pairs_with(reg, process_env(), None)
}

/// pairs, with the environment read through getenv and rush's folder at
/// state_dir: for tests.
pub fn pairs_with(reg: &Registry, getenv: Getenv, state_dir: Option<PathBuf>) -> Vec<Pair> {
    PAIRS
        .iter()
        .map(|p| Pair {
            def: *p,
            reg: reg.clone(),
            getenv: getenv.clone(),
            state_dir: state_dir.clone(),
        })
        .collect()
}

fn err(s: impl Into<String>) -> rush_core::adapter::Error {
    s.into().into()
}

impl Pair {
    /// The harness's folder for the pair: apart from your own, so it reads
    /// none of your plugins, and its sessions never mix with them. On a
    /// plan it's Pi's own, where its /login keeps the sign-in.
    pub fn home(&self) -> String {
        if self.plan {
            return pi_home(&self.getenv);
        }
        let dir = self.state_dir.clone().unwrap_or_else(rush_state::dir);
        dir.join(self.kind).to_string_lossy().into_owned()
    }

    fn harness_driver(&self) -> Result<rush_core::adapter::AdapterRef> {
        self.reg
            .get(self.harness)
            .filter(|a| a.driver().is_some())
            .ok_or_else(|| err(format!("rush can't run {}", self.harness)))
    }

    /// Runs Pi on the provider's plan its /login signed in to, with the
    /// provider's key variable emptied so a key never pays instead.
    fn start_on_plan(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>> {
        if o.profile.dir.is_empty() {
            o.profile.dir = self.home();
        }
        o.api_key.clear();
        o.effort.clear();
        o.binary = self.reg.path(self.harness);
        o.env.push(format!("{}=", self.reg.key_env(self.provider)));
        o.flags
            .splice(0..0, ["--provider".into(), self.pi_provider.into()]);
        let h = self.harness_driver()?;
        h.driver().expect("checked").start(o, sink)
    }

    /// Readies Pi's folder for the pair: the key in the variable Pi reads
    /// for a provider it knows, else a models.json naming rush's own
    /// provider, with the key in it (only you can read it).
    pub fn pi_setup(&self, dir: &Path, key: &str) -> std::io::Result<Vec<String>> {
        if self.pi_api.is_empty() {
            return Ok(vec![format!("{}={key}", self.reg.key_env(self.provider))]);
        }
        let models: Vec<Value> = self
            .models
            .iter()
            .map(|m| json!({"id": m, "name": m, "input": ["text"]}))
            .collect();
        let cfg = json!({"providers": {self.pi_provider: {
            "baseUrl": self.url, "api": self.pi_api, "apiKey": key, "models": models,
        }}});
        write_private(dir, "models.json", &cfg)?;
        Ok(Vec::new())
    }
}

/// Writes v into dir/name, indented two spaces, through a temporary file
/// only you can read.
pub(crate) fn write_private(dir: &Path, name: &str, v: &Value) -> std::io::Result<()> {
    // Made only you able to read it, as Go's MkdirAll(dir, 0o700); a
    // folder already there is left as it is.
    let mut d = std::fs::DirBuilder::new();
    d.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        d.mode(0o700);
    }
    d.create(dir)?;
    let mut b = serde_json::to_vec_pretty(v).map_err(std::io::Error::other)?;
    b.push(b'\n');
    let tmp = dir.join(format!("{name}.tmp"));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&b)?;
    }
    std::fs::rename(tmp, dir.join(name))
}

/// Pi's own folder: PI_CODING_AGENT_DIR, else ~/.pi/agent (Go's pi.Home).
pub fn pi_home(getenv: &Getenv) -> String {
    if let Some(d) = getenv("PI_CODING_AGENT_DIR") {
        return d;
    }
    match crate::home_dir(getenv) {
        Some(h) => Path::new(&h)
            .join(".pi")
            .join("agent")
            .to_string_lossy()
            .into_owned(),
        None => String::new(),
    }
}

/// The models Claude Code names for itself; each goes to the one picked,
/// so none asks the provider for a Claude it hasn't.
const CLAUDE_ALIASES: [&str; 7] = [
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
];

/// Points Claude Code at url with key, never your own Anthropic key or
/// login. With no model picked, the provider maps Claude's names to its
/// own.
pub fn claude_env(url: &str, key: &str, model: &str) -> Vec<String> {
    let mut env = vec![
        format!("ANTHROPIC_BASE_URL={url}"),
        format!("ANTHROPIC_AUTH_TOKEN={key}"),
        "ANTHROPIC_API_KEY=".to_string(),
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1".to_string(),
    ];
    if !model.is_empty() {
        env.extend(CLAUDE_ALIASES.iter().map(|a| format!("{a}={model}")));
    }
    env
}

impl Adapter for Pair {
    fn kind(&self) -> Kind {
        Kind::from(self.kind)
    }
    fn name(&self) -> String {
        self.name.into()
    }
    /// The harness's, less the account it would be signed in to: the key
    /// pays, and has no window to show.
    fn features(&self) -> Features {
        let mut out = self.reg.features(self.harness).unwrap_or_default();
        // Its sessions are in a folder of its own, found by nothing yet,
        // and the harness's commands and prices are for its own provider.
        for f in [Feature::Live, Feature::History, Feature::Commands] {
            out.insert(
                f,
                Support::NO.with("not for a provider in another's harness yet"),
            );
        }
        let no = |s: &str| Support::NO.with(s);
        out.insert(
            Feature::Pricing,
            no("the provider's own prices, which rush doesn't know"),
        );
        out.insert(Feature::Quota, no("paid per token with the API key"));
        out.insert(
            Feature::Switch,
            no("one API key, kept in Settings › Providers"),
        );
        out.insert(Feature::SignIn, no("an API key, not a sign-in"));
        out.insert(Feature::Effort, no("as the provider serves it"));
        if self.plan {
            out.insert(Feature::Quota, no("your plan's, which Pi doesn't report"));
            out.insert(Feature::Switch, no("the one Pi's /login signed in"));
            out.insert(Feature::SignIn, no("Pi's own /login, in Pi"));
        }
        if self.harness == CLAUDE {
            out.insert(
                Feature::Rewind,
                no("provider-specific transcript branching and file rewind are not integrated"),
            );
        }
        out
    }
    fn level(&self) -> Level {
        Level::Preview
    }
    /// rush's own folder for the pair, when its harness is here.
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg.installed(self.kind) {
            return Vec::new();
        }
        vec![Profile {
            kind: Kind::from(self.kind),
            name: self.name.into(),
            dir: self.home(),
        }]
    }
    rush_core::has!(
        driver: Driver,
        programmer: Programmer,
        rider: Rider,
        provided: Provided,
        plan_rider: PlanRider,
    );
}

impl Rider for Pair {
    fn rides(&self) -> Kind {
        Kind::from(self.harness)
    }
}

impl Provided for Pair {
    fn provider(&self) -> String {
        self.provider.into()
    }
}

impl PlanRider for Pair {
    fn on_plan(&self) -> bool {
        self.plan
    }
}

impl Programmer for Pair {
    /// The harness's: it runs wherever that's installed.
    fn program(&self) -> (String, Vec<String>) {
        self.reg
            .get(self.harness)
            .and_then(|a| a.programmer().map(|p| p.program()))
            .unwrap_or_default()
    }
}

impl Driver for Pair {
    /// Runs the harness on the provider's API, with its key.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>> {
        if self.plan {
            return self.start_on_plan(o, sink);
        }
        let key = if o.api_key.is_empty() {
            rush_state::vault::api_key(self.provider)
        } else {
            o.api_key.clone()
        };
        if key.is_empty() {
            return Err(err(format!(
                "{} has no API key: add one in Settings › Providers",
                self.reg.provider_label(self.provider)
            )));
        }
        if o.profile.dir.is_empty() {
            o.profile.dir = self.home();
        }
        o.api_key.clear();
        o.effort.clear();
        o.binary = self.reg.path(self.harness);
        let h = self.harness_driver()?;
        if self.harness == CLAUDE {
            o.env.extend(claude_env(self.url, &key, &o.model));
            return h.driver().expect("checked").start(o, sink);
        }
        let env = self.pi_setup(Path::new(&o.profile.dir), &key)?;
        o.env.extend(env);
        if o.model.is_empty()
            && let Some(m) = self.models.first()
        {
            o.model = m.to_string();
        }
        o.flags
            .splice(0..0, ["--provider".into(), self.pi_provider.into()]);
        h.driver().expect("checked").start(o, sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(reg: &Registry, kind: &str) -> Pair {
        pairs_with(reg, Arc::new(|_| None), None)
            .into_iter()
            .find(|p| p.kind == kind)
            .unwrap()
    }

    // Claude Code on another provider goes to its URL with its key, never
    // your own Anthropic key; a model picked stands in for Claude's names.
    #[test]
    fn claude_env_points_at_the_provider() {
        let env = claude_env("https://api.deepseek.com/anthropic", "sk-1", "");
        for want in [
            "ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic",
            "ANTHROPIC_AUTH_TOKEN=sk-1",
            "ANTHROPIC_API_KEY=",
        ] {
            assert!(env.iter().any(|e| e == want), "missing {want} in {env:?}");
        }
        assert!(
            !env.iter().any(|e| e.starts_with("ANTHROPIC_MODEL=")),
            "with no model picked, the provider maps Claude's names itself"
        );
        assert!(
            claude_env("u", "k", "glm-4.6")
                .contains(&"ANTHROPIC_DEFAULT_OPUS_MODEL=glm-4.6".to_string()),
            "a model picked stands in for opus"
        );
    }

    // Pi reads a provider it knows from the key's variable, and one it
    // doesn't from a models.json only you can read.
    #[test]
    fn pi_setup() {
        let reg = Registry::new();
        reg.register(rush_claude::Claude::new());
        let dir = tempfile::tempdir().unwrap();
        let env = pair(&reg, "anthropic-pi")
            .pi_setup(dir.path(), "sk-a")
            .unwrap();
        assert_eq!(env, ["ANTHROPIC_API_KEY=sk-a"]);
        let env = pair(&reg, "deepseek-pi")
            .pi_setup(dir.path(), "sk-d")
            .unwrap();
        assert!(env.is_empty());
        let f = dir.path().join("models.json");
        let b = std::fs::read_to_string(&f).unwrap();
        assert!(
            b.contains("\"rush-deepseek\"") && b.contains("sk-d") && b.contains("deepseek-chat"),
            "models.json: {b}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the key's file is {mode:o}");
        }
    }

    // A pair is its provider in its harness: named so, its harness's
    // features less the account, and found as one of the provider's.
    #[test]
    fn pairs_are_riders() {
        let reg = Registry::new();
        reg.register(rush_claude::Claude::new());
        for p in pairs(&reg) {
            reg.register(p);
        }
        assert_eq!(reg.harness_of("glm-claude").as_str(), "claude");
        assert_eq!(reg.provider_of("glm-claude"), "glm");
        assert!(
            reg.harnesses("deepseek")
                .iter()
                .any(|k| k.as_str() == "deepseek-claude")
        );
        assert_eq!(
            reg.program_of("deepseek-claude"),
            "",
            "a rider has no program of its own"
        );
        let f = reg.features("deepseek-claude").unwrap();
        assert_eq!(f[&Feature::Quota].note, "paid per token with the API key");
        assert_eq!(f[&Feature::Live].is, rush_core::adapter::Has::No);
        assert!(f.contains_key(&Feature::Rewind));
        let plan = reg.features("anthropic-plan-pi").unwrap();
        assert_eq!(plan[&Feature::SignIn].note, "Pi's own /login, in Pi");
        assert!(!plan.contains_key(&Feature::Rewind));
        assert!(
            reg.get("openai-plan-pi")
                .unwrap()
                .plan_rider()
                .unwrap()
                .on_plan()
        );
        // Pi isn't registered here: nothing runs a pair in it.
        let e = pair(&reg, "deepseek-pi")
            .start(
                StartOptions {
                    api_key: "sk".into(),
                    profile: Profile {
                        dir: tempfile::tempdir().unwrap().path().to_string_lossy().into(),
                        ..Profile::default()
                    },
                    ..StartOptions::default()
                },
                Arc::new(Nothing),
            )
            .err()
            .unwrap();
        assert_eq!(e.to_string(), "rush can't run pi");
    }

    struct Nothing;
    impl Sink for Nothing {
        fn event(&self, _: rush_core::event::Event) {}
        fn ended(&self, _: Option<rush_core::adapter::Error>) {}
    }

    #[test]
    fn homes() {
        let reg = Registry::new();
        let getenv: Getenv = Arc::new(|k| (k == "HOME").then(|| "/h".to_string()));
        let ps = pairs_with(&reg, getenv, Some(PathBuf::from("/rush")));
        let by = |k: &str| ps.iter().find(|p| p.kind == k).unwrap().home();
        assert_eq!(by("glm-claude"), "/rush/glm-claude");
        assert_eq!(by("openai-plan-pi"), "/h/.pi/agent");
    }
}
