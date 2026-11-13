//! Build caches in a worktree that rebuild themselves (target,
//! node_modules, .next, .turbo, dist at its top, and each app's
//! node_modules in apps/*): what Clean up and rush's own clean when the
//! disk is dire may remove, with the checks remove_left has (refuse what
//! isn't one, what changed lately). Rust-only: Go had no such clean.

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

/// The caches of worktree wt that are there now: at its top, then each
/// app's node_modules (apps/*/node_modules), by name.
pub fn caches_in(wt: &str) -> Vec<String> {
    let wt = wt.trim_end_matches('/');
    let mut out: Vec<String> = CACHE_NAMES
        .iter()
        .map(|n| format!("{wt}/{n}"))
        .filter(|p| real_dir(p))
        .collect();
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

/// The worktree of worktrees that path is a cache of, by its text: one of
/// CACHE_NAMES at its top, or apps/<app>/node_modules in it.
fn cache_of<'a>(path: &str, worktrees: &'a [String]) -> Option<&'a String> {
    let p = Path::new(path);
    let name = p.file_name()?.to_string_lossy();
    let parent = p.parent()?;
    let is = |d: &Path, w: &String| crate::worktree::clean(w) == d.to_string_lossy();
    if CACHE_NAMES.contains(&name.as_ref())
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

/// Deletes cache folder path, one of CACHE_NAMES directly in one of
/// worktrees (those nobody works in) or an app's node_modules in one,
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
                "won't delete {path}: it isn't a build cache in a worktree nobody works in"
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
