//! A long call rush ran as a script shows the line it's on from its
//! trace, how long the lines before took, and a click on a line's number
//! sets a breakpoint there (scripts_test.go TestScriptViewsFollowTheTrace,
//! rushmode.go:5073). The trace is written by hand: no script runs.

use rush_convo::render::script_line_ref;
use rush_core::event::{Event, Message, Part, PartKind};
use rush_core::tool::{Call, Input, Kind as ToolKind};
use rush_draw::{Button, MouseKind};

use super::tests::{agent, flash, open, screen};
use super::{conn, scripts};
use crate::frame::Mouse;
use crate::model::Model;
use crate::screens::session_core::host::Fake;
use rush_core::agent::State;

const CMD: &str = "cd /tmp\necho a\nsleep 1\necho b";

/// A session whose latest call, id, runs CMD as a script, on line 3 of
/// it since 2s ago, line 1 having taken 500ms. Each test has its own id:
/// tests share the process's home.
fn running(id: &str) -> Model {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Working), &fake, 120, 40);
    let now = m.now;
    let key = m.session.as_ref().unwrap().key.clone();
    let s = &mut m.session.as_mut().unwrap().sess;
    s.sent("go".into(), Vec::new(), now - 4_000);
    s.apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "m1".into(),
            parts: vec![Part {
                kind: PartKind::ToolCall,
                call: Some(Box::new(Call {
                    id: id.into(),
                    name: "Bash".into(),
                    kind: ToolKind::Shell,
                    input: Input {
                        command: CMD.into(),
                        ..Input::default()
                    },
                    ..Call::default()
                })),
                ..Part::default()
            }],
            ..Message::default()
        }),
        now - 3_500,
    );
    let dir = rush_sys::script::Scripts::new(rush_state::dir());
    std::fs::create_dir_all(dir.dir()).unwrap();
    std::fs::write(dir.dir().join(format!("{id}.sh")), format!("{CMD}\n")).unwrap();
    let t0 = (now - 3_000) as f64 / 1000.0;
    let trace = format!(
        "pid 1 {}\n1 {t0:.6}\n2 {:.6}\n3 {:.6}\n",
        t0 as i64,
        t0 + 0.5,
        t0 + 1.0
    );
    std::fs::write(dir.dir().join(format!("{id}.trace")), trace).unwrap();
    // Read off the UI (at once, with no loop), shown from the next frame.
    scripts::sync(&mut m, &key);
    m
}

fn breaks(id: &str) -> Vec<i32> {
    rush_sys::script::Scripts::new(rush_state::dir()).breaks(id)
}

#[test]
fn a_script_follows_its_trace() {
    let mut m = running("b1");
    let key = m.session.as_ref().unwrap().key.clone();
    let v = m.session.as_ref().unwrap().o.scripts.get("b1").cloned();
    let v = v.expect("its view, once read");
    assert_eq!(
        (v.at, v.took.get(&1).copied(), v.held),
        (3, Some(500), false)
    );
    // The conversation draws it as the file it ran from.
    m.session.as_mut().unwrap().verbose = true;
    let s = screen(&mut m).join("\n");
    assert!(s.contains("running as a script"), "{s}");
    assert!(s.contains("3 ▸ sleep 1"), "{s}");

    let r = |n| script_line_ref("t1:s:b1", n);
    assert!(scripts::toggle_break(&mut m, &key, &r(4)).is_some());
    assert_eq!(breaks("b1"), [4], "clicking line 4 stops the script there");
    assert_eq!(
        flash(&m),
        "breakpoint at line 4 · the script stops when it gets there"
    );
    scripts::toggle_break(&mut m, &key, &r(4));
    assert!(breaks("b1").is_empty(), "clicking it again takes it away");
    assert!(scripts::toggle_break(&mut m, &key, &r(2)).is_some());
    assert!(breaks("b1").is_empty());
    assert_eq!(flash(&m), "line 2 has already run · pick a line below 3");
    conn(&mut m, &key).scripts.get_mut("b1").unwrap().live = false;
    assert!(scripts::toggle_break(&mut m, &key, &r(4)).is_some());
    assert!(
        breaks("b1").is_empty(),
        "a finished script takes no breakpoint"
    );
    // Not a script's line: left to the rest.
    assert!(scripts::toggle_break(&mut m, &key, "t1:s:b1").is_none());
}

/// A click on a line's number in the conversation sets the breakpoint.
#[test]
fn a_click_on_a_line_sets_a_breakpoint() {
    let mut m = running("b2");
    m.session.as_mut().unwrap().verbose = true;
    let s = screen(&mut m);
    let y = s
        .iter()
        .position(|l| l.contains("4   echo b"))
        .expect("line 4's row");
    for kind in [MouseKind::Down(Button::Left), MouseKind::Up(Button::Left)] {
        m.mouse_event(&Mouse {
            kind,
            x: 10,
            y: y as u16,
            mods: rush_draw::KeyMods::NONE,
        });
    }
    assert_eq!(breaks("b2"), [4]);
    // Its dot, once read again.
    let s = screen(&mut m).join("\n");
    assert!(s.contains("● 4   echo b"), "{s}");
}
