//! A reload restarts a host on the installed rush only once that rush has
//! said it can run the session, and puts the session back on the rush
//! before it when the new host doesn't come up.
//!
//! Ported from internal/host/reload.go and reloadwant.go (WantReload
//! :22, ReloadWanted :32, ReloadDone :43, ReloadAgents :47).

use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rush_core::adapter::{Registry, migrated};
use serde_json::json;

use crate::events::log_event;
use crate::wire::Config;

/// The line `rush host preflight` prints when it can.
pub const PREFLIGHT_OK: &str = r#"{"ok":true}"#;

thread_local! {
    /// The rush a restart starts hosts on, for a test: one of its own.
    static INSTALLED: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Makes the rush a restart starts hosts on exe, on this thread: for tests.
pub fn set_installed_exe(exe: Option<PathBuf>) {
    INSTALLED.with(|i| *i.borrow_mut() = exe);
}

/// The rush a restart starts hosts on: the one installed where this one
/// runs from.
pub fn installed_exe() -> std::io::Result<PathBuf> {
    if let Some(e) = INSTALLED.with(|i| i.borrow().clone()) {
        return Ok(e);
    }
    rush_sys::selfexe::path() // waits out an install putting it in place
}

/// Whether this rush can run session id's host: its config reads, its
/// folder is still there (a removed worktree isn't), its agent is one this
/// rush knows and can drive, and the program it was told to run is there.
/// It touches nothing of the session's; it is what `rush host preflight
/// <id>` answers.
pub fn preflight_here(reg: &Registry, id: &str) -> Result<(), String> {
    let cfg = crate::client::read_config(id).map_err(|e| e.to_string())?;
    let kind = migrated(&cfg.kind);
    let Some(a) = reg.get(kind.as_str()) else {
        return Err(format!(
            "rush doesn't know the agent {}",
            go_quote(&cfg.kind)
        ));
    };
    if a.driver().is_none() {
        return Err(format!("rush can't run {}", a.name()));
    }
    if !cfg.cwd.is_empty()
        && let Err(e) = std::fs::metadata(&cfg.cwd)
    {
        return Err(format!(
            "its folder is gone: stat {}: {}",
            cfg.cwd,
            os_err(&e)
        ));
    }
    if !cfg.binary.is_empty() {
        let path = std::env::var("PATH").unwrap_or_default();
        if let Err(e) = rush_gate::cli::look_path(&cfg.binary, &path) {
            return Err(format!("{}'s program: {e}", a.name()));
        }
    }
    Ok(())
}

/// An os error's words as Go says them: "no such file or directory".
pub(crate) fn os_err(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_lowercase(),
        None => s,
    }
}

pub(crate) fn go_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// Asks the rush at exe whether it can run session id's host. A rush from
/// before it could be asked answers with its usage, and is let through.
pub fn preflight(exe: &Path, id: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["host", "preflight", id]);
    let out = match rush_sys::cmd::combined_within(&mut cmd, Duration::from_secs(5)) {
        Err(rush_sys::cmd::Error::Timeout) => return Err("it didn't answer in 5s".into()),
        Err(e) => return Err(e.to_string()),
        Ok(o) => o,
    };
    let text = String::from_utf8_lossy(&out.out).into_owned();
    if out.ok() && text.contains(PREFLIGHT_OK) {
        return Ok(());
    }
    if text.contains("usage: rush host run") {
        return Ok(());
    }
    let last = crate::client::last_line(&text).trim().to_string();
    if out.ok() {
        return Err(format!(
            "it didn't say ok: {}",
            crate::client::last_line(&text)
        ));
    }
    if !last.is_empty() {
        return Err(last);
    }
    Err(out.err_text())
}

/// Where the rush installed before the one at exe is kept (rush update
/// keeps it): the way back.
pub fn prev_exe(exe: &Path) -> PathBuf {
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let mut s = exe.into_os_string();
    s.push(".prev");
    PathBuf::from(s)
}

/// Starts session cfg's host again on the rush installed before exe, whose
/// host didn't come up (failed says how), and logs what that host last
/// wrote. The restart still failed: the session is on the old rush.
pub fn roll_back(cfg: &Config, exe: &Path, failed: &str) -> String {
    let prev = prev_exe(exe);
    let mut f = json!({
        "exe": exe.to_string_lossy(),
        "prev": prev.to_string_lossy(),
        "err": failed,
        "log": log_tail(&cfg.id, 2 << 10),
    });
    if let Err(e) = crate::client::spawn_on(cfg.clone(), Some(&prev)) {
        f["prevErr"] = e.to_string().into();
        log_event(&cfg.id, "reload-failed", f);
        return format!("{failed}, and the rush before it didn't start it either: {e}");
    }
    log_event(&cfg.id, "reload-rolled-back", f);
    format!("{failed}, so it runs on the rush before it again")
}

/// The last n bytes of session id's host.log.
pub fn log_tail(id: &str, n: u64) -> String {
    let Ok(mut f) = std::fs::File::open(crate::paths::dir(id).join("host.log")) else {
        return String::new();
    };
    if let Ok(m) = f.metadata()
        && m.len() > n
    {
        let _ = f.seek(SeekFrom::End(-(n as i64)));
    }
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    String::from_utf8_lossy(&b).into_owned()
}

/// rush reload's second half: every session's host on an older rush
/// that's between turns, restarted on this one. A session mid turn
/// finishes its turn first. One the installed rush can't run stays on the
/// one it has, and is counted failed; one whose folder is gone is counted
/// gone, never tried. It's (stale, restarted, failed, gone)
/// (reloadwant.go:47).
pub fn reload_agents(
    l: &mut crate::client::Lister,
    installed: crate::wire::BinStamp,
) -> (usize, usize, usize, usize) {
    let (stale, restarted, failed, gone) = l.sweep(installed, true);
    if stale > failed {
        let _ = want_reload(installed);
    }
    (stale, restarted, failed, gone)
}

/// The note a reload leaves for the sessions still in a turn:
/// reload-agents in rush's folder, naming the rush it asked for
/// (reloadwant.go:18).
fn reload_note() -> PathBuf {
    rush_state::dir().join("reload-agents")
}

/// Asks for every session's host to be restarted onto the rush stamped
/// installed, each once it is between turns (WantReload).
pub fn want_reload(installed: crate::wire::BinStamp) -> std::io::Result<()> {
    let b = serde_json::to_vec(&installed).map_err(std::io::Error::other)?;
    std::fs::write(reload_note(), b)?;
    std::fs::set_permissions(
        reload_note(),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
}

/// Whether a reload onto the rush stamped installed still has sessions to
/// move: a rush on any other build leaves the note alone (ReloadWanted).
pub fn reload_wanted(installed: crate::wire::BinStamp) -> bool {
    std::fs::read(reload_note())
        .ok()
        .and_then(|b| serde_json::from_slice::<crate::wire::BinStamp>(&b).ok())
        .is_some_and(|w| w.mod_ == installed.mod_ && w.size == installed.size)
}

/// Ends the reload: every session is on the rush it asked for
/// (ReloadDone).
pub fn reload_done() {
    let _ = std::fs::remove_file(reload_note());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    // Go says which file it couldn't open: "open …/config.json: no such
    // file or directory", for host preflight and host run alike.
    #[test]
    fn a_missing_session_names_its_config() {
        let _h = Home::new();
        let want = format!(
            "open {}: no such file or directory",
            crate::paths::dir("nope").join("config.json").display()
        );
        assert_eq!(preflight_here(&Registry::new(), "nope"), Err(want.clone()));
        assert_eq!(
            crate::server::run_with(Registry::new(), crate::server::Hooks::quiet(), "nope"),
            Err(want)
        );
    }

    #[test]
    fn preflight_reads_config_folder_and_agent() {
        let _h = Home::new();
        let reg = Registry::new();
        reg.register(crate::server::tests::Fake::default());
        let id = "pf";
        std::fs::create_dir_all(crate::paths::dir(id)).unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let write =
            |b: String| std::fs::write(crate::paths::dir(id).join("config.json"), b).unwrap();
        write(format!(
            r#"{{"id":"{id}","kind":"fake","cwd":"{}"}}"#,
            cwd.path().display()
        ));
        preflight_here(&reg, id).unwrap();
        write(format!(
            r#"{{"id":"{id}","kind":"fake","cwd":"{}/removed-worktree"}}"#,
            cwd.path().display()
        ));
        let e = preflight_here(&reg, id).unwrap_err();
        assert!(e.contains("folder is gone"), "{e}");
        write(format!(r#"{{"id":"{id}","kind":"nova"}}"#));
        let e = preflight_here(&reg, id).unwrap_err();
        assert!(e.contains("nova"), "{e}");
    }

    // reload_test.go TestSweepCountsAGoneFolderApart: a session whose
    // folder is gone is never restarted: it's counted gone, not stale or
    // failed. (Mid-turn here, and its host this test, so a sweep that got
    // it wrong counts it stale and still touches nothing.)
    #[test]
    fn sweep_counts_a_gone_folder_apart() {
        let _h = Home::new();
        let id = "gone-0000-aaaa";
        let d = crate::paths::dir(id);
        std::fs::create_dir_all(&d).unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let info = crate::wire::Info {
            id: id.into(),
            state: "working".into(),
            host_pid: std::process::id() as i64,
            exe: crate::wire::BinStamp {
                mod_: rush_state::Time::from_unix(1_700_000_000, 0),
                size: 1,
            },
            ..Default::default()
        };
        std::fs::write(d.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();
        let config = |cwd: String| {
            std::fs::write(
                d.join("config.json"),
                format!(r#"{{"id":"{id}","cwd":"{cwd}","owner":1}}"#),
            )
            .unwrap()
        };
        let newer = crate::wire::BinStamp {
            mod_: rush_state::Time::from_unix(1_800_000_000, 0),
            size: 1,
        };
        config(cwd.path().join("removed-worktree").display().to_string());
        let mut l = crate::client::Lister::default();
        assert_eq!(
            l.sweep(newer, true),
            (0, 0, 0, 1),
            "(stale, restarted, failed, gone)"
        );
        config(cwd.path().display().to_string());
        let mut l = crate::client::Lister::default();
        assert_eq!(
            l.sweep(newer, true),
            (1, 0, 0, 0),
            "its folder there: stale"
        );
    }

    // reloadwant.go: the note names the rush it asked for; another build
    // leaves it alone, and done ends it.
    #[test]
    fn the_reload_note_names_its_rush() {
        let _h = Home::new();
        let want = crate::wire::BinStamp {
            mod_: rush_state::Time::from_unix(1_800_000_000, 0),
            size: 7,
        };
        assert!(!reload_wanted(want), "no note");
        want_reload(want).unwrap();
        assert!(reload_wanted(want));
        let other = crate::wire::BinStamp { size: 8, ..want };
        assert!(!reload_wanted(other), "another build");
        reload_done();
        assert!(!reload_wanted(want));
    }

    #[test]
    fn asks_the_installed_rush() {
        let d = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str| {
            let p = d.path().join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let ok = script("ok", &format!("echo '{PREFLIGHT_OK}'\n"));
        preflight(&ok, "x").unwrap();
        let old = script("old", "echo 'usage: rush host run <id>' >&2; exit 1\n");
        preflight(&old, "x").unwrap();
        let no = script("no", "echo 'rush: unknown agent \"nova\"' >&2\nexit 1\n");
        assert_eq!(
            preflight(&no, "x").unwrap_err(),
            "rush: unknown agent \"nova\""
        );
        assert_eq!(
            prev_exe(Path::new("/nowhere/rush")),
            PathBuf::from("/nowhere/rush.prev")
        );
    }
}
