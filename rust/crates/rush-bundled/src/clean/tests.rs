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
