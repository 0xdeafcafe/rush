//! The bundled plugin that lets rush offer a cleanup agent when the disk
//! runs low or the memory stays under pressure (the screen's machine care
//! does the watching and the offering). It's off until you turn it on;
//! rush recommends it the first time the disk is low.
//!
//! What the agent is told comes from rush's own check of worktrees and
//! /tmp, and from every plugin that provides the cleanup.report service:
//! Haven, go-clean, rust-clean, js-clean and git-clean, and any plugin you
//! install that says it does.
//! Ported from internal/bundled/cleanup/plugin.go:1-53.

use rush_plugin::{Bundle, Manifest, SERVICE_CLEANUP_REPORT};

/// The plugin's.
pub const NAME: &str = "cleanup";

/// The cleanup plugin's.
pub fn manifest() -> Manifest {
    Manifest {
        uses: vec![SERVICE_CLEANUP_REPORT.into()],
        ..crate::manifest(
            NAME,
            "machine",
            "When the disk runs low or the memory stays under pressure, rush asks whether to start a cleanup agent. \
             It reads your worktrees, what agents left in /tmp and what other plugins report (Haven, go-clean, rust-clean, js-clean, git-clean), \
             removes what can go without losing work, and says what it did. Nothing starts until you say yes.",
            16,
        )
    }
}

/// Whether you have it on. It reads the disk.
pub fn on() -> bool {
    rush_plugin::bundled_on(NAME)
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), true, crate::run_switch)
}
