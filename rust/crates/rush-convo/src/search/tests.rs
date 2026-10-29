//! After internal/convo/find_test.go, wordindex_test.go and
//! convo_test.go's TestSearchEdges.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use super::find::{Stat, holds_all, lower_ascii};
use super::fuzzy::find_fold;
use super::pieces;
use super::*;
use crate::transcript::{Tail, decode};

fn claude() -> Reader {
    Reader::Transcript(decode(rush_claude::messages))
}

fn write_transcript(dir: &tempfile::TempDir, lines: &[&str]) -> String {
    let p: PathBuf = dir.path().join("s.jsonl");
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p.to_string_lossy().into_owned()
}

fn append(p: &str, line: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
    writeln!(f, "{line}").unwrap();
}

const FIND_LINES: [&str; 4] = [
    r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"fix the Flaky login test"},"origin":{"kind":"human"}}"#,
    r#"{"type":"assistant","timestamp":"2026-09-23T20:00:02Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go test ./auth"}}]}}"#,
    r#"{"type":"user","timestamp":"2026-09-23T20:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"Exit code 1\nFAIL TestLogin","is_error":true}]},"toolUseResult":{"stdout":"FAIL TestLogin","stderr":""}}"#,
    r#"{"type":"assistant","timestamp":"2026-09-23T20:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"The session cookie expired too early."}]}}"#,
];

#[test]
fn search_file() {
    let d = tempfile::tempdir().unwrap();
    let lib = Library::new(None);
    let p = write_transcript(&d, &FIND_LINES);
    let f = lib.search(&p, "LOGIN flaky", 10, &claude()).unwrap();
    assert_eq!(f.total, 1, "{f:?}");
    assert_eq!((f.hits[0].who.as_str(), f.hits[0].turn), ("you", 1));
    assert_ne!(f.hits[0].at, 0);
    assert!(f.first != 0 && f.last >= f.first, "{} {}", f.first, f.last);
    let f = lib.search(&p, "cookie", 10, &claude()).unwrap();
    assert_eq!((f.total, f.hits[0].who.as_str()), (1, WHO_AGENT));
    assert_eq!(
        lib.search(&p, "testlogin is:failed", 10, &claude())
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        lib.search(&p, "login nowhere", 10, &claude())
            .unwrap()
            .total,
        0
    );
    // A query without words reads no file.
    assert!(
        lib.search(&p, "is:failed", 10, &claude())
            .unwrap()
            .hits
            .is_empty()
    );
    // A typo still finds it; a short word must be as typed.
    assert_eq!(lib.search(&p, "cokie", 10, &claude()).unwrap().total, 1);
    assert_eq!(lib.search(&p, "fla", 10, &claude()).unwrap().total, 1);
}

/// The closest match scores highest: a whole word, then the start of one,
/// then inside one, then a typo; words side by side as typed count extra.
#[test]
fn match_scores() {
    let q = parse_query("login");
    let score = |t: &str| q.score(t).unwrap_or(0);
    let (whole, start, inside, typo) = (
        score("the login page"),
        score("the loginpage"),
        score("relogin now"),
        score("the lgoin page"),
    );
    assert!(
        whole > start && start > inside && inside > typo && typo > 0,
        "whole {whole} start {start} inside {inside} typo {typo}"
    );
    assert_eq!(score("nothing like it"), 0);
    let two = parse_query("flaky login");
    let side = two.score("a flaky login test").unwrap();
    let apart = two.score("login is flaky").unwrap();
    assert!(side > apart, "side by side {side}, apart {apart}");
    assert_eq!(match_span("fix the lgoin bug", "login"), Some((8, 13)));
}

#[test]
fn holds_all_across_chunks() {
    // The word straddles the 64 KiB read buffer's edge.
    let s = "x".repeat((64 << 10) - 3) + "NeedleY";
    assert!(holds_all(s.as_bytes(), &["needley".into()]).unwrap());
    assert!(!holds_all(s.as_bytes(), &["needley".into(), "absent".into()]).unwrap());
    // Words raw JSON can't be searched for are left to the full search.
    assert!(holds_all(&b"plain"[..], &["\"quoted\"".into(), "über".into()]).unwrap());
}

/// lower_ascii lowers every ASCII capital and nothing else, eight bytes at
/// a time or one.
#[test]
fn lower_ascii_table() {
    let all: Vec<u8> = (0..=255u8).collect();
    let ins: [&[u8]; 5] = [
        &all,
        "Hello WORLD @[`{ ÄÖÜ zZ".as_bytes(),
        b"AZ",
        b"@[",
        b"QWERTYUIOPASDFGHJKL",
    ];
    for inp in ins {
        for off in 0..8 {
            let mut b = inp[off.min(inp.len())..].to_vec();
            let mut want = b.clone();
            want.make_ascii_lowercase();
            lower_ascii(&mut b);
            assert_eq!(b, want, "{inp:?} at {off}");
        }
    }
}

/// A conversation a search read is held for the next: one that grows is
/// read on, one rewritten is read again, and nothing stale is found.
#[test]
fn search_holds_what_it_read() {
    let d = tempfile::tempdir().unwrap();
    let lib = Library::new(None);
    let p = write_transcript(&d, &FIND_LINES[..1]);
    assert_eq!(lib.search(&p, "flaky", 3, &claude()).unwrap().total, 1);
    assert!(lib.held(&p).is_some(), "not held after a search read it");
    append(
        &p,
        r#"{"type":"user","timestamp":"2026-09-23T20:05:00Z","cwd":"/work","message":{"role":"user","content":"the flaky one again"},"origin":{"kind":"human"}}"#,
    );
    assert_eq!(lib.search(&p, "flaky", 3, &claude()).unwrap().total, 2);
    std::fs::write(
        &p,
        r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"nothing here"},"origin":{"kind":"human"}}"#.to_string() + "\n",
    )
    .unwrap();
    assert_eq!(lib.search(&p, "flaky", 3, &claude()).unwrap().total, 0);
}

#[test]
fn search_edges() {
    let s = "İstanbul — Ünïcode ERROR here";
    let (i, j) = find_fold(s, "error").unwrap();
    assert_eq!(&s[i..j], "ERROR");
    let long = "— ".repeat(40) + "needle";
    let _ = snippet(&long, &["needle".into()]); // a str can't be cut mid-char: it'd panic
    let p = parse_query("turn:13-10 is:foo x");
    assert_eq!((p.from, p.to, p.unknown.len()), (10, 13, 1));
    let p = parse_query("turn:5-");
    assert!(p.from == 5 && p.to > 1000);
    assert!(filtered("is:failed") && !filtered("plain words"));
    assert_eq!(words("is:edit fix  Login"), ["fix", "login"]);
}

/// The index has the words of what was said and done, not of the folder
/// it ran in; it answers for typos and parts of words as search does, and
/// grows with the transcript.
#[test]
fn word_index() {
    let d = tempfile::tempdir().unwrap();
    let lib = Library::new(None);
    let p = write_transcript(
        &d,
        &[
            r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/src/worktrees/x","message":{"role":"user","content":"rotate the Zebracorn keys at the café"},"origin":{"kind":"human"}}"#,
        ],
    );
    assert_eq!(lib.search(&p, "rotate", 3, &claude()).unwrap().total, 1);
    let (st, _) = Stat::of(&p).unwrap();
    let ix = lib
        .index_for(&p, st)
        .expect("no index after a search read it");
    let cases: [(&[&str], bool); 7] = [
        (&["zebracorn"], true),
        (&["ebraco"], true),     // part of a word
        (&["zebracron"], true),  // a typo
        (&["cafe"], true),       // a letter past ASCII, a typo away
        (&["worktrees"], false), // only in the folder it ran in
        (&["rotate", "nowhere"], false),
        (&["keys.json"], true), // can't say: the session does
    ];
    for (ws, want) in cases {
        let ws: Vec<String> = ws.iter().map(|s| s.to_string()).collect();
        assert_eq!(ix.may_hold(&ws), want, "{ws:?}");
    }
    // Grown: the new line's words are taken in, raw, escapes and all.
    append(
        &p,
        r#"{"type":"user","timestamp":"2026-09-23T20:01:00Z","message":{"role":"user","content":"then\nunicornify it"}}"#,
    );
    let (st, _) = Stat::of(&p).unwrap();
    let ix = lib.index_for(&p, st).unwrap();
    assert!(ix.may_hold(&["unicornify".into()]) && ix.may_hold(&["then".into()]));
    assert_eq!(lib.search(&p, "unicornify", 3, &claude()).unwrap().total, 1);
}

/// An index is kept on disk and read back by a rush started afresh; one of
/// a file since rewritten isn't trusted.
#[test]
fn word_index_on_disk() {
    let d = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let lib = Library::new(Some(home.path().to_path_buf()));
    let p = write_transcript(&d, &FIND_LINES);
    assert_eq!(lib.search(&p, "cookie", 3, &claude()).unwrap().total, 1);
    let fresh = Library::new(Some(home.path().to_path_buf()));
    let (st, _) = Stat::of(&p).unwrap();
    let ix = fresh.index_for(&p, st).expect("not read back from disk");
    assert!(ix.may_hold(&["cookie".into()]) && !ix.may_hold(&["zebracorn".into()]));
    std::fs::write(&p, FIND_LINES[0].to_string() + "\n").unwrap();
    let (st, _) = Stat::of(&p).unwrap();
    assert!(
        Library::new(Some(home.path().to_path_buf()))
            .index_for(&p, st)
            .is_none()
    );
}

/// The indexer indexes what it's given (here at once, as its thread does).
#[test]
fn index_one() {
    let d = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let lib = Library::new(Some(home.path().to_path_buf()));
    let p = write_transcript(&d, &FIND_LINES);
    let src = Source {
        path: p.clone(),
        read: claude(),
    };
    assert!(!lib.indexed(&p));
    assert!(lib.index_one(&src));
    assert!(lib.indexed(&p));
    // Indexed as it stands: not read again.
    assert!(!lib.index_one(&src));
    // Without a folder, nothing runs in the background.
    Library::new(None).index_in_background(vec![src]);
}

fn read(p: &str) -> crate::Session {
    let mut t = Tail::new(p, decode(rush_claude::messages));
    let mut s = crate::Session::new();
    t.read(&mut s).unwrap();
    s
}

/// The pieces find just what reading the session whole does.
fn same_pieces(p: &str, queries: &[&str]) {
    let s = read(p);
    let (st, _) = Stat::of(p).unwrap();
    let pi = pieces::build(&s, st);
    for q in queries {
        let want = pieces::found(s.search(q), 1 << 30, s.first, s.last);
        let got = pi.search(&parse_query(q), 1 << 30);
        assert_eq!(got, want, "{q:?}");
    }
}

#[test]
fn pieces_find_as_the_session_does() {
    let d = tempfile::tempdir().unwrap();
    same_pieces(
        &write_transcript(&d, &FIND_LINES),
        &[
            "login",
            "flaky login",
            "LOGIN flaky",
            "cookie",
            "cokie",
            "testlogin",
            "go test",
            "auth",
            "fail",
            "exit code",
            "nowhere",
            "the",
            "fla",
            "flaky nowhere",
        ],
    );
}

/// So do a written one's pieces, read back from disk; a search with words
/// alone goes by them, one with is: reads the session.
#[test]
fn pieces_on_disk() {
    let d = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().to_path_buf();
    let lib = Library::new(Some(dir.clone()));
    let p = write_transcript(&d, &FIND_LINES);
    let s = read(&p);
    let (st, _) = Stat::of(&p).unwrap();
    let pi = pieces::build(&s, st);
    pieces::save(&dir, &p, &pi).unwrap();
    let back = pieces::load(Some(&dir), &p, st).expect("read back");
    for q in ["login", "cookie", "go test"] {
        assert_eq!(
            pi.search(&parse_query(q), 9),
            back.search(&parse_query(q), 9),
            "{q}"
        );
    }
    assert!(pieces::stand(Some(&dir), &p, st));
    lib.index_session(&p, st, &s);
    lib.forget(&p);
    assert_eq!(lib.search(&p, "cookie", 3, &claude()).unwrap().total, 1);
    assert_eq!(
        lib.held(&p),
        Some(true),
        "the search didn't go by the pieces"
    );
    assert_eq!(
        lib.search(&p, "testlogin is:failed", 3, &claude())
            .unwrap()
            .total,
        1
    );
}

/// Another agent's conversation is read through its own reader, only once
/// its file holds every word.
#[test]
fn search_read() {
    let d = tempfile::tempdir().unwrap();
    let lib = Library::new(None);
    let p = write_transcript(&d, &FIND_LINES);
    let pp = p.clone();
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n = reads.clone();
    let other = Reader::Other(Arc::new(move || {
        n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(read(&pp))
    }));
    assert_eq!(lib.search(&p, "zebracorn", 3, &other).unwrap().total, 0);
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(lib.search(&p, "cookie", 3, &other).unwrap().total, 1);
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
}
