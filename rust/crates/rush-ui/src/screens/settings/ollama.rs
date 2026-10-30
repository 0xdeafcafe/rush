//! Settings › Providers › Ollama: the context a model runs with, the
//! local models found, and Pruning, the decision models #compact prunes
//! with (pull, mend, remove, and which prunes). After Go's
//! internal/ui/settings_ollama_models.go (ollamaModelSettings :18, pulls
//! :105, decisionSection :109, pullModel :212) and settings_providers.go:550
//! (providerForm's Ollama sections). What Ollama is asked runs off the UI
//! thread; nothing here reads the disk or the network while drawing.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_core::adapter::Choice;
use rush_ollama::Local;

use super::common::reg;
use super::form::{Section, Setting};
use super::paint::first_non_empty;
use super::{State, ask, changed, config, config_mut};
use crate::frame::Out;
use crate::frame::confirm::Confirm;
use crate::model::Model;
use crate::style::tokens;
use crate::worker::Note;

/// The process's Ollama, as its adapters registered it.
pub fn local() -> Arc<Local> {
    rush_ollama::shared(reg())
}

/// The kinds that run on Ollama, whose models a refresh lists
/// (ollama.Kind, CodexKind, PiKind, VibeKind).
const KINDS: [&str; 4] = [
    rush_ollama::KIND,
    rush_ollama::CODEX_KIND,
    rush_ollama::PI_KIND,
    rush_ollama::VIBE_KIND,
];

/// The Ollama section: the context model runs with, a refresh of the
/// local models, and each installed model that can't run a coding
/// harness, saying why (ollamaModelSettings).
pub fn model_settings(l: &Arc<Local>, model: &str) -> Vec<Setting> {
    let n = l.context_size(model);
    let value = if n > 0 {
        tokens(n)
    } else {
        "automatic".to_string()
    };
    let mut info = "Loaded on demand when a session starts. Context changes apply to new sessions."
        .to_string();
    if let Some(known) = l.cached_model(model) {
        if known.context > 0 {
            info.push_str(&format!(
                " Currently loaded: {} tokens.",
                tokens(known.context)
            ));
        }
        if known.max_context > 0 {
            info.push_str(&format!(
                " Model maximum: {} tokens.",
                tokens(known.max_context)
            ));
        }
    }
    let (l1, m1) = (l.clone(), model.to_string());
    let context = Setting {
        label: "Context size".into(),
        value,
        what: info,
        keys: vec!["enter".into(), "change".into()],
        key: Some(Rc::new(move |m, s| {
            if s != "enter" {
                return Out::Pass;
            }
            let (l, model) = (l1.clone(), m1.clone());
            let now = l.context_size(&model).to_string();
            ask(
                m,
                "context tokens (0 = automatic)",
                &now,
                Rc::new(move |m, input| {
                    let Ok(n) = input.trim().parse::<i64>() else {
                        m.flash_err("Enter a whole number of context tokens");
                        return Out::Draw;
                    };
                    let (l, model) = (l.clone(), model.clone());
                    m.later(
                        move || l.set_context_size(&model, n).map_err(|e| e.to_string()),
                        |m, r| {
                            match r {
                                Err(e) => m.fail(&e),
                                Ok(()) => m.flash("Context saved for new Ollama sessions"),
                            }
                            Out::Draw
                        },
                    );
                    Out::Draw
                }),
            );
            Out::Draw
        })),
        ..Setting::default()
    };
    let l2 = l.clone();
    let refresh = Setting {
        label: "Refresh local models".into(),
        what: "Starts Ollama if needed and discovers installed models. Models load into memory only when used.".into(),
        keys: vec!["enter".into(), "refresh".into()],
        key: Some(Rc::new(move |m, s| {
            if s != "enter" {
                return Out::Pass;
            }
            let l = l2.clone();
            m.later(
                move || {
                    l.installed_models(Some(Duration::from_secs(30)))
                        .map_err(|e| e.to_string())
                },
                refreshed,
            );
            Out::Draw
        })),
        ..Setting::default()
    };
    let mut rows = if model.is_empty() {
        vec![refresh]
    } else {
        vec![context, refresh]
    };
    for md in l.cached_installed_models().iter() {
        // Decision models are under Pruning.
        if !md.can_code() && !md.can("decision") {
            rows.push(Setting {
                label: md.name.clone(),
                value: "no coding tools".into(),
                what: "Installed in Ollama. This model does not advertise tool calling, which Rush coding harnesses require.".into(),
                ..Setting::default()
            });
        }
    }
    rows
}

/// The models a refresh found, as every Ollama agent's models.
fn refreshed(m: &mut Model, r: Result<Vec<rush_ollama::Model>, String>) -> Out {
    let models = match r {
        Ok(v) => v,
        Err(e) => {
            m.fail(&e);
            return Out::Draw;
        }
    };
    let choices: Vec<Choice> = models
        .iter()
        .filter(|md| md.can_code())
        .map(|md| Choice {
            id: md.name.clone(),
            context: md.max_context,
            note: format!("{} · loads on demand", md.params),
        })
        .collect();
    for k in KINDS {
        m.kept::<State>()
            .listed
            .insert(k.to_string(), choices.clone());
        crate::screens::accounts::state::accounts(m)
            .listed
            .insert(k.to_string(), choices.clone());
    }
    m.flash(&format!(
        "Ollama ready · {} installed models · {} support coding tools",
        models.len(),
        choices.len()
    ));
    Out::Draw
}

/// The Pruning section: each installed decision model (enter prunes with
/// it, x removes it), each Ollama can't read (enter pulls it again), and
/// a row that pulls one by name (decisionSection).
pub fn decision_section(m: &Model, l: &Arc<Local>) -> Section {
    let mut in_use = config(m).prune_model.clone();
    if in_use.is_empty() {
        in_use = l.cached_decision_model();
    }
    let mut rows = Vec::new();
    for md in l.cached_installed_models().iter() {
        if !md.can("decision") {
            continue;
        }
        let name = md.name.clone();
        let mut value = first_non_empty(&[&md.params, "installed"]).to_string();
        if name == in_use {
            value.push_str(" · prunes");
        }
        let l1 = l.clone();
        rows.push(Setting {
            label: name.clone(),
            value,
            keys: ["enter", "prune with it", "x", "remove"].map(String::from).to_vec(),
            what: "A decision model: it scores which tool steps a conversation still needs, and pruning keeps those word for word.".into(),
            key: Some(Rc::new(move |m, s| match s {
                "enter" => {
                    config_mut(m).prune_model = name.clone();
                    changed(m);
                    m.flash(&format!("Pruning with {name}"));
                    Out::Draw
                }
                "x" => {
                    remove(m, &l1, &name, true);
                    Out::Draw
                }
                _ => Out::Pass,
            })),
            ..Setting::default()
        });
    }
    let mut broken: Vec<(String, String)> = l.broken().into_iter().collect();
    broken.sort();
    for (name, why) in broken {
        let l1 = l.clone();
        rows.push(Setting {
            label: name.clone(),
            value: "files missing".into(),
            keys: ["enter", "pull again", "x", "remove"].map(String::from).to_vec(),
            what: format!(
                "Ollama lists it but can't read it ({why}). Pulling it again fetches what's missing."
            ),
            key: Some(Rc::new(move |m, s| match s {
                "enter" => pull_model(m, &l1, &name),
                "x" => {
                    remove(m, &l1, &name, false);
                    Out::Draw
                }
                _ => Out::Pass,
            })),
            ..Setting::default()
        });
    }
    let l1 = l.clone();
    rows.push(Setting {
        label: "Pull a decision model".into(),
        keys: vec!["enter".into(), "pull".into()],
        what: "Downloads a decision model into Ollama by name: tev1 or nimble for text.".into(),
        key: Some(Rc::new(move |m, s| {
            if s != "enter" {
                return Out::Pass;
            }
            let l = l1.clone();
            ask(
                m,
                "decision model to pull",
                "tev1",
                Rc::new(move |m, name| {
                    let name = name.trim();
                    if name.is_empty() {
                        return Out::Idle;
                    }
                    pull_model(m, &l, name)
                }),
            );
            Out::Draw
        })),
        ..Setting::default()
    });
    // Judged by Codex on real sessions, tev1 ranked steps below Claude
    // Haiku (AUC 0.73 vs 0.84) at 1.6s a step: #compact only, for now.
    Section {
        title: "Pruning".into(),
        note: "experimental · local decision models for #compact".into(),
        rows,
        ..Section::default()
    }
}

/// Removes name from Ollama once you say so; an installed one that
/// prunes no longer does (decisionSection's x).
fn remove(m: &mut Model, l: &Arc<Local>, name: &str, installed: bool) {
    let (l, name) = (l.clone(), name.to_string());
    m.ask(Confirm::new(
        format!("Remove {name} from Ollama?"),
        move |m| {
            m.later(
                move || {
                    let r = l.remove(&name).map_err(|e| e.to_string());
                    (name, r)
                },
                move |m, (name, r)| {
                    if let Err(e) = r {
                        m.fail(&e);
                        if installed {
                            return Out::Draw;
                        }
                    } else if installed {
                        if config(m).prune_model == name {
                            config_mut(m).prune_model.clear();
                            changed(m);
                        }
                        m.flash(&format!("Removed {name}"));
                    }
                    Out::Draw
                },
            );
            Out::Draw
        },
    ));
}

/// The pulls under way, by model (pulls).
static PULLS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn pulls<R>(f: impl FnOnce(&mut HashSet<String>) -> R) -> R {
    let mut g = PULLS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashSet::new))
}

/// Pulls name, how far it's come said each second until it's in
/// (pullModel).
pub fn pull_model(m: &mut Model, l: &Arc<Local>, name: &str) -> Out {
    let name = name.to_string();
    if !pulls(|p| p.insert(name.clone())) {
        m.flash_err(&format!("{name} is already being pulled"));
        return Out::Draw;
    }
    m.flash(&format!("pulling {name}…"));
    let tx = m.sender().map(|t| t.lane(1));
    let l = l.clone();
    m.later(
        move || {
            let n2 = name.clone();
            let mut last = Instant::now();
            let mut said = -1;
            let mut progress = |done: i64, total: i64| {
                let pct = done * 100 / total.max(1);
                if pct == said || last.elapsed() < Duration::from_secs(1) {
                    return;
                }
                (said, last) = (pct, Instant::now());
                if let Some(tx) = &tx {
                    let n = n2.clone();
                    tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                        if pulls(|p| p.contains(&n)) {
                            m.flash(&format!("pulling {n} · {pct}%"));
                        }
                        Out::Draw
                    })));
                }
            };
            let r = l
                .pull(&name, Some(&mut progress))
                .and_then(|()| {
                    l.installed_models(Some(Duration::from_secs(30)))
                        .map(|_| ())
                })
                .map_err(|e| e.to_string());
            pulls(|p| p.remove(&name));
            (name, r)
        },
        |m, (name, r)| {
            match r {
                Err(e) => m.fail(&format!("{name}: {e}")),
                Ok(()) => m.flash(&format!("{name} is ready")),
            }
            Out::Draw
        },
    );
    Out::Draw
}
