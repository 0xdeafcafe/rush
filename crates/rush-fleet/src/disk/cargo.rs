//! Cargo targets and what in them can go, what removing something frees,
//! and whether anything uses a folder. Ported from rush-bundled's
//! clean/disk.rs as of 239f592a (is_target, find_targets, profile_dirs,
//! target_activity, old_artifacts, build_of, superseded_artifacts,
//! freeable, in_use_from, in_use); the artifacts are listed once
//! (artifacts) and judged without the disk.
//!
//! A folder is a target only when cargo's CACHEDIR.TAG and
//! .rustc_info.json are both in it; nothing is looked for inside one.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use rush_core::time::Ms;

use super::{Artifact, InUse};
use crate::temp::ms_of;

/// How deep under a scratch root targets are looked for:
/// tmp/<project>/<session>/<name>-target.
pub const SCAN_DEPTH: usize = 3;

/// Whether dir is a cargo target: cargo writes both into every one.
pub fn is_target(dir: &Path) -> bool {
    real_dir(dir)
        && !dir.join(".git").exists()
        && !dir.join("Cargo.toml").exists()
        && dir
            .join("CACHEDIR.TAG")
            .symlink_metadata()
            .is_ok_and(|m| m.is_file())
        && crate::cache_tagged(&dir.to_string_lossy())
        && dir
            .join(".rustc_info.json")
            .symlink_metadata()
            .is_ok_and(|m| m.is_file())
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

pub(super) fn real_dir(p: &Path) -> bool {
    p.symlink_metadata().is_ok_and(|m| m.is_dir())
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::symlink_metadata(p).ok()?.modified().ok()
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
            .filter(|p| real_dir(p) && real_dir(&p.join(".fingerprint")))
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
pub fn target_activity(target: &Path) -> Ms {
    let mut paths = vec![target.to_path_buf(), target.join(".rustc_info.json")];
    for p in profile_dirs(target) {
        for sub in ["deps", ".fingerprint", ".cargo-lock", "build"] {
            paths.push(p.join(sub));
        }
        paths.push(p);
    }
    paths.iter().filter_map(|p| mtime(p)).max().map_or(0, ms_of)
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

/// What's in each profile's deps and incremental, with when each was last
/// written and, in deps, the build it's of; sorted by path.
pub fn artifacts(target: &Path) -> Vec<Artifact> {
    let mut out = Vec::new();
    for p in profile_dirs(target) {
        managed_graphs(&p, &mut out);
        for (sub, incremental) in [("deps", false), ("incremental", true)] {
            if !real_dir(&p.join(sub)) {
                continue;
            }
            for e in std::fs::read_dir(p.join(sub))
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = e.path();
                let name = e.file_name();
                let (krate, build) = match name.to_str().and_then(build_of) {
                    Some((k, b)) if !incremental => (k.to_string(), b.to_string()),
                    _ => (String::new(), String::new()),
                };
                out.push(Artifact {
                    mtime: mtime(&path).map_or(0, ms_of),
                    path,
                    profile: p.clone(),
                    incremental,
                    krate,
                    build,
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Only the crate directories inside a managed unit's rustc directory are
/// Cargo artifacts. Keep kache's metadata, seed markers and lock inode.
fn managed_graphs(profile: &Path, out: &mut Vec<Artifact>) {
    let root = profile.join("incremental.kache-auto");
    if !real_dir(&root) {
        return;
    }
    for version in ["v1", "v2"] {
        let dir = root.join(version);
        if !real_dir(&dir) {
            continue;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let unit = e.path();
            let key = e.file_name();
            let key = key.as_encoded_bytes();
            if key.len() != 64 || !key.iter().all(u8::is_ascii_hexdigit) || !real_dir(&unit) {
                continue;
            }
            let rustc = unit.join("rustc");
            if !real_dir(&rustc) {
                continue;
            }
            let Some(used) = graph_used(&unit) else {
                continue;
            };
            for graph in std::fs::read_dir(rustc).into_iter().flatten().flatten() {
                if graph.file_type().is_ok_and(|t| t.is_dir()) {
                    out.push(Artifact {
                        path: graph.path(),
                        profile: profile.to_owned(),
                        mtime: used,
                        incremental: true,
                        ..Artifact::default()
                    });
                }
            }
        }
    }
}

/// kache updates the unit directory on use and may record a later clock.
fn graph_used(unit: &Path) -> Option<Ms> {
    let modified = ms_of(mtime(unit)?);
    let path = unit.join("state.json");
    match path.symlink_metadata() {
        Ok(m) if m.is_file() && m.len() < 64 * 1024 => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(modified),
        _ => return None,
    }
    let state = match std::fs::read(path) {
        Ok(data) => serde_json::from_slice::<serde_json::Value>(&data)
            .ok()?
            .get("last_used_secs")?
            .as_i64()?
            .checked_mul(1000)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(_) => return None,
    };
    Some(modified.max(state))
}

pub(super) fn managed_graph(a: &Artifact) -> bool {
    a.incremental && a.path.starts_with(a.profile.join("incremental.kache-auto"))
}

/// Take the same exclusive lock as kache. A build that used this unit
/// since discovery, a busy lock, or any link in the layout keeps it.
pub(super) fn lock_graph(graph: &Path, used: Ms) -> Option<std::fs::File> {
    let rustc = graph.parent()?;
    let unit = rustc.parent()?;
    let version = unit.parent()?;
    let root = version.parent()?;
    let profile = root.parent()?;
    let key = unit.file_name()?.as_encoded_bytes();
    if key.len() != 64
        || !key.iter().all(u8::is_ascii_hexdigit)
        || !profile
            .ancestors()
            .skip(1)
            .take(2)
            .take_while(|d| real_dir(d))
            .any(is_target)
        || rustc.file_name()? != "rustc"
        || root.file_name()? != "incremental.kache-auto"
        || !matches!(version.file_name()?.to_str()?, "v1" | "v2")
        || ![graph, rustc, unit, version, root, profile]
            .into_iter()
            .all(real_dir)
    {
        return None;
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(unit.join("unit.lock"))
        .ok()?;
    if !lock.metadata().ok()?.is_file() {
        return None;
    }
    lock.try_lock().ok()?;
    (graph_used(unit)? == used).then_some(lock)
}

/// Of the ordinary Cargo artifacts, those last written at least max_age before now, sorted; one
/// whose time isn't known (0) stays.
pub fn old_artifacts(arts: &[Artifact], now: Ms, max_age: Ms) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = arts
        .iter()
        .filter(|a| !managed_graph(a) && a.mtime > 0 && now - a.mtime >= max_age)
        .map(|a| a.path.clone())
        .collect();
    out.sort();
    out
}

/// Of arts, in each profile's deps, the files of a crate's builds older
/// than its newest keep, whose newest file is at least min_age old,
/// sorted. A build cargo still uses keeps its old time when it's reused,
/// so age alone can't tell; being superseded by keep newer ones can.
/// Cargo builds again any it later finds missing.
pub fn superseded_artifacts(arts: &[Artifact], now: Ms, keep: usize, min_age: Ms) -> Vec<PathBuf> {
    // (profile, crate) -> build -> (its newest file's time, its files)
    type Builds<'a> = BTreeMap<&'a str, (Ms, Vec<&'a Path>)>;
    let mut crates: BTreeMap<(&Path, &str), Builds<'_>> = BTreeMap::new();
    for a in arts
        .iter()
        .filter(|a| !a.incremental && !a.build.is_empty())
    {
        let b = crates
            .entry((a.profile.as_path(), a.krate.as_str()))
            .or_default()
            .entry(a.build.as_str())
            .or_insert((0, Vec::new()));
        b.0 = b.0.max(a.mtime);
        b.1.push(a.path.as_path());
    }
    let mut out = Vec::new();
    for builds in crates.into_values() {
        let mut builds: Vec<_> = builds.into_values().collect();
        builds.sort_by_key(|b| std::cmp::Reverse(b.0));
        for (t, files) in builds.into_iter().skip(keep) {
            if now - t >= min_age {
                out.extend(files.into_iter().map(Path::to_path_buf));
            }
        }
    }
    out.sort();
    out
}

/// What removing path would free: the blocks of the files under it with no
/// other hard link, links not followed.
pub fn freeable(path: &Path) -> u64 {
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
        m.blocks() * 512
    } else {
        0
    }
}

/// Whether anything uses path, from what lsof +D printed (None when it
/// couldn't run) and what ps printed of every process with its arguments
/// and environment (None when it couldn't run): a file open in it, a
/// process working in it, or one naming it, other than me. Not knowing is
/// Unknown, never No.
pub fn in_use_from(lsof: Option<&[u8]>, ps: Option<&str>, path: &Path, me: u32) -> InUse {
    match lsof {
        None => return InUse::Unknown,
        Some(out) if !out.is_empty() => return InUse::Yes,
        Some(_) => {}
    }
    let Some(ps) = ps.filter(|s| !s.trim().is_empty()) else {
        return InUse::Unknown;
    };
    let needle = path.to_string_lossy();
    let needle = needle.trim_end_matches('/');
    let me = me.to_string();
    let named = ps.lines().any(|l| {
        let l = l.trim_start();
        let pid = l.split_whitespace().next().unwrap_or("");
        pid != me && l.contains(needle)
    });
    if named { InUse::Yes } else { InUse::No }
}

/// lsof, by its path: it's in /usr/sbin, which a plugin's PATH lacks, and
/// not finding it answered "in use" for every target.
pub const LSOF: &str = "/usr/sbin/lsof";
const PS: &str = "/bin/ps";

/// Whether anything uses path now: lsof +D, then every process's arguments
/// and environment (macOS hides the environment of platform binaries;
/// Homebrew's cargo and rustc show theirs). Asked right before a removal.
pub fn in_use(path: &Path) -> InUse {
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
    let lsof = lsof
        .as_ref()
        .filter(|o| !o.stdout.is_empty() || (o.status.code() == Some(1) && o.stderr.is_empty()))
        .map(|o| o.stdout.as_slice());
    match in_use_from(lsof, ps.as_deref(), path, me) {
        InUse::No if real != path => in_use_from(Some(&[][..]), ps.as_deref(), &real, me),
        u => u,
    }
}
