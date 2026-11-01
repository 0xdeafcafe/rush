//! Where the push-to-talk helper lives, building it and starting it. New in
//! the Rust port (no Go original); built the way the menu bar app is
//! (crates/rush-menubar/src/bundle.rs).
//!
//! The helper is rush itself: the bundle's program is a copy of this rush
//! binary, which runs the helper when it finds itself inside the bundle
//! ([`crate::bundled`]), so building it needs no compiler.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{BUNDLE_ID, EXECUTABLE};

/// The helper's icon: the menu bar app's, on a full square, as macOS 26
/// draws its own tile (and the helper needs 26).
const ICON: &[u8] = include_bytes!("../../rush-menubar/icons/AppIcon-full.icns");

/// Bumped when the bundle's makings change, so an old one is rebuilt.
const MAKINGS: &str = "rush-voice-1";

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// The helper's folder in rush's.
pub fn dir() -> PathBuf {
    rush_state::dir().join("voice")
}

/// Where the built helper lives.
pub fn app_path() -> PathBuf {
    dir().join("rush voice.app")
}

/// The program inside the built helper.
pub fn program() -> PathBuf {
    app_path().join("Contents").join("MacOS").join(EXECUTABLE)
}

/// The rush binary the helper is a copy of: this one, links followed.
fn exe() -> PathBuf {
    rush_sys::selfexe::resolved().unwrap_or_else(|| PathBuf::from("rush"))
}

pub(crate) fn make_dir(d: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(d)
}

/// What the stamp beside the helper is made from: the rush it's a copy of
/// (by path, size and modification time) and the bundle's makings.
pub fn stamp(bin: &Path) -> String {
    let (len, modified) = fs::metadata(bin)
        .map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            (m.len(), t)
        })
        .unwrap_or((0, 0));
    format!("{MAKINGS}\n{}\n{len}\n{modified}\n", bin.display())
}

/// Makes the helper's bundle, unless the one there was built from this
/// rush. Returns whether it built one.
pub fn build_app() -> Result<bool, String> {
    let app = app_path();
    let bin = exe();
    let stamp = stamp(&bin);
    // Beside it: inside would break its signature.
    let stamp_path = PathBuf::from(format!("{}.stamp", app.display()));
    if app.exists() && fs::read_to_string(&stamp_path).is_ok_and(|s| s == stamp) {
        return Ok(false);
    }
    let _ = make_dir(&dir());
    let _ = fs::remove_dir_all(&app);
    for d in ["MacOS", "Resources"] {
        fs::create_dir_all(app.join("Contents").join(d)).map_err(|e| e.to_string())?;
    }
    // A copy, never a link: signing it changes the file.
    fs::copy(&bin, program()).map_err(|e| format!("building rush voice: {e}"))?;
    fs::write(app.join("Contents/Resources/AppIcon.icns"), ICON).map_err(|e| e.to_string())?;
    let version = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    fs::write(app.join("Contents/Info.plist"), plist(version)).map_err(|e| e.to_string())?;
    // macOS keeps the microphone and speech permissions for a signed app;
    // an ad-hoc signature is enough on the Mac that built it.
    let out = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app)
        .output()
        .map_err(|e| format!("signing rush voice: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "signing rush voice: {}\n{}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let _ = Command::new(LSREGISTER).arg("-f").arg(&app).status();
    fs::write(&stamp_path, stamp).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Opens the helper in the background, without taking focus.
pub(crate) fn launch() -> Result<(), String> {
    let st = Command::new("/usr/bin/open")
        .args(["-g", "-j", "-a"])
        .arg(app_path())
        .status()
        .map_err(|e| format!("starting rush voice: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("starting rush voice: open said {st}"))
    }
}

/// Quits a running helper: after a rebuild, so the new one takes over.
pub(crate) fn quit() {
    let _ = Command::new("/usr/bin/pkill")
        .arg("-f")
        .arg(quote_meta(&program().to_string_lossy()))
        .status();
}

/// Unregisters the helper at path with Launch Services: for tests, whose
/// helper mustn't stand in for the real one.
pub fn unregister(path: &Path) {
    let _ = Command::new(LSREGISTER).arg("-u").arg(path).status();
}

/// The major version of a "26.1" product version.
pub fn major(v: &str) -> i32 {
    v.trim()
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0)
}

/// s matched as it is, in pkill's pattern.
fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The helper's Info.plist: in the background, macOS 26 on, and the words
/// macOS shows when it asks for the microphone and speech recognition.
pub fn plist(version: u64) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
	<key>CFBundleName</key><string>rush voice</string>
	<key>CFBundleDisplayName</key><string>rush voice</string>
	<key>CFBundleExecutable</key><string>{EXECUTABLE}</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>1.0</string>
	<key>CFBundleVersion</key><string>{version}</string>
	<key>LSMinimumSystemVersion</key><string>26.0</string>
	<key>LSUIElement</key><true/>
	<key>NSMicrophoneUsageDescription</key><string>rush listens while you hold its push-to-talk key, and only then.</string>
	<key>NSSpeechRecognitionUsageDescription</key><string>rush turns what you say into text on this Mac with Apple's on-device speech model; nothing leaves your Mac.</string>
</dict>
</plist>
"#
    )
}
