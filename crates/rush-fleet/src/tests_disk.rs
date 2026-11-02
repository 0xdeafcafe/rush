//! Port of internal/fleet/temp_test.go, left_test.go, scratch_test.go and
//! diskusage_test.go: temp work, what agents left behind, the top of
//! /tmp, and disk use counted as du does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rush_core::adapter::{Registry, TempDir};
use rush_core::agent::{Agent, Left};
use rush_core::time::{Ms, now};

use crate::*;

const MINUTE: Ms = 60_000;
const HOUR: Ms = 60 * MINUTE;

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Sets a file's or folder's modified time.
fn touch(p: &Path, at: Ms) {
    let t = UNIX_EPOCH + Duration::from_millis(at as u64);
    std::fs::File::open(p)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(t).set_accessed(t))
        .unwrap();
}

/// rush's folder, for this test's thread only.
struct RushHome(#[allow(dead_code)] tempfile::TempDir);

impl RushHome {
    fn new() -> RushHome {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().canonicalize().unwrap()));
        RushHome(d)
    }
}

impl Drop for RushHome {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
    }
}

fn row(key: &str, pid: i32) -> Agent {
    Agent {
        key: key.into(),
        pid,
        ..Agent::default()
    }
}

// A running agent's temp work is measured again after ten minutes, or
// after fifty times as long as the last walk took when that was long.
#[test]
fn temp_due_backs_off() {
    let t = now();
    let ts = TempSizes::new();
    ts.set(HashMap::from([
        (
            "quick".to_string(),
            TempSize {
                at: t - 11 * MINUTE,
                took: 100_000_000,
                ..TempSize::default()
            },
        ),
        (
            "slow".to_string(),
            TempSize {
                at: t - 11 * MINUTE,
                took: 30_000_000_000,
                ..TempSize::default()
            },
        ),
    ]));
    let agents = [row("quick", 1), row("slow", 2)];
    let due: Vec<&str> = ts.due(&agents, t).iter().map(|a| a.key.as_str()).collect();
    assert_eq!(due, ["quick"]);
    assert_eq!(
        ts.due(&agents[1..], t + 24 * MINUTE).len(),
        1,
        "the slow one is due at 25m"
    );
}

// A stopped agent isn't walked again however long it sits.
#[test]
fn stopped_temp_not_rewalked() {
    let t = now();
    let mut a = row("old", 0);
    a.job.updated_at = t - 48 * HOUR;
    let ts = TempSizes::new();
    ts.set(HashMap::from([(
        "old".to_string(),
        TempSize {
            at: t - 24 * HOUR,
            ..TempSize::default()
        },
    )]));
    assert!(ts.due(&[a], t).is_empty());
}

// The tidy-up empties a gone session's own tmp folder once it has sat
// untouched, keeps the folder, and leaves one changed recently or whose
// host still runs.
#[test]
fn clean_stale_temp_folder() {
    let _h = RushHome::new();
    let t = now();
    let mut a = Agent {
        rush: true,
        ..Agent::default()
    };
    a.job.id = "gone".into();
    let dir = PathBuf::from(temp_dir(&a.job.id));
    let (old, fresh) = (dir.join("old"), dir.join("fresh"));
    std::fs::create_dir_all(old.join("deep")).unwrap();
    std::fs::create_dir_all(&fresh).unwrap();
    let long = t - 5 * HOUR;
    touch(&old, long);
    assert!(
        !clean_stale_temp(&a, 3 * HOUR, t).unwrap(),
        "emptied one changed lately"
    );
    touch(&fresh, long);
    let mut running = a.clone();
    running.pid = 1;
    assert!(
        !clean_stale_temp(&running, 3 * HOUR, t).unwrap(),
        "emptied a running session's"
    );
    assert!(
        clean_stale_temp(&a, 3 * HOUR, t).unwrap(),
        "left a stale folder"
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

// One thing in a gone session's temp folder goes alone; nothing outside
// it does, nor anything while the session runs.
#[test]
fn remove_one_temp_entry() {
    let _h = RushHome::new();
    let reg = Registry::new();
    let mut a = Agent {
        rush: true,
        ..Agent::default()
    };
    a.job.id = "one".into();
    let dir = PathBuf::from(temp_dir(&a.job.id));
    std::fs::create_dir_all(dir.join("a").join("deep")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    let mut running = a.clone();
    running.pid = 1;
    assert!(remove_temp_entry(&reg, &running, &s(&dir.join("a"))).is_err());
    assert!(remove_temp_entry(&reg, &a, &s(&dir.join("a").join("deep"))).is_err());
    assert!(
        remove_temp_entry(&reg, &a, &s(&dir)).is_err(),
        "deleted the temp folder itself"
    );
    remove_temp_entry(&reg, &a, &s(&dir.join("a"))).unwrap();
    let left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["b"]);
}

// What a finished agent made in a temp folder can go; what was there
// before it, what's changed since, and anything nested are kept or
// folded.
#[test]
fn look_left_classes() {
    let _h = RushHome::new();
    let tmp = tempfile::tempdir_in("/tmp").unwrap(); // a system temp folder, whatever runs the test
    let before = tmp.path().join("before");
    std::fs::create_dir(&before).unwrap();
    std::fs::create_dir_all(host_root().join("s1")).unwrap(); // the session begins
    let made = tmp.path().join("clone");
    std::fs::create_dir_all(made.join("sub")).unwrap();
    let left = |p: &Path| Left {
        path: s(p),
        ..Left::default()
    };
    let mut a = Agent {
        display_name: "a".into(),
        left: vec![
            left(&made),
            left(&made.join("sub")),
            left(&before),
            left(Path::new("/nope/gone")),
        ],
        ..Agent::default()
    };
    a.job.id = "s1".into();
    a.job.updated_at = now();
    let items = look_left(std::slice::from_ref(&a));
    assert_eq!(items.len(), 3, "nested not folded: {items:#?}");
    let got: HashMap<&str, &LeftItem> = items.iter().map(|i| (i.path.as_str(), i)).collect();
    let (m, b) = (got[s(&made).as_str()], got[s(&before).as_str()]);
    assert!(
        m.safe() && !b.safe() && !b.why().is_empty() && got["/nope/gone"].gone,
        "{items:#?}"
    );
    assert!(
        remove_left(b).is_err(),
        "removed what was there before the agent"
    );
    remove_left(m).unwrap();
    assert!(!made.exists(), "didn't remove what it made");
    a.job.updated_at = now() - HOUR; // a later write is someone else's
    std::fs::create_dir(&made).unwrap();
    let it = look_left(&[a]);
    assert!(
        !(it[0].path == s(&made) && it[0].safe()),
        "touched since, yet safe"
    );
}

// Only what nothing has touched, anywhere inside, for a day goes.
#[test]
fn scratch() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    let reg = Registry::new();
    let old = now() - 2 * SCRATCH_IDLE;
    let mk = |p: &Path, at: Ms| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
        touch(p, at);
    };
    mk(&dir.join("old.log"), old);
    mk(&dir.join("cache").join("a").join("b"), old);
    touch(&dir.join("cache").join("a"), old);
    touch(&dir.join("cache"), old);
    mk(&dir.join("busy").join("old"), old);
    mk(&dir.join("busy").join("deep").join("new"), now());
    touch(&dir.join("busy"), old);
    mk(&dir.join("new.log"), now());
    let sc = find_scratch(&reg, &s(dir));
    assert!(
        sc.items == 4 && sc.stale_items == 2 && sc.stale != 0,
        "{sc:?}"
    );
    assert_eq!(stale_scratch(&reg, &s(dir)).len(), 2);
    let (_, n, err) = clear_scratch(&reg, &s(dir));
    assert!(err.is_none() && n == 2, "{n} {err:?}");
    for (name, want) in [
        ("old.log", false),
        ("cache", false),
        ("busy", true),
        ("new.log", true),
    ] {
        assert_eq!(dir.join(name).exists(), want, "{name}");
    }
}

/// A tree of folders and files, links among them.
fn disk_fixture(dir: &Path) {
    for (i, p) in [
        "a.txt",
        "b/c.bin",
        "b/d/e.txt",
        "b/d/f/g.txt",
        "h/i.txt",
        &format!("long-{}", "n".repeat(200)),
    ]
    .iter()
    .enumerate()
    {
        let p = dir.join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![0u8; (i + 1) * 9000]).unwrap();
    }
    for i in 0..300 {
        // More than one call's worth of entries.
        std::fs::write(
            dir.join("h")
                .join(format!("many-{}-{i}", "x".repeat(i % 50))),
            "x",
        )
        .unwrap();
    }
    std::fs::create_dir_all(dir.join("node_modules").join("pkg")).unwrap();
    std::fs::write(
        dir.join("node_modules").join("pkg").join("index.js"),
        vec![0u8; 20000],
    )
    .unwrap();
    std::os::unix::fs::symlink(dir.join("b"), dir.join("link")).unwrap();
    std::os::unix::fs::symlink("/", dir.join("root")).unwrap();
}

// However the platform walks a folder, it counts what a stat walk does:
// every file's and folder's blocks, links not followed.
#[test]
fn dir_usage_matches_stat() {
    let d = tempfile::tempdir().unwrap();
    disk_fixture(d.path());
    let td = |p: PathBuf| TempDir {
        path: s(&p),
        keep: false,
    };
    let dirs = [
        td(d.path().to_path_buf()),
        td(d.path().join("missing")),
        td(d.path().join("link")),
    ];
    assert_eq!(disk_usage_background(&dirs), disk_usage(&dirs));
    let want = crate::temp::stat_usage_for_test(&s(d.path()));
    let top = std::fs::symlink_metadata(d.path()).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert!(want > 0);
    assert_eq!(disk_usage(&dirs[..1]), want + top.blocks() as i64 * 512);
}

// A heavy folder's total is reused while it's unchanged and counted
// afresh once its own listing changes.
#[test]
fn heavy_folder_reused() {
    let d = tempfile::tempdir().unwrap();
    disk_fixture(d.path());
    let dirs = [TempDir {
        path: s(d.path()),
        keep: false,
    }];
    let first = disk_usage_background(&dirs);
    assert_eq!(first, disk_usage(&dirs));
    std::fs::write(
        d.path().join("node_modules").join("new.js"),
        vec![0u8; 40000],
    )
    .unwrap();
    assert!(
        disk_usage_background(&dirs) > first,
        "a changed heavy folder was not counted afresh"
    );
}

#[allow(dead_code)]
fn unused(_: SystemTime) {}
