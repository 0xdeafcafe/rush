//! How many threads a running Claude Code session holds while it waits
//! for its next message (DESIGN.md, "The host"). A count is the whole
//! process's, so this file holds one test, and the process is its own.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use rush_claude::headless::{Options, start};

/// Its output's reader: stdin is written on a thread only while there's
/// something to write, stderr is read in the reader's wait, and the
/// process group's guard has no thread.
const WAITING_SESSION: usize = 1;

/// This process's thread count once it has fallen to at most n, or after
/// a while: a writer ends soon after what it wrote. It never sleeps, only
/// yields.
fn settled(n: usize) -> usize {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let c = rush_sys::threads::count();
        if c <= n || Instant::now() >= end {
            return c;
        }
        std::thread::yield_now();
    }
}

#[test]
fn a_waiting_session_holds_one_thread() {
    rush_testenv::assert_sandboxed();
    let dir = tempfile::tempdir().unwrap();
    // A claude that answers each line on stdin with one on stdout, and
    // says something on stderr first.
    let bin = dir.path().join("claude");
    std::fs::write(
        &bin,
        "#!/bin/sh\necho starting >&2\nwhile read -r l; do echo '{\"type\":\"heard\"}'; done\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (heard_tx, heard) = mpsc::channel::<Vec<u8>>();
    let heard_tx = std::sync::Mutex::new(heard_tx);
    let o = Options {
        dir: dir.path().display().to_string(),
        binary: bin.display().to_string(),
        tap: Some(Arc::new(move |l: &[u8]| {
            let _ = heard_tx.lock().unwrap().send(l.to_vec());
        })),
        ..Options::default()
    };

    let before = rush_sys::threads::count();
    assert!(before > 0, "no thread count");
    let (s, r) = start(&o).unwrap();
    let (end_tx, ended) = mpsc::channel();
    r.spawn(
        s.clone(),
        |_| {},
        move |e| {
            let _ = end_tx.send(e);
        },
    );
    s.send("hello").unwrap();
    let wait = Duration::from_secs(10);
    assert_eq!(heard.recv_timeout(wait).unwrap(), br#"{"type":"heard"}"#);
    let added = settled(before + WAITING_SESSION).saturating_sub(before);
    assert_eq!(
        added,
        WAITING_SESSION,
        "a waiting session's threads: {:?}",
        rush_sys::threads::names()
    );

    // Written to again, it answers again; stopped, it leaves none.
    s.send("again").unwrap();
    assert_eq!(heard.recv_timeout(wait).unwrap(), br#"{"type":"heard"}"#);
    s.stop(Duration::from_secs(5)).unwrap();
    ended.recv_timeout(wait).unwrap();
    assert_eq!(
        settled(before).saturating_sub(before),
        0,
        "a stopped session's threads: {:?}",
        rush_sys::threads::names()
    );
}
