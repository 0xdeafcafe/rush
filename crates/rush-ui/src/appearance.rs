//! The system going light or dark, followed so a terminal that switches
//! its theme with it is asked its colours again and rush's are made anew
//! (rush_draw::ask_colours_again). Rust-only: Go asked once, as it
//! started.
//!
//! macOS tells programs through a distributed notification
//! (AppleInterfaceThemeChangedNotification), but only on the main
//! thread's run loop, and rush's main thread draws. The choice is saved
//! in the global preferences file as it changes, auto at sunrise and
//! sunset too, so that file is watched (kqueue, no timer) and the choice
//! read again when it's written. The terminal switches its own colours a
//! moment after the system, so it's asked twice: soon, and once more a
//! little later.

#[cfg(target_os = "macos")]
use std::time::Duration;

/// When the terminal is asked after the system switched.
#[cfg(target_os = "macos")]
const ASK_AFTER: [Duration; 2] = [Duration::from_millis(300), Duration::from_millis(1700)];

/// Starts the thread: redraw is told each time the colours are to be
/// asked again, to have a frame ask them.
#[cfg(target_os = "macos")]
pub(crate) fn follow(redraw: impl Fn() + Send + 'static) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let prefs = std::path::Path::new(&home).join("Library/Preferences/.GlobalPreferences.plist");
    let _ = std::thread::Builder::new()
        .name("rush-appearance".into())
        .spawn(move || {
            let w = rush_sys::fswait::Watcher::new();
            // Watched before it's read: a change from then on wakes.
            w.watch(&[&prefs]);
            let mut was = dark();
            loop {
                if !w.wait(None, None) {
                    return;
                }
                // Replaced as it's saved: watched again, what's there now.
                let _ = w.changes();
                w.watch(&[&prefs]);
                let now = dark();
                if now == was {
                    continue;
                }
                was = now;
                for after in ASK_AFTER {
                    std::thread::sleep(after);
                    rush_draw::ask_colours_again();
                    redraw();
                }
            }
        });
}

/// Elsewhere the window coming back into focus asks again (rush-draw).
#[cfg(not(target_os = "macos"))]
pub(crate) fn follow(_redraw: impl Fn() + Send + 'static) {}

/// Whether the system is dark: AppleInterfaceStyle is "Dark", and gone
/// when it's light.
#[cfg(target_os = "macos")]
fn dark() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && is_dark(&o.stdout))
}

#[cfg(any(target_os = "macos", test))]
fn is_dark(out: &[u8]) -> bool {
    out.trim_ascii().eq_ignore_ascii_case(b"dark")
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_systems_choice() {
        for (out, want) in [(&b"Dark\n"[..], true), (b"Light\n", false), (b"", false)] {
            assert_eq!(super::is_dark(out), want, "{out:?}");
        }
    }
}
