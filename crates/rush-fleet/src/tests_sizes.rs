//! Rust-only: honest sizes. Each hard-linked file counted once, what's
//! freeable, and the cargo targets outside a worktree tied back to it
//! (temp.rs Space, built.rs).

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rush_core::adapter::TempDir;

use crate::*;

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn write(p: &Path, n: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![1u8; n]).unwrap();
}

fn alloc(p: &Path) -> i64 {
    std::fs::symlink_metadata(p).unwrap().blocks() as i64 * 512
}

// A file is counted once however many of its links are measured, and is
// freeable only when every link is: kache's store keeps what it links.
#[test]
fn space_counts_links_once() {
    // (case, root's name, links beside it inside, links outside)
    for (case, root, inside, outside) in [
        ("one link", "m", 0, 0),
        ("linked twice inside", "m", 1, 0),
        ("three times inside", "m", 2, 0),
        ("linked from kache's store", "m", 0, 1),
        ("inside and from the store", "m", 1, 1),
        ("a scratch target, inside", "hud3-target", 1, 0),
        ("a scratch target, from the store", "hud3-target", 0, 1),
    ] {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join(root);
        let store = d.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        let f = root.join("deps").join("libx.rlib");
        write(&f, 40_000);
        write(&root.join("own.txt"), 9_000);
        for i in 0..inside {
            std::fs::create_dir_all(root.join(format!("l{i}"))).unwrap();
            std::fs::hard_link(&f, root.join(format!("l{i}")).join("libx.rlib")).unwrap();
        }
        for i in 0..outside {
            std::fs::hard_link(&f, store.join(format!("x{i}"))).unwrap();
        }
        let mut folders = alloc(&root) + alloc(&root.join("deps"));
        for i in 0..inside {
            folders += alloc(&root.join(format!("l{i}")));
        }
        let (big, own) = (alloc(&f), alloc(&root.join("own.txt")));
        let want = Space {
            on_disk: folders + big + own,
            freeable: folders + own + if outside == 0 { big } else { 0 },
        };
        let dirs = [TempDir {
            path: s(&root),
            keep: false,
        }];
        assert_eq!(disk_space(&dirs), want, "{case}");
        assert_eq!(disk_usage(&dirs), want.on_disk, "{case}");
        // Paced, and again from a heavy folder's kept total.
        assert_eq!(disk_space_background(&dirs), want, "{case}: paced");
        assert_eq!(disk_space_background(&dirs), want, "{case}: kept");
    }
}

// Two folders measured together share one count: a file linked from one
// into the other is counted once and freed with both.
#[test]
fn space_across_folders() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("wt"), d.path().join("scratch"));
    let f = a.join("f.bin");
    write(&f, 30_000);
    std::fs::create_dir_all(&b).unwrap();
    std::fs::hard_link(&f, b.join("f.bin")).unwrap();
    let td = |p: &Path| TempDir {
        path: s(p),
        keep: false,
    };
    let folders = alloc(&a) + alloc(&b);
    let both = disk_space(&[td(&a), td(&b)]);
    assert_eq!(
        both,
        Space {
            on_disk: folders + alloc(&f),
            freeable: folders + alloc(&f),
        }
    );
    let one = disk_space(&[td(&a)]);
    assert_eq!(one.freeable, alloc(&a), "linked from outside: not freed");
}

/// A cargo target at p, its newest dep-info naming names.
fn target(p: &Path, names: &[&str]) {
    std::fs::create_dir_all(p.join("release").join("deps")).unwrap();
    std::fs::write(
        p.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55",
    )
    .unwrap();
    std::fs::write(p.join(".rustc_info.json"), "{}").unwrap();
    std::fs::write(
        p.join("release").join("deps").join("serde-1.d"),
        "/home/.cargo/registry/src/serde/lib.rs:\n",
    )
    .unwrap();
    for (i, n) in names.iter().enumerate() {
        std::fs::write(
            p.join("release").join("deps").join(format!("own-{i}.d")),
            format!("x.d: {n}/crates/x/src/lib.rs\n"),
        )
        .unwrap();
    }
}

// Each target outside a worktree goes to the one it was built for, by a
// process naming it, its session, what cargo wrote, or its name; to none
// when nothing says which.
#[test]
fn targets_tied_to_worktrees() {
    const NONE: &[&str] = &[];
    struct Case {
        name: &'static str,
        /// Worktree folder names.
        wts: [&'static str; 3],
        /// The agents working in each.
        agents: [&'static [&'static str]; 3],
        /// Targets in session s1's scratch: (name, owner key, the
        /// worktrees its dep-info names).
        scratch: &'static [(&'static str, &'static str, &'static [usize])],
        /// (the worktree a process works in, None elsewhere; target name,
        /// "IN" for one inside worktree 0).
        procs: &'static [(Option<usize>, &'static str)],
        want: [&'static [&'static str]; 3],
    }
    let wts = ["h1-hud3", "sky-depth", "viewer-sky"];
    let cases = [
        Case {
            name: "a process in one worktree names it",
            wts,
            agents: [NONE; 3],
            scratch: &[("lane-target", "", &[])],
            procs: &[(Some(1), "lane-target")],
            want: [NONE, &["lane-target"], NONE],
        },
        Case {
            name: "processes in two worktrees name it",
            wts,
            agents: [NONE; 3],
            scratch: &[],
            procs: &[(Some(0), "lane-target"), (Some(1), "lane-target")],
            want: [NONE; 3],
        },
        Case {
            name: "a process elsewhere names it too (a shared target)",
            wts,
            agents: [NONE; 3],
            scratch: &[],
            procs: &[(Some(0), "lane-target"), (None, "lane-target")],
            want: [NONE; 3],
        },
        Case {
            name: "its session works in that worktree alone",
            wts,
            agents: [&["s1"], NONE, NONE],
            scratch: &[("coord-target", "s1", &[])],
            procs: &[],
            want: [&["coord-target"], NONE, NONE],
        },
        Case {
            name: "its session works in several; cargo's dep-info says which",
            wts,
            agents: [&["s1"], &["s1"], NONE],
            scratch: &[("coord-target", "s1", &[1])],
            procs: &[],
            want: [NONE, &["coord-target"], NONE],
        },
        Case {
            name: "its session works in several; its name says which",
            wts,
            agents: [&["s1"], &["s1"], &["s1"]],
            scratch: &[("hud3-target", "s1", &[])],
            procs: &[],
            want: [&["hud3-target"], NONE, NONE],
        },
        Case {
            name: "its name fits two of them: none",
            wts,
            agents: [NONE, &["s1"], &["s1"]],
            scratch: &[("sky-target", "s1", &[])],
            procs: &[],
            want: [NONE; 3],
        },
        Case {
            name: "a session rush doesn't list: dep-info only",
            wts,
            agents: [NONE; 3],
            scratch: &[("old-target", "", &[2]), ("hud3-target", "", &[])],
            procs: &[],
            want: [NONE, NONE, &["old-target"]],
        },
        Case {
            name: "one inside a worktree is counted there already",
            wts,
            agents: [NONE; 3],
            scratch: &[],
            procs: &[(Some(0), "IN")],
            want: [NONE; 3],
        },
        Case {
            name: "a folder that isn't a cargo target",
            wts,
            agents: [NONE; 3],
            scratch: &[],
            procs: &[(Some(0), "plain")],
            want: [NONE; 3],
        },
        Case {
            name: "the process's word beats the session's",
            wts,
            agents: [&["s1"], NONE, NONE],
            scratch: &[("lane-target", "s1", &[])],
            procs: &[(Some(2), "lane-target")],
            want: [NONE, NONE, &["lane-target"]],
        },
    ];
    for c in cases {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let repo = root.join("repo");
        let paths: Vec<String> = c
            .wts
            .iter()
            .map(|n| s(&repo.join(".claude/worktrees").join(n)))
            .collect();
        let scratch_dir = root.join("tmp").join("repo-1").join("s1");
        let at = |name: &str| match name {
            "IN" => format!("{}/target", paths[0]),
            n => s(&scratch_dir.join(n)),
        };
        let mut found = Vec::new();
        for (n, owner, names) in c.scratch {
            let named: Vec<&str> = names.iter().map(|&i| paths[i].as_str()).collect();
            target(Path::new(&at(n)), &named);
            found.push(ScratchTarget {
                path: at(n),
                owner: owner.to_string(),
            });
        }
        let mut procs = Vec::new();
        for (w, n) in c.procs {
            match *n {
                "plain" => std::fs::create_dir_all(at(n)).unwrap(),
                _ => target(Path::new(&at(n)), &[]),
            }
            procs.push(ProcTarget {
                cwd: w.map_or_else(
                    || s(&root.join("elsewhere")),
                    |i| paths[i].clone() + "/crates",
                ),
                target: at(n),
            });
        }
        let mut ws: Vec<Worktree> = (0..3)
            .map(|i| Worktree {
                path: paths[i].clone(),
                repo: s(&repo),
                agents: c.agents[i].iter().map(|k| k.to_string()).collect(),
                ..Worktree::default()
            })
            .collect();
        tie_targets_with(&mut ws, &found, &procs);
        for ((w, want), name) in ws.iter().zip(c.want).zip(c.wts) {
            let want: Vec<String> = want.iter().map(|&n| at(n)).collect();
            assert_eq!(w.targets, want, "{}: {name}", c.name);
        }
    }
}

// A worktree measured with its targets: their size added, a file kache's
// store links too counted once and not as freed.
#[test]
fn worktree_measured_with_targets() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let wt = root.join("wt");
    write(&wt.join("src.rs"), 5_000);
    let t = root.join("tmp").join("p").join("s1").join("wt-target");
    target(&t, &[]);
    write(&t.join("release").join("deps").join("libbig.rlib"), 80_000);
    let store = root.join("kache");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::hard_link(
        t.join("release").join("deps").join("libbig.rlib"),
        store.join("big"),
    )
    .unwrap();
    let mut w = Worktree {
        path: s(&wt),
        ..Worktree::default()
    };
    w.measure();
    let alone = (w.size, w.freeable);
    w.targets = vec![s(&t)];
    w.measure();
    let both = disk_space(&[
        TempDir {
            path: s(&wt),
            keep: false,
        },
        TempDir {
            path: s(&t),
            keep: false,
        },
    ]);
    assert_eq!((w.size, w.freeable), (both.on_disk, both.freeable));
    let big = alloc(&t.join("release").join("deps").join("libbig.rlib"));
    assert!(w.size >= alone.0 + big, "{w:?}");
    assert_eq!(w.size - w.freeable, big, "the store's file is not freed");
    assert_eq!(w.frees(), w.freeable);
}
