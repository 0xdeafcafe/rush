//! What agents wrote outside their projects and scratch, and the user's
//! own leftovers at the top of /tmp. Port of internal/fleet/left.go,
//! scratch.go and born_darwin.go.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use rush_core::adapter::Registry;
use rush_core::agent::{Agent, Left};
use rush_core::time::{Ms, now};
use serde_json::json;

use crate::hosted::{host_root, is_temp, temp_root};
use crate::temp::{agent_event, err_text, ms_of};

/// How long after an agent was last active something it wrote may still
/// change and count as its own: a clone or build it started finishes
/// after the call that made it.
const LEFT_SLACK: Ms = 5 * 60_000;

/// A place an agent wrote outside its project and scratch, as it is on
/// disk now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeftItem {
    pub path: String,
    pub agent: Agent,
    /// Read from a shell command: a guess.
    pub shell: bool,
    pub size: i64,
    /// In a system temp folder: scratch by nature.
    pub temp: bool,
    pub gone: bool,
    /// Changed since the agent was last active: someone else uses it now.
    pub touched: bool,
    /// git keeps it: part of some repository.
    pub tracked: bool,
    /// There from before the session began, or not known to be newer.
    pub before: bool,
    /// Home, a folder at its top or the disk's, or a temp folder itself.
    pub shallow: bool,
}

impl LeftItem {
    /// Whether it can go without asking more: scratch in a temp folder
    /// nothing has touched since a finished agent left it.
    pub fn safe(&self) -> bool {
        self.temp && self.why().is_empty() && self.agent.pid == 0
    }

    /// What holds it back from going, or "".
    pub fn why(&self) -> &'static str {
        if self.gone {
            "gone already"
        } else if self.shallow {
            "too near the top of the disk to delete from here"
        } else if self.before {
            "was there before the agent; it only wrote into it"
        } else if self.tracked {
            "git keeps it"
        } else if self.touched {
            "changed since the agent left it"
        } else {
            ""
        }
    }

    /// It, looked at afresh.
    pub fn again(&self) -> LeftItem {
        look_left_one(self.clone())
    }
}

/// Looks at what the agents wrote outside their projects, each place once
/// (the outermost, the latest writer's), on disk now: off the UI.
pub fn look_left(agents: &[Agent]) -> Vec<LeftItem> {
    let mut by_path: BTreeMap<&str, (&Left, &Agent)> = BTreeMap::new();
    for a in agents {
        for l in &a.left {
            match by_path.get(l.path.as_str()) {
                Some((r, _)) if l.at <= r.at => {}
                _ => {
                    by_path.insert(&l.path, (l, a));
                }
            }
        }
    }
    let mut out: Vec<LeftItem> = Vec::new();
    for (p, (l, a)) in by_path {
        if out.iter().any(|it| p.starts_with(&format!("{}/", it.path))) {
            continue; // inside one already listed
        }
        out.push(look_left_one(LeftItem {
            path: p.to_string(),
            agent: a.clone(),
            shell: l.shell,
            ..LeftItem::default()
        }));
    }
    out
}

/// Fills in what it is on disk now.
fn look_left_one(mut it: LeftItem) -> LeftItem {
    it.temp = is_temp(&it.path);
    it.shallow = shallow(&it.path);
    if std::fs::symlink_metadata(&it.path).is_err() {
        it.gone = true;
        return it;
    }
    let b = born(&it.path);
    let began = born(&host_root().join(&it.agent.job.id).to_string_lossy());
    it.before = match (b, began) {
        (Some(b), Some(began)) => b < began,
        _ => true,
    };
    let (size, newest) = scratch_walk(&it.path);
    it.size = size;
    it.touched = newest > it.agent.job.updated_at + LEFT_SLACK;
    it.tracked = tracked(&it.path);
    it
}

/// A path too near the top to delete from here, wherever an agent wrote:
/// /, home, their own folders (/opt, ~/Documents), and the temp folders.
fn shallow(p: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let tmp = crate::worktree::clean(&std::env::temp_dir().to_string_lossy());
    let troot = temp_root().to_string_lossy().into_owned();
    let tops = [
        "/",
        &home,
        "/tmp",
        "/private/tmp",
        "/var/folders",
        "/private/var/folders",
        &tmp,
        &troot,
    ];
    if tops.contains(&p) {
        return true;
    }
    let d = Path::new(p)
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into());
    d == "/" || d == home || d.ends_with("/var/folders") || d == "/Volumes"
}

/// Whether git keeps p, or anything in it.
fn tracked(p: &str) -> bool {
    let path = Path::new(p);
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-files", "--"])
        .arg(name)
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
}

/// Deletes it once looked at afresh, refusing what's running, gone, kept
/// by git or touched since.
pub fn remove_left(it: &LeftItem) -> Result<i64, String> {
    let r = (|| {
        if it.agent.pid != 0 {
            return Err(format!(
                "{} is still running; stop it first",
                it.agent.display_name
            ));
        }
        if !it.path.starts_with('/') || it.path == "/" {
            return Err(format!("won't delete {}", it.path));
        }
        let n = it.again();
        let why = n.why();
        if !why.is_empty() {
            return Err(format!("{}: {why}; it stays", it.path));
        }
        let p = Path::new(&it.path);
        let rm = if std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
        rm.map(|_| n.size).map_err(|e| e.to_string())
    })();
    agent_event(
        Some(&it.agent),
        "left-removed",
        json!({"path": it.path, "bytes": r.as_ref().copied().unwrap_or(0), "err": err_text(&r)}),
    );
    r
}

/// When p was made, where the file system says (to the nanosecond, as
/// the file system keeps it).
pub(crate) fn born(p: &str) -> Option<std::time::SystemTime> {
    let m = std::fs::symlink_metadata(p).ok()?;
    #[cfg(target_os = "macos")]
    {
        m.created().ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = m;
        None
    }
}

// ---- The top of /tmp (scratch.go) ----

/// How long something in /tmp goes untouched before it's taken as left
/// behind.
pub const SCRATCH_IDLE: Ms = 24 * 3_600_000;

/// What's yours at the top of /tmp. Stale is what nothing has touched,
/// anywhere inside, for SCRATCH_IDLE.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scratch {
    pub items: usize,
    pub stale_items: usize,
    pub size: i64,
    pub stale: i64,
    pub checked: Ms,
}

/// One thing of yours at the top of /tmp.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScratchItem {
    pub path: String,
    pub size: i64,
}

/// What clear_scratch would remove now, biggest first. Off the UI.
pub fn stale_scratch(reg: &Registry, dir: &str) -> Vec<ScratchItem> {
    let mut out: Vec<ScratchItem> = scratch_entries(reg, dir)
        .into_iter()
        .filter_map(|p| {
            let (size, newest) = scratch_walk(&p);
            (now() - newest >= SCRATCH_IDLE).then_some(ScratchItem { path: p, size })
        })
        .collect();
    out.sort_by_key(|i| std::cmp::Reverse(i.size));
    out
}

/// Walks the user's entries at the top of dir (/tmp). Off the UI.
pub fn find_scratch(reg: &Registry, dir: &str) -> Scratch {
    let mut s = Scratch {
        checked: now(),
        ..Scratch::default()
    };
    for p in scratch_entries(reg, dir) {
        let (size, newest) = scratch_walk(&p);
        s.items += 1;
        s.size += size;
        if now() - newest >= SCRATCH_IDLE {
            s.stale_items += 1;
            s.stale += size;
        }
    }
    s
}

/// Removes what's stale, looking again at each first: anything touched
/// since it was counted stays. It says how much it freed and how many.
pub fn clear_scratch(reg: &Registry, dir: &str) -> (i64, usize, Option<String>) {
    let (mut freed, mut n, mut err) = (0, 0, None);
    for p in scratch_entries(reg, dir) {
        let (size, newest) = scratch_walk(&p);
        if now() - newest < SCRATCH_IDLE {
            continue;
        }
        let path = Path::new(&p);
        let r = if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(e) = r {
            err.get_or_insert(e.to_string());
            continue;
        }
        freed += size;
        n += 1;
    }
    let mut f = json!({"dir": dir, "removed": n, "bytes": freed});
    if let Some(e) = &err {
        f["err"] = json!(e);
    }
    rush_sys::events::log("scratch-cleared", f);
    (freed, n, err)
}

/// The files and folders at the top of dir the user owns, but for agents'
/// own scratch roots (cleaned with their sessions), tmux's, and anything
/// that isn't a plain file or folder.
fn scratch_entries(reg: &Registry, dir: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc::getuid() };
    let own: Vec<String> = reg
        .all()
        .iter()
        .filter_map(|a| a.scratcher().map(|s| s.scratch_root()))
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            Path::new(&r)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if own.contains(&name) || name.starts_with("tmux-") {
            continue;
        }
        let Ok(m) = e.metadata() else { continue };
        let t = m.file_type();
        if !(t.is_file() || t.is_dir()) || m.uid() != uid {
            continue;
        }
        out.push(format!("{}/{name}", dir.trim_end_matches('/')));
    }
    out.sort();
    out
}

/// How much disk p takes and when anything in it last changed.
pub(crate) fn scratch_walk(p: &str) -> (i64, Ms) {
    let (mut size, mut newest) = (0i64, 0);
    let mut stack = vec![p.to_string()];
    while let Some(q) = stack.pop() {
        let Ok(m) = std::fs::symlink_metadata(&q) else {
            continue;
        };
        if let Ok(t) = m.modified() {
            newest = newest.max(ms_of(t));
        }
        size += m.blocks() as i64 * 512;
        if m.is_dir()
            && let Ok(rd) = std::fs::read_dir(&q)
        {
            stack.extend(
                rd.flatten()
                    .map(|e| e.path().to_string_lossy().into_owned()),
            );
        }
    }
    (size, newest)
}
