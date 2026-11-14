//! The models Codex's account offers, from its models cache, and what a
//! new session can start with. Port of internal/adapters/codex/models.go:
//! 1-77 and choices.go:1-45.

use std::path::Path;

use rush_core::adapter::{Choice, Choices, Media};

use crate::json::{arr, int, st};

/// One model in Codex's models cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CachedModel {
    pub slug: String,
    pub visibility: String,
    pub description: String,
    pub context_window: i64,
}

/// Every model Codex's cache in dir has, listed or not, with first the
/// model config.toml in dir sets when the cache doesn't have it: OpenAI
/// serves a new model (gpt-6.2-sol) before Codex's cache lists it, and
/// whoever set it there runs on it.
pub(crate) fn read_models(dir: &str) -> Vec<CachedModel> {
    let mut models = cached_models(dir);
    if let Some(slug) = configured_model(dir)
        && !models.is_empty()
        && !models.iter().any(|m| m.slug.eq_ignore_ascii_case(&slug))
    {
        // As big a context as the cache's model of its name (sol) has.
        let short = &slug[slug.rfind('-').map_or(0, |i| i + 1)..];
        let context_window = models
            .iter()
            .find(|m| m.slug.ends_with(&format!("-{short}")))
            .map_or(0, |m| m.context_window);
        models.insert(
            0,
            CachedModel {
                slug,
                visibility: "list".into(),
                description: "Codex's default model, set in its config.toml.".into(),
                context_window,
            },
        );
    }
    models
}

fn cached_models(dir: &str) -> Vec<CachedModel> {
    let Ok(b) = std::fs::read(Path::new(dir).join("models_cache.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) else {
        return Vec::new();
    };
    arr(&v, "models")
        .iter()
        .map(|m| CachedModel {
            slug: st(m, "slug"),
            visibility: st(m, "visibility"),
            description: st(m, "description"),
            context_window: int(m, "context_window").unwrap_or(0),
        })
        .collect()
}

/// The model config.toml in dir sets, if any.
fn configured_model(dir: &str) -> Option<String> {
    let text = std::fs::read_to_string(Path::new(dir).join("config.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    match table.get("model")? {
        toml::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// The models Codex offers the account signed in at dir, as its models
/// cache last had them: the ones its picker lists, with what each is for.
pub(crate) fn list_models(dir: &str) -> Vec<Choice> {
    read_models(dir)
        .into_iter()
        .filter(|m| !m.slug.is_empty() && m.visibility == "list")
        .map(|m| Choice {
            id: m.slug,
            note: m
                .description
                .trim()
                .strip_suffix('.')
                .unwrap_or(m.description.trim())
                .to_string(),
            context: m.context_window,
        })
        .collect()
}

/// A model Codex doesn't have.
#[derive(Debug)]
pub struct NoModel(pub String);

impl std::fmt::Display for NoModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NoModel {}

/// name as Codex knows it: a model of models, or the one whose last word
/// it is (astra for gpt-6-astra). With no models known, name stands as it
/// is, as does a whole GPT id (gpt-6.2-sol): OpenAI serves models before
/// Codex's cache lists them. Else one Codex doesn't have is an error that
/// says which it has, rather than a turn its server refuses.
pub(crate) fn model_id(name: &str, models: &[CachedModel]) -> Result<String, NoModel> {
    if name.is_empty() || models.is_empty() {
        return Ok(name.to_string());
    }
    let low = name.to_lowercase();
    let mut by = Vec::new();
    for m in models {
        if m.slug.eq_ignore_ascii_case(name) || m.slug.to_lowercase() == low {
            return Ok(m.slug.clone());
        }
        if m.slug.to_lowercase().ends_with(&format!("-{low}")) {
            by.push(m.slug.clone());
        }
    }
    if by.len() == 1 {
        return Ok(by.remove(0));
    }
    if low.starts_with("gpt-") {
        return Ok(low);
    }
    let listed: Vec<&str> = models
        .iter()
        .filter(|m| m.visibility == "list")
        .map(|m| m.slug.as_str())
        .collect();
    Err(NoModel(format!(
        "codex has no model {name:?}; it has {}",
        listed.join(", ")
    )))
}

fn choice(id: &str, note: &str) -> Choice {
    Choice {
        id: id.into(),
        note: note.into(),
        context: 0,
    }
}

/// Codex's reasoning efforts and rush's three presets of its approval
/// policy and sandbox (modes). Its models come and go with the account,
/// so Settings offers the ones your sessions have used, or a typed one.
// ponytail: efforts as models_cache.json's supported_reasoning_levels had
// them (2026-09); read them per model if they drift.
pub(crate) fn choices() -> Choices {
    Choices {
        models: Vec::new(),
        efforts: vec![
            choice("low", "quick; fine for simple, well-specified tasks."),
            choice("medium", "a balance of speed and care."),
            choice("high", "careful; for tricky work."),
            choice("xhigh", "more careful still; for hard problems."),
            choice("max", "as much as the model has, bar ultra."),
            choice("ultra", "the most, on the models that have it (not luna)."),
        ],
        modes: vec![
            choice(
                "read-only",
                "reads and answers; asks before changing anything.",
            ),
            choice(
                "auto",
                "edits in the workspace without asking; asks before the network or outside it.",
            ),
            choice(
                "full-access",
                "never asks, and can reach anything. Only for sandboxed or throwaway work.",
            ),
        ],
    }
}

/// What model reads, by family: every GPT reads images, but Spark, which
/// reads only text. Codex sends nothing else.
// ponytail: a table by name; app-server's model/list says each model's
// input modalities, for when a new family gets it wrong.
pub(crate) fn reads(model: &str) -> Option<Media> {
    if model.contains("spark") {
        return Some(Media::default());
    }
    if model.starts_with("gpt-") {
        return Some(Media::IMAGE);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(slug: &str, vis: &str) -> CachedModel {
        CachedModel {
            slug: slug.into(),
            visibility: vis.into(),
            ..CachedModel::default()
        }
    }

    // models_test.go:TestListModels
    #[test]
    fn lists_the_pickers_models() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("models_cache.json"),
            r#"{"models":[
		{"slug":"gpt-6-astra","visibility":"list","description":"Frontier intelligence."},
		{"slug":"gpt-reserve","visibility":"hide","description":"Hidden."},
		{"slug":"gpt-6-luna","visibility":"list","description":"Fast and cheap."}]}"#,
        )
        .unwrap();
        let got = list_models(&dir.path().to_string_lossy());
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got[0].id.as_str(), got[0].note.as_str(), got[1].id.as_str()),
            ("gpt-6-astra", "Frontier intelligence", "gpt-6-luna")
        );
    }

    // models_test.go:TestModelID
    #[test]
    fn model_ids() {
        let models = vec![
            m("gpt-6-astra", "list"),
            m("gpt-reserve", "hide"),
            m("gpt-6-luna", "list"),
        ];
        for (input, want) in [
            ("astra", "gpt-6-astra"),
            ("GPT-6-Luna", "gpt-6-luna"),
            ("gpt-reserve", "gpt-reserve"),
            ("", ""),
        ] {
            assert_eq!(model_id(input, &models).unwrap(), want, "{input}");
        }
        let err = model_id("opus[1m]", &models).unwrap_err();
        assert!(err.0.contains("gpt-6-astra, gpt-6-luna"), "{err}");
        assert_eq!(model_id("anything", &[]).unwrap(), "anything");
        // A GPT the cache doesn't list yet goes to the server as named.
        assert_eq!(model_id("GPT-6.2-Sol", &models).unwrap(), "gpt-6.2-sol");
    }

    // A model config.toml sets that the cache doesn't list comes first,
    // with the context of the cache's model of its name; one it lists, or
    // none set, changes nothing.
    #[test]
    fn configs_model_listed() {
        let cache = r#"{"models":[
		{"slug":"gpt-6.1-sol","visibility":"list","description":"Workhorse.","context_window":500000},
		{"slug":"gpt-6-luna","visibility":"list","description":"Fast."}]}"#;
        for (config, want) in [
            (
                "model = \"gpt-6.2-sol\"\nmodel_reasoning_effort = \"high\"\n",
                vec![
                    ("gpt-6.2-sol", 500000),
                    ("gpt-6.1-sol", 500000),
                    ("gpt-6-luna", 0),
                ],
            ),
            (
                "model = \"gpt-6-luna\"\n",
                vec![("gpt-6.1-sol", 500000), ("gpt-6-luna", 0)],
            ),
            ("", vec![("gpt-6.1-sol", 500000), ("gpt-6-luna", 0)]),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("models_cache.json"), cache).unwrap();
            std::fs::write(dir.path().join("config.toml"), config).unwrap();
            let got: Vec<(String, i64)> = list_models(&dir.path().to_string_lossy())
                .into_iter()
                .map(|c| (c.id, c.context))
                .collect();
            let want: Vec<(String, i64)> =
                want.into_iter().map(|(s, c)| (s.to_string(), c)).collect();
            assert_eq!(got, want, "{config}");
        }
    }

    // installed_test.go:TestReads
    #[test]
    fn reads_by_family() {
        assert_eq!(reads("gpt-5.5-codex"), Some(Media::IMAGE));
        assert_eq!(reads("gpt-5.3-codex-spark"), Some(Media::default()));
        assert_eq!(reads("o9"), None);
    }
}
