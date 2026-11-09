//! The host's Emeritus, driven through a kept conn: the nudge, the
//! retire op, and the handover's commit and roll back with a stand-in
//! for the successor's start.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rush_core::adapter::Conn;
use rush_core::event::{self, Exchange, Peer};
use rush_state::Time;

use super::super::tests::{Kept, host, with_conn};
use super::super::{Hooks, Host, build};
use crate::emeritus::{self as em, CALLED_OFF, NUDGE_START, Phase, SECTIONS, Start, read_journal};
use crate::fake::registry;
use crate::op::Op;
use crate::paths::dir;
use crate::subtool::{answer_path, report, report_path};
use crate::testing::Home;
use crate::wire::{Config, Emeritus, Task};

/// Turns Emeritus on, its threshold the default.
fn turn_on() {
    let mut s = rush_state::Store::load();
    s.config.emeritus.on = true;
    s.config.emeritus.tokens = 0;
    s.save_config().unwrap();
}

/// The threshold of the fake agent's sessions.
fn past() -> i64 {
    rush_state::Emeritus {
        on: true,
        ..Default::default()
    }
    .threshold(registry().context_window("fake", ""))
}

fn baton() -> String {
    SECTIONS
        .iter()
        .map(|s| format!("## {s}\n{s} of the task."))
        .collect::<Vec<_>>()
        .join("\n")
}

fn cfg(id: &str, f: impl FnOnce(&mut Config)) -> Config {
    let mut c = Config {
        id: id.into(),
        kind: "fake".into(),
        session_id: format!("s-{id}"),
        resume: true,
        cwd: "/tmp".into(),
        ..Config::default()
    };
    f(&mut c);
    c
}

/// Ends the turn under way.
fn turn_end(h: &Host, conn: &Arc<dyn Conn>, reason: &str) {
    let mut st = h.lock();
    st.info.state = "working".into();
    h.on_turn_end(
        &mut st,
        conn,
        event::TurnEnd {
            reason: reason.into(),
            ..Default::default()
        },
    );
}

fn sent(c: &Kept) -> Vec<String> {
    c.got
        .lock()
        .unwrap()
        .iter()
        .map(|i| i.text.clone())
        .collect()
}

fn exchange(id: &str, text: &str) -> Op {
    let mut o = Op::named("send");
    o.text = text.into();
    o.exchange = Some(Box::new(Exchange {
        id: id.into(),
        direction: "received".into(),
        text: text.into(),
        sender: Peer {
            session_id: "par1".into(),
            name: "Parent".into(),
            ..Peer::default()
        },
        ..Exchange::default()
    }));
    o
}

fn typed(text: &str) -> Op {
    let mut o = Op::named("send");
    o.text = text.into();
    o
}

// The host owns the trigger: at a turn's end it nudges once, past the
// threshold, while the cache is warm, and with nothing waiting on the
// user; an agent another started isn't nudged once its task is done.
#[test]
fn the_nudge_at_a_turns_end() {
    struct Case {
        name: &'static str,
        on: bool,
        over: i64,
        warm: bool,
        spawned: bool,
        nudged_at: Option<i64>,
        want: bool,
    }
    let base = Case {
        name: "",
        on: true,
        over: 1,
        warm: true,
        spawned: false,
        nudged_at: None,
        want: true,
    };
    let cases = [
        Case {
            name: "past it, warm",
            ..base
        },
        Case {
            name: "short of it",
            over: -1,
            want: false,
            ..base
        },
        Case {
            name: "cache lapsed",
            warm: false,
            want: false,
            ..base
        },
        Case {
            name: "off",
            on: false,
            want: false,
            ..base
        },
        Case {
            name: "another's subagent, its task done",
            spawned: true,
            want: false,
            ..base
        },
        Case {
            name: "nudged at this context",
            nudged_at: Some(0),
            want: false,
            ..base
        },
        Case {
            name: "nudged, and grown a quarter since",
            nudged_at: Some(past() / em::RENUDGE_SHARE + 1),
            ..base
        },
    ];
    for (n, c) in cases.iter().enumerate() {
        let _home = Home::new();
        if c.on {
            turn_on();
        }
        let ctx = past() + c.over;
        let h = host(cfg(&format!("nudge{n}"), |cf| {
            if c.spawned {
                cf.meta.insert("spawnedBy".into(), "par1".into());
            }
            if let Some(grown) = c.nudged_at {
                cf.emeritus = Some(Emeritus {
                    nudged_at: Time::now(),
                    nudged_context: ctx - grown,
                    ..Emeritus::default()
                });
            }
        }));
        let k = Arc::new(Kept::default());
        let conn: Arc<dyn Conn> = k.clone();
        with_conn(&h, conn.clone());
        {
            let mut st = h.lock();
            st.info.context_tokens = ctx;
            st.info.cache_warm = if c.warm {
                Time::now() + Duration::from_secs(300)
            } else {
                Time::from_ms(Time::now().to_ms() - 1000)
            };
        }
        turn_end(&h, &conn, "done");
        let got = sent(&k);
        let nudged = got.iter().any(|t| t.contains(NUDGE_START));
        assert_eq!(nudged, c.want, "{}: {got:?}", c.name);
        if c.want {
            let st = h.lock();
            let e = st.cfg.emeritus.clone().unwrap();
            assert!(
                e.nudged_context == ctx && e.nudged_past == past(),
                "{}",
                c.name
            );
            drop(st);
            // Once is enough: the next turn's end isn't nudged again.
            turn_end(&h, &conn, "done");
            let again = sent(&k).iter().filter(|t| t.contains(NUDGE_START)).count();
            assert_eq!(again, 1, "{}: nudged twice", c.name);
        }
    }
}

// New work that comes to a subagent past the threshold waits behind a
// nudge, its cache still warm; the nudge's turn answers no one, and the
// work goes once it ends.
#[test]
fn new_work_waits_behind_a_nudge() {
    let _home = Home::new();
    turn_on();
    let h = host(cfg("kidn", |c| {
        c.meta.insert("spawnedBy".into(), "par1".into());
    }));
    let k = Arc::new(Kept::default());
    let conn: Arc<dyn Conn> = k.clone();
    with_conn(&h, conn.clone());
    {
        let mut st = h.lock();
        st.info.context_tokens = past() + 10;
        st.info.cache_warm = Time::now() + Duration::from_secs(300);
    }
    h.do_op(exchange("x1", "the next task")).unwrap();
    let got = sent(&k);
    assert!(got.len() == 1 && got[0].contains(NUDGE_START), "{got:?}");
    assert_eq!(h.lock().info.queue, ["the next task"]);
    turn_end(&h, &conn, "done");
    let got = sent(&k);
    assert!(
        got.len() == 2 && got[1].contains("the next task"),
        "{got:?}"
    );
    assert!(!answer_path("kidn").exists(), "the nudge's turn answered");
}

/// The host, its kept conn, the successors' starts, and their answers.
type Handing = (
    Host,
    Arc<Kept>,
    Arc<dyn Conn>,
    Receiver<Config>,
    Sender<Start>,
);

/// A host whose successor's start is the test's to settle: each start
/// is sent on starts, and waits for go's answer.
fn handing_host(id: &str, f: impl FnOnce(&mut Config)) -> Handing {
    let (starts_tx, starts) = channel::<Config>();
    let (go, go_rx) = channel::<Start>();
    let (starts_tx, go_rx) = (Mutex::new(starts_tx), Mutex::new(go_rx));
    let hooks = Hooks {
        successor: Some(Arc::new(move |b: Config| {
            // Its host would write its config first.
            std::fs::create_dir_all(dir(&b.id)).unwrap();
            em::write_config(&b).unwrap();
            starts_tx.lock().unwrap().send(b).unwrap();
            go_rx.lock().unwrap().recv().unwrap()
        })),
        ..Hooks::quiet()
    };
    let c = cfg(id, f);
    std::fs::create_dir_all(dir(id)).unwrap();
    em::write_config(&c).unwrap();
    let (h, _rx) = build(registry(), hooks, c);
    let k = Arc::new(Kept::default());
    let conn: Arc<dyn Conn> = k.clone();
    with_conn(&h, conn.clone());
    {
        let mut st = h.lock();
        st.info.state = "working".into();
        st.info.context_tokens = 310_000;
        st.info.cost_usd = 15.5;
    }
    (h, k, conn, starts, go)
}

/// Waits for h's handover to be settled.
fn settled(h: &Host) {
    let mut st = h.lock();
    while st.em.settled == 0 {
        st = h.cv.wait(st).unwrap();
    }
}

// The handover is a transaction. Asked mid-turn, it waits for the turn's
// end; frozen, the session refuses what you type; once the successor's
// first request is back it commits: retired for good, its children,
// reports and the work that waited for it moved to the successor, the
// handover logged for the Efficiency place. Then nothing gets in.
#[test]
fn a_handover_commits_and_retires_for_good() {
    let _home = Home::new();
    turn_on();
    let (h, _k, conn, starts, go) = handing_host("hova", |c| {
        c.meta.insert("spawnedBy".into(), "par1".into());
        c.model = "opus".into();
    });
    std::fs::create_dir_all(dir("kidh")).unwrap();
    em::write_config(&cfg("kidh", |c| {
        c.meta.insert("spawnedBy".into(), "hova".into());
    }))
    .unwrap();
    report("kidh", "hova").unwrap();
    report("hova", "par1").unwrap();
    let mut o = Op::named("retire");
    o.text = baton();
    h.do_op(o).unwrap();
    assert_eq!(h.lock().info.emeritus, "retiring");
    assert!(read_journal("hova").is_none(), "it handed over mid-turn");
    // Work for it that comes during its last turn is its successor's.
    h.do_op(exchange("x1", "and then the docs")).unwrap();
    turn_end(&h, &conn, "done");
    let b = starts.recv().unwrap();
    {
        let st = h.lock();
        assert!(st.em.handing && st.info.emeritus == "handing");
    }
    assert_eq!(read_journal("hova").unwrap().phase, Phase::Starting);
    let err = h.do_op(typed("are you there?")).unwrap_err();
    assert!(err.contains("handing over"), "{err}");
    go.send(Start::Started(22_000)).unwrap();
    settled(&h);

    // The successor: where and as it worked, on its baton.
    let got = (
        b.meta["predecessor"].as_str(),
        b.meta["lineage"].as_str(),
        b.meta["spawnedBy"].as_str(),
        b.model.as_str(),
        b.resume,
    );
    assert_eq!(got, ("hova", "1", "par1", "opus", false));
    for want in [
        "## Destination",
        "## Tests that pin text",
        "## State (recorded by rush)",
        "From Parent (session par1):\nand then the docs",
    ] {
        assert!(b.prompt.contains(want), "{want} in {}", b.prompt);
    }
    assert!(em::baton_path("hova").exists());

    // Committed: retired for good, what was its moved.
    let a = crate::client::read_config("hova").unwrap();
    assert!(a.retired() && a.emeritus.as_ref().unwrap().successor == b.id);
    let j = read_journal("hova").unwrap();
    assert!(j.phase == Phase::Committed && j.to == b.id && j.from_context == 310_000);
    let kid = crate::client::read_config("kidh").unwrap();
    assert_eq!(kid.meta["spawnedBy"], b.id);
    assert_eq!(std::fs::read_to_string(report_path("kidh")).unwrap(), b.id);
    assert_eq!(std::fs::read_to_string(report_path(&b.id)).unwrap(), "par1");
    assert!(!report_path("hova").exists());
    assert!(
        !answer_path("hova").exists(),
        "its last turn answered for it"
    );
    {
        let st = h.lock();
        assert!(st.quit && st.info.state == "stopped" && st.info.successor == b.id);
        assert!(st.info.queue.is_empty(), "the work stayed behind");
    }
    let evs = rush_efficiency::events::load_events();
    let hs: Vec<_> = evs.iter().filter_map(|e| e.handover.as_ref()).collect();
    assert!(
        hs.len() == 1
            && hs[0].from == "hova"
            && hs[0].to == b.id
            && hs[0].from_context == 310_000
            && hs[0].from_cost_usd == 15.5
            && hs[0].to_context == 22_000
            && hs[0].subagent,
        "{hs:?}"
    );

    // Terminal: nothing gets in, nothing wakes it.
    for o in [typed("hello?"), exchange("x2", "more"), Op::named("rewind")] {
        let err = h.do_op(o).unwrap_err();
        assert!(err.contains("is retired") && err.contains(&b.id), "{err}");
    }
    let err = crate::client::ensure("hova", "").unwrap_err().to_string();
    assert!(err.contains("is retired"), "{err}");
}

// A successor that can't start is retired instead, and the session
// carries on, told why: what you type goes to it again, and the work
// that waited goes to it.
#[test]
fn a_failed_start_rolls_back() {
    let _home = Home::new();
    turn_on();
    let (h, k, conn, starts, go) = handing_host("hovr", |_| {});
    let mut o = Op::named("retire");
    o.text = baton();
    h.do_op(o).unwrap();
    turn_end(&h, &conn, "done");
    let b = starts.recv().unwrap();
    h.do_op(exchange("x1", "a late question")).unwrap();
    go.send(Start::Failed("couldn't start: not signed in".into()))
        .unwrap();
    settled(&h);
    let a = crate::client::read_config("hovr").unwrap();
    assert!(!a.retired(), "rolled back, but retired");
    let j = read_journal("hovr").unwrap();
    assert!(
        j.phase == Phase::RolledBack && j.why.contains("not signed in"),
        "{j:?}"
    );
    let v = crate::client::read_config(&b.id).unwrap();
    assert!(
        v.retired() && v.emeritus.as_ref().unwrap().why.contains("rolled back"),
        "{v:?}"
    );
    let got = sent(&k);
    assert!(
        got.last()
            .unwrap()
            .contains("your handover didn't happen: your successor couldn't start: not signed in"),
        "{got:?}"
    );
    {
        let st = h.lock();
        assert!(!st.em.handing && st.info.emeritus.is_empty());
        assert_eq!(st.info.queue, ["a late question"]);
    }
    // The note's turn ends; what waited goes to it, and so does yours.
    turn_end(&h, &conn, "done");
    assert!(sent(&k).last().unwrap().contains("a late question"));
    turn_end(&h, &conn, "done");
    h.do_op(typed("carry on")).unwrap();
    assert_eq!(sent(&k).last().unwrap(), "carry on");
}

// A retire is called off, not carried out, when the turn was stopped,
// when you wrote to it meanwhile (that's for it, never another), or
// while its background tasks run; and refused while Emeritus is off.
#[test]
fn a_retire_called_off() {
    let cases: &[(&str, &str, bool, bool, &str)] = &[
        ("interrupted", "interrupted", false, false, ""),
        ("you wrote", "done", true, false, CALLED_OFF),
        (
            "background tasks",
            "done",
            false,
            true,
            "waits for your background tasks",
        ),
    ];
    for (n, (name, reason, wrote, tasks, said)) in cases.iter().enumerate() {
        let _home = Home::new();
        turn_on();
        let h = host(cfg(&format!("off{n}"), |_| {}));
        let k = Arc::new(Kept::default());
        let conn: Arc<dyn Conn> = k.clone();
        with_conn(&h, conn.clone());
        h.lock().info.state = "working".into();
        let mut o = Op::named("retire");
        o.text = baton();
        h.do_op(o).unwrap();
        if *wrote {
            h.do_op(typed("wait, one more thing")).unwrap();
        }
        if *tasks {
            h.lock().info.background = vec![Task {
                id: "t1".into(),
                ..Task::default()
            }];
        }
        turn_end(&h, &conn, reason);
        let st = h.lock();
        assert!(
            !st.em.handing && st.em.due.is_none() && st.info.emeritus.is_empty(),
            "{name}"
        );
        assert!(
            read_journal(&format!("off{n}")).is_none(),
            "{name}: handed over"
        );
        drop(st);
        let got = sent(&k);
        if !said.is_empty() {
            assert!(got.last().unwrap().contains(said), "{name}: {got:?}");
        }
        if *wrote {
            assert!(
                got.last().unwrap().contains("wait, one more thing"),
                "{name}: {got:?}"
            );
        }
    }
    let _home = Home::new();
    let h = host(cfg("offx", |_| {}));
    let mut o = Op::named("retire");
    o.text = baton();
    let err = h.do_op(o).unwrap_err();
    assert!(err.contains("Emeritus is off"), "{err}");
    let mut o = Op::named("retire");
    turn_on();
    o.text = "## Destination\nonly this".into();
    let err = h.do_op(o).unwrap_err();
    assert!(err.contains("lacks Core ideas"), "{err}");
}
