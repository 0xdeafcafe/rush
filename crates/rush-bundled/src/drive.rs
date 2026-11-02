//! The bundled plugin that has rush drive pull requests for agents: rush pr
//! follow, reply and resolve (the pr drive module), and the prompt that
//! tells agents of them. It's off until you turn it on; off, rush pr
//! refuses and the followers already running end.
//! Ported from internal/bundled/drive/plugin.go:1-50, with the prompt from
//! internal/prdrive/playbook.go:3-4.

use rush_plugin::{Bundle, Manifest};

/// The plugin's.
pub const NAME: &str = "drive";

/// What every agent rush runs is told of driving a pull request
/// (prdrive.Prompt).
pub const PROMPT: &str = "To drive a pull request (get it green, answer its review, \"drive this PR\"), run rush pr follow <pr>... and work the TO DO it prints, all of it, before one push; then end your turn. rush follows the PR and sends you a message only with news you can act on (a failed check with its log lines, a new trusted review finding, a conflict, ready for approval), so never wait on checks or poll. Reply on review threads only with rush pr reply (it refuses a second word or an argument) and resolve with rush pr resolve. Stop and tell the user at READY FOR APPROVAL: review bots never approve, and a push clears approvals. rush pr help has the rest.";

/// The drive plugin's.
pub fn manifest() -> Manifest {
    Manifest {
        prompt: PROMPT.into(),
        ..crate::manifest(
            NAME,
            "pull-requests",
            "Drives pull requests for your agents: rush pr follow says what's left before merge, in the order to \
             do it, then follows the PR and wakes the agent only with news it can act on (a failed check with its log, a \
             trusted review finding, a conflict, ready for approval). Agents are told to use it.",
            16,
        )
    }
}

/// Whether you have it on.
pub fn on() -> bool {
    rush_plugin::bundled_on(NAME)
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), true, crate::run_switch)
}
