//! Waiting out GitHub's rate limit: what counts as one (REST and GraphQL),
//! when it resets, the backoff when nothing says, and what a watch does
//! about it. GitHub's answers and the clock are injected; nothing sleeps.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_state::Time;
use rush_watch::gh::{GhOutput, set_thread_gh};
use rush_watch::limit::{
    self, Limit, classify, reset_from_headers, reset_from_rate_limit, timed_out_line, wait_for,
    waiting_line,
};
use rush_watch::*;

fn home() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    d
}

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

fn failed() -> Option<rush_gh::ghpr::RunError> {
    Some(rush_gh::ghpr::RunError::Other("exit status 1".into()))
}

fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

const NOW: i64 = 1_700_000_000;

fn now() -> Time {
    Time::from_unix(NOW, 0)
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn what_counts_as_a_rate_limit() {
    let reset = (NOW + 300).to_string();
    // (status, headers, body, stderr, limited, its reset)
    let cases = vec![
        // REST: the hourly limit spent.
        (
            403,
            headers(&[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", &reset),
            ]),
            r#"{"message":"API rate limit exceeded for user ID 1."}"#,
            "",
            true,
            Some(Time::from_unix(NOW + 300, 0)),
        ),
        (
            403,
            headers(&[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", &reset),
            ]),
            "",
            "",
            true,
            Some(Time::from_unix(NOW + 300, 0)),
        ),
        // 429, whatever it says.
        (429, headers(&[]), "", "", true, None),
        (
            429,
            headers(&[("retry-after", "30")]),
            "",
            "",
            true,
            Some(Time::from_unix(NOW + 30, 0)),
        ),
        // A secondary limit: Retry-After, or only its message.
        (
            403,
            headers(&[("retry-after", "120")]),
            "{}",
            "",
            true,
            Some(Time::from_unix(NOW + 120, 0)),
        ),
        (
            403,
            headers(&[("x-ratelimit-remaining", "12")]),
            r#"{"message":"You have exceeded a secondary rate limit."}"#,
            "",
            true,
            None,
        ),
        // GraphQL: gh prints it, with no HTTP status.
        (
            0,
            headers(&[]),
            "",
            "gh: GraphQL: API rate limit already exceeded for user ID 42.",
            true,
            None,
        ),
        (
            0,
            headers(&[]),
            r#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#,
            "",
            true,
            None,
        ),
        // Not rate limits.
        (
            403,
            headers(&[("x-ratelimit-remaining", "4000")]),
            r#"{"message":"Resource not accessible by integration"}"#,
            "",
            false,
            None,
        ),
        (
            404,
            headers(&[]),
            r#"{"message":"Not Found"}"#,
            "",
            false,
            None,
        ),
        (
            404,
            headers(&[("x-ratelimit-remaining", "0")]),
            r#"{"message":"Not Found"}"#,
            "",
            false,
            None,
        ),
        (
            0,
            headers(&[]),
            "",
            "To get started with GitHub CLI, please run: gh auth login",
            false,
            None,
        ),
        (
            200,
            headers(&[("x-ratelimit-remaining", "0")]),
            "{}",
            "",
            false,
            None,
        ),
        (304, headers(&[]), "", "gh: HTTP 304", false, None),
        // A reset that has passed names nothing to wait for.
        (
            403,
            headers(&[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", &(NOW - 5).to_string()),
            ]),
            "",
            "",
            true,
            None,
        ),
    ];
    for (i, (status, h, body, stderr, limited, reset)) in cases.into_iter().enumerate() {
        let got = classify(status, &h, body.as_bytes(), stderr.as_bytes(), now());
        assert_eq!(got.is_some(), limited, "case {i}: {status} {body} {stderr}");
        if let Some(Limit { reset: r, .. }) = got {
            assert_eq!(r, reset, "case {i}");
        }
    }
}

#[test]
fn graphql_limits_are_told_apart() {
    let g = classify(
        0,
        &headers(&[]),
        b"",
        b"gh: GraphQL: API rate limit already exceeded",
        now(),
    );
    assert_eq!(g.map(|l| l.graphql), Some(true));
    let r = classify(429, &headers(&[]), b"", b"", now());
    assert_eq!(r.map(|l| l.graphql), Some(false));
}

#[test]
fn headers_say_when_it_resets_never_more_than_an_hour_away() {
    let cases = [
        (headers(&[("retry-after", "90")]), Some(NOW + 90)),
        (headers(&[("retry-after", "0")]), Some(NOW + 1)),
        (headers(&[("retry-after", "999999")]), Some(NOW + 3600)),
        // Retry-After first, then the reset.
        (
            headers(&[
                ("retry-after", "10"),
                ("x-ratelimit-reset", &(NOW + 500).to_string()),
            ]),
            Some(NOW + 10),
        ),
        (
            headers(&[("x-ratelimit-reset", &(NOW + 500).to_string())]),
            Some(NOW + 500),
        ),
        (
            headers(&[("x-ratelimit-reset", &(NOW + 90_000).to_string())]),
            Some(NOW + 3600),
        ),
        (
            headers(&[("x-ratelimit-reset", &(NOW - 1).to_string())]),
            None,
        ),
        (headers(&[("retry-after", "soon")]), None),
        (headers(&[]), None),
    ];
    for (h, want) in cases {
        assert_eq!(
            reset_from_headers(&h, now()),
            want.map(|s| Time::from_unix(s, 0)),
            "{h:?}"
        );
    }
}

#[test]
fn gh_api_rate_limit_says_which_resource_is_spent() {
    let body = |core: i64, graphql: i64| {
        format!(
            r#"{{"resources":{{"core":{{"limit":5000,"remaining":{core},"reset":{}}},"graphql":{{"limit":5000,"remaining":{graphql},"reset":{}}}}}}}"#,
            NOW + 100,
            NOW + 700
        )
    };
    // (core left, graphql left, was it graphql's, its reset)
    for (core, graphql, gql, want) in [
        (0, 4000, false, Some(NOW + 100)),
        (4000, 0, false, Some(NOW + 700)),
        (4000, 0, true, Some(NOW + 700)),
        (0, 0, true, Some(NOW + 700)),
        (0, 0, false, Some(NOW + 700)),
        (4000, 4000, false, None),
    ] {
        assert_eq!(
            reset_from_rate_limit(body(core, graphql).as_bytes(), gql, now()),
            want.map(|s| Time::from_unix(s, 0)),
            "{core} {graphql} {gql}"
        );
    }
    for bad in ["", "{}", "not json", r#"{"resources":5}"#] {
        assert_eq!(reset_from_rate_limit(bad.as_bytes(), false, now()), None);
    }
}

#[test]
fn the_backoff_doubles_from_a_minute_to_ten() {
    for (strikes, want) in [
        (0, 60),
        (1, 120),
        (2, 240),
        (3, 480),
        (4, 600),
        (5, 600),
        (50, 600),
        (u32::MAX, 600),
    ] {
        assert_eq!(limit::backoff(strikes), secs(want), "{strikes}");
    }
}

#[test]
fn waiting_is_until_the_reset_or_the_backoff() {
    let at = |s: u64| Some(now() + secs(s));
    for (known, strikes, want) in [
        (at(300), 0, 300),
        (at(300), 3, 300),
        // Just reset: a few seconds, not a spin.
        (at(0), 0, 5),
        (Some(Time::from_unix(NOW - 50, 0)), 0, 5),
        (None, 0, 60),
        (None, 1, 120),
        (None, 2, 240),
        (None, 9, 600),
    ] {
        assert_eq!(
            wait_for(known, now(), strikes),
            secs(want),
            "{known:?} {strikes}"
        );
    }
}

#[test]
fn the_lines_say_when_it_resumes() {
    // Written in UTC here: 1_700_000_000 is 22:13:20.
    let until = (now() + secs(6 * 60)).with_offset(0);
    assert_eq!(
        waiting_line(until, now()),
        "GitHub's rate limit is spent; waiting until 22:19 (resets in 6m)"
    );
    let until = (now() + secs(45)).with_offset(0);
    assert_eq!(
        waiting_line(until, now()),
        "GitHub's rate limit is spent; waiting until 22:14 (resets in 45s)"
    );
    let until = (now() + secs(30 * 60)).with_offset(0);
    assert_eq!(
        timed_out_line(until, now(), secs(5 * 60)),
        "GitHub's rate limit is spent until 22:43 (resets in 30m), past the 5m timeout; run it again after"
    );
}

// What GitHub says, through the shared answers.

#[test]
fn a_graphql_limit_is_waited_for_until_gh_says_it_resets() {
    let _home = home();
    let reset = (Time::now() + secs(600)).unix_nanos() as i64 / 1_000_000_000;
    let asked = fake_gh(move |args| {
        if args.last().map(String::as_str) == Some("rate_limit") {
            return GhOutput {
                stdout: format!(
                    "HTTP/2.0 200 OK\r\n\r\n{{\"resources\":{{\"core\":{{\"remaining\":4000,\"reset\":{reset}}},\"graphql\":{{\"remaining\":0,\"reset\":{reset}}}}}}}"
                )
                .into_bytes(),
                ..Default::default()
            };
        }
        GhOutput {
            stderr: b"gh: GraphQL: API rate limit already exceeded for user ID 42.".to_vec(),
            err: failed(),
            ..Default::default()
        }
    });
    assert_eq!(
        get("repos/o/r/pulls/1", Duration::ZERO),
        Err(Error::Limited)
    );
    assert_eq!(limited_until(), Some(Time::from_unix(reset, 0)));
    // Held off for every watcher: nobody asks again.
    assert_eq!(
        get("repos/o/r/pulls/2", Duration::ZERO),
        Err(Error::Limited)
    );
    assert_eq!(
        asked.lock().unwrap().len(),
        2,
        "the question, and when it resets"
    );
    set_thread_gh(None);
}

#[test]
fn a_limit_that_says_nothing_of_its_reset_backs_off_a_minute() {
    let _home = home();
    fake_gh(|_| {
        GhOutput {
        stdout: b"HTTP/2.0 403 Forbidden\r\n\r\n{\"message\":\"You have exceeded a secondary rate limit.\"}".to_vec(),
        err: failed(),
        ..Default::default()
    }
    });
    let before = Time::now();
    assert_eq!(
        get("repos/o/r/pulls/1", Duration::ZERO),
        Err(Error::Limited)
    );
    let wait = limited_until().expect("held off").sub_nanos(before);
    assert!(
        (59_000_000_000..=62_000_000_000).contains(&wait),
        "{wait}ns"
    );
    set_thread_gh(None);
}

#[test]
fn a_real_failure_is_not_a_limit() {
    let _home = home();
    fake_gh(|_| {
        GhOutput {
        stdout: b"HTTP/2.0 404 Not Found\r\nX-Ratelimit-Remaining: 4000\r\n\r\n{\"message\":\"Not Found\"}".to_vec(),
        err: failed(),
        ..Default::default()
    }
    });
    assert_eq!(
        get("repos/o/r/pulls/1", Duration::ZERO),
        Err(Error::Other("GitHub says: Not Found (HTTP 404)".into()))
    );
    assert_eq!(limited_until(), None);
    set_thread_gh(None);
}

#[test]
fn gh_itself_saying_the_graphql_limit_is_spent_holds_everyone_off() {
    let _home = home();
    let asked = fake_gh(|_| GhOutput {
        stderr: b"GraphQL: API rate limit already exceeded for user ID 42.".to_vec(),
        err: failed(),
        ..Default::default()
    });
    assert!(matches!(
        gh::run_checked(None, &["pr", "view"]),
        Err(Error::Limited)
    ));
    assert!(limited_until().is_some());
    let n = asked.lock().unwrap().len();
    assert!(matches!(
        gh::run_checked(None, &["pr", "view"]),
        Err(Error::Limited)
    ));
    assert_eq!(asked.lock().unwrap().len(), n, "not asked while it's spent");
    set_thread_gh(None);
}

// Patience: what comes before a watch.

#[test]
fn patience_waits_out_each_limit_and_says_once() {
    let clock = Cell::new(Time::from_unix(NOW, 0));
    let said = RefCell::new(Vec::new());
    let slept = RefCell::new(Vec::new());
    let p = Patience {
        wait: true,
        start: clock.get(),
        timeout: Duration::ZERO,
        say: Box::new(|s| said.borrow_mut().push(s.to_string())),
        sleep: Box::new(|d| {
            slept.borrow_mut().push(d);
            clock.set(clock.get() + d);
            true
        }),
        now: Box::new(|| clock.get()),
        // Nothing says when it resets: the backoff.
        limited: Box::new(|| None),
    };
    let tries = Cell::new(0);
    let got = p.call(|| {
        tries.set(tries.get() + 1);
        if tries.get() <= 4 {
            Err(Error::Limited)
        } else {
            Ok("yes")
        }
    });
    assert_eq!(got, Ok("yes"));
    assert_eq!(
        *slept.borrow(),
        vec![secs(60), secs(120), secs(240), secs(480)]
    );
    // A line when it starts, and when the resume moves by more than 30s.
    assert_eq!(said.borrow().len(), 4);
    assert!(said.borrow()[0].starts_with("GitHub's rate limit is spent; waiting until "));
    assert!(said.borrow()[0].ends_with("(resets in 1m)"));
}

#[test]
fn patience_gives_up_when_the_reset_is_after_the_timeout() {
    let clock = Cell::new(Time::from_unix(NOW, 0));
    let slept = RefCell::new(Vec::new());
    let p = Patience {
        wait: true,
        start: clock.get(),
        timeout: secs(300),
        say: Box::new(|_| {}),
        sleep: Box::new(|d| {
            slept.borrow_mut().push(d);
            true
        }),
        now: Box::new(|| clock.get()),
        limited: Box::new(|| Some(Time::from_unix(NOW + 1800, 0))),
    };
    let got: Result<(), _> = p.call(|| Err(Error::Limited));
    let Err(Waited::TimedOut(line)) = got else {
        panic!("{got:?}");
    };
    assert!(
        line.contains("(resets in 30m), past the 5m timeout"),
        "{line}"
    );
    assert!(slept.borrow().is_empty());
}

#[test]
fn patience_that_does_not_wait_hands_the_limit_back_and_other_errors_too() {
    let p = Patience::none();
    assert_eq!(
        p.call(|| Err::<(), _>(Error::Limited)),
        Err(Waited::Err(Error::Limited))
    );
    assert_eq!(
        p.call(|| Err::<(), _>(Error::Other("x".into()))),
        Err(Waited::Err(Error::Other("x".into())))
    );
}

#[test]
fn resolving_waits_out_gh_saying_the_limit_is_spent() {
    let _home = home();
    let spent = Arc::new(Mutex::new(true));
    let flag = spent.clone();
    fake_gh(move |args| {
        if args.last().map(String::as_str) == Some("rate_limit") {
            return GhOutput {
                err: failed(),
                ..Default::default()
            };
        }
        if std::mem::take(&mut *flag.lock().unwrap()) {
            return GhOutput {
                stderr: b"GraphQL: API rate limit already exceeded for user ID 42.".to_vec(),
                err: failed(),
                ..Default::default()
            };
        }
        GhOutput {
            stdout: b"o/r\n".to_vec(),
            ..Default::default()
        }
    });
    let clock = Cell::new(Time::now());
    let said = RefCell::new(Vec::new());
    let p = Patience {
        wait: true,
        start: clock.get(),
        timeout: secs(25 * 60),
        say: Box::new(|s| said.borrow_mut().push(s.to_string())),
        sleep: Box::new(|d| {
            // The reset comes: nobody is held off any more.
            let _ = std::fs::remove_file(rush_state::dir().join("watch/gh/ratelimit.json"));
            clock.set(clock.get() + d);
            true
        }),
        now: Box::new(|| clock.get()),
        limited: Box::new(limited_until),
    };
    let (repo, n) = resolve_with(Path::new(""), "7", &p).unwrap();
    assert_eq!((repo.as_str(), n), ("o/r", 7));
    assert_eq!(said.borrow().len(), 1);
    assert!(
        said.borrow()[0].ends_with("(resets in 1m)"),
        "{:?}",
        said.borrow()
    );
    set_thread_gh(None);
}

#[test]
fn resolving_ends_as_timed_out_when_the_reset_is_after_the_timeout() {
    let _home = home();
    fake_gh(|_| GhOutput {
        stdout: b"HTTP/2.0 403 Forbidden\r\nRetry-After: 1800\r\n\r\n{}".to_vec(),
        stderr: b"GraphQL: API rate limit already exceeded".to_vec(),
        err: failed(),
    });
    let p = Patience {
        wait: true,
        start: Time::now(),
        timeout: secs(300),
        say: Box::new(|_| {}),
        sleep: Box::new(|_| panic!("it must not wait")),
        now: Box::new(Time::now),
        limited: Box::new(|| Some(Time::now() + secs(1800))),
    };
    let got = resolve_with(Path::new(""), "7", &p);
    assert!(matches!(got, Err(Unresolved::TimedOut(_))), "{got:?}");
    set_thread_gh(None);
}

// Watches.

fn pr(state: &str) -> PR {
    PR {
        number: 7,
        title: "t".into(),
        state: state.into(),
        head: "abc".into(),
        checks: vec![Check {
            id: 1,
            name: "test".into(),
            state: "passed".into(),
            actions: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A pr watch whose looks are Err for a spent limit, on a clock that moves
/// only when it sleeps: its code, what it said, how many looks, its sleeps.
fn pr_script(
    timeout: Duration,
    looks: Vec<Result<PR, ()>>,
    known: Option<Duration>,
) -> (i32, Vec<String>, usize, Vec<Duration>) {
    let said = RefCell::new(Vec::new());
    let slept = RefCell::new(Vec::new());
    let clock = Cell::new(Time::from_unix(NOW, 0));
    let n = Cell::new(0usize);
    let o = Options {
        repo: "o/r".into(),
        number: 7,
        until: Until::Done,
        timeout,
        say: Some(Box::new(|s| said.borrow_mut().push(s.to_string()))),
        fetch: Some(Box::new(|_, _, _| {
            let l = looks[n.get().min(looks.len() - 1)].clone();
            n.set(n.get() + 1);
            match l {
                Ok(p) => (Ok(p), 4000),
                Err(()) => (Err(Error::Limited), 0),
            }
        })),
        sleep: Some(Box::new(|d| {
            slept.borrow_mut().push(d);
            clock.set(clock.get() + d);
            true
        })),
        now: Some(Box::new(|| clock.get())),
        log: Some(Box::new(|_, _| {})),
        stopped: Some(Box::new(|| false)),
        limited: Some(Box::new(|| known.map(|d| Time::from_unix(NOW, 0) + d))),
        ..Default::default()
    };
    let code = watch_pr(&o);
    drop(o);
    (code, said.into_inner(), n.get(), slept.into_inner())
}

fn here(s: i64) -> Time {
    Time::from_unix(s, 0).local()
}

#[test]
fn a_pr_watch_waits_out_a_spent_limit_and_carries_on() {
    let (code, said, looks, slept) =
        pr_script(secs(3600), vec![Err(()), Ok(pr("merged"))], Some(secs(390)));
    assert_eq!((code, looks), (EXIT_PASSED, 2));
    assert_eq!(slept, vec![secs(390)]);
    assert_eq!(
        said[0],
        waiting_line(here(NOW + 390), Time::from_unix(NOW, 0))
    );
    assert!(said[0].ends_with("(resets in 6m)"), "{}", said[0]);
}

#[test]
fn a_pr_watch_backs_off_when_nothing_says_when_it_resets() {
    let (code, said, looks, slept) = pr_script(
        secs(3600),
        vec![
            Err(()),
            Err(()),
            Err(()),
            Err(()),
            Err(()),
            Err(()),
            Ok(pr("merged")),
        ],
        None,
    );
    assert_eq!((code, looks), (EXIT_PASSED, 7));
    assert_eq!(
        slept,
        vec![
            secs(60),
            secs(120),
            secs(240),
            secs(480),
            secs(600),
            secs(600)
        ]
    );
    assert!(said.len() >= 2, "{said:?}");
}

#[test]
fn a_pr_watch_times_out_when_the_reset_is_after_its_timeout() {
    let (code, said, looks, slept) = pr_script(secs(300), vec![Err(())], Some(secs(1800)));
    assert_eq!((code, looks), (EXIT_TIMED_OUT, 1));
    assert!(slept.is_empty());
    assert_eq!(
        said,
        vec![timed_out_line(
            here(NOW + 1800),
            Time::from_unix(NOW, 0),
            secs(300)
        )]
    );
}

#[test]
fn a_pr_watch_that_is_closed_still_ends_as_a_failure() {
    let (code, _, _, _) = pr_script(secs(3600), vec![Err(()), Ok(pr("closed"))], Some(secs(60)));
    assert_eq!(code, EXIT_ERROR);
}
