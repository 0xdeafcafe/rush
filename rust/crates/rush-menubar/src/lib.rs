//! rush's menu bar icon: a small native app that shows every account's
//! usage, what's working and who needs you, and answers questions from
//! their notification. Port of internal/menubar (feed.go:1-7) and the
//! `rush menubar` command (cmd/rush/main.go:514-540 menuBar).
//!
//! Everything the app shows comes from `rush menubar feed`, which it runs
//! and talks to over stdin and stdout, one JSON object per line ([`wire`]).
//! Go's app is Swift, built on this Mac; this one is rush itself, drawn
//! with AppKit through objc2 ([`app`]): the bundle's program is a copy of
//! the rush binary, which runs the app when it finds itself inside the
//! bundle. The binary's main calls [`bundled`] first and, when it says
//! so, [`app_main`]; `rush menubar …` is [`cli`].

pub mod bundle;
pub mod feed;
pub mod host;
pub mod menu;
pub mod source;
pub mod terminal;
pub mod wire;

#[cfg(target_os = "macos")]
pub mod app;

use std::path::PathBuf;

pub use bundle::{BUNDLE_ID, app_path, build_app, forget, running, start, stop};
pub use terminal::{Here, goto, here, show};

/// The rush binary the app should run: this one, links followed.
pub fn exe() -> PathBuf {
    let Ok(p) = std::env::current_exe() else {
        return PathBuf::from("rush");
    };
    std::fs::canonicalize(&p).unwrap_or(p)
}

/// Whether this process is the menu bar app: the bundle's program, started
/// from inside it.
pub fn bundled() -> bool {
    std::env::current_exe().is_ok_and(|p| is_bundle_program(&p))
}

fn is_bundle_program(p: &std::path::Path) -> bool {
    p.file_name().is_some_and(|n| n == bundle::EXECUTABLE)
        && p.parent()
            .is_some_and(|d| d.ends_with(std::path::Path::new("Contents/MacOS")))
}

/// Runs the menu bar app until it quits; on the main thread, as the
/// bundle's program.
pub fn app_main() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        app::run();
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    Err("the menu bar icon is macOS only".into())
}

/// `rush menubar [off|feed]`: runs the menu bar icon, its feed (which the
/// icon runs), or takes it away. What it prints goes to stdout.
pub fn cli(args: &[String]) -> Result<(), String> {
    let mut st = rush_state::Store::load();
    match args.first().map(String::as_str) {
        Some("feed") => {
            let src = source::FleetSource::new(rush_core::adapter::global().clone());
            let stdin = std::io::BufReader::new(std::io::stdin());
            feed::feed(
                stdin,
                &mut std::io::stdout(),
                Box::new(src),
                std::sync::Arc::new(feed::Local),
            )
            .map_err(|e| e.to_string())
        }
        Some("app") => app_main(),
        Some("off") => {
            st.config.menu_bar = false;
            st.config.menu_bar_asked = true;
            stop();
            forget();
            println!("off — the menu bar icon is gone");
            st.save_config().map_err(|e| e.to_string())
        }
        Some(_) => Err("usage: rush menubar [off]".into()),
        None => {
            println!("starting the menu bar icon (the first time builds it, a few seconds)…");
            start()?;
            st.config.menu_bar = true;
            st.config.menu_bar_asked = true;
            println!("on — rush is in your menu bar. Its menu can open it at login.");
            st.save_config().map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_its_bundle() {
        for (p, want) in [
            ("/x/menubar/rush.app/Contents/MacOS/rush-menubar", true),
            ("/usr/local/bin/rush", false),
            ("/x/rush-menubar", false),
        ] {
            assert_eq!(is_bundle_program(std::path::Path::new(p)), want, "{p}");
        }
    }

    #[test]
    fn usage_for_anything_else() {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().to_path_buf()));
        assert_eq!(
            cli(&["on".to_string()]),
            Err("usage: rush menubar [off]".to_string())
        );
        rush_state::set_thread_dir(None);
    }
}
