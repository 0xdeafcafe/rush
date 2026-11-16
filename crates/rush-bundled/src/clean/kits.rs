//! The four kits: what go-clean, rust-clean, js-clean and git-clean each
//! find. Ported from internal/bundled/clean/kits.go:1-241 (rustKit,
//! cargoHome and sccacheDir at :90-151). Rust-only: sccache's caches
//! where projects put them too (sccache_dirs).

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

static RUST: LazyLock<Kit> = LazyLock::new(|| Kit {
    name: "rust-clean",
    about: "Cargo's downloaded crates and git dependencies, sccache's cache, target folders in agents' worktrees, and the rust-analyzer processes they leave running: \
            how much each takes, and cleaning it up when you pick it. Off until you turn it on; needs cargo.",
    needs: &["cargo"],
    procs: &["rust-analyzer", "bacon", "cargo-watch", "sccache"],
    things: rust_things,
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

pub(crate) fn rust_kit() -> &'static Kit {
    &RUST
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

fn rust_things(_: &Ctx, seen: &[Session]) -> Vec<Thing> {
    rust_things_with(&|k| std::env::var(k).ok().filter(|v| !v.is_empty()), seen)
}

/// rust_things with the environment as env says.
pub(super) fn rust_things_with(
    env: &dyn Fn(&str) -> Option<String>,
    seen: &[Session],
) -> Vec<Thing> {
    let mut out = Vec::new();
    let home = cargo_home(env);
    for (id, title, dir, does) in [
        (
            "crates",
            "Cargo's downloaded crates",
            "registry/cache",
            "deletes them: crates download again when next built",
        ),
        (
            "crates-src",
            "Cargo's unpacked crates",
            "registry/src",
            "deletes them: cargo unpacks them again from what it downloaded",
        ),
        (
            "cargo-git",
            "Cargo's git dependencies",
            "git",
            "deletes them: git dependencies are cloned again when next built",
        ),
    ] {
        let d = Path::new(&home).join(dir).to_string_lossy().into_owned();
        if is_dir(&d) {
            out.push(Thing::new(id, title, does, removed(&d)).at(d));
        }
    }
    for (i, d) in sccache_dirs(env, seen).into_iter().enumerate() {
        // The first is sccache's own, as Go named it; the rest by folder.
        let (id, title) = if i == 0 && d == sccache_dir(env) {
            ("sccache".to_string(), "sccache cache".to_string())
        } else {
            let base = Path::new(&d)
                .file_name()
                .map(|b| b.to_string_lossy().into_owned())
                .unwrap_or_default();
            (format!("sccache:{d}"), format!("sccache cache in {base}"))
        };
        out.push(
            Thing::new(
                id,
                title,
                "deletes it: builds are slower until it fills again",
                removed(&d),
            )
            .at(d),
        );
    }
    // One target folder every project builds into.
    if let Some(d) = env("CARGO_TARGET_DIR").filter(|d| is_dir(d)) {
        out.push(
            Thing::new(
                "target-dir",
                "Shared target folder",
                "deletes it: every project builds again from scratch",
                removed(&d),
            )
            .at(d),
        );
    }
    // What was built in worktrees no agent is working in. The main
    // checkout's is left alone: that's where you work.
    let (wts, busy) = worktrees(seen);
    for w in wts {
        let t = Path::new(&w.path)
            .join("target")
            .to_string_lossy()
            .into_owned();
        if busy.contains_key(&w.path)
            || !is_dir(&t)
            || !Path::new(&w.path).join("Cargo.toml").is_file()
        {
            continue;
        }
        let base = Path::new(&w.path)
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(
            Thing::new(
                format!("target:{}", w.path),
                format!("target in {base}"),
                "deletes it: git ignores it, and a build puts it back",
                removed(&t),
            )
            .at(t)
            .within(w.path),
        );
    }
    out
}

/// Where cargo keeps what it downloads.
fn cargo_home(env: &dyn Fn(&str) -> Option<String>) -> String {
    if let Some(d) = env("CARGO_HOME") {
        return d;
    }
    let home = env("HOME").unwrap_or_default();
    Path::new(&home)
        .join(".cargo")
        .to_string_lossy()
        .into_owned()
}

/// sccache's local cache: SCCACHE_DIR, else its own folder under the
/// user's cache folder.
fn sccache_dir(env: &dyn Fn(&str) -> Option<String>) -> String {
    if let Some(d) = env("SCCACHE_DIR") {
        return d;
    }
    let d = cache_dir("Mozilla.sccache"); // macOS
    if is_dir(&d) {
        return d;
    }
    cache_dir("sccache")
}

/// Every sccache cache there is: sccache_dir's first, then the
/// SCCACHE_DIR a project sets (its .cargo/config.toml's [env], or a .env
/// or .envrc) in a folder sessions work in or one above it, up to its
/// repository's top, then any folder in the user's cache folders named
/// for sccache (one agents pass to each command, which nothing on disk
/// says). Those that are folders, each once. Rust-only past the first.
pub(super) fn sccache_dirs(env: &dyn Fn(&str) -> Option<String>, seen: &[Session]) -> Vec<String> {
    let home = env("HOME").unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut add = |d: String| {
        let d = d.trim_end_matches('/').to_string();
        if is_dir(&d) && !out.contains(&d) {
            out.push(d);
        }
    };
    add(sccache_dir(env));
    let mut looked = std::collections::HashSet::new();
    for s in seen {
        let mut dir = Some(Path::new(&s.cwd));
        while let Some(d) = dir {
            if d.as_os_str().is_empty() || !looked.insert(d.to_path_buf()) {
                break;
            }
            for f in [".cargo/config.toml", ".cargo/config"] {
                if let Some(v) = cargo_env_sccache(&d.join(f), d) {
                    add(expand(&v, &home, d));
                }
            }
            for f in [".env", ".envrc"] {
                if let Some(v) = env_file_sccache(&d.join(f)) {
                    add(expand(&v, &home, d));
                }
            }
            if d.join(".git").exists() || d == Path::new(&home) {
                break;
            }
            dir = d.parent();
        }
    }
    for root in ["Library/Caches", ".cache"] {
        let root = Path::new(&home).join(root);
        if home.is_empty() {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut named: Vec<String> = rd
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("sccache")
            })
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect();
        named.sort();
        for d in named {
            add(d);
        }
    }
    out
}

/// The SCCACHE_DIR a cargo config at path sets in [env]: a string, or a
/// table's value (relative to the folder holding .cargo when it says so).
fn cargo_env_sccache(path: &Path, top: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc: toml::Table = text.parse().ok()?;
    match doc.get("env")?.get("SCCACHE_DIR")? {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Table(t) => {
            let v = t.get("value")?.as_str()?;
            let rel = t.get("relative").and_then(toml::Value::as_bool) == Some(true);
            Some(if rel && !v.starts_with('/') {
                top.join(v).to_string_lossy().into_owned()
            } else {
                v.to_string()
            })
        }
        _ => None,
    }
}

/// The SCCACHE_DIR an env file sets: SCCACHE_DIR=… or export SCCACHE_DIR=…,
/// the last one, unquoted.
fn env_file_sccache(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    // The last one is what a shell is left with.
    text.lines().rev().find_map(|l| {
        let l = l.trim();
        let l = l.strip_prefix("export ").unwrap_or(l).trim_start();
        let v = l.strip_prefix("SCCACHE_DIR=")?.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(v);
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// v with ~, $HOME and ${HOME} as home, and relative to dir when it isn't
/// absolute.
fn expand(v: &str, home: &str, dir: &Path) -> String {
    let v = if let Some(r) = v.strip_prefix("~/") {
        format!("{home}/{r}")
    } else {
        v.replace("${HOME}", home).replace("$HOME", home)
    };
    if v.starts_with('/') {
        v
    } else {
        dir.join(v).to_string_lossy().into_owned()
    }
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
