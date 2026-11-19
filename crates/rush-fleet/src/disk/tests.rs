//! The disk engine through its public API: levels, the rules file, what
//! plugins hand in.

use std::fs::FileTimes;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use rush_core::time::Ms;
use serde_json::json;

use crate::disk::*;

const GB: u64 = 1 << 30;

/// rush's folder, for this test's thread only.
struct RushHome(tempfile::TempDir);

impl RushHome {
    fn new() -> RushHome {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().canonicalize().unwrap()));
        RushHome(d)
    }

    fn file(&self) -> PathBuf {
        self.0.path().canonicalize().unwrap().join(DISK_RULES_FILE)
    }
}

impl Drop for RushHome {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
    }
}

// Nothing at or above 50 GB free, or when free can't be read (0); normal
// under it, low under 20 GB.
#[test]
fn levels_by_free_space() {
    for (free, want) in [
        (0, None),
        (80 * GB, None),
        (50 * GB, None),
        (50 * GB - 1, Some(Level::Normal)),
        (20 * GB, Some(Level::Normal)),
        (20 * GB - 1, Some(Level::Low)),
        (GB, Some(Level::Low)),
    ] {
        assert_eq!(level(free), want, "{free}");
    }
}

fn rule<'a>(rules: &'a [Rule], id: &str) -> &'a Rule {
    rules.iter().find(|r| r.id == id).unwrap()
}

fn secs(p: Option<&Params>) -> (Option<u64>, Option<u64>, Option<u32>, Option<u64>) {
    let p = p.cloned().unwrap_or_default();
    (p.idle_secs, p.age_secs, p.keep, p.superseded_secs)
}

// The built-in rules are what the clean-ups did on their own: targets 2
// days idle (30 minutes low), artifacts 2 days old (6 hours low), builds
// past the newest 3 a day old (an hour low), worktrees 3 days idle,
// kache's store only when low; and Clean up's cache folders, only when
// low.
#[test]
fn built_in_rules_are_todays() {
    let r = built_in_rules();
    const H: u64 = 3600;
    for (id, lv, want) in [
        (
            "cargo-target",
            Level::Normal,
            (Some(48 * H), None, None, None),
        ),
        ("cargo-target", Level::Low, (Some(1800), None, None, None)),
        (
            "cargo-artifacts",
            Level::Normal,
            (None, Some(48 * H), Some(3), Some(24 * H)),
        ),
        (
            "cargo-artifacts",
            Level::Low,
            (None, Some(6 * H), Some(3), Some(H)),
        ),
        ("worktree", Level::Normal, (Some(72 * H), None, None, None)),
        ("worktree", Level::Low, (Some(72 * H), None, None, None)),
        ("kache", Level::Low, (None, None, None, None)),
        (
            "cache:.cache/prisma",
            Level::Low,
            (Some(30 * 24 * H), None, None, None),
        ),
        (
            "cache:Library/Caches/go-build",
            Level::Low,
            (Some(0), None, None, None),
        ),
    ] {
        let rl = rule(&r, id);
        assert!(rl.at(lv).is_some(), "{id} runs {lv:?}");
        assert_eq!(secs(rl.at(lv)), want, "{id} {lv:?}");
    }
    for id in ["kache", "cache:Library/Caches/go-build", "go-mod-cache"] {
        assert_eq!(rule(&r, id).at(Level::Normal), None, "{id} only when low");
    }
    assert_eq!(
        rule(&r, "kache").kind,
        Kind::ToolCommand {
            command: ["kache", "clean", "--cache", "--yes"]
                .map(String::from)
                .to_vec(),
            why: "kache's store; builds fill it again".into(),
        }
    );
    assert_eq!(
        rule(&r, "cache:.nx").kind,
        Kind::CacheDir {
            path: "~/.nx".into(),
            why: "an Nx workspace's cache".into(),
            each: true,
            go_clean: false,
        }
    );
    // Every BUILT_IN cache is a rule, and Go's module cache.
    assert_eq!(
        r.iter()
            .filter(|r| matches!(r.kind, Kind::CacheDir { .. }))
            .count(),
        crate::BUILT_IN.len() + 1
    );
    // A rule turned off runs at no level.
    let mut off = rule(&r, "cargo-target").clone();
    off.enabled = false;
    assert_eq!(off.at(Level::Low), None);
}

// No file: the built-in rules. A change keeps only what differs from
// them, so an untouched built-in follows rush's defaults; your rules and
// proposals stay; a built-in can be turned off but not removed.
#[test]
fn rules_file_keeps_your_changes() {
    let home = RushHome::new();
    let none = load_rules().unwrap();
    assert_eq!(none.rules, built_in_rules());
    assert!(none.proposed.is_empty());
    assert!(!home.file().exists());

    let mine = Rule {
        id: "node-modules".into(),
        title: "node_modules nobody installed into lately".into(),
        kind: Kind::PluginCandidate {
            kind: "node-modules".into(),
        },
        enabled: true,
        normal: None,
        low: Some(Params {
            idle_secs: Some(7 * 24 * 3600),
            ..Params::default()
        }),
    };
    let proposal = Proposal {
        rule: Rule {
            id: "xcode-derived".into(),
            ..mine.clone()
        },
        evidence: "~/Library/Developer/Xcode/DerivedData: 12G, Xcode rebuilds it".into(),
        by: "cleanup agent".into(),
        at: 1_700_000_000_000,
    };
    let wrote = update_rules(|f| {
        f.rules
            .iter_mut()
            .find(|r| r.id == "worktree")
            .unwrap()
            .enabled = false;
        f.rules.retain(|r| r.id != "kache");
        f.rules.push(mine.clone());
        f.proposed.push(proposal.clone());
    })
    .unwrap();
    assert_eq!(wrote, load_rules().unwrap());
    assert!(!rule(&wrote.rules, "worktree").enabled);
    assert_eq!(
        rule(&wrote.rules, "kache"),
        rule(&built_in_rules(), "kache")
    );
    assert_eq!(wrote.rules.last(), Some(&mine));
    assert_eq!(wrote.proposed, [proposal]);

    // The file: only the turned-off built-in, yours, and the proposal.
    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.file()).unwrap()).unwrap();
    let ids: Vec<&str> = on_disk["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["worktree", "node-modules"]);
    assert_eq!(on_disk["rules"][1]["kind"]["type"], "plugin-candidate");
    assert_eq!(on_disk["rules"][1]["low"]["idleSecs"], 7 * 24 * 3600);
    assert_eq!(on_disk["proposed"][0]["rule"]["id"], "xcode-derived");
    assert!(home.0.path().join(DISK_RULES_LOCK).exists());

    // A second change keeps the first.
    update_rules(|f| f.proposed.clear()).unwrap();
    let again = load_rules().unwrap();
    assert!(!rule(&again.rules, "worktree").enabled);
    assert!(again.proposed.is_empty());
}

// A file that can't be read is an error, never the defaults, and a
// change leaves it as it is.
#[test]
fn unreadable_rules_file_stays() {
    let home = RushHome::new();
    std::fs::write(home.file(), "{ not json").unwrap();
    assert!(load_rules().is_err());
    assert!(update_rules(|f| f.rules.clear()).is_err());
    assert_eq!(std::fs::read_to_string(home.file()).unwrap(), "{ not json");
}

// What a plugin hands in, as rush-plugin's DiskCandidates writes it: each
// marked as the plugin's; what isn't absolute or claims a kind the engine
// finds itself is left out.
#[test]
fn plugin_candidates_handed_in() {
    let answer = json!({"candidates": [
        {
            "path": "/Users/me/src/app/node_modules",
            "kind": "node-modules",
            "why": "app's packages",
            "bytesOnDisk": 900,
            "bytesFreeable": 800,
            "facts": {"mtime": 1_700_000_000_000i64, "inUse": "no", "session": "s1"}
        },
        {"path": "relative/node_modules", "kind": "node-modules"},
        {"path": "/Users/me/src/app/target", "kind": "cargo-target"},
        {"path": "/Users/me/src/app/x", "kind": ""}
    ]});
    let got = from_answer("js-clean", &answer).unwrap();
    assert_eq!(
        got,
        [Candidate {
            path: "/Users/me/src/app/node_modules".into(),
            kind: "node-modules".into(),
            why: "app's packages".into(),
            plugin: "js-clean".into(),
            bytes_on_disk: 900,
            bytes_freeable: 800,
            facts: Facts {
                mtime: Some(1_700_000_000_000),
                in_use: InUse::No,
                session: "s1".into(),
                ..Facts::default()
            },
            ..Candidate::default()
        }]
    );
    assert!(from_answer("x", &json!({"candidates": 3})).is_err());
    assert_eq!(from_answer("x", &json!({})).unwrap(), []);
}

// ---- cargo targets: found, judged, pruned ----

const HOUR_MS: Ms = 3_600_000;
const DAY_MS: Ms = 24 * HOUR_MS;

fn now_ms() -> Ms {
    rush_core::time::now()
}

/// Sets p's modified time to at.
fn set_mtime(p: &Path, at: Ms) {
    let t = UNIX_EPOCH + Duration::from_millis(at as u64);
    std::fs::File::open(p)
        .unwrap()
        .set_times(FileTimes::new().set_modified(t).set_accessed(t))
        .unwrap();
}

/// A cargo target at root/name with a debug profile: an old and a new
/// artifact in deps, an incremental session, every part last written ago
/// before now except what's named in fresh.
fn target_at(root: &Path, name: &str, ago: Ms, fresh: &[&str]) -> PathBuf {
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
    let old = now_ms() - ago;
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
        set_mtime(&p, old);
    }
    for p in fresh {
        set_mtime(&t.join(p), now_ms());
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
    let release = target_at(tmp, "h2-1a2b3c4d/sess/release-target", HOUR_MS, &[]);
    target_at(&release, "nested-target", HOUR_MS, &[]);
    target_at(tmp, "a/b/c/too-deep-target", HOUR_MS, &[]);
    std::fs::create_dir_all(tmp.join("h2-1a2b3c4d/sess/run")).unwrap();
    std::fs::create_dir_all(tmp.join("h2-1a2b3c4d/sess/tagged-only")).unwrap();
    std::fs::write(
        tmp.join("h2-1a2b3c4d/sess/tagged-only/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&release, tmp.join("h2-1a2b3c4d/sess/link-target")).unwrap();
    let mut found = std::collections::BTreeSet::new();
    find_targets(tmp, SCAN_DEPTH, &mut found);
    assert_eq!(found.into_iter().collect::<Vec<_>>(), [release]);
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
        assert_eq!(build_of(&name), want, "{name}");
    }
}

// Of one crate's builds, the 3 newest stay however old; older ones go once
// their newest file is old enough. Other crates and other files are left.
#[test]
fn superseded_builds() {
    let d = tempfile::tempdir().unwrap();
    let t = target_at(d.path(), "t", 60_000, &[]);
    let deps = t.join("debug/deps");
    let now = now_ms();
    // Five builds of foo, newest first: 0, 1, 2, 3 and 30 hours old.
    for (i, ago) in [0, 1, 2, 3, 30].into_iter().enumerate() {
        let h = format!("{i:016x}");
        for f in [format!("libfoo-{h}.rlib"), format!("foo-{h}.d")] {
            let p = deps.join(&f);
            std::fs::write(&p, b"x").unwrap();
            set_mtime(&p, now - ago * HOUR_MS);
        }
    }
    // One old build of bar: the newest of its crate, so it stays.
    let bar = deps.join("libbar-ffffffffffffffff.rlib");
    std::fs::write(&bar, b"x").unwrap();
    set_mtime(&bar, now - 100 * HOUR_MS);
    let arts = artifacts(&t);
    let names = |v: Vec<PathBuf>| -> Vec<String> {
        v.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    for (min_age, want) in [
        (
            HOUR_MS,
            vec![
                "foo-0000000000000003.d",
                "foo-0000000000000004.d",
                "libfoo-0000000000000003.rlib",
                "libfoo-0000000000000004.rlib",
            ],
        ),
        (
            DAY_MS,
            vec!["foo-0000000000000004.d", "libfoo-0000000000000004.rlib"],
        ),
    ] {
        assert_eq!(
            names(superseded_artifacts(&arts, now, 3, min_age)),
            want,
            "{min_age}"
        );
    }
}

// In use: lsof printed anything; a process other than rush names the path
// in its arguments or environment. lsof or ps not running is not knowing.
#[test]
fn in_use_from_what_lsof_and_ps_said() {
    let t = Path::new("/cfg/tmp/h2/s1/release-target");
    let ps = "  101 cargo build CARGO_TARGET_DIR=/cfg/tmp/h2/s1/release-target HOME=/u\n  7 rush plugin /cfg/tmp/h2/s1/release-target\n";
    let quiet = "  101 sleep 60 HOME=/u\n";
    for (lsof, ps, want, why) in [
        (None, Some(quiet), InUse::Unknown, "lsof couldn't run"),
        (
            Some(&b"cargo 101 u cwd DIR /cfg/tmp\n"[..]),
            Some(quiet),
            InUse::Yes,
            "a file is open",
        ),
        (Some(&b""[..]), None, InUse::Unknown, "ps couldn't run"),
        (
            Some(&b""[..]),
            Some(ps),
            InUse::Yes,
            "cargo names it in its environment",
        ),
        (Some(&b""[..]), Some(quiet), InUse::No, "nothing uses it"),
        (
            Some(&b""[..]),
            Some("  7 rush plugin /cfg/tmp/h2/s1/release-target\n"),
            InUse::No,
            "only rush names it",
        ),
    ] {
        assert_eq!(in_use_from(lsof, ps, t, 7), want, "{why}");
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
    let own = freeable(&t.join("own"));
    assert!(own >= 64 << 10, "{own}");
    assert_eq!(freeable(&t.join("linked")), 0);
    assert_eq!(freeable(&t), own);
}

fn scan_of(scratch: &Path, sessions: Vec<Session>) -> Scan {
    Scan {
        scratch: vec![scratch.to_path_buf()],
        sessions,
        home: std::env::var("HOME").unwrap(),
        cwds: Some(Vec::new()),
        ..Scan::default()
    }
}

// The built-in rules at normal, end to end on disk: an idle, unused
// target goes whole; an idle one in use stays; in a target built in
// lately only artifacts older than 2 days go; one a session is working on
// is never removed whole, but its old artifacts go. Each removal asks
// again, and one in use by then stays.
#[test]
fn targets_planned_and_applied() {
    let d = tempfile::tempdir().unwrap();
    let scratch = d.path().join("tmp");
    let three_days = 3 * DAY_MS;
    let idle = target_at(&scratch, "p/s1/idle-target", three_days, &[]);
    let held = target_at(&scratch, "p/s2/held-target", three_days, &[]);
    let live = target_at(
        &scratch,
        "p/s3/live-target",
        three_days,
        &["debug/.fingerprint", "debug/deps/libnew.rlib"],
    );
    let working = target_at(&scratch, "p/s4/work-target", three_days, &[]);
    let rules = built_in_rules();
    let now = now_ms();
    let scan = scan_of(
        &scratch,
        vec![Session {
            id: "s4".into(),
            working: true,
            ..Session::default()
        }],
    );
    let mut cands = find_candidates(&scan, &rules, Level::Normal, now);
    for c in &mut cands {
        if c.path == held {
            c.facts.in_use = InUse::Yes;
        }
    }
    assert!(cands.iter().all(|c| c.kind == CARGO_TARGET));
    let (acts, kept) = plan(&cands, &rules, Level::Normal, now);
    let got: Vec<(&Path, &str, &How)> = acts
        .iter()
        .map(|a| (a.path.as_path(), a.rule.as_str(), &a.how))
        .collect();
    assert_eq!(
        got,
        [
            (idle.as_path(), "cargo-target", &How::Remove),
            (
                live.as_path(),
                "cargo-artifacts",
                &How::Prune {
                    paths: vec![
                        live.join("debug/deps/libold.rlib"),
                        live.join("debug/incremental/s1"),
                    ],
                }
            ),
            // A session works in it: never whole, but its old artifacts go.
            (
                working.as_path(),
                "cargo-artifacts",
                &How::Prune {
                    paths: vec![
                        working.join("debug/deps/libnew.rlib"),
                        working.join("debug/deps/libold.rlib"),
                        working.join("debug/incremental/s1"),
                    ],
                }
            ),
        ]
    );
    assert_eq!(
        kept,
        [Kept {
            path: held.clone(),
            rule: "cargo-target".into(),
            why: "idle, but in use".into(),
        }]
    );
    // Asked again right before: in use now, or not known, so it stays.
    for now_used in [InUse::Yes, InUse::Unknown] {
        assert!(apply(&acts[0], &|_| now_used).is_err());
        assert!(idle.exists());
    }
    assert!(apply(&acts[0], &|_| InUse::No).unwrap() > 0);
    assert!(!idle.exists());
    let o = apply_all(&acts[1..2], Vec::new(), &|_| InUse::No);
    assert!(o.freed > 0 && o.left.is_empty(), "{o:?}");
    assert_eq!(o.did[0].what, "removed 2 old build files in it");
    assert!(!live.join("debug/deps/libold.rlib").exists());
    assert!(!live.join("debug/incremental/s1").exists());
    assert!(live.join("debug/deps/libnew.rlib").exists() && is_target(&live));
    // A target gone since it was planned isn't one any more.
    let gone = apply_all(&acts[..1], Vec::new(), &|_| InUse::No);
    assert!(gone.left[0].why.ends_with("no longer a target"), "{gone:?}");
}

// A target built in every few minutes is never idle: with plenty free its
// 7-hour-old artifacts stay, and when low they go.
#[test]
fn low_disk_prunes_younger_artifacts() {
    let d = tempfile::tempdir().unwrap();
    let scratch = d.path().join("tmp");
    let busy = target_at(
        &scratch,
        "p/s1/busy-target",
        7 * HOUR_MS,
        &["debug/.fingerprint", "debug/deps/libnew.rlib"],
    );
    // Go's module cache, wherever this machine keeps it, isn't this test's.
    let mut rules = built_in_rules();
    rules.retain(|r| r.id != "go-mod-cache");
    let scan = scan_of(&scratch, Vec::new());
    let plan_at = |lv: Level| {
        let now = now_ms();
        let cands = find_candidates(&scan, &rules, lv, now);
        plan(&cands, &rules, lv, now).0
    };
    assert_eq!(plan_at(Level::Normal), []);
    let low = plan_at(Level::Low);
    let hows: Vec<&How> = low.iter().map(|a| &a.how).collect();
    assert_eq!(
        hows,
        [
            &How::Prune {
                paths: vec![
                    busy.join("debug/deps/libold.rlib"),
                    busy.join("debug/incremental/s1"),
                ],
            },
            // kache's store is emptied when low, after every removal.
            &How::Run {
                command: ["kache", "clean", "--cache", "--yes"]
                    .map(String::from)
                    .to_vec(),
            },
        ]
    );
}

// ---- the plan, without the disk ----

fn target(path: &str, last_build_ago: Ms, f: impl FnOnce(&mut Facts)) -> Candidate {
    let mut facts = Facts {
        last_build: Some(T - last_build_ago),
        ..Facts::default()
    };
    f(&mut facts);
    Candidate {
        path: path.into(),
        kind: CARGO_TARGET.into(),
        facts,
        ..Candidate::default()
    }
}

fn wt(path: &str, idle: Ms, f: impl FnOnce(&mut Facts)) -> Candidate {
    let mut facts = Facts {
        mtime: Some(T - idle),
        merged: "feat is merged into origin/main".into(),
        changed: Some(0),
        held: Some(0),
        repo: Some("/r".into()),
        ..Facts::default()
    };
    f(&mut facts);
    Candidate {
        path: path.into(),
        kind: WORKTREE.into(),
        facts,
        ..Candidate::default()
    }
}

fn art(path: &str, ago: Ms) -> Artifact {
    Artifact {
        path: path.into(),
        mtime: T - ago,
        profile: "/t/debug".into(),
        ..Artifact::default()
    }
}

const T: Ms = 1_800_000_000_000;

/// What a plan does and keeps, as said() words them.
type Said<'a> = (Vec<&'a str>, Vec<&'a str>);

/// The plan, shortly: each act as "rule path how", each kept as "rule
/// path: why".
fn said(cands: &[Candidate], rules: &[Rule], lv: Level) -> (Vec<String>, Vec<String>) {
    let (acts, kept) = plan(cands, rules, lv, T);
    let how = |h: &How| match h {
        How::Remove => "remove".to_string(),
        How::Prune { paths } => format!(
            "prune {}",
            paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(",")
        ),
        How::RemoveWorktree { repo } => format!("worktree in {}", repo.display()),
        How::GoClean => "go clean".into(),
        How::Run { command } => format!("run {}", command.join(" ")),
    };
    (
        acts.iter()
            .map(|a| format!("{} {} {}", a.rule, a.path.display(), how(&a.how)))
            .collect(),
        kept.iter()
            .map(|k| format!("{} {}: {}", k.rule, k.path.display(), k.why))
            .collect(),
    )
}

fn cache_rule(id: &str, idle_secs: u64, go_clean: bool) -> Rule {
    Rule {
        id: id.into(),
        title: id.into(),
        kind: Kind::CacheDir {
            path: "~/c".into(),
            why: "a cache".into(),
            each: false,
            go_clean,
        },
        enabled: true,
        normal: None,
        low: Some(Params {
            idle_secs: Some(idle_secs),
            ..Params::default()
        }),
    }
}

fn plugin_rule(kind: &str, idle_secs: u64) -> Rule {
    Rule {
        id: format!("plugin:{kind}"),
        title: "what a plugin found".into(),
        kind: Kind::PluginCandidate { kind: kind.into() },
        enabled: true,
        normal: Some(Params {
            idle_secs: Some(idle_secs),
            ..Params::default()
        }),
        low: None,
    }
}

// Each kind of candidate, against the built-in rules (and a cache and a
// plugin rule): what goes, what stays and why, and what's neither.
#[test]
fn plan_table() {
    let mut rules = built_in_rules();
    rules.push(cache_rule("cache:now", 0, false));
    rules.push(cache_rule("cache:week", 7 * 24 * 3600, false));
    rules.push(cache_rule("gomod", 0, true));
    rules.push(plugin_rule("node-modules", 7 * 24 * 3600));
    let kache = "kache ";
    let none: Said = (vec![], vec![]);
    let cache = |rule: &str, ago: Option<Ms>, used: InUse| Candidate {
        path: "/c".into(),
        kind: CACHE_DIR.into(),
        rule: rule.into(),
        facts: Facts {
            mtime: ago.map(|a| T - a),
            in_use: used,
            ..Facts::default()
        },
        ..Candidate::default()
    };
    let nm = |ago: Ms, f: fn(&mut Facts)| {
        let mut c = Candidate {
            path: "/n".into(),
            kind: "node-modules".into(),
            plugin: "js-clean".into(),
            why: "app's packages".into(),
            facts: Facts {
                mtime: Some(T - ago),
                ..Facts::default()
            },
            ..Candidate::default()
        };
        f(&mut c.facts);
        c
    };
    let cases: Vec<(&str, Candidate, Level, Said)> = vec![
        (
            "a target idle 3 days goes whole",
            target("/t", 3 * DAY_MS, |_| {}),
            Level::Normal,
            (vec!["cargo-target /t remove"], vec![]),
        ),
        (
            "idle 1 hour: only when low",
            target("/t", HOUR_MS, |_| {}),
            Level::Normal,
            none.clone(),
        ),
        (
            "idle 1 hour, low",
            target("/t", HOUR_MS, |_| {}),
            Level::Low,
            (vec!["cargo-target /t remove", kache], vec![]),
        ),
        (
            "idle but in use",
            target("/t", 3 * DAY_MS, |f| f.in_use = InUse::Yes),
            Level::Normal,
            (vec![], vec!["cargo-target /t: idle, but in use"]),
        ),
        (
            "not knowing whether it's used plans it; apply asks",
            target("/t", 3 * DAY_MS, |f| f.in_use = InUse::Unknown),
            Level::Normal,
            (vec!["cargo-target /t remove"], vec![]),
        ),
        (
            "a working session's target is never whole",
            target("/t", 3 * DAY_MS, |f| f.working = true),
            Level::Normal,
            (
                vec![],
                vec!["cargo-artifacts /t: a session is working on it"],
            ),
        ),
        (
            "a working session's target is pruned",
            target("/t", 3 * DAY_MS, |f| {
                f.working = true;
                f.artifacts = vec![art("/t/debug/deps/libold.rlib", 3 * DAY_MS)];
            }),
            Level::Normal,
            (
                vec!["cargo-artifacts /t prune /t/debug/deps/libold.rlib"],
                vec![],
            ),
        ),
        (
            "old artifacts in a busy target, a build using it",
            target("/t", HOUR_MS, |f| {
                f.in_use = InUse::Yes;
                f.artifacts = vec![art("/t/debug/deps/libold.rlib", 3 * DAY_MS)];
            }),
            Level::Normal,
            (
                vec![],
                vec!["cargo-artifacts /t: has old builds, but a build is using it"],
            ),
        ),
        (
            "artifacts 7 hours old go only when low",
            target("/t", HOUR_MS, |f| {
                f.artifacts = vec![art("/t/debug/deps/libold.rlib", 7 * HOUR_MS)];
            }),
            Level::Normal,
            none.clone(),
        ),
        (
            "a clean, merged, idle worktree goes",
            wt("/w", 4 * DAY_MS, |_| {}),
            Level::Normal,
            (vec!["worktree /w worktree in /r"], vec![]),
        ),
        (
            "a worktree idle 2 days isn't due",
            wt("/w", 2 * DAY_MS, |_| {}),
            Level::Normal,
            none.clone(),
        ),
        (
            "a worktree a session is working in",
            wt("/w", 4 * DAY_MS, |f| f.working = true),
            Level::Normal,
            none.clone(),
        ),
        (
            "the main checkout",
            wt("/w", 4 * DAY_MS, |f| f.main = true),
            Level::Normal,
            none.clone(),
        ),
        (
            "a worktree with reasons to stay",
            wt("/w", 4 * DAY_MS, |f| {
                f.locked = true;
                f.changed = Some(2);
                f.held = Some(1);
                f.merged.clear();
            }),
            Level::Normal,
            (
                vec![],
                vec![
                    "worktree /w: locked, 2 uncommitted files, 1 process working there, its commits aren't all in the default branch",
                ],
            ),
        ),
        (
            "a worktree git and the process table couldn't look at",
            wt("/w", 4 * DAY_MS, |f| {
                f.changed = None;
                f.held = None;
            }),
            Level::Normal,
            (
                vec![],
                vec!["worktree /w: git status failed, can't tell what runs there"],
            ),
        ),
        (
            "a cache folder only when low",
            cache("cache:now", Some(0), InUse::Unknown),
            Level::Normal,
            none.clone(),
        ),
        (
            "a cache folder, low",
            cache("cache:now", Some(0), InUse::Unknown),
            Level::Low,
            (vec!["cache:now /c remove", kache], vec![]),
        ),
        (
            "a cache folder written lately",
            cache("cache:week", Some(DAY_MS), InUse::No),
            Level::Low,
            (vec![kache], vec![]),
        ),
        (
            "a cache folder untouched a week",
            cache("cache:week", Some(8 * DAY_MS), InUse::Yes),
            Level::Low,
            (vec![kache], vec!["cache:week /c: in use"]),
        ),
        (
            "Go's module cache goes through go clean",
            cache("gomod", None, InUse::Unknown),
            Level::Low,
            (vec!["gomod /c go clean", kache], vec![]),
        ),
        (
            "a cache folder no rule names",
            cache("cache:gone", Some(0), InUse::No),
            Level::Low,
            (vec![kache], vec![]),
        ),
        (
            "a plugin's candidate, idle",
            nm(8 * DAY_MS, |_| {}),
            Level::Normal,
            (vec!["plugin:node-modules /n remove"], vec![]),
        ),
        (
            "a plugin's candidate, written lately",
            nm(DAY_MS, |_| {}),
            Level::Normal,
            none.clone(),
        ),
        (
            "a plugin's candidate a session works in",
            nm(8 * DAY_MS, |f| f.working = true),
            Level::Normal,
            (
                vec![],
                vec!["plugin:node-modules /n: a session is working on it"],
            ),
        ),
        (
            "a plugin's candidate no rule names",
            Candidate {
                kind: "pip-cache".into(),
                ..nm(8 * DAY_MS, |_| {})
            },
            Level::Normal,
            none.clone(),
        ),
    ];
    for (name, c, lv, (acts, kept)) in cases {
        let (got_acts, got_kept) = said(&[c], &rules, lv);
        let want: Vec<String> = acts
            .iter()
            .map(|a| {
                if *a == kache {
                    "kache  run kache clean --cache --yes".to_string()
                } else {
                    a.to_string()
                }
            })
            .collect();
        assert_eq!(got_acts, want, "{name}: acts");
        assert_eq!(got_kept, kept, "{name}: kept");
    }
}

// A rule turned off, or with no numbers at a level, does nothing there;
// of two rules of one kind the first that runs decides.
#[test]
fn plan_follows_your_rules() {
    let idle = target("/t", 3 * DAY_MS, |_| {});
    let mut rules = built_in_rules();
    rules
        .iter_mut()
        .find(|r| r.id == "cargo-target")
        .unwrap()
        .enabled = false;
    assert_eq!(
        said(std::slice::from_ref(&idle), &rules, Level::Normal),
        (vec![], vec![])
    );
    let mine = Rule {
        id: "mine".into(),
        title: "targets a week idle".into(),
        kind: Kind::CargoTarget,
        enabled: true,
        normal: Some(Params {
            idle_secs: Some(7 * 24 * 3600),
            ..Params::default()
        }),
        low: Some(Params {
            idle_secs: Some(3600),
            ..Params::default()
        }),
    };
    rules.push(mine);
    assert_eq!(
        said(std::slice::from_ref(&idle), &rules, Level::Normal),
        (vec![], vec![])
    );
    assert_eq!(
        said(&[idle], &rules, Level::Low).0,
        ["mine /t remove", "kache  run kache clean --cache --yes"]
    );
}

// What a run logs: the free bytes, the level, what it did and what it
// left and why, what it freed; why it did nothing when it didn't run.
#[test]
fn disk_clean_event() {
    let quiet = event_fields(60 * GB, None, &Outcome::default(), "enough free");
    assert_eq!(
        quiet,
        json!({"free": 60 * GB, "freed": 0, "why": "enough free"})
    );
    let o = Outcome {
        did: vec![
            Done {
                rule: "cargo-target".into(),
                path: "/t".into(),
                what: "removed".into(),
                freed: 5,
            },
            Done {
                rule: "kache".into(),
                path: PathBuf::new(),
                what: "ran kache clean --cache --yes".into(),
                freed: 2,
            },
        ],
        left: vec![Kept {
            path: "/w".into(),
            rule: "worktree".into(),
            why: "locked".into(),
        }],
        freed: 7,
    };
    assert_eq!(
        event_fields(10 * GB, Some(Level::Low), &o, ""),
        json!({
            "free": 10 * GB,
            "level": "low",
            "freed": 7,
            "did": ["/t: removed (cargo-target, 5)", "ran kache clean --cache --yes (kache, 2)"],
            "left": ["/w: locked (worktree)"],
        })
    );
}

// git worktree list --porcelain: the first is the main checkout; branch,
// detached, locked, prunable and bare as git says them.
#[test]
fn worktrees_listed() {
    let out = "worktree /r\nHEAD 1111\nbranch refs/heads/main\n\nworktree /r/.claude/worktrees/a\nHEAD 2222\nbranch refs/heads/feat/a\nlocked agent at work\n\nworktree /tmp/b\nHEAD 3333\ndetached\nprunable gitdir file points to non-existent location\n";
    assert_eq!(
        parse_worktrees(out),
        [
            GitWorktree {
                path: "/r".into(),
                branch: "main".into(),
                main: true,
                ..GitWorktree::default()
            },
            GitWorktree {
                path: "/r/.claude/worktrees/a".into(),
                branch: "feat/a".into(),
                locked: true,
                ..GitWorktree::default()
            },
            GitWorktree {
                path: "/tmp/b".into(),
                prunable: true,
                ..GitWorktree::default()
            },
        ]
    );
}

// ---- worktrees, with real git ----

fn git_run(dir: &Path, args: &[&str]) {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Every file and folder under p (and p) last changed at.
fn age_tree(p: &Path, at: Ms) {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return;
    };
    if m.is_dir() {
        for e in std::fs::read_dir(p).unwrap().flatten() {
            age_tree(&e.path(), at);
        }
    }
    if !m.file_type().is_symlink() {
        set_mtime(p, at);
    }
}

// A worktree whose branch is in origin/main, clean and untouched for 4
// days goes through git, its branch kept; one with an uncommitted file
// stays, saying so; the main checkout is never one. A target in a
// worktree is that worktree's.
#[test]
fn worktrees_found_and_removed() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_run(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a"), "a").unwrap();
    std::fs::write(repo.join(".gitignore"), "target\n").unwrap();
    git_run(&repo, &["add", "a", ".gitignore"]);
    git_run(&repo, &["commit", "-q", "-m", "a"]);
    git_run(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git_run(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let done = root.join("wt/done");
    let dirty = root.join("wt/dirty");
    for (w, b) in [(&done, "done"), (&dirty, "dirty")] {
        git_run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                b,
                &w.to_string_lossy(),
                "HEAD",
            ],
        );
    }
    std::fs::write(dirty.join("new"), "work").unwrap();
    let t = target_at(&done, "target", 4 * DAY_MS, &[]);
    let old = now_ms() - 4 * DAY_MS;
    age_tree(&root, old);
    age_tree(&repo.join(".git"), old);

    let scan = Scan {
        repos: vec![dirty.clone()],
        cwds: Some(Vec::new()),
        home: std::env::var("HOME").unwrap(),
        ..Scan::default()
    };
    let rules = built_in_rules();
    let now = now_ms();
    let cands = find_candidates(&scan, &rules, Level::Normal, now);
    let kinds: Vec<(&Path, &str)> = cands
        .iter()
        .map(|c| (c.path.as_path(), c.kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (dirty.as_path(), WORKTREE),
            (done.as_path(), WORKTREE),
            (t.as_path(), CARGO_TARGET),
        ]
    );
    assert_eq!(cands[2].facts.worktree, done.to_string_lossy());
    assert_eq!(cands[1].facts.merged, "done is merged into origin/main");
    assert!(cands[1].facts.clean());
    let (acts, kept) = plan(&cands, &rules, Level::Normal, now);
    let got: Vec<(&Path, &str)> = acts
        .iter()
        .map(|a| (a.path.as_path(), a.rule.as_str()))
        .collect();
    assert_eq!(
        got,
        [(done.as_path(), "worktree"), (t.as_path(), "cargo-target")]
    );
    assert_eq!(
        kept,
        [Kept {
            path: dirty.clone(),
            rule: "worktree".into(),
            why: "1 uncommitted file".into(),
        }]
    );
    assert_eq!(acts[0].how, How::RemoveWorktree { repo: repo.clone() });
    let o = apply_all(&acts[..1], Vec::new(), &|_| InUse::No);
    assert!(o.left.is_empty(), "{o:?}");
    assert!(!done.exists());
    assert!(dirty.exists());
    git_run(&repo, &["rev-parse", "--verify", "refs/heads/done"]);
}
