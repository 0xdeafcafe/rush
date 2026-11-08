//! Rush's own scratch that no session owns: what's under the sessions'
//! scratch root (tmp/) but isn't tmp/<project>/<a known session's id>. A
//! script writing "$TMPDIR$name" before TMPDIR ended in "/" left
//! tmp/<project>/<id><name> beside the session's folder; files dropped in
//! tmp/<project>/, folders made at the top of tmp/, and the folders of
//! sessions since deleted are the rest. Rust-only: Go's layout
//! (internal/host/tmp.go) never looked for them. Looked at off the UI, as
//! left.rs and temp.rs are.

use std::collections::HashSet;
use std::path::Path;

use rush_core::time::Ms;
use serde_json::json;

use crate::hosted::{host_root, temp_root};
use crate::left::scratch_walk;
use crate::temp::err_text;

/// How long something no session owns sits untouched before it's taken as
/// left behind: days, as a script still writing there would have touched it.
pub const ORPHAN_IDLE: Ms = 3 * 24 * 3_600_000;

/// Something under rush's scratch root no session owns, as last looked at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TempOrphan {
    pub path: String,
    pub size: i64,
    /// When anything in it last changed.
    pub newest: Ms,
    /// The known session whose id its name starts with, the one it
    /// probably came from; "" when none.
    pub session: String,
    pub gone: bool,
}

impl TempOrphan {
    /// Whether nothing in it has changed for ORPHAN_IDLE.
    pub fn idle(&self, now: Ms) -> bool {
        now - self.newest >= ORPHAN_IDLE
    }

    /// Whether a running session (one of their ids) may still use it: its
    /// name starts with one's id.
    pub fn used_by<'a>(&self, running: &'a [String]) -> Option<&'a str> {
        let name = base(&self.path);
        running
            .iter()
            .map(String::as_str)
            .find(|id| !id.is_empty() && name.starts_with(id))
    }

    /// It, looked at afresh.
    pub fn again(&self) -> TempOrphan {
        look(&self.path, &known_sessions())
    }
}

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// The ids of the sessions rush knows: a folder each in its sessions folder.
fn known_sessions() -> HashSet<String> {
    let Ok(rd) = std::fs::read_dir(host_root()) else {
        return HashSet::new();
    };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Whether name is a project's folder as rush names them (rush-host's
/// tmp::slug): its name, "-" and 8 hex digits of its path's hash.
fn is_project_slug(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() > 9
        && b[b.len() - 9] == b'-'
        && b[b.len() - 8..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// Whether p is a session's own folder: tmp/<project>/<id> of a session
/// rush knows.
fn owned(p: &Path, known: &HashSet<String>) -> bool {
    p.file_name()
        .is_some_and(|n| known.contains(n.to_string_lossy().as_ref()))
        && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// Whether clean path p is something no session owns, where only those
/// are: directly in the scratch root and not a project's folder, or
/// directly in a project's folder and not a known session's own. Never
/// the scratch root or a project's folder itself.
fn orphan_at(p: &Path, known: &HashSet<String>) -> bool {
    let root = temp_root();
    let Some(parent) = p.parent() else {
        return false;
    };
    let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return false;
    };
    if parent == root {
        // A project's folder never is; anything else at the top is.
        let dir = std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
        return !dir || !is_project_slug(&name);
    }
    parent.parent() == Some(root.as_path())
        && parent
            .file_name()
            .is_some_and(|n| is_project_slug(&n.to_string_lossy()))
        && !owned(p, known)
}

/// What it is on disk now.
fn look(path: &str, known: &HashSet<String>) -> TempOrphan {
    let name = base(path);
    let session = known
        .iter()
        .filter(|id| !id.is_empty() && name.starts_with(id.as_str()))
        .max_by_key(|id| id.len())
        .cloned()
        .unwrap_or_default();
    if std::fs::symlink_metadata(path).is_err() {
        return TempOrphan {
            path: path.to_string(),
            session,
            gone: true,
            ..TempOrphan::default()
        };
    }
    let (size, newest) = scratch_walk(path);
    TempOrphan {
        path: path.to_string(),
        size,
        newest,
        session,
        gone: false,
    }
}

/// Everything under rush's scratch root no session owns, each with its
/// size and when anything in it last changed, oldest first. Off the UI.
pub fn find_temp_orphans() -> Vec<TempOrphan> {
    let known = known_sessions();
    let root = temp_root();
    let mut paths = Vec::new();
    for e in sorted(&root) {
        let p = root.join(&e);
        if orphan_at(&p, &known) {
            paths.push(p);
            continue;
        }
        if !std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        for c in sorted(&p) {
            let q = p.join(&c);
            if orphan_at(&q, &known) {
                paths.push(q);
            }
        }
    }
    let mut out: Vec<TempOrphan> = paths
        .iter()
        .map(|p| look(&p.to_string_lossy(), &known))
        .collect();
    out.sort_by_key(|o| o.newest);
    out
}

fn sorted(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Deletes it once looked at afresh: only something no session owns, in
/// rush's scratch root (never the root or a project's folder), that no
/// running session's id (running) prefixes, and that hasn't changed since
/// it was looked at. It says how much it freed.
pub fn remove_temp_orphan(o: &TempOrphan, running: &[String]) -> Result<i64, String> {
    let r = (|| {
        let clean = crate::worktree::clean(&o.path);
        let p = Path::new(&clean);
        if !o.path.starts_with('/') || clean != o.path || !orphan_at(p, &known_sessions()) {
            return Err(format!(
                "won't delete {}: it isn't scratch no session owns",
                o.path
            ));
        }
        if let Some(id) = o.used_by(running) {
            return Err(format!(
                "{}: session {id} is running and may use it; it stays",
                o.path
            ));
        }
        let n = o.again();
        if n.gone {
            return Err(format!("{}: gone already", o.path));
        }
        if n.newest != o.newest {
            return Err(format!(
                "{}: changed since it was looked at; it stays",
                o.path
            ));
        }
        let why = if o.session.is_empty() {
            "no session owns it".to_string()
        } else {
            format!("no session owns it (from {}'s, probably)", o.session)
        };
        if rush_core::dryrun::would("remove-temp-orphan", &o.path, &why) {
            return Ok(0);
        }
        let rm = if std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
        rm.map(|_| n.size).map_err(|e| e.to_string())
    })();
    rush_sys::events::log(
        "temp-orphan-removed",
        json!({
            "path": o.path,
            "session": o.session,
            "bytes": r.as_ref().copied().unwrap_or(0),
            "err": err_text(&r),
        }),
    );
    r
}
