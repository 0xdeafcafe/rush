//! How long each shell command took the last time it ran, by project
//! (Rust-only; Go has none): took.json under rush's folder. The hosts
//! write it as their agents' calls end; the PreToolUse hook reads it, to
//! run a command known to outlast a subagent's prompt cache in the
//! background from the start (rush_core::subkeep::ahead).
//!
//! Only commands that ran a while are kept (a quick one isn't worth a
//! line), and only the newest of them, so the file stays small. Two hosts
//! writing at once may lose one's line: it's a hint, not a record.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Shorter than this, a command isn't written down (nor read as long).
pub const MIN_TOOK: i64 = 30_000;
/// The most commands kept for one project, and projects kept.
const PER_PROJECT: usize = 200;
const PROJECTS: usize = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Ran {
    /// How long it took, in milliseconds.
    took: i64,
    /// When it ended, in Unix milliseconds.
    at: i64,
}

type Book = BTreeMap<String, BTreeMap<String, Ran>>;

/// rush's took.json.
pub fn path() -> PathBuf {
    crate::dir().join("took.json")
}

fn read(p: &Path) -> Book {
    std::fs::read(p)
        .ok()
        .and_then(|b| crate::json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Notes that command took `took` ms in project, ending at `at`. A quick
/// run of one known as long is noted too: it isn't long any more.
pub fn record(p: &Path, project: &str, command: &str, took: i64, at: i64) {
    let command = command.trim();
    if project.is_empty() || command.is_empty() {
        return;
    }
    let mut book = read(p);
    let known = book.get(project).is_some_and(|c| c.contains_key(command));
    if took < MIN_TOOK && !known {
        return;
    }
    let cmds = book.entry(project.to_string()).or_default();
    if took < MIN_TOOK {
        cmds.remove(command);
    } else {
        cmds.insert(command.to_string(), Ran { took, at });
    }
    if cmds.len() > PER_PROJECT {
        let mut by: Vec<(i64, String)> = cmds.iter().map(|(k, r)| (r.at, k.clone())).collect();
        by.sort();
        for (_, k) in by.iter().take(cmds.len() - PER_PROJECT) {
            cmds.remove(k);
        }
    }
    if cmds.is_empty() {
        book.remove(project);
    }
    if book.len() > PROJECTS {
        let newest = |c: &BTreeMap<String, Ran>| c.values().map(|r| r.at).max().unwrap_or(0);
        let mut by: Vec<(i64, String)> = book.iter().map(|(k, c)| (newest(c), k.clone())).collect();
        by.sort();
        for (_, k) in by.iter().take(book.len() - PROJECTS) {
            book.remove(k);
        }
    }
    let _ = crate::write_json(p, &book);
}

/// How long command took the last time it ran in project, when that was
/// long enough to be noted.
pub fn took(p: &Path, project: &str, command: &str) -> Option<i64> {
    read(p).get(project)?.get(command.trim()).map(|r| r.took)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_runs_are_kept_and_quick_ones_forget_them() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("took.json");
        let cases: &[(&str, &str, i64, Option<i64>)] = &[
            ("/w", "cargo test", 5_000, None),
            ("/w", "cargo test", 300_000, Some(300_000)),
            ("/w", " cargo test ", 400_000, Some(400_000)),
            ("/v", "cargo test", 1, None),
            ("/w", "cargo test", 2_000, None),
            ("", "x", 300_000, None),
        ];
        for &(project, cmd, ms, want) in cases {
            record(&p, project, cmd, ms, 1);
            assert_eq!(took(&p, project, cmd), want, "{project} {cmd} {ms}");
        }
    }

    #[test]
    fn only_the_newest_are_kept() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("took.json");
        for i in 0..(PER_PROJECT as i64 + 5) {
            record(&p, "/w", &format!("run {i}"), MIN_TOOK, i);
        }
        assert_eq!(took(&p, "/w", "run 0"), None);
        assert_eq!(took(&p, "/w", "run 204"), Some(MIN_TOOK));
        assert_eq!(read(&p)["/w"].len(), PER_PROJECT);
    }
}
