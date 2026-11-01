//! The conversation's other views (Options.preset): what each keeps and
//! leaves out of the same session, and the checks read from commands.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::{check_names, plain};
use rush_convo::{Options, Preset, Renderer, Session};
use serde_json::json;

mod common;

use common::*;

const DECISIONS: &str = "I need one thing settled.\n\nDecisions:\n1. **Ship it:** Whether to ship today. (a) yes; (b) no.\nMy picks: 1b.";

/// A finished turn that reads, searches, builds, edits and fails a check,
/// then one that ends asking a decision.
fn session() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    let evs = vec![
        (0, sent("fix the rounding bug")),
        (1, say("Looking at how totals round.")),
        (
            2,
            tool_use("r1", "Read", json!({"file_path": "/work/app/src/total.rs"})),
        ),
        (3, tool_result("r1", "…", false, None)),
        (
            3,
            tool_use(
                "g1",
                "Grep",
                json!({"pattern": "round", "path": "/work/app/src"}),
            ),
        ),
        (4, tool_result("g1", "src/total.rs", false, None)),
        (
            5,
            tool_use("e1", "Edit", json!({"file_path": "/work/app/src/total.rs"})),
        ),
        (
            6,
            tool_result(
                "e1",
                "ok",
                false,
                Some(
                    json!({"structuredPatch": [{"oldStart": 10, "oldLines": 1, "newStart": 10, "newLines": 2,
                    "lines": [" fn total() {", "-    x.round()", "+    x.round_ties_even()", "+    // banker's"]}]}),
                ),
            ),
        ),
        (
            7,
            tool_use("b1", "Bash", json!({"command": "cargo fmt && cargo build"})),
        ),
        (
            9,
            tool_result("b1", "", false, Some(json!({"stdout": "", "stderr": ""}))),
        ),
        (
            10,
            tool_use(
                "b2",
                "Bash",
                json!({"command": "cargo clippy -- -D warnings"}),
            ),
        ),
        (
            12,
            tool_result(
                "b2",
                "Exit code 101\nerror: unused variable",
                true,
                Some(json!({"stdout": "", "stderr": "error: unused variable"})),
            ),
        ),
        (
            13,
            say(
                "Fixed the rounding: totals now round half to even.\nSecond line.\nThird line.\nFourth line.",
            ),
        ),
        (14, done(0.42)),
        (20, sent("should we ship?")),
        (21, say(DECISIONS)),
        (22, done(0.05)),
    ];
    for (sec, e) in evs {
        s.feed(e, at(sec));
    }
    s
}

fn draw(s: &Session, p: Preset, open: &[&str]) -> String {
    let mut o = Options {
        width: 120,
        now: at(100),
        preset: p,
        ..Options::default()
    };
    for r in open {
        o.open.insert(r.to_string(), true);
    }
    plain(&Renderer::new().render(s, &o))
}

#[test]
fn checks_are_read_from_any_tool() {
    let cases: &[(&str, &[&str])] = &[
        ("cargo test -p rush-convo", &["tests"]),
        (
            "cargo fmt --all && cargo clippy -- -D warnings",
            &["fmt", "clippy"],
        ),
        ("tools/sbtest.sh cargo +nightly test", &["tests"]),
        ("go test ./... && go vet ./...", &["tests", "vet"]),
        ("pnpm run lint", &["lint"]),
        ("npm test", &["tests"]),
        ("npx vitest run", &["tests"]),
        ("uv run pytest -x", &["tests"]),
        ("ruff format . && ruff check .", &["fmt", "lint"]),
        ("make build", &["build"]),
        ("npx tsc --noEmit", &["typecheck"]),
        ("ls -la", &[]),
        ("git commit -m 'test'", &[]),
    ];
    for (cmd, want) in cases {
        assert_eq!(check_names(cmd), *want, "{cmd}");
    }
}

#[test]
fn presets_have_names_and_go_round() {
    for p in Preset::ALL {
        assert_eq!(Preset::from_name(p.name()), p);
    }
    assert_eq!(Preset::from_name(""), Preset::Normal);
    assert_eq!(Preset::from_name("nonsense"), Preset::Normal);
    let mut p = Preset::Normal;
    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(p);
        p = p.next();
    }
    assert_eq!(seen, Preset::ALL);
    assert_eq!(p, Preset::Normal);
}

#[test]
fn normal_is_as_it_always_was() {
    let s = session();
    let o = Options {
        width: 120,
        now: at(100),
        ..Options::default()
    };
    let before = plain(&Renderer::new().render(&s, &o));
    assert_eq!(draw(&s, Preset::Normal, &[]), before);
}

#[test]
fn chat_keeps_what_is_said_and_asked() {
    let s = session();
    let got = draw(&s, Preset::Chat, &[]);
    assert!(got.contains("fix the rounding bug"), "{got}");
    assert!(got.contains("Looking at how totals round."), "{got}");
    assert!(got.contains("Fourth line."), "the answer whole: {got}");
    assert!(got.contains("Ship it"), "the decision: {got}");
    assert!(!got.contains("cargo build"), "no steps: {got}");
    assert!(!got.contains("total.rs"), "no reads or edits: {got}");
    // The five steps between the words are one row, with what failed.
    let row = got
        .lines()
        .find(|l| l.contains("hidden"))
        .expect("a row for what's left out");
    assert!(row.contains("5 steps hidden"), "{row}");
    assert!(row.contains("1 failed"), "{row}");
    // Opened, the steps are there.
    let r = row.split('|').next().unwrap().trim();
    assert!(r.starts_with("t1:hid:"), "{r}");
    let open = draw(&s, Preset::Chat, &[r]);
    assert!(open.contains("hide 5 steps"), "{open}");
    assert!(open.contains("cargo build"), "{open}");
}

#[test]
fn code_shows_edits_as_diffs_and_checks_as_lines() {
    let s = session();
    let got = draw(&s, Preset::Code, &[]);
    assert!(got.contains("x.round_ties_even()"), "the diff: {got}");
    assert!(got.contains("x.round()"), "the old line: {got}");
    let fmt = got
        .lines()
        .find(|l| l.contains("fmt, build"))
        .expect(got.as_str());
    assert!(fmt.contains("✓") && fmt.contains("pass"), "{fmt}");
    let clippy = got
        .lines()
        .find(|l| l.contains("clippy"))
        .expect(got.as_str());
    assert!(
        clippy.contains("✗") && clippy.contains("exit 101"),
        "{clippy}"
    );
    assert!(got.contains("2 steps hidden"), "read and search: {got}");
    assert!(got.contains("Fixed the rounding"), "{got}");
    assert!(!got.contains("Fourth line."), "prose cut short: {got}");
    assert!(got.contains("… 1 more line"), "{got}");
}

#[test]
fn catch_up_is_a_card_a_turn() {
    let s = session();
    let got = draw(&s, Preset::CatchUp, &[]);
    let line = |w: &str| {
        got.lines()
            .filter(|l| l.contains(w))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(line("asked").contains("fix the rounding bug"), "{got}");
    assert!(line("said").contains("Fixed the rounding"), "{got}");
    let changed = line("changed");
    assert!(
        changed.contains("1 file") && changed.contains("+2") && changed.contains("−1"),
        "{changed}"
    );
    let checks = line("checks");
    assert!(
        checks.contains("✓ fmt") && checks.contains("✓ build"),
        "{checks}"
    );
    assert!(checks.contains("✗ clippy"), "{checks}");
    assert!(line("open").contains("1 decision yours to make"), "{got}");
    assert!(got.contains("$0.42"), "its cost: {got}");
    // Each card is its turn's: its top row carries the turn's ref.
    assert!(got.lines().any(|l| l.starts_with("t1 ")), "{got}");
    assert!(got.lines().any(|l| l.starts_with("t2 ")), "{got}");
    // An answered decision isn't open any more.
    let mut s = s;
    s.feed(sent("1b. Ship it: no"), at(30));
    s.feed(done(0.01), at(31));
    let got = draw(&s, Preset::CatchUp, &[]);
    assert!(!got.contains("yours to make"), "{got}");
}

#[test]
fn trail_is_a_line_a_step() {
    let s = session();
    let got = draw(&s, Preset::Trail, &[]);
    let steps: Vec<&str> = got.lines().filter(|l| l.contains(":s:")).collect();
    assert_eq!(steps.len(), 5, "{got}");
    let edit = steps
        .iter()
        .find(|l| l.contains("total.rs") && l.contains("+2"))
        .expect(got.as_str());
    assert!(edit.contains("−1") && edit.contains("✓"), "{edit}");
    let clippy = steps
        .iter()
        .find(|l| l.contains("clippy"))
        .expect(got.as_str());
    assert!(
        clippy.contains("✗") && clippy.contains("exit 101"),
        "{clippy}"
    );
    assert!(
        !got.contains("Looking at how totals round."),
        "no prose: {got}"
    );
    // A row naming each turn.
    assert!(
        got.lines()
            .any(|l| l.starts_with("t2 ") && l.contains("should we ship?")),
        "{got}"
    );
}

/// A long conversation in another view lays out only what's in view.
#[test]
fn another_view_draws_only_a_window() {
    let s = bench_session(300);
    for p in Preset::ALL {
        let o = Options {
            width: 120,
            now: at(100_000),
            preset: p,
            ..Options::default()
        };
        let mut r = Renderer::new();
        let rows = r.tail(&s, &o, 50).len();
        assert!(rows > 0, "{p:?}");
        let (lo, hi) = r.window_turns();
        assert!(hi - lo < 60, "{p:?} drew turns {lo}..{hi}");
    }
}
