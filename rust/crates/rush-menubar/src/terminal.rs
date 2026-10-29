//! Clicking a notification, or an agent in the menu, comes back to rush in
//! the terminal it's open in (Warp, iTerm, Ghostty…), on that agent: each
//! rush leaves its terminal's bundle ID under open/<pid> while it runs, and
//! picks up an agent to go to from open/<pid>.goto. Port of
//! internal/menubar/terminal.go:1-115.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::bundle::{BUNDLE_ID, dir};

fn open_dir() -> PathBuf {
    dir().join("open")
}

fn last_terminal() -> PathBuf {
    dir().join("terminal")
}

fn write_private(p: &Path, b: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(p)?
        .write_all(b)
}

/// Marks this rush as open in its terminal until it's dropped.
#[must_use = "rush is marked open only while this lives"]
pub struct Here(Option<PathBuf>);

impl Drop for Here {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(&p);
            let _ = fs::remove_file(goto_of(&p));
        }
    }
}

fn goto_of(p: &Path) -> PathBuf {
    PathBuf::from(format!("{}.goto", p.display()))
}

/// Marks this rush as open in its terminal (terminal.go:22 Here).
pub fn here() -> Here {
    here_in(&terminal_id())
}

/// Marks this rush as open in the terminal of bundle ID id.
fn here_in(id: &str) -> Here {
    if !cfg!(target_os = "macos") || id.is_empty() {
        return Here(None);
    }
    let p = open_dir().join(std::process::id().to_string());
    let made = fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(open_dir());
    if made.is_err() || write_private(&p, id.as_bytes()).is_err() {
        return Here(None);
    }
    let _ = write_private(&last_terminal(), id.as_bytes());
    Here(Some(p))
}

/// The agent the menu bar app last asked this rush to go to, once.
pub fn goto() -> String {
    let p = open_dir().join(format!("{}.goto", std::process::id()));
    let Ok(b) = fs::read_to_string(&p) else {
        return String::new();
    };
    let _ = fs::remove_file(&p);
    b.trim().to_string()
}

/// The bundle ID of the app rush's terminal is: macOS hands it to
/// everything an app starts, tmux included; TERM_PROGRAM names the usual
/// ones when it didn't.
pub fn terminal_id() -> String {
    terminal_id_of(|k| std::env::var(k).unwrap_or_default())
}

fn terminal_id_of(env: impl Fn(&str) -> String) -> String {
    let id = env("__CFBundleIdentifier");
    if !id.is_empty() && id != BUNDLE_ID {
        return id;
    }
    match env("TERM_PROGRAM").as_str() {
        "Apple_Terminal" => "com.apple.Terminal",
        "iTerm.app" => "com.googlecode.iterm2",
        "WarpTerminal" => "dev.warp.Warp-Stable",
        "ghostty" => "com.mitchellh.ghostty",
        "WezTerm" => "com.github.wez.wezterm",
        "vscode" => "com.microsoft.VSCode",
        _ => "",
    }
    .to_string()
}

/// Whether pid has gone: no such process.
fn gone(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether pid exists.
    let r = unsafe { libc::kill(pid, 0) };
    r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// The rush open most recently, and the terminal it's in; (0, "") with
/// none. Those gone are forgotten.
pub fn open() -> (i32, String) {
    let Ok(ents) = fs::read_dir(open_dir()) else {
        return (0, String::new());
    };
    let (mut pid, mut id, mut at) = (0, String::new(), SystemTime::UNIX_EPOCH);
    for e in ents.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        let p = e.path();
        if gone(n) {
            let _ = fs::remove_file(&p);
            let _ = fs::remove_file(goto_of(&p));
            continue;
        }
        let modified = e.metadata().and_then(|m| m.modified());
        let b = fs::read_to_string(&p).unwrap_or_default();
        if let Ok(m) = modified
            && !b.is_empty()
            && m > at
        {
            (pid, id, at) = (n, b, m);
        }
    }
    (pid, id)
}

/// Brings rush forward on the agent key (if any): the terminal it's open
/// in, or, with none open, a new rush in the terminal it last was.
pub fn show(key: &str) -> Result<(), String> {
    let (pid, id) = open();
    if pid != 0 {
        if !key.is_empty() {
            let _ = write_private(&open_dir().join(format!("{pid}.goto")), key.as_bytes());
        }
        return open_with(&["-b", &id]);
    }
    let bin = crate::exe().to_string_lossy().into_owned();
    let last = fs::read_to_string(last_terminal()).unwrap_or_default();
    if runs_files(&last) && open_with(&["-b", &last, &bin]).is_ok() {
        return Ok(());
    }
    open_with(&[&bin])
}

fn open_with(args: &[&str]) -> Result<(), String> {
    let st = Command::new("/usr/bin/open")
        .args(args)
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err(st.to_string())
    }
}

/// Whether the terminal runs a program it's asked to open; the others
/// don't take one this way, so a new rush opens in Terminal.
pub fn runs_files(id: &str) -> bool {
    id == "com.apple.Terminal" || id == "com.googlecode.iterm2" || id.starts_with("dev.warp.Warp")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rush open in Warp is the one the menu bar comes back to, on the
    /// agent asked for; once it's closed, Warp is remembered
    /// (terminal_test.go TestHere).
    #[test]
    fn here() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().to_path_buf()));
        let gone = open_dir().join("999999"); // a pid that isn't running
        let release = here_in("dev.warp.Warp-Stable");
        fs::write(&gone, "com.apple.Terminal").unwrap();
        let me = std::process::id() as i32;
        assert_eq!(open(), (me, "dev.warp.Warp-Stable".to_string()));
        assert!(!gone.exists(), "a rush that's gone should be forgotten");
        fs::write(open_dir().join(format!("{me}.goto")), "acct/abc").unwrap();
        assert_eq!(goto(), "acct/abc");
        assert_eq!(goto(), "", "only once");
        drop(release);
        assert_eq!(open().0, 0, "closed, it isn't open");
        let last = fs::read_to_string(last_terminal()).unwrap();
        assert_eq!(last, "dev.warp.Warp-Stable");
        assert!(runs_files(&last));
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn terminal_from_the_environment() {
        type Env = &'static [(&'static str, &'static str)];
        let cases: [(Env, &str); 4] = [
            (
                &[("__CFBundleIdentifier", "dev.warp.Warp-Stable")],
                "dev.warp.Warp-Stable",
            ),
            (
                &[
                    ("__CFBundleIdentifier", BUNDLE_ID),
                    ("TERM_PROGRAM", "ghostty"),
                ],
                "com.mitchellh.ghostty",
            ),
            (&[("TERM_PROGRAM", "iTerm.app")], "com.googlecode.iterm2"),
            (&[("TERM_PROGRAM", "tmux")], ""),
        ];
        for (pairs, want) in cases {
            let env = |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map_or(String::new(), |(_, v)| v.to_string())
            };
            assert_eq!(terminal_id_of(env), want, "{pairs:?}");
        }
    }

    #[test]
    fn terminals_that_run_files() {
        for (id, want) in [
            ("com.apple.Terminal", true),
            ("com.googlecode.iterm2", true),
            ("dev.warp.Warp-Preview", true),
            ("com.mitchellh.ghostty", false),
            ("", false),
        ] {
            assert_eq!(runs_files(id), want, "{id}");
        }
    }
}
