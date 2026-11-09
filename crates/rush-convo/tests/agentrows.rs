//! A subagent Claude Code put in the background (its Agent call returned
//! at once, async_launched): its run drawn as its row (Options::runs) in
//! place of the boilerplate the call returned, or, with no row known, a
//! short note naming the agent. Rust-only.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::plain;
use rush_convo::{Options, Preset, Renderer, Session};
use rush_core::text::{Line, Span};
use serde_json::json;

mod common;

use common::*;

/// What Claude Code's Agent call returns when its agent goes to the
/// background.
const LAUNCHED: &str = "Async agent launched successfully.\nagentId: a99802d28fa3252bb (This is an internal ID for your use, do not mention it to the user. Use SendMessage with to: 'a99802d28fa3252bb' and summary: '<5-10 word recap>' to continue this agent.)\nThe agent is working in the background. You will be notified automatically when it completes.\noutput_file: /tmp/claude/tasks/a99802d28fa3252bb.output\nDo NOT Read or tail this file: you will be notified.";

/// What a foreground agent said back: drawn as ever.
const ANSWER: &str = "Found 3 flaky tests in sync_test.go.";

fn session(result: &str, structured: Option<serde_json::Value>) -> Session {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("find the flaky tests"), at(0));
    s.feed(
        tool_use(
            "toolu_a",
            "Agent",
            json!({"description": "find flaky tests", "prompt": "find the flaky tests", "run_in_background": true}),
        ),
        at(1),
    );
    s.feed(tool_result("toolu_a", result, false, structured), at(2));
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
        o.runs
            .insert((*id).into(), Line::from_spans(vec![Span::plain(*text)]));
    }
    o
}

/// A case: the result, its structured part, the runs' rows, the lines
/// wanted and the text not wanted.
type Case<'a> = (
    &'a str,
    Option<serde_json::Value>,
    Vec<(&'a str, &'a str)>,
    Vec<String>,
    Vec<&'a str>,
);

#[test]
fn a_background_agent_is_drawn_as_its_run() {
    const ROW: &str = "✢ find flaky tests · working…  open ›";
    let launched =
        Some(json!({"isAsync": true, "status": "async_launched", "agentId": "a99802d28fa3252bb"}));
    // (result, structured, rows, wanted, unwanted)
    let cases: [Case; 4] = [
        // No row known yet: a short note with the agent's id.
        (
            LAUNCHED,
            launched.clone(),
            vec![],
            vec!["↳ running in the background · a99802d28fa3252bb".into()],
            vec![
                "output_file",
                "Do NOT Read",
                "5-10 word recap",
                "notified automatically",
            ],
        ),
        // Its id read from the words when the result says nothing.
        (
            LAUNCHED,
            None,
            vec![],
            vec!["↳ running in the background · a99802d28fa3252bb".into()],
            vec!["output_file", "Do NOT Read"],
        ),
        // Its run's row, in place of the note.
        (
            LAUNCHED,
            launched,
            vec![("toolu_a", ROW)],
            vec![format!("↳ {ROW}")],
            vec![
                "output_file",
                "running in the background",
                "Async agent launched",
            ],
        ),
        // An agent that answered is drawn as ever, row or not.
        (
            ANSWER,
            None,
            vec![("toolu_a", ROW)],
            vec![ANSWER.into()],
            vec!["open ›", "running in the background"],
        ),
    ];
    for (i, (result, structured, rows, want, unwanted)) in cases.into_iter().enumerate() {
        let s = session(result, structured);
        let got = plain(&Renderer::new().render(&s, &opts(&rows)));
        for w in &want {
            assert!(got.contains(w.as_str()), "case {i}: want {w:?}:\n{got}");
        }
        for u in unwanted {
            assert!(!got.contains(u), "case {i}: {u:?} drawn:\n{got}");
        }
    }
}

/// As its run moves on, the same renderer draws its row again.
#[test]
fn its_row_is_drawn_again_as_it_changes() {
    let s = session(LAUNCHED, None);
    let mut r = Renderer::new();
    let working = "✢ find flaky tests · working…  open ›";
    let got = plain(&r.render(&s, &opts(&[("toolu_a", working)])));
    assert!(got.contains(working), "{got}");
    let done = "✓ find flaky tests · done  2m  open ›";
    let got = plain(&r.render(&s, &opts(&[("toolu_a", done)])));
    assert!(got.contains(done), "{got}");
    assert!(!got.contains(working), "{got}");
}
