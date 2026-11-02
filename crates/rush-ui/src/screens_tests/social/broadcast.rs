//! The tests of rush-ui-social's broadcast.rs that need the whole UI.

use crate::model::Model;
use rush_core::agent::Agent;
use rush_ui_social::broadcast::*;

fn agent(key: &str, name: &str) -> Agent {
    Agent {
        key: key.into(),
        display_name: name.into(),
        ..Agent::default()
    }
}

#[test]
fn picks() {
    let mut m = crate::kit::model(100, 30);
    let mut term = agent("d", "terminal one");
    term.interactive = true;
    let mut old = agent("c", "old work");
    old.past = true;
    let agents = vec![agent("a", "fix login bug"), agent("b", "docs"), old, term];
    m.set_fleet(crate::worker::Fleet {
        roots: vec![String::new(); agents.len()],
        agents,
        ..Default::default()
    });
    let picked = |m: &mut Model| {
        let mut v: Vec<String> = m
            .kept::<Broadcast>()
            .picked
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        v.sort();
        v
    };
    m.broadcast_command("all");
    assert_eq!(
        picked(&mut m),
        ["a", "b"],
        "all picks the agents a message can reach"
    );
    m.broadcast_command("@docs");
    assert_eq!(picked(&mut m), ["b"], "a tag picks that agent");
    m.broadcast_pick("a");
    m.broadcast_pick("d"); // can't be messaged from here
    m.broadcast_pick("b");
    assert_eq!(picked(&mut m), ["a"], "a click picks, a second unpicks");
    m.broadcast_command("@nobody hi");
    assert_eq!(picked(&mut m), ["a"], "an unknown tag changes nothing");
    m.broadcast_command("hello there");
    assert!(picked(&mut m).is_empty() && m.input.string() == "hello there");
    m.press("esc");
    assert!(m.kept::<Broadcast>().picked.is_none(), "esc stops");
}

#[test]
fn handles_are_titles() {
    // An agent goes by its title, kebab-cased; with none yet, by a plain
    // name from its id, never an animal's.
    let a = agent("k", "Fix the login bug!");
    assert_eq!(mention_name(&a), "fix-the-login-bug");
    assert!(
        tags(&a, &mention_name(&a))
            && tags(&a, "Fix-the-login-bug")
            && tags(&a, "@fix-the-login-bug")
    );
    assert!(!tags(&a, "fix-the-login"));
    let mut b = agent("claude:default:abc", "");
    b.job.id = "abcd1234".into();
    assert_eq!(mention_name(&b), "session-abcd1234");
}
