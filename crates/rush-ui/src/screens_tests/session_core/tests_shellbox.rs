//! Shell commands rush runs itself (shellbox.rs): a shell block of a
//! reply run by a click on ▶ run or enter on it picked, c copying it, x
//! stopping it, r running it again, ✎ edit changing it before it runs;
//! → send sending the command and its output to the agent, ✎ draft
//! putting them in the message box, ≈ summary sending what the quick
//! model made of them (and nothing going to the agent by itself); `!`
//! in the empty box turning it to running commands, esc
//! and a backspace at its start turning it back, enter running one in
//! the session's folder, drawn in the conversation; and the runs kept on
//! disk for the session opened again. Nothing runs: a fake stands in for
//! the shell.

use std::sync::{Arc, Mutex};

use rush_core::adapter::Profile;
use rush_core::agent::State;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::text::width;
use rush_draw::{Button, KeyMods, MouseKind};

use super::host::{Fake, Op};
use super::tests::{KEY, agent, open, press, screen, sess, typed};
use super::viewsuggest::{self, Ask, Call};
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
    replied_as(fake, keep, false)
}

/// The same in a session rush hosts, what's sent to it a Send to fake.
fn replied_hosted(fake: &Arc<Fake>) -> Model {
    let mut m = replied_as(fake, false, true);
    assert!(conn(&mut m, KEY).hosted(), "the link is open");
    m
}

fn replied_as(fake: &Arc<Fake>, keep: bool, host: bool) -> Model {
    shellbox::set_fake(Some(printed));
    shellbox::set_keep(keep);
    let kind = if host { "steer" } else { "plain" };
    if host {
        fake.host(KEY);
    }
    let mut m = open(agent(kind, State::Idle), fake, 120, 40);
    if host {
        // The link open first: it takes the conversation as its host has
        // it (nothing), and this turn goes in after.
        super::live::sync(&mut m);
    }
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
    m.session.as_mut().unwrap().read = true;
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
    assert!(s.contains("▶ run  ✎ edit  ⧉ copy"), "{s}");
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
    assert!(s.contains("→ send"), "{s}");
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
    assert!(
        s.contains("enter runs it · e edits it · c copies it"),
        "{s}"
    );
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

const SENT: &str = "I ran `rm ~/x.p12` (exit 0):\n```\nran rm ~/x.p12 in /src/checkout\n```";

/// What was sent to the agent, in order.
fn sends(fake: &Fake) -> Vec<String> {
    fake.ops()
        .into_iter()
        .filter_map(|o| match o {
            Op::Send { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

/// The finished box's foot: → send, ✎ draft, ≈ summary, ↻, ✕.
#[test]
fn a_finished_box_has_three_ways_to_send() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    pick_the_box(&mut m);
    press(&mut m, "enter");
    let s = shown(&mut m);
    let foot = s
        .lines()
        .find(|l| l.contains("✓ exit 0"))
        .expect(&s)
        .to_string();
    assert!(foot.contains("→ send"), "{foot}");
    assert!(foot.contains("✎ draft"), "{foot}");
    assert!(foot.contains("≈ summary"), "{foot}");
    assert!(foot.contains('↻') && foot.contains('✕'), "{foot}");
    // Its keys are on the bottom row.
    assert!(s.contains("send to agent"), "{s}");
    assert!(s.contains("draft it first"), "{s}");
    assert!(s.contains("summarise and send"), "{s}");
}

/// → send sends the command and its output to the agent at once, as a
/// message you typed; what you were typing stays in the box.
#[test]
fn send_sends_it_to_the_agent() {
    let fake = Fake::new();
    let mut m = replied_hosted(&fake);
    pick_the_box(&mut m);
    press(&mut m, "enter");
    let _ = fake.ops();
    conn(&mut m, KEY).input.set("half a thought");
    let (x, y) = cell_of(&mut m, "→ send");
    click(&mut m, x + 2, y);
    assert_eq!(sends(&fake), [SENT]);
    assert_eq!(conn(&mut m, KEY).input.string(), "half a thought");
    let run = shellbox::run_of(&mut m, KEY, BOX).unwrap();
    assert!(run.told && !run.sent);
    assert!(shown(&mut m).contains("✓ sent"));
}

/// s on the picked box sends it too.
#[test]
fn s_sends_it_to_the_agent() {
    let fake = Fake::new();
    let mut m = replied_hosted(&fake);
    pick_the_box(&mut m);
    press(&mut m, "enter");
    let _ = fake.ops();
    press(&mut m, "s");
    assert_eq!(sends(&fake), [SENT]);
    assert!(conn(&mut m, KEY).input.is_empty());
}

/// ✎ draft (d) puts the command and its output in the message box, for
/// you to change and send; nothing is sent until you do.
#[test]
fn draft_fills_the_box() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    pick_the_box(&mut m);
    press(&mut m, "enter");
    let (x, y) = cell_of(&mut m, "✎ draft");
    click(&mut m, x + 2, y);
    assert_eq!(conn(&mut m, KEY).input.string(), SENT);
    assert!(fake.ops().is_empty(), "nothing sent");
    assert!(fake.replies.lock().unwrap().is_empty());
    assert!(shown(&mut m).contains("✓ in"));
    // d does the same.
    let mut m = replied(&fake);
    pick_the_box(&mut m);
    press(&mut m, "enter");
    press(&mut m, "d");
    assert_eq!(conn(&mut m, KEY).input.string(), SENT);
    assert!(fake.ops().is_empty(), "nothing sent");
}

/// The quick model, answering the same each time and keeping what it was
/// asked.
struct Quick {
    answer: Result<String, String>,
    asked: Mutex<Vec<String>>,
}

impl Ask for Quick {
    fn models(&self, _: &Profile) -> (String, String) {
        ("quick".into(), "careful".into())
    }
    fn ask(&self, _: &Profile, c: &Call) -> (f64, Result<Vec<u8>, String>) {
        assert_eq!(c.model, "quick");
        self.asked.lock().unwrap().push(c.prompt.clone());
        (0.0, self.answer.clone().map(String::into_bytes))
    }
}

fn quick(m: &mut Model, answer: Result<&str, &str>) -> Arc<Quick> {
    let q = Arc::new(Quick {
        answer: answer.map(str::to_string).map_err(str::to_string),
        asked: Mutex::new(Vec::new()),
    });
    viewsuggest::use_asker(m, q.clone());
    q
}

/// ≈ summary (m): the quick model sums the run up, and its summary goes
/// to the agent at once under a line saying it's one.
#[test]
fn summary_sends_what_the_quick_model_made_of_it() {
    let fake = Fake::new();
    let mut m = replied_hosted(&fake);
    let q = quick(&mut m, Ok(r#"{"summary":"removed it; no errors"}"#));
    pick_the_box(&mut m);
    press(&mut m, "enter");
    let _ = fake.ops();
    press(&mut m, "m");
    let asked = q.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0].starts_with("This command was run `rm ~/x.p12` (exit 0):"),
        "{}",
        asked[0]
    );
    assert!(asked[0].contains("ran rm ~/x.p12 in /src/checkout"));
    assert_eq!(
        sends(&fake),
        ["A summary of a command I ran, `rm ~/x.p12` (exit 0):\nremoved it; no errors"]
    );
    let run = shellbox::run_of(&mut m, KEY, BOX).unwrap();
    assert!(run.told_summary && !run.summing && run.note.is_empty());
    assert!(shown(&mut m).contains("✓ summary"));
}

/// A summary that fails sends nothing: the box says why, and ≈ summary
/// is there to try again.
#[test]
fn a_failed_summary_sends_nothing() {
    let fake = Fake::new();
    let mut m = replied_hosted(&fake);
    quick(&mut m, Err("timed out"));
    pick_the_box(&mut m);
    press(&mut m, "enter");
    let _ = fake.ops();
    let (x, y) = cell_of(&mut m, "≈ summary");
    click(&mut m, x + 2, y);
    assert!(sends(&fake).is_empty(), "nothing sent");
    assert!(conn(&mut m, KEY).input.is_empty());
    let run = shellbox::run_of(&mut m, KEY, BOX).unwrap();
    assert!(!run.told && !run.summing, "{run:?}");
    assert_eq!(run.note, "couldn't sum it up: timed out");
    let s = shown(&mut m);
    assert!(s.contains("✗ couldn't sum it up: timed out"), "{s}");
    assert!(s.contains("≈ summary"), "{s}");
}

// ---- editing a block's commands before they run ----

/// ✎ edit (e) turns the box to an editor of its commands: what's typed
/// changes them, ⇧enter (ctrl+j) adds a line, enter runs them as
/// edited. The box shows and sends the edited commands, marked edited;
/// the agent's reply is as it was.
#[test]
fn edit_then_run_runs_the_edited_commands() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    let (x, y) = cell_of(&mut m, "✎ edit");
    click(&mut m, x + 2, y);
    assert!(shellbox::editing(&mut m, KEY));
    let s = shown(&mut m);
    assert!(s.contains("shell · editing"), "{s}");
    assert!(s.contains("enter runs it"), "{s}");
    assert!(s.contains("esc as it was"), "{s}");
    typed(&mut m, " -f");
    press(&mut m, "ctrl+j");
    typed(&mut m, "ls");
    assert!(shellbox::ran().is_empty(), "nothing runs while it's edited");
    assert!(
        conn(&mut m, KEY).input.is_empty(),
        "nothing typed in the box"
    );
    let s = shown(&mut m);
    assert!(s.contains("$ rm ~/x.p12 -f"), "{s}");
    assert!(s.contains("$ ls"), "{s}");
    press(&mut m, "enter");
    assert!(!shellbox::editing(&mut m, KEY));
    assert_eq!(
        shellbox::ran(),
        [("rm ~/x.p12 -f\nls".to_string(), "/src/checkout".to_string())]
    );
    let run = shellbox::run_of(&mut m, KEY, BOX).unwrap();
    assert!(run.edited);
    let text = run.send_text();
    assert!(
        text.starts_with("I ran this (exit 0):\n```sh\nrm ~/x.p12 -f\nls\n```"),
        "{text}"
    );
    let s = shown(&mut m);
    assert!(s.contains("shell · edited"), "{s}");
    assert!(s.contains("✓ exit 0"), "{s}");
    assert!(s.contains("$ rm ~/x.p12 -f"), "{s}");
}

/// esc leaves the commands as they were: nothing runs, and enter then
/// runs the agent's.
#[test]
fn esc_keeps_the_commands_as_they_were() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    pick_the_box(&mut m);
    press(&mut m, "e");
    assert!(shellbox::editing(&mut m, KEY));
    typed(&mut m, " -f");
    press(&mut m, "esc");
    assert!(!shellbox::editing(&mut m, KEY));
    assert!(shellbox::ran().is_empty());
    let s = shown(&mut m);
    assert!(!s.contains("editing"), "{s}");
    assert!(!s.contains("-f"), "{s}");
    press(&mut m, "enter");
    assert_eq!(
        shellbox::ran(),
        [("rm ~/x.p12".to_string(), "/src/checkout".to_string())]
    );
    assert!(!shellbox::run_of(&mut m, KEY, BOX).unwrap().edited);
}

/// ✎ edit on a finished box: enter runs it again as edited, and ↻ after
/// that runs the edited commands too.
#[test]
fn edit_on_a_finished_box_runs_it_again_edited() {
    let fake = Fake::new();
    let mut m = replied(&fake);
    pick_the_box(&mut m);
    press(&mut m, "enter");
    assert_eq!(shellbox::ran().len(), 1);
    press(&mut m, "e");
    assert!(shellbox::editing(&mut m, KEY));
    typed(&mut m, " -f");
    press(&mut m, "enter");
    let ran = shellbox::ran();
    assert_eq!(ran.len(), 2);
    assert_eq!(ran[1].0, "rm ~/x.p12 -f");
    assert!(shown(&mut m).contains("shell · edited"));
    press(&mut m, "r");
    assert_eq!(shellbox::ran()[2].0, "rm ~/x.p12 -f", "↻ keeps the edit");
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
/// went to the agent. Its ✎ draft fills the box as a block's.
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
    let (x, y) = cell_of(&mut m, "✎ draft");
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
    assert!(s.contains("→ send"), "{s}");
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
