//! The usage-limit card: what it says of the limit, its keys, and r
//! reading usage again without answering it. After Go's
//! usage_limit_test.go and rushmode.go's cardKey (limit).

use rush_core::usage::{Quota, Window};

use super::limit::{LimitUsage, countdown};
use super::tests::*;
use super::*;
use crate::screens::session_core::host::Limit;

const NOW: i64 = 1_793_190_896_789;

fn limited() -> Model {
    let mut m = hosted(100, 40);
    set_info(&mut m, |i| {
        i.limit = Some(Limit {
            resets_at: NOW + 3 * 3_600_000 + 25 * 60_000 + 7_000,
            window: "five_hour".into(),
            continue_: false,
            ask: true,
        });
    });
    m
}

#[test]
fn the_card_says_the_limit_and_its_keys() {
    let mut m = limited();
    let out = text(&rows(&mut m, 90, 40));
    for want in [
        "Provider quota reached · usage numbers unavailable",
        "Reset in 03:25:07 · ",
        "Last checked: no successful usage reading",
        "Continue at reset? New messages stay queued.",
        "y continue",
        "n wait for me",
        "r refresh usage",
        "esc later",
    ] {
        assert!(out.contains(want), "no {want:?} in\n{out}");
    }
    // Short of room, its keys stay and the question goes.
    let short = text(&rows(&mut m, 90, 5));
    assert!(short.contains("r refresh usage"), "{short}");
    assert!(!short.contains("Continue at reset?"), "{short}");
}

#[test]
fn a_reading_names_the_window_and_when_it_was_checked() {
    let mut m = limited();
    conn(&mut m, "k").limit_usage = LimitUsage {
        reading: Quota {
            fetched_at: NOW - 90_000,
            windows: vec![
                Window {
                    id: "five_hour".into(),
                    name: "5-hour".into(),
                    percent: 100.0,
                    ..Window::default()
                },
                Window {
                    id: "seven_day".into(),
                    name: "Weekly".into(),
                    percent: 41.0,
                    ..Window::default()
                },
            ],
            ..Quota::default()
        },
        ..LimitUsage::default()
    };
    let out = text(&rows(&mut m, 90, 40));
    for want in [
        "5-hour · 100% used",
        "Last checked ",
        " · 1m 30s ago",
        "Weekly · 41% used",
    ] {
        assert!(out.contains(want), "no {want:?} in\n{out}");
    }
}

#[test]
fn y_and_n_answer_it_and_r_only_reads_usage() {
    let mut m = limited();
    rows(&mut m, 90, 40); // it came while you weren't typing: the keys are its
    assert!(m.cards().conns["k"].card_focus);
    press(&mut m, "r");
    assert!(acts(&mut m).is_empty(), "r answers nothing");
    // No agent in the list here: nothing to read it through, and it says.
    let out = text(&rows(&mut m, 90, 40));
    assert!(
        out.contains(": The session's account is unavailable"),
        "{out}"
    );
    assert!(m.cards().conns["k"].card_focus, "still the card's");
    press(&mut m, "y");
    assert_eq!(acts(&mut m), vec![Act::ContinueAtReset(true)]);
    assert!(!m.cards().conns["k"].card_focus);
    // alt+n from anywhere.
    m.kept::<Fake>().acts.clear();
    press(&mut m, "alt+n");
    assert_eq!(acts(&mut m), vec![Act::ContinueAtReset(false)]);
}

#[test]
fn countdown_words() {
    assert_eq!(countdown(0, NOW), "Reset time unknown");
    assert_eq!(
        countdown(NOW - 1, NOW),
        "Reset time passed · refresh to check usage"
    );
    assert!(countdown(NOW + 61_000, NOW).starts_with("Reset in 00:01:01 · "));
}

#[test]
fn a_refresh_keeps_the_last_good_reading() {
    let mut lu = LimitUsage {
        reading: Quota {
            fetched_at: NOW - 60_000,
            ..Quota::default()
        },
        refreshing: true,
        ..LimitUsage::default()
    };
    lu.apply(
        Quota {
            problem: "rate limited".into(),
            ..Quota::default()
        },
        NOW,
        false,
    );
    assert!(!lu.refreshing);
    assert_eq!(lu.problem, "rate limited");
    assert_eq!(lu.reading.fetched_at, NOW - 60_000, "kept");
    lu.apply(Quota::default(), NOW, false);
    assert_eq!(
        lu.problem,
        "No usage reading available; the session remains paused"
    );
    lu.apply(
        Quota {
            fetched_at: NOW,
            ..Quota::default()
        },
        NOW,
        true,
    );
    assert_eq!(lu.problem, "Offline · showing the cached reading");
    assert_eq!(lu.reading.fetched_at, NOW);
}
