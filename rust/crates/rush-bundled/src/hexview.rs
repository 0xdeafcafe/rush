//! The bundled plugin that lets a step's output be seen as its bytes: v on
//! a step cycles to a hex viewer. The viewer is rush's own (the
//! conversation module); the plugin is the switch for it.
//! Ported from internal/bundled/hexview/plugin.go:1-40.

use rush_plugin::{Bundle, Manifest};

/// The hex plugin's.
pub fn manifest() -> Manifest {
    crate::manifest(
        "hex",
        "views",
        "Adds a hex view to what agents' steps print and read: v on a step cycles to its bytes, \
         coloured by kind, with an ASCII column. Binary output opens in it.",
        16,
    )
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), true, crate::run_switch)
}
