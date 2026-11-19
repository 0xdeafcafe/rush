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
        auto: None,
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

// ---- disk.rs: targets, the in-use check, the auto mode's plan ----

/// A cargo target at root/name with a debug profile: an old and a new
/// artifact in deps, an incremental session, every part last written ago
/// before now except what's named in fresh.
fn target_at(
    root: &std::path::Path,
    name: &str,
    ago: Duration,
    fresh: &[&str],
) -> std::path::PathBuf {
    let t = root.join(name);
    let prof = t.join("debug");
    for d in [".fingerprint", "deps", "incremental/s1"] {
        std::fs::create_dir_all(prof.join(d)).unwrap();
    }
    std::fs::write(
        t.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    std::fs::write(t.join(".rustc_info.json"), "{}").unwrap();
    for f in [
        "deps/libold.rlib",
        "deps/libnew.rlib",
        "incremental/s1/q.bin",
    ] {
        std::fs::write(prof.join(f), vec![7u8; 8192]).unwrap();
    }
    let at = |p: &std::path::Path, when: SystemTime| {
        std::fs::File::open(p)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(when)
                    .set_accessed(when),
            )
            .unwrap();
    };
    let old = SystemTime::now() - ago;
    // Deepest first: writing in a folder touches it.
    for p in [
        "debug/deps/libold.rlib",
        "debug/deps/libnew.rlib",
        "debug/incremental/s1/q.bin",
        "debug/incremental/s1",
        "debug/incremental",
        "debug/deps",
        "debug/.fingerprint",
        "debug",
        ".rustc_info.json",
        "CACHEDIR.TAG",
        "",
    ] {
        let p = if p.is_empty() { t.clone() } else { t.join(p) };
        at(&p, old);
    }
    for p in fresh {
        at(&t.join(p), SystemTime::now());
    }
    t
}

// A target is a folder with both CACHEDIR.TAG and .rustc_info.json, found
// up to 3 levels down, never inside another, never through a link; run
// folders and a tag alone aren't targets.
#[test]
fn targets_found() {
    let d = tempfile::tempdir().unwrap();
    let tmp = d.path();
    let hour = Duration::from_secs(3600);
    let release = target_at(tmp, "h2-1a2b3c4d/sess/release-target", hour, &[]);
    target_at(&release, "nested-target", hour, &[]);
    target_at(tmp, "a/b/c/too-deep-target", hour, &[]);
    std::fs::create_dir_all(tmp.join("h2-1a2b3c4d/sess/run")).unwrap();
    std::fs::create_dir_all(tmp.join("h2-1a2b3c4d/sess/tagged-only")).unwrap();
    std::fs::write(
        tmp.join("h2-1a2b3c4d/sess/tagged-only/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&release, tmp.join("h2-1a2b3c4d/sess/link-target")).unwrap();
    let mut found = std::collections::BTreeSet::new();
    disk::find_targets(tmp, disk::SCAN_DEPTH, &mut found);
    assert_eq!(found.into_iter().collect::<Vec<_>>(), [release]);
}

// The auto mode does nothing at or above 50 GB free; under it, targets
// go after 2 days idle and artifacts at 2 days old; under 20 GB, half an
// hour and 6 hours.
#[test]
fn auto_levels() {
    const GB: u64 = 1 << 30;
    let normal = disk::Level {
        target_idle: disk::TARGET_IDLE,
        artifact_age: disk::ARTIFACT_AGE,
        superseded_age: disk::SUPERSEDED_AGE,
    };
    let low = disk::Level {
        target_idle: disk::LOW_TARGET_IDLE,
        artifact_age: disk::LOW_ARTIFACT_AGE,
        superseded_age: disk::LOW_SUPERSEDED_AGE,
    };
    for (free, want) in [
        (80 * GB, None),
        (50 * GB, None),
        (49 * GB, Some(normal)),
        (20 * GB, Some(normal)),
        (19 * GB, Some(low)),
    ] {
        assert_eq!(disk::level(free), want, "{free}");
    }
    assert_eq!(disk::LOW_ARTIFACT_AGE, Duration::from_secs(6 * 3600));
    for (free, low) in [
        (0, false),
        (GB, true),
        (19 * GB, true),
        (20 * GB, false),
        (49 * GB, false),
    ] {
        assert_eq!(disk::is_low(free), low, "{free}");
    }
    assert_eq!(disk::LOW_EVERY, Duration::from_secs(3 * 60));
}

// A file in deps names its crate and build; a lib's "lib" is dropped so it
// and its test binaries are one crate; anything else is no build.
#[test]
fn builds_named() {
    let h = "0123456789abcdef";
    for (name, want) in [
        (format!("libserde-{h}.rlib"), Some(("serde", h))),
        (format!("libserde-{h}.rmeta"), Some(("serde", h))),
        (format!("serde-{h}.d"), Some(("serde", h))),
        (format!("live_server-{h}"), Some(("live_server", h))),
        (format!("live_server-{h}.dSYM"), Some(("live_server", h))),
        (
            format!("libsqlx_sqlite-{h}.dylib"),
            Some(("sqlx_sqlite", h)),
        ),
        (format!("lib-{h}.rlib"), Some(("lib", h))),
        ("libserde.rlib".to_string(), None),
        ("serde-0123.d".to_string(), None),
        (format!("serde-{}.d", "g".repeat(16)), None),
        (format!("-{h}.d"), None),
    ] {
        assert_eq!(disk::build_of(&name), want, "{name}");
    }
}

// Of one crate's builds, the 3 newest stay however old; older ones go once
// their newest file is old enough. Other crates and other files are left.
#[test]
fn superseded_builds() {
    let d = tempfile::tempdir().unwrap();
    let t = target_at(d.path(), "t", Duration::from_secs(60), &[]);
    let deps = t.join("debug/deps");
    let now = SystemTime::now();
    let at = |p: &std::path::Path, when: SystemTime| {
        std::fs::File::open(p)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap();
    };
    let hours = |n: u64| now - Duration::from_secs(n * 3600);
    // Five builds of foo, newest first: 0, 1, 2, 3 and 30 hours old.
    let mut files = Vec::new();
    for (i, ago) in [0, 1, 2, 3, 30].into_iter().enumerate() {
        let h = format!("{i:016x}");
        for f in [format!("libfoo-{h}.rlib"), format!("foo-{h}.d")] {
            let p = deps.join(&f);
            std::fs::write(&p, b"x").unwrap();
            at(&p, hours(ago));
            files.push(p);
        }
    }
    // One old build of bar: the newest of its crate, so it stays.
    let bar = deps.join("libbar-ffffffffffffffff.rlib");
    std::fs::write(&bar, b"x").unwrap();
    at(&bar, hours(100));
    let rel = |v: Vec<std::path::PathBuf>| -> Vec<String> {
        v.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    // An hour: the 4th (3 hours) and 5th (30 hours) builds go.
    assert_eq!(
        rel(disk::superseded_artifacts(
            &t,
            now,
            disk::LOW_SUPERSEDED_AGE
        )),
        [
            "foo-0000000000000003.d",
            "foo-0000000000000004.d",
            "libfoo-0000000000000003.rlib",
            "libfoo-0000000000000004.rlib",
        ]
    );
    // A day: only the 30-hour one.
    assert_eq!(
        rel(disk::superseded_artifacts(&t, now, disk::SUPERSEDED_AGE)),
        ["foo-0000000000000004.d", "libfoo-0000000000000004.rlib"]
    );
}

// A target built in every few minutes is never idle: with plenty free its
// 7-hour-old artifacts stay, and under 20 GB they go.
#[test]
fn low_disk_prunes_younger_artifacts() {
    const GB: u64 = 1 << 30;
    let d = tempfile::tempdir().unwrap();
    let scratch = d.path().join("tmp");
    let busy = target_at(
        &scratch,
        "p/s1/busy-target",
        Duration::from_secs(7 * 3600),
        &["debug/.fingerprint", "debug/deps/libnew.rlib"],
    );
    let plan = |free: u64| {
        disk::plan(
            &[],
            std::slice::from_ref(&scratch),
            SystemTime::now(),
            disk::level(free).unwrap(),
            &|_| false,
            &|_| false,
        )
        .0
    };
    assert_eq!(plan(30 * GB), []);
    assert_eq!(
        plan(10 * GB),
        [disk::Act::Prune {
            target: busy.clone(),
            paths: vec![
                busy.join("debug/deps/libold.rlib"),
                busy.join("debug/incremental/s1"),
            ],
        }]
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

// Each tick is in the event log: what it did, or why it did nothing.
#[test]
fn ticks_logged() {
    let quiet = tick_fields(
        "rust-clean",
        60 << 30,
        None,
        &Tick::default(),
        "enough free",
    );
    assert_eq!(quiet["why"], "enough free");
    assert_eq!(quiet["free"], 60u64 << 30);
    assert!(quiet.get("target_idle").is_none() && quiet.get("kept").is_none());
    let t = Tick {
        did: vec!["emptied kache's store".into()],
        freed: 5,
        kept: vec!["/t: idle, but in use".into()],
        failed: None,
    };
    let f = tick_fields("rust-clean", 10 << 30, disk::level(10 << 30), &t, "");
    assert_eq!(f["artifact_age"], 6 * 3600);
    assert_eq!(f["did"], json!(["emptied kache's store"]));
    assert_eq!(f["kept"], json!(["/t: idle, but in use"]));
    assert!(f.get("why").is_none());
}

// In use: lsof printed anything, or couldn't run; ps couldn't run; or a
// process other than rush names the path in its arguments or environment.
#[test]
fn in_use_from_what_lsof_and_ps_said() {
    let t = std::path::Path::new("/cfg/tmp/h2/s1/release-target");
    let ps = "  101 cargo build CARGO_TARGET_DIR=/cfg/tmp/h2/s1/release-target HOME=/u\n  7 rush plugin /cfg/tmp/h2/s1/release-target\n";
    let quiet = "  101 sleep 60 HOME=/u\n";
    for (lsof, ps, want, why) in [
        (None, Some(quiet), true, "lsof couldn't run"),
        (
            Some(&b"cargo 101 u cwd DIR /cfg/tmp\n"[..]),
            Some(quiet),
            true,
            "a file is open",
        ),
        (Some(&b""[..]), None, true, "ps couldn't run"),
        (
            Some(&b""[..]),
            Some(ps),
            true,
            "cargo names it in its environment",
        ),
        (Some(&b""[..]), Some(quiet), false, "nothing uses it"),
        (
            Some(&b""[..]),
            Some("  7 rush plugin /cfg/tmp/h2/s1/release-target\n"),
            false,
            "only rush names it",
        ),
    ] {
        assert_eq!(disk::in_use_from(lsof, ps, t, 7), want, "{why}");
    }
}

// Removing frees only files with no other hard link: kache's links free
// nothing.
#[test]
fn freeable_counts_lone_files() {
    let d = tempfile::tempdir().unwrap();
    let t = d.path().join("t");
    std::fs::create_dir_all(&t).unwrap();
    std::fs::write(t.join("own"), vec![1u8; 64 << 10]).unwrap();
    std::fs::write(d.path().join("store"), vec![1u8; 64 << 10]).unwrap();
    std::fs::hard_link(d.path().join("store"), t.join("linked")).unwrap();
    let own = disk::freeable(&t.join("own"));
    assert!(own >= 64 << 10, "{own}");
    assert_eq!(disk::freeable(&t.join("linked")), 0);
    assert_eq!(disk::freeable(&t), own);
}

// The plan: an idle, unused target goes whole; an idle one in use stays;
// in a target built in lately, only artifacts older than 2 days go; one a
// session works on is left out. Each removal asks again, and one in use
// by then stays.
#[test]
fn auto_plan_and_apply() {
    let d = tempfile::tempdir().unwrap();
    let scratch = d.path().join("tmp");
    let three_days = Duration::from_secs(3 * 24 * 3600);
    let idle = target_at(&scratch, "p/s1/idle-target", three_days, &[]);
    let held = target_at(&scratch, "p/s2/held-target", three_days, &[]);
    let live = target_at(
        &scratch,
        "p/s3/live-target",
        three_days,
        &["debug/.fingerprint", "debug/deps/libnew.rlib"],
    );
    let working = target_at(&scratch, "p/s4/work-target", three_days, &[]);
    let used = |p: &std::path::Path| p == held;
    let skip = |p: &std::path::Path| p.starts_with(scratch.join("p/s4"));
    let (acts, kept) = disk::plan(
        &[],
        std::slice::from_ref(&scratch),
        SystemTime::now(),
        disk::level(30 << 30).unwrap(),
        &skip,
        &used,
    );
    assert_eq!(
        acts,
        [
            disk::Act::Target(idle.clone()),
            disk::Act::Prune {
                target: live.clone(),
                paths: vec![
                    live.join("debug/deps/libold.rlib"),
                    live.join("debug/incremental/s1"),
                ],
            },
            // A session works in it: never whole, but its old artifacts go.
            disk::Act::Prune {
                target: working.clone(),
                paths: vec![
                    working.join("debug/deps/libnew.rlib"),
                    working.join("debug/deps/libold.rlib"),
                    working.join("debug/incremental/s1"),
                ],
            },
        ]
    );
    let kept: Vec<_> = kept.into_iter().map(|(p, _)| p).collect();
    assert_eq!(kept, std::slice::from_ref(&held));
    // Asked again right before: now in use, so it stays.
    assert!(disk::apply(&acts[0], &|_| true).is_err());
    assert!(idle.exists());
    assert!(disk::apply(&acts[0], &|_| false).unwrap() > 0);
    assert!(!idle.exists());
    assert!(disk::apply(&acts[1], &|_| false).unwrap() > 0);
    assert!(!live.join("debug/deps/libold.rlib").exists());
    assert!(!live.join("debug/incremental/s1").exists());
    assert!(live.join("debug/deps/libnew.rlib").exists() && disk::is_target(&live));
}

// What an auto mode says: nothing when it did nothing, else what and how
// much, and the first thing left alone.
#[test]
fn auto_says() {
    assert_eq!(
        kits::auto_said("rust-clean", 10 << 30, &[], 0, Some("x".into())),
        None
    );
    assert_eq!(
        kits::auto_said(
            "rust-clean",
            10 << 30,
            &["removed 2 targets no build touched lately".into()],
            3 << 30,
            Some("/t: in use now; it stays".into())
        )
        .as_deref(),
        Some(
            "rust-clean cleaned on its own with 10.0 GB free: removed 2 targets no build touched lately, freeing 3.0 GB · left alone: /t: in use now; it stays"
        )
    );
}
