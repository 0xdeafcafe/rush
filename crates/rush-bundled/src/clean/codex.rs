//! codex-clean: Codex's session transcripts no one has written to in a
//! while, under $CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl. Codex
//! never deletes them itself, so they grow for as long as it's used.
//! New in the port: the Go had no codex-clean.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime};

use rush_plugin::{Ctx, Manifest, SettingSpec};

use super::{Clean, Kit, Session, Thing, bytes, plural};

static CODEX: LazyLock<Kit> = LazyLock::new(|| Kit {
    name: "codex-clean",
    about: "Codex's session transcripts, which it keeps forever: those not written to in a while, \
            how much they take, and deleting them when you pick it. Off until you turn it on.",
    needs: &[],
    procs: &[],
    things: codex_things,
    settings: codex_settings(),
});

pub(crate) fn codex_kit() -> &'static Kit {
    &CODEX
}

/// How long a session sits unwritten before it's offered for deletion.
fn codex_settings() -> Vec<SettingSpec> {
    vec![SettingSpec {
        key: "session-age-days".into(),
        title: "Sessions older than (days)".into(),
        kind: "choice".into(),
        choices: ["7", "14", "30", "60", "90", "180"]
            .map(String::from)
            .to_vec(),
        default: "30".into(),
        description:
            "Days since a Codex session was last written to before it's offered for deletion".into(),
    }]
}

/// The age setting in days, read afresh each time it's asked.
fn age_days() -> u64 {
    let m = Manifest {
        name: "codex-clean".into(),
        settings: codex_settings(),
        ..Default::default()
    };
    let v = rush_plugin::setting_values_of(&m)
        .remove("session-age-days")
        .unwrap_or_default();
    match v.parse::<u64>() {
        Ok(n) if n >= 1 => n,
        _ => 30,
    }
}

/// CODEX_HOME, or ~/.codex, as rush-codex finds it.
fn codex_home() -> Option<PathBuf> {
    let env = |k| std::env::var(k).ok().filter(|v: &String| !v.is_empty());
    env("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| Path::new(&h).join(".codex")))
}

fn codex_things(_: &Ctx, _: &[Session]) -> Vec<Thing> {
    match codex_home() {
        Some(home) => codex_things_in(&home, age_days(), SystemTime::now()),
        None => Vec::new(),
    }
}

/// The one row for the sessions under home not written to in days.
pub(super) fn codex_things_in(home: &Path, days: u64, now: SystemTime) -> Vec<Thing> {
    let dir = home.join("sessions");
    let old = old_sessions(&dir, cutoff(now, days));
    if old.is_empty() {
        return Vec::new();
    }
    let size: i64 = old.iter().map(|(_, n)| *n).sum();
    vec![
        Thing::new(
            "codex-sessions",
            format!("Codex sessions older than {days} days"),
            "deletes them: they leave rush's history, and codex resume can't open them",
            deleted(dir, days),
        )
        .meta(format!(
            "{} · {}",
            bytes(size),
            plural(old.len(), "session", "sessions")
        )),
    ]
}

fn cutoff(now: SystemTime, days: u64) -> SystemTime {
    now.checked_sub(Duration::from_secs(days * 24 * 3600))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// The rollout files under dir last written before cutoff, with their
/// sizes. A session still going is written to, so it's never among them.
fn old_sessions(dir: &Path, cutoff: SystemTime) -> Vec<(PathBuf, i64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            let name = e.file_name();
            let name = name.to_string_lossy();
            if !ft.is_file() || !name.starts_with("rollout-") || !name.contains(".jsonl") {
                continue;
            }
            if let Ok(m) = e.metadata()
                && m.modified().is_ok_and(|t| t < cutoff)
            {
                out.push((p, m.len() as i64));
            }
        }
    }
    out.sort();
    out
}

/// Deletes the sessions under dir that are old when it runs, not when
/// they were listed, then the day, month and year folders left empty.
fn deleted(dir: PathBuf, days: u64) -> Clean {
    Arc::new(move |_| {
        let old = old_sessions(&dir, cutoff(SystemTime::now(), days));
        let (mut n, mut freed, mut failed) = (0, 0i64, 0);
        for (p, size) in &old {
            match std::fs::remove_file(p) {
                Ok(()) => {
                    n += 1;
                    freed += size;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => failed += 1,
            }
        }
        prune_empty(&dir);
        let mut said = format!(
            "Deleted {}, freed {}.",
            plural(n, "session", "sessions"),
            bytes(freed)
        );
        if failed > 0 {
            said.push_str(&format!(" {failed} couldn't be deleted."));
        }
        Ok(said)
    })
}

/// Removes the folders under dir left empty, deepest first; dir stays.
fn prune_empty(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            let p = e.path();
            prune_empty(&p);
            let _ = std::fs::remove_dir(&p); // fails, as it should, when not empty
        }
    }
}
