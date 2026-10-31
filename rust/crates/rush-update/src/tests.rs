//! After internal/update/update_test.go:1-70, for releases on GitHub: a
//! fake GitHub on localhost, temp dirs for rush's folder and the rush
//! that's replaced, and a fake brew. Nothing here reaches the network, the
//! real home or the real brew.

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

type Answer = dyn Fn(&str, &str) -> Option<Vec<u8>> + Send + Sync;

/// A GitHub on localhost that answers each path from answer(path, its own
/// URL), and counts what it's asked. It serves until the test ends.
fn serving(answer: Box<Answer>) -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let asked = Arc::new(AtomicUsize::new(0));
    let n = asked.clone();
    let base = url.clone();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut conn) = conn else { continue };
            n.fetch_add(1, Ordering::SeqCst);
            let mut r = BufReader::new(conn.try_clone().unwrap());
            let mut first = String::new();
            let _ = r.read_line(&mut first);
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("");
            let (status, body) = match answer(path, &base) {
                Some(b) => ("200 OK", b),
                None => ("404 Not Found", b"not found".to_vec()),
            };
            let _ = write!(
                conn,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = conn.write_all(&body);
        }
    });
    (url, asked)
}

const LATEST: &str = "/repos/0xdeafcafe/rush/releases/latest";

/// GitHub's answer for a release tagged tag, with files names served at
/// /dl/<name>.
fn release_json(base: &str, tag: &str, names: &[String]) -> Vec<u8> {
    let assets: Vec<Value> = names
        .iter()
        .map(|n| json!({"name": n, "browser_download_url": format!("{base}/dl/{n}"), "size": 1}))
        .collect();
    serde_json::to_vec(
        &json!({"tag_name": tag, "name": tag, "prerelease": false, "assets": assets}),
    )
    .unwrap()
}

/// A tar.gz of files as the release workflow's tar makes it (ustar).
fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, body) in files {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000755\0");
        h[108..116].copy_from_slice(b"0000000\0");
        h[116..124].copy_from_slice(b"0000000\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        h[136..148].copy_from_slice(b"00000000000\0");
        h[148..156].copy_from_slice(b"        ");
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u32 = h.iter().map(|&b| b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        tar.extend_from_slice(&h);
        tar.extend_from_slice(body);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.resize(tar.len() + 1024, 0);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

// The rush kept before an install is the one that was there, still, once the
// new one is renamed over it.
#[test]
fn keep_is_the_way_back() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("rush");
    std::fs::write(&exe, "old").unwrap();
    keep(&exe).unwrap();
    let new = dir.path().join("new");
    std::fs::write(&new, "new").unwrap();
    std::fs::rename(&new, &exe).unwrap();
    assert_eq!(std::fs::read_to_string(prev(&exe)).unwrap(), "old");
    keep(&exe).unwrap(); // and again, over the last one kept
    assert_eq!(
        std::fs::read_to_string(prev(&exe)).unwrap(),
        "new",
        "kept the second time"
    );
}

#[test]
fn newer_than() {
    for (cur, latest, want) in [
        ("0.2.0", "0.2.0", false),
        ("v0.2.0", "0.2.0", false),
        ("0.1.0", "0.2.0", true),
        ("0.2.0", "0.1.9", false),
        ("0.9.0", "0.10.0", true),
        ("1.0.0", "0.10.0", false),
        ("0.2.0-nightly.20261029", "0.2.0", true),
        ("0.2.0-nightly.20261029", "0.1.0", false),
        ("0.2.0", "0.3.0-nightly.20261101", true),
        ("0.2.0-nightly.20261029", "0.2.0-nightly.20261030", true),
        ("0.2.0-nightly.9", "0.2.0-nightly.10", true),
        ("0.2.0-alpha.1", "0.2.0-alpha", false),
        ("0.2.0+abc", "0.2.0", false),
        // What isn't semver is never newer: no answer, no nag.
        ("0.2.0", "nightly", false),
        ("0.2", "0.3", false),
        ("0.2.0", "0.3.0.1", false),
        ("0.2.0", "0.3.0-", false),
        ("garbage", "0.3.0", false),
        ("0.2.0", "0.3.é", false),
    ] {
        assert_eq!(newer(cur, latest), want, "newer({cur}, {latest})");
    }
}

#[test]
fn current_from_build() {
    for (v, want) in [
        ("", None),
        ("  ", None),
        ("v", None),
        ("0.2.0", Some("0.2.0")),
        ("v0.2.0", Some("0.2.0")),
        ("0.2.0-nightly.20261029", Some("0.2.0-nightly.20261029")),
    ] {
        assert_eq!(current_from(v).as_deref(), want, "{v:?}");
    }
}

#[test]
fn latest_reads_githubs_answer() {
    let (url, _) = serving(Box::new(|p, base| {
        (p == LATEST).then(|| release_json(base, "v0.3.0", &["a.tar.gz".into()]))
    }));
    let l = fetch(&url).unwrap();
    assert_eq!(l.version, "0.3.0");
    assert_eq!(l.short(), "v0.3.0");
    assert_eq!(
        l.assets,
        [Asset {
            name: "a.tar.gz".into(),
            url: format!("{url}/dl/a.tar.gz"),
        }]
    );
}

#[test]
fn github_failures_say_so() {
    let (url, _) = serving(Box::new(|_, _| None));
    assert_eq!(
        fetch(&url).unwrap_err().0,
        "GitHub has no release of rush yet"
    );

    let (url, _) = serving(Box::new(|_, _| Some(b"{\"tag_name\":\"\"}".to_vec())));
    assert_eq!(
        fetch(&url).unwrap_err().0,
        "GitHub's newest release of rush has no tag"
    );
}

// An answer far bigger than a release isn't read whole.
#[test]
fn big_answer_is_refused() {
    let (url, _) = serving(Box::new(|_, _| {
        Some(format!("\"{}\"", "a".repeat(2 << 20)).into_bytes())
    }));
    assert!(fetch(&url).is_err());
}

/// A host that notes what it's told.
#[derive(Default)]
struct Notes {
    down: bool,
    logs: Mutex<Vec<(String, Value)>>,
    done: Mutex<Vec<Option<String>>>,
}

impl Host for Notes {
    fn log(&self, kind: &str, fields: Value) {
        self.logs.lock().unwrap().push((kind.to_string(), fields));
    }
    fn net_run(&self, _job: &str) -> bool {
        !self.down
    }
    fn net_done(&self, _job: &str, err: Option<&str>) {
        self.done.lock().unwrap().push(err.map(str::to_string));
    }
}

fn at(s: &str) -> Ms {
    rush_core::time::parse_rfc3339(s)
}

#[test]
fn check_asks_once_per_every() {
    let (url, asked) = serving(Box::new(|p, base| {
        (p == LATEST).then(|| release_json(base, "v0.3.0", &[]))
    }));
    let dir = tempfile::tempdir().unwrap();
    // Go's rush keeps its own answer in the same folder; it's left alone.
    let go = r#"{"checked":"2026-10-01T02:00:00+02:00","latest":{"Version":"v0.0.0-20260924103914-d3b737083d4a","Time":"2026-09-24T10:39:14Z"}}"#;
    std::fs::write(dir.path().join("update.json"), go).unwrap();
    let t0 = at("2026-10-01T00:00:00Z");
    let notes = Notes::default();

    let (l, newer) = check_in(dir.path(), &url, "0.2.0", &notes, t0);
    assert_eq!(l.version, "0.3.0");
    assert!(newer);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(*notes.done.lock().unwrap(), [None]);
    assert_eq!(
        notes.logs.lock().unwrap()[0],
        (
            "update-checked".to_string(),
            json!({"current": "0.2.0", "latest": "0.3.0"})
        )
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("update.json")).unwrap(),
        go
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(CACHE)).unwrap(),
        format!(r#"{{"checked":{t0},"latest":{{"version":"0.3.0","assets":[]}}}}"#)
    );

    // Within Every, the answer kept is the answer.
    let (l2, newer2) = check_in(dir.path(), &url, "0.3.0", &notes, t0 + 60_000);
    assert_eq!(l2, l);
    assert!(!newer2);
    assert_eq!(asked.load(Ordering::SeqCst), 1, "asked again within Every");

    // After it, GitHub is asked again; unless the network is down.
    let down = Notes {
        down: true,
        ..Default::default()
    };
    let later = t0 + EVERY.as_millis() as Ms + 1;
    assert_eq!(
        check_in(dir.path(), &url, "0.2.0", &down, later),
        (Release::default(), false)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    check_in(dir.path(), &url, "0.2.0", &notes, later);
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[test]
fn check_failure_is_logged_not_kept() {
    let (url, _) = serving(Box::new(|_, _| None));
    let dir = tempfile::tempdir().unwrap();
    let notes = Notes::default();
    assert_eq!(
        check_in(dir.path(), &url, "0.2.0", &notes, 1),
        (Release::default(), false)
    );
    assert_eq!(
        *notes.done.lock().unwrap(),
        [Some("GitHub has no release of rush yet".to_string())]
    );
    assert!(!dir.path().join(CACHE).exists());
}

#[test]
fn homebrew_is_found_by_its_cellar() {
    for (exe, want) in [
        (
            "/opt/homebrew/Cellar/rush/0.2.0/bin/rush",
            Some("/opt/homebrew/bin/brew"),
        ),
        (
            "/usr/local/Cellar/rush/0.2.0_1/bin/rush",
            Some("/usr/local/bin/brew"),
        ),
        (
            "/home/linuxbrew/.linuxbrew/Cellar/rush/0.2.0/bin/rush",
            Some("/home/linuxbrew/.linuxbrew/bin/brew"),
        ),
        ("/opt/homebrew/bin/rush", None),
        ("/Users/a/.cargo/bin/rush", None),
        ("/opt/homebrew/Cellar/other/1.0/bin/rush", None),
    ] {
        assert_eq!(homebrew(Path::new(exe)), want.map(PathBuf::from), "{exe}");
    }
}

/// A brew in prefix/bin that notes how it was run in prefix/ran, and
/// exits with code.
fn fake_brew(prefix: &Path, code: i32) {
    let bin = prefix.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let brew = bin.join("brew");
    std::fs::write(
        &brew,
        format!(
            "#!/bin/sh\necho \"$@ secs=$HOMEBREW_AUTO_UPDATE_SECS\" > '{}'\necho 'Error: no bottle' >&2\nexit {code}\n",
            prefix.join("ran").display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn install_under_homebrew_runs_brew_upgrade() {
    let (url, _) = serving(Box::new(|p, base| {
        (p == LATEST).then(|| release_json(base, "v0.3.0", &[]))
    }));
    let prefix = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let exe = prefix.path().join("Cellar/rush/0.2.0/bin/rush");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "old").unwrap();

    // No brew where it should be.
    assert_eq!(
        install_at(&url, "0.2.0", &exe, state.path(), &Quiet)
            .unwrap_err()
            .0,
        format!(
            "rush was installed with Homebrew, but there's no brew at {}",
            prefix.path().join("bin/brew").display()
        )
    );

    fake_brew(prefix.path(), 0);
    let l = install_at(&url, "0.2.0", &exe, state.path(), &Quiet).unwrap();
    assert_eq!(l.version, "0.3.0");
    assert_eq!(
        std::fs::read_to_string(prefix.path().join("ran")).unwrap(),
        "upgrade 0xdeafcafe/tap/rush secs=0\n"
    );
    // brew put it in place: the rush in the Cellar isn't touched here.
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
    assert!(!prev(&exe).exists());
    assert!(state.path().join(CACHE).exists());

    fake_brew(prefix.path(), 1);
    assert_eq!(
        install_at(&url, "0.2.0", &exe, state.path(), &Quiet)
            .unwrap_err()
            .0,
        "brew upgrade: Error: no bottle"
    );
}

/// A GitHub serving release v0.3.0 with tgz as this machine's tar.gz and
/// sum as its .sha256.
fn serving_release(tgz: Vec<u8>, sum: String) -> String {
    let name = tarball("0.3.0", target().unwrap_or("none"));
    let sum_name = format!("{name}.sha256");
    let (url, _) = serving(Box::new(move |p, base| {
        if p == LATEST {
            Some(release_json(
                base,
                "v0.3.0",
                &[name.clone(), sum_name.clone()],
            ))
        } else if p == format!("/dl/{name}") {
            Some(tgz.clone())
        } else if p == format!("/dl/{sum_name}") {
            Some(sum.clone().into_bytes())
        } else {
            None
        }
    }));
    url
}

/// The files in dir, by name.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[cfg(unix)]
#[test]
fn install_replaces_the_running_rush() {
    let tgz = tar_gz(&[
        ("rush", b"new rush"),
        ("LICENSE", b"MIT"),
        ("README.md", b"# rush"),
    ]);
    let name = tarball("0.3.0", target().unwrap_or("none"));
    let sum = format!("{}  {name}\n", hex(&Sha256::digest(&tgz)));
    let url = serving_release(tgz, sum);
    let bin = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let exe = bin.path().join("rush");
    std::fs::write(&exe, "old rush").unwrap();
    let notes = Notes::default();

    let res = install_at(&url, "0.2.0", &exe, state.path(), &notes);
    if target().is_none() {
        assert_eq!(
            res.unwrap_err().0,
            "there's no release of rush built for this machine"
        );
        return;
    }
    assert_eq!(res.unwrap().version, "0.3.0");
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new rush");
    assert_eq!(std::fs::read_to_string(prev(&exe)).unwrap(), "old rush");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    assert_eq!(
        names(bin.path()),
        ["rush", "rush.prev"],
        "nothing left over"
    );
    assert!(notes.logs.lock().unwrap().is_empty());
    let c: Cache =
        serde_json::from_slice(&std::fs::read(state.path().join(CACHE)).unwrap()).unwrap();
    assert_eq!(c.latest.version, "0.3.0");
}

#[cfg(unix)]
#[test]
fn install_refuses_what_wasnt_released() {
    let Some(target) = target() else { return };
    let name = tarball("0.3.0", target);
    let good = tar_gz(&[("rush", b"new rush")]);
    let sum = format!("{}  {name}\n", hex(&Sha256::digest(&good)));
    let cases: Vec<(Vec<u8>, String, String)> = vec![
        // Not what its checksum says.
        (
            tar_gz(&[("rush", b"evil")]),
            sum.clone(),
            format!("{name} isn't what was released: its sha256 is"),
        ),
        // No rush in it.
        {
            let t = tar_gz(&[("README.md", b"# rush")]);
            let s = hex(&Sha256::digest(&t));
            (t, s, format!("{name}: no rush in it"))
        },
        // Not a tar.gz at all.
        {
            let t = b"<html>".to_vec();
            let s = hex(&Sha256::digest(&t));
            (t, s, format!("{name}: not a tar.gz"))
        },
    ];
    for (tgz, sum, want) in cases {
        let url = serving_release(tgz, sum);
        let bin = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let exe = bin.path().join("rush");
        std::fs::write(&exe, "old rush").unwrap();
        let err = install_at(&url, "0.2.0", &exe, state.path(), &Quiet)
            .unwrap_err()
            .0;
        assert!(err.starts_with(&want), "{err}");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old rush");
        assert_eq!(names(bin.path()), ["rush"], "nothing left over");
        assert!(!state.path().join(CACHE).exists());
    }

    // A release without this machine's files says which is missing.
    let (url, _) = serving(Box::new(|p, base| {
        (p == LATEST).then(|| release_json(base, "v0.3.0", &[]))
    }));
    let bin = tempfile::tempdir().unwrap();
    let exe = bin.path().join("rush");
    std::fs::write(&exe, "old rush").unwrap();
    assert_eq!(
        install_at(&url, "0.2.0", &exe, bin.path(), &Quiet)
            .unwrap_err()
            .0,
        format!("release v0.3.0 has no {name}")
    );

    // A rush not called rush is left alone.
    let other = bin.path().join("rush-dev");
    std::fs::write(&other, "dev").unwrap();
    let url = serving_release(good, sum);
    assert!(
        install_at(&url, "0.2.0", &other, bin.path(), &Quiet)
            .unwrap_err()
            .0
            .ends_with("isn't called rush, so it's left alone")
    );
}

/// A release that isn't newer than the rush running is never installed:
/// the same one again, an older one, or the release a newer nightly is
/// ahead of; a nightly's own release is newer than it.
#[cfg(unix)]
#[test]
fn install_refuses_what_isnt_newer() {
    for cur in ["0.3.0", "0.4.0", "0.3.1-nightly.20261030"] {
        let url = serving_release(tar_gz(&[("rush", b"new rush")]), String::new());
        let bin = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let exe = bin.path().join("rush");
        std::fs::write(&exe, "old rush").unwrap();
        assert_eq!(
            install_at(&url, cur, &exe, state.path(), &Quiet)
                .unwrap_err()
                .0,
            format!("rush v{cur} is the newest: v0.3.0 isn't newer"),
        );
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old rush");
        assert_eq!(names(bin.path()), ["rush"], "nothing left over");
        assert!(!state.path().join(CACHE).exists());
    }
    // Under Homebrew too: brew isn't run.
    let (url, _) = serving(Box::new(|p, base| {
        (p == LATEST).then(|| release_json(base, "v0.3.0", &[]))
    }));
    let prefix = tempfile::tempdir().unwrap();
    let exe = prefix.path().join("Cellar/rush/0.3.0/bin/rush");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "old").unwrap();
    fake_brew(prefix.path(), 0);
    assert!(install_at(&url, "0.3.0", &exe, prefix.path(), &Quiet).is_err());
    assert!(!prefix.path().join("ran").exists(), "brew ran");
    // The nightly before a release goes on to it.
    assert!(newer("0.3.0-nightly.20261029", "0.3.0"));
}

// What tar itself makes reads too: a PAX header (here for a long name)
// before a file is stepped over.
#[cfg(unix)]
#[test]
fn archive_reads_what_tar_makes() {
    let Some(tar) = ["/usr/bin/tar", "/bin/tar"]
        .into_iter()
        .find(|p| Path::new(p).exists())
    else {
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    std::fs::create_dir(&src).unwrap();
    std::fs::write(src.join("rush"), "the rush").unwrap();
    let long = format!("{}.md", "n".repeat(150));
    std::fs::write(src.join(&long), "long").unwrap();
    let out = d.path().join("a.tar.gz");
    let ok = std::process::Command::new(tar)
        .args(["--format", "pax", "-czf"])
        .arg(&out)
        .arg("-C")
        .arg(&src)
        .args([long.as_str(), "./rush"])
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let b = std::fs::read(&out).unwrap();
    assert_eq!(archive::find(&b, "rush", 1 << 20).unwrap(), b"the rush");
}

#[test]
fn archive_is_bounded() {
    let tgz = tar_gz(&[("rush", &[7u8; 4096])]);
    assert!(archive::find(&tgz, "rush", 1024).is_err());
    assert_eq!(
        archive::find(&tgz, "./rush", 1 << 20).unwrap_err(),
        "no ./rush in it"
    );
}
