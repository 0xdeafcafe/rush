//! The conversation's other views (Options.preset): what each keeps and
//! leaves out of the same session, and the checks read from commands.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::{check_names, plain};
use rush_convo::{Options, Preset, Renderer, Session};
use serde_json::json;

mod common;

use common::*;

const DECISIONS: &str = "Merged to main as f3760931. One decision for you.\n\nDecisions:\n1. **Read from the end:** When the transcript is read from its end. (a) now; (b) after the release.\nMy picks: 1a.";

/// A realistic session: you send a screenshot of a bug; the agent reads
/// and searches, edits two files, fails clippy, passes the tests on a
/// slow run, edits again and answers; then you say merge it, and it
/// merges and ends asking a decision.
fn session() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/rush".into();
    let read =
        |id: &str, f: &str| tool_use(id, "Read", json!({"file_path": format!("/work/rush/{f}")}));
    let patch = |old: i64, lines: serde_json::Value| {
        Some(
            json!({"structuredPatch": [{"oldStart": old, "oldLines": 3, "newStart": old, "newLines": 4, "lines": lines}]}),
        )
    };
    let evs = vec![
        (
            0,
            sent_images(
                "the decision card still overlaps on long options",
                &["/tmp/Screenshot 2026-09-23 at 22.13.55.png"],
            ),
        ),
        (2, say("Looking at how the card lays out its options.")),
        (3, read("r1", "src/dock.rs")),
        (4, tool_result("r1", "…", false, None)),
        (4, read("r2", "src/question.rs")),
        (5, tool_result("r2", "…", false, None)),
        (5, read("r3", "src/side.rs")),
        (6, tool_result("r3", "…", false, None)),
        (6, read("r4", "src/views.rs")),
        (7, tool_result("r4", "…", false, None)),
        (
            8,
            tool_use(
                "g1",
                "Grep",
                json!({"pattern": "label_rows", "path": "/work/rush/src"}),
            ),
        ),
        (10, tool_result("g1", "src/question.rs", false, None)),
        (
            12,
            tool_use("e1", "Edit", json!({"file_path": "/work/rush/src/dock.rs"})),
        ),
        (
            20,
            tool_result(
                "e1",
                "ok",
                false,
                patch(
                    212,
                    json!([
                        "     let room = pane.width",
                        "-        ;",
                        "+        .saturating_sub(side.width)",
                        "+        .saturating_sub(views.width);",
                        " "
                    ]),
                ),
            ),
        ),
        (
            21,
            tool_use(
                "e2",
                "Edit",
                json!({"file_path": "/work/rush/src/question.rs"}),
            ),
        ),
        (
            52,
            tool_result(
                "e2",
                "ok",
                false,
                patch(
                    40,
                    json!([
                        " fn label_rows(opts: &[Opt], w: usize) -> Vec<Row> {",
                        "-    opts.iter().map(|o| Row::one(&o.label)).collect()",
                        "+    opts.iter()",
                        "+        .flat_map(|o| wrap(&o.label, w))",
                        "+        .map(Row::one)",
                        "+        .collect()",
                        " }",
                        " fn a() {}",
                        " fn b() {}",
                        "-fn c() {}",
                        "+fn c() { b() }",
                        " fn d() {}",
                        "-fn e() {}",
                        "+fn e() { d() }"
                    ]),
                ),
            ),
        ),
        (
            60,
            tool_use(
                "b1",
                "Bash",
                json!({"command": "cargo clippy -p rush-ui-session-cards -- -D warnings"}),
            ),
        ),
        (
            310,
            tool_result(
                "b1",
                "Exit code 101\nerror: unused variable: `w`",
                true,
                Some(json!({"stdout": "", "stderr": "error: unused variable: `w`"})),
            ),
        ),
        (
            311,
            tool_use(
                "e3",
                "Edit",
                json!({"file_path": "/work/rush/src/question.rs"}),
            ),
        ),
        (
            316,
            tool_result(
                "e3",
                "ok",
                false,
                patch(
                    41,
                    json!([
                        "-    opts.iter()",
                        "+    opts.iter() // w wraps each option",
                        " "
                    ]),
                ),
            ),
        ),
        (
            320,
            tool_use(
                "b2",
                "Bash",
                json!({"command": "cargo test -p rush-ui-session-cards"}),
            ),
        ),
        (
            980,
            tool_result(
                "b2",
                "running 108 tests\ntest result: ok. 108 passed; 0 failed; 0 ignored",
                false,
                Some(
                    json!({"stdout": "running 108 tests\ntest result: ok. 108 passed; 0 failed; 0 ignored", "stderr": ""}),
                ),
            ),
        ),
        (
            1020,
            say(
                "Found it: the card was laid out at the full width, but the views beside it take room. Fixed, the options wrap now.\n\n- `dock.rs` takes the views' width off\n- `question.rs` wraps each option",
            ),
        ),
        (1021, done(1.20)),
        (1560, sent("nice, merge it")),
        (
            1562,
            tool_use(
                "b3",
                "Bash",
                json!({"command": "git merge --ff-only rush/card-wrap"}),
            ),
        ),
        (
            1564,
            tool_result(
                "b3",
                "Fast-forward",
                false,
                Some(json!({"stdout": "Fast-forward", "stderr": ""})),
            ),
        ),
        (1570, say(DECISIONS)),
        (1571, done(0.31)),
    ];
    for (sec, e) in evs {
        s.feed(e, at(sec));
    }
    s
}

fn opts(p: Preset, w: usize) -> Options {
    Options {
        width: w,
        now: at(1800),
        zone: ZONE,
        speaker: "Claude".into(),
        preset: p,
        ..Options::default()
    }
}

fn draw(s: &Session, p: Preset, open: &[&str]) -> String {
    let mut o = opts(p, 120);
    for r in open {
        o.open.insert(r.to_string(), true);
    }
    plain(&Renderer::new().render(s, &o))
}

/// Each view's rows checked against tests/snapshots/views_<view>_<w>.txt;
/// UPDATE_SNAPSHOTS=1 writes them again.
fn snapshot(name: &str, got: &str) {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !p.exists() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap();
    assert_eq!(got, want, "{name} differs; UPDATE_SNAPSHOTS=1 to accept");
}

#[test]
fn every_view_at_every_width() {
    let s = session();
    for p in [Preset::Chat, Preset::Code, Preset::CatchUp, Preset::Trail] {
        for w in [80, 120, 200] {
            let got = plain(&Renderer::new().render(&s, &opts(p, w)));
            snapshot(&format!("views_{}_{w}", p.name()), &got);
        }
    }
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
        now: at(1800),
        zone: ZONE,
        speaker: "Claude".into(),
        since: at(1100),
        ..Options::default()
    };
    let before = plain(&Renderer::new().render(&s, &o));
    assert_eq!(draw(&s, Preset::Normal, &[]), before);
}

#[test]
fn chat_is_bubbles_of_words() {
    let s = session();
    let got = draw(&s, Preset::Chat, &[]);
    assert!(got.contains("the decision card still overlaps"), "{got}");
    assert!(
        got.contains("▣ screenshot 22.13.55"),
        "the image a chip: {got}"
    );
    assert!(got.contains("Looking at how the card lays out"), "{got}");
    assert!(got.contains("Found it"), "{got}");
    assert!(
        got.contains("? 1 decision for you"),
        "the decision a line: {got}"
    );
    assert!(!got.contains("cargo test"), "no steps: {got}");
    assert!(!got.contains("saturating_sub"), "no diffs: {got}");
    // Yours on the right, the agent's on the left.
    let you = got.lines().find(|l| l.contains("nice, merge it")).unwrap();
    let col = you.split_once('|').unwrap().1.find('│').unwrap();
    assert!(col > 40, "{you}");
    let agent = got.lines().find(|l| l.contains("Found it")).unwrap();
    assert!(
        agent.split_once('|').unwrap().1.starts_with(" │"),
        "{agent}"
    );
    // The foot names the speaker and the steps, and opens the turn.
    let foot = got
        .lines()
        .find(|l| l.contains("Claude") && l.contains("10 steps"))
        .expect(got.as_str());
    assert!(foot.starts_with("t1 "), "{foot}");
    assert!(foot.contains("2 files"), "{foot}");
}

#[test]
fn code_shows_each_turns_diffs_and_checks() {
    let s = session();
    let got = draw(&s, Preset::Code, &[]);
    let head = got.lines().find(|l| l.contains("turn 1")).unwrap();
    assert!(
        head.contains("2 files") && head.contains("+9") && head.contains("−5"),
        "{head}"
    );
    assert!(
        got.contains(".saturating_sub(views.width);"),
        "the diff: {got}"
    );
    assert!(got.contains("src/question.rs"), "{got}");
    // A long diff is cut, and opens whole.
    let cap = got
        .lines()
        .find(|l| l.contains("enter opens the whole diff"))
        .expect(got.as_str());
    let r = cap.split('|').next().unwrap().trim();
    assert_eq!(r, "t1:file:1");
    assert!(!got.contains("fn e() { d() }"), "{got}");
    let open = draw(&s, Preset::Code, &[r]);
    assert!(open.contains("fn e() { d() }"), "{open}");
    // A check a line: how it came out, what it said, how long it took.
    let clippy = got.lines().find(|l| l.contains("cargo clippy")).unwrap();
    assert!(
        clippy.contains("✗") && clippy.contains("exit 101"),
        "{clippy}"
    );
    let tests = got.lines().find(|l| l.contains("cargo test")).unwrap();
    assert!(
        tests.contains("✓") && tests.contains("108 passed"),
        "{tests}"
    );
    assert!(tests.contains("11m 00s"), "{tests}");
    // Nothing else: no prose, no reads, no plain commands.
    assert!(!got.contains("Found it"), "{got}");
    assert!(!got.contains("git merge"), "{got}");
    assert!(
        got.lines()
            .any(|l| l.contains("turn 2") && l.contains("no code changes")),
        "{got}"
    );
}

#[test]
fn catch_up_is_one_card_since_you_looked() {
    let s = session();
    // Never looked: the last turns.
    let got = draw(&s, Preset::CatchUp, &[]);
    assert!(got.contains("The last 2 turns"), "{got}");
    let card = |got: &str, w: &str| {
        got.lines()
            .filter(|l| l.contains(w))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let needs = card(&got, "? needs you");
    assert!(
        needs.contains("Read from the end") && needs.contains("[answer]"),
        "{got}"
    );
    assert!(needs.starts_with("t2 "), "{needs}");
    // Needs you comes first.
    let rows: Vec<&str> = got.lines().collect();
    let row_of = |w: &str| rows.iter().position(|l| l.contains(w)).unwrap();
    assert!(row_of("? needs you") < row_of("Done"), "{got}");
    assert!(card(&got, "Done").contains("Found it:"), "{got}");
    assert!(got.contains("f3760931."), "{got}");
    let changed = card(&got, "Changed");
    assert!(
        changed.contains("2 files") && changed.contains("+9") && changed.contains("−5"),
        "{changed}"
    );
    let checks = card(&got, "Checks");
    assert!(checks.contains("✓ tests 108"), "{checks}");
    assert!(checks.contains("✗ clippy"), "{checks}");
    assert!(
        card(&got, "Said to you").contains("One decision for you."),
        "{got}"
    );
    assert!(got.contains("$1.51"), "{got}");
    // Looked after the first turn: only the second is in the card, the
    // first a row before it.
    let mut o = opts(Preset::CatchUp, 120);
    o.since = at(1100);
    let got = plain(&Renderer::new().render(&s, &o));
    assert!(
        got.contains("Since you last looked · 11m 40s ago · 1 turn"),
        "{got}"
    );
    assert!(
        got.lines()
            .any(|l| l.starts_with("t1 ") && l.contains("turn 1")),
        "{got}"
    );
    assert!(!card(&got, "Changed").contains("files"), "{got}");
    // Nothing since: says so.
    o.since = at(1700);
    let got = plain(&Renderer::new().render(&s, &o));
    assert!(got.contains("Nothing new since you last looked"), "{got}");
    // An answered decision doesn't need you.
    let mut s = s;
    s.feed(sent("1a"), at(1750));
    s.feed(done(0.01), at(1751));
    let got = draw(&s, Preset::CatchUp, &[]);
    assert!(!got.contains("needs you"), "{got}");
}

#[test]
fn trail_is_a_line_a_step_on_a_time_axis() {
    let s = session();
    let got = draw(&s, Preset::Trail, &[]);
    // Quick reads of a kind are one row.
    let reads = got.lines().find(|l| l.contains("read")).unwrap();
    assert!(reads.contains("4 files"), "{reads}");
    assert!(reads.starts_with("t1:s:r1 "), "{reads}");
    // The slowest step is marked, its bar the longest.
    let slow = got
        .lines()
        .find(|l| l.contains("← slow"))
        .expect(got.as_str());
    assert!(slow.contains("cargo test"), "{slow}");
    assert!(slow.contains("████"), "{slow}");
    let clippy = got.lines().find(|l| l.contains("cargo clippy")).unwrap();
    assert!(
        clippy.contains("✗") && clippy.contains("4m 10s"),
        "{clippy}"
    );
    assert!(got.contains("reply"), "{got}");
    assert!(got.contains("\"Found it: the card was laid out at the full width, but the views beside it take room.\""), "{got}");
    assert!(
        got.lines()
            .any(|l| l.contains("You") && l.contains("the decision card")),
        "{got}"
    );
    let totals = got.lines().find(|l| l.contains("thinking")).unwrap();
    assert!(
        totals.contains("turn 17m 01s") && totals.contains("waiting on you 0"),
        "{totals}"
    );
    assert!(!got.contains("Looking at how"), "no prose: {got}");
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
