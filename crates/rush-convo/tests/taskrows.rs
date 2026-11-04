//! A background task rush knows more of (a rush watch, Options::tasks)
//! drawn in the conversation in place of its call's "running in the
//! background" note, as it stands now, and drawn again as it changes.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::plain;
use rush_convo::{Options, Preset, Renderer, Session};
use rush_core::event::{Event, TaskKind, TaskStarted};
use rush_core::text::{Line, Span};
use serde_json::json;

mod common;

use common::*;

const CMD: &str = "rush watch run main --until done";
const BG: &str = "Command running in background with ID: bx1. Output is being written to: /tmp/s/tasks/bx1.output. You will be notified when it completes.";

/// A turn whose Bash call put a rush watch in the background, still
/// running.
fn watching() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("push and wait for CI"), at(0));
    s.feed(
        tool_use(
            "toolu_w",
            "Bash",
            json!({"command": CMD, "run_in_background": true}),
        ),
        at(1),
    );
    s.feed(tool_result("toolu_w", BG, false, None), at(2));
    s.apply(
        &Event::TaskStarted(TaskStarted {
            id: "bx1".into(),
            call_id: "toolu_w".into(),
            kind: TaskKind::Shell,
            label: CMD.into(),
            background: true,
            ..TaskStarted::default()
        }),
        at(2),
    );
    s
}

fn opts(rows: &[(&str, &str)]) -> Options {
    let mut o = Options {
        width: 110,
        now: at(30),
        preset: Preset::Normal,
        verbose: true,
        ..Options::default()
    };
    for (id, text) in rows {
        o.tasks
            .insert((*id).into(), Line::from_spans(vec![Span::plain(*text)]));
    }
    o
}

#[test]
fn a_watch_is_drawn_as_it_stands() {
    let s = watching();
    let mut r = Renderer::new();
    let got = plain(&r.render(&s, &opts(&[])));
    assert!(got.contains("running in the background · bx1"), "{got}");

    let row = "◷ watch main @ abc1234 CI  ━━━━── ✓4 ◌2  waiting on test  ~3m left · 1m";
    let got = plain(&r.render(&s, &opts(&[("bx1", row)])));
    assert!(got.contains(&format!("↳ {row}")), "{got}");
    assert!(!got.contains("running in the background"), "{got}");
    assert!(
        !got.contains("bx1.output"),
        "the file goes with the note: {got}"
    );

    // A row for another task changes nothing here.
    let got = plain(&r.render(&s, &opts(&[("other", "◷ watch #7")])));
    assert!(got.contains("running in the background · bx1"), "{got}");

    // As it moves on, the same renderer draws it again.
    let later = "◷ watch main @ abc1234 CI  ━━━━━━ ✓6  all finished  2m";
    let got = plain(&r.render(&s, &opts(&[("bx1", later)])));
    assert!(got.contains(later), "{got}");
}
