//! A picked step's own keys: v shows its output another way and w keeps
//! each line of it to one row (rushmode.go:4101-4117; Go's
//! convo/views_test.go TestNextView for the views a step has).

use rush_core::event::{Message, Part, PartKind};
use rush_core::tool::{Input, Kind as ToolKind, Output};

use super::*;

/// A session with a fetch whose output is JSON, the fetch picked; its
/// ref.
fn fetched() -> (Model, String) {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    let now = m.now;
    sess(&mut m).sent("what's in it?".into(), Vec::new(), now - 60_000);
    let call = Call {
        id: "f1".into(),
        name: "WebFetch".into(),
        kind: ToolKind::Fetch,
        input: Input {
            url: "https://example.com/a.json".into(),
            ..Input::default()
        },
        ..Call::default()
    };
    let msg = |role: &str, part: Part| {
        Event::Message(Message {
            role: role.into(),
            id: format!("{role}-{:?}", part.kind),
            parts: vec![part],
            ..Message::default()
        })
    };
    let s = sess(&mut m);
    s.apply(
        &msg(
            "assistant",
            Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(call)),
                ..Part::default()
            },
        ),
        now - 50_000,
    );
    s.apply(
        &msg(
            "user",
            Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(Output {
                    call_id: "f1".into(),
                    text: r#"{"name":"rush","tags":["tui","agents"],"stars":3}"#.into(),
                    ..Output::default()
                })),
                ..Part::default()
            },
        ),
        now - 40_000,
    );
    let n = sess(&mut m).turns.last().unwrap().n;
    let r = format!("t{n}:s:f1");
    m.session.as_mut().unwrap().sel = r.clone();
    (m, r)
}

#[test]
fn v_shows_a_picked_steps_output_another_way() {
    let (mut m, r) = fetched();
    let first = rush_convo::render::next_view(sess(&mut m), &r, "");
    assert!(!first.is_empty(), "JSON has another view");
    assert_eq!(press(&mut m, "v"), Out::Draw);
    let v = m.session.as_ref().unwrap();
    assert_eq!(v.o.view.get(&r), Some(&first));
    assert_eq!(v.o.open.get(&r), Some(&true), "opened to show it");
    press(&mut m, "v");
    let back = m.session.as_ref().unwrap().o.view.get(&r).cloned();
    assert!(back.is_some_and(|b| b != first), "v cycles");
    assert!(conn(&mut m, KEY).input.is_empty(), "nothing typed");
    assert_eq!(m.session.as_ref().unwrap().sel, r, "still picked");
}

#[test]
fn w_keeps_a_picked_steps_lines_to_one_row_and_back() {
    let (mut m, r) = fetched();
    let nw = rush_convo::render::no_wrap_ref(&r);
    assert_eq!(press(&mut m, "w"), Out::Draw);
    assert_eq!(m.session.as_ref().unwrap().o.open.get(&nw), Some(&true));
    press(&mut m, "w");
    assert_eq!(m.session.as_ref().unwrap().o.open.get(&nw), Some(&false));
    assert!(conn(&mut m, KEY).input.is_empty(), "nothing typed");
    // With something typed, w is a letter.
    typed(&mut m, "x");
    press(&mut m, "w");
    assert_eq!(conn(&mut m, KEY).input.string(), "xw");
}
