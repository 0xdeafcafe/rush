//! A running shell step's output, as it comes: its last lines under it,
//! dim, a fold above them for the rest, and the real result in their
//! place once it ends. Fed the generic way a harness streams it
//! (CallOutput), or as rush reads it off the step's output file
//! (Options::live). UPDATE_SNAPSHOTS=1 writes the snapshots again.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rush_convo::model::LiveOut;
use rush_convo::render::{StepLive, plain};
use rush_convo::{Options, Preset, Renderer, Session};
use rush_core::event::{CallOutput, Event};
use serde_json::json;

mod common;

use common::*;

const CMD: &str = "cargo test --workspace";

/// A turn whose Bash call runs CMD, started at 1s.
fn running() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("run the tests"), at(0));
    s.feed(tool_use("b1", "Bash", json!({"command": CMD})), at(1));
    s
}

fn out(id: &str, text: &str, whole: bool) -> Event {
    Event::CallOutput(CallOutput {
        id: id.into(),
        text: text.into(),
        whole,
    })
}

/// n lines of a test run's output: "test t1 ... ok".
fn lines(from: usize, n: usize) -> String {
    (from..from + n)
        .map(|i| format!("test t{i} ... \x1b[32mok\x1b[0m\n"))
        .collect()
}

fn opts(selected: &str, open: &[&str]) -> Options {
    let mut o = Options {
        width: 100,
        now: at(20),
        preset: Preset::Normal,
        selected: selected.into(),
        ..Options::default()
    };
    for r in open {
        o.open.insert((*r).to_string(), true);
    }
    o
}

fn draw(s: &Session, o: &Options) -> String {
    plain(&Renderer::new().render(s, o))
}

fn snapshot(name: &str, got: &str) {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"));
    // On CI a snapshot not yet written is a failure that shows it.
    assert!(
        p.exists() || std::env::var_os("CI").is_none(),
        "{name} has no snapshot yet; it draws:\n{got}"
    );
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !p.exists() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap();
    assert_eq!(got, want, "{name} differs; UPDATE_SNAPSHOTS=1 to accept");
}

/// The last 8 lines show under the running step, dim and without their
/// colours, and above them how many more there are.
#[test]
fn a_running_step_shows_its_tail() {
    let mut s = running();
    s.feed(out("b1", &lines(1, 12), false), at(5));
    let got = draw(&s, &opts("", &[]));
    snapshot("liveout_running_normal", &got);
    assert!(got.contains("▸ 4 more lines"), "{got}");
    for i in 5..=12 {
        assert!(got.contains(&format!("│ test t{i} ... ok")), "t{i}:\n{got}");
    }
    assert!(!got.contains("test t4 "), "{got}");
    assert!(!got.contains('\x1b'), "{got}");
}

/// What comes next moves the tail on; a harness that sends all of it so
/// far each time replaces what was there.
#[test]
fn the_tail_updates() {
    let mut s = running();
    s.feed(out("b1", &lines(1, 3), false), at(5));
    let got = draw(&s, &opts("", &[]));
    assert!(got.contains("test t3 ... ok"), "{got}");
    assert!(!got.contains("more line"), "nothing folded yet: {got}");
    s.feed(out("b1", &lines(4, 10), false), at(6));
    let got = draw(&s, &opts("", &[]));
    assert!(got.contains("│ test t13 ... ok"), "{got}");
    assert!(got.contains("▸ 5 more lines"), "{got}");
    s.feed(out("b1", "compiling\nlinking\n", true), at(7));
    let got = draw(&s, &opts("", &[]));
    assert!(got.contains("│ linking"), "{got}");
    assert!(!got.contains("test t13"), "whole replaces: {got}");
}

/// Opened, the fold shows the rest, and says how to fold it again.
#[test]
fn the_fold_opens() {
    let mut s = running();
    s.feed(out("b1", &lines(1, 20), false), at(5));
    let got = draw(&s, &opts("", &["t1:s:b1:live"]));
    snapshot("liveout_running_open", &got);
    assert!(got.contains("▾ fewer lines"), "{got}");
    for i in 1..=20 {
        assert!(got.contains(&format!("│ test t{i} ... ok")), "t{i}:\n{got}");
    }
}

/// Once the step ends, its real result is drawn in its place.
#[test]
fn the_finished_step_shows_its_result() {
    let mut s = running();
    s.feed(out("b1", &lines(1, 12), false), at(5));
    s.feed(
        tool_result(
            "b1",
            "test result: ok. 12 passed",
            false,
            Some(json!({"stdout": "test result: ok. 12 passed", "stderr": ""})),
        ),
        at(9),
    );
    let got = draw(&s, &opts("", &["t1:s:b1"]));
    assert!(got.contains("test result: ok. 12 passed"), "{got}");
    assert!(!got.contains("more lines"), "{got}");
    assert!(!got.contains("test t12 ... ok"), "{got}");
    // What comes after the end is let go.
    s.feed(out("b1", "late\n", false), at(10));
    assert!(!draw(&s, &opts("", &["t1:s:b1"])).contains("late"));
}

/// Output rush read itself, from the file the step's shell writes to,
/// is drawn the same; none, nothing new.
#[test]
fn output_read_from_its_file() {
    let s = running();
    let plain_run = draw(&s, &opts("", &[]));
    assert!(!plain_run.contains("test t"), "{plain_run}");
    let mut l = LiveOut::default();
    l.push(&lines(1, 9));
    let mut o = opts("", &[]);
    o.live = HashMap::from([(
        "b1".to_string(),
        StepLive {
            out: Some(Arc::new(l)),
            ..StepLive::default()
        },
    )]);
    let got = draw(&s, &o);
    assert!(got.contains("│ test t9 ... ok"), "{got}");
    assert!(
        got.contains("│ test t1 ... ok") && !got.contains("more line"),
        "one more line shows rather than fold: {got}"
    );
}
