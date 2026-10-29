//! Port of internal/fleet/logins_test.go: which login rush spends next, and
//! each login's reading.

use std::collections::HashMap;
use std::time::Duration;

use rush_core::time::{Ms, now};
use rush_core::usage::{Quota, Reading, Window};
use rush_state::{Config, Login};

use crate::*;

const MINUTE: Ms = 60_000;
const HOUR: Ms = 60 * MINUTE;

fn window(id: &str, label: &str, span: Duration, percent: f64) -> Window {
    Window {
        id: id.into(),
        label: label.into(),
        span,
        percent,
        ..Window::default()
    }
}

/// Claude's two windows, five hours and a week, as a reading says them.
fn windows(five: f64, week: f64) -> Vec<Window> {
    vec![
        window("five_hour", "5h", Duration::from_secs(5 * 3600), five),
        window("seven_day", "7d", Duration::from_secs(7 * 24 * 3600), week),
    ]
}

fn login_at(id: &str, current: bool, five: f64, week: f64) -> LoginView {
    LoginView {
        login: Login {
            id: id.into(),
            name: id.into(),
            ..Login::default()
        },
        current,
        quota: Quota {
            account: id.into(),
            windows: windows(five, week),
            fetched_at: now(),
            ..Quota::default()
        },
        ..LoginView::default()
    }
}

fn want(got: Option<LoginView>, want: &str, name: &str) {
    match (got, want) {
        (None, "") => {}
        (Some(g), "") => panic!("{name}: switched to {}, want no switch", g.login.id),
        (g, w) => assert_eq!(g.map(|g| g.login.id).as_deref(), Some(w), "{name}"),
    }
}

#[test]
fn next_login_table() {
    let l = login_at;
    for (name, logins, stopped, w) in [
        (
            "room left",
            vec![l("a", true, 60.0, 40.0), l("b", false, 0.0, 0.0)],
            false,
            "",
        ),
        (
            "idle at 96% runs on",
            vec![l("a", true, 96.0, 40.0), l("b", false, 10.0, 20.0)],
            false,
            "",
        ),
        (
            "5h nearly out",
            vec![l("a", true, 99.0, 40.0), l("b", false, 10.0, 20.0)],
            false,
            "b",
        ),
        (
            "7d nearly out",
            vec![l("a", true, 10.0, 99.0), l("b", false, 10.0, 20.0)],
            false,
            "b",
        ),
        (
            "most week to lose wins",
            vec![
                l("a", true, 99.0, 40.0),
                l("b", false, 50.0, 20.0),
                l("c", false, 5.0, 30.0),
            ],
            false,
            "b",
        ),
        (
            "others nearly out too",
            vec![l("a", true, 99.0, 40.0), l("b", false, 95.0, 20.0)],
            false,
            "",
        ),
        (
            "out, the others nearly",
            vec![
                l("a", true, 100.0, 27.0),
                l("b", false, 98.0, 12.0),
                l("c", false, 0.0, 97.0),
            ],
            false,
            "c",
        ),
        (
            "out, the others too",
            vec![l("a", true, 100.0, 27.0), l("b", false, 100.0, 12.0)],
            false,
            "",
        ),
        (
            "stopped, the others nearly out",
            vec![l("a", true, 90.0, 40.0), l("b", false, 97.0, 20.0)],
            true,
            "b",
        ),
        (
            "stopped switches below the threshold",
            vec![l("a", true, 80.0, 40.0), l("b", false, 90.0, 20.0)],
            true,
            "b",
        ),
        ("only one", vec![l("a", true, 99.0, 99.0)], true, ""),
    ] {
        want(next_login(&logins, stopped, now()), w, name);
    }
}

fn week_in(mut l: LoginView, in_: Ms) -> LoginView {
    for w in &mut l.quota.windows {
        if w.id == "seven_day" {
            w.resets_at = now() + in_;
        }
    }
    l
}

// Room left in a week that resets in hours is lost unless it's spent now;
// one that resets in days can wait.
#[test]
fn next_login_spends_what_resets_soonest() {
    let (l, w) = (login_at, week_in);
    for (name, logins, want_id) in [
        (
            "hours beats days",
            vec![
                w(l("a", true, 12.0, 2.0), 72 * HOUR),
                w(l("b", false, 0.0, 77.0), 4 * HOUR),
            ],
            "b",
        ),
        (
            "days stays on hours",
            vec![
                w(l("a", true, 12.0, 77.0), 4 * HOUR),
                w(l("b", false, 0.0, 2.0), 72 * HOUR),
            ],
            "",
        ),
        (
            "too little to move for",
            vec![
                w(l("a", true, 12.0, 2.0), 72 * HOUR),
                w(l("b", false, 0.0, 95.0), HOUR),
            ],
            "",
        ),
        (
            "nearly out takes the most urgent",
            vec![
                w(l("a", true, 99.0, 2.0), 72 * HOUR),
                w(l("b", false, 0.0, 0.0), 100 * HOUR),
                w(l("c", false, 0.0, 60.0), 10 * HOUR),
            ],
            "c",
        ),
        (
            "out of 5h for now",
            vec![
                w(l("a", true, 12.0, 2.0), 72 * HOUR),
                w(l("b", false, 100.0, 77.0), 4 * HOUR),
            ],
            "",
        ),
    ] {
        want(next_login(&logins, false, now()), want_id, name);
    }
}

#[test]
fn next_login_skips_logins_without_reading() {
    let unread = LoginView {
        login: Login {
            id: "b".into(),
            ..Login::default()
        },
        ..LoginView::default()
    };
    let mut stale = login_at("c", false, 0.0, 0.0);
    stale.quota.fetched_at = now() - 2 * HOUR;
    let mut older = login_at("e", false, 0.0, 0.0);
    older.quota.fetched_at = now() - 40 * MINUTE;
    want(
        next_login(&[login_at("a", true, 100.0, 55.0), older], false, now()),
        "e",
        "a login not in use only empties",
    );
    want(
        next_login(
            &[
                login_at("a", true, 100.0, 55.0),
                unread,
                stale,
                login_at("d", false, 1.0, 0.0),
            ],
            true,
            now(),
        ),
        "d",
        "the only one with a reading",
    );
}

#[test]
fn next_login_ignores_stale_reading() {
    let mut cur = login_at("a", true, 99.0, 40.0);
    cur.quota.fetched_at = now() - HOUR;
    want(
        next_login(&[cur, login_at("b", false, 0.0, 0.0)], false, now()),
        "",
        "an hour-old reading",
    );
}

fn reading(account: &str, at: Ms, five: f64) -> Reading {
    Reading {
        account_id: account.into(),
        fetched_at: at,
        windows: vec![window(
            "five_hour",
            "5h",
            Duration::from_secs(5 * 3600),
            five,
        )],
        ..Reading::default()
    }
}

fn five_hour(r: &Reading) -> f64 {
    r.windows
        .iter()
        .find(|w| w.id == "five_hour")
        .map_or(0.0, |w| w.percent)
}

// A reading made before the folder was signed in as another account is
// that account's: it mustn't be shown, or switched on, as the new one's.
#[test]
fn freshest_skips_other_accounts_reading() {
    let t = now();
    let mut fetched = HashMap::from([("/x".to_string(), reading("old", t, 97.0))]);
    let cached = reading("new", t - MINUTE, 3.0);
    let u = freshest(&fetched, "/x", cached.clone(), t);
    assert!(u.account_id == "new" && five_hour(&u) == 3.0, "{u:?}");
    fetched.insert("/x".into(), reading("new", t, 5.0));
    assert_eq!(five_hour(&freshest(&fetched, "/x", cached, t)), 5.0);
}

// A reading kept by login is the folder's as soon as it's newer.
#[test]
fn freshest_takes_login_reading() {
    let t = now();
    let fetched = HashMap::from([
        ("/x".to_string(), reading("a", t - 4 * MINUTE, 90.0)),
        ("login:a".to_string(), reading("a", t, 96.0)),
    ]);
    let cached = reading("a", t - HOUR, 0.0);
    assert_eq!(five_hour(&freshest(&fetched, "/x", cached, t)), 96.0);
}

fn cfg() -> Config {
    let lg = |id: &str| Login {
        id: id.into(),
        name: id.into(),
        ..Login::default()
    };
    Config {
        logins: vec![lg("a"), lg("b")],
        ..Config::default()
    }
}

#[test]
fn logins_use_own_reading_after_switch() {
    let t = now();
    let fetched = HashMap::from([
        ("login:a".to_string(), reading("a", t - MINUTE, 97.0)),
        ("login:b".to_string(), reading("b", t - MINUTE, 4.0)),
    ]);
    // The home is now signed in as b, with a fresher reading of its own.
    let root = AccountView {
        usage: reading("b", t, 6.0),
        ..AccountView::default()
    };
    let got = crate::logins::logins(&cfg(), &root, "", &fetched, &mut HashMap::new(), t);
    assert!(
        got[1].current && five_hour(&got[1].usage) == 6.0,
        "b: {:?}",
        got[1]
    );
    assert!(
        !got[0].current && five_hour(&got[0].usage) == 97.0,
        "a: {:?}",
        got[0]
    );
}

// The login in use in its home is the current one, whatever the agent's
// home is signed in as.
#[test]
fn login_in_use_in_its_home() {
    let t = now();
    let fetched = HashMap::from([("login:a".to_string(), reading("a", t, 30.0))]);
    let root = AccountView {
        usage: reading("b", t, 80.0),
        ..AccountView::default()
    };
    let got = crate::logins::logins(&cfg(), &root, "a", &fetched, &mut HashMap::new(), t);
    assert!(
        got[0].current && five_hour(&got[0].usage) == 30.0,
        "a: {:?}",
        got[0]
    );
    assert!(
        !got[1].current && five_hour(&got[1].usage) == 80.0,
        "b: {:?}",
        got[1]
    );
}

// The screenshot's three: borrowed in use, personal's week ending in 4h
// once its 5h is back in 3m, alex's week full for 42h.
#[test]
fn chain_order() {
    let t = now();
    let at = |mut l: LoginView, five_in: Ms, week_in: Ms| {
        for w in &mut l.quota.windows {
            match w.id.as_str() {
                "five_hour" => w.resets_at = t + five_in,
                "seven_day" => w.resets_at = t + week_in,
                _ => {}
            }
        }
        l
    };
    let got = chain(
        &[
            at(login_at("alex", false, 0.0, 100.0), 5 * HOUR, 42 * HOUR),
            at(login_at("borrowed", true, 12.0, 2.0), 4 * HOUR, 72 * HOUR),
            at(
                login_at("personal", false, 100.0, 77.0),
                3 * MINUTE,
                4 * HOUR,
            ),
            LoginView {
                login: Login {
                    id: "unread".into(),
                    ..Login::default()
                },
                ..LoginView::default()
            },
        ],
        t,
    );
    let names: Vec<&str> = got.iter().map(|k| k.login.login.name.as_str()).collect();
    assert_eq!(names, ["borrowed", "personal", "alex"]);
    assert_eq!((got[1].wait, got[2].wait), (3 * MINUTE, 42 * HOUR));
}

// A borrowed login is never switched to, nor in the switch order unless
// you're on it.
#[test]
fn next_login_skips_borrowed() {
    let mut team = login_at("team", false, 0.0, 0.0);
    team.login.borrowed = true;
    want(
        next_login(
            &[login_at("a", true, 100.0, 40.0), team.clone()],
            true,
            now(),
        ),
        "",
        "borrowed",
    );
    want(
        next_login(
            &[
                login_at("a", true, 99.0, 40.0),
                team.clone(),
                login_at("b", false, 10.0, 20.0),
            ],
            false,
            now(),
        ),
        "b",
        "past borrowed",
    );
    assert_eq!(
        chain(&[login_at("a", true, 10.0, 10.0), team], now()).len(),
        1
    );
}
