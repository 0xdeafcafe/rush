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
