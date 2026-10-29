//! Switching a hosted session: in place on the same agent paid the same
//! way, else its conversation handed over to a new one that starts as the
//! setup; a busy one waits, an agent that can't take a conversation on
//! says so; the conversation as it's told for a hand-off. After Go's
//! setup.go:325-503, handoff_ready_test.go and convo/handoff_test.go. No
//! loop: handed-off work runs at once against the Fake host.

use rush_core::adapter::{Adapter, Feature, Features, Kind, Level, Profile, Support};
use rush_core::agent::State;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};

use super::host::{Fake, Op};
use super::switch::{conversation, handoff_clip, in_place};
use super::tests::{KEY, agent, open};
use super::{conn, core};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::accounts::setup::StartOver;
use crate::screens::accounts::state::accounts;

/// An agent that takes conversations on from others.
struct Taker;

impl Adapter for Taker {
    fn kind(&self) -> Kind {
        Kind::from("taker")
    }
    fn name(&self) -> String {
        "Taker".into()
    }
    fn features(&self) -> Features {
        [(Feature::HandoffIn, Support::YES)].into_iter().collect()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
}

/// What the session runs as now, for every test.
fn now_as(_: &Model, _: &str) -> Option<StartOver> {
    Some(StartOver {
        kind: "steer".into(),
        model: "m1".into(),
        mode: "default".into(),
        mode_set: true,
        ..StartOver::default()
    })
}

/// A rush session on steer, hosted and open, its conversation one turn.
fn model() -> (Model, std::sync::Arc<Fake>) {
    let fake = Fake::new();
    fake.host(KEY);
    let mut a = agent("steer", State::Idle);
    a.rush = true;
    let mut m = open(a, &fake, 120, 40);
    super::live::sync(&mut m);
    m.reg.register(Taker);
    m.roots = vec![String::new()];
    accounts(&mut m).session_start = Some(now_as);
    super::start::install(&mut m);
    let s = &mut m.session.as_mut().unwrap().sess;
    s.sent("fix the parser".into(), Vec::new(), 1_000);
    s.apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "a1".into(),
            parts: vec![Part {
                kind: PartKind::Text,
                text: "Fixed it.".into(),
                ..Part::default()
            }],
            ..Message::default()
        }),
        2_000,
    );
    s.apply(&Event::TurnEnd(TurnEnd::default()), 3_000);
    (m, fake)
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

fn switch(m: &mut Model, o: StartOver, msg: &str) -> Out {
    let f = accounts(m).switcher.expect("installed");
    f(m, KEY, o, msg)
}

fn steer(model: &str, effort: &str) -> StartOver {
    StartOver {
        kind: "steer".into(),
        model: model.into(),
        effort: effort.into(),
        ..now_as(&Model::new("0"), "").unwrap()
    }
}

#[test]
fn what_switches_in_place() {
    let o = |kind: &str, billing: &str, model: &str| StartOver {
        kind: kind.into(),
        billing: billing.into(),
        model: model.into(),
        ..StartOver::default()
    };
    for (from, to, provider, want) in [
        (o("a", "", "m1"), o("a", "", "m2"), "anthropic", true),
        (o("a", "", "m1"), o("b", "", "m1"), "anthropic", false),
        (o("a", "", "m1"), o("a", "key", "m1"), "anthropic", false),
        (o("a", "", "m1"), o("a", "", "m2"), "ollama", false),
        (o("a", "", "m1"), o("a", "", "m1"), "ollama", true),
    ] {
        assert_eq!(in_place(&from, &to, provider), want, "{from:?} → {to:?}");
    }
}

/// On the same agent its model and effort change in place, your message
/// going after them; nothing to change says so.
#[test]
fn switches_in_place() {
    let (mut m, fake) = model();
    assert_eq!(switch(&mut m, steer("m2", "high"), "carry on"), Out::Draw);
    assert_eq!(
        fake.ops(),
        [
            Op::Model("m2".into()),
            Op::Effort("high".into()),
            Op::Send {
                text: "carry on".into(),
                images: Vec::new(),
                now: false
            }
        ]
    );
    assert!(
        flash(&m).ends_with(" · m2 · high effort from the next turn"),
        "{}",
        flash(&m)
    );
    switch(&mut m, steer("m1", ""), "");
    assert!(flash(&m).starts_with("already "), "{}", flash(&m));
    assert_eq!(fake.ops().len(), 3);
}

/// To another agent, its conversation is handed over: the new session
/// named for it, the old one put away once it runs, the new one picked.
#[test]
fn hands_over_to_another_agent() {
    let (mut m, fake) = model();
    let to = StartOver {
        kind: "taker".into(),
        ..StartOver::default()
    };
    switch(&mut m, to, "and the tests");
    let handed = fake.handed.lock().unwrap().clone();
    assert_eq!(handed.len(), 1);
    let h = &handed[0];
    assert_eq!(h.start.kind, "taker");
    assert_eq!(h.start.name, "tidy the parser · on Taker");
    assert_eq!(h.start.cwd, "/src/checkout");
    assert_eq!((h.from.as_str(), h.retire), ("k1", true));
    assert_eq!(h.message, "and the tests");
    assert_eq!(h.conv.first, "fix the parser");
    assert_eq!(h.conv.name, "tidy the parser");
    assert_eq!(
        core(&mut m).select_on_load,
        rush_state::dir::key("default", "a:fake-1")
    );
    assert_eq!(flash(&m), "started tidy the parser · on Taker");
    assert!(m.agents[0].done, "the one it came from is put away");
}

/// A turn under way, or one that can't take a conversation on, isn't
/// handed over; what you meant to say waits in the box.
#[test]
fn what_isnt_handed_over() {
    let (mut m, fake) = model();
    conn(&mut m, KEY).info.state = "working".into();
    let to = StartOver {
        kind: "taker".into(),
        ..StartOver::default()
    };
    switch(&mut m, to, "and the tests");
    assert_eq!(
        flash(&m),
        "switch harness between turns · finish or stop this turn and send or clear queued messages first"
    );
    assert_eq!(conn(&mut m, KEY).input.string(), "and the tests");
    let plain = StartOver {
        kind: "plain".into(),
        ..StartOver::default()
    };
    switch(&mut m, plain, "");
    assert_eq!(
        flash(&m),
        "Plain Code can't take a conversation on from another"
    );
    assert!(fake.handed.lock().unwrap().is_empty());
}

/// A usage limit's hand-on goes straight to the hand-over.
#[test]
fn a_limit_hands_over() {
    let (mut m, fake) = model();
    let f = accounts(&mut m).hand_over.expect("installed");
    let to = StartOver {
        kind: "taker".into(),
        ..StartOver::default()
    };
    f(&mut m, KEY, to, "");
    assert_eq!(fake.handed.lock().unwrap().len(), 1);
}

/// The conversation as told: how it started, its turns as history, its
/// last turns, the agent it ran on.
#[test]
fn the_conversation_told() {
    let (m, _) = model();
    let s = &m.session.as_ref().unwrap().sess;
    let c = conversation(s, &Kind::from("steer"));
    assert_eq!(c.from.as_str(), "steer");
    assert_eq!(c.first, "fix the parser");
    let lines: Vec<(&str, &str)> = c
        .history
        .iter()
        .map(|l| (l.role.as_str(), l.text.as_str()))
        .collect();
    assert_eq!(
        lines,
        [("user", "fix the parser"), ("assistant", "Fixed it.")]
    );
    assert_eq!(c.recent.len(), 2);
    assert_eq!(c.earlier_steps, 0);
}

/// A message too long for a hand-off keeps its start and end, never
/// split within a character.
#[test]
fn a_long_message_is_clipped() {
    assert_eq!(handoff_clip("short", 100), "short");
    let s = "é".repeat(100);
    let c = handoff_clip(&s, 60);
    assert!(c.contains("\n[… middle omitted for handoff …]\n"), "{c}");
    assert!(c.starts_with("éé") && c.ends_with("éé"), "{c}");
    assert!(c.len() <= 62, "{}", c.len());
}
