//! Vibe's config, models and saved sessions. Port of
//! internal/adapters/vibe/history.go:1-199.

use std::path::{Path, PathBuf};

use regex::RegexBuilder;
use rush_core::adapter::{
    self, Choice, Choices, Chooser, Discoverer, Follow, HistoryFollower, HistoryReader,
    ModelLister, Profile, Session, TailReader,
};
use rush_core::event::Event;
use rush_core::time::{Ms, parse_rfc3339};
use serde::Deserialize;
use std::collections::HashMap;
use toml::{Table, Value};

use super::Vibe;
use crate::history::{Follower, MessageLines, messages_each, read_messages_tail};
use crate::rpc::Error;
use crate::util::{env, glob, home_dir, matches, mod_ms, path_str};

/// Vibe's config.toml (VIBE_CONFIG_PATH's when set), empty when it can't
/// be read.
fn read_config(p: &Profile) -> Table {
    let mut path = Path::new(&Vibe::dir(p)).join("config.toml");
    let custom = env("VIBE_CONFIG_PATH");
    if !custom.is_empty() {
        path = PathBuf::from(custom);
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|b| b.parse::<Table>().ok())
        .unwrap_or_default()
}

fn s(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_string()
}

impl Chooser for Vibe {
    fn choices(&self) -> Choices {
        Choices {
            efforts: ["off", "low", "medium", "high", "max"]
                .iter()
                .map(|id| Choice {
                    id: id.to_string(),
                    ..Choice::default()
                })
                .collect(),
            ..Choices::default()
        }
    }
}

impl ModelLister for Vibe {
    fn list_models(&self, p: &Profile) -> Vec<Choice> {
        let cfg = read_config(p);
        // Vibe merges model overrides with its built-in catalog. Onboarding
        // normally writes no models section, so a list from the TOML alone
        // would wrongly be empty.
        let c = |id: &str, note: &str| Choice {
            id: id.into(),
            note: note.into(),
            context: 0,
        };
        let mut out = vec![
            c("mistral-medium-3.5", "Mistral Medium 3.5 · Vibe built-in"),
            c("local", "Devstral · local llama.cpp server"),
        ];
        let add = |out: &mut Vec<Choice>, id: &str, m: &Table| {
            let name = s(m.get("name"));
            let mut id = id.to_string();
            let alias = s(m.get("alias"));
            if !alias.is_empty() {
                id = alias;
            }
            if id.is_empty() {
                id = name.clone();
            }
            if id.is_empty() {
                return;
            }
            let mut note = s(m.get("display_name"));
            if note.is_empty() {
                note = name;
            }
            let mut choice = Choice {
                id: id.clone(),
                note: note.clone(),
                context: 0,
            };
            if let Some(old) = out.iter_mut().find(|o| o.id == id) {
                if note.is_empty() {
                    choice.note = old.note.clone();
                }
                *old = choice;
                return;
            }
            out.push(choice);
        };
        match cfg.get("models") {
            Some(Value::Array(models)) => {
                for m in models.iter().filter_map(Value::as_table) {
                    add(&mut out, "", m);
                }
            }
            Some(Value::Table(models)) => {
                for (id, m) in models {
                    if let Some(m) = m.as_table() {
                        add(&mut out, id, m);
                    }
                }
            }
            _ => {}
        }
        if let Some(Value::Array(models)) = cfg.get("routed_extra_models") {
            for m in models.iter().filter_map(Value::as_table) {
                add(&mut out, "", m);
            }
        }
        let active = s(cfg.get("active_model"));
        if !active.is_empty() && !out.iter().any(|m| m.id == active) {
            out.push(c(&active, "configured active model"));
        }
        if let Some(Value::Array(allowed)) = cfg.get("allowed_models")
            && !allowed.is_empty()
        {
            let filtered: Vec<Choice> = out
                .iter()
                .filter(|model| {
                    allowed.iter().filter_map(Value::as_str).any(|pattern| {
                        let pattern = pattern.trim();
                        match pattern.strip_prefix("re:") {
                            Some(expr) => RegexBuilder::new(&format!("^(?:{expr})$"))
                                .case_insensitive(true)
                                .build()
                                .is_ok_and(|re| re.is_match(&model.id)),
                            None => matches(&pattern.to_lowercase(), &model.id.to_lowercase()),
                        }
                    })
                })
                .cloned()
                .collect();
            // Vibe keeps every model when a stale filter matches none.
            if !filtered.is_empty() {
                out = filtered;
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

impl Discoverer for Vibe {
    fn live(&self, _: &Profile) -> Vec<Session> {
        Vec::new()
    }

    fn past(&self, p: &Profile) -> Vec<Session> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Config {
            active_model: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Meta {
            session_id: String,
            title: String,
            start_time: String,
            environment: HashMap<String, String>,
            config: Config,
        }
        let mut root = Path::new(&Vibe::dir(p)).join("logs").join("session");
        if let Some(Value::Table(logging)) = read_config(p).get("session_logging") {
            let mut path = s(logging.get("save_dir"));
            if !path.is_empty() {
                if let Some(rest) = path.strip_prefix("~/") {
                    path = path_str(&Path::new(&home_dir()).join(rest));
                }
                root = PathBuf::from(path);
            }
        }
        let mut out = Vec::new();
        for path in glob(&root, &["*", "meta.json"]) {
            let Ok(b) = std::fs::read(&path) else {
                continue;
            };
            let Ok(m) = serde_json::from_slice::<Meta>(&b) else {
                continue;
            };
            if m.session_id.is_empty() {
                continue;
            }
            let transcript = path
                .parent()
                .unwrap_or(Path::new(""))
                .join("messages.jsonl");
            let Ok(st) = std::fs::metadata(&transcript) else {
                continue;
            };
            out.push(Session {
                kind: self.agent.id.clone(),
                profile: p.clone(),
                id: m.session_id,
                name: m.title,
                cwd: m
                    .environment
                    .get("working_directory")
                    .cloned()
                    .unwrap_or_default(),
                model: m.config.active_model,
                transcript: path_str(&transcript),
                state: "done".into(),
                created_at: parse_rfc3339(&m.start_time),
                updated_at: mod_ms(&st),
                ..Session::default()
            });
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }
}

fn no_transcript() -> adapter::Error {
    Box::new(Error::other("vibe: this session has no transcript"))
}

impl HistoryReader for Vibe {
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> adapter::Result<()> {
        if s.transcript.is_empty() {
            return Err(no_transcript());
        }
        Ok(messages_each(&s.transcript, before, most, each)?)
    }
}

impl HistoryFollower for Vibe {
    fn follow_history(&self, s: &Session) -> adapter::Result<Box<dyn Follow>> {
        if s.transcript.is_empty() {
            return Err(no_transcript());
        }
        Ok(Box::new(Follower::new(&s.transcript, MessageLines::boxed)))
    }
}

impl TailReader for Vibe {
    fn history_tail(&self, s: &Session, most: i64) -> adapter::Result<(Vec<Event>, bool)> {
        Ok(read_messages_tail(&s.transcript, most)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(dir: &Path) -> Profile {
        Profile {
            dir: path_str(dir),
            ..Profile::default()
        }
    }

    // history_test.go: TestConfiguredModelsAndSessionDiscovery
    #[test]
    fn configured_models_and_discovery() {
        if !env("VIBE_CONFIG_PATH").is_empty() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(
            dir.join("config.toml"),
            "active_model = \"local\"\n[[models]]\nname = \"devstral-custom\"\nalias = \"local\"\n",
        )
        .unwrap();
        let a = Vibe::without_keychain();
        let models = a.list_models(&profile(dir));
        assert_eq!(models.len(), 2, "{models:?}");
        assert_eq!(models[0].id, "local");
        let root = dir.join("logs/session/saved");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("meta.json"),
            r#"{"session_id":"s","title":"Review","start_time":"2026-01-01T00:00:00Z","environment":{"working_directory":"/work"},"config":{"active_model":"local"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("messages.jsonl"),
            r#"{"role":"user","content":"hi"}"#,
        )
        .unwrap();
        let past = a.past(&profile(dir));
        assert_eq!(past.len(), 1);
        assert_eq!(
            (
                past[0].model.as_str(),
                past[0].cwd.as_str(),
                past[0].name.as_str()
            ),
            ("local", "/work", "Review")
        );
    }

    // history_test.go: TestDefaultAndAllowedModels
    #[test]
    fn default_and_allowed_models() {
        if !env("VIBE_CONFIG_PATH").is_empty() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let a = Vibe::without_keychain();
        assert!(
            a.list_models(&profile(d.path())).len() >= 2,
            "a fresh setup has the built-ins"
        );
        std::fs::write(
            d.path().join("config.toml"),
            "allowed_models = [\"mistral-*\"]\n",
        )
        .unwrap();
        let models = a.list_models(&profile(d.path()));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "mistral-medium-3.5");
        std::fs::write(
            d.path().join("config.toml"),
            "allowed_models = [\"re:LOC.*\"]\n",
        )
        .unwrap();
        assert_eq!(a.list_models(&profile(d.path()))[0].id, "local");
        assert!(
            a.choices().models.is_empty(),
            "choices never read the catalog"
        );
    }
}
