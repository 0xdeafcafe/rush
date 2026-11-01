//! The conversation's views (presets.rs): the key through them, the
//! config remembering the one picked for every session, its label on the
//! tabs row, and enter on a catch-up card opening its turn in Normal.

use rush_convo::Preset;
use rush_core::agent::State;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::time::Ms;

use super::host::Fake;
use super::tests::{agent, flash, open, press, screen, sess};
use crate::model::Model;

/// A session of n turns, each a question and its answer, whose config is
/// held in memory only: the tests share one home, and a view kept on disk
/// would show in every other test's Session.
fn talked(n: usize) -> Model {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    crate::accounts::use_store(&mut m, rush_state::Store::default());
    crate::accounts::accounts(&mut m).read_only = true;
    let now = m.now;
    for i in 0..n {
        let at = now - (n - i) as Ms * 60_000;
        sess(&mut m).sent(format!("question {i}"), Vec::new(), at);
        sess(&mut m).apply(
            &Event::Message(Message {
                role: "assistant".into(),
                id: format!("a{i}"),
                parts: vec![Part {
                    kind: PartKind::Text,
                    text: format!("answer {i}"),
                    ..Part::default()
                }],
                ..Message::default()
            }),
            at + 1000,
        );
        sess(&mut m).apply(
            &Event::TurnEnd(TurnEnd {
                reason: "done".into(),
                ..TurnEnd::default()
            }),
            at + 2000,
        );
    }
    m
}

fn kept(m: &Model) -> String {
    crate::accounts::loaded_config(m)
        .map(|c| c.convo_view.clone())
        .unwrap_or_default()
}

/// ⌥s goes through the five views and round to Normal, saying each; the
/// config keeps the one picked, for every session, Normal as nothing.
#[test]
fn the_key_goes_through_the_views_and_the_config_keeps_it() {
    let mut m = talked(3);
    assert_eq!(super::presets::current(&m), Preset::Normal);
    for want in [
        Preset::Chat,
        Preset::Code,
        Preset::CatchUp,
        Preset::Trail,
        Preset::Normal,
    ] {
        press(&mut m, "alt+s");
        assert_eq!(super::presets::current(&m), want);
        assert_eq!(flash(&m), format!("{} · {}", want.title(), want.about()));
        let name = if want == Preset::Normal {
            ""
        } else {
            want.name()
        };
        assert_eq!(kept(&m), name);
        assert_eq!(m.cfg.convo_view, name);
        let _ = screen(&mut m);
        assert_eq!(m.session.as_ref().unwrap().o.preset, want, "drawn so");
    }
}

/// Another view than Normal is named on the tabs row; Normal's rows are
/// as they were.
#[test]
fn the_view_is_named_on_the_tabs_row() {
    let mut m = talked(3);
    let normal = screen(&mut m).join("\n");
    assert!(!normal.contains("view Normal"), "{normal}");
    press(&mut m, "alt+s");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("view Chat ▾"), "{f}");
    assert!(f.contains("answer 2"), "{f}");
}

/// Enter on a catch-up card opens its turn in Normal, picked.
#[test]
fn enter_on_a_card_opens_its_turn_in_normal() {
    let mut m = talked(3);
    super::presets::set(&mut m, Preset::CatchUp);
    let f = screen(&mut m).join("\n");
    assert!(f.contains("asked") && f.contains("question 1"), "{f}");
    m.session.as_mut().unwrap().sel = "t2".into();
    let f = screen(&mut m).join("\n");
    assert!(f.contains("enter opens it in Normal"), "{f}");
    press(&mut m, "enter");
    assert_eq!(super::presets::current(&m), Preset::Normal);
    let v = m.session.as_ref().unwrap();
    assert_eq!(v.sel, "t2");
    assert_eq!(v.o.open.get("t2"), Some(&true));
    assert_eq!(kept(&m), "");
}
