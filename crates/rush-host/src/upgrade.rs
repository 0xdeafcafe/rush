//! Moving a host onto a newer rush without stopping its agent. Rust-only:
//! Go's host restarted, and its agent with it.
//!
//! The host, between turns, holds its agent's session between two of its
//! lines (the adapter's [`rush_core::adapter::Upgrader`]), writes what
//! the next build needs to take it up ([`Kept`]: its info, the replay, the
//! agent's files and process) beside the session, keeps its socket and the
//! agent's pipes open across an exec, and execs the installed rush as
//! `rush host adopt <id> <state>`. The process stays: its pid, its agent
//! (its child still, the same process, same pipes, same conversation),
//! and the socket clients reach it on. Its clients' connections close with
//! the exec, and they dial again and are replayed as after any other let
//! go. The new build reads the state, takes the agent up through its
//! adapter's [`rush_core::adapter::Adopter`], and serves on.
//!
//! First it asks the installed rush (`rush host can-upgrade <id>`) whether
//! it can take this session's agent up: a rush from before this, or one
//! whose adapter can't, says no, and the host stays where it is, its agent
//! untouched, and says so (Info.upgrade). It never stops the agent to
//! move.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rush_core::adapter::{Registry, Upgrade};
use serde::{Deserialize, Serialize};

use crate::op::Op;
use crate::paths::dir;
use crate::wire::{BinStamp, Info};

/// The line `rush host can-upgrade` prints when it can.
pub const ADOPT_OK: &str = r#"{"ok":true,"adopts":true}"#;

/// What the host passes to itself across the exec.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// The rush it ran before.
    pub from: BinStamp,
    /// Its socket's listener, by number, open across the exec.
    pub listen: i32,
    /// Its agent, running; None when none was.
    pub agent: Option<Upgrade>,
    pub info: Info,
    pub began: bool,
    pub turn: i64,
    pub untold: String,
    pub start_cwd: String,
    /// The replay, each line as the ring keeps it (packed), base64.
    pub ring: Vec<String>,
    /// The session's slash commands and its context, as last said.
    pub commands: String,
    pub context: String,
}

/// Where a host writes what it passes on: in the session's own folder,
/// readable by its owner only.
pub fn state_path(id: &str) -> PathBuf {
    dir(id).join("upgrade.json")
}

impl Kept {
    /// Writes k where the next build reads it.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let b = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        crate::json::write_private(path, &b)
    }

    /// Reads what the build before passed on, and removes it: it's read
    /// once.
    pub fn take(path: &Path) -> Result<Kept, String> {
        let b = std::fs::read(path)
            .map_err(|e| format!("open {}: {}", path.display(), crate::reload::os_err(&e)))?;
        let _ = std::fs::remove_file(path);
        serde_json::from_slice(&b).map_err(|e| e.to_string())
    }
}

/// A line as base64.
pub fn enc(b: &[u8]) -> String {
    STANDARD.encode(b)
}

/// A base64 line back; empty when it isn't one.
pub fn dec(s: &str) -> Vec<u8> {
    STANDARD.decode(s).unwrap_or_default()
}

/// Whether this rush can take session id's agent up as it runs: what
/// preflight asks, and its agent's adapter takes sessions up.
pub fn can_adopt_here(reg: &Registry, id: &str) -> Result<(), String> {
    crate::reload::preflight_here(reg, id)?;
    let cfg = crate::client::read_config(id).map_err(|e| e.to_string())?;
    let kind = rush_core::adapter::migrated(&cfg.kind);
    match reg.get(kind.as_str()) {
        Some(a) if a.adopter().is_some() => Ok(()),
        Some(a) => Err(format!(
            "{} can't be moved onto another rush while it runs",
            a.name()
        )),
        None => Err("rush doesn't know its agent".into()),
    }
}

/// Whether reg's adapter for kind takes sessions up: a host on it says it
/// upgrades (Info.upgrades).
pub fn adopts(reg: &Registry, kind: &str) -> bool {
    let kind = rush_core::adapter::migrated(kind);
    reg.get(kind.as_str())
        .is_some_and(|a| a.adopter().is_some())
}

/// Asks the rush at exe whether it can take session id's agent up. A rush
/// from before upgrades answers with its usage: no.
pub fn can_adopt(exe: &Path, id: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["host", "can-upgrade", id]);
    let out = match rush_sys::cmd::combined_within(&mut cmd, Duration::from_secs(5)) {
        Err(rush_sys::cmd::Error::Timeout) => return Err("it didn't answer in 5s".into()),
        Err(e) => return Err(e.to_string()),
        Ok(o) => o,
    };
    let text = String::from_utf8_lossy(&out.out).into_owned();
    if out.ok() && text.contains(ADOPT_OK) {
        return Ok(());
    }
    if text.contains("usage: rush host run") {
        return Err("the installed rush can't take a session up as it runs".into());
    }
    let last = crate::client::last_line(&text).trim().to_string();
    let last = last.strip_prefix("rush: ").unwrap_or(&last).to_string();
    if !last.is_empty() {
        return Err(last);
    }
    Err(out.err_text())
}

/// Asks session id's host to move onto the installed rush, once it's
/// between turns: at once when it is.
pub fn ask(id: &str) -> crate::client::Result<()> {
    let c = crate::client::Client::dial(id)?;
    c.send_op(Op::named("upgrade"))?;
    c.close();
    Ok(())
}

/// Whether info's host can be asked to move: alive, awake, on this
/// machine, and of a build that moves in place.
pub fn movable(i: &Info) -> bool {
    i.upgrades && !i.is_remote() && i.state != "stopped" && !i.sleeping
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // What a host passes on reads back as it was written, once.
    #[test]
    fn kept_round_trips_once() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("upgrade.json");
        let mut fds = BTreeMap::new();
        fds.insert("stdin".to_string(), 7);
        fds.insert("stdout".to_string(), 8);
        let k = Kept {
            from: BinStamp {
                mod_: rush_state::Time::from_unix(1_800_000_000, 0),
                size: 42,
            },
            listen: 5,
            agent: Some(Upgrade {
                pid: 1234,
                fds,
                unread: b"{\"type\":\"partial".to_vec(),
                state: serde_json::json!({"sid": "s1", "seq": 9}),
            }),
            info: Info {
                id: "u1".into(),
                state: "idle".into(),
                turn: 3,
                queue: vec!["next".into()],
                ..Info::default()
            },
            began: true,
            turn: 3,
            untold: String::new(),
            start_cwd: "/w".into(),
            ring: vec![enc(b"\x00line one"), enc(b"{\"type\":\"x\"}")],
            commands: enc(b"{\"type\":\"commands\"}"),
            context: String::new(),
        };
        k.write(&p).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let back = Kept::take(&p).unwrap();
        assert_eq!(back, k);
        assert_eq!(dec(&back.ring[0]), b"\x00line one");
        assert!(!p.exists(), "read twice");
        assert!(Kept::take(&p).unwrap_err().contains("no such file"));
    }

    // A rush that can't answer, or answers no, keeps the host where it is.
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
        can_adopt(&script("ok", &format!("echo '{ADOPT_OK}'\n")), "x").unwrap();
        let old = script(
            "old",
            "echo 'rush: usage: rush host run <id>' >&2; exit 1\n",
        );
        assert!(can_adopt(&old, "x").unwrap_err().contains("can't take"));
        let no = script("no", "echo 'rush: Kimi can'\\''t be moved' >&2; exit 1\n");
        assert_eq!(can_adopt(&no, "x").unwrap_err(), "Kimi can't be moved");
    }

    #[test]
    fn only_a_local_awake_upgrading_host_moves() {
        let i = Info {
            upgrades: true,
            state: "working".into(),
            ..Info::default()
        };
        assert!(movable(&i), "mid-turn: asked, it moves once the turn ends");
        assert!(!movable(&Info {
            upgrades: false,
            ..i.clone()
        }));
        assert!(!movable(&Info {
            remote: "mac".into(),
            ..i.clone()
        }));
        assert!(!movable(&Info {
            sleeping: true,
            ..i.clone()
        }));
    }
}
