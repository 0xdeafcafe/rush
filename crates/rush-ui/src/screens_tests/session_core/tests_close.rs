//! Closing, restarting and putting away the agent of an open Session:
//! esc twice asks to close it, with restart beside yes; ctrl+x hides it
//! for good; alt+d puts it away in Earlier. After Go's hosted_test.go
//! (TestEscStopsThenAsksToClose), rushmode.go:4145-4152, :4370, :4440 and
//! logins.go:703 (restart).

use super::*;

/// A rush agent with no process of its own: stopping it goes through the
/// fake host, never a signal.
fn ours(state: State) -> Agent {
    let mut a = agent("steer", state);
    a.rush = true;
    a.pid = 0;
    a
}

fn more_keys(m: &Model) -> Vec<String> {
    m.confirm
        .as_ref()
        .map(|c| c.more.iter().map(|x| x.0.clone()).collect())
        .unwrap_or_default()
}

/// esc on a turn under way stops it; esc again soon after asks to close,
/// with restart, and r restarts: mid-turn it asks first, and the host is
/// told.
#[test]
fn esc_twice_asks_to_close_and_r_restarts() {
    let (mut m, fake) = hosted("steer", Vec::new());
    m.agents[0].rush = true;
    m.agents[0].pid = 0;
    working(&mut m, "go");
    press(&mut m, "esc");
    assert!(m.confirm.is_none(), "one esc doesn't ask");
    press(&mut m, "esc");
    let c = m.confirm.as_ref().expect("esc twice asks");
    assert_eq!(c.question, "Hide tidy the parser?");
    assert_eq!(c.yes_text, "hide");
    assert!(
        more_keys(&m).contains(&"r".to_string()),
        "{:?}",
        more_keys(&m)
    );
    press(&mut m, "r");
    assert_eq!(
        m.confirm.as_ref().map(|c| c.question.as_str()),
        Some("Restart tidy the parser?"),
        "mid-turn, restarting asks"
    );
    press(&mut m, "y");
    assert_eq!(
        *fake.restarted.lock().unwrap(),
        [("k1".to_string(), String::new())]
    );
    assert_eq!(
        flash(&m),
        "restarted tidy the parser · it starts again with your next message"
    );
}

/// u, beside restart, has rush restart under the agent and what it
/// spawned, its program running on: the host is asked, not the agent.
/// Rust-only.
#[test]
fn u_restarts_rush_under_the_agent() {
    let (mut m, fake) = hosted("steer", Vec::new());
    m.agents[0].rush = true;
    m.agents[0].pid = 0;
    working(&mut m, "go");
    press(&mut m, "esc");
    press(&mut m, "esc");
    assert!(
        more_keys(&m).contains(&"u".to_string()),
        "{:?}",
        more_keys(&m)
    );
    press(&mut m, "u");
    assert!(m.confirm.is_none(), "it asks nothing: nothing stops");
    assert_eq!(*fake.rehosted.lock().unwrap(), ["k1".to_string()]);
    assert!(fake.restarted.lock().unwrap().is_empty());
    assert_eq!(
        flash(&m),
        "rush restarts for tidy the parser · once it's between turns · its agent runs on"
    );
}

/// What #rehost says of what it asked, for each shape of tree.
#[test]
fn rehost_says_what_it_asked() {
    use crate::host::Rehost;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let l = |v: &[(&str, &str)]| {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect::<Vec<_>>()
    };
    let cases: Vec<(Rehost, Result<&str, &str>)> = vec![
        (
            Rehost {
                asked: s(&["p"]),
                now: 1,
                left: vec![],
            },
            Ok("rush restarts for P now · its agent runs on"),
        ),
        (
            Rehost {
                asked: s(&["p", "a", "b", "c"]),
                now: 1,
                left: vec![],
            },
            Ok(
                "rush restarts for P and its 3 agents · each once it's between turns · their agents run on",
            ),
        ),
        (
            Rehost {
                asked: s(&["p", "a"]),
                now: 0,
                left: l(&[("b", "it's asleep")]),
            },
            Ok(
                "rush restarts for P and the agent it spawned · each once it's between turns · their agents run on · 1 agent stays: it's asleep",
            ),
        ),
        (
            Rehost {
                asked: s(&["a", "b"]),
                now: 2,
                left: l(&[("p", "it runs on another machine"), ("c", "x"), ("d", "y")]),
            },
            Ok(
                "rush restarts for P's 2 agents now · their agents run on · P stays: it runs on another machine · 2 agents stay as they are",
            ),
        ),
        (
            Rehost {
                asked: vec![],
                now: 0,
                left: l(&[("p", "it isn't running")]),
            },
            Err("rush can't restart under P: it isn't running"),
        ),
    ];
    for (r, want) in &cases {
        let got = crate::screens::cmdbar::stops::rehost_said("P", "p", r);
        assert_eq!(got.as_deref().map_err(String::as_str), *want, "{r:?}");
    }
}

/// Something typed: esc clears it and doesn't count toward closing.
#[test]
fn esc_with_text_doesnt_count() {
    let fake = Fake::new();
    let mut m = open(ours(State::Idle), &fake, 120, 40);
    for c in "draft".chars() {
        press(&mut m, &c.to_string());
    }
    press(&mut m, "esc");
    assert!(m.confirm.is_none());
    assert_eq!(core(&mut m).esc_at, 0, "esc on text doesn't arm");
}

/// A limit stopped it: restart carries on at once with "continue", with
/// nothing to ask (logins.go:707, :738).
#[test]
fn restart_after_a_limit_continues() {
    let fake = Fake::new();
    let mut a = ours(State::Blocked);
    a.job.detail = "usage limit · resets 5pm".into();
    let mut m = open(a.clone(), &fake, 120, 40);
    crate::screens::cmdbar::stops::restart(&mut m, a, String::new());
    assert!(m.confirm.is_none());
    assert_eq!(
        *fake.restarted.lock().unwrap(),
        [("k1".to_string(), "continue".to_string())]
    );
    assert_eq!(flash(&m), "restarted tidy the parser");
}

/// One rush doesn't run is relaunched instead; an agent that can't move
/// its sessions says how to (relaunch).
#[test]
fn restart_outside_rush_relaunches() {
    let fake = Fake::new();
    let a = agent("steer", State::Idle);
    let mut m = open(a.clone(), &fake, 120, 40);
    crate::screens::cmdbar::stops::restart(&mut m, a, String::new());
    assert!(fake.restarted.lock().unwrap().is_empty());
    assert!(
        flash(&m).ends_with("can't move a session outside rush mode · /rush moves it over first"),
        "{}",
        flash(&m)
    );
}

/// ctrl+x in the Session hides its agent at once; one whose turn ended
/// with work it left running asks first (dismiss).
#[test]
fn ctrl_x_hides_the_agent() {
    let fake = Fake::new();
    let mut a = ours(State::Done);
    // A process of its own (busy needs one); rush stops it through its
    // host, so no signal is sent, and no process has this pid.
    a.pid = i32::MAX;
    a.job.in_flight = 1;
    a.job.background = vec!["task\0port the next round".into()];
    let mut m = open(a, &fake, 120, 40);
    m.roots = vec![String::new()];
    press(&mut m, "ctrl+x");
    assert_eq!(
        m.confirm.as_ref().map(|c| c.question.as_str()),
        Some("Stop tidy the parser and hide it?")
    );
    press(&mut m, "y");
    assert!(m.agents.is_empty(), "gone from the list");
    assert_eq!(*fake.stopped_all.lock().unwrap(), ["k1"]);
}

/// alt+d in the Session puts its agent away in Earlier, stopped with all
/// it started.
#[test]
fn alt_d_puts_it_away() {
    let fake = Fake::new();
    let mut m = open(ours(State::Done), &fake, 120, 40);
    m.roots = vec![String::new()];
    press(&mut m, "alt+d");
    assert!(m.agents[0].done);
    assert_eq!(*fake.stopped_all.lock().unwrap(), ["k1"]);
}

/// #reincarnate stops a rush agent with all it started and starts it
/// again at once, kept in the list; mid-turn it asks first; one rush
/// doesn't run says how to bring it over.
#[test]
fn reincarnate_stops_then_starts_again() {
    let fake = Fake::new();
    let mut m = open(ours(State::Idle), &fake, 120, 40);
    m.command("#reincarnate");
    assert!(m.confirm.is_none());
    assert_eq!(*fake.stopped_all.lock().unwrap(), ["k1".to_string()]);
    assert_eq!(
        *fake.wakes.lock().unwrap(),
        [(m.agents[0].key.clone(), String::new(), false)]
    );
    assert!(!m.agents[0].done, "it stays in the list");
    assert_eq!(
        flash(&m),
        "reincarnated tidy the parser · it carries on where it was"
    );

    let fake = Fake::new();
    let mut m = open(ours(State::Working), &fake, 120, 40);
    m.command("#reincarnate");
    assert_eq!(
        m.confirm.as_ref().map(|c| c.question.as_str()),
        Some("Reincarnate tidy the parser?")
    );
    assert!(
        fake.stopped_all.lock().unwrap().is_empty(),
        "not before yes"
    );
    press(&mut m, "y");
    assert_eq!(*fake.stopped_all.lock().unwrap(), ["k1".to_string()]);
    assert_eq!(fake.wakes.lock().unwrap().len(), 1);

    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    m.command("#reincarnate");
    assert!(fake.stopped_all.lock().unwrap().is_empty());
    assert_eq!(
        flash(&m),
        "tidy the parser isn't a rush session · #rush moves it over first"
    );
}
