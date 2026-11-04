//! How long a watch's runs should take (eta.rs): the median of a
//! workflow's past successful runs, when the last run still going should
//! end, and GitHub's lists of runs read for both. Nothing asks GitHub.

rush_testenv::sandboxed!();

use std::time::Duration;

use rush_state::Time;
use rush_watch::eta::{Started, expected_end, median, started_of, usual_of};

fn secs(s: &[u64]) -> Vec<Duration> {
    s.iter().map(|&s| Duration::from_secs(s)).collect()
}

#[test]
fn the_usual_time_is_the_median() {
    let cases: [(&[u64], Option<u64>); 5] = [
        (&[], None),
        (&[300], Some(300)),
        (&[300, 900], Some(300)),
        (&[900, 300, 600], Some(600)),
        (&[100, 400, 400, 5000], Some(400)),
    ];
    for (ds, want) in cases {
        assert_eq!(median(secs(ds)), want.map(Duration::from_secs), "{ds:?}");
    }
}

fn at(s: i64) -> Time {
    Time::from_unix(s, 0)
}

#[test]
fn the_last_run_going_says_when_its_done() {
    let usual = |w: i64| match w {
        1 => Some(Duration::from_secs(600)),
        2 => Some(Duration::from_secs(120)),
        _ => None,
    };
    let run = |workflow, start, done| Started {
        workflow,
        at: at(start),
        done,
    };
    let cases: Vec<(&str, Vec<Started>, Option<Time>)> = vec![
        ("none", vec![], None),
        ("one", vec![run(1, 100, false)], Some(at(700))),
        (
            "the later of two",
            vec![run(1, 100, false), run(2, 900, false)],
            Some(at(1020)),
        ),
        (
            "a finished one is left out",
            vec![run(1, 100, true), run(2, 900, false)],
            Some(at(1020)),
        ),
        (
            "an unknown workflow is left out",
            vec![run(3, 100, false), run(2, 0, false)],
            None,
        ),
        (
            "not started yet",
            vec![Started {
                workflow: 1,
                ..Default::default()
            }],
            None,
        ),
    ];
    for (name, runs, want) in cases {
        assert_eq!(expected_end(&runs, usual), want, "{name}");
    }
}

#[test]
fn githubs_lists_of_runs_are_read() {
    let past = r#"{"workflow_runs":[
        {"workflow_id":1,"status":"completed","run_started_at":"2026-11-01T10:00:00Z","updated_at":"2026-11-01T10:06:00Z"},
        {"workflow_id":1,"status":"completed","run_started_at":"2026-11-01T11:00:00Z","updated_at":"2026-11-01T11:04:00Z"},
        {"workflow_id":1,"status":"completed","run_started_at":"2026-11-01T12:00:00Z","updated_at":"2026-11-01T12:20:00Z"},
        {"workflow_id":1,"status":"completed","run_started_at":null,"updated_at":"2026-11-01T12:20:00Z"}
    ]}"#;
    assert_eq!(usual_of(past.as_bytes()), Some(Duration::from_secs(360)));
    assert_eq!(usual_of(b"{}"), None);
    assert_eq!(usual_of(b"not json"), None);

    let on_sha = r#"{"workflow_runs":[
        {"workflow_id":7,"status":"in_progress","run_started_at":"2026-11-04T10:00:00Z"},
        {"workflow_id":8,"status":"completed","run_started_at":"2026-11-04T09:58:00Z"},
        {"workflow_id":7,"status":"completed","run_started_at":"2026-11-04T09:00:00Z"}
    ]}"#;
    assert_eq!(
        started_of(on_sha.as_bytes()),
        [
            Started {
                workflow: 7,
                at: Time::parse("2026-11-04T10:00:00Z").unwrap(),
                done: false
            },
            Started {
                workflow: 8,
                at: Time::parse("2026-11-04T09:58:00Z").unwrap(),
                done: true
            },
        ]
    );
}
