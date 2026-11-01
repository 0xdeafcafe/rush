//! The tests of rush-ui-projects-machine's memreview.rs that need the whole UI.

use crate::frame::sheet::Sheet;
use crate::frame::{Out, Press};
use crate::model::Model;
use rush_core::adapter::{MemoryFile, Profile};
use rush_memory::Keeper;
use rush_ui_projects_machine::memreview::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use std::sync::Mutex;

/// A keeper that merges by appending, as Claude Code's does, and says
/// what it was asked.
#[derive(Default)]
struct Fake {
    asked: Mutex<Vec<String>>,
}

impl Keeper for Fake {
    fn remember(
        &self,
        _: &Profile,
        _: &str,
        _: &rush_memory::MemoryNote,
    ) -> rush_memory::Result<String> {
        Err("no".into())
    }
    fn tidy_memory(&self, _: &str) -> rush_memory::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn merge_notes(&self, keep: &str, drop: &str) -> rush_memory::Result<()> {
        let add = std::fs::read_to_string(drop).map_err(|e| e.to_string())?;
        let mut to = std::fs::read_to_string(keep).map_err(|e| e.to_string())?;
        to += &add;
        std::fs::write(keep, to).map_err(|e| e.to_string())?;
        self.asked.lock().unwrap().push(format!("merge {drop}"));
        std::fs::remove_file(drop).map_err(|e| e.to_string())
    }
    fn make_global(&self, _: &Profile, path: &str) -> rush_memory::Result<()> {
        self.asked.lock().unwrap().push(format!("global {path}"));
        Ok(())
    }
    fn read_note(&self, _: &str) -> rush_memory::Result<rush_memory::MemoryNote> {
        Err("no".into())
    }
    fn count_opens(
        &self,
        _: &Profile,
        _: &rush_core::adapter::Memory,
    ) -> HashMap<String, rush_memory::Opens> {
        HashMap::new()
    }
}

fn note(dir: &Path, name: &str, kind: &str, body: &str) -> MemoryFile {
    let path = dir.join(format!("{name}.md"));
    std::fs::write(&path, body).unwrap();
    MemoryFile {
        group: "Auto memory".into(),
        path: path.to_string_lossy().into_owned(),
        name: name.into(),
        kind: kind.into(),
        ..MemoryFile::default()
    }
}

/// Two notes worth a look: one repeats another, one is old.
fn review(dir: &Path) -> MemReview {
    let tests = note(dir, "tests", "feedback", "Targeted tests.");
    let again = note(dir, "tests-again", "feedback", "Again.");
    let launch = note(dir, "launch", "project", "Launch Friday.");
    let items = vec![
        MemReviewItem {
            f: again.clone(),
            why: "repeats tests".into(),
            text: "Again.".into(),
            same: tests.path.clone(),
        },
        MemReviewItem {
            f: launch.clone(),
            why: "unchanged since 2 Jan · is it still true?".into(),
            text: "Launch Friday.".into(),
            same: String::new(),
        },
    ];
    MemReview {
        p: Profile::default(),
        title: "app".into(),
        items,
        notes: vec![tests, again, launch],
    }
}

fn press(s: &mut MemReviewSheet, m: &mut Model, k: &str) -> Out {
    s.key(m, &Press::of(k).unwrap())
}

fn body(s: &mut MemReviewSheet, m: &Model) -> String {
    s.body(m, 100, 30)
        .iter()
        .map(|l| l.plain())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The repeat merges straight into the note it repeats; a kept note
/// isn't asked about again until it changes (TestMemReview).
#[test]
fn mem_review() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = crate::new_model("t");
    let fake = Arc::new(Fake::default());
    let mut s = MemReviewSheet::new(review(dir.path()), Some(fake.clone()));
    let b = body(&mut s, &m);
    for want in [
        "1 of 2",
        "m merge into tests",
        "g to CLAUDE.md",
        "! repeats tests",
    ] {
        assert!(b.contains(want), "sheet lacks {want:?}:\n{b}");
    }
    let first = s.cur().unwrap().f.path.clone();
    press(&mut s, &mut m, "m");
    assert!(
        !Path::new(&first).exists(),
        "the repeat should be merged away"
    );
    let kept = std::fs::read_to_string(dir.path().join("tests.md")).unwrap();
    assert!(
        kept.contains("Targeted tests.") && kept.contains("Again."),
        "{kept}"
    );
    let launch = s.cur().unwrap().f.clone();
    press(&mut s, &mut m, "k");
    assert!(s.cur().is_none(), "review should be done: {:?}", s.done);
    let b = body(&mut s, &m);
    assert!(b.contains("1 kept") && b.contains("1 merged"), "{b}");
    let mut f = launch;
    f.mod_time = std::fs::metadata(&f.path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(rush_memory::is_kept(&f.path, f.mod_time));
    assert_eq!(press(&mut s, &mut m, "esc"), Out::Close);
}

/// Merging with nothing else left to merge into says so, and never
/// picks past the list; back steps over notes the review already
/// forgot (TestMemReviewEdges).
#[test]
fn mem_review_edges() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = crate::new_model("t");
    let mut s = MemReviewSheet::new(review(dir.path()), Some(Arc::new(Fake::default())));
    s.r.notes = vec![s.cur().unwrap().f.clone()];
    s.r.items[0].same.clear();
    press(&mut s, &mut m, "m");
    assert!(!s.merging, "no picker with nothing to merge into");
    s.merging = true;
    press(&mut s, &mut m, "down");
    assert_eq!(press(&mut s, &mut m, "enter"), Out::Idle);
    assert_eq!(s.pick, 0);
    s.merging = false;
    let gone = s.r.items[0].f.path.clone();
    s.done.insert(gone, "forgotten".into());
    s.i = 1;
    press(&mut s, &mut m, "left");
    assert_eq!(s.i, 1, "back landed on a forgotten note");
}

/// The repeat the agent found is the one m merges into (f.Same).
#[test]
fn same_is_the_agents() {
    let dir = tempfile::tempdir().unwrap();
    let r = review(dir.path());
    let mut f = r.items[0].f.clone();
    f.same = r.notes[0].path.clone();
    f.reads = 4;
    f.last_read = 1;
    let mut s = MemReviewSheet::new(
        MemReview {
            items: vec![MemReviewItem {
                same: f.same.clone(),
                f,
                why: "repeats tests".into(),
                text: String::new(),
            }],
            ..r
        },
        Some(Arc::new(Fake::default())),
    );
    let m = crate::new_model("t");
    let b = body(&mut s, &m);
    assert!(b.contains("m merge into tests"), "{b}");
    assert!(
        b.contains("feedback · tests-again · opened 4× · last "),
        "{b}"
    );
}

/// e opens the note in $EDITOR; once that's done it's counted edited
/// and the review moves on (memreview.go:221).
#[test]
fn e_edits_it_in_the_editor() {
    use crate::screens::session_core::memedit::{Done, set_editor};
    let dir = tempfile::tempdir().unwrap();
    let mut m = crate::new_model("t");
    type Slot = Arc<Mutex<Option<(String, Done)>>>;
    let slot: Slot = Arc::new(Mutex::new(None));
    let s2 = slot.clone();
    set_editor(
        &mut m,
        Arc::new(move |_, path: &str, done| {
            *s2.lock().unwrap() = Some((path.to_string(), done));
            Out::Draw
        }),
    );
    let r = review(dir.path());
    let first = r.items[0].f.path.clone();
    m.open_sheet(Box::new(MemReviewSheet::new(r, None)));
    m.sheet_key(&Press::of("e").unwrap());
    let (path, done) = slot.lock().unwrap().take().expect("the editor ran");
    assert_eq!(path, first);
    assert_eq!(done(&mut m, Ok(())), Out::Draw);
    let s = m.sheet_as::<MemReviewSheet>().unwrap();
    assert_eq!(s.done.get(&first).map(String::as_str), Some("edited"));
    assert_eq!(s.i, 1, "on to the next");
}

/// e hands the terminal to the editor for real: a fake one, run once
/// the key has been handled, edits the note, and the review counts it.
#[test]
fn e_runs_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = crate::new_model("t");
    m.set_editor_program("f() { echo edited >> \"$1\"; }; f");
    let r = review(dir.path());
    let first = r.items[0].f.path.clone();
    m.open_sheet(Box::new(MemReviewSheet::new(r, None)));
    m.sheet_key(&Press::of("e").unwrap());
    assert_eq!(m.run_outside(), 1);
    let text = std::fs::read_to_string(&first).unwrap();
    assert!(text.ends_with("edited\n"), "{text}");
    let s = m.sheet_as::<MemReviewSheet>().unwrap();
    assert_eq!(s.done.get(&first).map(String::as_str), Some("edited"));
}

/// Without a keeper, merging and moving do nothing.
#[test]
fn mem_review_without_a_keeper() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = crate::new_model("t");
    let mut s = MemReviewSheet::new(review(dir.path()), None);
    assert_eq!(press(&mut s, &mut m, "m"), Out::Idle);
    assert_eq!(press(&mut s, &mut m, "g"), Out::Idle);
    assert!(!s.merging && m.confirm.is_none());
}
