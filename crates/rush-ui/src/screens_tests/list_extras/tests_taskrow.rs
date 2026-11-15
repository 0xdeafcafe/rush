//! A background task's rows, as the dock and the conversation draw them
//! (taskrow.rs, Rust-only): a rush watch running, holding a failure,
//! passed and failed; a cargo build and tests with progress; a plain
//! command; each at 80 columns and narrower. Checked against
//! tests/snapshots/taskrow_<name>.txt; UPDATE_SNAPSHOTS=1 writes them.

use rush_bundled::prwatch::{Live, LiveJob, card_of};
use rush_convo::{Job, Session};
use rush_core::event::{Event, TaskKind, TaskStarted};
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_state::Time;

use super::jobs::*;
use super::taskrow::*;
use super::tests::{MIN, NOW};

const SEC: Ms = 1000;

/// The widths each is drawn at: the dock at 80, a column, and narrow.
const WIDTHS: [usize; 3] = [80, 56, 40];

fn t(ms: Ms) -> Time {
    Time::from_ms(ms)
}

/// got checked against tests/snapshots/taskrow_<name>.txt;
/// UPDATE_SNAPSHOTS=1 writes it. A missing one fails, printed whole, so
/// CI shows it.
fn snapshot(name: &str, got: &str) {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("taskrow_{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap_or_default();
    assert!(
        got == want,
        "taskrow_{name} differs; UPDATE_SNAPSHOTS=1 to accept\n=== taskrow_{name} drawn ===\n{got}=== end taskrow_{name} ==="
    );
}

/// rows drawn at each of WIDTHS, under a line saying which.
fn at_widths(rows: impl Fn(usize) -> Vec<Line>) -> String {
    let mut out = String::new();
    for w in WIDTHS {
        out += &format!("== {w} wide\n");
        for r in rows(w) {
            assert!(
                r.width() <= w,
                "{} cells at {w}: {:?}",
                r.width(),
                r.plain()
            );
            out += &format!("{}\n", r.plain().trim_end());
        }
    }
    out
}

fn job(name: &str, state: &str, started: Ms, ended: Ms, note: &str) -> LiveJob {
    LiveJob {
        name: name.into(),
        state: state.into(),
        started: if started == 0 {
            Time::default()
        } else {
            t(started)
        },
        ended: if ended == 0 {
            Time::default()
        } else {
            t(ended)
        },
        note: note.into(),
    }
}

/// A release run watch 6m in, its workflow usually 9m: one build done,
/// one building, publishing still to come.
fn release() -> Live {
    Live {
        task: "bw1".into(),
        title: "Release".into(),
        what: "v0.1.0-beta.18 @ 2683631".into(),
        start: t(NOW - 6 * MIN),
        asked: t(NOW - 10 * SEC),
        next: t(NOW + 5 * SEC),
        expect: t(NOW + 3 * MIN),
        passed: 1,
        pending: 2,
        jobs: vec![
            job(
                "build x86_64-unknown-linux-gnu",
                "passed",
                NOW - 6 * MIN,
                NOW - 6 * MIN + 252 * SEC,
                "",
            ),
            job(
                "build aarch64-apple-darwin",
                "pending",
                NOW - 6 * MIN + 2 * SEC,
                0,
                "",
            ),
            job("publish", "pending", 0, 0, ""),
        ],
        ..Default::default()
    }
}

/// A CI watch holding a failure until its last job finishes.
fn ci_failing() -> Live {
    Live {
        task: "bw2".into(),
        title: "CI".into(),
        what: "rush/beside-asks @ 8379d25".into(),
        start: t(NOW - 7 * MIN),
        asked: t(NOW - 5 * SEC),
        passed: 1,
        failed: 1,
        pending: 1,
        failed_checks: vec!["check".into()],
        running: vec!["linux".into()],
        jobs: vec![
            job("fmt", "passed", NOW - 7 * MIN, NOW - 6 * MIN, ""),
            job(
                "check",
                "failed",
                NOW - 7 * MIN,
                NOW - MIN,
                "test render::presets::orchestrator ... FAILED",
            ),
            job("linux", "pending", NOW - 7 * MIN, 0, ""),
        ],
        ..Default::default()
    }
}

fn watch(l: &Live) -> String {
    let c = card_of(l, t(NOW));
    at_widths(|w| watch_rows(&c, " ", 3, NOW, w))
}

#[test]
fn snapshot_a_watch_running() {
    snapshot("watch_running", &watch(&release()));
}

#[test]
fn snapshot_a_watch_holding_a_failure() {
    snapshot("watch_failing", &watch(&ci_failing()));
}

/// Ended, a watch is one row: how it went, how long it took, how long ago.
#[test]
fn snapshot_a_watch_ended() {
    let mut passed = release();
    passed.title = "CI".into();
    passed.what = "rush/guests @ 714c3c5".into();
    passed.ended = t(NOW - 2 * MIN);
    passed.outcome = "passed".into();
    for j in passed.jobs.iter_mut() {
        j.state = "passed".into();
    }
    let mut failed = ci_failing();
    failed.ended = t(NOW - 30 * SEC);
    failed.outcome = "failed".into();
    let got = format!("-- passed\n{}-- failed\n{}", watch(&passed), watch(&failed));
    snapshot("watch_ended", &got);
}

/// More checks than two rows hold: those that passed are one chip, and
/// what's left says how many more.
#[test]
fn snapshot_a_watch_with_many_checks() {
    let mut l = release();
    l.title = "CI".into();
    l.jobs = (0..14)
        .map(|i| {
            let state = if i < 9 { "passed" } else { "pending" };
            let end = if i < 9 { NOW - MIN } else { 0 };
            job(&format!("test shard {i}"), state, NOW - 5 * MIN, end, "")
        })
        .collect();
    (l.passed, l.pending) = (9, 5);
    snapshot("watch_many", &watch(&l));
}

fn shell(cmd: &str, out: &[&str], ran: Ms) -> String {
    let lines: Vec<String> = out.iter().map(|l| l.to_string()).collect();
    at_widths(|w| {
        shell_rows(
            &ShellTask {
                cmd,
                lines: &lines,
                start: NOW - ran,
                now: NOW,
                mark: rush_core::text::Span::plain("✻"),
                used: Vec::new(),
            },
            " ",
            w,
        )
    })
}

#[test]
fn snapshot_a_cargo_build_with_progress() {
    let got = shell(
        "cd ~/src/rush && cargo build --release 2>&1 | tail -40",
        &[
            "   Compiling rush-hl v0.1.0 (/Users/me/src/rush/crates/rush-hl)",
            "   Compiling rush-convo v0.1.0 (/Users/me/src/rush/crates/rush-convo)",
            "    Building [=======================>   ] 212/430: rush-convo, rush-ui-base",
        ],
        64 * SEC,
    );
    snapshot("cargo_build", &got);
}

#[test]
fn snapshot_cargo_tests_with_progress() {
    let got = shell(
        "cargo test -p rush-convo",
        &[
            "     Running tests/presets.rs (target/debug/deps/presets-1a2b)",
            "",
            "running 8 tests",
            "test render::presets::chat ... ok",
            "test render::presets::code ... ok",
            "test render::presets::orchestrator ... ok",
        ],
        64 * SEC,
    );
    snapshot("cargo_test", &got);
}

#[test]
fn snapshot_a_plain_command() {
    let got = shell(
        "npm run dev -- --port 3000",
        &[
            "> app@1.0.0 dev",
            "  ready in 412 ms",
            "listening on :3000",
            "",
        ],
        12 * MIN,
    );
    snapshot("plain", &got);
}

/// The dock's block of two background commands at 80 and narrower.
#[test]
fn snapshot_the_docks_tasks() {
    let mut s = Session::new();
    for (id, label) in [("b1", "cargo test -p rush-convo"), ("b2", "npm run dev")] {
        s.apply(
            &Event::TaskStarted(TaskStarted {
                id: id.into(),
                call_id: format!("t-{id}"),
                kind: TaskKind::Shell,
                label: label.into(),
                background: true,
                ..TaskStarted::default()
            }),
            NOW - 64 * SEC,
        );
    }
    let jobs = dock_jobs(&s);
    let tests = Tailed {
        size: 10,
        mod_at: NOW,
        lines: vec![
            "running 4 tests".into(),
            "test a ... ok".into(),
            "test b ... ok".into(),
        ],
        ..Tailed::default()
    };
    let dev = Tailed {
        size: 10,
        mod_at: NOW,
        lines: vec!["listening on :3000".into()],
        ..Tailed::default()
    };
    let out = |j: &Job| Some(if j.id == "b1" { &tests } else { &dev });
    let d = Dock {
        sel: "",
        focus: true,
        tick: 2,
        now: NOW,
        out: &out,
        who: &nobody,
        flow: &no_flow,
        usage: &no_usage,
    };
    let got = at_widths(|w| jobs_preview(&s, &jobs, w, &d, Line::new()).0);
    snapshot("dock", &got);
}

/// Table: a clock as a task row says it.
#[test]
fn clocks() {
    for (ms, want) in [
        (0, "0s"),
        (42 * SEC, "42s"),
        (64 * SEC, "1m04s"),
        (12 * MIN, "12m"),
        (65 * MIN, "1h05m"),
    ] {
        assert_eq!(clock(ms), want, "{ms}");
    }
}

/// Table: a command as a task row names it.
#[test]
fn short_commands() {
    for (cmd, want) in [
        ("cargo test", "cargo test"),
        ("cd /tmp && make 2>&1 | tail -20", "make"),
        ("pnpm build > /dev/null", "pnpm build"),
        ("go test ./... 2>&1 | tee out.log", "go test ./..."),
    ] {
        assert_eq!(short_command(cmd), want, "{cmd}");
    }
}

/// Table: how far any command's output says it has got.
#[test]
fn progress_of_any_command() {
    let cases: [(&str, &[&str], Option<(usize, usize)>); 6] = [
        ("make", &["building", "[ 45%] Linking"], Some((450, 1000))),
        ("rsync -a", &["file 12/30"], Some((12, 30))),
        ("npm test", &["GET / 200"], None),
        ("date", &["2026/11/15"], None),
        (
            "cargo test",
            &["running 3 tests", "test a ... ok", "test b ... FAILED"],
            Some((2, 3)),
        ),
        (
            "cargo build",
            &["    Building [==>  ] 5/20: x"],
            Some((5, 20)),
        ),
    ];
    for (cmd, lines, want) in cases {
        let got = rush_convo::render::progress_any(cmd, lines).and_then(|p| p.done);
        assert_eq!(got, want, "{cmd} {lines:?}");
    }
}
