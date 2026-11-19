//! The cargo targets built for a worktree that live outside it, tied back
//! to it so its size says what it really takes. Sessions build with
//! CARGO_TARGET_DIR in their scratch (tmp/<project>/<session>/<name>-target),
//! and nothing in the worktree names them. Rust-only: Go measured the
//! worktree's folder alone (internal/fleet/worktree.go:27 Size).
//!
//! A target goes to one worktree at most, by the first of:
//! 1. a process working in that worktree names it as CARGO_TARGET_DIR;
//! 2. the session whose scratch holds it works in that worktree alone;
//! 3. its newest dep-info (.d) file naming one of the session's
//!    worktrees (any worktree, for a session rush doesn't know in one);
//! 4. its name: hud3-target for the session's one worktree whose name
//!    has hud3 as a dash-separated part.
//!
//! Anything else stays untied, counted toward no worktree.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use rush_core::agent::Agent;

use crate::hosted::{temp_dir, temp_roots};
use crate::worktree::{Worktree, clean};

/// Whether dir is a cargo target: cargo writes both into every one.
pub fn is_target(dir: &Path) -> bool {
    dir.join("CACHEDIR.TAG").is_file() && dir.join(".rustc_info.json").is_file()
}

/// A target a process names: the folder it works in and its
/// CARGO_TARGET_DIR, made absolute against it.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcTarget {
    pub cwd: String,
    pub target: String,
}

/// A target in a session's scratch, and the key of the agent whose
/// scratch it is ("" when rush lists no such session now).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScratchTarget {
    pub path: String,
    pub owner: String,
}

/// Ties each target sessions built for the worktrees into their targets:
/// the scratch root's and the agents' scratch, and what processes working
/// in them name. Reads the disk and the process table: off the UI.
pub fn tie_targets(wts: &mut [Worktree], agents: &[Agent]) {
    let dirs: Vec<String> = wts.iter().map(|w| w.path.clone()).collect();
    let procs = process_targets(&dirs);
    let scratch = scratch_targets(agents);
    tie_targets_with(wts, &scratch, &procs);
}

/// tie_targets with the scratch's targets and the processes' as given.
pub fn tie_targets_with(wts: &mut [Worktree], scratch: &[ScratchTarget], procs: &[ProcTarget]) {
    for w in wts.iter_mut() {
        w.targets.clear();
    }
    let paths: Vec<String> = wts.iter().map(|w| w.path.clone()).collect();
    let inside_any = |t: &str| wts.iter().any(|w| within(t, &w.path) || within(t, &w.repo));
    let mut tied: HashMap<String, usize> = HashMap::new();
    // 1. Named by processes working in one worktree, and none elsewhere
    // among them.
    let mut named: HashMap<String, Vec<usize>> = HashMap::new();
    for p in procs {
        let t = clean(&p.target);
        let at = paths.iter().position(|w| within(&p.cwd, w));
        let e = named.entry(t).or_default();
        match at {
            Some(i) if !e.contains(&i) => e.push(i),
            Some(_) => {}
            None => e.push(usize::MAX),
        }
    }
    for (t, ws) in named {
        if let [i] = ws[..]
            && i != usize::MAX
            && !inside_any(&t)
            && is_target(Path::new(&t))
        {
            tied.insert(t, i);
        }
    }
    // 2-4. What's in sessions' scratch.
    let all: Vec<usize> = (0..wts.len()).collect();
    for s in scratch {
        let t = clean(&s.path);
        if tied.contains_key(&t) || inside_any(&t) {
            continue;
        }
        let key = (!s.owner.is_empty()).then_some(s.owner.as_str());
        let theirs: Vec<usize> = match key {
            Some(k) => all
                .iter()
                .copied()
                .filter(|&i| wts[i].agents.iter().any(|a| a == k))
                .collect(),
            None => Vec::new(),
        };
        let pick = match theirs[..] {
            [i] => Some(i),
            [] => {
                // A session rush no longer lists, or one whose work in
                // worktrees it doesn't know: only what cargo wrote says.
                let cands: Vec<&str> = all.iter().map(|&i| paths[i].as_str()).collect();
                built_for(&t, &cands)
            }
            _ => {
                let cands: Vec<&str> = theirs.iter().map(|&i| paths[i].as_str()).collect();
                built_for(&t, &cands)
                    .map(|j| theirs[j])
                    .or_else(|| by_name(&t, &cands).map(|j| theirs[j]))
            }
        };
        if let Some(i) = pick {
            tied.insert(t, i);
        }
    }
    let mut tied: Vec<(String, usize)> = tied.into_iter().collect();
    tied.sort();
    for (t, i) in tied {
        wts[i].targets.push(t);
    }
}

/// Whether d is path or under it.
fn within(d: &str, path: &str) -> bool {
    !path.is_empty()
        && d.starts_with(path)
        && (d.len() == path.len() || d.as_bytes()[path.len()] == b'/')
}

/// The targets in sessions' scratch folders: two below each scratch root
/// (<root>/<project>/<session>/<name>-target), and in each rush agent's own
/// (wherever its tmp link points), each with the agent whose it is.
pub fn scratch_targets(agents: &[Agent]) -> Vec<ScratchTarget> {
    let mut out: Vec<ScratchTarget> = Vec::new();
    for a in agents.iter().filter(|a| a.rush && !a.job.id.is_empty()) {
        targets_in(Path::new(&temp_dir(&a.job.id)), &a.key, &mut out);
    }
    // Each scratch root: the real /tmp's for new sessions, rush's folder's
    // for older ones.
    for root in temp_roots() {
        let Ok(projects) = std::fs::read_dir(root) else {
            continue;
        };
        for p in projects.flatten() {
            let Ok(sessions) = std::fs::read_dir(p.path()) else {
                continue;
            };
            for s in sessions.flatten() {
                if !s.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let id = s.file_name().to_string_lossy().into_owned();
                let owner = agents
                    .iter()
                    .find(|a| a.rush && a.job.id == id)
                    .map_or("", |a| a.key.as_str());
                targets_in(&s.path(), owner, &mut out);
            }
        }
    }
    out
}

/// The targets directly in dir, owner's, into out once each.
fn targets_in(dir: &Path, owner: &str, out: &mut Vec<ScratchTarget>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) || !is_target(&e.path()) {
            continue;
        }
        let path = e.path().to_string_lossy().into_owned();
        if !out.iter().any(|s| s.path == path) {
            out.push(ScratchTarget {
                path,
                owner: owner.to_string(),
            });
        }
    }
}

/// Each CARGO_TARGET_DIR named by a process of this user working in one
/// of dirs (or under it). Only those processes' environments are read.
pub fn process_targets(dirs: &[String]) -> Vec<ProcTarget> {
    if dirs.is_empty() {
        return Vec::new();
    }
    let t = rush_sys::proc::snapshot();
    let mut out = Vec::new();
    for &pid in t.procs.keys() {
        let Some(cwd) = rush_sys::proc::cwd(pid) else {
            continue;
        };
        let cwd = cwd.to_string_lossy().into_owned();
        if !dirs.iter().any(|d| within(&cwd, d)) {
            continue;
        }
        for kv in rush_sys::proc::env(pid) {
            if let Some(v) = kv.strip_prefix("CARGO_TARGET_DIR=")
                && !v.is_empty()
            {
                let target = if v.starts_with('/') {
                    v.to_string()
                } else {
                    format!("{cwd}/{v}")
                };
                out.push(ProcTarget {
                    cwd: cwd.clone(),
                    target: clean(&target),
                });
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Whether anything uses target now: a process working in it, or one
/// naming it in its arguments or environment (cargo and rustc carry
/// CARGO_TARGET_DIR down). Asked right before a removal.
pub(crate) fn target_in_use(target: &str) -> bool {
    let t = rush_sys::proc::snapshot();
    let me = std::process::id() as i32;
    t.procs.keys().filter(|&&p| p != me).any(|&pid| {
        rush_sys::proc::cwd(pid).is_some_and(|c| within(&c.to_string_lossy(), target))
            || rush_sys::proc::args(pid).iter().any(|a| a.contains(target))
            || rush_sys::proc::env(pid).iter().any(|e| e.contains(target))
    })
}

/// At most this many dep-info files are read of a target, newest first.
const DEP_INFO_MAX: usize = 4000;

/// What a target's dep-info last said, while its deps folders are as
/// they were: (target, candidates) → (stamp, the candidate it names).
type Said = HashMap<(String, String), (Vec<SystemTime>, Option<usize>)>;

static SAID: Mutex<Option<Said>> = Mutex::new(None);

/// Which of cands (worktree paths) target's newest dep-info file naming
/// any of them names: cargo writes the sources each crate was built from
/// into deps/*.d (a workspace's own crates relative, so not always).
fn built_for(target: &str, cands: &[&str]) -> Option<usize> {
    if cands.is_empty() {
        return None;
    }
    let deps: Vec<std::path::PathBuf> = std::fs::read_dir(target)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("deps"))
        .filter(|d| d.is_dir())
        .collect();
    let stamp: Vec<SystemTime> = deps
        .iter()
        .filter_map(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok())
        .collect();
    let key = (target.to_string(), cands.join("\n"));
    if let Some((s, hit)) = SAID
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|c| c.get(&key))
        && *s == stamp
    {
        return *hit;
    }
    let mut files: Vec<(SystemTime, std::path::PathBuf)> = Vec::new();
    for d in &deps {
        let Ok(rd) = std::fs::read_dir(d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "d")
                && let Ok(t) = e.metadata().and_then(|m| m.modified())
            {
                files.push((t, p));
            }
        }
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let needles: Vec<String> = cands.iter().map(|c| format!("{c}/")).collect();
    let mut hit = None;
    for (_, f) in files.iter().take(DEP_INFO_MAX) {
        let Ok(text) = std::fs::read(f) else {
            continue;
        };
        let text = String::from_utf8_lossy(&text[..text.len().min(1 << 16)]);
        if let Some(i) = needles.iter().position(|n| text.contains(n.as_str())) {
            hit = Some(i);
            break;
        }
    }
    let mut c = SAID.lock().unwrap_or_else(|e| e.into_inner());
    let c = c.get_or_insert_with(HashMap::new);
    if c.len() > 1024 {
        c.clear();
    }
    c.insert(key, (stamp, hit));
    hit
}

/// The one of cands whose folder name has target's name (less -target)
/// as itself or a dash-separated part.
fn by_name(target: &str, cands: &[&str]) -> Option<usize> {
    let name = target.rsplit('/').next()?;
    let n = name.strip_suffix("-target")?;
    if n.is_empty() {
        return None;
    }
    let hits: Vec<usize> = cands
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let b = c.rsplit('/').next().unwrap_or(c);
            b == n || b.split('-').any(|p| p == n)
        })
        .map(|(i, _)| i)
        .collect();
    match hits[..] {
        [i] => Some(i),
        _ => None,
    }
}
