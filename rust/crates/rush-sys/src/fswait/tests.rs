//! Ported from internal/fswait/fswait_test.go and watch_test.go, without
//! the sleeps: a write before the wait starts is caught by its first check.

use super::*;
use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc;

fn append(p: &std::path::Path, s: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
    f.write_all(s.as_bytes()).unwrap();
}

#[test]
fn grown() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("t.jsonl");
    std::fs::write(&p, "a\n").unwrap();
    let stop = Arc::new(Stop::new().unwrap());
    let (tx, rx) = mpsc::channel();
    let file = |size| {
        vec![File {
            path: p.clone(),
            size,
        }]
    };
    {
        let (stop, files, tx) = (stop.clone(), file(2), tx.clone());
        std::thread::spawn(move || tx.send(super::grown(&stop, &files)).unwrap());
    }
    append(&p, "b\n");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)),
        Ok(true),
        "a write wasn't noticed"
    );

    // Already grown before the watch: seen at once.
    assert!(
        super::grown(&stop, &file(2)),
        "missed growth before watching"
    );

    // Stop ends it.
    {
        let (stop, files, tx) = (stop.clone(), file(4), tx.clone());
        std::thread::spawn(move || tx.send(super::grown(&stop, &files)).unwrap());
    }
    stop.close();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)),
        Ok(false),
        "stop wasn't noticed"
    );
    // Closed, it stays closed.
    assert!(!super::grown(&stop, &file(4)));
}

#[cfg(target_os = "macos")]
#[test]
fn grown_replaced_and_missing() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("t.jsonl");
    std::fs::write(&p, "ab").unwrap();
    let stop = Arc::new(Stop::new().unwrap());
    let (tx, rx) = mpsc::channel();
    {
        let (stop, p) = (stop.clone(), p.clone());
        std::thread::spawn(move || {
            tx.send(super::grown(&stop, &[File { path: p, size: 2 }]))
                .unwrap()
        });
    }
    // Replaced by one the same size: told, so whoever reads starts over.
    // (Replaced before the watch, it would watch the new one: wait for it.)
    darwin::watched::wait(&p);
    let tmp = d.path().join("t.tmp");
    std::fs::write(&tmp, "cd").unwrap();
    std::fs::rename(&tmp, &p).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(true));
    // Not there: falls back to a stat loop, which stop still ends.
    stop.close();
    assert!(!super::grown(
        &stop,
        &[File {
            path: d.path().join("none"),
            size: 0
        }]
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn watcher() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    let f = dir.join("t.jsonl");
    std::fs::write(&f, "a\n").unwrap();
    let w = Watcher::new();
    w.watch(&[&dir, &f]);
    assert!(w.changed(), "the first ask should say to look");
    assert!(
        !w.changed(),
        "nothing happened, but it said something changed"
    );
    append(&f, "b\n");
    assert!(w.changed(), "an append wasn't noticed");
    assert!(!w.changed(), "one append counted twice");
    // A file replaced by rename, as info.json is: the folder says so.
    let tmp = dir.join("info.json.tmp");
    std::fs::write(&tmp, "{}").unwrap();
    std::fs::rename(&tmp, dir.join("info.json")).unwrap();
    assert!(w.changed(), "a rename into the folder wasn't noticed");
    // A watched file removed and written again is watched afresh.
    std::fs::remove_file(&f).unwrap();
    assert!(w.changed(), "removal wasn't noticed");
    std::fs::write(&f, "c\n").unwrap();
    w.watch(&[&dir, &f]);
    w.changed();
    append(&f, "d\n");
    assert!(w.changed(), "the new file's append wasn't noticed");
    w.close();
    assert!(w.changed(), "a closed watcher can't tell, so says to look");
}

// changes names what changed: each of many files written, and only those.
#[cfg(target_os = "macos")]
#[test]
fn watcher_changes() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    let files: Vec<PathBuf> = (0..40) // more than one drain's 32 events
        .map(|i| {
            let f = dir.join(format!("{i}.jsonl"));
            std::fs::write(&f, "a\n").unwrap();
            f
        })
        .collect();
    let quiet = dir.join("quiet");
    std::fs::write(&quiet, "").unwrap();
    let w = Watcher::new();
    let mut all_paths = vec![quiet.clone()];
    all_paths.extend(files.iter().cloned());
    w.watch(&all_paths);
    let (got, all) = w.changes();
    assert!(
        all && got.contains(&quiet),
        "first ask: {} all {all}",
        got.len()
    );
    for f in &files {
        append(f, "b\n");
    }
    let (got, all) = w.changes();
    assert!(
        !all && got.len() == files.len() && !got.contains(&quiet),
        "after 40 appends: {} paths, all {all}",
        got.len()
    );
    // Removed: told, and no longer watched.
    std::fs::remove_file(&files[0]).unwrap();
    let (got, _) = w.changes();
    assert!(got.contains(&files[0]) && !w.watching(&files[0]));
    // Watched again, it's told once as changed: it may have been written
    // between being read and being watched.
    std::fs::write(&files[0], "c\n").unwrap();
    w.watch(&all_paths);
    let (got, _) = w.changes();
    assert_eq!(got, HashSet::from([files[0].clone()]));
    assert!(w.watching(&quiet) && !w.watching(dir.join("missing")));
}

// A path that isn't there yet waits on the folder above it, and is watched
// once it appears there, however deep: nothing is tried on a timer.
#[cfg(target_os = "macos")]
#[test]
fn watcher_follows_missing_paths_in() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("jobs").join("state.json");
    let w = Watcher::new();
    w.watch(&[&p]);
    assert!(!w.watching(&p));
    assert!(w.inner().missing.contains(&p));
    w.changes();
    // Its folder appears: a wait sleeps through that, as the file isn't
    // there yet, until it is.
    std::fs::create_dir(d.path().join("jobs")).unwrap();
    std::fs::write(&p, "{}").unwrap();
    let woke = w.wait(None, Some(Duration::from_secs(30)));
    assert!(woke);
    // jobs/ was made first: the file may arrive on a second wake.
    if !w.watching(&p) {
        assert!(w.wait(None, Some(Duration::from_secs(30))));
    }
    assert!(w.watching(&p), "not watched once it appeared");
    assert!(w.changes().0.contains(&p), "its appearing wasn't told");
    assert!(w.inner().missing.is_empty());
    // A change to it wakes a wait as any other's does.
    append(&p, "x");
    assert!(w.wait(None, Some(Duration::from_secs(30))));
    assert!(w.changes().0.contains(&p));
}

// A folder's change that brings nothing asked for is slept through: the
// wait runs to its timeout.
#[cfg(target_os = "macos")]
#[test]
fn watcher_sleeps_through_a_stand_ins_noise() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("missing");
    let w = Watcher::new();
    w.watch(&[&p]);
    std::fs::write(d.path().join("other"), "x").unwrap();
    assert!(w.wait(None, Some(Duration::from_millis(50))));
    assert!(!w.changed());
}

use std::collections::HashSet;

// wait wakes for a write, even one made before it started, and for stop;
// a file removed and written again is followed once watched again.
#[cfg(target_os = "macos")]
#[test]
fn watcher_wait() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("t.jsonl");
    std::fs::write(&f, "a\n").unwrap();
    let w = Arc::new(Watcher::new());
    w.watch(&[&f]);
    append(&f, "b\n");
    assert!(w.wait(None, Some(Duration::from_secs(30))));
    assert!(w.changes().0.contains(&f), "the wait's change wasn't told");
    // Nothing more: a short wait times out.
    assert!(w.wait(None, Some(Duration::ZERO)));
    assert!(!w.changed());
    // Removed: told, and watched afresh once written again.
    std::fs::remove_file(&f).unwrap();
    assert!(w.wait(None, Some(Duration::from_secs(30))));
    assert!(!w.watching(&f));
    std::fs::write(&f, "c\n").unwrap();
    w.watch(&[&f]);
    assert!(w.watching(&f));
    // Stop wakes a wait with nothing to say.
    let stop = Arc::new(Stop::new().unwrap());
    let (tx, rx) = mpsc::channel();
    {
        let (w, stop) = (w.clone(), stop.clone());
        std::thread::spawn(move || tx.send(w.wait(Some(&stop), None)).unwrap());
    }
    stop.close();
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(false));
}
