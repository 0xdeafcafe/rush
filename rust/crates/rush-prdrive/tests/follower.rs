//! A follower's life on disk: started detached, listed only while it holds
//! its lock, stopped by taking its entry away (internal/prdrive/follow.go
//! Start, Following, Hold, Stop). The follower started is `true`, not
//! rush, so nothing follows anything.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::path::Path;

use rush_prdrive::{Entry, following, hold, stop};

#[test]
fn a_follower_is_listed_while_it_holds_its_lock() {
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().to_path_buf()));
    let mut e = Entry::new("s1", "o/r", 7);
    e.told = Some(rush_prdrive::State {
        verdict: rush_prdrive::WAITING.into(),
        ..Default::default()
    });
    assert!(rush_prdrive::follow::start_with(e.clone(), Path::new("/usr/bin/true")).unwrap());
    let saved = std::fs::read(home.path().join("prfollow").join("s1-o_r-7.json")).unwrap();
    let saved: Entry = rush_state::json::from_slice(&saved).unwrap();
    assert_eq!((saved.repo.as_str(), saved.number), ("o/r", 7));
    assert!(!saved.started.is_zero());
    // `true` ended at once, holding nothing: not following.
    assert!(following("s1").is_empty());

    // This process takes the lock, as rush pr follow-run does, and keeps
    // what the entry was told.
    let held = hold(Entry::new("s1", "o/r", 7)).expect("the lock is free");
    assert_eq!(held.told, e.told);
    assert_eq!(held.started, saved.started);
    let listed = following("s1");
    assert_eq!(listed.len(), 1);
    assert_eq!(following("").len(), 1, "every session's");
    assert!(following("s2").is_empty(), "another session's");
    // Held, another start doesn't start a second.
    assert!(!rush_prdrive::follow::start_with(e.clone(), Path::new("/usr/bin/true")).unwrap());
    assert!(hold(Entry::new("s1", "o/r", 7)).is_none(), "held twice");

    stop(&e).unwrap();
    assert!(following("s1").is_empty(), "listed after it was stopped");
    stop(&e).unwrap(); // stopping what's stopped is fine
    rush_state::set_thread_dir(None);
}
