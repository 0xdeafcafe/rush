//! Emeritus in the conversation (docs/emeritus.md): rush's nudge to hand
//! over, the retire call and its baton, a handover rolled back, a retire
//! you called off by writing, and a successor's start, each drawn as
//! rush's own rather than as your words or a raw tool call.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::emeritus::{CALLED_OFF, NUDGE_START, ROLLED_BACK_START};
use rush_convo::{Options, Renderer, Session};
use serde_json::json;

mod common;
use common::*;

const BATON: &str = "## Destination\nCI green on rush/fix-flaky with the flaky test fixed for good.\n## Core ideas\n- The race is in the watcher.\n## Methods\ncargo test -p rush-fleet\n## Journey\nA sleep hid it.\n## Limits\n❌ never a sleep in a test\n## Tests that pin text\nnone\n## Key files\n- crates/rush-fleet/src/watch.rs: the watcher\n## Next action\nPush and watch CI.";

/// The rows as drawn, without refs.
fn rows(s: &Session, o: &Options) -> String {
    Renderer::new()
        .render(s, o)
        .iter()
        .map(|l| format!("{}\n", l.line.plain().trim_end()))
        .collect()
}

/// got checked against tests/snapshots/emeritus_<name>.txt;
/// UPDATE_SNAPSHOTS=1 writes it. A missing one fails, printed whole, so CI
/// shows it.
fn snapshot(name: &str, got: &str) {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("emeritus_{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap_or_default();
    assert!(
        got == want,
        "emeritus_{name} differs; UPDATE_SNAPSHOTS=1 to accept\n=== drawn ===\n{got}=== end ==="
    );
}

fn opts(now: i64) -> Options {
    Options {
        width: 100,
        now: at(now),
        kind: "claude".into(),
        zone: ZONE,
        ..Options::default()
    }
}

/// A session rush nudged, that wrote its baton and asked to retire.
fn retiring() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    s.feed(sent("fix the flaky test"), at(0));
    s.feed(say("Fixed the watcher's race."), at(10));
    s.feed(done(1.2), at(11));
    s.feed(
        sent(&format!(
            "{NUDGE_START}312k tokens, past the 300k where each call's re-read of it costs more than a fresh start. Hand the task over."
        )),
        at(20),
    );
    s.feed(
        tool_use("r1", "mcp__rush__retire", json!({"baton": BATON})),
        at(30),
    );
    s.feed(
        tool_result(
            "r1",
            "Retiring after this turn. End your turn now.",
            false,
            None,
        ),
        at(31),
    );
    s.feed(say("Handing over."), at(32));
    s.feed(done(0.2), at(33));
    s
}

// The nudge is rush's line, not your message; the retire call is a row
// saying it hands over, by where the task is going.
#[test]
fn the_nudge_and_the_retire() {
    let s = retiring();
    let got = rows(&s, &opts(40));
    assert!(
        !got.contains(NUDGE_START),
        "the nudge drawn as yours:\n{got}"
    );
    assert!(!got.contains("mcp__rush__retire"), "a raw call:\n{got}");
    snapshot("retiring", &got);
}

// Opened, the retire call shows its baton as the Markdown it was written in.
#[test]
fn the_baton_opened() {
    let s = retiring();
    let mut o = opts(40);
    o.open.insert("t2:s:r1".into(), true);
    let got = rows(&s, &o);
    assert!(got.contains("Next action"), "{got}");
    snapshot("baton_opened", &got);
}

// A handover rolled back, and a retire you called off by writing: rush's
// lines, your words still yours.
#[test]
fn rolled_back_and_called_off() {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    s.feed(
        sent(&format!(
            "{ROLLED_BACK_START}hit a usage limit. You aren't retired: carry on with the task yourself."
        )),
        at(0),
    );
    s.feed(say("Carrying on."), at(5));
    s.feed(done(0.1), at(6));
    s.feed(
        sent(&format!("{CALLED_OFF}\n\nwait, one more thing first")),
        at(10),
    );
    s.feed(say("Sure."), at(12));
    s.feed(done(0.1), at(13));
    let got = rows(&s, &opts(20));
    assert!(!got.contains("rush (Emeritus)"), "{got}");
    assert!(got.contains("wait, one more thing first"), "{got}");
    snapshot("rolled_back_called_off", &got);
}

// A successor's first turn says whom it took over from, its baton folded
// under it; opened, the baton is drawn as Markdown.
#[test]
fn a_successor_starts() {
    let prompt = format!(
        "You take over this task from \"fix the flaky test\" (rush session a1b2c3), an agent that handed it over near the end of its context and has retired. Its baton follows.\n\n{BATON}"
    );
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    s.feed(sent(&prompt), at(0));
    s.feed(say("Picking it up from the baton."), at(5));
    s.feed(done(0.3), at(6));
    let folded = rows(&s, &opts(10));
    assert!(folded.contains("its baton"), "{folded}");
    assert!(!folded.contains("You take over"), "{folded}");
    let mut o = opts(10);
    o.open.insert("t1:baton".into(), true);
    let opened = rows(&s, &o);
    snapshot("successor", &format!("{folded}---- opened ----\n{opened}"));
}
