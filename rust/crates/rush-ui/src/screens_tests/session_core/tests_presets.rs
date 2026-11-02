//! The conversation's views (presets.rs): the key through them, the
//! config remembering the one picked for every session, its label on the
//! tabs row, and enter on a catch-up card opening its turn in Normal.

use rush_convo::Preset;
use rush_core::agent::State;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::time::Ms;

use rush_draw::{Button, KeyMods, MouseKind};

use super::host::Fake;
use super::tests::{agent, flash, open, press, screen, sess};
use crate::frame::Mouse;
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

/// Enter on the catch-up card opens the first turn it covers in Normal,
/// picked; with no look before, the card covers the last turns.
#[test]
fn enter_on_a_card_opens_its_turn_in_normal() {
    let mut m = talked(6);
    super::presets::set(&mut m, Preset::CatchUp);
    let f = screen(&mut m).join("\n");
    assert!(f.contains("The last 4 turns"), "{f}");
    assert!(f.contains("Done") && f.contains("answer 5"), "{f}");
    assert!(f.contains("question 1"), "a turn before the card: {f}");
    m.session.as_mut().unwrap().sel = "t3".into();
    let _ = screen(&mut m);
    press(&mut m, "enter");
    assert_eq!(super::presets::current(&m), Preset::Normal);
    let v = m.session.as_ref().unwrap();
    assert_eq!(v.sel, "t3");
    assert_eq!(v.o.open.get("t3"), Some(&true));
    assert_eq!(kept(&m), "");
}

/// Chat draws bubbles; enter on a bubble's foot opens its turn in Normal.
#[test]
fn enter_on_a_bubble_opens_its_turn_in_normal() {
    let mut m = talked(3);
    super::presets::set(&mut m, Preset::Chat);
    let f = screen(&mut m).join("\n");
    assert!(f.contains("╭") && f.contains("answer 2"), "{f}");
    m.session.as_mut().unwrap().sel = "t2".into();
    let _ = screen(&mut m);
    press(&mut m, "enter");
    assert_eq!(super::presets::current(&m), Preset::Normal);
    assert_eq!(m.session.as_ref().unwrap().sel, "t2");
}

/// The row the view tabs are on, and the screen.
fn tab_rows(m: &mut Model) -> (usize, Vec<String>) {
    let s = screen(m);
    let i = s
        .iter()
        .position(|l| l.contains("Normal") && l.contains("Chat") && l.contains("Trail"))
        .unwrap_or_else(|| panic!("no tab row:\n{}", s.join("\n")));
    (i, s)
}

/// The conversation has a row of the five views with a rule under it, the
/// one showing lit; the key moves the lit one on.
#[test]
fn the_views_are_tabs_above_the_conversation() {
    let mut m = talked(3);
    let (i, s) = tab_rows(&mut m);
    assert!(
        s[i].contains("Catch-up") && s[i].contains("alt+s"),
        "{}",
        s[i]
    );
    let before = s[i + 1]
        .find('\u{2501}')
        .unwrap_or_else(|| panic!("{}", s[i + 1]));
    press(&mut m, "alt+s");
    let (i, s) = tab_rows(&mut m);
    let after = s[i + 1].find('\u{2501}').unwrap();
    assert!(after > before, "the lit tab moved on: {}", s[i + 1]);
    assert!(s.join("\n").contains("answer 2"));
}

/// A click on a tab shows that view, as the key would.
#[test]
fn a_click_on_a_tab_switches_the_view() {
    let mut m = talked(3);
    let (i, s) = tab_rows(&mut m);
    let x = rush_core::text::width(&s[i][..s[i].find("Code").unwrap()]);
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(Button::Left),
        x: x as u16,
        y: i as u16,
        mods: KeyMods::NONE,
    });
    assert_eq!(super::presets::current(&m), Preset::Code);
    let (_, s) = tab_rows(&mut m);
    assert!(s.join("\n").contains("view Code"));
}

/// Narrow, the names shorten and the key goes; the row never wraps.
#[test]
fn the_tabs_shorten_when_narrow() {
    let c = rush_core::text::Color::Default;
    let (rows, _) = super::presets::tabs(Preset::Normal, None, "alt+s", 60, c);
    assert!(rows[0].plain().contains("Catch-up") && rows[0].plain().contains("alt+s"));
    let (rows, _) = super::presets::tabs(Preset::Normal, None, "alt+s", 40, c);
    assert!(rows[0].plain().contains("Catch ") && !rows[0].plain().contains("alt+s"));
    assert!(rows.iter().all(|l| l.width() <= 40));
}

/// A suggested view has a star before its name, in orange; none without one.
#[test]
fn a_suggested_tab_has_a_star() {
    use crate::style::ORANGE;
    let c = rush_core::text::Color::Default;
    let (plain, _) = super::presets::tabs(Preset::Normal, None, "alt+s", 80, c);
    assert!(!plain[0].plain().contains('\u{2726}'));
    let (rows, at) = super::presets::tabs(Preset::Normal, Some(Preset::Trail), "alt+s", 80, c);
    let t = rows[0].plain();
    assert!(
        t.contains("\u{2726} Trail") && t.matches('\u{2726}').count() == 1,
        "{t}"
    );
    assert!(
        rows[0]
            .spans
            .iter()
            .any(|s| s.text == "\u{2726} " && s.style.fg == ORANGE)
    );
    assert_eq!(at.len(), 5);
}
