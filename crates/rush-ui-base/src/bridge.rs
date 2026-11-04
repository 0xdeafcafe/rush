//! rush-state keeps its own list of what each agent answers (its
//! providers, harnesses, keys, program, choices), which its profiles and
//! routes read: Go's state reads internal/agent's registry itself. Until
//! the two are one, the adapters the UI knows (Model::reg) are told to
//! state's list once, before Accounts reads a profile.

use rush_core::adapter::Registry;
use rush_state::agents;

use crate::accounts::accounts;
use crate::model::Model;

/// Tells state's list of agents about m's, once per model and again each
/// time the registry finds the programs: never waiting, it finds none for
/// a moment at the start, and a profile's installed providers (the
/// Prompt's shelf, Go's Profile.Installed reading agent.Runs live) would
/// stay none.
pub fn sync(m: &mut Model) {
    let at = m.reg.found_at();
    if accounts(m).synced == Some(at) {
        return;
    }
    accounts(m).synced = Some(at);
    tell(&m.reg);
}

/// Registers every adapter of reg with state's list.
pub fn tell(reg: &Registry) {
    agents::tell(reg);
}
