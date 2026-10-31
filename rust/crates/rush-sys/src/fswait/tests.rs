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

/// A file followed a line at a time, as an adapter's Follow reads one.
struct Lines {
    path: std::path::PathBuf,
    off: u64,
    seen: Option<(u64, u64)>,
}

impl rush_core::adapter::Follow for Lines {
    fn next(&mut self, _most: usize) -> rush_core::adapter::Result<rush_core::adapter::Slice> {
        use std::io::{Read, Seek};
        let mut f = std::fs::File::open(&self.path)?;
        let m = f.metadata()?;
        let mut out = rush_core::adapter::Slice::default();
        if rush_core::adapter::rewritten(&mut self.seen, self.off, &m) {
            self.off = 0;
            out.reset = true;
        }
        f.seek(std::io::SeekFrom::Start(self.off))?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        self.off += s.len() as u64;
        out.events = s
            .lines()
            .map(|l| {
                rush_core::event::Event::Status(rush_core::event::Status {
                    text: l.into(),
                    ..Default::default()
                })
            })
            .collect();
        Ok(out)
    }
    fn wait_on(&self) -> rush_core::adapter::WaitOn {
        rush_core::adapter::WaitOn {
            paths: vec![self.path.clone()],
            ..Default::default()
        }
    }
}

fn status_texts(s: &rush_core::adapter::Slice) -> Vec<String> {
    s.events
        .iter()
        .filter_map(|e| match e {
            rush_core::event::Event::Status(s) => Some(s.text.clone()),
            _ => None,
        })
        .collect()
}

/// A write landing after the last read and before the wait wakes the
/// wait: the watch was registered before the read. And a file renamed
/// over by a longer one is a rewrite, though it didn't shrink.
#[cfg(target_os = "macos")]
#[test]
fn follow_misses_no_write() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("t.jsonl");
    std::fs::write(&p, "a\n").unwrap();
    let stop = Arc::new(Stop::new().unwrap());
    let (tx, rx) = mpsc::channel();
    let (p2, st) = (p.clone(), stop.clone());
    std::thread::spawn(move || {
        let mut f = Lines {
            path: p2.clone(),
            off: 0,
            seen: None,
        };
        let mut n = 0;
        super::follow(&mut f, 1 << 20, &st, &mut |s| {
            let s = s.unwrap();
            n += 1;
            tx.send((s.reset, status_texts(&s))).unwrap();
            match n {
                // Between the last read and the wait.
                1 => append(&p2, "b\n"),
                2 => {
                    let new = p2.with_extension("new");
                    std::fs::write(&new, "x\ny\nz\n").unwrap();
                    std::fs::rename(&new, &p2).unwrap();
                }
                _ => return false,
            }
            true
        });
    });
    let got = |what: &str| {
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{what} wasn't noticed"))
    };
    assert_eq!(got("the first read"), (false, vec!["a".to_string()]));
    assert_eq!(
        got("a write before the wait"),
        (false, vec!["b".to_string()])
    );
    let (reset, texts) = got("a file renamed over");
    assert!(reset, "a file renamed over it wasn't a rewrite");
    assert_eq!(texts, ["x", "y", "z"]);
    stop.close();
}

// A watcher of many: a folder of many watched paths is watched through
// one FSEvents stream, with no descriptor each, and tells what kqueue
// would: a folder's entries added, removed or renamed, a file written,
// but not a file written deep inside a watched folder.
#[cfg(target_os = "macos")]
#[test]
fn watcher_many() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("sessions");
    std::fs::create_dir(&root).unwrap();
    let dirs: Vec<PathBuf> = (0..20)
        .map(|i| {
            let s = root.join(format!("s{i}"));
            std::fs::create_dir(&s).unwrap();
            std::fs::write(s.join("events.jsonl"), "a\n").unwrap();
            s
        })
        .collect();
    let files: Vec<PathBuf> = (0..10)
        .map(|i| {
            let f = root.join(format!("f{i}.json"));
            std::fs::write(&f, "{}").unwrap();
            f
        })
        .collect();
    let lone = d.path().join("state.json");
    std::fs::write(&lone, "{}").unwrap();
    let mut paths = vec![root.clone(), lone.clone()];
    paths.extend(dirs.iter().cloned());
    paths.extend(files.iter().cloned());
    let w = Watcher::many();
    // Nothing keeps a descriptor for having changed lately, till below.
    w.inner().recent = Duration::ZERO;
    w.watch(&paths);
    // Only the few have a descriptor each.
    assert_eq!(w.inner().fds.len(), 2);
    assert!(paths.iter().all(|p| w.watching(p)));
    let (got, all) = w.changes();
    assert!(all && got.len() == paths.len(), "first ask: {}", got.len());
    // Told once a stream's news is in: wait for it (a wait times out true
    // too, so what's told is what counts).
    let next = |w: &Watcher| {
        w.wait(None, Some(Duration::from_millis(500)));
        w.changes()
    };
    assert_eq!(next(&w), (HashSet::new(), false), "nothing happened");
    // What's told until p is, over a few waits.
    let told = |w: &Watcher, p: &PathBuf| {
        let mut got = w.changes().0;
        for _ in 0..10 {
            if got.contains(p) {
                break;
            }
            got.extend(next(w).0);
        }
        got
    };

    // info.json replaced by rename in a session's folder: the folder.
    let tmp = dirs[3].join("info.json.tmp");
    std::fs::write(&tmp, "{}").unwrap();
    let t0 = std::time::Instant::now();
    std::fs::rename(&tmp, dirs[3].join("info.json")).unwrap();
    assert!(w.wait(None, Some(Duration::from_secs(10))));
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "slow: {:?}",
        t0.elapsed()
    );
    assert_eq!(told(&w, &dirs[3]), HashSet::from([dirs[3].clone()]));

    // Appends to a file inside a watched folder: kqueue on the folder
    // wouldn't tell, nor does this.
    for _ in 0..3 {
        append(&dirs[5].join("events.jsonl"), "b\n");
    }
    assert_eq!(next(&w).0, HashSet::new(), "a deep append was told");

    // A watched file written in place is told.
    append(&files[2], " ");
    assert_eq!(told(&w, &files[2]), HashSet::from([files[2].clone()]));

    // A folder removed: told, and no longer watched; back, it's watched
    // again by the next watch and told once.
    std::fs::remove_dir_all(&dirs[7]).unwrap();
    let got = told(&w, &dirs[7]);
    assert!(got.contains(&dirs[7]) && !w.watching(&dirs[7]), "{got:?}");
    // The sessions folder itself has a descriptor: told too.
    assert!(got.contains(&root), "{got:?}");
    std::fs::create_dir(&dirs[7]).unwrap();
    w.watch(&paths);
    assert!(w.watching(&dirs[7]));
    assert!(w.changes().0.contains(&dirs[7]));

    // The lone file keeps kqueue's word: told at once.
    append(&lone, " ");
    assert!(w.changes().0.contains(&lone));

    // New since the folder was first watched, a path keeps a descriptor
    // a while (a session just made is likely live): told at once.
    std::fs::remove_dir_all(&dirs[13]).unwrap();
    assert!(told(&w, &dirs[13]).contains(&dirs[13]));
    std::fs::create_dir(&dirs[13]).unwrap();
    w.inner().recent = Duration::from_secs(3600);
    w.watch(&paths);
    assert!(
        w.inner().fds.contains_key(&dirs[13]),
        "a new one on the stream"
    );
    w.changes();
    std::fs::write(dirs[13].join("info.json"), "{}").unwrap();
    assert!(w.changes().0.contains(&dirs[13]), "not told at once");
    // Quiet a while, it goes onto the stream, its descriptor kept till
    // the next ask has read what it told.
    w.inner().recent = Duration::ZERO;
    w.watch(&paths);
    assert!(w.inner().fds.contains_key(&dirs[13]) && w.watching(&dirs[13]));
    std::fs::write(dirs[13].join("events.jsonl.tmp"), "").unwrap();
    assert!(
        w.changes().0.contains(&dirs[13]),
        "lost moving to the stream"
    );
    assert!(!w.inner().fds.contains_key(&dirs[13]) && w.watching(&dirs[13]));

    // Told by the stream, a path stays on it: no stream made again.
    let before = w.inner().streamed.clone();
    std::fs::write(dirs[9].join("info.json"), "{}").unwrap();
    assert!(told(&w, &dirs[9]).contains(&dirs[9]));
    w.watch(&paths);
    assert_eq!(w.inner().streamed, before);

    // Written in deep inside, a folder gets a descriptor of its own
    // (writes there cost the stream a delivery each); quiet, it goes back.
    for _ in 0..3 {
        append(&dirs[11].join("events.jsonl"), "b\n");
    }
    let mut got = false;
    for _ in 0..20 {
        w.wait(None, Some(Duration::from_millis(100)));
        if w.inner().fds.contains_key(&dirs[11]) {
            got = true;
            break;
        }
    }
    assert!(got, "a busy folder stayed on the stream");
    assert!(!w.inner().streamed.iter().any(|p| p.ends_with("s11")));
    std::fs::write(dirs[11].join("x"), "").unwrap();
    assert!(w.changes().0.contains(&dirs[11]), "busy, not told at once");
    w.watch(&paths);
    w.changes();
    assert!(!w.inner().fds.contains_key(&dirs[11]) && w.watching(&dirs[11]));

    // Fewer than many in the folder: back to a descriptor each.
    w.watch(&[&root, &dirs[0], &dirs[1]]);
    assert_eq!(w.inner().fds.len(), 3);
    assert!(w.inner().stream.is_none());
    w.close();
}
