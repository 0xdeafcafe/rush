//! The little of the fleet module the clean-up plugins use: the linked
//! worktrees of the repos sessions work in, how much disk a folder takes,
//! and which local branches have landed.
//! Ported from internal/fleet/worktree.go:101-197 and :427-461 (git,
//! FindWorktrees, mainCheckout), temp.go:53-70 and diskusage.go:14-36
//! (DiskUsage, statUsage), and verdict.go:205-245 (LandedBranches). Only
//! what the plugins read of a worktree is kept.

use std::collections::{BTreeSet, HashMap};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use rush_gh::ghpr::Status;
use rush_plugin::Ctx;

/// A linked git worktree an agent works in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    /// the main checkout it belongs to
    pub repo: String,
    pub branch: String,
    /// keys of the sessions working in it
    pub agents: Vec<String>,
    pub locked: bool,
}

/// A session as find_worktrees takes it: its key and its folder.
pub struct Agent {
    pub key: String,
    pub cwd: String,
}

/// git -C dir args, its output with the trailing newlines dropped, within
/// 20s; Err when it failed.
pub(crate) fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    // A look must not block the agent's own git.
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    let out = crate::exec::run(
        cmd,
        &Ctx::background(),
        Some(Duration::from_secs(20)),
        false,
    )
    .map_err(|e| e.to_string())?;
    if !out.success() {
        return Err("exit status".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim_end_matches('\n')
        .to_string())
}

/// The main checkout dir is in, walking up; "" for none. seen keeps what
/// each folder walked was found to be in.
fn main_checkout(dir: &str, seen: &mut HashMap<String, String>) -> String {
    let mut walked = Vec::new();
    let mut main = String::new();
    let mut d = dir.to_string();
    while !d.is_empty() && d != "/" && d != "." {
        if let Some(m) = seen.get(&d) {
            main = m.clone();
            break;
        }
        walked.push(d.clone());
        let p = Path::new(&d).join(".git");
        if let Ok(st) = std::fs::metadata(&p) {
            if st.is_dir() {
                main = d.clone();
                break;
            }
            let b = std::fs::read_to_string(&p).unwrap_or_default();
            let g = b.trim().trim_start_matches("gitdir:").trim();
            let g = if Path::new(g).is_absolute() {
                g.to_string()
            } else {
                Path::new(&d).join(g).to_string_lossy().into_owned()
            };
            if let Some(i) = g.find("/.git/worktrees/") {
                main = g[..i].to_string();
            }
            break;
        }
        d = Path::new(&d)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    if !main.is_empty() {
        for w in walked {
            seen.insert(w, main.clone());
        }
    }
    main
}

/// The linked worktrees of every repository the agents work in, each with
/// the agents working in it, by path.
pub fn find_worktrees(agents: &[Agent]) -> Vec<Worktree> {
    let mut seen = HashMap::new();
    let repos: BTreeSet<String> = agents
        .iter()
        .filter(|a| !a.cwd.is_empty())
        .map(|a| main_checkout(&a.cwd, &mut seen))
        .filter(|m| !m.is_empty())
        .collect();
    let mut out = Vec::new();
    for repo in repos {
        let Ok(list) = git(&repo, &["worktree", "list", "--porcelain"]) else {
            continue;
        };
        let mut w: Option<Worktree> = None;
        let mut flush = |w: &mut Option<Worktree>, out: &mut Vec<Worktree>| {
            if let Some(x) = w.take()
                && x.path != repo
            {
                out.push(x);
            }
        };
        for l in list.split('\n') {
            if let Some(p) = l.strip_prefix("worktree ") {
                flush(&mut w, &mut out);
                w = Some(Worktree {
                    path: p.to_string(),
                    repo: repo.clone(),
                    ..Default::default()
                });
                continue;
            }
            let Some(x) = w.as_mut() else {
                continue;
            };
            if let Some(b) = l.strip_prefix("branch ") {
                x.branch = b.trim_start_matches("refs/heads/").to_string();
            } else if l == "locked" || l.starts_with("locked ") {
                x.locked = true;
            } else if l == "prunable" || l.starts_with("prunable ") {
                w = None; // its folder is already gone
            }
        }
        flush(&mut w, &mut out);
    }
    for w in &mut out {
        for a in agents {
            let d = &a.cwd;
            if d.starts_with(&w.path)
                && (d.len() == w.path.len() || d.as_bytes()[w.path.len()] == b'/')
            {
                w.agents.push(a.key.clone());
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// How much disk the folder takes, counted as du does (allocated blocks),
/// without following links.
pub fn disk_usage(path: &str) -> i64 {
    let Ok(st) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    let mut n = st.blocks() as i64 * 512;
    if st.is_dir() {
        n += dir_usage(Path::new(path));
    }
    n
}

fn dir_usage(dir: &Path) -> i64 {
    let Ok(ents) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in ents.flatten() {
        let Ok(st) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        n += st.blocks() as i64 * 512;
        if st.is_dir() {
            n += dir_usage(&e.path());
        }
    }
    n
}

/// repo's local branches, checked out nowhere, whose work is on target: in
/// origin/<target> or <target>, or in the head of their merged pull
/// request (prs, by branch). Deleting them loses nothing. It runs git.
pub fn landed_branches(repo: &str, target: &str, prs: &HashMap<String, Status>) -> Vec<String> {
    let Ok(out) = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)\t%(worktreepath)",
            "refs/heads",
        ],
    ) else {
        return Vec::new();
    };
    let mut merged = std::collections::HashSet::new();
    for r in [
        format!("refs/remotes/origin/{target}"),
        format!("refs/heads/{target}"),
    ] {
        if let Ok(m) = git(
            repo,
            &[
                "for-each-ref",
                "--merged",
                &r,
                "--format=%(refname:short)",
                "refs/heads",
            ],
        ) {
            merged.extend(m.split('\n').map(String::from));
        }
    }
    let mut landed = Vec::new();
    for l in out.split('\n') {
        let (name, wt) = l.split_once('\t').unwrap_or((l, ""));
        if name.is_empty() || name == target || !wt.is_empty() {
            continue;
        }
        if !merged.contains(name) {
            let Some(pr) = prs.get(name) else {
                continue;
            };
            if !pr.merged() || pr.head_oid.is_empty() {
                continue;
            }
            let r = format!("refs/heads/{name}");
            if git(repo, &["merge-base", "--is-ancestor", &r, &pr.head_oid]).is_err() {
                continue;
            }
        }
        landed.push(name.to_string());
    }
    landed.sort();
    landed
}
