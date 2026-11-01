use super::*;

const T0: Ms = 1_800_000_000_000;

/// Readings every 10 minutes from `from` before T0 to T0, the week going
/// from a to b evenly, resetting at reset.
fn steady(from: Ms, a: f64, b: f64, reset: Ms) -> Vec<Sample> {
    let n = (from / (10 * MIN)) as usize;
    (0..=n)
        .map(|i| {
            let t = T0 - from + i as Ms * 10 * MIN;
            Sample(t, a + (b - a) * i as f64 / n as f64, reset)
        })
        .collect()
}

fn on() -> PaceCfg {
    PaceCfg::default()
}

#[test]
fn pace_against_a_reset_far_and_close() {
    // 6% of the week in the last 2 hours: 3% an hour, with 60% left.
    for (name, reset_in, ratio, level) in [
        (
            "reset in 100h: 0.6%/h is fair",
            100 * HOUR,
            5.0,
            Level::Over,
        ),
        ("reset in 30h: 2%/h is fair", 30 * HOUR, 1.5, Level::Warn),
        ("reset in 10h: 6%/h is fair", 10 * HOUR, 0.5, Level::Under),
        // Under 15 minutes left counts as 15: it's spent or lost anyway.
        ("reset in 5m", 5 * MIN, 0.0125, Level::Under),
    ] {
        let v = steady(2 * HOUR, 34.0, 40.0, T0 + reset_in);
        let p = pace(&v, &on(), false, T0);
        assert!((p.burn - 3.0).abs() < 1e-9, "{name}: burn {}", p.burn);
        assert!((p.ratio - ratio).abs() < 1e-9, "{name}: ratio {}", p.ratio);
        assert_eq!(p.level, level, "{name}");
        assert_eq!(p.frees_at != 0, level == Level::Over, "{name}");
    }
}

#[test]
fn pace_needs_half_an_hour_read() {
    let v = steady(20 * MIN, 30.0, 40.0, T0 + 100 * HOUR);
    let p = pace(&v, &on(), false, T0);
    assert_eq!(p.level, Level::Unknown);
    assert!(p.fair > 0.0);
    assert_eq!(pace(&[], &on(), false, T0).level, Level::Unknown);
}

#[test]
fn tripped_stays_until_clearly_under() {
    // 1.5×: a warning, unless it had tripped, when it stays over until
    // under 1.2×.
    let v = steady(2 * HOUR, 34.0, 40.0, T0 + 30 * HOUR);
    assert_eq!(pace(&v, &on(), false, T0).level, Level::Warn);
    assert_eq!(pace(&v, &on(), true, T0).level, Level::Over);
    let v = steady(2 * HOUR, 36.0, 40.0, T0 + 30 * HOUR); // 1.0×
    assert_eq!(pace(&v, &on(), true, T0).level, Level::Under);
}

#[test]
fn a_new_week_counts_from_nothing() {
    let mut v = steady(2 * HOUR, 90.0, 99.0, T0 + 10 * MIN);
    // The week resets an hour ago: after it, 1% then 2%.
    v.retain(|s| s.0 < T0 - HOUR);
    v.push(Sample(T0 - 30 * MIN, 1.0, T0 + 167 * HOUR));
    v.push(Sample(T0, 2.0, T0 + 167 * HOUR));
    let p = pace(&v, &on(), false, T0);
    // 90 → 93.75 before it, then 1 and 1 more: the drop to 1 adds 1, not
    // -92.75. 5.75 over 2 hours.
    assert!((p.burn - 5.75 / 2.0).abs() < 1e-9, "{}", p.burn);
    assert_eq!(p.week, 2.0);
}

#[test]
fn over_frees_up_as_its_use_ages_out() {
    // 10% in the last hour, nothing the hour before; reset in 100h.
    let mut v = steady(HOUR, 30.0, 40.0, T0 + 100 * HOUR);
    v.insert(0, Sample(T0 - 2 * HOUR, 30.0, T0 + 100 * HOUR));
    let c = on();
    let p = pace(&v, &c, false, T0);
    assert_eq!(p.level, Level::Over);
    assert!(
        p.frees_at > T0 && p.frees_at <= T0 + 2 * HOUR,
        "{}",
        p.frees_at
    );
    // No more use: just after it frees up, it's under release.
    let mut later = v.clone();
    later.push(Sample(p.frees_at + MIN, 40.0, T0 + 100 * HOUR));
    let q = pace(&later, &c, true, p.frees_at + MIN);
    assert!(q.ratio < c.release, "{q:?}");
    // Just before, still over.
    let q = pace(&v, &c, true, p.frees_at - MIN);
    assert_eq!(q.level, Level::Over, "{q:?}");
}

#[test]
fn readings_kept_and_pruned() {
    let mut h = History::default();
    assert!(h.note("a", Sample(T0, 10.0, T0), T0));
    assert!(!h.note("a", Sample(T0, 10.0, T0), T0), "the same reading");
    assert!(h.note("a", Sample(T0 + 2 * HOUR, 12.0, T0), T0 + 2 * HOUR));
    assert!(h.note("a", Sample(T0 + HOUR, 11.0, T0), T0 + 2 * HOUR));
    let ts: Vec<Ms> = h.of("a").iter().map(|s| s.0 - T0).collect();
    assert_eq!(ts, [0, HOUR, 2 * HOUR]);
    // At 4h30 the one at 1h is where 3 hours back begins.
    assert!(h.note(
        "a",
        Sample(T0 + 4 * HOUR + 30 * MIN, 13.0, T0),
        T0 + 4 * HOUR + 30 * MIN
    ));
    let ts: Vec<Ms> = h.of("a").iter().map(|s| s.0 - T0).collect();
    assert_eq!(ts, [HOUR, 2 * HOUR, 4 * HOUR + 30 * MIN]);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(FILE);
    h.save(&p).unwrap();
    assert_eq!(History::load(&p), h);
    assert!(h.keep_only(&["b"]));
    assert!(h.0.is_empty());
}

// ---- What the guard does ----

fn acct(id: &str, current: bool, ratio: f64, level: Level) -> Acct {
    Acct {
        id: id.into(),
        name: id.into(),
        current,
        pace: Pace {
            ratio,
            level,
            week: 40.0,
            frees_at: if level == Level::Over { T0 + HOUR } else { 0 },
            ..Pace::default()
        },
        ..Acct::default()
    }
}

fn seat(id: &str, login: &str) -> Seat {
    Seat {
        id: id.into(),
        login: login.into(),
        turns: 5,
        context: 412_000,
        ..Seat::default()
    }
}

fn working(id: &str, login: &str, agents: usize) -> Seat {
    Seat {
        working: true,
        agents,
        ..seat(id, login)
    }
}

fn pace_on() -> Settings {
    Settings {
        pace: Some(PaceCfg::default()),
        move_gap: 6 * HOUR,
        ..Settings::default()
    }
}

fn did<'a>(p: &'a Plan, id: &str) -> &'a Do {
    &p.seats.iter().find(|s| s.0 == id).unwrap().1
}

fn held(d: &Do) -> bool {
    matches!(d, Do::Hold(_))
}

#[test]
fn off_unless_set() {
    let s = Settings::default();
    assert!(!s.on());
    let accts = [
        acct("a", true, 9.0, Level::Over),
        acct("b", false, 0.1, Level::Under),
    ];
    let seats = [
        working("s1", "a", 30),
        seat("s2", "a"),
        Seat {
            turns: 0,
            ..seat("s3", "a")
        },
    ];
    let p = plan(&s, &accts, &seats, 50, true, T0);
    assert!(p.seats.iter().all(|(_, d)| *d == Do::Go), "{p:?}");
    assert_eq!((p.new_on, p.spawns, p.cap), (None, None, 0));
    assert!(p.over.is_empty());
}

#[test]
fn a_new_session_starts_on_the_account_with_the_most_room() {
    let accts = [
        acct("a", true, 2.4, Level::Over),
        acct("b", false, 0.9, Level::Under),
        acct("c", false, 0.3, Level::Under),
        Acct {
            borrowed: true,
            ..acct("d", false, 0.0, Level::Under)
        },
        Acct {
            out: true,
            ..acct("e", false, 0.0, Level::Under)
        },
    ];
    let new = Seat {
        turns: 0,
        ..seat("new", "a")
    };
    let p = plan(&pace_on(), &accts, &[new], 0, false, T0);
    assert_eq!(p.new_on.as_deref(), Some("c"));
    match did(&p, "new") {
        Do::Move { to, cold, .. } => assert_eq!((to.as_str(), *cold), ("c", true)),
        d => panic!("{d:?}"),
    }
}

#[test]
fn a_warm_session_waits_and_a_cold_one_moves() {
    let accts = [
        acct("a", true, 2.4, Level::Over),
        acct("b", false, 0.5, Level::Under),
    ];
    let warm = Seat {
        warm_until: T0 + 4 * MIN,
        ..seat("warm", "a")
    };
    let cold = Seat {
        warm_until: T0 - MIN,
        ..seat("cold", "a")
    };
    let p = plan(&pace_on(), &accts, &[warm, cold], 0, false, T0);
    match did(&p, "warm") {
        Do::Hold(Hold::Pace { acct, why, .. }) => {
            assert_eq!(acct, "a");
            assert!(
                why.contains("cache is warm") && why.contains("412000 tokens"),
                "{why}"
            );
        }
        d => panic!("{d:?}"),
    }
    match did(&p, "cold") {
        Do::Move {
            to,
            cold,
            context,
            why,
            ..
        } => {
            assert_eq!((to.as_str(), *cold, *context), ("b", true, 412_000));
            assert!(why.contains("cache is cold"), "{why}");
        }
        d => panic!("{d:?}"),
    }
}

#[test]
fn no_second_move_inside_the_window() {
    let accts = [
        acct("a", true, 2.4, Level::Over),
        acct("b", false, 0.5, Level::Under),
    ];
    let moved = Seat {
        moved_at: T0 - 5 * HOUR,
        ..seat("s", "a")
    };
    let p = plan(
        &pace_on(),
        &accts,
        std::slice::from_ref(&moved),
        0,
        false,
        T0,
    );
    match did(&p, "s") {
        Do::Hold(Hold::Pace { why, .. }) => assert!(why.contains("once already"), "{why}"),
        d => panic!("{d:?}"),
    }
    let later = Seat {
        moved_at: T0 - 7 * HOUR,
        ..moved
    };
    let p = plan(&pace_on(), &accts, &[later], 0, false, T0);
    assert!(matches!(did(&p, "s"), Do::Move { .. }));
}

#[test]
fn work_waits_when_every_account_is_past_its_pace() {
    // b is under the trip but not clearly under its pace: no move there.
    let accts = [
        acct("a", true, 2.4, Level::Over),
        acct("b", false, 1.5, Level::Warn),
    ];
    let seats = [
        seat("s", "a"),
        Seat {
            turns: 0,
            ..seat("new", "a")
        },
    ];
    let p = plan(&pace_on(), &accts, &seats, 0, false, T0);
    assert_eq!(p.new_on, None);
    assert_eq!(p.all_paced, Some(T0 + HOUR));
    for id in ["s", "new"] {
        assert_eq!(
            *did(&p, id),
            Do::Hold(Hold::AllPaced {
                frees_at: T0 + HOUR
            }),
            "{id}"
        );
    }
}

#[test]
fn no_flip_flop_near_the_line() {
    // Moved to b; now b is over, and a, which tripped, is only back to
    // 1.5×: nothing moves back to a until it's under 1.2×.
    let accts = [
        acct("a", false, 1.5, Level::Warn),
        acct("b", true, 2.1, Level::Over),
    ];
    let p = plan(
        &pace_on(),
        &accts,
        &[Seat {
            turns: 0,
            ..seat("new", "b")
        }],
        0,
        false,
        T0,
    );
    assert_eq!(p.new_on, None);
    assert!(held(did(&p, "new")));
    // Hysteresis in the pace itself: b at 1.9× having tripped is still over.
    let v = steady(2 * HOUR, 34.0, 40.0, T0 + 33 * HOUR);
    let r = pace(&v, &PaceCfg::default(), true, T0);
    assert!(r.ratio > 1.2 && r.ratio < 2.0, "{r:?}");
    assert_eq!(r.level, Level::Over);
    let accts = [
        acct("a", false, 1.1, Level::Under),
        acct("b", true, 2.1, Level::Over),
    ];
    let p = plan(
        &pace_on(),
        &accts,
        &[Seat {
            turns: 0,
            ..seat("new", "b")
        }],
        0,
        false,
        T0,
    );
    assert_eq!(p.new_on.as_deref(), Some("a"));
}

#[test]
fn a_running_turn_is_never_cut() {
    // Whatever trips, a working session is never moved: at most its next
    // turn waits.
    let accts = [
        acct("a", true, 2.4, Level::Over),
        acct("b", false, 0.5, Level::Under),
    ];
    let s = Settings {
        cap: 1,
        away: Some(AwayCfg {
            after: 20 * MIN,
            cap: 1,
        }),
        ..pace_on()
    };
    for away in [false, true] {
        let seats = [
            working("w", "a", 3),
            Seat {
                turns: 0,
                ..working("wnew", "a", 1)
            },
            Seat {
                warm_until: 0,
                ..working("wcold", "a", 1)
            },
        ];
        let p = plan(&s, &accts, &seats, 4, away, T0);
        for (id, d) in &p.seats {
            assert!(matches!(d, Do::Hold(_)), "away {away}: {id}: {d:?}");
        }
    }
}

#[test]
fn the_cap_queues_and_releases() {
    let s = Settings {
        cap: 3,
        ..Settings::default()
    };
    // 2 + 1 working: the third is held, and spawns are refused.
    let seats = [
        working("a", "x", 2),
        working("b", "x", 1),
        Seat {
            queued: true,
            ..seat("c", "x")
        },
    ];
    let p = plan(&s, &[], &seats, 0, false, T0);
    assert_eq!(p.working, 3);
    assert_eq!(*did(&p, "c"), Do::Hold(Hold::Cap { working: 3, cap: 3 }));
    assert_eq!(*did(&p, "a"), Do::Go);
    assert!(p.spawns.as_deref().unwrap().contains("3 of 3"));
    // Agents outside rush's sessions count too.
    let p = plan(&s, &[], &seats[1..], 2, false, T0);
    assert!(held(did(&p, "c")));

    // a finishes: one working, room for two. c and d wait with messages,
    // e with none: released in the order held, as far as there's room.
    let seats = [
        seat("a", "x"),
        working("b", "x", 2),
        Seat {
            queued: true,
            held_since: T0 - 2 * MIN,
            ..seat("d", "x")
        },
        Seat {
            queued: true,
            held_since: T0 - 5 * MIN,
            ..seat("c", "x")
        },
        Seat {
            held_since: T0 - MIN,
            ..seat("e", "x")
        },
    ];
    let p = plan(&s, &[], &seats, 0, false, T0);
    assert_eq!(p.spawns, None);
    assert_eq!(*did(&p, "c"), Do::Release, "held first, goes first");
    assert!(held(did(&p, "d")), "no room left for d");
    assert_eq!(
        *did(&p, "e"),
        Do::Release,
        "nothing waiting: nothing starts"
    );
    assert_eq!(*did(&p, "a"), Do::Go);
}

#[test]
fn away_tightens_then_relaxes() {
    let s = Settings {
        cap: 6,
        away: Some(AwayCfg {
            after: 20 * MIN,
            cap: 2,
        }),
        ..Settings::default()
    };
    let seats = [
        working("w", "x", 1),
        seat("idle", "x"),
        Seat {
            keeps_on: true,
            ..seat("loop", "x")
        },
    ];
    let p = plan(&s, &[], &seats, 0, true, T0);
    assert_eq!(p.cap, 2);
    assert!(p.spawns.as_deref().unwrap().contains("away"));
    assert_eq!(
        *did(&p, "w"),
        Do::Hold(Hold::Away),
        "its turn ends, then holds"
    );
    assert_eq!(*did(&p, "idle"), Do::Hold(Hold::Away));
    assert_eq!(*did(&p, "loop"), Do::Go, "set to work on while you're away");
    // Back: what was held for it is let go.
    let back: Vec<Seat> = seats
        .iter()
        .map(|x| Seat {
            held_since: if x.keeps_on { 0 } else { T0 - MIN },
            ..x.clone()
        })
        .collect();
    let p = plan(&s, &[], &back, 0, false, T0);
    assert_eq!((p.cap, p.spawns.clone()), (6, None));
    assert_eq!(*did(&p, "idle"), Do::Release);
    assert_eq!(*did(&p, "w"), Do::Release);
    // Away while off: nothing.
    let p = plan(&Settings { away: None, ..s }, &[], &seats, 0, true, T0);
    assert!(p.seats.iter().all(|(_, d)| *d == Do::Go));
}

#[test]
fn let_through_once() {
    let accts = [acct("a", true, 2.4, Level::Over)];
    let x = Seat {
        let_through: true,
        held_since: T0 - MIN,
        ..seat("s", "a")
    };
    let p = plan(&pace_on(), &accts, &[x], 0, false, T0);
    assert_eq!(*did(&p, "s"), Do::Release);
}

#[test]
fn holds_said() {
    let clock = |_: Ms| "14:20".to_string();
    let h = Hold::Pace {
        acct: "alex".into(),
        ratio: 2.06,
        frees_at: T0,
        why: String::new(),
    };
    assert_eq!(
        h.say(&clock),
        "held: past your pace on alex · 2.1× · frees at 14:20"
    );
    assert_eq!(
        Hold::Cap { working: 6, cap: 6 }.say(&clock),
        "held: 6 of 6 agents working"
    );
}
