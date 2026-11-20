//! After internal/bundled/clean/clean_test.go and reap_test.go.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::*;

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// clean_test.go TestMergedBranches: only branches on the remote's default
// branch, checked out nowhere, are offered; and one that moved on since
// isn't deleted.
#[test]
fn merged_branches() {
    if rush_core::adapter::find("git", &[]).is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let (origin, repo) = (root.join("origin.git"), root.join("repo"));
    git(&root, &["init", "--bare", origin.to_str().unwrap()]);
    git(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git(&repo, &["push", "-q", "origin", "main"]);
    git(&repo, &["remote", "set-head", "origin", "main"]);
    git(&repo, &["branch", "done"]);
    git(&repo, &["branch", "later"]);
    git(&repo, &["branch", "held"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            root.join("wt").to_str().unwrap(),
            "held",
        ],
    );
    git(&repo, &["checkout", "-q", "-b", "wip"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "unpushed"]);
    git(&repo, &["checkout", "-q", "main"]);

    let ctx = Ctx::background();
    let r = repo.to_str().unwrap();
    let (base, merged) = kits::merged_branches(&ctx, r);
    assert_eq!(
        (base.as_str(), merged.clone()),
        ("origin/main", vec!["done".to_string(), "later".into()])
    );
    // "later" gets work of its own after it was listed.
    git(&repo, &["checkout", "-q", "later"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "new work"]);
    git(&repo, &["checkout", "-q", "main"]);
    let said = kits::delete_branches(r, merged)(&ctx);
    assert_eq!(said, Ok("Deleted 1 branch.".into()));
    let out = Command::new("git")
        .args(["-C", r, "branch", "--format=%(refname:short)"])
        .output()
        .unwrap();
    let got: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(String::from)
        .collect();
    assert_eq!(got, ["held", "later", "main", "wip"], "branches left");

    // The worktree is found from a session in it, and in the repo.
    let wts = fleet::find_worktrees(&[fleet::Agent {
        key: "a".into(),
        cwd: root.join("wt").to_string_lossy().into_owned(),
    }]);
    assert_eq!(wts.len(), 1, "{wts:?}");
    assert!(
        wts[0].branch == "held" && wts[0].repo == r && wts[0].agents == ["a"],
        "{wts:?}"
    );
    assert_eq!(
        kits::count_prunable("worktree /a\nprunable gitdir file points to non-existent location\n"),
        1
    );
}

fn noop() -> Clean {
    Arc::new(|_| Ok(String::new()))
}

// clean_test.go TestPick: the list shows sizes, totals and what's
// running, each on its tab.
#[test]
fn picks() {
    let k = Kit {
        name: "go-clean",
        about: "",
        needs: &[],
        procs: &["gopls"],
        things: |_, _| Vec::new(),
        settings: Vec::new(),
    };
    let mut gocache =
        Thing::new("gocache", "Go build cache", "runs go clean -cache", noop()).at("/c");
    gocache.size = 3 << 30;
    let things = vec![
        gocache,
        Thing::new("merged:r", "r: branches merged", "deletes them", noop()).meta("2 branches"),
    ];
    let run = vec![Running {
        p: proc::Proc {
            pid: 7,
            comm: "gopls".into(),
            footprint: 512 << 20,
            start: SystemTime::now(),
            ..Default::default()
        },
        cmd: "gopls serve".into(),
    }];
    let now = Time::now();
    let p = pick(&k, &things, Some(now), &run, now);
    assert_eq!(p.tabs, ["On disk · 3.0 GB", "Running · 512 MB"]);
    assert!(
        p.items.len() == 3
            && p.items[1].meta == "2 branches"
            && p.items[2].tab == 1
            && p.items[2].id.starts_with("pid:7:"),
        "items: {:?}",
        p.items
    );
    assert_eq!(
        p.items[0].text,
        "Go build cache\n/c\nenter runs go clean -cache"
    );
    let p = pick(&k, &[], None, &[], now);
    assert_eq!(p.empty[0], "Measuring…", "before measuring");
    rush_plugin::clean_pick(p).unwrap();
}

// reap_test.go TestRefusals: a worktree goes only when nothing refuses
// it, and each refusal says why.
#[test]
fn refusal_reasons() {
    let day = Duration::from_secs(24 * 3600);
    let idle_for = day * 3;
    let ok = ReapFacts {
        merged: "x is merged into origin/main".into(),
        idle: day * 4,
        ..Default::default()
    };
    type Edit = fn(&mut ReapFacts);
    let cases: Vec<(&str, Edit, Vec<&str>)> = vec![
        ("clean, merged and idle", |_| {}, vec![]),
        (
            "upstream gone",
            |f| f.merged = "x's upstream is gone".into(),
            vec![],
        ),
        (
            "detached in main",
            |f| f.merged = "detached at a commit in origin/main".into(),
            vec![],
        ),
        (
            "main checkout",
            |f| (f.main, f.changed) = (true, 3),
            vec!["the main checkout"],
        ),
        (
            "check failed",
            |f| f.err = "git status failed".into(),
            vec!["git status failed"],
        ),
        ("locked", |f| f.locked = true, vec!["locked"]),
        ("dirty", |f| f.changed = 2, vec!["2 uncommitted files"]),
        (
            "process there",
            |f| f.held = 1,
            vec!["1 process working there"],
        ),
        (
            "processes unknown",
            |f| f.held = -1,
            vec!["can't tell what runs there"],
        ),
        (
            "unmerged",
            |f| f.merged = String::new(),
            vec!["its commits aren't all in the default branch"],
        ),
        (
            "recent",
            |f| f.idle = Duration::from_secs(24 * 3600),
            vec!["touched 1d ago"],
        ),
        (
            "dirty and unmerged",
            |f| (f.changed, f.merged) = (1, String::new()),
            vec![
                "1 uncommitted file",
                "its commits aren't all in the default branch",
            ],
        ),
    ];
    for (name, edit, want) in cases {
        let mut f = ok.clone();
        edit(&mut f);
        assert_eq!(refusals(&f, idle_for), want, "{name}");
    }
}

// reap_test.go TestReapNotice: it says worktrees can be reaped only when
// some can, and at most every four hours; kept ones don't count.
#[test]
fn reap_notices() {
    let now = Time::parse("2026-10-09T12:00:00Z").unwrap();
    let t = |id: &str| Thing::new(id, "", "", noop());
    let two = vec![t("reap:/a"), t("stale:/b"), t("reap:/c"), t("gc:/r")];
    let hour = 3600 * 1_000_000_000i64;
    let cases: Vec<(&str, &[Thing], Option<Time>, &str)> = vec![
        (
            "never said",
            &two,
            None,
            "2 worktrees can be reaped: #git-clean.clean to review",
        ),
        (
            "one",
            &two[..1],
            None,
            "1 worktree can be reaped: #git-clean.clean to review",
        ),
        (
            "said long ago",
            &two,
            Some(now.add_nanos(-4 * hour)),
            "2 worktrees can be reaped: #git-clean.clean to review",
        ),
        ("said lately", &two, Some(now.add_nanos(-hour)), ""),
        ("only kept", &two[1..2], None, ""),
        ("nothing", &[], None, ""),
    ];
    for (name, things, said, want) in cases {
        assert_eq!(reap_notice(things, said, now), want, "{name}");
    }
}

#[test]
fn byte_sizes() {
    for (n, want) in [
        (3 << 30, "3.0 GB"),
        (512 << 20, "512 MB"),
        (2048, "2 KB"),
        (12, "12 B"),
    ] {
        assert_eq!(bytes(n), want);
    }
}

// clean_test.go TestRustThings: rust-clean finds cargo's caches where
// CARGO_HOME says, and leaves a folder named target alone unless it's a
// Cargo project's.
#[test]
fn rust_things() {
    let home = tempfile::tempdir().unwrap();
    for d in ["registry/cache", "git"] {
        std::fs::create_dir_all(home.path().join(d)).unwrap();
    }
    let h = home.path().to_string_lossy().into_owned();
    let none = home.path().join("none").to_string_lossy().into_owned();
    let env = |k: &str| match k {
        "CARGO_HOME" => Some(h.clone()),
        "SCCACHE_DIR" => Some(none.clone()),
        _ => None,
    };
    let ids: Vec<String> = kits::rust_things_with(&env, &[], &[])
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, ["crates", "cargo-git"]);
}

// rust-clean finds sccache's caches beyond its own: the SCCACHE_DIR a
// project's .cargo/config.toml sets in [env] (relative to the project
// when it says so), an .envrc's, and a folder in the cache folder named
// for sccache that nothing on disk names; each once, each a folder.
#[test]
fn sccache_where_projects_put_it() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_string_lossy().into_owned();
    let p = |s: &str| format!("{h}/{s}");
    for d in [
        "own",
        "Library/Caches/proj-sccache",
        "Library/Caches/unrelated",
        "repo/.git",
        "repo/.cargo",
        "repo/cache/sc",
        "repo/crates/app",
        "envsc",
    ] {
        std::fs::create_dir_all(p(d)).unwrap();
    }
    std::fs::write(
        p("repo/.cargo/config.toml"),
        "[env]\nSCCACHE_DIR = { value = \"cache/sc\", relative = true }\n",
    )
    .unwrap();
    std::fs::write(
        p("repo/.envrc"),
        "export SCCACHE_DIR=\"$HOME/envsc\"\nexport SCCACHE_DIR=\"$HOME/gone\"\n",
    )
    .unwrap();
    let own = p("own");
    let env = |k: &str| match k {
        "HOME" => Some(h.clone()),
        "SCCACHE_DIR" => Some(own.clone()),
        _ => None,
    };
    let seen = [Session {
        id: "a".into(),
        cwd: p("repo/crates/app"),
        state: "idle".into(),
    }];
    assert_eq!(
        kits::sccache_dirs(&env, &seen),
        [
            p("own"),
            p("repo/cache/sc"),
            p("Library/Caches/proj-sccache")
        ],
        "the .envrc's last SCCACHE_DIR is gone, so it isn't listed"
    );
    std::fs::create_dir_all(p("gone")).unwrap();
    assert_eq!(
        kits::sccache_dirs(&env, &seen),
        [
            p("own"),
            p("repo/cache/sc"),
            p("gone"),
            p("Library/Caches/proj-sccache")
        ]
    );
    let titles: Vec<String> = kits::rust_things_with(&env, &seen, &[])
        .into_iter()
        .filter(|t| t.id.starts_with("sccache"))
        .map(|t| t.title)
        .collect();
    assert_eq!(
        titles,
        [
            "sccache cache",
            "sccache cache in sc",
            "sccache cache in gone",
            "sccache cache in proj-sccache"
        ]
    );
}

// The clean-ups run with your tools on PATH, after what it had, each once:
// lsof's /usr/sbin, and kache's ~/.cargo/bin.
#[test]
fn user_path_finds_tools() {
    assert_eq!(
        user_path("/Users/me", "/usr/bin:/bin"),
        "/usr/bin:/bin:/usr/sbin:/sbin:/Users/me/.cargo/bin:/Users/me/go/bin:/Users/me/.bun/bin:/Users/me/.local/bin:/opt/homebrew/bin:/usr/local/bin"
    );
    assert_eq!(
        user_path("/h", "/opt/homebrew/bin:/usr/bin"),
        "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin:/h/.cargo/bin:/h/go/bin:/h/.bun/bin:/h/.local/bin:/usr/local/bin"
    );
}

// ---- disk.candidates: what the kits report to the disk engine ----

/// A folder at p with a file of n bytes in it.
fn folder_with(p: &std::path::Path, n: usize) {
    std::fs::create_dir_all(p).unwrap();
    std::fs::write(p.join("f"), vec![1u8; n]).unwrap();
}

// A folder's sizes in one walk: on disk counts each file once however
// many links it has there; freeable counts only files with no other link;
// newest is the latest change in it.
#[test]
fn sizes_count_each_file_once() {
    let d = tempfile::tempdir().unwrap();
    let base = d.path();
    folder_with(&base.join("own"), 64 << 10);
    let own = sizes(&base.join("own"));
    assert!(
        own.on_disk >= 64 << 10 && own.freeable >= 64 << 10,
        "{own:?}"
    );
    assert!(own.newest.is_some());
    // Two links to one file, both inside: once on disk, never freeable.
    folder_with(&base.join("twice"), 64 << 10);
    std::fs::hard_link(base.join("twice/f"), base.join("twice/g")).unwrap();
    let twice = sizes(&base.join("twice"));
    let file = sizes(&base.join("twice/f"));
    for (name, got, want) in [
        ("a linked file frees nothing", twice.freeable, 0),
        ("nor does it alone", file.freeable, 0),
    ] {
        assert_eq!(got, want, "{name}");
    }
    assert!(twice.on_disk < 2 * (64 << 10), "{twice:?}: counted twice");
    // A link to a folder outside isn't followed.
    std::fs::create_dir_all(base.join("linky")).unwrap();
    std::os::unix::fs::symlink(base.join("own"), base.join("linky/to-own")).unwrap();
    assert!(sizes(&base.join("linky")).on_disk < 64 << 10);
    assert_eq!(sizes(&base.join("gone")), Sizes::default());
}

// The folders a built-in cache-dir rule names are the engine's own: a kit
// leaves them, and anything in them, out of its candidates.
#[test]
fn engine_finds_its_own_caches() {
    for (path, want) in [
        ("/h/Library/Caches/go-build", true),
        ("/h/Library/Caches/go-build/ab", true),
        ("/h/Library/Caches/golangci-lint", true),
        ("/h/.cache/uv", true),
        ("/h/Library/Caches/go-builds", false),
        ("/h/Library/Caches/gopls", false),
        ("/h/.cargo/registry/cache", false),
        ("/elsewhere/go-build", false),
    ] {
        assert_eq!(engine_finds(path, "/h"), want, "{path}");
        assert_eq!(engine_finds(path, "/h/"), want, "{path}, home with a slash");
    }
    assert!(
        !engine_finds("/Library/Caches/go-build", ""),
        "no home, no ~"
    );
}

// A kit's disk.candidates answer: each thing with a kind and an absolute
// path, with its sizes and facts; not what has no kind (branches, git's
// objects, targets and worktrees, which the engine finds itself), no
// path, or a path the engine's own cache rules find. Nothing is removed.
#[test]
fn candidates_answered() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_string_lossy().into_owned();
    let p = |s: &str| format!("{h}/{s}");
    let mut gopls = Thing::new("gopls", "gopls cache", "deletes it", noop())
        .at(p("Library/Caches/gopls"))
        .kind(kits::KIND_GO_GOPLS_CACHE);
    gopls.size = 4096;
    gopls.freeable = 1024;
    gopls.mtime = Some(1_700_000_000_000);
    let nm = Thing::new("nm:/w", "node_modules in w", "deletes it", noop())
        .at(p("w/node_modules"))
        .kind(kits::KIND_JS_NODE_MODULES)
        .of_session("s1")
        .within(p("w"));
    let things = vec![
        gopls,
        nm,
        // The engine's own: Go's build cache where it looks.
        Thing::new("gocache", "Go build cache", "", noop())
            .at(p("Library/Caches/go-build"))
            .kind(kits::KIND_GO_BUILD_CACHE),
        // No kind: a target, branches.
        Thing::new("target:/w", "target in w", "", noop()).at(p("w/target")),
        Thing::new("merged:r", "r: branches", "", noop()).meta("2 branches"),
        // Not absolute, or no path.
        Thing::new("x", "x", "", noop())
            .at("rel/x")
            .kind(kits::KIND_RUST_SCCACHE),
        Thing::new("y", "y", "", noop()).kind(kits::KIND_RUST_SCCACHE),
    ];
    let got = candidates(&things, &h);
    assert_eq!(
        got.candidates,
        [
            DiskCandidate {
                path: p("Library/Caches/gopls"),
                kind: "go.gopls-cache".into(),
                why: "gopls cache".into(),
                bytes_on_disk: 4096,
                bytes_freeable: 1024,
                facts: DiskFacts {
                    mtime: Some(1_700_000_000_000),
                    ..DiskFacts::default()
                },
            },
            DiskCandidate {
                path: p("w/node_modules"),
                kind: "js.node-modules".into(),
                why: "node_modules in w".into(),
                facts: DiskFacts {
                    session: "s1".into(),
                    worktree: p("w"),
                    ..DiskFacts::default()
                },
                ..DiskCandidate::default()
            },
        ]
    );
    // The engine takes every one in as the kit's, of the kind it said.
    let back =
        rush_fleet::disk::from_answer("go-clean", &serde_json::to_value(&got).unwrap()).unwrap();
    assert_eq!(
        back.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(),
        ["go.gopls-cache", "js.node-modules"]
    );
    assert!(back.iter().all(|c| c.plugin == "go-clean"));
}

// rust-clean's folders each carry their kind, sized as measured; its
// targets carry none (the engine finds those), and a removal picked goes
// through the engine's apply, which refuses what isn't absolute.
#[test]
fn rust_kinds() {
    let home = tempfile::tempdir().unwrap();
    for d in ["registry/cache", "registry/src", "git", "sc", "shared"] {
        folder_with(&home.path().join(d), 4096);
    }
    let h = home.path().to_string_lossy().into_owned();
    let sc = format!("{h}/sc");
    let shared = format!("{h}/shared");
    let env = |k: &str| match k {
        "CARGO_HOME" => Some(h.clone()),
        "HOME" => Some(h.clone()),
        "SCCACHE_DIR" => Some(sc.clone()),
        "CARGO_TARGET_DIR" => Some(shared.clone()),
        _ => None,
    };
    let kinds: Vec<(String, String)> = kits::rust_things_with(&env, &[], &[])
        .into_iter()
        .map(|t| (t.id, t.kind))
        .collect();
    let want: Vec<(String, String)> = [
        ("crates", "rust.registry-cache"),
        ("crates-src", "rust.registry-src"),
        ("cargo-git", "rust.git-deps"),
        ("sccache", "rust.sccache"),
        ("target-dir", "rust.shared-target"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(kinds, want);
    let ctx = Ctx::background();
    let got = removed("relative/node_modules", kits::KIND_JS_NODE_MODULES)(&ctx);
    assert_eq!(
        got,
        Err("won't delete relative/node_modules: not an absolute path".into())
    );
}

// Nothing in the clean-ups deletes on its own any more: no kit has an
// auto-clean setting, each answers disk.candidates for the engine, and no
// code of theirs removes a file or folder but through the engine's apply.
#[test]
fn nothing_deletes_on_its_own() {
    for k in [
        kits::go_kit(),
        kits::rust_kit(),
        kits::js_kit(),
        kits::git_kit(),
    ] {
        let m = k.manifest();
        assert!(
            m.settings.iter().all(|s| s.key != "auto-clean"),
            "{}: {:?}",
            k.name,
            m.settings
        );
        assert!(
            m.provides.iter().any(|s| s == SERVICE_DISK_CANDIDATES)
                && m.provides.iter().any(|s| s == SERVICE_CLEANUP_REPORT)
                && m.provides.iter().any(|s| s == SERVICE_CLEANUP_CLEAN),
            "{}: {:?}",
            k.name,
            m.provides
        );
    }
    for (file, src) in [
        ("mod.rs", include_str!("mod.rs")),
        ("kits.rs", include_str!("kits.rs")),
        ("reap.rs", include_str!("reap.rs")),
        ("fleet.rs", include_str!("fleet.rs")),
    ] {
        for word in [
            "remove_dir_all",
            "remove_file",
            "\"auto-clean\"",
            "LOW_EVERY",
        ] {
            assert!(!src.contains(word), "{file} has {word}");
        }
        assert!(
            !src.contains("\"worktree\", \"remove\""),
            "{file} removes worktrees itself"
        );
    }
}

/// An App of kit k that has measured things, and knows sessions seen.
fn app_with(k: &'static Kit, things: Vec<Thing>, seen: Vec<Session>) -> Arc<App> {
    Arc::new(App {
        k,
        st: Mutex::new(State {
            things,
            measured: Some(Time::now()),
            seen: seen.into_iter().map(|s| (s.id.clone(), s)).collect(),
            ..Default::default()
        }),
        wake: Condvar::new(),
        ctx: Ctx::background(),
    })
}

fn service_call(service: &str, params: serde_json::Value) -> Box<serde_json::value::RawValue> {
    rush_plugin::rpc::to_raw(&serde_json::json!({"service": service, "params": params})).unwrap()
}

// cleanup.report's items are the kit's things, one each by id, with what
// cleaning loses: rust-clean's caches come back by themselves, and the
// report's text is as it was for the cleanup agent.
#[test]
fn report_items_are_the_things() {
    let home = tempfile::tempdir().unwrap();
    for d in ["registry/cache", "git"] {
        std::fs::create_dir_all(home.path().join(d)).unwrap();
    }
    let h = home.path().to_string_lossy().into_owned();
    let none = home.path().join("none").to_string_lossy().into_owned();
    let env = |k: &str| match k {
        "CARGO_HOME" => Some(h.clone()),
        "SCCACHE_DIR" => Some(none.clone()),
        _ => None,
    };
    let mut things = kits::rust_things_with(&env, &[], &[]);
    things[0].size = 2 << 30;
    let mut kept = Thing::new("stale:/w", "w is stale but kept", "does nothing", noop())
        .meta("kept")
        .within("/w");
    kept.size = 9;
    things.push(kept);
    let got = items(&things);
    assert_eq!(
        got.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        things.iter().map(|t| t.id.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(
        got[0],
        CleanupItem {
            id: "crates".into(),
            title: "Cargo's downloaded crates".into(),
            bytes: 2 << 30,
            path: format!("{h}/registry/cache"),
            can_go: true,
            comes_back: true,
            does: "deletes them: crates download again when next built".into(),
            ..Default::default()
        }
    );
    assert!(got[1].can_go && got[1].comes_back, "{:?}", got[1]);
    // What's kept is no size, loses something, and says it's in /w.
    assert_eq!(
        got[2],
        CleanupItem {
            id: "stale:/w".into(),
            title: "w is stale but kept".into(),
            meta: "kept".into(),
            worktree: "/w".into(),
            does: "does nothing".into(),
            ..Default::default()
        }
    );

    let k: &'static Kit = kits::rust_kit();
    let a = app_with(k, things, Vec::new());
    let raw = a
        .service(Some(&service_call(
            SERVICE_CLEANUP_REPORT,
            serde_json::json!(null),
        )))
        .unwrap()
        .unwrap();
    let rep: CleanupReport = serde_json::from_str(raw.get()).unwrap();
    assert_eq!(rep.items, got);
    assert!(
        rep.report
            .contains("- Cargo's downloaded crates · 2.0G · deletes them"),
        "{}",
        rep.report
    );
    // Before it has measured: no items, and the text says so.
    let a = app_with(k, Vec::new(), Vec::new());
    a.lock().measured = None;
    let rep = a.report();
    assert!(rep.items.is_empty() && rep.report == "It hasn't measured yet.");
}

// cleanup.clean cleans one thing by id, as enter on its row does: an id
// it doesn't have, one an agent works in, and one being cleaned already
// are refused, each saying why; one it can is cleaned once.
#[test]
fn cleanup_clean_by_id() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let ran = Arc::new(AtomicUsize::new(0));
    let counted: Clean = {
        let ran = ran.clone();
        Arc::new(move |_| {
            ran.fetch_add(1, Ordering::SeqCst);
            Ok("Deleted 2 branches.".into())
        })
    };
    let things = vec![
        Thing::new("merged:/r", "r: branches merged", "deletes them", counted)
            .meta("2 branches")
            .lost_nothing(),
        Thing::new("nm:/w", "node_modules in w", "deletes it", noop())
            .at("/w/node_modules")
            .within("/w"),
    ];
    let seen = vec![Session {
        id: "s1".into(),
        cwd: "/w/src".into(),
        state: "working".into(),
    }];
    let a = app_with(kits::git_kit(), things, seen);
    let clean = |id: &str| {
        a.service(Some(&service_call(
            SERVICE_CLEANUP_CLEAN,
            serde_json::json!({"id": id}),
        )))
        .map(|r| serde_json::from_str::<CleanupDone>(r.unwrap().get()).unwrap())
        .map_err(|e| e.message)
    };
    for (id, want) in [
        ("nope", "no such item: nope; measure again and look"),
        (
            "nm:/w",
            "node_modules in w: an agent is working there now, so it's left alone",
        ),
    ] {
        assert_eq!(clean(id).unwrap_err(), want, "{id}");
    }
    a.lock().busy.insert("merged:/r".into());
    assert_eq!(
        clean("merged:/r").unwrap_err(),
        "r: branches merged: it's being cleaned up already"
    );
    a.lock().busy.clear();
    assert_eq!(
        clean("merged:/r").unwrap(),
        CleanupDone {
            said: "r: branches merged: done. Deleted 2 branches.".into(),
            freed: 0,
        }
    );
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    let st = a.lock();
    assert!(
        st.busy.is_empty() && st.measured.is_none(),
        "let go, and measured again soon"
    );
    drop(st);
    // A failed clean is an error that names it.
    let failing: Clean = Arc::new(|_| Err("git: locked".into()));
    let a = app_with(
        kits::git_kit(),
        vec![Thing::new(
            "gc:/r",
            "r: git's objects",
            "runs git gc",
            failing,
        )],
        Vec::new(),
    );
    assert_eq!(
        a.service(Some(&service_call(
            SERVICE_CLEANUP_CLEAN,
            serde_json::json!({"id": "gc:/r"})
        )))
        .unwrap_err()
        .message,
        "r: git's objects: git: locked"
    );
    assert!(a.lock().busy.is_empty());
}
