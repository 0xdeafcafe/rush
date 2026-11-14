//! The Live view's watcher (storywatch.rs): nothing asked in any other
//! view; asked as the Live view opens, then only when a turn ends; shown
//! the dashboard as it stands each time; its answer drawn, its failure
//! said; and the Now line the small-job model's for the step running.

use std::sync::{Arc, Mutex};

use rush_convo::Preset;
use rush_core::adapter::Profile;
use rush_core::agent::State;
use rush_core::event::{Event, Message, Part, PartKind, TurnEnd};
use rush_core::time::Ms;
use rush_core::tool::{Call as ToolCall, Input, Kind as ToolKind};

use super::host::Fake;
use super::presets;
use super::storywatch;
use super::tests::{NOW, agent, open, screen, sess};
use super::viewsuggest::{Ask, Call};
use crate::model::Model;

/// The watcher, answering the same each time and keeping what it was
/// asked and with which model.
struct Watcher {
    answer: Result<String, String>,
    asked: Mutex<Vec<(String, String)>>,
}

impl Watcher {
    fn new(answer: Result<&str, &str>) -> Arc<Watcher> {
        Arc::new(Watcher {
            answer: answer.map(str::to_string).map_err(str::to_string),
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<(String, String)> {
        self.asked.lock().unwrap().clone()
    }
}

impl Ask for Watcher {
    fn models(&self, _: &Profile) -> (String, String) {
        ("quick".into(), "careful".into())
    }
    fn ask(&self, _: &Profile, c: &Call) -> (f64, Result<Vec<u8>, String>) {
        assert_eq!(c.system, rush_convo::live::SYSTEM);
        self.asked
            .lock()
            .unwrap()
            .push((c.model.clone(), c.prompt.clone()));
        (0.0, self.answer.clone().map(String::into_bytes))
    }
}

fn model(w: &Arc<Watcher>) -> Model {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Idle), &fake, 120, 40);
    crate::accounts::use_store(&mut m, rush_state::Store::default());
    crate::accounts::accounts(&mut m).read_only = true;
    storywatch::use_asker(&mut m, w.clone());
    storywatch::use_now(
        &mut m,
        Arc::new(|words: &str| Some(format!("now: {words}"))),
    );
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

/// A turn of talk at at: a question and its answer.
fn talk(m: &mut Model, at: Ms) {
    sess(m).sent(format!("question {at}"), Vec::new(), at);
    let e = said(
        "assistant",
        format!("a{at}"),
        words(&format!("answer {at}")),
    );
    sess(m).apply(&e, at + 1000);
    sess(m).apply(
        &Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            ..TurnEnd::default()
        }),
        at + 2000,
    );
}

/// A turn at at, running `command` and not done yet.
fn running(m: &mut Model, at: Ms, command: &str) {
    sess(m).sent("build it".into(), Vec::new(), at);
    let call = Part {
        kind: PartKind::ToolCall,
        call: Some(Box::new(ToolCall {
            id: format!("b{at}"),
            name: "Bash".into(),
            kind: ToolKind::Shell,
            input: Input {
                command: command.into(),
                ..Input::default()
            },
            ..ToolCall::default()
        })),
        ..Part::default()
    };
    sess(m).apply(&said("assistant", format!("c{at}"), call), at + 500);
}

fn frame_at(m: &mut Model, at: Ms) -> String {
    m.clock = Some(at);
    m.now = at;
    screen(m).join("\n")
}

const PATCH: &str = r#"{"headline":"Answering questions","progress":[1,2],"blocks":[{"id":"plan","kind":"steps","items":[{"text":"read","done":true},{"text":"answer","tone":"busy"}]}]}"#;

#[test]
fn claude_codes_sessions_ask_sonnet() {
    assert_eq!(storywatch::watch_model("claude", "haiku".into()), "sonnet");
    assert_eq!(storywatch::watch_model("codex", "luna".into()), "luna");
}

#[test]
fn nothing_is_asked_outside_the_live_view() {
    let w = Watcher::new(Ok(PATCH));
    let mut m = model(&w);
    talk(&mut m, NOW);
    frame_at(&mut m, NOW + 5_000);
    assert!(w.asked().is_empty());
    assert!(storywatch::story(&mut m, super::tests::KEY).is_none());
}

#[test]
fn asked_as_it_opens_then_as_turns_end_and_shown_the_dashboard() {
    let w = Watcher::new(Ok(PATCH));
    let mut m = model(&w);
    talk(&mut m, NOW);
    presets::show(&mut m, Preset::Live);
    let got = frame_at(&mut m, NOW + 5_000);
    let asked = w.asked();
    assert_eq!(asked.len(), 1, "asked once as it opens");
    assert_eq!(
        asked[0].0, "quick",
        "a harness not Claude Code's asks its quick model"
    );
    assert!(
        asked[0].1.starts_with("The dashboard now:\n(empty"),
        "{}",
        asked[0].1
    );
    assert!(
        asked[0].1.contains(&format!("question {NOW}")),
        "{}",
        asked[0].1
    );
    assert!(got.contains("Answering questions"), "{got}");
    assert!(got.contains("1/2  50%"), "{got}");
    assert!(got.contains("✓ read ─ ◉ answer"), "{got}");
    // Frames go by with nothing new: not asked again.
    frame_at(&mut m, NOW + 6_000);
    frame_at(&mut m, NOW + 60_000);
    assert_eq!(w.asked().len(), 1);
    // A turn ends: asked, shown what it said last time, told only what's
    // new.
    talk(&mut m, NOW + 70_000);
    frame_at(&mut m, NOW + 75_000);
    let asked = w.asked();
    assert_eq!(asked.len(), 2);
    let p = &asked[1].1;
    assert!(p.contains(r#""headline":"Answering questions""#), "{p}");
    assert!(p.contains(&format!("question {}", NOW + 70_000)), "{p}");
    assert!(!p.contains(&format!("question {NOW}\n")), "told again: {p}");
}

#[test]
fn a_failure_is_said_and_asked_again_later() {
    let w = Watcher::new(Err("rate limited"));
    let mut m = model(&w);
    talk(&mut m, NOW);
    presets::show(&mut m, Preset::Live);
    let got = frame_at(&mut m, NOW + 5_000);
    assert!(
        got.contains("the watcher couldn't say: rate limited"),
        "{got}"
    );
    frame_at(&mut m, NOW + 6_000);
    assert_eq!(w.asked().len(), 1, "not again at once");
    frame_at(&mut m, NOW + 5_000 + rush_convo::live::EVERY);
    assert_eq!(w.asked().len(), 2, "again after a while");
}

#[test]
fn the_now_line_is_the_small_job_models() {
    let w = Watcher::new(Ok(PATCH));
    let mut m = model(&w);
    talk(&mut m, NOW);
    running(&mut m, NOW + 10_000, "cargo build --release");
    presets::show(&mut m, Preset::Live);
    let got = frame_at(&mut m, NOW + 12_000);
    let now = got
        .lines()
        .find(|l| l.contains("Now"))
        .unwrap_or_else(|| panic!("{got}"));
    assert!(now.contains("now: ran `cargo build --release`"), "{now}");
}
