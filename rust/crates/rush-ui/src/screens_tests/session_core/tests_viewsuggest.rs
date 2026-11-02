//! Rush suggesting the conversation view that fits (viewsuggest.rs): off by
//! default and doing nothing then; a suggestion as you open a session, come
//! back to it, or a turn ends; waiting while you type; the key taking it;
//! switching only as you come back, with the key undoing it; the quick
//! model asked only when the rules can't tell, at most every 10 minutes,
//! with counts only; and three turned down in a row stopping one.

use std::sync::{Arc, Mutex};

use rush_convo::Preset;
use rush_core::adapter::Profile;
use rush_core::agent::State;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::time::Ms;
use rush_core::tool::{Call as ToolCall, Input, Kind as ToolKind, Output};

use super::host::Fake;
use super::presets;
use super::tests::{KEY, NOW, agent, open, press, screen, sess};
use super::viewsuggest::{self, Ask, Call, Mode};
use crate::model::Model;

/// The quick model, answering the same each time and keeping what it was
/// asked.
struct Quick {
    answer: Result<String, String>,
    asked: Mutex<Vec<String>>,
}

impl Quick {
    fn new(answer: Result<&str, &str>) -> Arc<Quick> {
        Arc::new(Quick {
            answer: answer.map(str::to_string).map_err(str::to_string),
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
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

/// A session with setting as its "Suggest a conversation view", its
/// config and what's learned in memory only (the tests share one home).
fn model(setting: &str, quick: &Arc<Quick>) -> Model {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    crate::accounts::use_store(&mut m, rush_state::Store::default());
    crate::accounts::accounts(&mut m).read_only = true;
    crate::accounts::config_mut(&mut m).view_suggest = setting.into();
    viewsuggest::in_memory(&mut m);
    viewsuggest::use_asker(&mut m, quick.clone());
    m
}

fn said(role: &str, id: String, part: Part) -> Event {
    Event::Message(Message {
        role: role.into(),
        id,
        parts: vec![part],
        ..Message::default()
    })
}

fn words(text: &str) -> Part {
    Part {
        kind: PartKind::Text,
        text: text.into(),
        ..Part::default()
    }
}

fn end(m: &mut Model, at: Ms) {
    sess(m).apply(
        &Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            ..TurnEnd::default()
        }),
        at,
    );
}

/// A turn of talk at at: a question and its answer.
fn talk(m: &mut Model, at: Ms) {
    sess(m).sent(format!("question {at}"), Vec::new(), at);
    let e = said(
        "assistant",
        format!("a{at}"),
        words(&format!("answer {at}")),
    );
    sess(m).apply(&e, at + 1000);
    end(m, at + 2000);
}

/// A turn that edits the parser at at and, with check, runs it.
fn edit(m: &mut Model, at: Ms, check: Option<&str>) {
    sess(m).sent("fix the secret parser".into(), Vec::new(), at);
    let mut calls = vec![ToolCall {
        id: format!("e{at}"),
        name: "Edit".into(),
        kind: ToolKind::Edit,
        input: Input {
            path: "/src/checkout/src/secret_parser.rs".into(),
            ..Input::default()
        },
        ..ToolCall::default()
    }];
    if let Some(c) = check {
        calls.push(ToolCall {
            id: format!("b{at}"),
            name: "Bash".into(),
            kind: ToolKind::Shell,
            input: Input {
                command: c.into(),
                ..Input::default()
            },
            ..ToolCall::default()
        });
    }
    let mut t = at;
    for c in calls {
        let id = c.id.clone();
        let shell = c.kind == ToolKind::Shell;
        let call = Part {
            kind: PartKind::ToolCall,
            call: Some(Box::new(c)),
            ..Part::default()
        };
        sess(m).apply(&said("assistant", format!("c{id}"), call), t + 500);
        let out = Part {
            kind: PartKind::ToolResult,
            output: Some(Box::new(Output {
                call_id: id.clone(),
                text: "ok".into(),
                exit: shell.then_some(0),
                ..Output::default()
            })),
            ..Part::default()
        };
        sess(m).apply(&said("user", format!("r{id}"), out), t + 1500);
        t += 2000;
    }
    let e = said(
        "assistant",
        format!("a{at}"),
        words("Fixed the secret parser."),
    );
    sess(m).apply(&e, t + 500);
    end(m, t + 1000);
}

/// The clock at at, and a frame drawn then.
fn frame_at(m: &mut Model, at: Ms) -> String {
    m.clock = Some(at);
    m.now = at;
    screen(m).join("\n")
}

/// Looking elsewhere: frames go by without this conversation.
fn look_away(m: &mut Model) {
    m.frames += 5;
}

fn shown(m: &mut Model) -> Option<viewsuggest::Shown> {
    viewsuggest::shown(m, KEY)
}

/// Off is the default, and then nothing is followed, asked or kept, and
/// the key does nothing.
#[test]
fn off_by_default_does_no_work() {
    assert_eq!(rush_state::Config::default().view_suggest, "");
    let quick = Quick::new(Ok(r#"{"view":"trail","reason":"long"}"#));
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    crate::accounts::use_store(&mut m, rush_state::Store::default());
    crate::accounts::accounts(&mut m).read_only = true;
    viewsuggest::use_asker(&mut m, quick.clone());
    assert_eq!(viewsuggest::mode(&m), Mode::Off);
    edit(&mut m, NOW - 60_000, None);
    frame_at(&mut m, NOW);
    look_away(&mut m);
    edit(&mut m, NOW + 60_000, Some("cargo test"));
    let f = frame_at(&mut m, NOW + 20 * 60_000);
    assert_eq!(viewsuggest::followed(&mut m), 0);
    assert!(quick.asked().is_empty());
    assert!(shown(&mut m).is_none());
    assert!(!f.contains("fits"), "{f}");
    press(&mut m, "alt+t");
    assert_eq!(presets::current(&m), Preset::Normal);
}

/// Opening a session of talk suggests Chat: the line under the box says
/// so and why, the view isn't changed, and the key takes it.
#[test]
fn suggests_as_you_open_and_the_key_takes_it() {
    let quick = Quick::new(Err("unused"));
    let mut m = model("suggest", &quick);
    talk(&mut m, NOW - 120_000);
    talk(&mut m, NOW - 60_000);
    frame_at(&mut m, NOW);
    let f = frame_at(&mut m, NOW + 100);
    let s = shown(&mut m).expect("a suggestion");
    assert_eq!(
        (s.view, s.reason.as_str()),
        (Preset::Chat, "mostly talk, no steps")
    );
    assert_eq!(s.from, None);
    assert_eq!(viewsuggest::suggested(&mut m, KEY), Some(Preset::Chat));
    assert_eq!(presets::current(&m), Preset::Normal, "never switched");
    let hint = viewsuggest::hint(&mut m, KEY, 120).unwrap().plain();
    assert!(
        hint.starts_with("tab: Chat fits · mostly talk, no steps"),
        "{hint}"
    );
    assert!(hint.contains("take it"), "{hint}");
    assert!(f.contains("tab: Chat fits"), "{f}");
    assert!(quick.asked().is_empty(), "the rules knew");
    press(&mut m, "alt+t");
    assert_eq!(presets::current(&m), Preset::Chat);
    assert!(shown(&mut m).is_none());
    assert_eq!(viewsuggest::count(&mut m, "back:chat").taken, 1);
}

/// Back after ten minutes away with turns you haven't seen: Catch-up; the
/// suggestion left showing before it counts as turned down.
#[test]
fn back_after_away_suggests_catch_up() {
    let quick = Quick::new(Err("unused"));
    let mut m = model("suggest", &quick);
    talk(&mut m, NOW - 60_000);
    frame_at(&mut m, NOW);
    assert_eq!(shown(&mut m).unwrap().view, Preset::Chat);
    look_away(&mut m);
    talk(&mut m, NOW + 60_000);
    talk(&mut m, NOW + 120_000);
    frame_at(&mut m, NOW + 11 * 60_000);
    let s = shown(&mut m).unwrap();
    assert_eq!(
        (s.view, s.reason.as_str()),
        (Preset::CatchUp, "2 turns finished while you were away")
    );
    assert_eq!(viewsuggest::count(&mut m, "back:chat").declined, 1);
    // A glance away and back with nothing new isn't coming back.
    presets::set(&mut m, Preset::CatchUp);
    look_away(&mut m);
    frame_at(&mut m, NOW + 12 * 60_000);
    assert!(shown(&mut m).is_none());
}

/// A turn that changed files and ran the tests ends while you watch:
/// Code, with what it changed and ran.
#[test]
fn a_turn_ending_suggests_code() {
    let quick = Quick::new(Err("unused"));
    let mut m = model("suggest", &quick);
    talk(&mut m, NOW - 60_000);
    frame_at(&mut m, NOW);
    edit(&mut m, NOW + 1000, Some("cargo test -p parser"));
    frame_at(&mut m, NOW + 10_000);
    let s = shown(&mut m).unwrap();
    assert_eq!(
        (s.view, s.reason.as_str()),
        (Preset::Code, "changed 1 file, ran tests")
    );
    assert_eq!(s.kind, "ended:code");
}

/// While you type, the moment waits; once you've stopped a few seconds it
/// comes.
#[test]
fn waits_while_you_type() {
    let quick = Quick::new(Err("unused"));
    let mut m = model("suggest", &quick);
    talk(&mut m, NOW - 60_000);
    m.clock = Some(NOW);
    m.now = NOW;
    press(&mut m, "h");
    frame_at(&mut m, NOW + 1000);
    assert!(shown(&mut m).is_none(), "typing");
    frame_at(&mut m, NOW + 5000);
    assert_eq!(shown(&mut m).unwrap().view, Preset::Chat);
}

/// When the rules can't tell, the quick model is asked, with counts only;
/// at most once in ten minutes for a session.
#[test]
fn the_quick_model_is_asked_when_unclear_and_throttled() {
    let quick = Quick::new(Ok(r#"{"view":"trail","reason":"where the time went"}"#));
    let mut m = model("suggest", &quick);
    edit(&mut m, NOW - 60_000, None);
    frame_at(&mut m, NOW);
    let asked = quick.asked();
    assert_eq!(asked.len(), 1);
    for w in ["secret", "parser", "Fixed", "/src", "question"] {
        assert!(!asked[0].contains(w), "{w} in {}", asked[0]);
    }
    let s = shown(&mut m).unwrap();
    assert_eq!(
        (s.view, s.reason.as_str()),
        (Preset::Trail, "where the time went")
    );
    // Another unclear turn a minute later: not asked again.
    edit(&mut m, NOW + 1000, None);
    frame_at(&mut m, NOW + 60_000);
    assert_eq!(quick.asked().len(), 1);
    assert!(shown(&mut m).is_none());
    // Ten minutes on, it may be.
    edit(&mut m, NOW + 61_000, None);
    frame_at(&mut m, NOW + 11 * 60_000);
    assert_eq!(quick.asked().len(), 2);
    assert_eq!(shown(&mut m).unwrap().view, Preset::Trail);
}

/// The quick model failing (timed out, unreadable) is no suggestion.
#[test]
fn a_failed_question_is_no_suggestion() {
    for answer in [Err("timed out"), Ok("not json")] {
        let quick = Quick::new(answer);
        let mut m = model("suggest", &quick);
        edit(&mut m, NOW - 60_000, None);
        frame_at(&mut m, NOW);
        assert_eq!(quick.asked().len(), 1);
        assert!(shown(&mut m).is_none());
    }
}

/// Switch when I come back: the view changes as you open the session and
/// says so, the key goes back; a turn ending while you're in it changes
/// nothing.
#[test]
fn switches_only_as_you_come_back() {
    let quick = Quick::new(Err("unused"));
    let mut m = model("switch", &quick);
    talk(&mut m, NOW - 60_000);
    frame_at(&mut m, NOW);
    assert_eq!(presets::current(&m), Preset::Chat, "switched");
    let s = shown(&mut m).unwrap();
    assert_eq!(s.from, Some(Preset::Normal));
    assert_eq!(viewsuggest::suggested(&mut m, KEY), None, "no ✦ once in it");
    let hint = viewsuggest::hint(&mut m, KEY, 120).unwrap().plain();
    assert!(
        hint.starts_with("switched to Chat · mostly talk, no steps"),
        "{hint}"
    );
    assert!(hint.contains("back to Normal"), "{hint}");
    press(&mut m, "alt+t");
    assert_eq!(presets::current(&m), Preset::Normal, "undone");
    assert_eq!(viewsuggest::count(&mut m, "back:chat").declined, 1);
    // A turn ends while you're in it: nothing switches.
    edit(&mut m, NOW + 1000, Some("cargo test"));
    frame_at(&mut m, NOW + 10_000);
    assert_eq!(presets::current(&m), Preset::Normal);
    assert!(shown(&mut m).is_none());
}

/// Turned down three times in a row, a suggestion isn't made again; one
/// taken starts the count over. Switching elsewhere turns it down.
#[test]
fn three_turned_down_in_a_row_stop_it() {
    let quick = Quick::new(Err("unused"));
    let mut m = model("suggest", &quick);
    talk(&mut m, NOW - 60_000);
    frame_at(&mut m, NOW);
    let mut at = NOW;
    let mut next = |m: &mut Model| {
        at += 10_000;
        talk(m, at);
        frame_at(m, at + 5000);
    };
    // Four turns end; each suggestion is let be until the next.
    for _ in 0..3 {
        next(&mut m);
        assert_eq!(shown(&mut m).unwrap().kind, "ended:chat");
    }
    next(&mut m);
    let c = viewsuggest::count(&mut m, "ended:chat");
    assert_eq!((c.declined, c.streak), (3, 3));
    assert!(shown(&mut m).is_none(), "stopped");
    next(&mut m);
    assert!(shown(&mut m).is_none(), "still stopped");

    // Another kind: turned down twice (once by switching elsewhere), then
    // taken.
    let quick = Quick::new(Err("unused"));
    let mut m = model("suggest", &quick);
    talk(&mut m, NOW - 60_000);
    frame_at(&mut m, NOW);
    presets::set(&mut m, Preset::Trail);
    assert_eq!(viewsuggest::count(&mut m, "back:chat").streak, 1);
    look_away(&mut m);
    talk(&mut m, NOW + 50_000);
    frame_at(&mut m, NOW + 61_000);
    let s = shown(&mut m).unwrap();
    assert_eq!(s.kind, "back:catch-up");
    presets::set(&mut m, Preset::Normal);
    look_away(&mut m);
    frame_at(&mut m, NOW + 20 * 60_000);
    assert_eq!(shown(&mut m).unwrap().kind, "back:chat");
    press(&mut m, "alt+t");
    let c = viewsuggest::count(&mut m, "back:chat");
    assert_eq!((c.taken, c.declined, c.streak), (1, 1, 0));
}
