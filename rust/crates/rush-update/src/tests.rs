//! Ported from internal/update/update_test.go:1-70, plus the check against
//! update.json that every rush shares.

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A module proxy on localhost that answers each path from answer, and
/// counts what it's asked. It serves until the test ends.
fn proxy_serving(answer: fn(&str) -> Option<String>) -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let asked = Arc::new(AtomicUsize::new(0));
    let n = asked.clone();
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
            let (status, body) = match answer(path) {
                Some(b) => ("200 OK", b),
                None => ("404 Not Found", "not found".to_string()),
            };
            let _ = write!(
                conn,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, asked)
}

fn at(s: &str) -> Ms {
    rush_core::time::parse_rfc3339(s)
}

// The rush kept before an install is the one that was there, still, once the
// new one is written the way go install writes it: removed, then new.
#[test]
fn keep_is_the_way_back() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("rush");
    std::fs::write(&exe, "old").unwrap();
    keep(&exe).unwrap();
    std::fs::remove_file(&exe).unwrap();
    std::fs::write(&exe, "new").unwrap();
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
    let (url, _) = proxy_serving(|p| {
        (p == format!("/{MODULE}/@v/v0.2.0.info"))
            .then(|| r#"{"Version":"v0.2.0","Time":"2026-09-01T00:00:00Z"}"#.to_string())
    });
    let latest = Info {
        version: "v0.0.0-20260924103914-d3b737083d4a".into(),
        time: at("2026-09-24T10:39:14Z"),
    };
    let v = |s: &str, t: &str| Info {
        version: s.into(),
        time: if t.is_empty() { 0 } else { at(t) },
    };
    for (cur, want) in [
        (latest.clone(), false),
        (
            Info {
                version: format!("{}+dirty", latest.version),
                time: latest.time,
            },
            false,
        ),
        (
            v("v0.0.0-20260920000000-aaaaaaaaaaaa", "2026-09-20T00:00:00Z"),
            true,
        ),
        (
            v("v0.0.0-20260930000000-bbbbbbbbbbbb", "2026-09-30T00:00:00Z"),
            false,
        ),
        (v("v0.2.0", ""), true),  // its time is asked for
        (v("v9.9.9", ""), false), // unknown to the proxy: no answer, no nag
    ] {
        assert_eq!(
            newer_at(&url, &cur, &latest),
            want,
            "newer({})",
            cur.version
        );
    }
    assert_eq!(
        &latest.short()[..7],
        "d3b7370",
        "short() = {}",
        latest.short()
    );
}

#[test]
fn short_names() {
    assert_eq!(
        Info {
            version: "v0.2.0+dirty".into(),
            time: 0
        }
        .short(),
        "v0.2.0"
    );
    let s = Info {
        version: "v0.0.0-20260924103914-d3b737083d4a+dirty".into(),
        time: at("2026-09-24T12:00:00Z"),
    }
    .short();
    assert!(s.starts_with("d3b7370 (Sep 2"), "{s}");
}

#[test]
fn current_from_build() {
    for v in ["", "(devel)"] {
        assert_eq!(current_from(v), None, "{v:?}");
    }
    assert_eq!(
        current_from("v0.2.0"),
        Some(Info {
            version: "v0.2.0".into(),
            time: 0
        })
    );
    assert_eq!(
        current_from("v0.0.0-20260924103914-d3b737083d4a+dirty").map(|i| i.time),
        Some(at("2026-09-24T10:39:14Z"))
    );
    assert_eq!(current_from("v0.0.0-20261324103914-d3b737083d4a"), None);
}

#[test]
fn proxy_from_goproxy() {
    for (env, want) in [
        ("", "https://proxy.golang.org"),
        ("direct", "https://proxy.golang.org"),
        ("off|https://a.example/,direct", "https://a.example"),
        ("http://b.example,https://c.example", "http://b.example"),
    ] {
        assert_eq!(proxy_from(env), want, "{env:?}");
    }
}

#[test]
fn proxy_failure_says_so() {
    let (url, _) = proxy_serving(|_| None);
    assert_eq!(
        fetch(&url, "@latest").unwrap_err().0,
        "the Go module proxy said 404 Not Found"
    );
}

#[test]
fn times_as_go_writes_them() {
    for (t, s) in [
        (0, "0001-01-01T00:00:00Z"),
        (at("2026-09-24T10:39:14Z"), "2026-09-24T10:39:14Z"),
        (at("2026-09-24T10:39:14.5Z"), "2026-09-24T10:39:14.5Z"),
        (at("1969-12-31T23:59:59.123Z"), "1969-12-31T23:59:59.123Z"),
    ] {
        assert_eq!(gotime::format(t), s);
        assert_eq!(gotime::parse(s), t);
    }
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

#[test]
fn check_asks_once_per_every() {
    let (url, asked) = proxy_serving(|p| {
        (p == format!("/{MODULE}/@latest")).then(|| {
            r#"{"Version":"v0.0.0-20260924103914-d3b737083d4a","Time":"2026-09-24T10:39:14Z"}"#
                .to_string()
        })
    });
    let dir = tempfile::tempdir().unwrap();
    let cur = current_from("v0.0.0-20260920000000-aaaaaaaaaaaa").unwrap();
    let t0 = at("2026-10-01T00:00:00Z");
    let notes = Notes::default();

    let (l, newer) = check_in(dir.path(), &url, &cur, &notes, t0);
    assert_eq!(l.version, "v0.0.0-20260924103914-d3b737083d4a");
    assert!(newer);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(*notes.done.lock().unwrap(), [None]);
    assert_eq!(
        notes.logs.lock().unwrap()[0],
        (
            "update-checked".to_string(),
            json!({"current": cur.version, "latest": l.version})
        )
    );
    // As Go writes update.json, so either rush reads the other's.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("update.json")).unwrap(),
        r#"{"checked":"2026-10-01T00:00:00Z","latest":{"Version":"v0.0.0-20260924103914-d3b737083d4a","Time":"2026-09-24T10:39:14Z"}}"#
    );

    // Within Every, the answer kept is the answer.
    let (l2, _) = check_in(dir.path(), &url, &cur, &notes, t0 + 60_000);
    assert_eq!(l2, l);
    assert_eq!(asked.load(Ordering::SeqCst), 1, "asked again within Every");

    // After it, the proxy is asked again; unless the network is down.
    let down = Notes {
        down: true,
        ..Default::default()
    };
    let later = t0 + EVERY.as_millis() as Ms + 1;
    assert_eq!(
        check_in(dir.path(), &url, &cur, &down, later),
        (Info::default(), false)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    check_in(dir.path(), &url, &cur, &notes, later);
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[test]
fn check_reads_what_go_wrote() {
    let dir = tempfile::tempdir().unwrap();
    // Go writes the time it checked where you are, to the nanosecond.
    std::fs::write(
        dir.path().join("update.json"),
        r#"{"checked":"2026-10-01T02:00:00.123456789+02:00","latest":{"Version":"v0.2.0","Time":"2026-09-01T00:00:00Z"}}"#,
    )
    .unwrap();
    let cur = current_from("v0.2.0").unwrap();
    // No proxy at all: within Every nothing is asked.
    let (l, newer) = check_in(
        dir.path(),
        "http://127.0.0.1:9",
        &cur,
        &Quiet,
        at("2026-10-01T01:00:00Z"),
    );
    assert_eq!(l.version, "v0.2.0");
    assert!(!newer);
}
