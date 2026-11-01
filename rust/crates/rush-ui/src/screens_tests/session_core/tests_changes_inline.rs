//! rush-ui-session-core's changes.rs tests: they make a whole UI model.

use super::changes::*;
use super::paint::text;
use crate::style::*;
use rush_convo::Session;
use rush_convo::render::Places;

// changes.go:306, :786: until git is read there's no root, and git
// never runs with an empty -C (rush's own folder); not a repository,
// it runs in the folder and a full view is the file as it is, a
// terminal's codes taken out of it (FileView's cleanOutput).
#[test]
fn full_file_waits_for_git_and_drops_terminal_codes() {
    let mut m = crate::new_model("0.1.0");
    let reading = Tree {
        dir: String::new(),
        err: "reading git…".into(),
        ..Tree::default()
    };
    let f = TreeFile {
        path: "/nowhere/a.txt".into(),
        ..TreeFile::default()
    };
    assert_eq!(tree_diff(&mut m, &reading, &f, true), vec!["reading git…"]);
    assert!(m.kept::<Trees>().diffs.is_empty(), "no git was run");

    let d = std::env::temp_dir().join(format!("rush-full-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let path = d.join("a.txt").to_string_lossy().into_owned();
    std::fs::write(&path, "\x1b[31mred\x1b[0m\x07\n").unwrap();
    let dir = d.to_string_lossy().into_owned();
    let mut v = crate::session::SessionView::new("k".into(), crate::session::Source::Hosted, 1);
    v.sess.cwd = dir.clone();
    m.session = Some(v);
    m.kept::<Trees>().by_dir.insert(
        dir.clone(),
        Tree {
            dir,
            err: "not a git repository".into(),
            at: 1,
            ..Tree::default()
        },
    );
    file_view(&mut m, &path, 80); // the diff is read, then drawn
    let rows: Vec<String> = file_view(&mut m, &path, 80)
        .iter()
        .map(|(l, _)| l.plain().trim_end().to_string())
        .collect();
    let _ = std::fs::remove_dir_all(&d);
    assert!(rows.iter().any(|r| r.ends_with("1   red")), "{rows:?}");
    assert!(
        rows.iter()
            .all(|r| !r.contains('\x1b') && !r.contains('\x07')),
        "{rows:?}"
    );
}

#[test]
fn diff_lines_drop_headers() {
    let d = "diff --git a/x b/x\nindex 1..2\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file\n";
    assert_eq!(diff_lines(d), vec!["@@ -1 +1 @@", "-a", "+b"]);
}

#[test]
fn turns_leave_the_row_when_they_crowd_it() {
    let c = line([plain("+1 −0")]);
    assert_eq!(turns_right(c.clone(), &[3, 4], 80).plain(), "+1 −0   #3 #4");
    let many: Vec<u32> = (1..60).collect();
    assert_eq!(turns_right(c, &many, 40).plain(), "");
}

// render.go:2560 rel, :2571 shortAbs: from the first folder it's in,
// its symlinks resolved too; else home as ~ and a long one's middle
// left out.
#[test]
fn rel_paths() {
    let b = ["/tmp/w".to_string(), "/private/tmp/w".to_string()];
    let home = std::env::var("HOME").unwrap();
    for (p, want) in [
        ("/tmp/w/src/a.rs".to_string(), "src/a.rs"),
        ("/private/tmp/w/src/a.rs".into(), "src/a.rs"),
        ("/tmp/w".into(), "."),
        ("/tmp/wx/a.rs".into(), "/tmp/wx/a.rs"),
        ("/x/a.rs".into(), "/x/a.rs"),
        (format!("{home}/n/a.rs"), "~/n/a.rs"),
        (
            "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/u/v/w.rs".into(),
            "/a/…/v/w.rs",
        ),
    ] {
        assert_eq!(Places::with_bases("", b.to_vec()).rel(&p), want, "{p}");
    }
}

// render.go:2007 fileLink: a path, made absolute from the folder,
// opens as a file:// URL; one with nowhere to be from isn't linked.
#[test]
fn file_rows_link() {
    let l = file_link(
        &Places::with_bases("/w", vec![]),
        "src/a b.rs",
        text("src/a b.rs"),
    );
    assert_eq!(
        l.style.link.url().as_deref(),
        Some("file:///w/src/a%20b.rs")
    );
    let l = file_link(&Places::default(), "/x/a.rs", text("a.rs"));
    assert_eq!(l.style.link.url().as_deref(), Some("file:///x/a.rs"));
    assert_eq!(
        file_link(&Places::default(), "a.rs", text("a.rs"))
            .style
            .link
            .url()
            .as_deref(),
        None
    );
}

/// How long working out a real session's edits takes, the first time
/// and again (MEMO_TRANSCRIPTS names a file listing transcripts; without
/// it there's nothing to do). For measuring: `cargo test --release -p
/// rush-ui changes_cost -- --nocapture`.
#[test]
fn changes_cost() {
    let Ok(list) = std::env::var("MEMO_TRANSCRIPTS") else {
        return;
    };
    for p in std::fs::read_to_string(list).unwrap().lines().take(12) {
        let mut s = Session::new();
        let mut t = rush_convo::Tail::new(p, rush_convo::decode(rush_claude::messages));
        t.read(&mut s).unwrap();
        let time = || {
            let t0 = std::time::Instant::now();
            let n = changes(&s).len();
            (t0.elapsed().as_micros(), n)
        };
        let (a, n) = time();
        let (b, m) = time();
        assert_eq!(n, m);
        println!(
            "{} steps {} files {n}: first {a} us, again {b} us",
            p.rsplit('/').next().unwrap_or(""),
            s.steps().len()
        );
    }
}
