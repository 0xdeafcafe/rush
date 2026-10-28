//! Ported from internal/harnessup/harnessup_test.go (parse, newer and plan
//! live with their modules), plus the cache and update runs, against
//! made-up programs in a temp folder.

use super::*;
use std::os::unix::fs::PermissionsExt;

fn program(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn states() {
    let s = |installed: &str, latest: &str| Status {
        installed: installed.into(),
        latest: latest.into(),
        ..Status::default()
    };
    for (st, want) in [
        (s("", "1.0"), State::Unknown),
        (s("1.0", ""), State::Unknown),
        (s("1.0", "1.1"), State::Available),
        (s("1.1", "1.1"), State::UpToDate),
        (s("1.2", "1.1"), State::UpToDate),
    ] {
        assert_eq!(st.state(), want, "{st:?}");
    }
}

#[test]
fn check_keeps_the_newest_for_every() {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let bin = d.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let a = program(&bin, "a", "echo 'a-cli 1.2.3 (made up)'");
    let b = program(&bin, "b", "echo no version");
    let at = SystemTime::now() - Duration::from_secs(60);
    let stale = SystemTime::now() - EVERY - Duration::from_secs(60);
    let cache = json!({
        "a": {"latest": "1.3.0", "at": rush_sys::timefmt::rfc3339_nano(at)},
        "b": {"latest": "9.9.9", "at": rush_sys::timefmt::rfc3339_nano(stale)},
        "gone": {"latest": "1.0.0", "at": "2020-01-02T03:04:05.123456789+01:00"},
    });
    std::fs::write(cache_path(&state), cache.to_string()).unwrap();
    let h = |kind: &str, path: &Path| Harness {
        kind: kind.into(),
        name: kind.to_uppercase(),
        path: path.to_path_buf(),
        published: Published::default(),
    };
    let rows = check(
        &state,
        &[
            h("a", &a),
            h("a2", &a),
            h("b", &b),
            h("none", Path::new("")),
        ],
        false,
        &|_| None,
    );
    assert_eq!(
        rows.len(),
        2,
        "one row a program, none without one: {rows:?}"
    );
    assert_eq!(
        (rows[0].installed.as_str(), rows[0].latest.as_str()),
        ("1.2.3", "1.3.0")
    );
    assert_eq!(rows[0].state(), State::Available);
    // b's kept version is stale and nothing says a newer one: it's kept as
    // it was, and b's version is unknown.
    assert_eq!(
        (rows[1].installed.as_str(), rows[1].latest.as_str()),
        ("", "9.9.9")
    );
    assert_eq!(rows[1].state(), State::Unknown);
    assert_eq!(rows[1].update, Update::default());
    // Written back as Go reads it, every kind kept.
    let back: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache_path(&state)).unwrap()).unwrap();
    assert_eq!(back["a"]["latest"], "1.3.0");
    let back_at = rush_sys::timefmt::parse(back["a"]["at"].as_str().unwrap()).unwrap();
    assert!(
        at.duration_since(back_at).unwrap() < Duration::from_millis(1),
        "kept to the millisecond"
    );
    assert_eq!(back["gone"]["at"], "2020-01-02T02:04:05.123Z");
}

#[test]
fn check_writes_a_new_cache_privately() {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("new");
    assert!(check(&state, &[], false, &|_| None).is_empty());
    assert_eq!(std::fs::read_to_string(cache_path(&state)).unwrap(), "{}");
    assert_eq!(
        std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(cache_path(&state))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn real_program_past_the_shims() {
    let d = tempfile::tempdir().unwrap();
    let shims = d.path().join("shims");
    let real = d.path().join("real");
    let other = d.path().join("other");
    for p in [&shims, &real, &other] {
        std::fs::create_dir(p).unwrap();
    }
    program(&shims, "claude", "exit 0");
    let want = program(&real, "claude", "exit 0");
    std::fs::write(other.join("claude"), "not runnable").unwrap();
    let path = std::env::join_paths([&other, &real]).unwrap();
    assert_eq!(
        real_program(&shims.join("claude"), "claude", &shims, &path),
        Some(want.clone())
    );
    assert_eq!(real_program(&want, "claude", &shims, &path), Some(want));
    let only_other = std::env::join_paths([&other]).unwrap();
    assert_eq!(
        real_program(&shims.join("claude"), "claude", &shims, &only_other),
        None
    );
    assert_eq!(real_program(Path::new(""), "claude", &shims, &path), None);
}

#[test]
fn run_updates() {
    let d = tempfile::tempdir().unwrap();
    let ok = program(d.path(), "ok", "exit 0");
    let bad = program(
        d.path(),
        "bad",
        "echo working\necho 'E404 not found' >&2\nexit 1",
    );
    let quiet = program(d.path(), "quiet", "exit 2");
    let s = |name: &str, label: &str, argv: &[&str]| Status {
        name: name.into(),
        update: Update {
            label: label.into(),
            argv: argv.iter().map(|a| a.to_string()).collect(),
        },
        ..Status::default()
    };
    let find = |n: &str| (n == "ok").then(|| ok.clone());
    assert_eq!(
        run(&s("x", "", &[]), &find).unwrap_err(),
        "rush doesn't know how x was installed: update it the way you installed it"
    );
    assert_eq!(
        run(&s("x", "brew upgrade x", &["brew", "upgrade"]), &find).unwrap_err(),
        "brew isn't on your PATH"
    );
    assert_eq!(run(&s("x", "ok", &["ok"]), &find), Ok(()));
    let bad = bad.to_string_lossy().into_owned();
    assert_eq!(
        run(&s("x", "bad up", &[&bad]), &find).unwrap_err(),
        "bad up: E404 not found"
    );
    let quiet = quiet.to_string_lossy().into_owned();
    assert_eq!(
        run(&s("x", "q", &[&quiet]), &find).unwrap_err(),
        "q: exit status 2"
    );
}
