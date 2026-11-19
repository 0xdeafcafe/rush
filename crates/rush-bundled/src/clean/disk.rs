//! Cargo targets and what in them can go, the check that nothing uses a
//! folder, and the clean-ups' auto mode: when you turn it on, git-clean
//! and rust-clean clean on their own while the disk is low, only what
//! loses nothing. Rust-only: Go had no such clean. Ported from h2recomp's
//! crates/diskclean/src/lib.rs (is_target, find_targets, target_activity,
//! profile_dirs, old_artifacts, in_use, freeable, plan, apply) and
//! docs/dev/disk.md.
//!
//! A folder is a target only when cargo's CACHEDIR.TAG and
//! .rustc_info.json are both in it; nothing is looked for inside one, and
//! no other folder (run folders, data roots) is ever touched. "In use" is
//! asked again right before each removal, and a removal goes only when
//! the answer is no. Sizes count files with no other hard link only: an
//! artifact kache links from its store frees nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

const GB: u64 = 1 << 30;

/// At or above this free, the auto mode does nothing.
pub const AUTO_UNDER: u64 = 50 * GB;
/// Under this free, it's low: idle targets go after half an hour, and
/// kache's store is emptied.
pub const LOW_UNDER: u64 = 20 * GB;

pub const DAY: Duration = Duration::from_secs(24 * 3600);
/// How long no build may have touched a target before it goes whole.
pub const TARGET_IDLE: Duration = Duration::from_secs(2 * 24 * 3600);
/// The same, when the disk is low: busy machines build into every target
/// daily, and whoever comes back to one removed rebuilds it.
pub const LOW_TARGET_IDLE: Duration = Duration::from_secs(30 * 60);
/// In targets that stay, deps artifacts and incremental sessions not
/// written for this long go; cargo rebuilds any it finds missing.
pub const ARTIFACT_AGE: Duration = Duration::from_secs(2 * 24 * 3600);
/// The same, when the disk is low: a target built every few minutes is
/// never idle, and piles up copies of crates no build reads any more
/// (49 GB in one, half of it untouched for 6 hours).
pub const LOW_ARTIFACT_AGE: Duration = Duration::from_secs(6 * 3600);
/// How many builds of one crate a target keeps, however old: the newest.
/// A crate gets a build per set of features and profile settings, and a
/// workspace builds a few of those; the rest are what earlier builds left
/// (178 builds of one crate in a target built every few minutes).
pub const KEEP_BUILDS: usize = 3;
/// How old a build of a crate past the newest KEEP_BUILDS must be before it
/// goes. A build cargo still uses keeps its old time when it's reused, so
/// age alone can't tell; being superseded by 3 newer ones can.
pub const SUPERSEDED_AGE: Duration = DAY;
/// The same, when the disk is low.
pub const LOW_SUPERSEDED_AGE: Duration = Duration::from_secs(3600);
/// How often the auto mode runs while the disk is low, between measures: a
/// machine building in many targets fills 8 GB in minutes, and a target
/// left because a build was using it is worth asking about again soon.
pub const LOW_EVERY: Duration = Duration::from_secs(3 * 60);
/// How deep under rush's scratch root targets are looked for:
/// tmp/<project>/<session>/<name>-target.
pub const SCAN_DEPTH: usize = 3;

/// How hard the auto mode cleans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    /// How long no build may have touched a target before it goes whole.
    pub target_idle: Duration,
    /// How old an artifact in a target that stays may get.
    pub artifact_age: Duration,
    /// How old a superseded build of a crate may get.
    pub superseded_age: Duration,
}

/// How hard the auto mode cleans at free bytes free: None when there's
/// enough.
pub fn level(free: u64) -> Option<Level> {
    if free >= AUTO_UNDER {
        None
    } else if free < LOW_UNDER {
        Some(Level {
            target_idle: LOW_TARGET_IDLE,
            artifact_age: LOW_ARTIFACT_AGE,
            superseded_age: LOW_SUPERSEDED_AGE,
        })
    } else {
        Some(Level {
            target_idle: TARGET_IDLE,
            artifact_age: ARTIFACT_AGE,
            superseded_age: SUPERSEDED_AGE,
        })
    }
}

/// Whether free bytes free is low (under LOW_UNDER), so the auto mode runs
/// every LOW_EVERY. 0 is not knowing, not low.
pub fn is_low(free: u64) -> bool {
    free > 0 && free < LOW_UNDER
}

/// Whether dir is a cargo target: cargo writes both into every one.
pub fn is_target(dir: &Path) -> bool {
    dir.join("CACHEDIR.TAG").is_file() && dir.join(".rustc_info.json").is_file()
}

/// The targets at or below root, depth levels down, never inside one and
/// never through a link, .git or node_modules.
pub fn find_targets(root: &Path, depth: usize, out: &mut BTreeSet<PathBuf>) {
    if std::fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
        return;
    }
    if is_target(root) {
        out.insert(root.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        if e.file_type().is_ok_and(|t| t.is_dir()) && name != ".git" && name != "node_modules" {
            find_targets(&e.path(), depth - 1, out);
        }
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::symlink_metadata(p).ok()?.modified().ok()
}

fn age(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or(Duration::ZERO)
}

/// target/<profile> and target/<triple>/<profile>: those with a
/// .fingerprint.
fn profile_dirs(target: &Path) -> Vec<PathBuf> {
    let profiles = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join(".fingerprint").is_dir())
            .collect()
    };
    let mut out = profiles(target);
    for e in std::fs::read_dir(target).into_iter().flatten().flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) && !p.join(".fingerprint").is_dir() {
            out.extend(profiles(&p));
        }
    }
    out.sort();
    out
}

/// When a build last touched target: the folders and lock files cargo
/// writes on every build of a profile.
pub fn target_activity(target: &Path) -> SystemTime {
    let mut paths = vec![target.to_path_buf(), target.join(".rustc_info.json")];
    for p in profile_dirs(target) {
        for sub in ["deps", ".fingerprint", ".cargo-lock", "build"] {
            paths.push(p.join(sub));
        }
        paths.push(p);
    }
    paths
        .iter()
        .filter_map(|p| mtime(p))
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// The artifacts in deps and the sessions in incremental last written at
/// least max_age before now, sorted.
pub fn old_artifacts(target: &Path, now: SystemTime, max_age: Duration) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in profile_dirs(target) {
        for sub in ["deps", "incremental"] {
            for e in std::fs::read_dir(p.join(sub))
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = e.path();
                if mtime(&path).is_some_and(|t| age(now, t) >= max_age) {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

/// The crate and the build a file or folder in deps is of, by its name:
/// lib<crate>-<16 hex>.rlib, .rmeta, .dylib, <crate>-<16 hex>.d, a test
/// binary <crate>-<16 hex> and its .dSYM. A lib's "lib" is dropped, so a
/// crate's library and its test binaries are one crate. None for anything
/// else.
pub fn build_of(name: &str) -> Option<(&str, &str)> {
    let stem = name.split('.').next()?;
    let (krate, hash) = stem.rsplit_once('-')?;
    let hex = hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex || krate.is_empty() {
        return None;
    }
    Some((
        krate
            .strip_prefix("lib")
            .filter(|k| !k.is_empty())
            .unwrap_or(krate),
        hash,
    ))
}

/// In each profile's deps, the files of a crate's builds older than the
/// newest KEEP_BUILDS, whose newest file is at least min_age old, sorted.
/// Cargo builds again any it later finds missing.
pub fn superseded_artifacts(target: &Path, now: SystemTime, min_age: Duration) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in profile_dirs(target) {
        // crate -> build -> (its newest file's time, its files)
        let mut crates: BTreeMap<String, BTreeMap<String, (SystemTime, Vec<PathBuf>)>> =
            BTreeMap::new();
        for e in std::fs::read_dir(p.join("deps"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = e.file_name();
            let Some((krate, hash)) = name.to_str().and_then(build_of) else {
                continue;
            };
            let path = e.path();
            let t = mtime(&path).unwrap_or(SystemTime::UNIX_EPOCH);
            let b = crates
                .entry(krate.to_string())
                .or_default()
                .entry(hash.to_string())
                .or_insert((SystemTime::UNIX_EPOCH, Vec::new()));
            b.0 = b.0.max(t);
            b.1.push(path);
        }
        for builds in crates.into_values() {
            let mut builds: Vec<_> = builds.into_values().collect();
            builds.sort_by_key(|b| std::cmp::Reverse(b.0));
            for (t, files) in builds.into_iter().skip(KEEP_BUILDS) {
                if age(now, t) >= min_age {
                    out.extend(files);
                }
            }
        }
    }
    out.sort();
    out
}

/// What removing path would free: the blocks of the files under it with no
/// other hard link, links not followed.
pub fn freeable(path: &Path) -> i64 {
    let Ok(m) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if m.is_dir() {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| freeable(&e.path()))
            .sum()
    } else if m.nlink() <= 1 {
        m.blocks() as i64 * 512
    } else {
        0
    }
}

/// Whether anything uses path, from what lsof +D printed (None when it
/// couldn't run) and what ps printed of every process with its arguments
/// and environment (None when it couldn't run): a file open in it, a
/// process working in it, or one naming it, other than me. Not knowing is
/// yes.
pub fn in_use_from(lsof: Option<&[u8]>, ps: Option<&str>, path: &Path, me: u32) -> bool {
    match lsof {
        None => return true,
        Some(out) if !out.is_empty() => return true,
        Some(_) => {}
    }
    let Some(ps) = ps else {
        return true;
    };
    let needle = path.to_string_lossy();
    let needle = needle.trim_end_matches('/');
    let me = me.to_string();
    ps.lines().any(|l| {
        let l = l.trim_start();
        let pid = l.split_whitespace().next().unwrap_or("");
        pid != me && l.contains(needle)
    })
}

/// lsof, by its path: it's in /usr/sbin, which a plugin's PATH lacks, and
/// not finding it answered "in use" for every target.
pub const LSOF: &str = "/usr/sbin/lsof";
const PS: &str = "/bin/ps";

/// Whether anything uses path now: lsof +D, then every process's arguments
/// and environment (macOS hides the environment of platform binaries;
/// Homebrew's cargo and rustc show theirs). Asked right before a removal.
pub fn in_use(path: &Path) -> bool {
    let lsof = Command::new(LSOF).arg("+D").arg(path).output().ok();
    let ps = Command::new(PS)
        .args(["axeww", "-o", "pid=,command="])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    // A link to path names it as its real path.
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let me = std::process::id();
    let lsof = lsof.as_ref().map(|o| o.stdout.as_slice());
    in_use_from(lsof, ps.as_deref(), path, me)
        || real != path && in_use_from(Some(&[][..]), ps.as_deref(), &real, me)
}

/// One thing the auto mode does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// A target no build has touched for long enough: it goes whole.
    Target(PathBuf),
    /// Old artifacts in a target that stays.
    Prune {
        target: PathBuf,
        paths: Vec<PathBuf>,
    },
}

/// What the auto mode would do with the targets at roots' tops (worktrees
/// and main checkouts: root/target only) and SCAN_DEPTH under scans, and
/// why it keeps the rest it looked at. A target goes whole once untouched
/// for the level's target_idle and unused; in the rest that are unused,
/// artifacts older than its artifact_age go, and builds of a crate
/// superseded by KEEP_BUILDS newer ones once superseded_age old. skip
/// names targets a session is working on: never removed whole, but pruned
/// like the rest when nothing has them open (a main checkout every session
/// works in would otherwise never be).
pub fn plan(
    roots: &[PathBuf],
    scans: &[PathBuf],
    now: SystemTime,
    lv: Level,
    skip: &dyn Fn(&Path) -> bool,
    in_use: &dyn Fn(&Path) -> bool,
) -> (Vec<Act>, Vec<(PathBuf, String)>) {
    let mut targets = BTreeSet::new();
    for r in roots {
        find_targets(&r.join("target"), 0, &mut targets);
    }
    for s in scans {
        find_targets(s, SCAN_DEPTH, &mut targets);
    }
    let (mut acts, mut kept) = (Vec::new(), Vec::new());
    for t in targets {
        let working = skip(&t);
        if !working && age(now, target_activity(&t)) >= lv.target_idle {
            if in_use(&t) {
                kept.push((t, "idle, but in use".into()));
            } else {
                acts.push(Act::Target(t));
            }
            continue;
        }
        let mut paths = old_artifacts(&t, now, lv.artifact_age);
        paths.extend(superseded_artifacts(&t, now, lv.superseded_age));
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            if working {
                kept.push((t, "a session is working on it".into()));
            }
            continue;
        }
        if in_use(&t) {
            kept.push((t, "has old artifacts, but a build is using it".into()));
        } else {
            acts.push(Act::Prune { target: t, paths });
        }
    }
    (acts, kept)
}

/// Does one act, once nothing uses it (asked again now), and says how much
/// it freed, or why it didn't.
pub fn apply(a: &Act, in_use: &dyn Fn(&Path) -> bool) -> Result<i64, String> {
    match a {
        Act::Target(t) => {
            if !is_target(t) {
                return Err(format!("{}: no longer a target", t.display()));
            }
            if in_use(t) {
                return Err(format!("{}: in use now; it stays", t.display()));
            }
            let n = freeable(t);
            std::fs::remove_dir_all(t).map_err(|e| format!("{}: {e}", t.display()))?;
            Ok(n)
        }
        Act::Prune { target, paths } => {
            if in_use(target) {
                return Err(format!("{}: in use now; it stays", target.display()));
            }
            let mut n = 0;
            for p in paths.iter().filter(|p| p.starts_with(target)) {
                n += freeable(p);
                let dir = std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
                let r = if dir {
                    std::fs::remove_dir_all(p)
                } else {
                    std::fs::remove_file(p)
                };
                match r {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("{}: {e}", p.display())),
                }
            }
            Ok(n)
        }
    }
}

/// Whether the auto mode is on for the kit with manifest m: its
/// auto-clean setting, read afresh.
pub fn auto_on(m: &rush_plugin::Manifest) -> bool {
    rush_plugin::setting_values_of(m)
        .remove(AUTO_KEY)
        .is_some_and(|v| v == "true")
}

/// The setting that turns the auto mode on.
pub const AUTO_KEY: &str = "auto-clean";

/// The auto mode's switch, off until you turn it on. what says what this
/// kit cleans on its own.
pub fn auto_setting(what: &str) -> rush_plugin::SettingSpec {
    rush_plugin::SettingSpec {
        key: AUTO_KEY.into(),
        title: "Clean on its own when disk is low".into(),
        kind: "bool".into(),
        choices: Vec::new(),
        default: "false".into(),
        description: format!(
            "Every 15 minutes under 50 GB free, and every 3 under 20 GB, cleans without asking only what loses nothing: {what}. Says what it freed."
        ),
    }
}
