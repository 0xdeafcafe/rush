//! This machine's own Codex threads, read but never changed: their list,
//! and the largest rollouts read back whole and followed a slice at a
//! time, which must agree. RUSH_CODEX_REAL=1 runs it (as Go's
//! TestWhoIsHere reads the real sign-in).

use std::time::Instant;

use rush_codex::Codex;
use rush_core::adapter::{Adapter, SLICE, Session, read_history};
use rush_core::event::marshal;

#[test]
fn real_rollouts_read_whole_and_followed_agree() {
    if std::env::var("RUSH_CODEX_REAL").as_deref() != Ok("1") {
        eprintln!("RUSH_CODEX_REAL=1 reads this machine's ~/.codex");
        return;
    }
    let home = std::env::var("HOME").unwrap();
    let c = Codex::with_env("", &home, || true);
    let Some(p) = c.profiles().into_iter().next() else {
        eprintln!("no ~/.codex here");
        return;
    };
    let t = Instant::now();
    let past = c.discoverer().unwrap().past(&p);
    eprintln!("{} past threads in {:?}", past.len(), t.elapsed());
    let t = Instant::now();
    let again = c.discoverer().unwrap().past(&p);
    eprintln!("again, from the heads kept, in {:?}", t.elapsed());
    assert_eq!(past.len(), again.len());

    let mut biggest: Vec<(u64, &Session)> = past
        .iter()
        .map(|s| (std::fs::metadata(&s.transcript).map_or(0, |m| m.len()), s))
        .collect();
    biggest.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    let reader = c.history_reader().unwrap();
    for (n, s) in biggest.iter().take(5) {
        let t = Instant::now();
        let whole = read_history(reader, s, 0).unwrap();
        let took = t.elapsed();
        let mut followed = Vec::new();
        let mut f = c.history_follower().unwrap().follow_history(s).unwrap();
        loop {
            let sl = f.next(SLICE).unwrap();
            followed.extend(sl.events);
            if !sl.more {
                break;
            }
        }
        // A thread still being written may have grown between the two.
        let k = whole.len().min(followed.len());
        let line = |e| marshal(e).unwrap_or_default();
        for i in 0..k {
            assert_eq!(line(&whole[i]), line(&followed[i]), "{} event {i}", s.id);
        }
        eprintln!(
            "{} MB, {} events in {:?}: {}",
            n >> 20,
            whole.len(),
            took,
            s.name
        );
    }
}
