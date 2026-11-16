//! Build caches in a worktree that rebuild themselves (target,
//! node_modules, .next, .turbo, dist at its top, any folder there with a
//! CACHEDIR.TAG, and each app's node_modules in apps/*), and those at the
//! top of a session's $TMPDIR (temp_caches_in: cargo's target folders
//! wherever CARGO_TARGET_DIR put them, by their tag): what Clean up and
//! rush's own clean when the disk is dire may remove, with the checks
//! remove_left has (refuse what isn't one, what changed lately).
//! Rust-only: Go had no such clean.

use std::path::Path;

use rush_core::agent::Agent;
use rush_core::time::Ms;
use serde_json::json;

use crate::left::scratch_walk;
use crate::temp::{agent_event, err_text};

/// The folders a build makes again by itself, by name, at a worktree's top.
pub const CACHE_NAMES: [&str; 5] = ["target", "node_modules", ".next", ".turbo", "dist"];

/// How much disk p takes and when anything in it last changed. Off the UI.
pub fn walk(p: &str) -> (i64, Ms) {
    scratch_walk(p)
}

fn real_dir(p: &str) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// The file that marks a folder as a cache anything may delete
/// (bford.info/cachedir): cargo writes one in every target folder, and
/// Python in every venv.
pub const CACHE_TAG: &str = "CACHEDIR.TAG";

/// What a CACHEDIR.TAG starts with when it's one.
const CACHE_TAG_HEAD: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Whether folder p is tagged a cache: its CACHEDIR.TAG has the signature.
pub fn cache_tagged(p: &str) -> bool {
    use std::io::Read;
    let mut head = [0u8; CACHE_TAG_HEAD.len()];
    std::fs::File::open(Path::new(p).join(CACHE_TAG))
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|_| head == CACHE_TAG_HEAD)
}

/// The folders at the top of dir, as paths, sorted.
fn tops(dir: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| format!("{dir}/{}", e.file_name().to_string_lossy()))
                .filter(|p| real_dir(p))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The caches at the top of a session's $TMPDIR that are there now: the
/// folders named in CACHE_NAMES and those tagged, by name.
pub fn temp_caches_in(dir: &str) -> Vec<String> {
    let dir = dir.trim_end_matches('/');
    tops(dir)
        .into_iter()
        .filter(|p| {
            let name = p.rsplit('/').next().unwrap_or("");
            CACHE_NAMES.contains(&name) || cache_tagged(p)
        })
        .collect()
}

/// The caches of worktree wt that are there now: at its top (by name,
/// then those tagged), then each app's node_modules
/// (apps/*/node_modules), by name.
pub fn caches_in(wt: &str) -> Vec<String> {
    let wt = wt.trim_end_matches('/');
    let mut out: Vec<String> = CACHE_NAMES
        .iter()
        .map(|n| format!("{wt}/{n}"))
        .filter(|p| real_dir(p))
        .collect();
    let named = out.clone();
    out.extend(
        tops(wt)
            .into_iter()
            .filter(|p| !named.contains(p) && cache_tagged(p)),
    );
    let mut apps: Vec<String> = std::fs::read_dir(format!("{wt}/apps"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_type().is_ok_and(|t| t.is_dir())
                        && !e.file_name().to_string_lossy().starts_with('.')
                })
                .map(|e| format!("{wt}/apps/{}/node_modules", e.file_name().to_string_lossy()))
                .filter(|p| real_dir(p))
                .collect()
        })
        .unwrap_or_default();
    apps.sort();
    out.extend(apps);
    out
}

/// The folder of roots (worktrees, or sessions' $TMPDIRs) that path is a
/// cache of: one of CACHE_NAMES or a tagged folder at its top, or
/// apps/<app>/node_modules in it.
fn cache_of<'a>(path: &str, worktrees: &'a [String]) -> Option<&'a String> {
    let p = Path::new(path);
    let name = p.file_name()?.to_string_lossy();
    let parent = p.parent()?;
    let is = |d: &Path, w: &String| crate::worktree::clean(w) == d.to_string_lossy();
    if (CACHE_NAMES.contains(&name.as_ref()) || cache_tagged(path))
        && let Some(w) = worktrees.iter().find(|w| is(parent, w))
    {
        return Some(w);
    }
    let apps = parent.parent()?;
    let app = parent.file_name()?.to_string_lossy();
    if name != "node_modules" || app.starts_with('.') || apps.file_name()? != "apps" {
        return None;
    }
    let wt = apps.parent()?;
    worktrees.iter().find(|w| is(wt, w))
}

/// Deletes cache folder path, a cache (cache_of) of one of worktrees
/// (those nobody works in, or the $TMPDIRs of sessions between turns),
/// unless anything in it changed at or after not_since. why is said in the
/// event log and the dry run's line; by, when given, is the session it's
/// logged for (`rush why`). It says how much it freed.
pub fn remove_cache(
    path: &str,
    worktrees: &[String],
    not_since: Ms,
    why: &str,
    by: Option<&Agent>,
) -> Result<i64, String> {
    let r = (|| {
        let clean = crate::worktree::clean(path);
        if clean != path || cache_of(&clean, worktrees).is_none() {
            return Err(format!(
                "won't delete {path}: it isn't a build cache in a folder nobody works in"
            ));
        }
        if !real_dir(path) {
            return Err(format!("{path}: gone already"));
        }
        let (size, newest) = scratch_walk(path);
        if newest >= not_since {
            return Err(format!("{path}: changed lately; it stays"));
        }
        if rush_core::dryrun::would("remove-cache", path, why) {
            return Ok(0);
        }
        crate::sweep::remove_tree(path).map(|_| size)
    })();
    agent_event(
        by,
        "cache-removed",
        json!({
            "path": path,
            "why": why,
            "bytes": r.as_ref().copied().unwrap_or(0),
            "err": err_text(&r),
        }),
    );
    r
}
