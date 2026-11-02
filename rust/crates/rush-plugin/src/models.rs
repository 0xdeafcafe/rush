//! The language models plugins offer for rush's own small jobs (session
//! titles, feed summaries, quick ask, memory notes): what they say of them,
//! and asking them through the broker. A plugin with "models": true in its
//! manifest answers models.list and models.ask on fd 3; the broker gathers
//! every running plugin's for rush. New in the Rust port: Go had no
//! plugin models.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::client::{dial_broker, ensure_broker};
use crate::ctx::Ctx;

/// How long the broker waits on one plugin's models.list: one slower than
/// this is left out of the list.
pub const MODELS_LIST_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the broker waits on one plugin's models.ask.
pub const MODELS_ASK_TIMEOUT: Duration = Duration::from_secs(60);

/// One model a plugin offers, as its models.list says it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Model {
    /// What models.ask calls it: "on-device".
    pub name: String,
    /// What rush shows: "Apple on-device model".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// How many tokens it holds, prompt and answer together; 0 is unsaid.
    #[serde(skip_serializing_if = "is_zero")]
    pub context_tokens: i64,
    /// Whether it can answer now.
    pub available: bool,
    /// Why it can't, when it isn't available.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// A plugin's answer to models.list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelList {
    pub models: Vec<Model>,
}

/// One model, with the plugin that offers it: the broker's models.list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginModel {
    pub plugin: String,
    #[serde(flatten)]
    pub model: Model,
}

/// The broker's answer to models.list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginModels {
    pub models: Vec<PluginModel>,
}

/// What models.ask is sent: to a plugin, without `plugin`; to the broker,
/// with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ask {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plugin: String,
    pub model: String,
    pub system: String,
    pub prompt: String,
}

/// models.ask's answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Answer {
    pub text: String,
}

/// Every model the running plugins offer, available or not, by plugin.
/// With no broker running it starts one (if a plugin is approved) and
/// says it isn't running: the next ask finds it.
pub fn list_models() -> Result<Vec<PluginModel>, String> {
    let Ok(c) = dial_broker() else {
        let _ = ensure_broker();
        return Err("the plugin broker is not running".into());
    };
    // The broker's own wait on each plugin, and a little for itself.
    let ctx = Ctx::background().with_timeout(MODELS_LIST_TIMEOUT + Duration::from_secs(2));
    let out: Result<PluginModels, _> = c.call(&ctx, "models.list", &serde_json::json!({}));
    c.close();
    out.map(|l| l.models).map_err(|e| e.to_string())
}

/// Asks plugin's model, through the broker, and gives its answer. A
/// prompt too big for it fails with an error saying "too small".
pub fn ask_model(plugin: &str, model: &str, system: &str, prompt: &str) -> Result<String, String> {
    let c = dial_broker().map_err(|_| "the plugin broker is not running".to_string())?;
    let ctx = Ctx::background().with_timeout(MODELS_ASK_TIMEOUT + Duration::from_secs(5));
    let q = Ask {
        plugin: plugin.into(),
        model: model.into(),
        system: system.into(),
        prompt: prompt.into(),
    };
    let out: Result<Answer, _> = c.call(&ctx, "models.ask", &q);
    c.close();
    out.map(|a| a.text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The wire shapes the plugins and the broker speak.
    #[test]
    fn wire_shapes() {
        let l: ModelList = serde_json::from_str(
            r#"{"models":[{"name":"on-device","label":"Apple on-device model","contextTokens":4096,"available":true,"why":""}]}"#,
        )
        .unwrap();
        let m = &l.models[0];
        assert_eq!(
            (m.name.as_str(), m.context_tokens, m.available),
            ("on-device", 4096, true)
        );
        let pm = PluginModel {
            plugin: "apple-intelligence".into(),
            model: m.clone(),
        };
        assert_eq!(
            serde_json::to_string(&pm).unwrap(),
            r#"{"plugin":"apple-intelligence","name":"on-device","label":"Apple on-device model","contextTokens":4096,"available":true}"#
        );
        let q = Ask {
            model: "on-device".into(),
            system: "s".into(),
            prompt: "p".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&q).unwrap(),
            r#"{"model":"on-device","system":"s","prompt":"p"}"#
        );
    }
}
