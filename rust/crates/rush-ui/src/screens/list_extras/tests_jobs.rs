//! A session's tasks in its dock, after Go's internal/ui jobs_test.go
//! (TestJobDock, TestJobCrashed, TestTailFollows) and
//! job_activity_test.go.

use rush_convo::{Job, Session};
use rush_core::event::{Event, TaskDone, TaskKind, TaskStarted, TaskUpdated};
use rush_core::text::Line;

use super::jobs::*;
use super::tests::{MIN, NOW};

fn started(id: &str, label: &str, background: bool) -> Event {
    Event::TaskStarted(TaskStarted {
        id: id.into(),
        call_id: format!("t-{id}"),
        kind: TaskKind::Shell,
        label: label.into(),
        background,
        ..TaskStarted::default()
    })
}

fn session() -> Session {
    let mut s = Session::new();
    s.apply(&started("b1", "go test ./...", false), NOW - 10_000);
    s.apply(&started("b2", "npm run dev", true), NOW - MIN);
    s.apply(&started("b3", "ls", false), NOW);
    s
}

fn plain(ls: &[Line]) -> String {
    ls.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

/// What runs in the background shows in the dock with its latest output;
/// what the turn waits on doesn't.
#[test]
fn job_dock() {
    let s = session();
    let jobs = dock_jobs(&s);
    assert_eq!(
        jobs.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(),
        ["b2"]
    );
    let tail = Tailed {
        size: 10,
        mod_at: NOW,
        lines: tail_lines_of("first line\nnpm warn\nbuilding\r50%\r100%\nlistening on :3000\n\n"),
    };
    let out = |j: &Job| (j.id == "b2").then_some(&tail);
    let d = Dock {
        sel: "",
        focus: true,
        tick: 0,
        now: NOW,
        out: &out,
    };
    let got = plain(&jobs_preview(&s, &jobs, 110, &d, Line::new()));
    for want in [
        "shell ·1  in the background",
        "npm run dev",
        "in background",
        "› listening on :3000",
    ] {
        assert!(got.contains(want), "no {want:?} in\n{got}");
    }
    assert!(!got.contains("go test") && !got.contains("› ls"), "{got}");
    // Picked, its keys head the block and its row is lit.
    let d = Dock { sel: "job:b2", ..d };
    let rows = jobs_preview(&s, &jobs, 110, &d, Line::new());
    assert!(rows[0].plain().contains("x stop"), "{}", rows[0].plain());
    assert!(rows[1].plain().starts_with('▍'), "{}", rows[1].plain());
    assert_eq!(rows[1].width(), 110);
}

fn tail_lines_of(s: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("rush-ui-jobs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{}.output", s.len()));
    std::fs::write(&p, s).unwrap();
    let got = tail_lines(&p, JOB_TAIL_MOST);
    let _ = std::fs::remove_file(&p);
    got
}

/// A progress bar redrawn with \r counts as its latest state; blank lines
/// and escapes go.
#[test]
fn tail_lines_clean() {
    assert_eq!(
        tail_lines_of("first line\n\x1b[32mnpm warn\x1b[0m\nbuilding\r50%\r100%\n\n"),
        ["first line", "npm warn", "100%"]
    );
    assert_eq!(last_lines("a\n\nb\t c \nd", 2), ["b   c", "d"]);
    assert_eq!(first_word("FOO=1 make test"), "make");
}

/// A running task whose output ends on a crash, quiet a while, is called
/// out; still writing, it may be restarting.
#[test]
fn job_crashed_after_quiet() {
    let s = session();
    let j = s.job("b2").unwrap();
    let mut t = Tailed {
        size: 10,
        mod_at: NOW - MIN,
        lines: vec![
            "listening on :3000".into(),
            "TypeError: x is undefined".into(),
            "[nodemon] app crashed - waiting for file changes before starting...".into(),
        ],
    };
    assert!(
        job_right(j, Some(&t), NOW)
            .plain()
            .contains("error in recent output")
    );
    t.mod_at = NOW;
    assert!(
        !job_right(j, Some(&t), NOW)
            .plain()
            .contains("error in recent output")
    );
}

/// What a task's output says of it, never a guess at its process.
#[test]
fn job_activity_evidence() {
    let mut j = Job {
        start: NOW - 10 * MIN,
        ..Job::default()
    };
    let mut t = Tailed {
        size: 10,
        mod_at: NOW - 2 * MIN,
        lines: Vec::new(),
    };
    let got = job_activity(&j, Some(&t), NOW).plain();
    assert!(
        got.contains("quiet for 2m") && got.contains("may be waiting"),
        "{got}"
    );
    t.mod_at = NOW - 2_000;
    assert!(
        job_activity(&j, Some(&t), NOW)
            .plain()
            .contains("output active")
    );
    t.size = 0;
    assert!(
        job_activity(&j, Some(&t), NOW)
            .plain()
            .contains("no captured output")
    );
    j.progress_at = NOW - 3_000;
    let got = job_activity(&j, None, NOW).plain();
    assert!(
        got.contains("progress updated 3s ago") && got.contains("no captured output"),
        "{got}"
    );
    j.status = "completed".into();
    assert_eq!(job_activity(&j, Some(&t), NOW).plain(), "");
}

/// How a task stands, and its keys by where it runs.
#[test]
fn job_state_and_hint() {
    let mut s = session();
    assert_eq!(
        job_state(s.job("b2").unwrap(), NOW).plain(),
        "in background · 1m"
    );
    assert!(
        job_state(s.job("b1").unwrap(), NOW)
            .plain()
            .starts_with("waiting on it")
    );
    s.apply(
        &Event::TaskUpdated(TaskUpdated {
            id: "b3".into(),
            background: Some(true),
            ..TaskUpdated::default()
        }),
        NOW,
    );
    s.apply(
        &Event::TaskDone(TaskDone {
            id: "b1".into(),
            status: "completed".into(),
            ..TaskDone::default()
        }),
        NOW,
    );
    assert!(
        job_state(s.job("b1").unwrap(), NOW)
            .plain()
            .starts_with("✓ done")
    );
    assert_eq!(dock_jobs(&s).len(), 2);
    let hint = job_hint(
        s.job("b1").unwrap(),
        false,
        Some(("FOO=1 pnpm test", "pause")),
    )
    .plain();
    assert!(
        hint.contains("kill pnpm") && hint.contains("p pause"),
        "{hint}"
    );
    assert_eq!((job_icon("monitor").0, job_icon("shell").0), ("◎", "$"));
}

/// Only a tail that follows names the files it reads (tailFollows).
#[test]
fn tail_follows_files() {
    assert_eq!(tail_follows("tail -f out.log", "/w"), ["/w/out.log"]);
    assert_eq!(
        tail_follows("tail -n 20 -F /tmp/a/../b.log", ""),
        ["/tmp/b.log"]
    );
    assert!(tail_follows("tail out.log", "/w").is_empty());
    assert!(tail_follows("cat -f x", "/w").is_empty());
}

/// A workflow lives with the subagents: not among the dock's tasks, and
/// picked, space opens its agents (jobs.go dockJobs, jobHint as of
/// bb17d8d2).
#[test]
fn workflow_with_the_subagents() {
    let mut s = session();
    s.apply(
        &Event::TaskStarted(TaskStarted {
            id: "w1".into(),
            call_id: "t-w1".into(),
            kind: TaskKind::Workflow,
            label: "port the list".into(),
            background: true,
            ..TaskStarted::default()
        }),
        NOW,
    );
    let ids: Vec<&str> = dock_jobs(&s).iter().map(|j| j.id.as_str()).collect();
    assert_eq!(ids, ["b2"]);
    let hint = job_hint(s.job("w1").unwrap(), true, None).plain();
    assert_eq!(hint, "space its agents  ·  x stop  ·  shift+x …and say why");
}
