//! Connections share one reader: twenty idle pairs cost the process at
//! most that reader's thread, where each connection had a thread of its
//! own (forty). One test, so nothing else in this process starts threads
//! while they're counted.

use std::os::unix::net::UnixStream;
use std::sync::Arc;

use rush_plugin::Ctx;
use rush_plugin::rpc::{Conn, Handler};

#[test]
fn idle_connections_cost_no_threads() {
    // Every test runs in a throwaway home (rush-testenv); its own test
    // saying so would run beside this one and start a thread.
    rush_testenv::assert_sandboxed();
    const N: usize = 20;
    let before = rush_sys::threads::count();
    assert!(before > 0, "threads can't be counted");
    let echo: Handler = Arc::new(|_, _, _, p| Ok(p.map(|p| p.to_owned())));
    let pairs: Vec<(Conn, Conn)> = (0..N)
        .map(|_| {
            let (x, y) = UnixStream::pair().unwrap();
            (
                Conn::new(x, Some(echo.clone())).unwrap(),
                Conn::new(y, Some(echo.clone())).unwrap(),
            )
        })
        .collect();
    let idle = rush_sys::threads::count();
    assert!(
        idle <= before + 2,
        "{before} threads, then {idle} with {N} pairs idle (a reader each: {})",
        before + 2 * N
    );
    // Each still works both ways.
    for (i, (a, b)) in pairs.iter().enumerate() {
        let got: usize = a.call(&Ctx::background(), "echo", &i).unwrap();
        assert_eq!(got, i);
        let got: usize = b.call(&Ctx::background(), "echo", &(i + N)).unwrap();
        assert_eq!(got, i + N);
    }
    for (a, b) in &pairs {
        a.close();
        assert_eq!(b.err(), "EOF");
        a.wait();
    }
}
