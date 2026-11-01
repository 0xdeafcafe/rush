//! The Wall's subagent tiles and its mouse, after Go's wall.go (wallItems
//! :136, wallGroups :150, wallSubTile :684, wallKey :1001, wallClick
//! :1066), and its decisions answered on their tile with the session's
//! card.

use rush_core::agent::{State, SubagentTile};
use rush_core::text::Line;
use rush_draw::{Button, KeyMods, MouseKind};

use super::tests::{agent, model_of};
use super::wall;
use crate::frame::{Mouse, Out, Press};
use crate::model::Model;
use crate::screens::session_cards::question::decisions_of;
use crate::screens::session_cards::{Act, Host, HostInfo, ModelCards};

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

const SAID: &str = "Here's what I found.\n\nDecisions:\n\
1. **Subagents on the Wall:** (a) fold them in; (b) small tiles.\n\
2. **Scope:** (a) all six; (b) two.\n\
My picks: 1a 2a.";

/// A Wall whose one tile, agent d's, ends its last reply with SAID's
/// decisions, in a terminal w×h; the Wall open.
fn deciding(w: usize, h: usize) -> Model {
    use rush_core::agent::{Preview, PreviewLine};
    let mut a = agent("d", "Making Rush Screen More Useful", State::Done, 101);
    a.interactive = false;
    let mut m = model_of(vec![a], w, h);
    m.previews.insert(
        "default/d".into(),
        Preview {
            recent: vec![PreviewLine {
                role: "assistant".into(),
                text: SAID.into(),
                at: 5,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    press(&mut m, "]");
    m
}

/// A host for the cards that keeps what's asked of each session: d's
/// open decisions card is SAID's, as d1.
#[derive(Default)]
struct Asked(Vec<(String, Act)>);

fn asked_info(_: &Model, key: &str) -> Option<HostInfo> {
    (key == "default/d").then(|| HostInfo {
        kind: "claude".into(),
        cold: true,
        wakes: true,
        decisions: decisions_of(SAID, "d1"),
        ..HostInfo::default()
    })
}

fn asked_act(m: &mut Model, key: &str, a: Act) -> Out {
    m.kept::<Asked>().0.push((key.into(), a));
    Out::Draw
}

fn recording(m: &mut Model) {
    m.cards().host = Some(Host {
        info: asked_info,
        act: asked_act,
        set_input: |_, _, _| {},
        ask: |_, _, _, _| false,
        setting: |_, _| String::new(),
        save_setting: |_, _, _| Ok(()),
    });
}

fn acts(m: &mut Model) -> Vec<Act> {
    m.kept::<Asked>().0.iter().map(|(_, a)| a.clone()).collect()
}

/// A tile whose last reply ends with decisions shows them; enter opens
/// their card on the tile, where ↓ and enter pick, a digit picks, and
/// enter on the review sends the answers as the session's card does,
/// through the session's own path: here its agent's daemon. The tile
/// stops asking, and the card's state, the session's, has it answered.
#[test]
fn answers_decisions_from_the_wall() {
    use std::sync::Arc;

    use crate::host::{Fake, set_hosts};

    let mut m = deciding(120, 40);
    m.cards().host = Some(crate::screens::session_core::cards_host::host());
    let fake = Fake::new();
    set_hosts(&mut m, Arc::new(fake.clone()));
    let f = screen(&mut m).join("\n");
    for t in [
        "1 Subagents on the Wall",
        "a fold them in  b small tiles",
        "2 Scope",
        "2 decisions to answer",
        "enter answer",
    ] {
        assert!(f.contains(t), "{t:?} on the tile:\n{f}");
    }
    assert!(
        !f.contains("Decisions:"),
        "the block isn't in the reply:\n{f}"
    );
    press(&mut m, "enter");
    let f = screen(&mut m).join("\n");
    for t in [
        "? decide",
        "2 decisions, yours to make",
        "1. Subagents on the Wall",
        "fold them in  ★ its pick",
        "a takes its picks",
        "Something else  opens the session to type your own",
    ] {
        assert!(f.contains(t), "{t:?} on the card:\n{f}");
    }
    assert!(
        !f.contains("type your own answer below"),
        "no box here:\n{f}"
    );
    for k in ["down", "enter", "1"] {
        press(&mut m, k);
    }
    assert!(
        screen(&mut m).join("\n").contains("Send these answers?"),
        "{}",
        screen(&mut m).join("\n")
    );
    assert_eq!(m.agents_page, 1, "still on the Wall");
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
    assert!(!f.contains("? decide"), "the card's closed:\n{f}");
    let c = &m.cards().conns["default/d"];
    assert!(c.q_gone.starts_with("wall-") && c.dec_answered == c.q_gone);
}

/// The card on the tile is the session's: its id, and its answer as the
/// card writes it. A click on an option picks it at once, and a takes
/// the agent's picks for the rest and sends.
#[test]
fn a_click_picks_and_a_takes_its_picks_on_a_tile() {
    let mut m = deciding(120, 40);
    recording(&mut m);
    // A click on the tile opens its card; one on an option picks it.
    let f = screen(&mut m);
    let click = |m: &mut Model, x: usize, y: usize| {
        m.mouse_event(&Mouse {
            kind: MouseKind::Down(Button::Left),
            x: x as u16,
            y: y as u16,
            mods: KeyMods::NONE,
        })
    };
    let (y, l) = f
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains("Scope"))
        .expect("the tile");
    click(
        &mut m,
        rush_core::text::width(&l[..l.find("Scope").unwrap()]),
        y,
    );
    let f = screen(&mut m);
    assert!(f.join("\n").contains("? decide"), "{}", f.join("\n"));
    let (y, l) = f
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains("small tiles"))
        .expect("option b");
    click(
        &mut m,
        rush_core::text::width(&l[..l.find("small").unwrap()]),
        y,
    );
    let c = &m.cards().conns["default/d"];
    assert_eq!(c.q_idx, 1, "picked, on to the next");
    assert_eq!(c.q_answer["1. Subagents on the Wall"], "small tiles");
    press(&mut m, "a");
    assert_eq!(
        acts(&mut m),
        vec![Act::Answer {
            id: "d1".into(),
            text: "1b. Subagents on the Wall: small tiles\n\n2a. Scope: all six".into()
        }]
    );
    assert!(!screen(&mut m).join("\n").contains("? decide"));
    assert_eq!(m.agents_page, 1, "still on the Wall");
}

/// a straight away sends every one of the agent's picks.
#[test]
fn a_sends_its_picks_from_a_tile() {
    let mut m = deciding(120, 40);
    recording(&mut m);
    press(&mut m, "enter");
    press(&mut m, "a");
    assert_eq!(
        acts(&mut m),
        vec![Act::Answer {
            id: "d1".into(),
            text: "1a. Subagents on the Wall: fold them in\n\n2a. Scope: all six".into()
        }]
    );
}

/// esc and s on a tile's card send nothing: the card is set aside as
/// the session's is (the host told, nothing to the agent), and the tile
/// stops asking; the Wall stays.
#[test]
fn esc_and_s_send_nothing_from_a_tile() {
    for k in ["esc", "s"] {
        let mut m = deciding(120, 40);
        recording(&mut m);
        press(&mut m, "enter");
        press(&mut m, "down");
        press(&mut m, k);
        assert_eq!(
            acts(&mut m),
            vec![Act::DismissDecisions("d1".into())],
            "{k}"
        );
        assert_eq!(m.agents_page, 1, "{k}: still on the Wall");
        let f = screen(&mut m).join("\n");
        assert!(!f.contains("? decide"), "{k}: closed:\n{f}");
        assert!(!f.contains("2 decisions to answer"), "{k}: set aside:\n{f}");
    }
}

/// A rush session whose host keeps its decisions card open asks on its
/// tile with no transcript to read: the card is its host's, so what the
/// tile sends answers the session's own card, by its id.
#[test]
fn a_rush_sessions_host_card_is_answered_on_its_tile() {
    let mut a = agent("r", "live alpha", State::Blocked, 101);
    a.interactive = false;
    a.decides = true;
    a.decisions_key = "Subagents on the Wall\x1fScope\x1f".into();
    let mut m = model_of(vec![a.clone()], 120, 40);
    recording(&mut m);
    let mut q = decisions_of(SAID, "decisions-host1").unwrap();
    q.id = "decisions-host1".into();
    m.kept::<wall::WallState>()
        .hosted
        .insert(a.key.clone(), (a.decisions_key.clone(), Some(q)));
    press(&mut m, "]");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("2 decisions to answer"), "{f}");
    press(&mut m, "enter");
    press(&mut m, "a");
    assert_eq!(
        acts(&mut m),
        vec![Act::Answer {
            id: "decisions-host1".into(),
            text: "1a. Subagents on the Wall: fold them in\n\n2a. Scope: all six".into()
        }]
    );
    assert!(!screen(&mut m).join("\n").contains("2 decisions to answer"));
}

/// A tile while its decisions are answered on it, at 120×40: its card in
/// place of its stream, the agent's pick under the cursor.
#[test]
fn snapshot_a_tile_answering() {
    let mut m = deciding(120, 40);
    recording(&mut m);
    press(&mut m, "enter");
    press(&mut m, "down");
    // The Wall's body and keys, not the header: it reads the machine.
    let got = screen(&mut m)[3..].join("\n") + "\n";
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/wall_answering_120x40.txt");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !p.exists() {
        std::fs::write(&p, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap();
    assert_eq!(got, want, "differs; UPDATE_SNAPSHOTS=1 to accept");
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
