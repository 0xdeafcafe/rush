//! A run opened whole (runview.rs), and the subagents list's own keys:
//! its conversation drawn and driven as a session's, opening at its end;
//! ↑↓ through the list whatever has the keys; older finished runs folded
//! under Earlier. Rust-only. No sleeps: a model with no loop reads at once.

use rush_convo::Subagent;
use rush_core::text::Line;

use super::host::Fake;
use super::tests_subview::{KEY, model, picked_beside, press};
use super::{conn, runview, subview, views};
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

/// The run's own view's pick, and what it has opened.
fn run_view(m: &mut Model) -> (String, Pos) {
    let d = conn(m, KEY).runs.detail.as_ref().expect("the run read");
    (d.view.sel.clone(), d.view.pos)
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

/// Opening a long run lands at its end, as a long session opens: its
/// last answer in sight, its first not, following it.
#[test]
fn opening_a_long_run_lands_at_its_end() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
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
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:x".into();
    assert!(runview::open(&mut m, KEY, "x"));
    // Read on as the loop would, till nothing more comes.
    for _ in 0..4 {
        let _ = m.frame();
        super::subs::refresh(&mut m, KEY);
    }
    let all = text(&m.frame());
    assert!(all.contains("answer 1399"), "not at its end:\n{all}");
    assert!(!all.contains("answer 0 "), "at its start:\n{all}");
    assert_eq!(run_view(&mut m).1, Pos::End);
}

/// In a run open whole its rows are a session's: ↑ from the empty box
/// picks the row by it and goes on up, enter opens the row picked, esc
/// lets the pick go; the session's own pick stays the run's row.
#[test]
fn up_and_a_row_expand_work_in_a_runs_view() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = "sub:x".into();
    let _ = m.frame();
    press(&mut m, "enter");
    assert_eq!(runview::open_id(&m, KEY).as_deref(), Some("x"));
    let _ = m.frame();
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
    let was = conn(&mut m, KEY)
        .runs
        .detail
        .as_ref()
        .unwrap()
        .view
        .o
        .open
        .get(&step)
        .copied();
    press(&mut m, "enter");
    let now = conn(&mut m, KEY)
        .runs
        .detail
        .as_ref()
        .unwrap()
        .view
        .o
        .open
        .get(&step)
        .copied();
    assert_ne!(was, now, "enter didn't open the step");
    assert_eq!(sel(&m), "sub:x", "the session's own pick moved");
    press(&mut m, "esc");
    assert_eq!(run_view(&mut m).0, "");
    assert!(
        runview::open_id(&m, KEY).is_some(),
        "esc with a pick closed it"
    );
}

/// ↑↓ move the pick through the list though a click gave the run's box
/// the keys: its box empty, they're the list's. (x runs; y, under it,
/// is finished.)
#[test]
fn arrows_pick_rows_in_the_list_from_the_runs_box() {
    let d = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let mut m = picked_beside(&d, &fake, true);
    press(&mut m, "tab");
    assert!(conn(&mut m, KEY).watch.msg_focus);
    press(&mut m, "down");
    assert_eq!(sel(&m), "sub:y");
    assert!(!conn(&mut m, KEY).watch.msg_focus);
    press(&mut m, "up");
    assert_eq!(sel(&m), "sub:x");
    // Nothing picked, ↓ picks the first row in sight.
    m.session.as_mut().unwrap().sel.clear();
    let _ = m.frame();
    press(&mut m, "down");
    assert!(!sel(&m).is_empty(), "↓ picked nothing");
}

/// Past the newest few, finished runs fold under Earlier, but the one
/// picked; enter on the fold opens it.
#[test]
fn older_finished_runs_fold_under_earlier() {
    let d = tempfile::tempdir().unwrap();
    let mut m = model(&d);
    let more: Vec<Subagent> = (0..10)
        .map(|i| Subagent {
            id: format!("e{i}"),
            typ: "Explore".into(),
            description: format!("look at part {i}"),
            path: format!("/s/subagents/agent-e{i}.jsonl"),
            born: 10 + i as i64,
            ..Subagent::default()
        })
        .collect();
    let r = &mut conn(&mut m, KEY).runs;
    r.subs.extend(more.iter().cloned());
    r.listed.extend(more);
    let list = |m: &mut Model| {
        text(
            &subview::list(m, KEY, 120, false)
                .into_iter()
                .map(|(l, _)| l)
                .collect::<Vec<_>>(),
        )
    };
    let got = list(&mut m);
    assert!(got.contains("Finished  12"), "{got}");
    assert!(got.contains("▸ Earlier · 4 · click to show"), "{got}");
    assert!(got.contains("look at part 9"), "{got}");
    assert!(!got.contains("review the diff"), "{got}");
    // The run picked stays in sight.
    m.session.as_mut().unwrap().sel = "sub:x".into();
    let got = list(&mut m);
    assert!(got.contains("▸ Earlier · 3"), "{got}");
    assert!(got.contains("review the diff"), "{got}");
    views::show_view(&mut m, KEY, "subagents");
    m.session.as_mut().unwrap().sel = subview::EARLIER_REF.into();
    press(&mut m, "enter");
    let got = list(&mut m);
    assert!(got.contains("▾ Earlier · 4"), "{got}");
    assert!(got.contains("review the diff"), "{got}");
}
