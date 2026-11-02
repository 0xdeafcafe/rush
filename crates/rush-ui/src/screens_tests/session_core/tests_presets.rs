//! The conversation's views (presets.rs): the Conversation tab naming the
//! view, the dropdown under it that picks one (by key and by click), the
//! config remembering the one picked for every session, and enter on a
//! catch-up card opening its turn in Normal.

use rush_convo::Preset;
use rush_core::agent::State;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::time::Ms;

use rush_draw::{Button, KeyMods, MouseKind};

use super::host::Fake;
use super::tests::{KEY, agent, flash, open, press, screen, sess};
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

/// The session's tab row and the screen.
fn tab_row(m: &mut Model) -> (usize, Vec<String>) {
    let s = screen(m);
    let i = s
        .iter()
        .position(|l| l.contains("Conversation") && l.contains("Overview"))
        .unwrap_or_else(|| panic!("no tab row:\n{}", s.join("\n")));
    (i, s)
}

/// The dropdown's rows on the screen: from its Normal row to its keys.
fn menu_rows(s: &[String]) -> Vec<String> {
    let from = s
        .iter()
        .position(|l| l.contains("Normal") && l.contains("everything, as usual"))
        .unwrap_or_else(|| panic!("no dropdown:\n{}", s.join("\n")));
    s[from..=from + 5]
        .iter()
        .map(|l| l.trim_end().to_string())
        .collect()
}

fn menu_open(m: &mut Model) -> bool {
    super::presets::menu_open(m, KEY)
}

/// A left click at x, y.
fn click(m: &mut Model, x: usize, y: usize) {
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(Button::Left),
        x: x as u16,
        y: y as u16,
        mods: KeyMods::NONE,
    });
}

/// The cell `what` starts at in row l.
fn col(l: &str, what: &str) -> usize {
    let at = l.find(what).unwrap_or_else(|| panic!("no {what:?} in {l}"));
    rush_core::text::width(&l[..at])
}

/// Normal: the Conversation tab says so with its ▾ alone, and no row of
/// views sits under the tabs.
#[test]
fn the_tab_bar_in_normal() {
    let mut m = talked(3);
    let (i, s) = tab_row(&mut m);
    assert!(
        s[i].trim_start()
            .starts_with("Conversation ▾   Overview   Changes"),
        "{}",
        s[i]
    );
    assert!(
        !s.iter()
            .any(|l| l.contains("Catch-up") || l.contains("alt+s")),
        "{}",
        s.join("\n")
    );
}

/// Another view than Normal is named on the Conversation tab.
#[test]
fn the_tab_bar_names_the_view() {
    let mut m = talked(3);
    super::presets::set(&mut m, Preset::Chat);
    let (i, s) = tab_row(&mut m);
    assert!(
        s[i].trim_start()
            .starts_with("Conversation · Chat ▾   Overview   Changes"),
        "{}",
        s[i]
    );
    assert!(s.join("\n").contains("answer 2"));
}

/// The conversation starts right under the tabs' rule: the row the views
/// had is its own again.
#[test]
fn the_conversation_has_the_row_back() {
    let mut m = talked(3);
    let (i, _) = tab_row(&mut m);
    let v = m.session.as_ref().unwrap();
    assert_eq!(v.top, i + 2, "the body starts under the rule");
    assert_eq!(v.top_off, 0);
}

/// ⌥s opens the dropdown under the tab: the five views, what each is for,
/// the one showing marked; ↓ and enter show the next, which the config
/// keeps for every session, and the dropdown closes.
#[test]
fn the_key_opens_the_dropdown_and_picks() {
    let mut m = talked(3);
    assert_eq!(super::presets::current(&m), Preset::Normal);
    press(&mut m, "alt+s");
    assert!(menu_open(&mut m));
    let (i, s) = tab_row(&mut m);
    let rows = menu_rows(&s);
    let at = s
        .iter()
        .position(|l| l.contains("everything, as usual"))
        .unwrap();
    assert_eq!(at, i + 2, "right under the tab's rule");
    assert!(rows[0].contains("▸ ● Normal"), "{rows:?}");
    for (r, p) in rows.iter().zip(Preset::ALL) {
        assert!(r.contains(p.title()) && r.contains(p.about()), "{r}");
    }
    assert!(rows[5].contains(super::presets::MENU_HOW), "{rows:?}");
    // Under the tab, a cell in.
    assert_eq!(col(&s[at], "▸"), col(&s[i], "Conversation"));
    press(&mut m, "down");
    let (_, s) = tab_row(&mut m);
    assert!(menu_rows(&s)[1].contains("▸   Chat"), "{:?}", menu_rows(&s));
    press(&mut m, "enter");
    assert!(!menu_open(&mut m));
    assert_eq!(super::presets::current(&m), Preset::Chat);
    assert_eq!(flash(&m), "Chat · what did we say?");
    assert_eq!(kept(&m), "chat");
    assert_eq!(m.cfg.convo_view, "chat");
    let (i, s) = tab_row(&mut m);
    assert!(s[i].contains("Conversation · Chat ▾"), "{}", s[i]);
    assert!(!s.join("\n").contains(super::presets::MENU_HOW));
    assert_eq!(
        m.session.as_ref().unwrap().o.preset,
        Preset::Chat,
        "drawn so"
    );
    // Opened again, Chat is the one marked and picked; ↑ goes round the
    // top to Trail.
    press(&mut m, "alt+s");
    let (_, s) = tab_row(&mut m);
    assert!(menu_rows(&s)[1].contains("▸ ● Chat"), "{:?}", menu_rows(&s));
    press(&mut m, "up");
    press(&mut m, "up");
    press(&mut m, "enter");
    assert_eq!(super::presets::current(&m), Preset::Trail);
    assert_eq!(kept(&m), "trail");
    press(&mut m, "alt+s");
    press(&mut m, "down");
    press(&mut m, "enter");
    assert_eq!(super::presets::current(&m), Preset::Normal);
    assert_eq!(kept(&m), "");
}

/// esc closes the dropdown and changes nothing; so does its key again.
#[test]
fn esc_closes_the_dropdown() {
    let mut m = talked(3);
    press(&mut m, "alt+s");
    let _ = screen(&mut m);
    press(&mut m, "down");
    press(&mut m, "esc");
    assert!(!menu_open(&mut m));
    assert_eq!(super::presets::current(&m), Preset::Normal);
    let f = screen(&mut m).join("\n");
    assert!(!f.contains(super::presets::MENU_HOW), "{f}");
    assert!(f.contains("answer 2"), "{f}");
    press(&mut m, "alt+s");
    assert!(menu_open(&mut m));
    press(&mut m, "alt+s");
    assert!(!menu_open(&mut m));
}

/// The dropdown says its keys under the box while it's open.
#[test]
fn the_dropdown_says_its_keys() {
    let mut m = talked(3);
    press(&mut m, "alt+s");
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("↑↓ choose") && f.contains("enter show it"),
        "{f}"
    );
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

/// A click on the Conversation tab while it shows opens the dropdown; a
/// click on one of its rows shows that view and closes it.
#[test]
fn a_click_on_the_tab_opens_the_dropdown_and_a_row_picks() {
    let mut m = talked(3);
    let (i, s) = tab_row(&mut m);
    click(&mut m, col(&s[i], "Conversation"), i);
    assert!(menu_open(&mut m));
    let (_, s) = tab_row(&mut m);
    let y = s
        .iter()
        .position(|l| l.contains("what did it change?"))
        .unwrap_or_else(|| panic!("no dropdown:\n{}", s.join("\n")));
    click(&mut m, col(&s[y], "Code"), y);
    assert!(!menu_open(&mut m));
    assert_eq!(super::presets::current(&m), Preset::Code);
    assert_eq!(kept(&m), "code");
    let (i, s) = tab_row(&mut m);
    assert!(s[i].contains("Conversation · Code ▾"), "{}", s[i]);
}

/// A click outside the dropdown closes it and changes nothing; a click on
/// the Conversation tab while it's open closes it too.
#[test]
fn a_click_outside_closes_the_dropdown() {
    let mut m = talked(3);
    press(&mut m, "alt+s");
    let (i, _) = tab_row(&mut m);
    click(&mut m, 100, i + 3);
    assert!(!menu_open(&mut m));
    assert_eq!(super::presets::current(&m), Preset::Normal);
    press(&mut m, "alt+s");
    let (i, s) = tab_row(&mut m);
    click(&mut m, col(&s[i], "Conversation"), i);
    assert!(!menu_open(&mut m));
    let f = screen(&mut m).join("\n");
    assert!(!f.contains(super::presets::MENU_HOW), "{f}");
}

/// From another of the session's tabs, a click on Conversation shows the
/// conversation, as it did, without the dropdown; ⌥s there shows the
/// conversation with the dropdown open.
#[test]
fn the_tab_from_another_view_shows_the_conversation() {
    let mut m = talked(3);
    let (i, s) = tab_row(&mut m);
    click(&mut m, col(&s[i], "Overview"), i);
    assert_eq!(super::views::view_name(&m, KEY), "overview");
    let (i, s) = tab_row(&mut m);
    click(&mut m, col(&s[i], "Conversation"), i);
    assert_eq!(super::views::view_name(&m, KEY), "conversation");
    assert!(!menu_open(&mut m));
    let (i, s) = tab_row(&mut m);
    click(&mut m, col(&s[i], "Overview"), i);
    press(&mut m, "alt+s");
    assert_eq!(super::views::view_name(&m, KEY), "conversation");
    assert!(menu_open(&mut m));
}

/// The dropdown's rows: the view showing marked ●, the one picked on ▸,
/// the one suggested with ✦ (in orange) and why, and its keys.
#[test]
fn the_dropdown_marks_the_suggested_view() {
    use crate::style::ORANGE;
    let why = "long turns, many steps".to_string();
    let rows = super::presets::menu_rows(Preset::Normal, Some((Preset::Trail, why)), 0, 120);
    let plain: Vec<String> = rows
        .iter()
        .map(|l| l.plain().trim_end().to_string())
        .collect();
    assert_eq!(
        plain,
        [
            " ▸ ● Normal     everything, as usual",
            "     Chat       what did we say?",
            "     Code       what did it change?",
            "     Catch-up   what happened while I was away?",
            "     Trail      where did the time go?  ✦ long turns, many steps",
            "   ↑↓ · enter shows it · esc closes",
        ]
    );
    assert!(rows.iter().all(|l| l.width() == rows[0].width()));
    assert!(
        rows[4]
            .spans
            .iter()
            .any(|s| s.text == "✦" && s.style.fg == ORANGE)
    );
    // None suggested: no ✦; narrow, the rows are cut to fit.
    let rows = super::presets::menu_rows(Preset::Code, None, 2, 30);
    assert!(
        rows.iter()
            .all(|l| !l.plain().contains('✦') && l.width() <= 30)
    );
    assert!(
        rows[2].plain().starts_with(" ▸ ● Code"),
        "{}",
        rows[2].plain()
    );
}
