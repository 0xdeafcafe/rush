//! rush watch run: what it's given, GitHub's runs and jobs (fixture JSON
//! through a fake gh) to an outcome and an exit code, the answer every
//! watcher shares, 304s, and waiting out a spent rate limit. Nothing here
//! asks GitHub.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_state::Time;
use rush_watch::gh::{GhOutput, set_thread_gh};
use rush_watch::*;

fn home() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    d
}

/// A fake gh, answering by what it's asked, and keeping what it was asked.
fn fake_gh(
    answer: impl Fn(&[String]) -> GhOutput + Send + Sync + 'static,
) -> Arc<Mutex<Vec<Vec<String>>>> {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let kept = asked.clone();
    set_thread_gh(Some(Arc::new(move |_: Option<&Path>, args: &[String]| {
        kept.lock().unwrap().push(args.to_vec());
        answer(args)
    })));
    asked
}

fn ok(body: &str) -> GhOutput {
    GhOutput {
        stdout: format!(
            "HTTP/2.0 200 OK\r\nEtag: \"e1\"\r\nX-Ratelimit-Remaining: 4999\r\n\r\n{body}"
        )
        .into_bytes(),
        ..Default::default()
    }
}

fn not_modified() -> GhOutput {
    GhOutput {
        stdout: b"HTTP/2.0 304 Not Modified\r\nX-Ratelimit-Remaining: 4998\r\n\r\n".to_vec(),
        stderr: b"gh: HTTP 304".to_vec(),
        err: Some(rush_gh::ghpr::RunError::Other("exit status 1".into())),
    }
}

const LOG: &str = "2026-10-14T10:00:00.0000000Z ##[group]Run cargo test\n2026-10-14T10:00:00.0000000Z ##[endgroup]\n2026-10-14T10:00:01.0000000Z thread 'parses' panicked at src/lib.rs:9:5\n2026-10-14T10:00:01.0000000Z ##[error]Process completed with exit code 101.\n";

/// Two workflows on abcdef1: CI (a newer run and an older, superseded
/// one) and Docs.
const RUNS: &str = r#"{"total_count": 3, "workflow_runs": [
 {"id": 100, "name": "CI", "html_url": "https://github.com/o/r/actions/runs/100", "status": "completed", "conclusion": "failure", "head_sha": "abcdef1234567890", "head_branch": "main", "workflow_id": 1},
 {"id": 200, "name": "Docs", "html_url": "https://github.com/o/r/actions/runs/200", "status": "completed", "conclusion": "success", "head_sha": "abcdef1234567890", "head_branch": "main", "workflow_id": 2},
 {"id": 90, "name": "CI", "html_url": "https://github.com/o/r/actions/runs/90", "status": "completed", "conclusion": "cancelled", "head_sha": "abcdef1234567890", "head_branch": "main", "workflow_id": 1}
]}"#;

const JOBS_100: &str = r#"{"total_count": 3, "jobs": [
 {"id": 11, "name": "test", "status": "completed", "conclusion": "failure", "html_url": "https://github.com/o/r/actions/runs/100/job/11"},
 {"id": 12, "name": "lint", "status": "completed", "conclusion": "success", "html_url": "https://github.com/o/r/actions/runs/100/job/12"},
 {"id": 13, "name": "e2e", "status": "completed", "conclusion": "skipped", "html_url": null}
]}"#;

const JOBS_200: &str = r#"{"total_count": 1, "jobs": [
 {"id": 21, "name": "build", "status": "completed", "conclusion": "success", "html_url": "https://github.com/o/r/actions/runs/200/job/21"}
]}"#;

/// GitHub as the fixtures have it.
fn actions(args: &[String]) -> GhOutput {
    let path = args.last().unwrap();
    if args.iter().any(|a| a.starts_with("If-None-Match")) {
        return not_modified();
    }
    if path.ends_with("/logs") {
        return GhOutput {
            stdout: format!("HTTP/2.0 200 OK\r\n\r\n{LOG}").into_bytes(),
            ..Default::default()
        };
    }
    if path.starts_with("repos/o/r/actions/runs?head_sha=abcdef1234567890") {
        return ok(RUNS);
    }
    if path.starts_with("repos/o/r/actions/runs/100/jobs") {
        return ok(JOBS_100);
    }
    if path.starts_with("repos/o/r/actions/runs/200/jobs") {
        return ok(JOBS_200);
    }
    panic!("asked for {path}")
}

fn main_target() -> Target {
    Target {
        repo: "o/r".into(),
        what: What::Sha {
            sha: "abcdef1234567890".into(),
            branch: "main".into(),
        },
        workflow: String::new(),
    }
}

/// A watch of t through the real fetch (and so gh), on a clock that moves
/// only when it sleeps: its code and what it said.
fn watch_through_gh(t: Target, until: Until) -> (i32, Vec<String>) {
    let said = RefCell::new(Vec::new());
    let now = Cell::new(Time::from_unix(0, 0));
    let o = RunOptions {
        target: Some(t),
        until,
        timeout: Duration::from_secs(3600),
        say: Some(Box::new(|s| said.borrow_mut().push(s.to_string()))),
        sleep: Some(Box::new(|d| {
            now.set(now.get() + d);
            true
        })),
        now: Some(Box::new(|| now.get())),
        log: Some(Box::new(|_, _| {})),
        stopped: Some(Box::new(|| false)),
        ..Default::default()
    };
    let code = watch_run(&o);
    drop(o);
    (code, said.into_inner())
}

#[test]
fn reads_what_to_watch() {
    let cases: Vec<(&str, (&str, Arg))> = vec![
        ("", ("", Arg::Current)),
        ("main", ("", Arg::Branch("main".into()))),
        ("feat/watch-run", ("", Arg::Branch("feat/watch-run".into()))),
        ("12345678901", ("", Arg::Run(12345678901))),
        ("abcdef1", ("", Arg::Sha("abcdef1".into()))),
        (
            "ABCDEF1234567890ABCDEF1234567890ABCDEF12",
            (
                "",
                Arg::Sha("abcdef1234567890abcdef1234567890abcdef12".into()),
            ),
        ),
        ("cafe", ("", Arg::Branch("cafe".into()))), // too short for a sha
        ("o/r@main", ("o/r", Arg::Branch("main".into()))),
        ("o/r@feat/x", ("o/r", Arg::Branch("feat/x".into()))),
        ("o/r@4242", ("o/r", Arg::Run(4242))),
        ("o/r@abcdef1234", ("o/r", Arg::Sha("abcdef1234".into()))),
        (
            "https://github.com/o/r/actions/runs/777/job/1",
            ("o/r", Arg::Run(777)),
        ),
    ];
    for (input, (repo, arg)) in cases {
        assert_eq!(parse_arg(input), Ok((repo.to_string(), arg)), "{input:?}");
    }
    assert!(parse_arg("o/r@").is_err());
}

#[test]
fn resolve_asks_github_for_a_branch_head_once() {
    let _home = home();
    let asked = fake_gh(|args| {
        let path = args.last().unwrap();
        assert_eq!(path, "repos/o/r/branches/feat/x");
        ok(r#"{"name": "feat/x", "commit": {"sha": "0123456789abcdef"}}"#)
    });
    let t = resolve_run(Path::new(""), "o/r@feat/x", "ci.yml").unwrap();
    assert_eq!(
        t.what,
        What::Sha {
            sha: "0123456789abcdef".into(),
            branch: "feat/x".into()
        }
    );
    assert_eq!(t.label(), "feat/x @ 0123456");
    assert_eq!(
        t.runs_path(),
        "repos/o/r/actions/workflows/ci.yml/runs?head_sha=0123456789abcdef&per_page=100"
    );
    // A run id and a sha need no asking.
    let t = resolve_run(Path::new(""), "o/r@99", "").unwrap();
    assert_eq!(t.runs_path(), "repos/o/r/actions/runs/99");
    assert_eq!(asked.lock().unwrap().len(), 1);
    set_thread_gh(None);
}

#[test]
fn fetch_keeps_each_workflows_newest_run_and_its_jobs() {
    let _home = home();
    fake_gh(actions);
    let (rs, left) = rush_watch::run::fetch(&main_target(), Duration::from_secs(10));
    let rs = rs.unwrap();
    assert_eq!(left, 4999);
    let ids: Vec<i64> = rs.runs.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![100, 200], "the superseded run 90 is left out");
    let jobs: Vec<String> = rs
        .jobs()
        .map(|(_, j)| format!("{}={}", j.name, j.state))
        .collect();
    assert_eq!(
        jobs,
        vec![
            "CI / test=failed",
            "CI / lint=passed",
            "CI / e2e=skipped",
            "Docs / build=passed"
        ]
    );
    assert_eq!(rs.counts(), (2, 1, 0, 0, 1));
    assert!(rs.done());
    set_thread_gh(None);
}

#[test]
fn a_failed_job_ends_1_with_its_log_and_the_run() {
    let _home = home();
    fake_gh(actions);
    let (code, said) = watch_through_gh(main_target(), Until::Done);
    assert_eq!((code, said.len()), (EXIT_FAILED, 1), "{said:?}");
    let s = &said[0];
    for want in [
        "main @ abcdef1 runs finished after 0s: 2 passed, 1 failed, 1 skipped · CI, Docs",
        "✗ CI / test failed · https://github.com/o/r/actions/runs/100/job/11",
        "    step: cargo test",
        "panicked at src/lib.rs:9:5",
        "run: https://github.com/o/r/actions/runs/100",
    ] {
        assert!(s.contains(want), "summary lacks {want:?}:\n{s}");
    }
    assert!(!s.contains("lint"), "passes are named:\n{s}");
    assert!(!s.contains("runs/200\n"), "a passed run's URL:\n{s}");
    set_thread_gh(None);
}

#[test]
fn two_watchers_share_one_answer() {
    let _home = home();
    let asked = fake_gh(actions);
    let (a, _) = watch_through_gh(main_target(), Until::Done);
    let (b, _) = watch_through_gh(main_target(), Until::Done);
    assert_eq!((a, b), (EXIT_FAILED, EXIT_FAILED));
    let asked = asked.lock().unwrap();
    let api: Vec<&String> = asked
        .iter()
        .map(|a| a.last().unwrap())
        .filter(|p| !p.ends_with("/logs"))
        .collect();
    // The runs and each run's jobs, once, for both.
    assert_eq!(api.len(), 3, "{api:?}");
    set_thread_gh(None);
}

#[test]
fn an_unchanged_answer_is_a_304_and_keeps_the_runs() {
    let _home = home();
    let asked = fake_gh(actions);
    let t = main_target();
    let (first, _) = rush_watch::run::fetch(&t, Duration::from_secs(10));
    let (again, left) = rush_watch::run::fetch(&t, Duration::ZERO);
    assert_eq!(first.unwrap(), again.unwrap());
    assert_eq!(left, 4998, "the 304's count");
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 6);
    for a in &asked[3..] {
        assert_eq!(a[2], "-H");
        assert_eq!(a[3], "If-None-Match: \"e1\"");
    }
    set_thread_gh(None);
}

#[test]
fn a_secondary_limit_holds_every_watcher_off_until_retry_after() {
    let _home = home();
    let asked = fake_gh(|_| {
        GhOutput {
        stdout: b"HTTP/2.0 403 Forbidden\r\nRetry-After: 120\r\n\r\n{\"message\": \"You have exceeded a secondary rate limit.\"}".to_vec(),
        err: Some(rush_gh::ghpr::RunError::Other("exit status 1".into())),
        ..Default::default()
    }
    });
    let before = Time::now();
    assert_eq!(
        get("repos/o/r/actions/runs/1", Duration::ZERO),
        Err(Error::Limited)
    );
    let until = limited_until().expect("held off");
    let wait = until.sub_nanos(before);
    assert!(
        (119_000_000_000..=125_000_000_000).contains(&wait),
        "{wait}ns"
    );
    // Another question, another watcher: nobody asks until then.
    assert_eq!(
        get("repos/o/r/actions/runs/2", Duration::ZERO),
        Err(Error::Limited)
    );
    assert_eq!(asked.lock().unwrap().len(), 1);
    set_thread_gh(None);
}

#[test]
fn the_last_of_the_limit_holds_off_until_its_reset() {
    let _home = home();
    let reset = Time::now() + Duration::from_secs(600);
    let reset_unix = (reset.unix_nanos() / 1_000_000_000) as i64;
    fake_gh(move |_| {
        GhOutput {
        stdout: format!(
            "HTTP/2.0 200 OK\r\nEtag: \"e\"\r\nX-Ratelimit-Remaining: 0\r\nX-Ratelimit-Reset: {reset_unix}\r\n\r\n{{}}"
        )
        .into_bytes(),
        ..Default::default()
    }
    });
    assert!(get("repos/o/r/actions/runs/3", Duration::ZERO).is_ok());
    assert_eq!(limited_until(), Some(Time::from_unix(reset_unix, 0)));
    assert_eq!(
        get("repos/o/r/actions/runs/4", Duration::ZERO),
        Err(Error::Limited)
    );
    set_thread_gh(None);
}

#[test]
fn every_watcher_sees_the_lowest_budget_left() {
    let _home = home();
    let reset = Time::now() + Duration::from_secs(1800);
    let reset_unix = (reset.unix_nanos() / 1_000_000_000) as i64;
    fake_gh(move |args| {
        let left = if args.last().unwrap().ends_with("/a") {
            4000
        } else {
            40
        };
        GhOutput {
            stdout: format!(
                "HTTP/2.0 200 OK\r\nEtag: \"e\"\r\nX-Ratelimit-Remaining: {left}\r\nX-Ratelimit-Reset: {reset_unix}\r\n\r\n{{}}"
            )
            .into_bytes(),
            ..Default::default()
        }
    });
    let hour = Duration::from_secs(3600);
    assert_eq!(get("repos/o/r/a", hour).unwrap().1, 4000);
    // Another watcher's answer says the budget is nearly gone…
    assert_eq!(get("repos/o/r/b", hour).unwrap().1, 40);
    assert_eq!(shared_remaining(), 40);
    // …so the first, reading its own kept answer, slows down too.
    let (_, left) = get("repos/o/r/a", hour).unwrap();
    assert_eq!(left, 40);
    assert!(interval(Duration::ZERO, left) >= Duration::from_secs(300));
    set_thread_gh(None);
}

fn wr(id: i64, status: &str, jobs: &[&str]) -> WorkflowRun {
    WorkflowRun {
        id,
        name: "CI".into(),
        url: format!("https://github.com/o/r/actions/runs/{id}"),
        status: status.into(),
        jobs: jobs
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let (name, state) = j.split_once('=').unwrap();
                Check {
                    id: id * 10 + i as i64,
                    name: name.into(),
                    state: state.into(),
                    actions: true,
                    ..Default::default()
                }
            })
            .collect(),
        ..Default::default()
    }
}

/// One answer per look: Err(()) for a spent rate limit.
type Look = Result<Runs, ()>;

fn runs(r: Vec<WorkflowRun>) -> Look {
    Ok(Runs { runs: r })
}

/// A watch over scripted looks, on a clock that moves only when it
/// sleeps: its code, what it said, how many looks, and its sleeps.
fn script(
    until: Until,
    timeout: Duration,
    looks: Vec<Look>,
    limited_for: Option<Duration>,
) -> (i32, Vec<String>, usize, Vec<Duration>) {
    let said = RefCell::new(Vec::new());
    let slept = RefCell::new(Vec::new());
    let now = Cell::new(Time::from_unix(0, 0));
    let n = Cell::new(0usize);
    let o = RunOptions {
        target: Some(main_target()),
        until,
        timeout,
        say: Some(Box::new(|s| said.borrow_mut().push(s.to_string()))),
        fetch: Some(Box::new(|_| {
            let l = looks[n.get().min(looks.len() - 1)].clone();
            n.set(n.get() + 1);
            match l {
                Ok(r) => (Ok(r), 4000),
                Err(()) => (Err(Error::Limited), 0),
            }
        })),
        sleep: Some(Box::new(|d| {
            slept.borrow_mut().push(d);
            now.set(now.get() + d);
            true
        })),
        logs: Some(Box::new(|_, _| Ok(LOG.as_bytes().to_vec()))),
        now: Some(Box::new(|| now.get())),
        log: Some(Box::new(|_, _| {})),
        stopped: Some(Box::new(|| false)),
        limited: Some(Box::new(|| limited_for.map(|d| Time::from_unix(0, 0) + d))),
        // The branch stays where it was.
        head: Some(Box::new(|_, _, _| None)),
        ..Default::default()
    };
    let code = watch_run(&o);
    drop(o);
    (code, said.into_inner(), n.get(), slept.into_inner())
}

const HOUR: Duration = Duration::from_secs(3600);

#[test]
fn done_waits_for_the_run_to_complete_and_says_once() {
    let (code, said, looks, _) = script(
        Until::Done,
        HOUR,
        vec![
            runs(vec![wr(1, "queued", &["test=pending"])]),
            runs(vec![wr(1, "in_progress", &["test=pending", "lint=passed"])]),
            runs(vec![wr(1, "completed", &["test=passed", "lint=passed"])]),
        ],
        None,
    );
    assert_eq!((code, looks), (EXIT_PASSED, 3));
    assert_eq!(
        said,
        vec!["main @ abcdef1 run finished after 30s: 2 passed · CI"]
    );
}

/// A branch's run cancelled with the branch at another commit since: the
/// watch ends as cancelled, and says where the branch is and whether that
/// commit has the one watched, for the agent to decide.
#[test]
fn a_run_a_push_replaced_says_where_the_branch_went() {
    for (has, want) in [
        (
            Some(true),
            "replaced: main is at fedcba9 now, a newer push, on top of abcdef1; to wait on it: rush watch run main",
        ),
        (
            Some(false),
            "replaced: main is at fedcba9 now, a push without abcdef1 (rebased or force-pushed?): what you pushed may not be in it; to wait on it: rush watch run main",
        ),
        (
            None,
            "replaced: main is at fedcba9 now, another push; to wait on it: rush watch run main",
        ),
    ] {
        let said = RefCell::new(Vec::new());
        let now = Cell::new(Time::from_unix(0, 0));
        let o = RunOptions {
            target: Some(main_target()),
            until: Until::Done,
            timeout: HOUR,
            say: Some(Box::new(|s| said.borrow_mut().push(s.to_string()))),
            fetch: Some(Box::new(|_| {
                (
                    runs(vec![wr(1, "completed", &["check=cancelled"])])
                        .map_err(|_| Error::Limited),
                    4000,
                )
            })),
            sleep: Some(Box::new(|d| {
                now.set(now.get() + d);
                true
            })),
            now: Some(Box::new(|| now.get())),
            log: Some(Box::new(|_, _| {})),
            stopped: Some(Box::new(|| false)),
            limited: Some(Box::new(|| None)),
            head: Some(Box::new(move |repo, branch, sha| {
                assert_eq!((repo, branch, sha), ("o/r", "main", "abcdef1234567890"));
                Some(("fedcba9876543210".into(), has))
            })),
            ..Default::default()
        };
        let code = watch_run(&o);
        drop(o);
        let said = said.into_inner();
        assert_eq!(code, EXIT_ERROR, "{said:?}");
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].lines().last(), Some(want), "{said:?}");
    }

    // The branch where it was: no word of a push.
    let (code, said, _, _) = script(
        Until::Done,
        HOUR,
        vec![runs(vec![wr(1, "completed", &["check=cancelled"])])],
        None,
    );
    assert_eq!(code, EXIT_ERROR);
    assert!(!said[0].contains("replaced"), "{said:?}");
}

#[test]
fn each_says_every_job_as_it_finishes() {
    let (code, said, _, _) = script(
        Until::Each,
        HOUR,
        vec![
            runs(vec![wr(1, "in_progress", &["a=pending", "b=pending"])]),
            runs(vec![wr(1, "in_progress", &["a=passed", "b=pending"])]),
            runs(vec![wr(1, "completed", &["a=passed", "b=failed"])]),
        ],
        None,
    );
    assert_eq!(code, EXIT_FAILED);
    assert_eq!(said[0], "✓ a passed");
    assert!(said[1].starts_with("✗ b failed"), "{said:?}");
    assert!(said[2].contains("1 passed, 1 failed"), "{said:?}");
    assert!(said[2].contains("run: https://github.com/o/r/actions/runs/1"));
}

#[test]
fn fail_ends_at_the_first_failed_job() {
    let (code, said, looks, _) = script(
        Until::Fail,
        HOUR,
        vec![
            runs(vec![wr(1, "in_progress", &["a=pending", "b=pending"])]),
            runs(vec![wr(1, "in_progress", &["a=failed", "b=pending"])]),
        ],
        None,
    );
    assert_eq!((code, looks), (EXIT_FAILED, 2));
    assert!(said[0].contains("1 still running"), "{said:?}");
}

#[test]
fn first_ends_at_the_first_job_to_finish() {
    let (code, said, _, _) = script(
        Until::First,
        HOUR,
        vec![
            runs(vec![wr(1, "in_progress", &["a=pending", "b=pending"])]),
            runs(vec![wr(1, "in_progress", &["a=passed", "b=pending"])]),
        ],
        None,
    );
    assert_eq!(code, EXIT_PASSED);
    assert_eq!(said, vec!["main @ abcdef1: 1/2 jobs finished\n✓ a passed"]);
}

#[test]
fn a_cancelled_run_ends_3() {
    let (code, said, _, _) = script(
        Until::Done,
        HOUR,
        vec![runs(vec![wr(1, "completed", &["a=passed", "b=cancelled"])])],
        None,
    );
    assert_eq!(code, EXIT_ERROR);
    assert!(said[0].contains("1 passed, 1 cancelled"), "{said:?}");
    assert!(said[0].contains("⊘ b cancelled"), "{said:?}");
}

#[test]
fn a_fresh_push_waits_for_its_run_then_gives_up() {
    let (code, said, _, _) = script(
        Until::Done,
        HOUR,
        vec![
            runs(vec![]),
            runs(vec![]),
            runs(vec![wr(1, "completed", &["a=passed"])]),
        ],
        None,
    );
    assert_eq!(code, EXIT_PASSED);
    assert_eq!(said.len(), 2, "said once that it waits: {said:?}");
    assert!(said[0].contains("no run yet; waiting up to 2m"), "{said:?}");

    let (code, said, _, _) = script(Until::Done, HOUR, vec![runs(vec![])], None);
    assert_eq!(code, EXIT_ERROR);
    assert!(
        said.last().unwrap().contains("no workflow run after 2m"),
        "{said:?}"
    );
}

#[test]
fn timeout_says_how_far_it_got() {
    let (code, said, _, _) = script(
        Until::Done,
        Duration::from_secs(40),
        vec![runs(vec![wr(1, "in_progress", &["a=passed", "b=pending"])])],
        None,
    );
    assert_eq!(code, EXIT_TIMED_OUT);
    assert_eq!(
        said,
        vec![
            "main @ abcdef1 still waiting after 40s: 1/2 jobs finished; run it again\n    running: b"
        ]
    );
}

#[test]
fn a_spent_limit_is_waited_out_and_said_once() {
    let (code, said, looks, slept) = script(
        Until::Done,
        HOUR,
        vec![
            Err(()),
            Err(()),
            runs(vec![wr(1, "completed", &["a=passed"])]),
        ],
        Some(Duration::from_secs(90)),
    );
    assert_eq!((code, looks), (EXIT_PASSED, 3));
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!(
        said[0],
        "rush watch: GitHub's rate limit is spent; asking again when it resets, in 1m"
    );
    // Until the reset, then (it having passed) a few seconds.
    assert_eq!(slept[0], Duration::from_secs(90));
    assert_eq!(slept[1], Duration::from_secs(5));
}
