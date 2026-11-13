//! Each worktree in one of four groups, by facts alone, with its reason:
//! what Clean up shows and the cleanup agent is told, so none is "stale
//! but kept" without saying why. Rust-only: Go had no such groups.
//!
//! The facts: its uncommitted files, noise told from work (a lone
//! .claude/settings.local.json, a lockfile); its pull request as GitHub
//! says (a squash merge is merged: Worktree::merged_pr); its commits on no
//! remote and in no merged pull request; when anything outside its
//! node_modules last changed; whether a process works in it (sweep::
//! WtFacts).
//!
//! - KEEP: someone works in it, git can't say, it's locked, or it holds
//!   work and was touched within WT_IDLE.
//! - CACHE-ONLY: it holds work (files, commits, an open pull request), so
//!   it stays, but nobody touched it for WT_IDLE: its build caches go.
//! - NOISE-ONLY: its work is in; all that's uncommitted is noise.
//! - SAFE: its work is in, nothing uncommitted: it goes whole.

use rush_core::time::Ms;

use crate::sweep::{WT_IDLE, WtFacts, span};
use crate::worktree::Worktree;

/// A worktree's group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Safe,
    CacheOnly,
    NoiseOnly,
    Keep,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Safe => "SAFE",
            Class::CacheOnly => "CACHE-ONLY",
            Class::NoiseOnly => "NOISE-ONLY",
            Class::Keep => "KEEP",
        }
    }
}

/// A worktree's group, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Triage {
    pub class: Class,
    pub reason: String,
}

/// Lockfiles: a reinstall rewrites them, and they hold no work.
const LOCKFILES: [&str; 8] = [
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "bun.lockb",
    "bun.lock",
    "Cargo.lock",
    "go.sum",
    "uv.lock",
];

/// The file a git status --porcelain line names (the new name of a
/// rename), unquoted.
fn status_path(line: &str) -> &str {
    let p = line.get(3..).unwrap_or("");
    let p = p.rsplit(" -> ").next().unwrap_or(p);
    p.trim_matches('"')
}

/// Whether a git status --porcelain line is noise: Claude Code's local
/// settings, or a lockfile anywhere.
pub fn noise(line: &str) -> bool {
    let p = status_path(line);
    let name = p.rsplit('/').next().unwrap_or(p);
    p == ".claude/settings.local.json" || p == ".claude/" || LOCKFILES.contains(&name)
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The first few of files, by name: "a, b and 3 more".
fn some(files: &[&String]) -> String {
    let names: Vec<&str> = files.iter().take(2).map(|l| status_path(l)).collect();
    match files.len() {
        0 => String::new(),
        n if n <= 2 => names.join(", "),
        n => format!("{} and {} more", names.join(", "), n - 2),
    }
}

impl Worktree {
    /// Its group by the facts (None: not measured yet), and why, as of now.
    pub fn triage(&self, f: Option<WtFacts>, now: Ms) -> Triage {
        let t = |class, reason: String| Triage { class, reason };
        if f.is_some_and(|f| f.works_here) {
            return t(Class::Keep, "a process works in it".into());
        }
        if !self.in_use.is_empty() {
            return t(Class::Keep, format!("{} is working in it", self.in_use));
        }
        if self.checked == 0 {
            return t(Class::Keep, "not looked at with git yet".into());
        }
        if !self.err.is_empty() {
            return t(Class::Keep, self.err.clone());
        }
        if self.locked {
            return t(Class::Keep, "locked: someone put a lock on it".into());
        }
        let (noisy, work): (Vec<&String>, Vec<&String>) = self.files.iter().partition(|l| noise(l));
        // files is empty from a cached look older than it: count changed.
        let uncounted = self.changed.saturating_sub(self.files.len());
        let mut held = Vec::new();
        if !work.is_empty() || uncounted > 0 {
            let n = work.len() + uncounted;
            let mut s = format!("{} uncommitted", plural(n, "file"));
            if !work.is_empty() {
                s += &format!(" ({})", some(&work));
            }
            held.push(s);
        }
        if self.unpushed > 0 && !self.merged_pr() {
            held.push(format!(
                "{} not pushed or merged",
                plural(self.unpushed, "commit")
            ));
        }
        if let Some(p) = &self.pr
            && p.open()
        {
            held.push(format!("pull request #{} is {}", p.number, p.state));
        }
        if !held.is_empty() {
            let held = held.join(", ");
            let Some(f) = f else {
                return t(Class::Keep, held);
            };
            let idle = now - f.newest_outside;
            if idle < WT_IDLE {
                return t(Class::Keep, format!("{held}; touched {} ago", span(idle)));
            }
            return t(
                Class::CacheOnly,
                format!(
                    "{held}; untouched for {}: its build caches can go",
                    span(idle)
                ),
            );
        }
        // Its verdict as if the noise were gone: what says its work is in.
        let mut quiet = self.clone();
        (quiet.changed, quiet.files) = (0, Vec::new());
        let done = quiet.verdict().reason;
        if !noisy.is_empty() {
            return t(
                Class::NoiseOnly,
                format!("{done}; only {} uncommitted", some(&noisy)),
            );
        }
        t(Class::Safe, done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sweep::{DAY, HOUR};
    use rush_core::agent::PrStatus;

    const NOW: Ms = 1_793_190_896_789;

    fn wt(files: &[&str], unpushed: usize, pr: Option<(&str, bool)>) -> Worktree {
        Worktree {
            path: "/src/r/.worktrees/x".into(),
            repo: "/src/r".into(),
            branch: "x".into(),
            checked: NOW,
            changed: files.len(),
            files: files.iter().map(|s| s.to_string()).collect(),
            unpushed,
            off_remote: unpushed,
            pr: pr.map(|(state, _)| PrStatus {
                number: 7,
                state: state.into(),
                ..PrStatus::default()
            }),
            in_pr: pr.is_some_and(|(_, head)| head),
            ..Worktree::default()
        }
    }

    #[test]
    fn noise_is_settings_and_lockfiles() {
        for (line, want) in [
            ("?? .claude/settings.local.json", true),
            ("?? .claude/", true),
            (" M pnpm-lock.yaml", true),
            (" M apps/web/package-lock.json", true),
            ("R  old.lock -> Cargo.lock", true),
            (" M src/main.rs", false),
            ("?? .claude/agents/x.md", false),
            ("?? notes.md", false),
        ] {
            assert_eq!(noise(line), want, "{line}");
        }
    }

    #[test]
    fn groups_by_the_facts() {
        let idle = WtFacts {
            newest_outside: NOW - 3 * DAY,
            works_here: false,
        };
        let fresh = WtFacts {
            newest_outside: NOW - 2 * HOUR,
            works_here: false,
        };
        let busy = WtFacts {
            works_here: true,
            ..idle
        };
        let merged = Some(("merged", true));
        let open = Some(("open", false));
        let cases: Vec<(Worktree, WtFacts, Class, &str)> = vec![
            (
                wt(&[], 0, merged),
                idle,
                Class::Safe,
                "its pull request #7 was merged",
            ),
            (
                wt(&[], 2, merged),
                idle,
                Class::Safe,
                "its pull request #7 was merged",
            ),
            (
                wt(&[], 0, None),
                idle,
                Class::Safe,
                "every commit is pushed",
            ),
            (
                wt(&["?? .claude/settings.local.json"], 0, merged),
                idle,
                Class::NoiseOnly,
                "its pull request #7 was merged; only .claude/settings.local.json uncommitted",
            ),
            (
                wt(
                    &[" M pnpm-lock.yaml", "?? .claude/settings.local.json"],
                    0,
                    merged,
                ),
                fresh,
                Class::NoiseOnly,
                "its pull request #7 was merged; only pnpm-lock.yaml, .claude/settings.local.json uncommitted",
            ),
            (
                wt(
                    &[" M a.rs", " M b.rs", "?? c.rs", " M yarn.lock"],
                    0,
                    merged,
                ),
                idle,
                Class::CacheOnly,
                "3 files uncommitted (a.rs, b.rs and 1 more); untouched for 3 days: its build caches can go",
            ),
            (
                wt(&[" M a.rs"], 0, merged),
                fresh,
                Class::Keep,
                "1 file uncommitted (a.rs); touched 2h ago",
            ),
            (
                wt(&[], 3, None),
                idle,
                Class::CacheOnly,
                "3 commits not pushed or merged; untouched for 3 days: its build caches can go",
            ),
            (
                wt(&[], 0, open),
                fresh,
                Class::Keep,
                "pull request #7 is open; touched 2h ago",
            ),
            (
                wt(&[], 0, merged),
                busy,
                Class::Keep,
                "a process works in it",
            ),
            (
                Worktree::default(),
                idle,
                Class::Keep,
                "not looked at with git yet",
            ),
        ];
        for (w, f, class, reason) in cases {
            let got = w.triage(Some(f), NOW);
            assert_eq!((got.class, got.reason.as_str()), (class, reason), "{w:?}");
        }
        let got = wt(&[" M a.rs"], 0, None).triage(None, NOW);
        assert_eq!(
            (got.class, got.reason.as_str()),
            (Class::Keep, "1 file uncommitted (a.rs)"),
            "not measured yet"
        );
    }
}
