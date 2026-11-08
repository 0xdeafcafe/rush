//! Build caches in a worktree that rebuild themselves (target,
//! node_modules, .next, .turbo, dist): what rush's own clean when the disk
//! is dire may remove, with the checks remove_left has (refuse what isn't
//! one, what changed lately). Rust-only: Go had no such clean.

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

/// The caches at the top of worktree wt that are there now.
pub fn caches_in(wt: &str) -> Vec<String> {
    CACHE_NAMES
        .iter()
        .map(|n| format!("{}/{n}", wt.trim_end_matches('/')))
        .filter(|p| {
            std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        })
        .collect()
}

/// Deletes cache folder path, one of CACHE_NAMES directly in one of
/// worktrees (those nobody works in), unless anything in it changed at or
/// after not_since. why is said in the event log and the dry run's line;
/// by, when given, is the session it's logged for (`rush why`). It says
/// how much it freed.
pub fn remove_cache(
    path: &str,
    worktrees: &[String],
    not_since: Ms,
    why: &str,
    by: Option<&Agent>,
) -> Result<i64, String> {
    let r = (|| {
        let clean = crate::worktree::clean(path);
        let p = Path::new(&clean);
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
        let parent = p.parent().map(|d| d.to_string_lossy().into_owned());
        let in_free = parent
            .as_deref()
            .is_some_and(|d| worktrees.iter().any(|w| crate::worktree::clean(w) == d));
        if clean != path || !name.as_deref().is_some_and(|n| CACHE_NAMES.contains(&n)) || !in_free {
            return Err(format!(
                "won't delete {path}: it isn't a build cache in a worktree nobody works in"
            ));
        }
        if !std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
            return Err(format!("{path}: gone already"));
        }
        let (size, newest) = scratch_walk(path);
        if newest >= not_since {
            return Err(format!("{path}: changed lately; it stays"));
        }
        if rush_core::dryrun::would("remove-cache", path, why) {
            return Ok(0);
        }
        std::fs::remove_dir_all(p)
            .map(|_| size)
            .map_err(|e| e.to_string())
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
