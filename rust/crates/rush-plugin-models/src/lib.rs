//! The language models plugins offer, as one helper adapter for rush's own
//! small jobs: session titles, feed summaries, quick ask, memory notes, and
//! #compact's list of summarisers. It's no agent: nothing starts a session
//! with it, and it has no accounts or profiles. Its models are named
//! "<plugin>/<model>" and asked through the plugin broker. New in the Rust
//! port: Go had no plugin models.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use rush_core::adapter::{
    Adapter, Features, Kind, Level, PLUGIN_MODELS, Profile, Registry, Result, Summarizer,
};
use rush_plugin::models::{PluginModel, ask_model, list_models};

/// How long a list of the plugins' models is used before asking again:
/// titles and feed summaries ask often.
const LISTED_FOR: Duration = Duration::from_secs(60);

/// How long a failed ask for the list stands: the broker may be starting.
const FAILED_FOR: Duration = Duration::from_secs(5);

/// A models.list from the broker, and when it was asked.
type Listed = Option<(Instant, std::result::Result<Vec<PluginModel>, String>)>;

/// The plugins' models, as one adapter.
#[derive(Default)]
pub struct PluginModels {
    /// The broker's last models.list, and when it was asked.
    listed: Mutex<Listed>,
}

impl PluginModels {
    pub fn new() -> PluginModels {
        PluginModels::default()
    }

    /// Every model the running plugins offer, available or not: the last
    /// list while it's fresh, else the broker's. Asks hold the lock, so
    /// many at once ask the broker once.
    fn listed(&self) -> std::result::Result<Vec<PluginModel>, String> {
        let mut l = self.listed.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, got)) = &*l {
            let fresh = if got.is_ok() { LISTED_FOR } else { FAILED_FOR };
            if at.elapsed() < fresh {
                return got.clone();
            }
        }
        let got = list_models();
        *l = Some((Instant::now(), got.clone()));
        got
    }

    /// The last list, without asking for a new one.
    fn last(&self) -> Vec<PluginModel> {
        let l = self.listed.lock().unwrap_or_else(|e| e.into_inner());
        match &*l {
            Some((_, Ok(ms))) => ms.clone(),
            _ => Vec::new(),
        }
    }
}

/// What rush calls a plugin's model: "<plugin>/<model>".
fn full_name(m: &PluginModel) -> String {
    format!("{}/{}", m.plugin, m.model.name)
}

impl Adapter for PluginModels {
    fn kind(&self) -> Kind {
        Kind::from(PLUGIN_MODELS)
    }
    fn name(&self) -> String {
        "Plugin models".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::Preview
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn helper(&self) -> bool {
        true
    }
    /// Here when a plugin you approved and switched on offers models: read
    /// from its approval, without asking the broker.
    fn here(&self) -> bool {
        rush_plugin::enabled().values().any(|a| a.manifest.models)
    }
    fn summarizer(&self) -> Option<&dyn Summarizer> {
        Some(self)
    }
}

impl Summarizer for PluginModels {
    /// The available ones, in plugin order; none when the broker isn't
    /// running (yet).
    fn summary_models(&self) -> Result<Vec<String>> {
        let ms = self.listed().unwrap_or_default();
        Ok(ms
            .iter()
            .filter(|m| m.model.available)
            .map(full_name)
            .collect())
    }

    fn summarize(&self, model: &str, system: &str, text: &str) -> Result<String> {
        let Some((plugin, name)) = model.split_once('/') else {
            return Err(format!("{model} isn't a plugin's model: name it <plugin>/<model>").into());
        };
        ask_model(plugin, name, system, text).map_err(|e| format!("{plugin}: {e}").into())
    }

    /// The plugins' models that can't answer now, and why.
    fn summary_exclusions(&self) -> Vec<String> {
        self.last()
            .iter()
            .filter(|m| !m.model.available)
            .map(|m| {
                let why = if m.model.why.is_empty() {
                    "unavailable"
                } else {
                    m.model.why.as_str()
                };
                format!("{}: {why}", full_name(m))
            })
            .collect()
    }

    /// The size its plugin says it holds.
    fn context_tokens(&self, model: &str) -> Option<usize> {
        self.last()
            .iter()
            .find(|m| full_name(m) == model)
            .and_then(|m| usize::try_from(m.model.context_tokens).ok())
            .filter(|&n| n > 0)
    }
}

/// Registers the plugins' models with reg.
pub fn register(reg: &Registry) {
    reg.register(PluginModels::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_plugin::models::Model;

    // Only available models are offered; the others say why not; a name
    // without its plugin is refused before anything is asked.
    #[test]
    fn offers_available_models_by_plugin() {
        let p = PluginModels::new();
        let m = |plugin: &str, name: &str, available: bool, why: &str| PluginModel {
            plugin: plugin.into(),
            model: Model {
                name: name.into(),
                available,
                why: why.into(),
                ..Default::default()
            },
        };
        *p.listed.lock().unwrap() = Some((
            Instant::now(),
            Ok(vec![
                m("apple-intelligence", "on-device", true, ""),
                m("other", "big", false, "Apple Intelligence is off"),
            ]),
        ));
        assert_eq!(
            p.summary_models().unwrap(),
            ["apple-intelligence/on-device"]
        );
        assert_eq!(
            p.summary_exclusions(),
            ["other/big: Apple Intelligence is off"]
        );
        assert!(p.summarize("on-device", "s", "t").is_err());
        assert!(p.helper());
        assert_eq!(p.kind().as_str(), PLUGIN_MODELS);
    }
}
