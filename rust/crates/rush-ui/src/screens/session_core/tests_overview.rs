//! The Overview's own rows beside its numbers: the agent's @handle after
//! its name, the permission mode on the now line, the Context section
//! (how the window grew turn by turn) and the Scratch section. After Go's
//! rushmode.go:2451 (the overview's first rows), convo/overview.go:286
//! (the now line) and :300-347 (Context), growth.go and
//! overviewsections.go:75 (Scratch).

use rush_core::event::{Message, Part, PartKind};

use super::*;

/// The row of the screen whose text has s.
fn row_with(m: &mut Model, s: &str) -> Option<usize> {
    screen(m).iter().position(|l| l.contains(s))
}

/// Two turns, each a request: 74k, then 75k in context.
fn two_turns() -> Model {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 50);
    let now = m.now;
    let said = |m: &mut Model, id: &str, ctx: i64, at: Ms| {
        sess(m).apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: id.into(),
                model: "claude-opus-5-5".into(),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text: format!("answer {id}"),
                    ..Part::default()
                }],
                tokens: Some(rush_core::agent::TokenUsage {
                    input: 1_000,
                    output: 300,
                    cache_read: ctx - 1_000,
                    ..Default::default()
                }),
                ..Message::default()
            }),
            at,
        );
    };
    sess(&mut m).sent("one".into(), Vec::new(), now - 600_000);
    said(&mut m, "a1", 74_000, now - 590_000);
    sess(&mut m).apply(&Event::TurnEnd(TurnEnd::default()), now - 580_000);
    sess(&mut m).sent("two".into(), Vec::new(), now - 500_000);
    said(&mut m, "a2", 75_000, now - 490_000);
    sess(&mut m).apply(&Event::TurnEnd(TurnEnd::default()), now - 480_000);
    m
}

#[test]
fn overview_has_the_handle_the_mode_and_how_the_context_grew() {
    let mut m = two_turns();
    sess(&mut m).info.permission_mode = "auto".into();
    press(&mut m, "]");
    let s = screen(&mut m);
    let y = row_with(&mut m, "spent").unwrap();
    assert!(s[y - 2].contains(" @"), "the handle: {:?}", s[y - 2]);
    let now = row_with(&mut m, "now  opus 5.5").unwrap();
    assert!(
        s[now].contains("now  opus 5.5 · effort default · permissions auto   in "),
        "{:?}",
        s[now]
    );
    let c = row_with(&mut m, "▾ Context").expect("the Context section");
    assert!(
        s[c + 1].trim().chars().all(|ch| "▁▂▃▄▅▆▇█".contains(ch)),
        "{:?}",
        s[c + 1]
    );
    assert_eq!(s[c + 2].trim(), "#1 74k → #2 75k · peak 75k");
    // The context goes before the turns' costs, as Go's.
    let p = row_with(&mut m, "▾ Per turn").unwrap();
    assert!(c < p);
}

#[test]
fn overview_has_its_scratch() {
    let mut m = two_turns();
    m.agents[0].temp = 3 << 20;
    let key = m.agents[0].key.clone();
    conn(&mut m, &key).info.began.temp_dir = "/tmp/rush-steer".into();
    press(&mut m, "]");
    let s = screen(&mut m);
    let y = row_with(&mut m, "▾ Scratch").expect("the Scratch section");
    assert!(
        s[y].contains("TMPDIR for it and everything it runs"),
        "{:?}",
        s[y]
    );
    assert_eq!(s[y + 1].trim(), "/tmp/rush-steer · 3M");
}
