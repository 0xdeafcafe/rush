//! A run's pane, open on the conversation's page (subpane.rs): its
//! conversation drawn and driven as a session's, opening at its end; its
//! box takes what's typed, never the session's; /compact says what it can
//! do. Rust-only. No sleeps: a model with no loop reads at once.

use rush_core::text::Line;

use super::host::Fake;
use super::tests_subview::{KEY, model, picked_beside, press, typed};
use super::{conn, openrun, subpane, subview};
use crate::model::Model;
use crate::session::Pos;

fn text(ls: &[Line]) -> String {
    ls.iter()
        .map(|l| l.plain().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn sel(m: &Model) -> String {
    m.session.as_ref().unwrap().sel.clone()
}

/// The run's own view's pick, and where it is.
fn run_view(m: &mut Model) -> (String, Pos) {
    let d = conn(m, KEY).runs.detail.as_ref().expect("the run read");
    (d.view.sel.clone(), d.view.pos)
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

/// A run's transcript of n exchanges, each answer long: well over the
/// 2 MB a long conversation's end is read in.
fn long_run(n: usize) -> String {
    let pad = "x".repeat(2000);
    let mut out = String::new();
    for i in 0..n {
        out.push_str(&format!(
            r#"{{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{{"role":"user","content":"question {i}"}}}}"#
        ));
        out.push('\n');
        out.push_str(&format!(
            r#"{{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{{"id":"m{i}","model":"claude-opus-4-1","role":"assistant","content":[{{"type":"text","text":"answer {i} {pad}"}}]}}}}"#
        ));
        out.push('\n');
    }
    out
}

/// Picking a long run shows its end in its pane, as a long session
/// opens: its last answer in sight, its first not, following it; only its
/// end is read till the pane's scrolled up to the top of it, and then
/// what came before is read above what shows, which stays in sight.
#[test]
fn a_long_run_shows_its_end() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    (m.w, m.h) = (180, 80);
    let x = d.path().join("agent-long.jsonl");
    let body = long_run(1400);
    assert!(body.len() > 2 << 20, "not long enough: {}", body.len());
    std::fs::write(&x, body).unwrap();
    let path = x.to_string_lossy().into_owned();
    let r = &mut conn(&mut m, KEY).runs;
    for sa in r.subs.iter_mut().chain(r.listed.iter_mut()) {
        if sa.id == "x" {
            sa.path.clone_from(&path);
        }
    }
    super::subs::kick(&mut m, KEY);
    super::subs::refresh(&mut m, KEY);
    subview::pick_run(&mut m, KEY, "x");
    // Read on as the loop would, till nothing more comes.
    for _ in 0..4 {
        let _ = m.frame();
        super::subs::refresh(&mut m, KEY);
    }
    let all = text(&m.frame());
    assert!(all.contains("answer 1399"), "not at its end:\n{all}");
    assert!(!all.contains("answer 0 "), "at its start:\n{all}");
    assert!(
        conn(&mut m, KEY).watch.frame[0] > 2,
        "not beside the conversation:\n{all}"
    );
    assert_eq!(run_view(&mut m).1, Pos::End);
    let end = &conn(&mut m, KEY).runs.detail.as_ref().unwrap().view.sess;
    assert!(end.partial, "its whole read, not scrolled up to");
    let (cut, first) = (end.turns.len(), end.turns[0].n);
    assert!(cut < 1400, "its whole read: {cut} turns");

    // Scrolled to the top of its end: what came before is read, and the
    // rows that showed still show.
    conn(&mut m, KEY).runs.detail.as_mut().unwrap().view.pos = Pos::At { n: first, off: 0 };
    let top = text(&m.frame());
    let shown = top
        .lines()
        .find_map(|l| {
            l.find("answer ")
                .map(|i| l[i..].split(' ').nth(1).unwrap().to_string())
        })
        .expect("an answer at the top");
    for _ in 0..4 {
        super::subs::refresh(&mut m, KEY);
        let _ = m.frame();
    }
    let whole = &conn(&mut m, KEY).runs.detail.as_ref().unwrap().view.sess;
    assert_eq!(whole.turns.len(), 1400, "what came before not read");
    assert!(!whole.partial);
    let all = text(&m.frame());
    let at = format!("answer {shown}");
    assert!(
        all.lines()
            .any(|l| l.ends_with(&at) || l.contains(&format!("{at} "))),
        "moved off answer {shown}:\n{all}"
    );
    assert!(!all.contains("answer 1399"), "back at its end:\n{all}");
}

/// The run opened has the keys, its pane's rows a session's: ↑ from the
/// empty box picks the row by it and goes on up, enter opens the row
/// picked, esc lets the pick go, esc again closes the run; the
/// conversation's own pick stays where it was throughout.
#[test]
fn the_panes_rows_are_a_sessions() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    let was = sel(&m);
    subview::pick_run(&mut m, KEY, "x");
    let _ = m.frame();
    assert!(
        conn(&mut m, KEY).watch.msg_focus,
        "it opened without the keys"
    );
    press(&mut m, "up");
    let (first, _) = run_view(&mut m);
    assert!(!first.is_empty(), "↑ picked nothing");
    // Up to the Grep step.
    for _ in 0..6 {
        if run_view(&mut m).0.contains(":s:") {
            break;
        }
        press(&mut m, "up");
        let _ = m.frame();
    }
    let (step, _) = run_view(&mut m);
    assert!(step.contains(":s:"), "no step picked: {step}");
    let open = |m: &mut Model| {
        conn(m, KEY)
            .runs
            .detail
            .as_ref()
            .unwrap()
            .view
            .o
            .open
            .get(&step)
            .copied()
    };
    let before = open(&mut m);
    press(&mut m, "enter");
    assert_ne!(before, open(&mut m), "enter didn't open the step");
    assert_eq!(sel(&m), was, "the conversation's pick moved");
    press(&mut m, "esc");
    assert_eq!(run_view(&mut m).0, "");
    assert!(conn(&mut m, KEY).watch.msg_focus, "esc with a pick left");
    press(&mut m, "esc");
    assert!(!openrun::is_open(&m, KEY), "esc didn't close it");
    assert!(!conn(&mut m, KEY).watch.msg_focus);
    assert_eq!(sel(&m), was);
}

/// ↑↓ are the pane's while it has the keys: the conversation's pick
/// stays; tab gives them back, and the run stays open.
#[test]
fn arrows_are_the_panes_while_it_has_the_keys() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    let was = sel(&m);
    press(&mut m, "tab");
    assert!(conn(&mut m, KEY).watch.msg_focus);
    press(&mut m, "up");
    assert_eq!(sel(&m), was, "↑ moved the conversation");
    assert!(conn(&mut m, KEY).watch.msg_focus, "↑ lost the keys");
    assert!(!run_view(&mut m).0.is_empty(), "↑ picked no row of the run");
    press(&mut m, "esc");
    press(&mut m, "tab");
    assert!(!conn(&mut m, KEY).watch.msg_focus);
    assert_eq!(openrun::open_id(&m, KEY).as_deref(), Some("x"));
}

/// What's typed with the pane's keys goes to the run's box, never the
/// session's; /compact for a Claude Code subagent says it can't be done,
/// and sends nothing.
#[test]
fn the_panes_box_is_the_runs() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    press(&mut m, "tab");
    typed(&mut m, "/compact");
    assert!(
        conn(&mut m, KEY).input.is_empty(),
        "it went to the main box"
    );
    assert_eq!(
        super::subbox::draft(&mut m, KEY, "x").input.string(),
        "/compact"
    );
    press(&mut m, "enter");
    assert!(
        flash(&m).contains("Claude Code can't compact a subagent"),
        "{}",
        flash(&m)
    );
    assert!(fake.ops().is_empty(), "sent: {:?}", fake.ops());
}

/// The key guide, opened from the pane, lists the pane's keys first.
#[test]
fn the_key_guide_lists_the_panes_keys() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    press(&mut m, "tab");
    let (title, keys) = subpane::here_keys(&mut m).expect("the pane's keys");
    assert_eq!(title, "Subagent's pane");
    assert!(keys.iter().any(|(k, _)| *k == "ctrl+o"), "{keys:?}");
    press(&mut m, "tab");
    assert!(
        subpane::here_keys(&mut m).is_none(),
        "the conversation has the keys"
    );
}
