//! Shell commands rush runs itself (shellbox.rs): a shell block of a
//! reply run by a click on ▶ run or enter on it picked, c copying it, x
//! stopping it, r running it again, → send to agent putting the command
//! and its output in the message box (and nothing going to the agent by
//! itself); `!` in the empty box turning it to running commands, esc
//! and a backspace at its start turning it back, enter running one in
//! the session's folder, drawn in the conversation; and the runs kept on
//! disk for the session opened again. Nothing runs: a fake stands in for
//! the shell.

use std::sync::Arc;

use rush_core::agent::State;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::text::width;
use rush_draw::{Button, KeyMods, MouseKind};

use super::host::Fake;
use super::tests::{KEY, agent, open, press, screen, sess, typed};
use super::{conn, core, shellbox};
use crate::frame::Mouse;
use crate::model::Model;

const REPLY: &str = "Remove the key:\n\n```sh\n$ rm ~/x.p12\n```\n\nThen you're done.";
const BOX: &str = "t1:sh:0:0";

fn printed(cmd: &str, cwd: &str) -> Option<(String, i32)> {
    Some((format!("ran {cmd} in {cwd}\n"), 0))
}

fn hangs(_: &str, _: &str) -> Option<(String, i32)> {
    None
}

/// The session open with one turn: a question and a reply holding a
/// shell block; its view in memory only, no map beside it.
fn replied(fake: &Arc<Fake>) -> Model {
    replied_keeping(fake, false)
}

/// The same, its runs kept on disk when keep.
fn replied_keeping(fake: &Arc<Fake>, keep: bool) -> Model {
    shellbox::set_fake(Some(printed));
    shellbox::set_keep(keep);
    let mut m = open(agent("plain", State::Idle), fake, 120, 40);
    crate::accounts::use_store(&mut m, rush_state::Store::default());
    crate::accounts::accounts(&mut m).read_only = true;
    core(&mut m).hide_map = true;
    let now = m.now;
    sess(&mut m).sent("how do I clean up?".into(), Vec::new(), now - 60_000);
    sess(&mut m).apply(
        &Event::Message(Message {
            role: "assistant".into(),
            id: "a1".into(),
            parts: vec![Part {
                kind: PartKind::Text,
                text: REPLY.into(),
                ..Part::default()
            }],
            ..Message::default()
        }),
        now - 59_000,
    );
    sess(&mut m).apply(
        &Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            ..TurnEnd::default()
        }),
        now - 58_000,
    );
    let _ = screen(&mut m);
    m
}

fn shown(m: &mut Model) -> String {
    // Twice: what a frame's dock takes in shows on the next.
    let _ = screen(m);
    screen(m).join("\n")
}

/// Where needle is drawn: its 0-based (column, row), in cells.
fn cell_of(m: &mut Model, needle: &str) -> (usize, usize) {
    let rows = screen(m);
    for (y, r) in rows.iter().enumerate() {
        if let Some(i) = r.find(needle) {
            return (width(&r[..i]), y);
        }
    }
    panic!("{needle:?} isn't on screen:\n{}", rows.join("\n"));
}

fn click(m: &mut Model, x: usize, y: usize) {
    for kind in [MouseKind::Down(Button::Left), MouseKind::Up(Button::Left)] {
        m.mouse_event(&Mouse {
            kind,
            x: x as u16,
            y: y as u16,
            mods: KeyMods::NONE,
        });
    }
}

fn pick_the_box(m: &mut Model) {
    for _ in 0..6 {
        if m.session.as_ref().is_some_and(|v| v.sel == BOX) {
            return;
        }
        press(m, "up");
        let _ = screen(m);
    }
    panic!(
        "↑ never picked the box; picked {:?}:\n{}",
        m.session.as_ref().map(|v| v.sel.clone()),
        screen(m).join("\n")
    );
}

/// The block is drawn as a box with ▶ run and ⧉ copy; a click on ▶ run
/// runs it in the session's folder, and its output and exit show in the
/// box. Nothing goes to the agent.
#[test]
fn a_click_on_run_runs_the_block() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    let s = shown(&mut m);
    assert!(s.contains("shell · /src/checkout"), "{s}");
    assert!(s.contains("▶ run  ⧉ copy"), "{s}");
    assert!(s.contains("$ rm ~/x.p12"), "{s}");
    assert!(shellbox::ran().is_empty(), "nothing runs by itself");
    let (x, y) = cell_of(&mut m, "▶ run");
    click(&mut m, x + 1, y);
    assert_eq!(
        shellbox::ran(),
        [("rm ~/x.p12".to_string(), "/src/checkout".to_string())]
    );
    let s = shown(&mut m);
    assert!(s.contains("ran rm ~/x.p12 in /src/checkout"), "{s}");
    assert!(s.contains("✓ exit 0"), "{s}");
    assert!(s.contains("→ send to agent"), "{s}");
    assert!(!s.contains("▶ run"), "{s}");
    assert!(fake.ops().is_empty(), "nothing went to the agent");
    assert!(conn(&mut m, KEY).input.is_empty());
}

/// A click on ⧉ copy copies the command as it runs, without the $.
#[test]
fn a_click_on_copy_copies_the_command() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    let (x, y) = cell_of(&mut m, "⧉ copy");
    click(&mut m, x + 1, y);
    assert_eq!(m.pending_copy.as_deref(), Some("rm ~/x.p12"));
    assert!(shellbox::ran().is_empty());
}

/// Picked with ↑, c copies it and enter runs it; enter again opens all
/// it wrote; r runs it again; x clears it, the box as before.
#[test]
fn keys_on_the_picked_block() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    pick_the_box(&mut m);
    let s = shown(&mut m);
    assert!(s.contains("enter runs it · c copies it"), "{s}");
    press(&mut m, "c");
    assert_eq!(m.pending_copy.as_deref(), Some("rm ~/x.p12"));
    assert!(shellbox::ran().is_empty(), "c doesn't run it");
    assert!(conn(&mut m, KEY).input.is_empty(), "c isn't typed");
    press(&mut m, "enter");
    assert_eq!(shellbox::ran().len(), 1);
    assert!(shown(&mut m).contains("✓ exit 0"));
    press(&mut m, "enter");
    assert_eq!(shellbox::ran().len(), 1, "enter on a run one opens it");
    assert_eq!(
        m.session.as_ref().unwrap().o.open.get(BOX).copied(),
        Some(true)
    );
    press(&mut m, "r");
    assert_eq!(shellbox::ran().len(), 2, "r runs it again");
    press(&mut m, "x");
    assert!(shellbox::run_of(&mut m, KEY, BOX).is_none(), "x clears it");
    assert!(shown(&mut m).contains("▶ run"));
}

/// A run that goes on: its foot says so with ✕ stop; x stops it.
#[test]
fn x_stops_a_run() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    shellbox::set_fake(Some(hangs));
    pick_the_box(&mut m);
    press(&mut m, "enter");
    let run = shellbox::run_of(&mut m, KEY, BOX).expect("it runs");
    assert!(run.running());
    let s = shown(&mut m);
    assert!(s.contains("✕ stop"), "{s}");
    press(&mut m, "x");
    let run = shellbox::run_of(&mut m, KEY, BOX).expect("kept");
    assert!(!run.running() && run.stopped);
    assert!(shown(&mut m).contains("✕ stopped"));
}

/// → send to agent puts the command and its output in the message box,
/// for you to send; nothing is sent until you do.
#[test]
fn send_to_agent_fills_the_box() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    pick_the_box(&mut m);
    press(&mut m, "enter");
    let (x, y) = cell_of(&mut m, "→ send to agent");
    click(&mut m, x + 2, y);
    let text = conn(&mut m, KEY).input.string();
    assert_eq!(
        text,
        "I ran `rm ~/x.p12` (exit 0):\n```\nran rm ~/x.p12 in /src/checkout\n```"
    );
    assert!(fake.ops().is_empty(), "nothing sent");
    assert!(fake.replies.lock().unwrap().is_empty());
    assert!(shown(&mut m).contains("✓ in your message"));
}

/// `!` into the empty box turns it to running commands: its edge says
/// so; esc turns it back, and so does a backspace at its start.
#[test]
fn bang_mode_in_and_out() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    typed(&mut m, "!");
    assert!(conn(&mut m, KEY).bang);
    assert!(conn(&mut m, KEY).input.is_empty(), "the ! isn't typed");
    let s = shown(&mut m);
    assert!(s.contains("! shell · /src/checkout"), "{s}");
    assert!(s.contains("enter runs · esc back"), "{s}");
    press(&mut m, "esc");
    assert!(!conn(&mut m, KEY).bang, "esc goes back");
    typed(&mut m, "!");
    typed(&mut m, "ls");
    press(&mut m, "backspace");
    press(&mut m, "backspace");
    assert!(conn(&mut m, KEY).bang, "still, with what's typed gone");
    press(&mut m, "backspace");
    assert!(
        !conn(&mut m, KEY).bang,
        "a backspace at its start goes back"
    );
    // ! with something typed is only a !.
    typed(&mut m, "hi!");
    assert!(!conn(&mut m, KEY).bang);
    assert_eq!(conn(&mut m, KEY).input.string(), "hi!");
}

/// enter runs what's typed after ! in the session's folder, drawn as a
/// box after the turn's reply; the box is back to messages, and nothing
/// went to the agent. Its → send to agent fills the box as a block's.
#[test]
fn bang_enter_runs_it_here() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    typed(&mut m, "!");
    typed(&mut m, "git status");
    press(&mut m, "enter");
    assert_eq!(
        shellbox::ran(),
        [("git status".to_string(), "/src/checkout".to_string())]
    );
    assert!(!conn(&mut m, KEY).bang);
    assert!(conn(&mut m, KEY).input.is_empty());
    let s = shown(&mut m);
    let reply = s.find("Then you're done.").expect(&s);
    let ran = s.find("$ git status").expect(&s);
    assert!(ran > reply, "after the reply:\n{s}");
    assert!(s.contains("ran git status in /src/checkout"), "{s}");
    assert!(fake.ops().is_empty(), "nothing went to the agent");
    let (x, y) = cell_of(&mut m, "→ send to agent");
    click(&mut m, x + 2, y);
    assert!(
        conn(&mut m, KEY)
            .input
            .string()
            .starts_with("I ran `git status` (exit 0):"),
        "{}",
        conn(&mut m, KEY).input.string()
    );
    assert!(fake.ops().is_empty(), "still nothing sent");
}

/// What ran is there when the session is opened again: kept in rush's
/// folder, read back by the next model.
#[test]
fn runs_are_kept_for_the_session() {
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().to_path_buf()));
    let fake = Fake::new();
    let mut m = replied_keeping(&fake, true);
    pick_the_box(&mut m);
    press(&mut m, "enter");
    assert!(shellbox::file_of(KEY).starts_with(home.path()));
    assert!(shellbox::file_of(KEY).exists(), "kept on disk");
    let mut again = replied_keeping(&fake, true);
    let s = shown(&mut again);
    assert!(s.contains("✓ exit 0"), "{s}");
    assert!(s.contains("ran rm ~/x.p12 in /src/checkout"), "{s}");
    rush_state::set_thread_dir(None);
}

/// `!` in a session that has had no turn yet: enter runs it, and its box
/// is the conversation, in place of "nothing yet".
#[test]
fn bang_in_a_session_with_no_turn() {
    shellbox::set_fake(Some(printed));
    shellbox::set_keep(false);
    let fake = Fake::new();
    let mut m = open(agent("plain", State::Idle), &fake, 120, 40);
    crate::accounts::use_store(&mut m, rush_state::Store::default());
    crate::accounts::accounts(&mut m).read_only = true;
    core(&mut m).hide_map = true;
    assert!(shown(&mut m).contains("nothing yet"));
    typed(&mut m, "!");
    assert!(conn(&mut m, KEY).bang);
    typed(&mut m, "git status");
    press(&mut m, "enter");
    assert_eq!(
        shellbox::ran(),
        [("git status".to_string(), "/src/checkout".to_string())]
    );
    let s = shown(&mut m);
    assert!(!s.contains("nothing yet"), "{s}");
    assert!(s.contains("$ git status"), "{s}");
    assert!(s.contains("ran git status in /src/checkout"), "{s}");
    assert!(s.contains("→ send to agent"), "{s}");
    let r = m
        .session
        .as_ref()
        .unwrap()
        .o
        .shells
        .keys()
        .next()
        .cloned()
        .expect("kept");
    assert!(r.starts_with("t0:bang:"), "{r}");
    assert!(fake.ops().is_empty(), "nothing went to the agent");
}

/// Runs rush was closed on, read back as the session opens: one whose
/// process is still the same one runs on in its box with what it wrote
/// so far; one whose code was written is done with it; one with
/// neither ended while rush was closed. The records are written as rush
/// writes them; this test's own process stands in for the one still
/// running (and is never stopped).
#[test]
fn runs_rush_was_closed_on_are_picked_up() {
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().to_path_buf()));
    let fake = Fake::new();
    let now = replied_keeping(&fake, false).now;
    let me = std::process::id() as i32;
    let my_start = shellbox::started_ms(me);
    let log = |r: &str, at: i64, text: &str| {
        let p = shellbox::log_of(KEY, r, at);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    };
    let (a, b, c) = (now - 30_000, now - 20_000, now - 10_000);
    let (ra, rb, rc) = (
        BOX.to_string(),
        format!("t1:bang:{b}"),
        format!("t1:bang:{c}"),
    );
    let done = log(&rb, b, "built\n");
    std::fs::write(shellbox::exit_of(std::path::Path::new(&done)), "4\n").unwrap();
    let mut runs = serde_json::Map::new();
    runs.insert(
        ra.clone(),
        serde_json::json!({
            "command": "rm ~/x.p12", "cwd": "/src/checkout", "start": a,
            "pid": me, "pid_start": my_start, "log": log(&ra, a, "removing…\n"),
        }),
    );
    runs.insert(
        rb.clone(),
        serde_json::json!({
            "command": "make", "cwd": "/src/checkout", "bang": true, "turn": 1,
            "after": 1, "start": b, "pid": me, "pid_start": my_start - 60_000,
            "log": done,
        }),
    );
    runs.insert(
        rc.clone(),
        serde_json::json!({
            "command": "sleep 100", "cwd": "/src/checkout", "bang": true, "turn": 1,
            "after": 1, "start": c, "pid": me, "pid_start": my_start - 60_000,
            "log": log(&rc, c, "zzz\n"),
        }),
    );
    let file = serde_json::json!({ "runs": runs });
    let path = shellbox::file_of(KEY);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

    let mut m = replied_keeping(&fake, true);
    let s = shown(&mut m);
    let run = shellbox::run_of(&mut m, KEY, &ra).expect("read back");
    assert!(run.running(), "{run:?}");
    assert!(s.contains("removing…"), "{s}");
    assert!(s.contains("✕ stop"), "{s}");
    let run = shellbox::run_of(&mut m, KEY, &rb).expect("read back");
    assert_eq!((run.running(), run.exit), (false, Some(4)));
    assert!(s.contains("✗ exit 4"), "{s}");
    assert!(s.contains("built"), "{s}");
    let run = shellbox::run_of(&mut m, KEY, &rc).expect("read back");
    assert!(run.lost && !run.running(), "{run:?}");
    assert!(s.contains("ended while rush was closed"), "{s}");
    assert!(s.contains("zzz"), "{s}");
    drop(m);
    rush_state::set_thread_dir(None);
}
