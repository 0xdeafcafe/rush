//! The Wall's subagent tiles and its mouse, after Go's wall.go (wallItems
//! :136, wallGroups :150, wallSubTile :684, wallKey :1001, wallClick
//! :1066).

use rush_core::agent::{State, SubagentTile};
use rush_core::text::Line;
use rush_draw::{Button, KeyMods, MouseKind};

use super::tests::{agent, model_of};
use super::wall;
use crate::frame::{Mouse, Press};
use crate::model::Model;

fn press(m: &mut Model, s: &str) {
    m.press_key(&Press::of(s).unwrap());
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

/// Each running subagent has a tile of its own, straight after its
/// agent's and beside it; enter on one opens its agent.
#[test]
fn subagents_have_their_own_tiles() {
    let mut a = agent("gpu", "GPU Terminal App Rust", State::Working, 101);
    for (id, d) in [
        ("s1", "Terminal comparison + README"),
        ("s2", "R.5 + R.7 memory"),
    ] {
        a.subagents.push(SubagentTile {
            id: id.into(),
            kind: "general-purpose".into(),
            description: d.into(),
            ..Default::default()
        });
    }
    let other = agent("other", "set up caching for CI", State::Idle, 104);
    let mut m = model_of(vec![a, other], 120, 40);
    let items = wall::wall_items(&m, false);
    let keys: Vec<&str> = items.iter().map(|it| it.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "default/gpu",
            "default/gpu\0s1",
            "default/gpu\0s2",
            "default/other"
        ]
    );
    assert_eq!(wall::groups(&items), [3, 1], "an agent and its subagents");
    press(&mut m, "]");
    let f = screen(&mut m).join("\n");
    for t in [
        "GPU Terminal App Rust",
        "Terminal comparison + README",
        "R.5 + R.7 memory",
        "general-purpose · ",
        "running · Terminal comparison + README",
    ] {
        assert!(f.contains(t), "{t:?} on the Wall:\n{f}");
    }
    // Right to the first subagent's tile; enter opens its agent.
    press(&mut m, "right");
    assert!(
        screen(&mut m)
            .iter()
            .any(|l| l.contains("┏━") && l.contains("Terminal comparison")),
        "{}",
        screen(&mut m).join("\n")
    );
    press(&mut m, "enter");
    assert_eq!(m.agents_page, 0, "back on the list");
    assert_eq!(m.sel, "default/gpu");
}

/// A click picks a tile; a second on it within 400ms opens its agent,
/// leaving the Wall.
#[test]
fn double_click_opens() {
    let a = agent("a", "fix the flaky tax rounding test", State::Working, 101);
    let b = agent("b", "set up caching for CI", State::Idle, 104);
    let mut m = model_of(vec![a, b], 120, 40);
    press(&mut m, "]");
    let f = screen(&mut m);
    let (y, l) = f
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains("set up caching for CI"))
        .expect("b's tile");
    let x = rush_core::text::width(&l[..l.find("set up caching").unwrap()]) as u16;
    let click = |m: &mut Model| {
        m.mouse_event(&Mouse {
            kind: MouseKind::Down(Button::Left),
            x,
            y: y as u16,
            mods: KeyMods::NONE,
        })
    };
    click(&mut m);
    assert_eq!(m.agents_page, 1, "one click picks");
    assert!(
        screen(&mut m)
            .iter()
            .any(|l| l.contains("┏━") && l.contains("set up caching"))
    );
    click(&mut m);
    assert_eq!(m.agents_page, 0, "the second opens it");
    assert_eq!(m.sel, "default/b");
}

/// A tile at work keeps the Wall waking a second after it was drawn: the
/// same time however often it's asked, so the wake comes due and the
/// spinner moves; with nothing at work it sleeps.
#[test]
fn a_working_tile_wakes_the_wall_each_second() {
    use crate::frame::places::Place;
    let a = agent("gpu", "GPU Terminal App Rust", State::Working, 101);
    let m = model_of(vec![a], 120, 40);
    let mut w = wall::Wall::default();
    assert_eq!(w.wake_at(&m), None, "not drawn yet");
    w.body(&m, 116, 36);
    let at = w.wake_at(&m).expect("a wake while a tile works");
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_eq!(w.wake_at(&m), Some(at), "the same wake, so it comes due");
    assert!(at <= std::time::Instant::now() + std::time::Duration::from_secs(1));
    let idle = agent("idle", "set up caching for CI", State::Idle, 104);
    let m = model_of(vec![idle], 120, 40);
    w.body(&m, 116, 36);
    assert_eq!(w.wake_at(&m), None, "nothing at work: no wake");
}
