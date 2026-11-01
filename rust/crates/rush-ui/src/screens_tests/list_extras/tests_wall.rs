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

/// Each running subagent has a small tile of its own under its agent's:
/// its name once, and what it's doing; enter on one opens its agent.
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
        "starting…",
    ] {
        assert!(f.contains(t), "{t:?} on the Wall:\n{f}");
    }
    assert_eq!(
        f.matches("Terminal comparison + README").count(),
        1,
        "its name once:\n{f}"
    );
    // Down to the first subagent's tile, under its agent's; enter opens
    // its agent.
    press(&mut m, "down");
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

/// A tile whose last reply ends with decisions shows them; typing 1b 2a
/// and enter answers it from the Wall, as the session's card would, and
/// the tile stops asking.
#[test]
fn answers_decisions_from_the_wall() {
    use rush_core::agent::{Preview, PreviewLine};
    use std::sync::Arc;

    use crate::host::{Fake, set_hosts};

    let a = agent("d", "Making Rush Screen More Useful", State::Done, 101);
    let mut m = model_of(vec![a], 120, 40);
    let fake = Fake::new();
    set_hosts(&mut m, Arc::new(fake.clone()));
    let said = "Here's what I found.\n\nDecisions:\n\
1. **Subagents on the Wall:** (a) fold them in; (b) small tiles.\n\
2. **Scope:** (a) all six; (b) two.\n\
My picks: 1a 2a.";
    m.previews.insert(
        "default/d".into(),
        Preview {
            recent: vec![PreviewLine {
                role: "assistant".into(),
                text: said.into(),
                at: 5,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    press(&mut m, "]");
    let f = screen(&mut m).join("\n");
    for t in [
        "1 Subagents on the Wall",
        "a fold them in  b small tiles",
        "2 Scope",
        "2 decisions · type 1a… to answer",
    ] {
        assert!(f.contains(t), "{t:?} on the tile:\n{f}");
    }
    assert!(
        !f.contains("Decisions:"),
        "the block isn't in the reply:\n{f}"
    );
    for k in ["1", "b", "space", "2", "a"] {
        press(&mut m, k);
    }
    assert_eq!(m.agents_page, 1, "still on the Wall");
    assert!(screen(&mut m).join("\n").contains("answer 1b 2a"));
    press(&mut m, "enter");
    assert_eq!(
        *fake.replies.lock().unwrap(),
        [(
            "default/d".to_string(),
            "1b. Subagents on the Wall: small tiles\n\n2a. Scope: all six".to_string()
        )]
    );
    assert_eq!(m.agents_page, 1, "still on the Wall");
    let f = screen(&mut m).join("\n");
    assert!(!f.contains("1 Subagents on the Wall"), "answered:\n{f}");
}

/// A decision's long topic and long option labels wrap under their own
/// words at any width; nothing is cut with …
#[test]
fn a_tiles_long_decisions_wrap_instead_of_being_cut() {
    use rush_convo::decisions::{Decision, DecisionOption};
    let long = "skip the timing and merge on the passing tests and the first round's numbers (Codex: end shown at once, ~135→80 MB)";
    let d = Decision {
        r#ref: "1".into(),
        topic: format!("Timing the read-from-end change: {long}"),
        options: vec![
            DecisionOption {
                key: "a".into(),
                label: long.into(),
                recommended: true,
                ..Default::default()
            },
            DecisionOption {
                key: "b".into(),
                label: "short".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    for w in [30, 44, 60, 100] {
        let rows: Vec<String> =
            wall::decision_lines(std::slice::from_ref(&d), &Default::default(), w)
                .iter()
                .map(|l| l.plain())
                .collect();
        let all = rows.join("\n");
        assert!(!all.contains('…'), "w={w}:\n{all}");
        assert!(
            rows.iter().all(|r| rush_core::text::width(r) <= w),
            "w={w}:\n{all}"
        );
        for word in ["merge", "numbers", "Codex:", "MB)", "short"] {
            assert!(all.contains(word), "w={w}: {word:?} lost:\n{all}");
        }
    }
}
