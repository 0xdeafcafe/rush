//! The disk engine's rules: what goes and when, per level, kept in rush's
//! folder as disk-rules.json (`{ "rules": [...], "proposed": [...] }`).
//! The built-in rules are today's clean-ups as data: rush-bundled's
//! clean/disk.rs (targets 2 days idle, 30 minutes when low; artifacts 2
//! days old, 6 hours when low; superseded builds past the newest 3, a day
//! old, an hour when low; kache's store emptied when low), reap.rs
//! (worktrees merged, clean and 3 days idle) and rules.rs BUILT_IN (cache
//! folders, and Go's module cache through go clean). Rust-only.
//!
//! The file keeps only what differs from the built-in rules (your edits,
//! rules you made, proposals): a built-in rule you never touched follows
//! rush's defaults as they change. Proposals never run; accepting one
//! moves it into rules. Every change is a read, change and write under a
//! flock on disk-rules.lock beside it, written with rush_state::write_json,
//! so two rushes changing it at once lose neither change.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;

use rush_core::time::Ms;
use serde::{Deserialize, Serialize};

use super::Level;
use crate::rules::BUILT_IN;

/// The rules file, in rush's folder.
pub const DISK_RULES_FILE: &str = "disk-rules.json";
/// The lock every change to it holds, beside it.
pub const DISK_RULES_LOCK: &str = "disk-rules.lock";

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// What a rule is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Kind {
    /// A cargo target goes whole once no build touched it for idle.
    CargoTarget,
    /// In a cargo target that stays, deps artifacts and incremental
    /// sessions older than age go, and builds of a crate past the newest
    /// keep once superseded old.
    CargoArtifacts,
    /// A linked worktree goes once its commits are in its base branch,
    /// nothing is uncommitted, nothing changed for idle, and nothing works
    /// there.
    Worktree,
    /// A cache folder its tool makes again goes once nothing in it changed
    /// for idle (0: whenever the rule runs). path may start with ~; with
    /// each, the rule is for each folder in it named in hex (a
    /// workspace's hash). go_clean clears Go's module cache with go clean
    /// -modcache, at path, or where Go keeps it when path is "".
    CacheDir {
        path: String,
        why: String,
        #[serde(default, skip_serializing_if = "is_false")]
        each: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        go_clean: bool,
    },
    /// A tool's own clean-up, run once per run at the levels it has.
    ToolCommand { command: Vec<String>, why: String },
    /// What plugins report of this kind (disk.candidates) goes once idle.
    PluginCandidate { kind: String },
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A rule's numbers at one level. What a kind doesn't use is None.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Params {
    /// How long nothing may have touched it before it goes, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_secs: Option<u64>,
    /// How old an artifact in a target that stays may get, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_secs: Option<u64>,
    /// How many builds of a crate a target keeps however old.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep: Option<u32>,
    /// How old a build past the newest keep may get, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_secs: Option<u64>,
}

/// One rule: what it's about, whether it's on, and its numbers at each
/// level; a level with none is one it doesn't run at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Under 50 GB free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal: Option<Params>,
    /// Under 20 GB free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<Params>,
}

fn yes() -> bool {
    true
}

impl Rule {
    /// Its numbers at lv, None when it's off or doesn't run there.
    pub fn at(&self, lv: Level) -> Option<&Params> {
        if !self.enabled {
            return None;
        }
        match lv {
            Level::Normal => self.normal.as_ref(),
            Level::Low => self.low.as_ref(),
        }
    }
}

/// A rule someone proposed (the cleanup agent's research), with what
/// shows it's safe. It never runs until you accept it into rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub rule: Rule,
    /// The paths, sizes, who made them and why it's safe, as text.
    pub evidence: String,
    /// Who proposed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
    /// When.
    #[serde(default)]
    pub at: Ms,
}

/// The rules file's content.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RulesFile {
    pub rules: Vec<Rule>,
    pub proposed: Vec<Proposal>,
}

fn at(idle_secs: u64) -> Params {
    Params {
        idle_secs: Some(idle_secs),
        ..Params::default()
    }
}

fn artifacts(age: u64, superseded: u64) -> Params {
    Params {
        age_secs: Some(age),
        keep: Some(3),
        superseded_secs: Some(superseded),
        ..Params::default()
    }
}

/// The rules rush starts with: what its clean-ups did on their own before
/// the engine, plus the cache folders Clean up offered, those only when
/// low.
pub fn built_in_rules() -> Vec<Rule> {
    let mut out = vec![
        Rule {
            id: "cargo-target".into(),
            title: "Cargo targets no build touched lately".into(),
            kind: Kind::CargoTarget,
            enabled: true,
            normal: Some(at(2 * DAY)),
            low: Some(at(30 * MINUTE)),
        },
        Rule {
            id: "cargo-artifacts".into(),
            title: "Old and superseded builds in cargo targets".into(),
            kind: Kind::CargoArtifacts,
            enabled: true,
            normal: Some(artifacts(2 * DAY, DAY)),
            low: Some(artifacts(6 * HOUR, HOUR)),
        },
        Rule {
            id: "worktree".into(),
            title: "Worktrees merged, clean and idle".into(),
            kind: Kind::Worktree,
            enabled: true,
            normal: Some(at(3 * DAY)),
            low: Some(at(3 * DAY)),
        },
        Rule {
            id: "kache".into(),
            title: "kache's store".into(),
            kind: Kind::ToolCommand {
                command: ["kache", "clean", "--cache", "--yes"]
                    .map(String::from)
                    .to_vec(),
                why: "kache's store; builds fill it again".into(),
            },
            enabled: true,
            normal: None,
            low: Some(Params::default()),
        },
    ];
    for b in BUILT_IN {
        out.push(Rule {
            id: format!("cache:{}", b.path),
            title: b.why.into(),
            kind: Kind::CacheDir {
                path: format!("~/{}", b.path),
                why: b.why.into(),
                each: b.each,
                go_clean: false,
            },
            enabled: true,
            normal: None,
            low: Some(at((b.idle / 1000) as u64)),
        });
    }
    out.push(Rule {
        id: "go-mod-cache".into(),
        title: "Go's module cache".into(),
        kind: Kind::CacheDir {
            path: String::new(),
            why: "Go's module cache; builds fetch it again".into(),
            each: false,
            go_clean: true,
        },
        enabled: true,
        normal: None,
        low: Some(at(0)),
    });
    out
}

/// f as rush uses it: the built-in rules in their order, each as f has it
/// when it does, then f's own rules. A built-in rule can't be removed,
/// only turned off.
pub fn merge_built_in(f: RulesFile) -> RulesFile {
    let mut rules: Vec<Rule> = built_in_rules()
        .into_iter()
        .map(|b| f.rules.iter().find(|r| r.id == b.id).cloned().unwrap_or(b))
        .collect();
    for r in f.rules {
        if !rules.iter().any(|o| o.id == r.id) {
            rules.push(r);
        }
    }
    RulesFile {
        rules,
        proposed: f.proposed,
    }
}

/// f as the file keeps it: built-in rules as built only left out.
fn unmerged(mut f: RulesFile) -> RulesFile {
    let built = built_in_rules();
    f.rules.retain(|r| !built.contains(r));
    f
}

/// Where the rules file is: rush's folder.
pub fn rules_path() -> PathBuf {
    rush_state::dir().join(DISK_RULES_FILE)
}

fn read_file() -> io::Result<RulesFile> {
    match fs::read(rules_path()) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(RulesFile::default()),
        Err(e) => Err(e),
        Ok(b) => serde_json::from_slice(&b).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", rules_path().display()),
            )
        }),
    }
}

/// The rules rush runs and the proposals waiting, the built-in rules
/// merged in; the built-in rules alone when there's no file. A file that
/// can't be read is an error, never the defaults: they'd run what you
/// turned off.
pub fn load_rules() -> io::Result<RulesFile> {
    read_file().map(merge_built_in)
}

/// Changes the rules file under its lock: read, f, write. f sees it with
/// the built-in rules merged in, as load_rules does; the file keeps what
/// differs from them. It hands back what it wrote, merged. A file that
/// can't be read is left as it is.
pub fn update_rules(f: impl FnOnce(&mut RulesFile)) -> io::Result<RulesFile> {
    let p = rules_path();
    if let Some(dir) = p.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let lf = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(p.with_file_name(DISK_RULES_LOCK))?;
    // SAFETY: flock on a descriptor lf owns; let go when it's closed.
    if unsafe { libc::flock(lf.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = merge_built_in(read_file()?);
    f(&mut file);
    let file = merge_built_in(file);
    rush_state::write_json(&p, &unmerged(file.clone()))?;
    drop(lf);
    Ok(file)
}
