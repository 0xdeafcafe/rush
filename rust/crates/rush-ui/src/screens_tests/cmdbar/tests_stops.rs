//! rush-ui-cmdbar's stops.rs tests: they make a whole UI model.

use super::stops::*;
use crate::host::{Fake, set_hosts};
use crate::model::Model;
use rush_core::agent::Job;
use rush_core::agent::{Agent, State};

fn agent(key: &str, state: State) -> Agent {
    Agent {
        key: key.into(),
        display_name: key.into(),
        rush: true,
        job: Job {
            id: key.into(),
            state,
            updated_at: 1_000,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn model(agents: Vec<Agent>) -> (Model, std::sync::Arc<Fake>) {
    let mut m = crate::kit::model(120, 30);
    let n = agents.len();
    m.set_fleet(crate::worker::Fleet {
        agents,
        roots: vec![String::new(); n],
        ..Default::default()
    });
    let h = Fake::new();
    set_hosts(&mut m, std::sync::Arc::new(h.clone()));
    (m, h)
}

// e0f011a4: #stop stops a rush agent with the agents it spawned, as
// ctrl+x does, and puts it away in Earlier.
#[test]
fn stop_goes_through_stop_all() {
    let (mut m, h) = model(vec![agent("a", State::Working)]);
    m.sel = "a".into();
    m.command("#stop");
    assert_eq!(*h.stopped_all.lock().unwrap(), ["a"]);
    assert!(m.agents[0].done, "a stopped agent is put away");
}

// alt+d on a finished rush agent: put away, and stopped with all it
// started; alt+d again brings it back.
#[test]
fn done_stops_and_brings_back() {
    let (mut m, h) = model(vec![agent("a", State::Done)]);
    m.sel = "a".into();
    m.press("alt+d");
    assert!(m.agents[0].done);
    assert_eq!(*h.stopped_all.lock().unwrap(), ["a"]);
    m.sel = "a".into(); // picked again in Earlier
    m.press("alt+d");
    assert!(!m.agents[0].done, "brought back");
    assert_eq!(m.sel, "a");
}

// A working one asks first.
#[test]
fn done_asks_while_working() {
    let (mut m, h) = model(vec![agent("a", State::Working)]);
    m.agents[0].pid = 0;
    m.sel = "a".into();
    m.command("#done");
    assert_eq!(m.confirm.as_ref().unwrap().question, "Mark a done?");
    m.press("y");
    assert!(m.agents[0].done && h.stopped_all.lock().unwrap().len() == 1);
}

// ctrl+x hides one, which leaves the list and stops.
#[test]
fn ctrl_x_hides() {
    let (mut m, h) = model(vec![agent("a", State::Done), agent("b", State::Done)]);
    m.sel = "a".into();
    m.press("ctrl+x");
    assert!(m.agents.iter().all(|a| a.key != "a"), "hidden");
    assert_eq!(*h.stopped_all.lock().unwrap(), ["a"]);
}

// #rm on a past conversation says there's nothing to stop.
#[test]
fn rm_on_past() {
    let (mut m, _) = model(vec![agent("a", State::Done)]);
    m.agents[0].past = true;
    m.sel = "a".into();
    m.command("#rm");
    assert!(m.confirm.is_none());
}

// Hibernate: a finished agent past its time, with a process, once;
// never in a rush that follows (model.go:2036, snapload.go:84).
#[test]
fn hibernate_leads_only() {
    let mut a = agent("a", State::Done);
    a.worker = Some(4242);
    let (mut m, _) = model(vec![a]);
    let now = 1_000 + 31 * 60_000;
    m.follows = true;
    assert!(
        to_hibernate(&mut m, 30, now).is_empty(),
        "a follower stops nothing"
    );
    m.follows = false;
    assert_eq!(to_hibernate(&mut m, 30, now).len(), 1);
    assert!(to_hibernate(&mut m, 30, now).is_empty(), "one try each");
    assert!(to_hibernate(&mut m, 0, now).is_empty());
}
