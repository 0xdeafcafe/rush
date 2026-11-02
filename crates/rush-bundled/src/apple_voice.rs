//! The bundled plugin that is rush's own push-to-talk: hold ctrl+space
//! and speak, let go and the words go into the message box. rush hears
//! the microphone you pick with Apple's on-device speech model, through
//! its helper app (rush_voice); the window does the work, the plugin is
//! the switch. No Go original: new in the Rust port, built like yap's.

use rush_plugin::{Bundle, Manifest};

/// The plugin's name, which its switch is kept under.
pub const NAME: &str = "apple-voice";

/// The apple voice plugin's.
pub fn manifest() -> Manifest {
    crate::manifest(
        NAME,
        "voice",
        "Push to talk: hold ctrl+space and speak, let go and the words go into the message box. \
         Hears the microphone you pick with Apple's on-device speech model; nothing leaves your Mac. \
         Needs macOS 26.",
        16,
    )
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_inside(manifest(), false, crate::start_switch)
}
