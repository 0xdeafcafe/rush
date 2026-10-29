//! Ported from internal/watch/watch_test.go and live_test.go: a watch
//! over scripted looks, on a clock that moves only when it sleeps.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_state::Time;
use rush_watch::gh::{GhOutput, set_thread_gh};
use rush_watch::*;

fn quiet() -> Option<Box<dyn Fn(&str, serde_json::Map<String, serde_json::Value>)>> {
    Some(Box::new(|_, _| {}))
}

const LOG: &str = "2026-10-14T10:00:00.0000000Z ##[group]Run go vet\n2026-10-14T10:00:00.0000000Z ##[endgroup]\n2026-10-14T10:00:01.0000000Z x.go:3:1: declared and not used: n\n2026-10-14T10:00:01.0000000Z ##[error]Process completed with exit code 1.\n";

/// A watch over polls, one PR per look: its code, what it said, how many
/// looks.
fn script(until: Until, timeout: Duration, polls: Vec<PR>) -> (i32, Vec<String>, usize) {
    let said = RefCell::new(Vec::new());
    let now = Cell::new(Time::from_unix(0, 0));
    let looks = Cell::new(0usize);
    let o = Options {
        repo: "o/r".into(),
        number: 7,
        until,
        timeout,
        say: Some(Box::new(|s| said.borrow_mut().push(s.to_string()))),
        fetch: Some(Box::new(|_, _, _| {
            let p = polls[looks.get().min(polls.len() - 1)].clone();
            looks.set(looks.get() + 1);
            (Ok(p), 4000)
        })),
        sleep: Some(Box::new(|d| {
            now.set(now.get() + d);
            true
        })),
        logs: Some(Box::new(|_, _| Ok(LOG.as_bytes().to_vec()))),
        now: Some(Box::new(|| now.get())),
        log: quiet(),
        stopped: Some(Box::new(|| false)),
        ..Default::default()
    };
    let code = watch_pr(&o);
    drop(o);
    (code, said.into_inner(), looks.get())
}

fn pr(head: &str, checks: &[&str]) -> PR {
    PR {
        number: 7,
        title: "t".into(),
        state: "open".into(),
        head: head.into(),
        checks: checks
            .iter()
            .map(|c| {
                let (name, state) = c.split_once('=').unwrap();
                Check {
                    id: 1,
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

#[test]
fn done_waits_for_every_check_and_says_once() {
    let (code, said, looks) = script(
        Until::Done,
        Duration::from_secs(3600),
        vec![
            pr("a", &["lint=pending", "test=pending"]),
            pr("a", &["lint=failed", "test=pending"]),
            pr("a", &["lint=failed", "test=passed"]),
        ],
    );
    assert_eq!((code, looks, said.len()), (EXIT_FAILED, 3, 1), "{said:?}");
    for want in ["1 passed, 1 failed", "✗ lint failed", "declared and not used: n"] {
        assert!(said[0].contains(want), "summary lacks {want:?}:\n{}", said[0]);
    }
    assert!(!said[0].contains("test"), "passes are named:\n{}", said[0]);
}

#[test]
fn fail_ends_at_the_first_failure() {
    let (code, _, looks) = script(
        Until::Fail,
        Duration::from_secs(3600),
        vec![
            pr("a", &["lint=pending", "test=pending"]),
            pr("a", &["lint=failed", "test=pending"]),
        ],
    );
    assert_eq!((code, looks), (EXIT_FAILED, 2));
}

#[test]
fn first_ends_at_the_first_to_finish() {
    let (code, said, _) = script(
        Until::First,
        Duration::from_secs(3600),
        vec![
            pr("a", &["lint=pending", "test=pending"]),
            pr("a", &["lint=passed", "test=pending"]),
        ],
    );
    assert_eq!(code, EXIT_PASSED);
    assert!(said[0].contains("1/2 finished") && said[0].contains("✓ lint passed"));
}

#[test]
fn each_says_every_check_then_sums() {
    let (_, said, _) = script(
        Until::Each,
        Duration::from_secs(3600),
        vec![
            pr("a", &["lint=pending", "test=pending"]),
            pr("a", &["lint=passed", "test=pending"]),
            pr("b", &["lint=pending", "test=pending"]),
            pr("b", &["lint=passed", "test=passed"]),
        ],
    );
    let got = said.join("\n");
    for want in ["✓ lint passed", "new commit b", "✓ test passed", "2 passed"] {
        assert!(got.contains(want), "lacks {want:?}:\n{got}");
    }
}

#[test]
fn timeout_says_how_far_it_got() {
    let (code, said, _) = script(
        Until::Done,
        Duration::from_secs(120),
        vec![pr("a", &["lint=passed", "test=pending"])],
    );
    assert_eq!(code, EXIT_TIMED_OUT);
    assert!(said[0].contains("1/2 finished") && said[0].contains("running: test"));
}

#[test]
fn merged_ends_it() {
    let mut p = pr("a", &["lint=pending"]);
    p.state = "merged".into();
    let (code, said, _) = script(Until::Done, Duration::from_secs(3600), vec![p]);
    assert_eq!(code, EXIT_PASSED);
    assert!(said[0].contains("merged"));
}

#[test]
fn no_checks_ends_after_a_while() {
    let (code, said, _) = script(Until::Done, Duration::from_secs(3600), vec![pr("a", &[])]);
    assert_eq!(code, EXIT_PASSED);
    assert!(said[0].contains("no checks"));
}

#[test]
fn interval_backs_off() {
    for (since, left, want) in [
        (0, -1, 15),
        (6 * 60, 4000, 30),
        (20 * 60, 4000, 60),
        (0, 300, 120),
        (0, 50, 300),
    ] {
        assert_eq!(
            interval(Duration::from_secs(since), left),
            Duration::from_secs(want),
            "interval({since}s, {left})"
        );
    }
}

#[test]
fn log_errors_say_why_each_step_failed() {
    let ts = "2026-10-14T10:00:00.1Z ";
    let log = [
        "##[group]Run actions/checkout@v4\n",
        "with: x\n",
        "##[endgroup]\n",
        " * [new branch]  fix/an-error-banner -> origin/fix/an-error-banner\n",
        "##[group]Run go test ./...\n",
        "go test ./...\n",
        "##[endgroup]\n",
        "ok  \tpkg/a\t0.1s\n",
        "\x1b[31m--- FAIL: TestX (0.00s)\x1b[0m\n",
        "    x_test.go:9: got 1, want 2\n",
        "FAIL\tpkg/b\t0.2s\n",
        "##[error]Process completed with exit code 1.\n",
        "##[group]Run pnpm format:check\n",
        "pnpm format:check\n",
        "##[endgroup]\n",
        "a.ts (0ms)\n",
        "Format issues found in above 1 files.\n",
        "[ELIFECYCLE] Command failed with exit code 1.\n",
        "##[error]Process completed with exit code 1.\n",
        "Post job cleanup. error noise\n",
    ]
    .iter()
    .map(|l| format!("{ts}{l}"))
    .collect::<String>();
    let got = log_errors(&log, 8).join("\n");
    let want = "step: go test ./...\nok  \tpkg/a\t0.1s\n--- FAIL: TestX (0.00s)\nx_test.go:9: got 1, want 2\nFAIL\tpkg/b\t0.2s\nstep: pnpm format:check\na.ts (0ms)\nFormat issues found in above 1 files.";
    assert_eq!(got, want);
    // A log with no steps: what it said before it gave up.
    let got = log_errors(
        &format!("{ts}make: *** [all] 2\n{ts}##[error]Process completed with exit code 2.\n"),
        3,
    );
    assert_eq!(got, vec!["make: *** [all] 2"]);
}

#[test]
fn pick_keeps_errors_and_the_tail() {
    let lines: Vec<String> = ["a", "error: one", "b", "c", "d", "e", "f"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(rush_watch::pr::pick(&lines, 4).join(","), "error: one,d,e,f");
}

/// A fake gh, answering by what it's asked, and keeping what it was asked.
fn fake_gh(answer: impl Fn(&[String]) -> GhOutput + Send + Sync + 'static) -> Arc<Mutex<Vec<Vec<String>>>> {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let kept = asked.clone();
    set_thread_gh(Some(Arc::new(move |_: Option<&Path>, args: &[String]| {
        kept.lock().unwrap().push(args.to_vec());
        answer(args)
    })));
    asked
}

fn home() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    d
}

#[test]
fn get_shares_and_sends_the_etag() {
    let _home = home();
    let asked = fake_gh(|args| {
        // gh api -i [-H tag] path
        if args.len() > 3 {
            return GhOutput {
                stdout: b"HTTP/2.0 304 Not Modified\r\nX-Ratelimit-Remaining: 99\r\n\r\n".to_vec(),
                stderr: b"gh: HTTP 304".to_vec(),
                err: Some(rush_gh::ghpr::RunError::Other("exit status 1".into())),
            };
        }
        GhOutput {
            stdout: b"HTTP/2.0 200 OK\r\nEtag: \"e1\"\r\nX-Ratelimit-Remaining: 100\r\n\r\n{\"n\":1}".to_vec(),
            ..Default::default()
        }
    });
    let (b, left) = get("repos/o/r/pulls/1", Duration::from_secs(3600)).unwrap();
    assert_eq!((b.as_slice(), left), (&b"{\"n\":1}"[..], 100));
    // Fresh: another watcher's answer, with no call.
    get("repos/o/r/pulls/1", Duration::from_secs(3600)).unwrap();
    assert_eq!(asked.lock().unwrap().len(), 1);
    // Stale: asked with the ETag; unchanged keeps the body.
    let (b, left) = get("repos/o/r/pulls/1", Duration::ZERO).unwrap();
    assert_eq!((b.as_slice(), left), (&b"{\"n\":1}"[..], 99));
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1][3], "If-None-Match: \"e1\"");
    assert_eq!(remaining(), 99);
    set_thread_gh(None);
}

#[test]
fn get_remembers_a_spent_rate_limit() {
    let _home = home();
    fake_gh(|_| GhOutput {
        stdout: b"HTTP/2.0 403 Forbidden\r\nX-Ratelimit-Remaining: 0\r\n\r\n{}".to_vec(),
        ..Default::default()
    });
    assert_eq!(get("repos/o/r/pulls/2", Duration::ZERO), Err(Error::Limited));
    assert!(interval(Duration::ZERO, 0) >= Duration::from_secs(300));
    set_thread_gh(None);
}

#[test]
fn resolve_reads_urls_and_owner_name() {
    for (input, want) in [
        ("https://github.com/0xdeafcafe/rush/pull/42", "0xdeafcafe/rush#42"),
        ("0xdeafcafe/rush#42", "0xdeafcafe/rush#42"),
    ] {
        let (repo, n) = resolve(Path::new(""), input).unwrap();
        assert_eq!(format!("{repo}#{n}"), want);
    }
    assert_eq!(
        resolve(Path::new(""), "o/r#x").unwrap_err(),
        "\"o/r#x\" isn't a pull request"
    );
    assert_eq!(
        resolve(Path::new(""), "nope").unwrap_err(),
        "\"nope\" isn't a pull request: give its number, owner/name#number or its URL"
    );
}

#[test]
fn a_watch_keeps_its_task_live_until_it_ends() {
    let _home = home();
    let now = Cell::new(Time::from_unix(1000, 0));
    let seen = RefCell::new(Vec::new());
    let polls = [
        pr("a", &["lint=failed", "test=pending"]),
        pr("a", &["lint=failed", "test=passed"]),
    ];
    let looks = Cell::new(0usize);
    let o = Options {
        repo: "o/r".into(),
        number: 7,
        until: Until::Done,
        timeout: Duration::from_secs(3600),
        task: "bx1".into(),
        fetch: Some(Box::new(|_, _, _| {
            let p = polls[looks.get().min(1)].clone();
            looks.set(looks.get() + 1);
            (Ok(p), 4000)
        })),
        sleep: Some(Box::new(|d| {
            if let Some(l) = live_for("bx1", now.get()) {
                seen.borrow_mut().push(l);
            }
            now.set(now.get() + d);
            true
        })),
        logs: Some(Box::new(|_, _| Ok(Vec::new()))),
        now: Some(Box::new(|| now.get())),
        log: quiet(),
        stopped: Some(Box::new(|| false)),
        ..Default::default()
    };
    let code = watch_pr(&o);
    drop(o);
    let seen = seen.into_inner();
    assert_eq!((code, seen.len()), (EXIT_FAILED, 1));
    let l = &seen[0];
    assert_eq!((l.number, l.failed, l.pending), (7, 1, 1));
    assert_eq!((l.failed_checks[0].as_str(), l.running[0].as_str()), ("lint", "test"));
    assert!(!l.reports());
    assert!(live_for("bx1", now.get()).is_none(), "the record outlived the watch");
}

#[test]
fn a_live_record_isnt_believed_past_its_timeout_or_its_process() {
    let _home = home();
    let start = Time::from_unix(1000, 0);
    rush_watch::live::save_live(&Live {
        task: "a".into(),
        pid: std::process::id() as i64,
        start,
        timeout: Duration::from_secs(600),
        ..Default::default()
    });
    assert!(live_for("a", start + Duration::from_secs(14 * 60)).is_some(), "not believed within its grace");
    assert!(live_for("a", start + Duration::from_secs(16 * 60)).is_none(), "believed past its timeout and grace");
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    rush_watch::live::save_live(&Live {
        task: "b".into(),
        pid: pid as i64,
        start,
        ..Default::default()
    });
    assert!(live_for("b", start).is_none(), "believed after its process went");
    assert!(live_for("../a", start).is_none(), "a task id that isn't one reads a file");
}

#[test]
fn a_live_record_is_go_s_json() {
    let l = Live {
        task: "b7".into(),
        pid: 42,
        repo: "o/r".into(),
        number: 7,
        until: Until::Fail,
        timeout: Duration::from_secs(1500),
        start: Time::parse("2026-10-14T10:00:00Z").unwrap(),
        failed: 1,
        failed_checks: vec!["lint".into()],
        ..Default::default()
    };
    let b = rush_state::json::to_vec(&l).unwrap();
    assert_eq!(
        String::from_utf8(b).unwrap(),
        r#"{"task":"b7","pid":42,"repo":"o/r","number":7,"until":"fail","timeout":1500000000000,"start":"2026-10-14T10:00:00Z","passed":0,"failed":1,"pending":0,"skipped":0,"failedChecks":["lint"]}"#
    );
}

/// The child task_id_is_the_output_file_claude_code_writes_to runs.
#[test]
fn task_id_child() {
    if std::env::var_os("WATCH_TASK_CHILD").is_some() {
        eprintln!("task={}", task_id());
    }
}

#[test]
fn task_id_is_the_output_file_claude_code_writes_to() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("s").join("tasks");
    std::fs::create_dir_all(&dir).unwrap();
    let out = std::fs::File::create(dir.join("b7xk2q.output")).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["task_id_child", "--exact", "--nocapture"])
        .env("WATCH_TASK_CHILD", "1")
        .stdout(out)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&child.stderr);
    assert!(said.contains("task=b7xk2q\n"), "child said {said:?}");
    assert_eq!(rush_watch::live::task_of("/x/s/tasks/b7xk2q.output"), "b7xk2q");
    assert_eq!(rush_watch::live::task_of("/x/s/tasks/b7xk2q.log"), "");
}
