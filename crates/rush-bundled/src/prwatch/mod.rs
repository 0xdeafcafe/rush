//! The bundled plugin that has agents wait on a pull request's checks
//! through rush rather than poll GitHub: rush watch pr (the watch module),
//! the prompt that tells agents of it, and the Bash hook's turning a poll
//! into it. It's off until you turn it on.
//! Ported from internal/bundled/prwatch/plugin.go:1-50, with the prompt
//! from internal/watch/hook.go:10-12.

mod card;
mod live;

pub use card::{JobCard, Seg, Tone, card_for};
pub use live::{Live, Until, live_dir, live_for, save_live};

use rush_plugin::{Bundle, Manifest};

/// The plugin's.
pub const NAME: &str = "watch";

/// What the watch plugin tells agents: wait on a pull request's checks
/// with rush watch pr (watch.Prompt).
pub const PROMPT: &str = "Never wait by polling (sleep loops, re-running gh pr checks or gh run view, gh run watch, scheduled wakeups, a monitor on a polling script): to wait on a pull request's checks, run rush watch pr [<n>|owner/name#<n>|<url>] --until done|fail|first|each [--timeout 25m]; to wait on a GitHub Actions run with no pull request (a pushed branch or commit), run rush watch run [<branch>|<sha>|<run id>] [--workflow ci.yml] with the same flags (nothing: this checkout's commit). It waits for you at no cost and prints once, briefly, what happened: the counts, and each failed check with the log lines that say why. It exits 0 when all passed or it merged, 1 when a check failed, 2 when it timed out (run it again), 3 when closed, cancelled or GitHub couldn't be asked. Run it as a background command where you can, so it wakes you when it ends; --until each prints a line as each check finishes, for a monitor. rush watch help says more.";

/// The watch plugin's.
pub fn manifest() -> Manifest {
    Manifest {
        prompt: PROMPT.into(),
        ..crate::manifest(
            NAME,
            "pull-requests",
            "Waits on pull requests' checks and GitHub Actions runs for your agents, so they never poll GitHub: \
             rush watch pr and rush watch run ask for them, cheaply and shared, and say once what happened. Agents \
             are told to use them, gh pr checks --watch and gh run watch are turned into them, and a sleep before \
             asking GitHub is turned back.",
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
