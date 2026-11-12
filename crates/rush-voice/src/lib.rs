//! rush's own push-to-talk, for when yap isn't there (no Go original: new
//! in the Rust port).
//!
//! macOS asks for the microphone and speech recognition per app, and a
//! terminal can't be granted speech recognition, so the listening happens
//! in a helper app of rush's own: this rush binary, copied into
//! `rush voice.app` (bundle id [`BUNDLE_ID`]) with the usage strings in its
//! Info.plist, ad-hoc signed, started with `open` so macOS asks on its
//! behalf. Like the menu bar app, the binary runs the helper when it finds
//! itself inside that bundle ([`bundled`], [`helper_main`]).
//!
//! The helper listens on a socket in rush's folder ([`socket`]). A window
//! connects, sends one JSON line per command and reads one JSON line per
//! change, the same shape yap says on its own socket ([`Live`]).
//!
//! The window side is [`start`]: it builds and starts the helper if need
//! be, begins a dictation on the chosen microphone, and hands out each
//! change until the dictation ends.

use std::path::PathBuf;

pub mod bundle;
mod client;
mod words;

#[cfg(target_os = "macos")]
mod audio;
#[cfg(target_os = "macos")]
mod helper;

pub use bundle::{app_path, build_app};
pub use words::{level, split};

/// The helper's identity: macOS keeps its microphone and speech
/// permissions under it.
pub const BUNDLE_ID: &str = "dev.rush.voice";

/// The bundle's program's name.
pub const EXECUTABLE: &str = "rush-voice";

/// One microphone the Mac has.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Device {
    /// CoreAudio's unique id for it, which stays the same across restarts:
    /// what the setting keeps.
    pub id: String,
    /// What macOS calls it ("MacBook Pro Microphone").
    pub name: String,
    /// Whether it's the Mac's input right now.
    pub default: bool,
}

/// Where a dictation is: the same shape yap says on its socket, so the
/// window draws both alike.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Live {
    /// listening, finishing (let go, tidying), done (the text is in
    /// settled and nothing more comes), failed (why is in error) or hidden
    /// (cancelled).
    pub phase: String,
    /// Words it's sure of.
    pub settled: String,
    /// Words it may still change.
    pub guessing: String,
    /// How loud, 0 to 100.
    pub level: i64,
    /// Why it failed, in words for the user ("rush voice can't use the
    /// microphone: allow it in System Settings › Privacy & Security ›
    /// Microphone").
    pub error: String,
}

/// A command the window sends the helper, one JSON line each.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Command {
    /// Begin listening on the device with this id; empty is the Mac's
    /// input. context is the words on the window's screen (names, repos,
    /// branches, files, identifiers) the speech model should expect, so
    /// they come out spelled as they're written; left out when empty, and
    /// a line without it still reads.
    Start {
        device: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        context: Vec<String>,
    },
    /// Stop listening and finish: the last change is done.
    Stop,
    /// Stop listening and drop it: the last change is hidden.
    Cancel,
}

/// How many of a dictation's context words the speech model is given:
/// Apple suggests no more than a hundred.
pub const CONTEXT_MAX: usize = 100;

/// A dictation under way, from [`start`]. Dropping it cancels it.
pub struct Dictation {
    inner: client::Conn,
}

impl Dictation {
    /// Let go: stop listening, finish the words, and send a last change
    /// whose phase is done.
    pub fn stop(&self) {
        self.inner.send(&Command::Stop);
    }

    /// Drop it: stop listening; the last change's phase is hidden.
    pub fn cancel(&self) {
        self.inner.send(&Command::Cancel);
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        if !self.inner.finished() {
            self.cancel();
        }
        self.inner.close();
    }
}

impl Live {
    /// Whether nothing more comes after this change.
    pub fn is_end(&self) -> bool {
        matches!(self.phase.as_str(), "done" | "failed" | "hidden")
    }
}

/// Begins a dictation on the microphone with this id (empty: the Mac's
/// input), expecting the words in context (the first [`CONTEXT_MAX`],
/// most wanted first), building and starting the helper first if it
/// isn't running.
/// Hands each change to out, on a thread of its own, until one whose
/// phase is done, failed or hidden. Returns at once; a helper that can't
/// be started is an Err, one that fails later is a failed change.
pub fn start(
    device: &str,
    context: Vec<String>,
    out: impl FnMut(Live) + Send + 'static,
) -> Result<Dictation, String> {
    client::start(device, context, out).map(|inner| Dictation { inner })
}

/// The Mac's microphones, the input first. Asked in this process (listing
/// them needs no permission).
#[cfg(target_os = "macos")]
pub fn devices() -> Vec<Device> {
    audio::devices()
}

/// The Mac's microphones: none, off macOS.
#[cfg(not(target_os = "macos"))]
pub fn devices() -> Vec<Device> {
    Vec::new()
}

/// Whether push-to-talk can work here at all: macOS 26 or later, where the
/// on-device speech model is.
pub fn available() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        // sw_vers, not NSProcessInfo: a binary built against an older
        // SDK is told an older version than the Mac runs.
        let out = std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        bundle::major(&out) >= 26
    })
}

/// The helper's socket, in rush's folder.
pub fn socket() -> PathBuf {
    rush_state::dir().join("voice").join("live.sock")
}

/// Whether this process is the helper: the binary inside its bundle.
pub fn bundled() -> bool {
    std::env::current_exe().is_ok_and(|p| is_bundle_program(&p))
}

/// Whether a program at p is the helper's: [`EXECUTABLE`] in a bundle's
/// Contents/MacOS.
pub fn is_bundle_program(p: &std::path::Path) -> bool {
    p.file_name().is_some_and(|n| n == EXECUTABLE)
        && p.parent()
            .is_some_and(|d| d.ends_with(std::path::Path::new("Contents/MacOS")))
}

/// Runs the helper until it quits; on the main thread, as the bundle's
/// program.
#[cfg(target_os = "macos")]
pub fn helper_main() -> Result<(), String> {
    helper::run()
}

/// The helper: push-to-talk runs only on macOS.
#[cfg(not(target_os = "macos"))]
pub fn helper_main() -> Result<(), String> {
    Err("push-to-talk runs only on macOS".into())
}
