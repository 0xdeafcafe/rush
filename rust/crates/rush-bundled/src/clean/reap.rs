//! git-clean's reaping of worktrees no longer needed: clean, merged, idle
//! a while, and nothing running there. Each that's stale is listed, with
//! why it's kept when it is; only one you pick goes.
//! Ported from internal/bundled/clean/reap.go:1-264.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rush_plugin::{Ctx, Manifest, SettingSpec};

use super::fleet::Worktree;
use super::{Clean, Session, Thing, command, plural, worktrees};

/// git-clean's: how long a worktree sits untouched before it's offered for
/// removal.
pub(crate) fn git_settings() -> Vec<SettingSpec> {
    vec![SettingSpec {
        key: "worktree-idle-days".into(),
        title: "Worktrees idle for (days)".into(),
        kind: "choice".into(),
        choices: ["1", "3", "7", "14", "30"].map(String::from).to_vec(),
        default: "3".into(),
        description: "Days with no file changed before a clean, merged worktree nothing runs in is offered for removal".into(),
    }]
}

const DAY: Duration = Duration::from_secs(24 * 3600);

/// The idle time setting, read afresh each time it's asked.
fn reap_after() -> Duration {
    let m = Manifest {
        name: "git-clean".into(),
        settings: git_settings(),
        ..Default::default()
    };
    let v = rush_plugin::setting_values_of(&m)
        .remove("worktree-idle-days")
        .unwrap_or_default();
    let n = match v.parse::<u64>() {
        Ok(n) if n >= 1 => n,
        _ => 3,
    };
    DAY * n as u32
}

/// What decides whether a worktree can go: gathered by look, judged by
/// refusals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReapFacts {
    /// the repo's own checkout
    pub main: bool,
    pub locked: bool,
    /// a check that failed
    pub err: String,
    /// uncommitted files, untracked ones included; ignored ones not
    pub changed: usize,
    /// processes working in it; -1 when that can't be told
    pub held: i64,
    /// how its work is kept: "" when it isn't
    pub merged: String,
    pub idle: Duration,
}

/// The reasons a worktree stays, none when it can go. The repo's own
/// checkout always stays.
pub fn refusals(f: &ReapFacts, idle_for: Duration) -> Vec<String> {
    if f.main {
        return vec!["the main checkout".into()];
    }
    let mut why = Vec::new();
    if !f.err.is_empty() {
        why.push(f.err.clone());
    }
    if f.locked {
        why.push("locked".into());
    }
    if f.changed > 0 {
        why.push(plural(f.changed, "uncommitted file", "uncommitted files"));
    }
    if f.held < 0 {
        why.push("can't tell what runs there".into());
    } else if f.held > 0 {
        why.push(format!(
            "{} working there",
            plural(f.held as usize, "process", "processes")
        ));
    }
    if f.merged.is_empty() {
        why.push("its commits aren't all in the default branch".into());
    }
    if f.idle < idle_for {
        why.push(format!("touched {} ago", days(f.idle)));
    }
    why
}

fn days(d: Duration) -> String {
    format!("{}d", d.as_secs() / (24 * 3600))
}

/// The stale worktrees of the repos sessions were seen in: those that can
/// go, and those idle but kept, with why. Worktrees touched within the
/// idle time aren't listed.
pub(crate) fn reap_rows(ctx: &Ctx, seen: &[Session]) -> Vec<Thing> {
    let (wts, busy) = worktrees(seen);
    if wts.is_empty() {
        return Vec::new();
    }
    let idle_for = reap_after();
    let cwds = process_cwds(ctx);
    let mut bases: HashMap<String, String> = HashMap::new();
    let mut out = Vec::new();
    for w in wts {
        if busy.contains_key(&w.path) {
            continue;
        }
        let base = bases
            .entry(w.repo.clone())
            .or_insert_with(|| {
                command(
                    ctx,
                    &w.repo,
                    "git",
                    &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
                )
                .unwrap_or_default()
            })
            .clone();
        let f = look(ctx, &w, &base, cwds.as_deref(), idle_for);
        if f.idle < idle_for {
            continue;
        }
        let label = crate::go_rel(&w.repo, &w.path).unwrap_or_else(|| w.path.clone());
        let repo_name = Path::new(&w.repo)
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = format!("{repo_name}: worktree {label}");
        let why = refusals(&f, idle_for);
        if !why.is_empty() {
            out.push(
                Thing::new(
                    format!("stale:{}", w.path),
                    format!("{name} is stale but kept"),
                    format!("does nothing: {}", why.join(", ")),
                    keep(why),
                )
                .meta("kept")
                .within(w.path.clone()),
            );
            continue;
        }
        out.push(
            Thing::new(
                format!("reap:{}", w.path),
                format!("{name} is no longer needed"),
                format!(
                    "runs git worktree remove, then git worktree prune: idle {}, {}, clean, nothing runs there",
                    days(f.idle),
                    f.merged
                ),
                reap(w.clone(), base, idle_for),
            )
            .at(w.path.clone())
            .within(w.path),
        );
    }
    out
}

/// Gathers a worktree's facts. It stops at its idle time when that's short
/// of idle_for, since the rest can't change the answer. cwds is None when
/// what runs where can't be told.
fn look(
    ctx: &Ctx,
    w: &Worktree,
    base: &str,
    cwds: Option<&[String]>,
    idle_for: Duration,
) -> ReapFacts {
    let mut f = ReapFacts {
        main: Path::new(&w.path) == Path::new(&w.repo),
        locked: w.locked,
        ..Default::default()
    };
    let since = SystemTime::now() - idle_for;
    f.idle = SystemTime::now()
        .duration_since(last_touched(Path::new(&w.path), since))
        .unwrap_or_default();
    if f.main || f.idle < idle_for {
        return f;
    }
    match command(
        ctx,
        &w.path,
        "git",
        &["status", "--porcelain", "--untracked-files=normal"],
    ) {
        Err(_) => f.err = "git status failed".into(),
        Ok(status) if !status.is_empty() => f.changed = status.split('\n').count(),
        Ok(_) => {}
    }
    f.held = -1;
    if let Some(cwds) = cwds {
        let dir = std::fs::canonicalize(&w.path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| w.path.clone());
        f.held = cwds
            .iter()
            .filter(|c| **c == dir || c.starts_with(&format!("{dir}/")))
            .count() as i64;
    }
    f.merged = merged(ctx, w, base);
    f
}

/// How a worktree's commits are kept, "" when they aren't: its branch is
/// in the default branch, or its upstream is gone and every patch is in
/// it, or it's detached at a commit the default branch has. The branch
/// itself stays.
fn merged(ctx: &Ctx, w: &Worktree, base: &str) -> String {
    if w.branch.is_empty() {
        if !base.is_empty()
            && command(
                ctx,
                &w.path,
                "git",
                &["merge-base", "--is-ancestor", "HEAD", base],
            )
            .is_ok()
        {
            return format!("detached at a commit in {base}");
        }
        return String::new();
    }
    if base.is_empty() {
        return String::new();
    }
    let head = format!("refs/heads/{}", w.branch);
    if command(
        ctx,
        &w.repo,
        "git",
        &["merge-base", "--is-ancestor", &head, base],
    )
    .is_ok()
    {
        return format!("{} is merged into {base}", w.branch);
    }
    // A gone upstream alone isn't enough: every commit's patch must be in
    // base (squash merges).
    if command(
        ctx,
        &w.repo,
        "git",
        &["for-each-ref", "--format=%(upstream:track)", &head],
    )
    .is_ok_and(|t| t == "[gone]")
        && command(ctx, &w.repo, "git", &["cherry", base, &head])
            .is_ok_and(|out| !format!("\n{out}").contains("\n+"))
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
fn last_touched(path: &Path, since: SystemTime) -> SystemTime {
    let mut last = SystemTime::UNIX_EPOCH;
    let mut touch = |t: SystemTime| {
        if t > last {
            last = t;
        }
        t > since
    };
    if let Ok(b) = std::fs::read_to_string(path.join(".git")) {
        let gd = b.trim().trim_start_matches("gitdir:").trim().to_string();
        let gd = if Path::new(&gd).is_absolute() {
            Path::new(&gd).to_path_buf()
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
    walk(path, path, &mut touch);
    last
}

/// Walks p in lexical order, as Go's WalkDir does, until touch says stop.
fn walk(root: &Path, p: &Path, touch: &mut impl FnMut(SystemTime) -> bool) -> bool {
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
    names.iter().any(|n| walk(root, n, touch))
}

/// The folders the user's processes work in, by their real paths; None
/// when that can't be told, which keeps every worktree.
fn process_cwds(ctx: &Ctx) -> Option<Vec<String>> {
    // SAFETY: getuid has no failure mode.
    let uid = unsafe { libc::getuid() };
    let mut cmd = Command::new("lsof");
    cmd.args(["-n", "-a", "-d", "cwd", "-Fn", "-u", &uid.to_string()]);
    let out = crate::exec::run(cmd, ctx, None, false).ok()?;
    if !out.success() && out.stdout.is_empty() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .split('\n')
            .filter_map(|l| l.strip_prefix('n').map(String::from))
            .collect(),
    )
}

/// The Clean of a worktree that can go: it looks again, as things may have
/// changed since it was listed, then removes it without force, so git
/// refuses anything dirty or locked too.
fn reap(w: Worktree, base: String, idle_for: Duration) -> Clean {
    Arc::new(move |ctx| {
        let cwds = process_cwds(ctx);
        let why = refusals(&look(ctx, &w, &base, cwds.as_deref(), idle_for), idle_for);
        if !why.is_empty() {
            return Err(format!("left alone: {}", why.join(", ")));
        }
        // Not stopped halfway, though the clean's time runs out: a removal
        // killed partway leaves the checkout hollowed out.
        command(
            &Ctx::background(),
            &w.repo,
            "git",
            &["worktree", "remove", &w.path],
        )?;
        command(ctx, &w.repo, "git", &["worktree", "prune"])?;
        Ok("Removed; its branch stays.".into())
    })
}

/// The Clean of a stale worktree that stays: it only says why.
fn keep(why: Vec<String>) -> Clean {
    Arc::new(move |_| Err(format!("left alone: {}", why.join(", "))))
}
