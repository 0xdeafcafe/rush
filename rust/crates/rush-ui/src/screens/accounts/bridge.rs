//! rush-state keeps its own list of what each agent answers (its
//! providers, harnesses, keys, program, choices), which its profiles and
//! routes read: Go's state reads internal/agent's registry itself. Until
//! the two are one, the adapters the UI knows (Model::reg) are told to
//! state's list once, before Accounts reads a profile.

use rush_core::adapter::{Feature, Registry};
use rush_state::agents::{self, Adapter, Choice, Choices, Program};

use super::state::accounts;
use crate::model::Model;

/// Tells state's list of agents about m's, once per model.
pub fn sync(m: &mut Model) {
    if accounts(m).synced {
        return;
    }
    accounts(m).synced = true;
    tell(&m.reg);
}

/// Registers every adapter of reg with state's list.
pub fn tell(reg: &Registry) {
    for a in reg.all().iter() {
        let k = a.kind();
        let ks = k.as_str();
        let choices = a.chooser().map(|c| {
            let ch = c.choices();
            let conv = |v: Vec<rush_core::adapter::Choice>| {
                v.into_iter()
                    .map(|c| Choice {
                        id: c.id,
                        note: c.note,
                        context: c.context,
                    })
                    .collect()
            };
            Choices {
                models: conv(ch.models),
                efforts: conv(ch.efforts),
                modes: conv(ch.modes),
            }
        });
        let program = if a.programmer().is_none() {
            Program::None
        } else if reg.runs(ks) {
            Program::Found
        } else if reg.installed(ks) {
            Program::Lesser { hint: reg.hint(ks) }
        } else {
            Program::Missing
        };
        agents::register(Adapter {
            kind: ks.to_string(),
            name: a.name(),
            provider: a.provided().map(|p| p.provider()),
            maker: a.made().map(|p| p.maker()),
            rides: a.rider().map(|r| r.rides().0),
            key_env: a.keyed().map(|p| p.key_env()),
            sign_in: reg.supports(ks, Feature::SignIn),
            on_plan: a.plan_rider().is_some_and(|p| p.on_plan()),
            speaks: a.speaker().map(|s| s.speaks()),
            replacement: a.replaced().map(|r| r.replacement().0),
            builtin_def: a.builtin_def_namer().map(|b| b.builtin_def()),
            home: a.homer().map(|h| {
                let p = h.home();
                (p.name, p.dir)
            }),
            choices,
            program,
        });
    }
}
