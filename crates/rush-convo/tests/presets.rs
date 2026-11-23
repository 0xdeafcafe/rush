//! The conversation's other views (Options.preset): what each keeps and
//! leaves out of the same session, and the checks read from commands.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use rush_convo::render::{check_names, plain};
use rush_convo::{Options, Preset, Renderer, Session};
use rush_core::event::{Event, TaskKind, TaskStarted};
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
/// UPDATE_SNAPSHOTS=1 writes them again. Whether it's the same; one that
/// isn't is printed whole.
fn snapshot(name: &str, got: &str) -> bool {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !p.exists() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return true;
    }
    let want = std::fs::read_to_string(&p).unwrap();
    if got != want {
        eprintln!(
            "{name} differs; UPDATE_SNAPSHOTS=1 to accept\n=== {name} drawn ===\n{got}=== end {name} ==="
        );
    }
    got == want
}

#[test]
fn every_view_at_every_width() {
    let s = session();
    let mut same = true;
    for p in [Preset::Chat, Preset::Code] {
        for w in [80, 120, 200] {
            let got = plain(&Renderer::new().render(&s, &opts(p, w)));
            same &= snapshot(&format!("views_{}_{w}", p.name()), &got);
        }
    }
    assert!(same, "a view differs from its snapshot");
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
        (
            "cd /w; python3 - <<'EOF'\nimport subprocess\nrun('make test')\nEOF\ngo vet ./...",
            &["vet"],
        ),
        ("cat <<EOF > f\ncargo test\nEOF", &[]),
        ("go test ./... 2>&1 | tail -5", &["tests"]),
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
    for _ in 0..Preset::ALL.len() {
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
    // A file's box: its top edge the file's row, its edits in it.
    let top = got
        .lines()
        .find(|l| l.contains("╭─ src/question.rs"))
        .expect(got.as_str());
    assert_eq!(top.split('|').next().unwrap().trim(), "t1:file:1");
    assert!(top.contains("+7 −4"), "{top}");
    assert!(got.contains("fn e() { d() }"), "{got}");
    assert!(got.contains("// w wraps each option"), "{got}");
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

/// A lead: you ask for three lanes; it starts two subagents and Sol
/// through rush; one subagent finishes, Sol reports and the lead says
/// what came of it.
fn lead() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/game".into();
    let evs = vec![
        (0, sent("start the depth, shader and review lanes")),
        (
            1,
            tool_use(
                "a1",
                "Agent",
                json!({"description": "Depth render-to-texture", "prompt": "fix it", "subagent_type": "general-purpose"}),
            ),
        ),
        (
            1,
            Ev::Do(Event::TaskStarted(TaskStarted {
                id: "job1".into(),
                call_id: "a1".into(),
                kind: TaskKind::Subagent,
                label: "Depth render-to-texture".into(),
                background: true,
                ..TaskStarted::default()
            })),
        ),
        (
            2,
            tool_use(
                "a2",
                "Agent",
                json!({"description": "Shaders: z-max clamp", "prompt": "fix it"}),
            ),
        ),
        (
            3,
            tool_use(
                "s1",
                "mcp__rush__spawn_agent",
                json!({"agent": "sol", "prompt": "Review the boot branch for races", "background": true}),
            ),
        ),
        (
            4,
            tool_result(
                "s1",
                "sol started in the background as agent 21bcc40e. Its answer is sent to you as a message when it finishes.",
                false,
                None,
            ),
        ),
        (
            60,
            tool_result("a2", "Clamped z-max; tests pass.", false, None),
        ),
        (61, say("Three lanes are going.")),
        (62, done(0.4)),
        (
            300,
            sent("Codex (agent 21bcc40e) finished:\n\nNo races found. One nit in boot.rs."),
        ),
        (301, say("Sol found no races in boot, only a nit.")),
        (302, done(0.1)),
    ];
    for (sec, e) in evs {
        s.feed(e, at(sec));
    }
    s
}

#[test]
fn orchestrator_is_the_agents_a_lead_hands_work_to() {
    let s = lead();
    let got = draw(&s, Preset::Orchestrator, &[]);
    let row = |w: &str| {
        got.lines()
            .find(|l| l.contains(w))
            .unwrap_or_else(|| panic!("no {w:?} in\n{got}"))
            .to_string()
    };
    // What you said, and what it said back.
    assert!(row("start the depth, shader").contains("You"), "{got}");
    assert!(row("Three lanes are going.").contains("Claude"), "{got}");
    // A report is no message of yours: its turn draws nothing of its own.
    assert!(!got.contains("Codex (agent"), "{got}");
    // The board: one at work, two done; the one at work first.
    assert!(row("Agents").contains("1 working · 2 done"), "{got}");
    assert!(row("Depth render-to-texture").contains("✻"), "{got}");
    assert!(row("Shaders: z-max clamp").contains("✓"), "{got}");
    let sol = row("Review the boot branch");
    assert!(sol.contains("✓") && sol.contains("sol"), "{sol}");
    let rows: Vec<&str> = got.lines().collect();
    let at = |w: &str| rows.iter().position(|l| l.contains(w)).unwrap();
    assert!(at("Depth render") < at("Review the boot"), "{got}");
    // Ended, an agent's own last words are on its row: Sol's report,
    // a subagent's answer.
    assert_eq!(at("done · No races found."), at("Review the boot"), "{got}");
    assert_eq!(at("done · Clamped z-max"), at("Shaders: z-max"), "{got}");
    // One at work with no step known yet says so.
    assert!(row("Depth render-to-texture").contains("working"), "{got}");
}

#[test]
fn orchestrator_without_agents_says_so() {
    let mut s = Session::new();
    s.feed(sent("hello"), at(0));
    s.feed(say("Hi."), at(1));
    s.feed(done(0.0), at(2));
    let got = draw(&s, Preset::Orchestrator, &[]);
    assert!(got.contains("none started yet"), "{got}");
    assert!(got.contains("no agents yet"), "{got}");
}

/// Rust-only: an Agent call another model answered (rush-claude's
/// others.rs) is one lane on the board, run by that model as rush hosts
/// it ("top · astra"), at work as long as it is: never Claude Code's
/// Haiku stand-in that ended at once.
#[test]
fn orchestrator_shows_the_model_that_answered_an_agent_call() {
    let mut s = Session::new();
    s.cwd = "/work/game".into();
    let evs = vec![
        (0, sent("have the top tier design the cache")),
        (
            1,
            tool_use(
                "n1",
                "Agent",
                json!({"description": "Design the cache", "prompt": "design it", "subagent_type": "top"}),
            ),
        ),
        (2, tool_result("n1", "ok", false, None)),
    ];
    for (sec, e) in evs {
        s.feed(e, at(sec));
    }
    for (live, mark) in [(true, "✻"), (false, "✓")] {
        let mut o = opts(Preset::Orchestrator, 120);
        o.kids.insert(
            "n1".into(),
            vec![rush_convo::render::Child {
                id: "h1".into(),
                spawn: rush_convo::render::Spawn {
                    kind: "codex".into(),
                    name: "top · astra".into(),
                    model: "gpt-6-astra".into(),
                    ..Default::default()
                },
                sess: None,
                live,
                start: at(2),
            }],
        );
        let got = plain(&Renderer::new().render(&s, &o));
        let lanes: Vec<&str> = got
            .lines()
            .filter(|l| l.contains("Design the cache"))
            .collect();
        assert_eq!(lanes.len(), 1, "{got}");
        assert!(lanes[0].contains("top · astra"), "{got}");
        assert!(!lanes[0].contains("gpt-6-astra"), "{got}");
        assert!(lanes[0].contains(mark), "{live}: {got}");
    }
}

/// Rust-only: the board is a row an agent, in columns: its model, its
/// newest step while it works, what its own session has cost and how long
/// it has run. Narrower, the cost goes, then the model; never the step.
#[test]
fn orchestrator_rows_say_step_model_cost_and_time() {
    let mut s = Session::new();
    s.cwd = "/work/game".into();
    let evs = vec![
        (0, sent("have sol review boot")),
        (
            1,
            tool_use(
                "s1",
                "mcp__rush__spawn_agent",
                json!({"agent": "sol", "prompt": "Review the boot branch", "background": true}),
            ),
        ),
        (
            2,
            tool_result(
                "s1",
                "sol started in the background as agent 21bcc40e.",
                false,
                None,
            ),
        ),
    ];
    for (sec, e) in evs {
        s.feed(e, at(sec));
    }
    // The agent's own session: a search, then a read, and its cost.
    let mut kid = Session::new();
    kid.cwd = "/work/game".into();
    let evs = vec![
        (3, sent("Review the boot branch")),
        (
            4,
            tool_use("k1", "Bash", json!({"command": "rg -n ask_guest crates"})),
        ),
        (5, tool_result("k1", "crates/x.rs:1", false, None)),
        (
            6,
            tool_use("k2", "Read", json!({"file_path": "/work/game/src/boot.rs"})),
        ),
        (7, tool_result("k2", "fn boot() {}", false, None)),
        (8, say("Looked.")),
        (9, done(1.25)),
    ];
    for (sec, e) in evs {
        kid.feed(e, at(sec));
    }
    let kid = std::sync::Arc::new(kid);
    // (width, in the row, not in it)
    let cases: [(usize, &[&str], &[&str]); 2] = [
        (
            120,
            &["✻", "gpt-6.2-sol", "Read boot.rs", "$1.25", "29m"],
            &["working"],
        ),
        (40, &["✻", "Read boot.rs", "29m"], &["$1.25", "gpt-6.2-sol"]),
    ];
    for (w, has, hasnt) in cases {
        let mut o = opts(Preset::Orchestrator, w);
        o.kids.insert(
            "s1".into(),
            vec![rush_convo::render::Child {
                id: "21bcc40e".into(),
                spawn: rush_convo::render::Spawn {
                    kind: "codex".into(),
                    name: "sol".into(),
                    model: "gpt-6.2-sol".into(),
                    ..Default::default()
                },
                sess: Some(kid.clone()),
                live: true,
                start: at(2),
            }],
        );
        let got = plain(&Renderer::new().render(&s, &o));
        let row = got
            .lines()
            .find(|l| l.contains("Review the"))
            .unwrap_or_else(|| panic!("{w}: no row in\n{got}"));
        for want in has {
            assert!(row.contains(*want), "{w}: no {want:?} in {row:?}\n{got}");
        }
        for not in hasnt {
            assert!(!row.contains(*not), "{w}: {not:?} in {row:?}\n{got}");
        }
        if w == 120 {
            let head = got.lines().find(|l| l.contains("Agents")).unwrap_or("");
            assert!(head.contains("1 working · $1.25"), "{got}");
        }
    }
}
