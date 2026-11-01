//! rush-ui-session-core's finish.rs tests: they make a whole UI model.

use super::finish::*;
use crate::model::Model;
use rush_core::agent::{Agent, State};

fn finished(m: &mut Model) -> usize {
    m.kept::<Finish>()
        .events
        .iter()
        .filter(|e| e.what == "finished")
        .count()
}

fn read(m: &mut Model, a: &Agent) {
    m.agents = vec![a.clone()];
    on_fleet(m);
}

// finishnote_test.go: a turn that ends with agents still running in
// the background hasn't finished; it's noted finished once they're
// done, not when the turn ends.
#[test]
fn finished_waits_for_background_work() {
    let mut m = crate::new_model("test");
    m.clock = Some(1_790_000_000_000);
    let mut a = Agent {
        key: "k".into(),
        display_name: "porter".into(),
        pid: std::process::id() as i32,
        ..Agent::default()
    };
    a.job.state = State::Working;
    read(&mut m, &a); // the first read only learns what's there
    read(&mut m, &a);
    a.job.state = State::Done;
    a.job.in_flight = 1;
    a.job.background = vec!["task\0port the next round".into()];
    read(&mut m, &a);
    assert_eq!(
        finished(&mut m),
        0,
        "noted finished while its workflow runs"
    );
    a.job.in_flight = 0;
    a.job.background.clear();
    read(&mut m, &a);
    assert_eq!(finished(&mut m), 1, "noted once its workflow ended");
    assert_eq!(m.kept::<Finish>().fx.kind, Fx::Done, "and clanker reacted");
}

fn fx(m: &mut Model) -> Fx {
    m.kept::<Finish>().fx.kind
}

/// Clanker reacts to an ask, to one you kept waiting moving on (busy
/// with what it left running counts as working), and to one finishing
/// (model.go:1975-1986).
#[test]
fn clanker_reacts() {
    let mut m = crate::new_model("test");
    m.clock = Some(1_790_000_000_000);
    let mut a = Agent {
        key: "k".into(),
        display_name: "porter".into(),
        pid: std::process::id() as i32,
        ..Agent::default()
    };
    a.job.state = State::Working;
    read(&mut m, &a);
    read(&mut m, &a);
    assert_eq!(fx(&mut m), Fx::None, "nothing happened");
    a.job.state = State::Blocked;
    a.job.needs = "allow Bash?".into();
    read(&mut m, &a);
    assert_eq!(fx(&mut m), Fx::Ask);
    // Answered: a weaker reaction doesn't take over a stronger one
    // running, so let the ask run out first.
    while fx_tick(&mut m.kept::<Finish>().fx) {}
    a.job.state = State::Done;
    a.job.needs.clear();
    a.job.in_flight = 1; // busy: it reads as working
    a.job.background = vec!["task\0port the next round".into()];
    read(&mut m, &a);
    assert_eq!(fx(&mut m), Fx::Answered);
    a.job.in_flight = 0;
    a.job.background.clear();
    read(&mut m, &a);
    assert_eq!(fx(&mut m), Fx::Answered, "done is weaker than answered");
}

/// A stronger reaction takes over a weaker one, never the other way;
/// each runs its frames and ends (react, onFXTick).
#[test]
fn reactions_by_strength() {
    let mut r = Reaction::default();
    react(&mut r, Fx::Done);
    react(&mut r, Fx::Error);
    assert_eq!(r.kind, Fx::Error);
    react(&mut r, Fx::Ask);
    assert_eq!(r.kind, Fx::Error, "an ask doesn't take over an error");
    let mut n = 1;
    while fx_tick(&mut r) {
        n += 1;
    }
    assert_eq!(n, Fx::Error.frames());
    assert_eq!(r, Reaction::default());
    react(&mut r, Fx::Ask);
    assert_eq!(r.kind, Fx::Ask, "anything once it's over");
}

fn told(m: &mut Model) -> Vec<(String, String)> {
    std::mem::take(&mut m.kept::<Finish>().told)
}

fn working_agent() -> Agent {
    let mut a = Agent {
        key: "k".into(),
        display_name: "porter".into(),
        pid: std::process::id() as i32,
        ..Agent::default()
    };
    a.job.state = State::Working;
    a
}

/// One that starts waiting on you is a notification, once for the same
/// decisions; not while you're looking at it, nor in a quiet rush or
/// one that follows another (model.go:1990-2001).
#[test]
fn needs_you_notifies() {
    let mut m = crate::new_model("test");
    m.clock = Some(1_790_000_000_000);
    let mut a = working_agent();
    read(&mut m, &a);
    read(&mut m, &a);
    a.job.state = State::Blocked;
    a.job.needs = "allow Bash?".into();
    a.decisions_key = "d1".into();
    read(&mut m, &a);
    assert_eq!(
        told(&mut m),
        [("porter needs you".to_string(), "allow Bash?".to_string())]
    );
    // Answered, then the same decisions asked again: said already.
    a.job.state = State::Working;
    read(&mut m, &a);
    a.job.state = State::Blocked;
    read(&mut m, &a);
    assert!(told(&mut m).is_empty(), "the same decisions twice");
    // Following another rush: it notifies.
    a.job.state = State::Working;
    a.decisions_key.clear();
    read(&mut m, &a);
    m.follows = true;
    a.job.state = State::Blocked;
    read(&mut m, &a);
    assert!(told(&mut m).is_empty(), "a follower is quiet");
}

/// A stall is said once, and is a notification (model.go:2004-2011).
#[test]
fn a_stall_notifies() {
    let mut m = crate::new_model("test");
    let now = 1_790_000_000_000;
    m.clock = Some(now);
    let mut a = working_agent();
    read(&mut m, &a);
    a.stalled = now - 5 * 60_000;
    read(&mut m, &a);
    assert_eq!(
        told(&mut m),
        [(
            "porter stalled".to_string(),
            "nothing from the model for 5m and no step running · it keeps waiting until you stop it or send it something".to_string()
        )]
    );
    read(&mut m, &a);
    assert!(told(&mut m).is_empty(), "once a stall");
}

/// A usage limit rush moves a session on from by itself is noted
/// quietly, not as an error: here another login has room
/// (limitHandled, model.go:1963-1969).
#[test]
fn a_handled_limit_is_noted_quietly() {
    let mut m = crate::new_model("test");
    let now = 1_790_000_000_000;
    m.clock = Some(now);
    let login = |current: bool| crate::accounts::LoginView {
        current,
        quota: rush_core::usage::Quota {
            fetched_at: now,
            ..Default::default()
        },
        ..Default::default()
    };
    crate::accounts::accounts(&mut m).logins = vec![login(true), login(false)];
    let mut a = working_agent();
    a.rush = true;
    a.kind = rush_state::LOGINS_KIND.into();
    read(&mut m, &a);
    read(&mut m, &a);
    a.job.state = State::Blocked;
    a.job.detail = "usage limit · resets 5pm".into();
    read(&mut m, &a);
    let f = m.kept::<Finish>();
    let last = f.events.last().expect("noted");
    assert_eq!(
        (last.glyph.as_str(), last.what.as_str()),
        ("↻", "usage limit · rush moves it on")
    );
    assert_ne!(f.fx.kind, Fx::Error, "nothing went wrong");
    // A rush that waits at a limit says it stopped.
    assert!(!limit_handled(
        &m,
        &Agent {
            rush: false,
            ..a.clone()
        }
    ));
}
