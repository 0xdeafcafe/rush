//! How many threads a host adds while it waits between turns (DESIGN.md,
//! "The host"). A count is the whole process's, so this file holds one
//! test, and the process is its own.

use std::time::{Duration, Instant};

use rush_host::fake;
use rush_host::wire::Config;

/// The threads an idle host runs: the one serving it (the process's main
/// in `rush host run`), the events log's writer, its timers and its
/// agents' queue.
const IDLE_HOST: usize = 7;

/// This process's thread count once it has fallen to at most n, or after
/// a while: what a start leaves for a moment (a client's reader, a look at
/// the workspace) ends soon after. It never sleeps, only yields.
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
fn an_idle_host_runs_few_threads() {
    rush_testenv::assert_sandboxed();
    let home = tempfile::Builder::new()
        .prefix("rt-")
        .tempdir_in("/tmp")
        .unwrap();
    let d = std::fs::canonicalize(home.path()).unwrap();
    rush_state::set_thread_dir(Some(d.clone()));
    rush_state::set_thread_cache_dir(Some(d.join("cache")));
    let acct = d.join("acct");
    std::fs::create_dir_all(&acct).unwrap();
    let mut cfg = Config {
        id: "threads1".into(),
        session_id: "threads1-conversation".into(),
        resume: true,
        kind: "fake".into(),
        cwd: d.to_string_lossy().into_owned(),
        ..Config::default()
    };
    cfg.account.kind = "fake".into();
    cfg.account.dir = acct.to_string_lossy().into_owned();

    let before = rush_sys::threads::count();
    assert!(before > 0, "no thread count");
    let c = fake::spawn(fake::registry(), cfg).unwrap();
    let added = settled(before + IDLE_HOST).saturating_sub(before);
    assert_eq!(added, IDLE_HOST, "an idle host's threads");
    rush_host::Client::dial(&c.id).unwrap().stop().unwrap();
}
