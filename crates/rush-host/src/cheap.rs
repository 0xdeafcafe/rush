//! Handing routine subagent work to a cheaper model (Rust-only): what
//! Settings › Usage › "Cheaper model for routine subagents" does when it
//! is on. Nothing here calls a model; it only decides what the agent is
//! told, and which model a harness starts its subagents on.

use rush_core::adapter::{Feature, Registry};
use rush_state::config::{CHEAP_MODE_ALL, CHEAP_MODE_ON, CheapSubagents};

/// The part of the system prompt the setting adds, for the cheaper model.
/// Word for word the same for the same model, so a cached prompt stays
/// cached; only "on" adds it.
pub fn prompt(model: &str) -> String {
    format!(
        "When you start a subagent for routine work that is already well specified (a described edit, a lookup, a search, running tests and reading the result, a mechanical refactor), pass the cheaper model: model: \"{model}\" on the Agent tool, or on rush's spawn_agent tool. Keep the strong model for design, debugging a cause you don't know yet, reviews, and anything vague: leave the model out there."
    )
}

/// What a harness's sessions get from the setting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The addition to the system prompt; empty when none.
    pub prompt: String,
    /// The one model every subagent that names none starts on; empty when
    /// none.
    pub all: String,
}

/// What the setting does for harness kind: nothing when it is off, when
/// the harness can't take it, or offers no cheaper model.
pub fn plan(reg: &Registry, cfg: &CheapSubagents, kind: &str) -> Plan {
    if cfg.mode.is_empty() {
        return Plan::default();
    }
    let offered: Vec<String> = reg.cheaper_models(kind).into_iter().map(|c| c.id).collect();
    let Some(model) = cfg.model_for(kind, &offered) else {
        return Plan::default();
    };
    match cfg.mode.as_str() {
        CHEAP_MODE_ON if reg.supports(kind, Feature::SubagentModel) => Plan {
            prompt: prompt(&model),
            ..Plan::default()
        },
        CHEAP_MODE_ALL if reg.supports(kind, Feature::SubagentModelAll) => Plan {
            all: model,
            ..Plan::default()
        },
        _ => Plan::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The part names the model, is the same every time, and says to keep
    // the strong model for design and debugging.
    #[test]
    fn the_prompt_part_is_stable_and_names_the_model() {
        let p = prompt("sonnet");
        assert_eq!(p, prompt("sonnet"));
        assert!(p.contains("model: \"sonnet\""), "{p}");
        assert!(p.contains("design, debugging"), "{p}");
        assert_ne!(p, prompt("haiku"));
    }

    // Off, or a harness nothing is known about, adds nothing.
    #[test]
    fn off_or_unknown_adds_nothing() {
        let reg = Registry::new();
        let on = CheapSubagents {
            mode: CHEAP_MODE_ON.into(),
            ..CheapSubagents::default()
        };
        assert_eq!(plan(&reg, &on, "claude"), Plan::default());
        assert_eq!(
            plan(&reg, &CheapSubagents::default(), "claude"),
            Plan::default()
        );
    }
}
