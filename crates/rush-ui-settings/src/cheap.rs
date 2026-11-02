//! Settings › Usage's "Cheaper model for routine subagents" (Rust-only):
//! off until turned on. What it does is rush_host::cheap's; it sits on
//! Settings › Usage, after the context guard.

use rush_core::adapter::Feature;
use rush_state::config::{CHEAP_MODE_ALL, CHEAP_MODE_ON};

use super::form::{Section, choice, set};
use super::{config, config_mut};
use crate::model::Model;

/// The setting and, under it, a model row for each harness that can take
/// one.
pub fn sections(m: &Model) -> Vec<Section> {
    let mode = config(m).cheap_subagents.mode.clone();
    let any_all = m
        .reg
        .all()
        .iter()
        .any(|a| m.reg.supports(a.kind().as_str(), Feature::SubagentModelAll));
    let mut modes = vec![
        (
            "",
            "off: the agent picks each subagent's model as it does now.",
        ),
        (
            CHEAP_MODE_ON,
            "rush tells the agent to start a subagent on the cheaper model when the work is routine and well specified.",
        ),
    ];
    if any_all {
        modes.push((
            CHEAP_MODE_ALL,
            "every subagent that names no model of its own starts on it; design and debugging included.",
        ));
    }
    let mut rows = vec![choice(
        "Cheaper model for routine subagents",
        &mode,
        "A subagent that searches, reads, runs tests or makes a described edit doesn't need your strongest model, and on a cheaper one it costs a fraction. On: rush adds a few words to the agent's instructions, telling it to start a subagent on the cheaper model for routine, well-specified work (a described edit, a lookup, a search, running and reading tests, a mechanical refactor) and to keep the strong model for design, debugging a cause it doesn't know yet, reviews and anything vague. The agent decides each time; rush calls no model for it. Applies to sessions started, or moved onto a new build, after the change. The Efficiency place shows how much work went to the cheaper model and about what it saved.",
        &modes,
        set(|m, v| config_mut(m).cheap_subagents.mode = v.to_string()),
    )];
    if !mode.is_empty() {
        for a in m.reg.all().iter() {
            let kind = a.kind().as_str().to_string();
            if !m.reg.supports(&kind, Feature::SubagentModel) {
                continue;
            }
            let offered = m.reg.cheaper_models(&kind);
            let label = format!("  {}", a.name());
            if offered.is_empty() {
                rows.push(choice(
                    &label,
                    "",
                    "This harness lists no model cheaper than the one it runs by default, so none is offered.",
                    &[("", "not offered.")],
                    None,
                ));
                continue;
            }
            let ids: Vec<String> = offered.iter().map(|c| c.id.clone()).collect();
            let cur = config(m)
                .cheap_subagents
                .model_for(&kind, &ids)
                .unwrap_or_default();
            let choices: Vec<(&str, &str)> = offered
                .iter()
                .map(|c| (c.id.as_str(), c.note.as_str()))
                .collect();
            let k = kind.clone();
            rows.push(choice(
                &label,
                &cur,
                "The model this harness's agents start routine subagents on, from the models it lists that cost less than the one it runs by default.",
                &choices,
                set(move |m, v| {
                    config_mut(m)
                        .cheap_subagents
                        .models
                        .insert(k.clone(), v.to_string());
                }),
            ));
        }
    }
    vec![Section {
        title: "Cheaper subagents".into(),
        note: "off until you turn it on".into(),
        rows,
        ..Section::default()
    }]
}
