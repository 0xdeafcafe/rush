//! The three kits: what go-clean, js-clean and git-clean each find.
//! Ported from internal/bundled/clean/kits.go:1-178.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use rush_plugin::Ctx;

use super::{
    Kit, Session, Thing, cache_dir, command, fleet, is_dir, plural, ran, removed, repos, worktrees,
};

static GO: LazyLock<Kit> = LazyLock::new(|| Kit {
    name: "go-clean",
    about: "Go's build and module caches, gopls's and golangci-lint's, and the gopls processes agents' worktrees leave running: \
            how much each takes, and cleaning it up when you pick it. Off until you turn it on; needs go.",
    needs: &["go"],
    procs: &["gopls", "golangci-lint", "dlv"],
    things: go_things,
    settings: Vec::new(),
});

static JS: LazyLock<Kit> = LazyLock::new(|| Kit {
    name: "js-clean",
    about: "pnpm's store, npm's, yarn's and bun's caches, node_modules in agents' worktrees, and the node processes left running: \
            how much each takes, and cleaning it up when you pick it. Off until you turn it on; needs node.",
    needs: &["node"],
    procs: &["node", "bun", "deno"],
    things: js_things,
    settings: Vec::new(),
});

static GIT: LazyLock<Kit> = LazyLock::new(|| Kit {
    name: "git-clean",
    about: "In the repos agents work in: worktrees whose folders are gone, worktrees no longer needed (clean, merged, idle, nothing running there), \
            local branches already merged, and what git gc would pack. Agents' worktrees themselves are in Projects. Off until you turn it on; needs git.",
    needs: &["git"],
    procs: &[],
    things: git_things,
    settings: super::reap::git_settings(),
});

pub(crate) fn go_kit() -> &'static Kit {
    &GO
}

pub(crate) fn js_kit() -> &'static Kit {
    &JS
}

pub(crate) fn git_kit() -> &'static Kit {
    &GIT
}

fn go_things(ctx: &Ctx, _: &[Session]) -> Vec<Thing> {
    let mut out = Vec::new();
    if let Ok(d) = command(ctx, "", "go", &["env", "GOCACHE"])
        && is_dir(&d)
    {
        out.push(
            Thing::new(
                "gocache",
                "Go build cache",
                "runs go clean -cache: builds and tests are slower until it fills again",
                ran("", "go", &["clean", "-cache"]),
            )
            .at(d),
        );
    }
    if let Ok(d) = command(ctx, "", "go", &["env", "GOMODCACHE"])
        && is_dir(&d)
    {
        out.push(
            Thing::new(
                "gomodcache",
                "Go module cache",
                "runs go clean -modcache: every module downloads again when next built",
                ran("", "go", &["clean", "-modcache"]),
            )
            .at(d),
        );
    }
    for (id, title, dir) in [
        ("gopls", "gopls cache", "gopls"),
        ("golangci-lint", "golangci-lint cache", "golangci-lint"),
    ] {
        let d = cache_dir(dir);
        if !d.is_empty() && is_dir(&d) {
            out.push(
                Thing::new(
                    id,
                    title,
                    "deletes it; it's built again as needed",
                    removed(&d),
                )
                .at(d),
            );
        }
    }
    out
}

fn js_things(ctx: &Ctx, seen: &[Session]) -> Vec<Thing> {
    let mut out = Vec::new();
    if let Ok(d) = command(ctx, "", "pnpm", &["store", "path"])
        && is_dir(&d)
    {
        out.push(
            Thing::new(
                "pnpm",
                "pnpm store",
                "runs pnpm store prune: removes packages no project uses",
                ran("", "pnpm", &["store", "prune"]),
            )
            .at(d),
        );
    }
    if let Ok(d) = command(ctx, "", "npm", &["config", "get", "cache"])
        && is_dir(&d)
    {
        out.push(
            Thing::new(
                "npm",
                "npm cache",
                "runs npm cache clean --force: packages download again when next installed",
                ran("", "npm", &["cache", "clean", "--force"]),
            )
            .at(d),
        );
    }
    if let Ok(d) = command(ctx, "", "yarn", &["cache", "dir"])
        && is_dir(&d)
    {
        out.push(
            Thing::new(
                "yarn",
                "yarn cache",
                "runs yarn cache clean: packages download again when next installed",
                ran("", "yarn", &["cache", "clean"]),
            )
            .at(d),
        );
    }
    let d = bun_cache();
    if is_dir(&d) {
        out.push(
            Thing::new(
                "bun",
                "bun cache",
                "runs bun pm cache rm: packages download again when next installed",
                ran("", "bun", &["pm", "cache", "rm"]),
            )
            .at(d),
        );
    }
    // Dependencies in worktrees no agent is working in. The main
    // checkout's are left alone: that's where you work.
    let (wts, busy) = worktrees(seen);
    for w in wts {
        let nm = Path::new(&w.path)
            .join("node_modules")
            .to_string_lossy()
            .into_owned();
        if busy.contains_key(&w.path) || !is_dir(&nm) {
            continue;
        }
        let base = Path::new(&w.path)
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(
            Thing::new(
                format!("nm:{}", w.path),
                format!("node_modules in {base}"),
                "deletes it: git ignores it, and an install puts it back",
                removed(&nm),
            )
            .at(nm)
            .within(w.path),
        );
    }
    out
}

fn bun_cache() -> String {
    if let Ok(d) = std::env::var("BUN_INSTALL_CACHE_DIR")
        && !d.is_empty()
    {
        return d;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.bun/install/cache")
}

fn git_things(ctx: &Ctx, seen: &[Session]) -> Vec<Thing> {
    let mut out = Vec::new();
    for repo in repos(ctx, seen) {
        let name = Path::new(&repo)
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_default();
        let n = stale_worktrees(ctx, &repo);
        if n > 0 {
            out.push(
                Thing::new(
                    format!("prune:{repo}"),
                    format!("{name}: worktrees whose folders are gone"),
                    "runs git worktree prune",
                    ran(&repo, "git", &["worktree", "prune"]),
                )
                .meta(plural(n, "entry", "entries")),
            );
        }
        let (base, merged) = merged_branches(ctx, &repo);
        if !merged.is_empty() {
            out.push(
                Thing::new(
                    format!("merged:{repo}"),
                    format!("{name}: branches merged into {base}"),
                    format!(
                        "deletes them locally: {}",
                        rush_plugin::clip(&merged.join(", "), 300)
                    ),
                    delete_branches(&repo, merged.clone()),
                )
                .meta(plural(merged.len(), "branch", "branches")),
            );
        }
        out.push(
            Thing::new(
                format!("gc:{repo}"),
                format!("{name}: git's objects"),
                "runs git gc: packs objects and drops unreachable ones older than two weeks",
                ran(&repo, "git", &["gc", "--quiet"]),
            )
            .at(Path::new(&repo).join(".git").to_string_lossy().into_owned()),
        );
    }
    out.extend(super::reap::reap_rows(ctx, seen));
    out
}

/// Counts a repo's worktrees whose folders are gone.
fn stale_worktrees(ctx: &Ctx, repo: &str) -> usize {
    command(ctx, repo, "git", &["worktree", "list", "--porcelain"])
        .map(|out| count_prunable(&out))
        .unwrap_or(0)
}

pub(crate) fn count_prunable(porcelain: &str) -> usize {
    porcelain
        .split('\n')
        .filter(|l| *l == "prunable" || l.starts_with("prunable "))
        .count()
}

/// A repo's local branches whose work is on its remote's default branch,
/// checked out nowhere: deleting them loses nothing. The rule is fleet's,
/// every clean-up's: merged into it here or on origin, or their pull
/// request merged, squashed too.
pub(crate) fn merged_branches(ctx: &Ctx, repo: &str) -> (String, Vec<String>) {
    let Ok(base) = command(
        ctx,
        repo,
        "git",
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) else {
        return (String::new(), Vec::new());
    };
    // Without gh, squash merges aren't known: the rest still are.
    let prs: HashMap<String, rush_gh::ghpr::Status> = rush_gh::ghpr::Gh::shared()
        .look_branches(repo)
        .map(|b| (*b).clone())
        .unwrap_or_default();
    let target = base.strip_prefix("origin/").unwrap_or(&base).to_string();
    let landed = fleet::landed_branches(repo, &target, &prs);
    (base, landed)
}

/// Deletes branches whose work is on the remote's default branch: those
/// listed that still are, since one may have moved since. git branch -d
/// would refuse any not merged into HEAD, which in a repo checked out
/// elsewhere means nothing, so it's -D.
pub(crate) fn delete_branches(repo: &str, listed: Vec<String>) -> super::Clean {
    let repo = repo.to_string();
    Arc::new(move |ctx| {
        let (_, now) = merged_branches(ctx, &repo);
        let names: Vec<&str> = listed
            .iter()
            .filter(|n| now.contains(n))
            .map(String::as_str)
            .collect();
        if names.is_empty() {
            return Ok("None is still merged.".into());
        }
        let mut args = vec!["branch", "-D"];
        args.extend(&names);
        command(ctx, &repo, "git", &args)?;
        Ok(format!(
            "Deleted {}.",
            plural(names.len(), "branch", "branches")
        ))
    })
}
