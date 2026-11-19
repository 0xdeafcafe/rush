//! The candidates the engine finds itself: cargo targets at each
//! worktree's and main checkout's target and SCAN_DEPTH under the scratch
//! roots, the linked worktrees git lists, and the folders cache-dir rules
//! name, each with the facts that decide it. Ported from rush-bundled's
//! clean/kits.rs rust_auto (roots, scratch, which session a target is
//! working for), reap.rs look, merged and last_touched (worktrees), and
//! this crate's rules.rs (cache folders, each).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rush_core::time::Ms;

use super::cargo::{SCAN_DEPTH, artifacts, find_targets, target_activity};
use super::{CACHE_DIR, CARGO_TARGET, Candidate, Facts, Kind, Level, Rule, WORKTREE};
use crate::temp::ms_of;
use crate::worktree::{git, git_within};

/// A session, as the engine needs it: where it works, and whether it's
/// working now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub cwd: PathBuf,
    pub working: bool,
}

/// Where the engine looks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Folders in the repos to look at (sessions' cwds): each repo's
    /// worktrees, its main checkout among them, are found with git.
    pub repos: Vec<PathBuf>,
    /// Where targets are looked for SCAN_DEPTH down: sessions' scratch
    /// roots (crate::temp_roots, rush_state::scratch_roots: the system
    /// tmp's rush/ and rush's old tmp/), where they build with
    /// CARGO_TARGET_DIR.
    pub scratch: Vec<PathBuf>,
    pub sessions: Vec<Session>,
    /// The home folder: ~ in cache-dir rules.
    pub home: String,
    /// The folders processes work in (sweep::process_cwds); None when
    /// that can't be told, which keeps every worktree.
    pub cwds: Option<Vec<String>>,
}

/// One entry of git worktree list --porcelain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitWorktree {
    pub path: PathBuf,
    /// Its branch, refs/heads/ left off; "" when detached.
    pub branch: String,
    pub locked: bool,
    /// git says it can be pruned: its folder is gone.
    pub prunable: bool,
    pub bare: bool,
    /// The first entry: the main checkout.
    pub main: bool,
}

/// git worktree list --porcelain's entries.
pub fn parse_worktrees(porcelain: &str) -> Vec<GitWorktree> {
    let mut out: Vec<GitWorktree> = Vec::new();
    for l in porcelain.lines() {
        if let Some(p) = l.strip_prefix("worktree ") {
            out.push(GitWorktree {
                path: PathBuf::from(p),
                main: out.is_empty(),
                ..GitWorktree::default()
            });
            continue;
        }
        let Some(w) = out.last_mut() else {
            continue;
        };
        let word = l.split(' ').next().unwrap_or("");
        match word {
            "branch" => {
                let b = l["branch ".len()..].trim();
                w.branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
            }
            "locked" => w.locked = true,
            "prunable" => w.prunable = true,
            "bare" => w.bare = true,
            _ => {}
        }
    }
    out
}

/// The main checkout of the repo dir is in; None when it's in none.
fn repo_of(dir: &Path) -> Option<PathBuf> {
    let common = git(
        &dir.to_string_lossy(),
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    Path::new(common.trim()).parent().map(Path::to_path_buf)
}

/// The session p is for: one whose cwd it's in, or whose id is a folder
/// on its path (a scratch target, tmp/<project>/<id>/…), a working one
/// first.
fn session_of<'a>(p: &Path, sessions: &'a [Session]) -> Option<&'a Session> {
    let ours = |s: &&Session| {
        !s.cwd.as_os_str().is_empty() && p.starts_with(&s.cwd)
            || !s.id.is_empty() && p.components().any(|c| c.as_os_str() == s.id.as_str())
    };
    sessions
        .iter()
        .filter(ours)
        .find(|s| s.working)
        .or_else(|| sessions.iter().find(ours))
}

/// Every candidate the engine finds, for the rules that run at lv, sorted
/// by path: targets when a cargo rule runs, worktrees when the worktree
/// rule does, each cache-dir rule's folders.
pub fn find_candidates(scan: &Scan, rules: &[Rule], lv: Level, now: Ms) -> Vec<Candidate> {
    let runs =
        |is: &dyn Fn(&Kind) -> bool| rules.iter().filter(|r| is(&r.kind)).find_map(|r| r.at(lv));
    let targets_run = runs(&|k| matches!(k, Kind::CargoTarget | Kind::CargoArtifacts)).is_some();
    let wt_idle = runs(&|k| *k == Kind::Worktree).map(|p| p.idle_secs.unwrap_or(0));

    let mut repos: BTreeSet<PathBuf> = BTreeSet::new();
    for d in &scan.repos {
        if let Some(r) = repo_of(d) {
            repos.insert(r);
        }
    }
    let mut wts: Vec<(PathBuf, GitWorktree)> = Vec::new();
    for r in &repos {
        let Ok(out) = git(&r.to_string_lossy(), &["worktree", "list", "--porcelain"]) else {
            continue;
        };
        for w in parse_worktrees(&out) {
            if !w.bare && !w.prunable {
                wts.push((r.clone(), w));
            }
        }
    }

    let mut out = Vec::new();
    if targets_run {
        let mut targets = BTreeSet::new();
        for (_, w) in &wts {
            find_targets(&w.path.join("target"), 0, &mut targets);
        }
        for s in &scan.scratch {
            find_targets(s, SCAN_DEPTH, &mut targets);
        }
        for t in targets {
            let s = session_of(&t, &scan.sessions);
            let worktree = wts
                .iter()
                .map(|(_, w)| &w.path)
                .filter(|w| t.starts_with(w))
                .max_by_key(|w| w.as_os_str().len())
                .map(|w| w.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(Candidate {
                kind: CARGO_TARGET.into(),
                facts: Facts {
                    last_build: Some(target_activity(&t)),
                    artifacts: artifacts(&t),
                    session: s.map(|s| s.id.clone()).unwrap_or_default(),
                    working: s.is_some_and(|s| s.working),
                    worktree,
                    ..Facts::default()
                },
                path: t,
                ..Candidate::default()
            });
        }
    }
    if let Some(idle) = wt_idle {
        let mut bases: HashMap<PathBuf, String> = HashMap::new();
        for (repo, w) in wts.iter().filter(|(_, w)| !w.main) {
            let base = bases
                .entry(repo.clone())
                .or_insert_with(|| {
                    git(
                        &repo.to_string_lossy(),
                        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
                    )
                    .unwrap_or_default()
                })
                .clone();
            out.push(look(scan, repo, w, &base, now, idle));
        }
    }
    for r in rules {
        if let (
            Kind::CacheDir {
                path, why, each, ..
            },
            Some(_),
        ) = (&r.kind, r.at(lv))
        {
            for p in cache_dirs(path, *each, &scan.home) {
                let (size, newest) = crate::walk(&p.to_string_lossy());
                if size <= 0 {
                    continue;
                }
                out.push(Candidate {
                    path: p,
                    kind: CACHE_DIR.into(),
                    rule: r.id.clone(),
                    why: why.clone(),
                    bytes_on_disk: size as u64,
                    facts: Facts {
                        mtime: Some(newest),
                        ..Facts::default()
                    },
                    ..Candidate::default()
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then(a.kind.cmp(&b.kind)));
    out
}

/// A name an each-rule takes: a workspace's hash.
fn hex_name(n: &str) -> bool {
    n.len() >= 6 && n.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// The folders a cache-dir rule's path names for home: ~ made home, ""
/// Go's module cache; with each, the hex-named folders in it. Only
/// folders, never links.
pub fn cache_dirs(path: &str, each: bool, home: &str) -> Vec<PathBuf> {
    let h = home.trim_end_matches('/');
    let p = if path.is_empty() {
        match crate::rules::go_mod_cache(home) {
            Some(p) => PathBuf::from(p),
            None => return Vec::new(),
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if h.is_empty() {
            return Vec::new();
        }
        Path::new(h).join(rest)
    } else {
        PathBuf::from(path)
    };
    if !p.is_absolute() {
        return Vec::new();
    }
    if !each {
        return if is_dir(&p) { vec![p] } else { Vec::new() };
    }
    let mut out: Vec<PathBuf> = std::fs::read_dir(&p)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| hex_name(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .filter(|p| is_dir(p))
        .collect();
    out.sort();
    out
}

/// A linked worktree's facts. It stops at its idle time when that's short
/// of the rule's idle, since the rest can't change the answer.
fn look(scan: &Scan, repo: &Path, w: &GitWorktree, base: &str, now: Ms, idle: u64) -> Candidate {
    let s = session_of(&w.path, &scan.sessions);
    let since = now - (idle as Ms).saturating_mul(1000);
    let last = last_touched(&w.path, since);
    let mut f = Facts {
        mtime: Some(last),
        locked: w.locked,
        repo: Some(repo.to_path_buf()),
        session: s.map(|s| s.id.clone()).unwrap_or_default(),
        working: s.is_some_and(|s| s.working),
        worktree: w.path.to_string_lossy().into_owned(),
        ..Facts::default()
    };
    let c = |f: Facts| Candidate {
        path: w.path.clone(),
        kind: WORKTREE.into(),
        facts: f,
        ..Candidate::default()
    };
    if last > since || f.working {
        return c(f);
    }
    let dir = w.path.to_string_lossy();
    f.changed = git(&dir, &["status", "--porcelain", "--untracked-files=normal"])
        .ok()
        .map(|s| s.lines().filter(|l| !l.is_empty()).count());
    if let Some(cwds) = &scan.cwds {
        let real = std::fs::canonicalize(&w.path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| dir.to_string());
        f.held = Some(
            cwds.iter()
                .filter(|c| **c == real || c.starts_with(&format!("{real}/")))
                .count(),
        );
    }
    f.merged = merged(repo, w, base);
    c(f)
}

fn ok(dir: &Path, args: &[&str]) -> bool {
    git(&dir.to_string_lossy(), args).is_ok()
}

/// How a worktree's commits are kept, "" when they aren't: its branch is
/// in the default branch, or its upstream is gone and every patch is in
/// it, or it's detached at a commit the default branch has.
fn merged(repo: &Path, w: &GitWorktree, base: &str) -> String {
    if base.is_empty() {
        return String::new();
    }
    if w.branch.is_empty() {
        if ok(&w.path, &["merge-base", "--is-ancestor", "HEAD", base]) {
            return format!("detached at a commit in {base}");
        }
        return String::new();
    }
    let head = format!("refs/heads/{}", w.branch);
    if ok(repo, &["merge-base", "--is-ancestor", &head, base]) {
        return format!("{} is merged into {base}", w.branch);
    }
    // A gone upstream alone isn't enough: every commit's patch must be in
    // base (squash merges).
    let r = repo.to_string_lossy();
    if git(&r, &["for-each-ref", "--format=%(upstream:track)", &head]).is_ok_and(|t| t == "[gone]")
        && git(&r, &["cherry", base, &head]).is_ok_and(|out| !format!("\n{out}").contains("\n+"))
    {
        return format!(
            "{}'s upstream is gone and its changes are in {base}",
            w.branch
        );
    }
    String::new()
}

/// When anything in a worktree last changed, outside node_modules, and its
/// git index and HEAD; it stops at the first change after since, as that
/// settles it.
pub fn last_touched(path: &Path, since: Ms) -> Ms {
    let mut last: Ms = 0;
    let mut touch = |t: SystemTime| {
        let t = ms_of(t);
        last = last.max(t);
        t > since
    };
    if let Ok(b) = std::fs::read_to_string(path.join(".git")) {
        let gd = b.trim().trim_start_matches("gitdir:").trim().to_string();
        let gd = if Path::new(&gd).is_absolute() {
            PathBuf::from(&gd)
        } else {
            path.join(gd)
        };
        for n in ["index", "HEAD", "logs/HEAD"] {
            if let Ok(t) = std::fs::metadata(gd.join(n)).and_then(|m| m.modified())
                && touch(t)
            {
                return last;
            }
        }
    }
    walk_until(path, path, &mut touch);
    last
}

/// Walks p in lexical order until touch says stop.
fn walk_until(root: &Path, p: &Path, touch: &mut impl FnMut(SystemTime) -> bool) -> bool {
    let Ok(st) = std::fs::symlink_metadata(p) else {
        return false;
    };
    if st.is_dir() && p != root && p.file_name().is_some_and(|n| n == "node_modules") {
        return false;
    }
    if let Ok(t) = st.modified()
        && touch(t)
    {
        return true;
    }
    if !st.is_dir() {
        return false;
    }
    let Ok(ents) = std::fs::read_dir(p) else {
        return false;
    };
    let mut names: Vec<_> = ents.flatten().map(|e| e.path()).collect();
    names.sort();
    names.iter().any(|n| walk_until(root, n, touch))
}

/// git worktree remove (never forced: git refuses one dirty or locked),
/// not stopped halfway, then git worktree prune.
pub(crate) fn remove_worktree_git(repo: &Path, path: &Path) -> Result<(), String> {
    let r = repo.to_string_lossy();
    git_within(None, &r, &["worktree", "remove", &path.to_string_lossy()])?;
    git(&r, &["worktree", "prune"]).map(|_| ())
}
