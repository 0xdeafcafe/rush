//! Mistral Vibe on Ollama: vibe-acp, as the vibe adapter drives it, in a
//! VIBE_HOME of rush's own whose config.toml points a provider named
//! "ollama" at Ollama's OpenAI-compatible API, so it never reads the
//! user's ~/.vibe, nor needs their Mistral key.
//!
//! Ported from Go's internal/adapters/ollama/vibe.go:1-160.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use rush_core::adapter::{
    Adapter, Choice, Conn, ContextWindower, Driver, Feature, Features, Kind, Level, Media,
    MediaReader, ModelLister, ModelNamer, Pricer, Profile, Programmer, Provided, Registry, Result,
    Rider, Sink, StartOptions, Support,
};
use rush_core::agent::TokenUsage;

use crate::api::Model;
use crate::codex::quote;
use crate::{KIND, Local, driver_of, harness_binary, manage, tune};

/// Ollama in Vibe's kind.
pub const VIBE_KIND: &str = "ollama-vibe";

/// Vibe's kind, which it rides.
const VIBE: &str = "vibe";

/// The provider rush names in Vibe's config.
const VIBE_PROVIDER: &str = "ollama";

/// Mistral Vibe on Ollama.
#[derive(Clone)]
pub struct OllamaVibe {
    reg: Registry,
    local: Arc<Local>,
}

impl OllamaVibe {
    pub fn new(reg: Registry, local: Arc<Local>) -> OllamaVibe {
        OllamaVibe { reg, local }
    }

    /// The VIBE_HOME sessions on Ollama keep to.
    pub fn home(&self) -> String {
        self.local
            .state_dir()
            .join(VIBE_KIND)
            .to_string_lossy()
            .into_owned()
    }
}

/// Vibe's features over ACP, less what a local model or rush's own
/// folder hasn't.
fn features() -> Features {
    use Feature::*;
    let yes = Support::YES;
    let no = Support::NO;
    Features::from([
        (Run, yes.clone()),
        (Resume, yes.clone()),
        (Interrupt, yes.clone()),
        (
            Model,
            yes.clone().with("any model Ollama has that calls tools"),
        ),
        (Modes, yes.clone().with("Vibe's own")),
        (Questions, yes.clone()),
        (Mcp, yes.clone()),
        (Images, yes.clone().with("on models that take them")),
        (HandoffIn, yes.clone()),
        (Pricing, yes.clone().with("free: it runs on this machine")),
        (
            Effort,
            no.clone().with("a model thinks or doesn't, as it was made"),
        ),
        (Subagents, yes.with("Vibe's task tool")),
        (
            Background,
            no.clone()
                .with("Vibe runs nothing beside the turn over ACP"),
        ),
        (Quota, no.clone().with("no limits but this machine's")),
        (Switch, no.clone().with("no accounts, only this machine")),
        (SignIn, no.with("nothing to sign in to")),
        (History, Support::PLANNED),
    ])
}

/// Vibe's config.toml with one provider, Ollama served at base, and only
/// the models given, the first active. Ollama's OpenAI API streams a
/// model's thinking as "reasoning". Vibe compacts a quarter short of each
/// model's window, when it's known.
pub fn vibe_config(models: &[Model], base: &str) -> String {
    // TOML's basic strings, for the names Ollama gives.
    let names: Vec<String> = models.iter().map(|m| quote(&m.name)).collect();
    let mut b = String::new();
    let _ = write!(
        b,
        "active_model = {}\nallowed_models = [{}]\n",
        names.first().map_or("\"\"", String::as_str),
        names.join(", ")
    );
    let _ = write!(
        b,
        "\n[[providers]]\nname = {}\napi_base = {}\napi_style = \"openai\"\nreasoning_field_name = \"reasoning\"\n",
        quote(VIBE_PROVIDER),
        quote(&format!("{base}/v1"))
    );
    for m in models {
        let _ = write!(
            b,
            "\n[[models]]\nname = {}\nprovider = {}\nalias = {}\ninput_price = 0.0\noutput_price = 0.0\n",
            quote(&m.name),
            quote(VIBE_PROVIDER),
            quote(&m.name)
        );
        if m.can("vision") {
            b.push_str("supports_images = true\n");
        }
        let n = tune::window(m);
        if n > 0 {
            let _ = writeln!(b, "auto_compact_threshold = {}", n - n / 4);
        }
    }
    b
}

/// Writes config.toml into Vibe's home dir. Written afresh each start, it
/// says what General's Every agent does about attribution; a top-level
/// key, so ahead of the tables.
fn write_vibe_config(dir: &Path, models: &[Model], base: &str, attribution: &str) -> Result<()> {
    let mut cfg = vibe_config(models, base);
    match attribution {
        "off" => cfg.insert_str(0, "include_commit_signature = false\n"),
        "on" => cfg.insert_str(0, "include_commit_signature = true\n"),
        _ => {}
    }
    crate::pi::write_private(dir, "config.toml", cfg.as_bytes())?;
    Ok(())
}

impl Adapter for OllamaVibe {
    fn kind(&self) -> Kind {
        Kind::from(VIBE_KIND)
    }
    fn name(&self) -> String {
        "Ollama in Vibe".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Tested
    }
    /// rush's own VIBE_HOME for Ollama, when ollama and vibe are both here.
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg.installed(VIBE_KIND) {
            return Vec::new();
        }
        vec![Profile {
            kind: Kind::from(VIBE_KIND),
            name: "Ollama in Vibe".into(),
            dir: self.home(),
        }]
    }
    rush_core::has!(
        programmer: Programmer,
        rider: Rider,
        provided: Provided,
        pricer: Pricer,
        driver: Driver,
        media_reader: MediaReader,
        model_lister: ModelLister,
        context_windower: ContextWindower,
        model_namer: ModelNamer,
    );
}

impl Programmer for OllamaVibe {
    /// ollama, which it needs beside Vibe.
    fn program(&self) -> (String, Vec<String>) {
        crate::claude::program()
    }
}

impl Rider for OllamaVibe {
    /// Vibe: its sessions are vibe's.
    fn rides(&self) -> Kind {
        Kind::from(VIBE)
    }
}

impl Provided for OllamaVibe {
    /// Ollama, whichever harness it runs in.
    fn provider(&self) -> String {
        KIND.into()
    }
}

impl Pricer for OllamaVibe {
    /// Nothing: the model runs here.
    fn cost(&self, _model: &str, _u: &TokenUsage) -> Option<f64> {
        Some(0.0)
    }
}

impl Driver for OllamaVibe {
    /// Picks and loads the model, writes Vibe's config for it, then runs
    /// Vibe on it.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>> {
        let h = driver_of(&self.reg, VIBE)?;
        let m = self.local.pick(&o.model)?;
        if o.profile.dir.is_empty() {
            o.profile.dir = self.home();
        }
        let models = self.local.tool_models(&m);
        let attribution = rush_state::Store::load().config.everywhere.attribution;
        write_vibe_config(
            Path::new(&o.profile.dir),
            &models,
            &self.local.server(),
            &attribution,
        )?;
        // The config starts it on m; setting it again would only write it
        // back.
        o.model.clear();
        o.effort.clear();
        // The host hands over the adapter's own program, which is
        // ollama's, unless it was told of a vibe to run.
        o.binary = harness_binary(&self.reg, &o.binary, VIBE);
        h.driver().expect("checked").start(o, sink)
    }
}

impl MediaReader for OllamaVibe {
    fn reads(&self, model: &str) -> Option<Media> {
        self.local.reads(model)
    }
}

impl ModelLister for OllamaVibe {
    fn list_models(&self, _p: &Profile) -> Vec<Choice> {
        self.local.list_models()
    }
}

impl ContextWindower for OllamaVibe {
    fn context_window(&self, model: &str) -> i64 {
        self.local.cached_window(model)
    }
}

impl ModelNamer for OllamaVibe {
    fn model_name(&self, id: &str) -> String {
        manage::model_name(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_points_at_ollama() {
        let got = vibe_config(
            &[
                Model {
                    context: 65536,
                    ..Model::named("qwen3.8:27b-mlx", &["tools", "vision"])
                },
                Model::named("qwen2.5:1.5b", &["tools"]),
            ],
            "http://127.0.0.1:11434",
        );
        for want in [
            "active_model = \"qwen3.8:27b-mlx\"\nallowed_models = [\"qwen3.8:27b-mlx\", \"qwen2.5:1.5b\"]",
            "[[providers]]\nname = \"ollama\"\napi_base = \"http://127.0.0.1:11434/v1\"\napi_style = \"openai\"",
            "name = \"qwen3.8:27b-mlx\"\nprovider = \"ollama\"\nalias = \"qwen3.8:27b-mlx\"",
            "supports_images = true\nauto_compact_threshold = 49152\n",
        ] {
            assert!(got.contains(want), "no {want:?} in\n{got}");
        }
        assert_eq!(got.matches("supports_images").count(), 1, "{got}");
        assert_eq!(got.matches("auto_compact_threshold").count(), 1, "{got}");
        let reg = Registry::new();
        crate::register(&reg);
        assert_eq!(reg.harness_of(VIBE_KIND).as_str(), VIBE);
        assert_eq!(reg.provider_of(VIBE_KIND), KIND);
    }

    #[test]
    fn attribution_goes_first() {
        let d = tempfile::tempdir().unwrap();
        write_vibe_config(d.path(), &[Model::named("m", &[])], "http://h:1", "off").unwrap();
        let got = std::fs::read_to_string(d.path().join("config.toml")).unwrap();
        assert!(
            got.starts_with("include_commit_signature = false\nactive_model = \"m\"\n"),
            "{got}"
        );
    }
}
