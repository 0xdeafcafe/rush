//! Ported from internal/remind/remind_test.go.

use super::*;

/// A rush folder of the test's own, and no waiters started.
struct Home {
    _dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().to_path_buf()));
        no_waiters(true);
        Home { _dir: d }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
        no_waiters(false);
    }
}

fn mins(n: i64) -> i64 {
    n * 60 * 1_000_000_000
}

// remind_test.go:11 (TestParse)
#[test]
fn parses_waits_clocks_and_dates() {
    let now = local::date(2026, 10, 28, 14, 0, 0);
    let at = |mo, d, h, m| local::date(2026, mo, d, h, m, 0);
    for (input, want) in [
        ("25m", now.add_nanos(mins(25))),
        ("in 2h", now.add_nanos(mins(120))),
        ("1h30m", now.add_nanos(mins(90))),
        ("15:30", at(10, 28, 15, 30)),
        ("9:00", at(10, 29, 9, 0)), // passed today
        ("3pm", at(10, 28, 15, 0)),
        ("3:30pm", at(10, 28, 15, 30)),
        ("3:30 pm", at(10, 28, 15, 30)),
        ("tomorrow 9:00", at(10, 29, 9, 0)),
        ("Tomorrow at 3pm", at(10, 29, 15, 0)),
        ("2026-11-02 09:00", at(11, 2, 9, 0)),
        ("2026-11-02T09:00", at(11, 2, 9, 0)),
        (
            "2026-10-28T15:00:00Z",
            Time::parse("2026-10-28T15:00:00Z").unwrap(),
        ),
    ] {
        let got = parse(input, now);
        assert_eq!(got, Ok(want), "parse({input:?})");
    }
    for input in ["", "soon", "-5m", "2026-10-27 09:00", "13pm", "25:00"] {
        assert!(parse(input, now).is_err(), "parse({input:?}) = no error");
    }
}

/// Writes a reminder without starting a waiter, as add would.
fn save(session: &str, due: Time, now: Time) -> Reminder {
    add(session, "look at the deploy", due, now).unwrap()
}

// remind_test.go:48 (TestWaiterSendsWhenDueAndDrops)
#[test]
fn waiter_sends_when_due_and_drops() {
    let _h = Home::new();
    let mut now = Time::parse("2026-10-28T14:00:00Z").unwrap();
    let r = save("s1", now.add_nanos(150 * 1_000_000_000), now);
    let (mut slept, mut sent) = (Vec::new(), Vec::new());
    let clock = std::cell::Cell::new(now);
    Waiter {
        now: &mut || clock.get(),
        sleep: &mut |d| {
            slept.push(d);
            clock.set(clock.get() + d);
        },
        deliver: &mut |got, text| {
            sent.push(format!("{}: {text}", got.session));
            Ok(())
        },
        gone: &mut |_| false,
    }
    .run(&r.id)
    .unwrap();
    now = clock.get();
    let _ = now;
    assert_eq!(
        slept,
        [SLICE, SLICE, Duration::from_secs(30)],
        "slept {slept:?}"
    );
    assert!(
        sent.len() == 1
            && sent[0].contains("s1: <rush-reminder")
            && sent[0].contains("look at the deploy"),
        "sent {sent:?}"
    );
    assert!(list("s1").is_empty(), "a sent reminder is kept");
    let left: Vec<_> = std::fs::read_dir(dir())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&r.id))
        .collect();
    assert!(left.is_empty(), "left {left:?}");
}

// remind_test.go:82 (TestWaiterEndsWhenCancelled)
#[test]
fn waiter_ends_when_cancelled() {
    let _h = Home::new();
    let now = Time::now();
    let r = save("s1", now.add_nanos(mins(60)), now);
    let other = save("s2", now.add_nanos(mins(60)), now);
    assert!(
        cancel("s2", &r.id).is_err(),
        "one session cancelled another's reminder"
    );
    let id = r.id.clone();
    Waiter {
        now: &mut || now,
        sleep: &mut |_| {
            let _ = cancel("s1", &id);
        },
        deliver: &mut |_, _| panic!("a cancelled reminder was sent"),
        gone: &mut |_| false,
    }
    .run(&r.id)
    .unwrap();
    let got = list("");
    assert!(
        got.len() == 1 && got[0].id == other.id,
        "left {got:?}; want only {}",
        other.id
    );
}

// remind_test.go:106 (TestWaiterGivesUpAfterTries)
#[test]
fn waiter_gives_up_after_tries() {
    let _h = Home::new();
    let now = Time::now();
    let r = save("s1", now.add_nanos(1_000_000_000), now);
    let clock = std::cell::Cell::new(now);
    let mut n = 0;
    let got = Waiter {
        now: &mut || clock.get(),
        sleep: &mut |d| clock.set(clock.get() + d),
        deliver: &mut |_, _| {
            n += 1;
            Err("host down".into())
        },
        gone: &mut |_| false,
    }
    .run(&r.id);
    assert!(got.is_err(), "no error after every try failed");
    assert_eq!(n, TRIES, "tried {n} times");
}

// remind_test.go:126 (TestWaiterDropsAGoneSessions)
#[test]
fn waiter_drops_a_gone_sessions() {
    let _h = Home::new();
    let now = Time::now();
    let r = save("s1", now.add_nanos(mins(60)), now);
    let got = Waiter {
        now: &mut || now,
        sleep: &mut |_| panic!("slept for a gone session"),
        deliver: &mut |_, _| panic!("sent to a gone session"),
        gone: &mut |_| true,
    }
    .run(&r.id);
    assert!(
        got.is_ok() && list("").is_empty(),
        "run = {got:?}, left {:?}",
        list("")
    );
}

// remind_test.go:143 (TestMessageSaysWhenLate)
#[test]
fn message_says_when_late() {
    let due = Time::parse("2026-10-28T14:00:00Z").unwrap();
    let r = Reminder {
        id: "ab".into(),
        text: "x".into(),
        due,
        created: due.add_nanos(-mins(60)),
        ..Default::default()
    };
    assert!(
        !message(&r, due.add_nanos(mins(1))).contains("late"),
        "a minute is called late"
    );
    let m = message(&r, due.add_nanos(mins(60)));
    assert!(m.contains("1h late"), "{m}");
}

#[test]
fn lines_and_spans() {
    let now = local::date(2026, 10, 28, 14, 0, 0);
    assert_eq!(until(now.add_nanos(mins(25)), now), "in 25m");
    assert_eq!(until(now.add_nanos(mins(90)), now), "in 1h30m");
    assert_eq!(until(now.add_nanos(mins(72 * 60)), now), "in 3d");
    assert_eq!(until(now.add_nanos(20 * 1_000_000_000), now), "now");
    let r = Reminder {
        id: "ab12".into(),
        text: format!("{}\nmore", "y".repeat(90)),
        due: local::date(2026, 10, 28, 15, 30, 0),
        ..Default::default()
    };
    assert_eq!(
        line(&r, now),
        format!("ab12  due Wed 28 Oct 15:30 (in 1h30m)  {}…", "y".repeat(79))
    );
}

#[test]
fn too_far_and_empty_are_refused() {
    let _h = Home::new();
    let now = Time::now();
    assert!(add("s1", "  ", now.add_nanos(mins(5)), now).is_err());
    assert!(add("", "x", now.add_nanos(mins(5)), now).is_err());
    let e = add("s1", "x", now.add_nanos(mins(91 * 24 * 60)), now).unwrap_err();
    assert_eq!(e, "that is more than 90 days away");
}
