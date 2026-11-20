//! Rush's disk engine: what may go when the disk runs low, decided by data
//! rules and nothing else. Rules (ruleset.rs, kept in disk-rules.json) say
//! what goes and when, per level of free space; candidates are what's on
//! disk that a rule may be about, each with the facts that decide it,
//! found by the engine itself or reported by plugins (disk.candidates);
//! the plan is acts and what's kept and why, and an outcome is what an act
//! did. Proposed rules (propose.rs) wait in the file until you accept one. Rust-only: Go had no such engine. Ported from rush-bundled's
//! clean/disk.rs (levels, targets, artifacts, in_use) and reap.rs
//! (worktrees), and this crate's rules.rs (cache folders).
//!
//! Nothing here removes anything but through sweep::remove_tree, which
//! refuses NEVER_TOUCH, or git worktree remove without force; every
//! removal asks whether anything uses its path right before, and goes only
//! when the answer is no.

mod apply;
mod cargo;
mod find;
mod plan;
mod propose;
pub mod quiet;
mod ruleset;
pub mod volume;

pub use apply::{KNOWN_TOOLS, apply, apply_all, event_fields, known_tool, run, run_at};
pub use cargo::{
    LSOF, SCAN_DEPTH, artifacts, build_of, find_targets, freeable, in_use, in_use_from, is_target,
    old_artifacts, superseded_artifacts, target_activity,
};
pub use find::{
    GitWorktree, Scan, Session, cache_dirs, find_candidates, last_touched, parse_worktrees,
};
pub use plan::plan;
pub use propose::{
    PROPOSALS_FILE, Proposed, add_proposals, cache_dir_allowed, check_rule, parse_proposals,
    propose, propose_file, same_rule,
};
pub use ruleset::{
    DISK_RULES_FILE, DISK_RULES_LOCK, Kind, Params, Proposal, Rule, RulesFile, built_in_rules,
    load_rules, merge_built_in, rules_path, update_rules,
};

use std::path::PathBuf;

use rush_core::time::Ms;
use serde::{Deserialize, Serialize};

const GB: u64 = 1 << 30;

/// Under this free, rush cleans at the normal level.
pub const NORMAL_UNDER: u64 = 50 * GB;
/// Under this free, it's low: rules clean harder, and some run only then.
pub const LOW_UNDER: u64 = 40 * GB;

/// How hard rush cleans, by the disk's free space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Under NORMAL_UNDER (50 GB) free.
    Normal,
    /// Under LOW_UNDER (40 GiB) free.
    Low,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Normal => "normal",
            Level::Low => "low",
        }
    }
}

/// The level at free bytes free: None when there's enough, or when free
/// is 0, which is not knowing.
pub fn level(free: u64) -> Option<Level> {
    if free == 0 || free >= NORMAL_UNDER {
        None
    } else if free < LOW_UNDER {
        Some(Level::Low)
    } else {
        Some(Level::Normal)
    }
}

/// A cargo target: a folder with both CACHEDIR.TAG and .rustc_info.json.
pub const CARGO_TARGET: &str = "cargo-target";
/// A linked git worktree.
pub const WORKTREE: &str = "worktree";
/// A folder a cache-dir rule names.
pub const CACHE_DIR: &str = "cache-dir";
/// What a tool-command rule runs: not a folder.
pub const TOOL_COMMAND: &str = "tool-command";

/// Whether anything uses a path: a file open in it, a process working in
/// it or naming it. Not knowing is never no.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InUse {
    #[default]
    Unknown,
    Yes,
    No,
}

/// Something on disk a rule may be about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Candidate {
    pub path: PathBuf,
    /// CARGO_TARGET, WORKTREE, CACHE_DIR, or a kind a plugin reports
    /// ("js.node-modules", "rust.sccache", …), which plugin-candidate rules
    /// name.
    pub kind: String,
    /// The rule it was found by: a cache-dir rule's id; "" otherwise.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub rule: String,
    /// What it is, in a few words.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// The plugin that reported it; "" when the engine found it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plugin: String,
    /// Its blocks on disk, every file counted once; 0 when not measured.
    pub bytes_on_disk: u64,
    /// What removing it would free: files with no other hard link.
    pub bytes_freeable: u64,
    pub facts: Facts,
}

/// What decides whether a candidate goes. What wasn't looked at is left
/// at its default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Facts {
    /// A cargo target: when a build last touched it (target_activity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_build: Option<Ms>,
    /// When anything in it last changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<Ms>,
    /// A worktree: how its commits are kept (its branch is merged into
    /// base, …); "" when they aren't, or that's not known.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub merged: String,
    /// A worktree: its uncommitted files, untracked ones included; None
    /// when git couldn't say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<usize>,
    /// A worktree: processes working in it; None when that can't be told.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<usize>,
    /// A worktree: the repo's own checkout, which never goes.
    #[serde(skip_serializing_if = "is_false")]
    pub main: bool,
    /// A worktree: locked with git worktree lock.
    #[serde(skip_serializing_if = "is_false")]
    pub locked: bool,
    /// A worktree: its main checkout, which git worktree remove runs in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<PathBuf>,
    /// A check that failed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub err: String,
    /// Whether anything uses it, as last asked (removal asks again).
    pub in_use: InUse,
    /// The session it belongs to (works in it, or built it); "" when none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session: String,
    /// That session is working now.
    #[serde(skip_serializing_if = "is_false")]
    pub working: bool,
    /// The worktree it belongs to; "" when none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worktree: String,
    /// A cargo target: its deps artifacts and incremental sessions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Facts {
    /// A worktree with no uncommitted files, as git said.
    pub fn clean(&self) -> bool {
        self.changed == Some(0)
    }

    /// How long since mtime at now; None when it's not known.
    pub fn idle(&self, now: Ms) -> Option<Ms> {
        self.mtime.map(|t| (now - t).max(0))
    }
}

/// A file or folder in a cargo target's profile: in deps, of a crate's
/// build (build_of), or an incremental session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Artifact {
    pub path: PathBuf,
    /// Last written.
    pub mtime: Ms,
    /// The profile folder it's in (target/debug, target/<triple>/release).
    pub profile: PathBuf,
    /// In incremental, not deps.
    #[serde(skip_serializing_if = "is_false")]
    pub incremental: bool,
    /// In deps, the crate and the build it's of; "" for anything else.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub krate: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub build: String,
}

/// What an act does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum How {
    /// The path goes whole, through sweep::remove_tree.
    Remove,
    /// These paths in it go, each through sweep::remove_tree; it stays.
    Prune { paths: Vec<PathBuf> },
    /// A kache-managed crate's rustc incremental data. Recheck its unit
    /// under kache's own lock before removing these rebuildable files.
    PruneGraph { last_used: Ms },
    /// git worktree remove (never forced) in repo, then git worktree prune.
    RemoveWorktree { repo: PathBuf },
    /// go clean -modcache, Go's module files being read-only.
    GoClean,
    /// A tool's own clean-up, run once.
    Run { command: Vec<String> },
}

/// One thing a plan does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Act {
    /// The rule that says so.
    pub rule: String,
    /// What it acts on; empty for a command.
    pub path: PathBuf,
    /// The candidate's kind, or TOOL_COMMAND.
    pub kind: String,
    /// Why it goes, in a few words.
    pub why: String,
    /// What it should free, as measured when found; 0 when not measured.
    pub bytes: u64,
    pub how: How,
}

/// Something a rule would act on that stays, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub path: PathBuf,
    pub rule: String,
    pub why: String,
}

/// An act done: what it freed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub rule: String,
    pub path: PathBuf,
    /// What was done, in a few words.
    pub what: String,
    pub freed: u64,
}

/// What a run did, what it left and why, and how much it freed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub did: Vec<Done>,
    pub left: Vec<Kept>,
    pub freed: u64,
}

/// The candidates in one plugin's disk.candidates answer (rush-plugin's
/// DiskCandidates, the same JSON), each marked as the plugin's. Refused
/// whole when it isn't that; a candidate whose path isn't absolute, or
/// whose kind is one the engine finds itself, is left out.
pub fn from_answer(plugin: &str, answer: &serde_json::Value) -> Result<Vec<Candidate>, String> {
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        candidates: Vec<Candidate>,
    }
    let a: Answer = serde_json::from_value(answer.clone())
        .map_err(|e| format!("{plugin}'s disk.candidates: {e}"))?;
    Ok(a.candidates
        .into_iter()
        .filter(|c| {
            c.path.is_absolute()
                && !c.kind.is_empty()
                && ![CARGO_TARGET, WORKTREE, CACHE_DIR, TOOL_COMMAND].contains(&c.kind.as_str())
        })
        .map(|mut c| {
            c.plugin = plugin.to_string();
            c.rule.clear();
            c
        })
        .collect())
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_propose;
