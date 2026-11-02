//! rush-ui-social's dictation.rs with yap in front and enter mid-dictation:
//! yap is told the window shows the dictation itself only while its tab is
//! in front with a box taking the words, and enter while a yap dictation
//! is still coming waits for its words, then sends them with the message.

use std::sync::mpsc;

use rush_draw::{App, Event, Msg};

use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::plugin_ui::ModelPlugins;
use rush_bundled::yap::Say;
use rush_ui_social::dictation::*;
use rush_ui_social::parts::YapState;

/// The messages the Agents box sent, in order.
#[derive(Default)]
struct Sent(Vec<String>);

fn record(m: &mut Model, t: &str) -> Out {
    m.kept::<Sent>().0.push(t.to_string());
    Out::Draw
}

fn sent(m: &mut Model) -> Vec<String> {
    m.kept::<Sent>().0.clone()
}

/// A window with the yap plugin on, its clock at 10s, its rush folder a
/// temp dir, what its Agents box sends recorded.
fn window() -> (Model, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir");
    rush_state::set_thread_dir(Some(dir.path().to_path_buf()));
    let mut m = crate::new_model("0.1.0");
    m.clock = Some(10_000);
    m.w = 80;
    m.h = 30;
    crate::screens::settings::use_store(&mut m, rush_state::Store::load_with_env(&|_| None));
    m.plugins().bundled_on.insert(YAP.to_string(), true);
    m.ext.agents_box = true;
    m.ext.submit.insert(0, record);
    m.kept::<Dictation>().frame = Some(0);
    (m, dir)
}

fn yap(phase: &str, settled: &str) -> YapState {
    YapState {
        phase: phase.into(),
        settled: settled.into(),
        ..YapState::default()
    }
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press_key(&Press::of(s).unwrap_or_else(|| panic!("no key {s}")))
}

/// The latest context line said to yap since the last look, if any.
fn latest(rx: &mpsc::Receiver<Say>) -> Option<String> {
    rx.try_iter()
        .filter_map(|s| match s {
            Say::Kept(l) => Some(l),
            Say::Once(_) => None,
        })
        .last()
}

fn shows(line: &str) -> bool {
    let v: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
    assert!(v["context"].is_object(), "not a context line: {line}");
    v["context"]["shows"].as_bool().unwrap_or(false)
}

// yap hears the window shows the dictation itself while its tab is in
// front with a box taking the words, and hears it doesn't the moment the
// tab loses focus or the box goes; nothing's said again while nothing
// changed.
#[test]
fn yap_hears_the_window_shows_it_only_while_in_front_with_a_box() {
    let (mut m, _d) = window();
    let (tx, rx) = mpsc::channel();
    tell_yap(&mut m, tx);
    assert!(shows(&latest(&rx).expect("said as it's followed")));
    after_frame(&mut m);
    assert_eq!(latest(&rx), None, "said again with nothing changed");
    // Another window in front: no frame follows, yap is told at once.
    let _ = m.update(Msg::Input(Event::FocusLost));
    assert!(!shows(&latest(&rx).expect("told on focus lost")));
    let _ = m.update(Msg::Input(Event::FocusGained));
    after_frame(&mut m);
    assert!(shows(&latest(&rx).expect("told on focus back")));
    // In front, but no box takes the words.
    m.ext.agents_box = false;
    after_frame(&mut m);
    assert!(!shows(&latest(&rx).expect("told the box went")));
}

// enter while yap is still hearing you waits, the box saying so; the
// words go into the box when yap's done, and the message is sent with
// them.
#[test]
fn enter_mid_dictation_waits_and_sends_with_the_words() {
    let (mut m, _d) = window();
    m.input.set("check the ");
    let _ = on_yap(&mut m, yap("listening", "migration"));
    assert_eq!(press(&mut m, "enter"), Out::Draw);
    assert_eq!(sent(&mut m), Vec::<String>::new(), "sent before the words");
    assert_eq!(m.input.string(), "check the ");
    assert_eq!(look(&mut m).expect("the look").hint, WAITING_HINT);
    let _ = on_yap(&mut m, yap("finishing", "migration replays"));
    assert_eq!(sent(&mut m), Vec::<String>::new(), "sent while finishing");
    let _ = on_yap(&mut m, yap("done", "migration replays"));
    assert_eq!(sent(&mut m), ["check the migration replays"]);
    assert!(look(&mut m).is_none(), "still waiting after it went");
    // yap's done said again, and its hidden, send nothing more.
    let _ = on_yap(&mut m, yap("done", "migration replays"));
    let _ = on_yap(&mut m, yap("hidden", ""));
    assert_eq!(sent(&mut m).len(), 1);
}

// enter again while it waits sends what's there now; the words that
// come after go into the box, unsent.
#[test]
fn enter_twice_sends_now() {
    let (mut m, _d) = window();
    m.input.set("check the ");
    let _ = on_yap(&mut m, yap("listening", "migration"));
    let _ = press(&mut m, "enter");
    let _ = press(&mut m, "enter");
    assert_eq!(sent(&mut m), ["check the"]);
    let _ = on_yap(&mut m, yap("done", "migration replays"));
    assert_eq!(sent(&mut m), ["check the"], "the words were sent too");
    assert_eq!(m.input.string(), "migration replays ");
}

// esc stops it waiting and keeps the text; the words then go in unsent.
#[test]
fn esc_stops_it_waiting() {
    let (mut m, _d) = window();
    m.input.set("check the ");
    let _ = on_yap(&mut m, yap("listening", "migration"));
    let _ = press(&mut m, "enter");
    assert_eq!(press(&mut m, "esc"), Out::Draw);
    assert_eq!(m.input.string(), "check the ", "esc lost the text");
    assert_ne!(look(&mut m).expect("the look").hint, WAITING_HINT);
    let _ = on_yap(&mut m, yap("done", "migration replays"));
    assert_eq!(sent(&mut m), Vec::<String>::new());
    assert_eq!(m.input.string(), "check the migration replays ");
}

// esc while yap listens is yap's: it says the dictation was stopped, and
// the message stays, unsent, its text kept.
#[test]
fn a_dictation_stopped_in_yap_leaves_it_unsent() {
    let (mut m, _d) = window();
    m.input.set("check the ");
    let _ = on_yap(&mut m, yap("listening", "migration"));
    let _ = press(&mut m, "enter");
    let stopped = |phase: &str| YapState {
        cancelled: true,
        ..yap(phase, "")
    };
    let _ = on_yap(&mut m, stopped("finishing"));
    let _ = on_yap(&mut m, stopped("hidden"));
    m.clock = Some(10_000 + 10 * WAIT_GRACE);
    let _ = settle(&mut m);
    assert_eq!(sent(&mut m), Vec::<String>::new());
    assert_eq!(m.input.string(), "check the ");
    assert!(look(&mut m).is_none(), "still waiting");
}

// A dictation that ends with nothing, fails, or a yap that goes: what's
// in the box is sent once nothing more has come for WAIT_GRACE.
#[test]
fn nothing_coming_sends_after_the_wait() {
    for end in [yap("done", ""), yap("hidden", ""), yap("failed", "")] {
        let (mut m, _d) = window();
        m.input.set("ship it");
        let _ = on_yap(&mut m, yap("listening", ""));
        let _ = press(&mut m, "enter");
        let phase = end.phase.clone();
        let _ = on_yap(&mut m, end);
        assert_eq!(sent(&mut m), Vec::<String>::new(), "{phase}: sent at once");
        assert_eq!(
            look(&mut m).expect("still waiting").hint,
            WAITING_HINT,
            "{phase}"
        );
        m.clock = Some(10_000 + WAIT_GRACE - 1);
        let _ = settle(&mut m);
        assert_eq!(sent(&mut m), Vec::<String>::new(), "{phase}: sent early");
        m.clock = Some(10_000 + WAIT_GRACE);
        let _ = settle(&mut m);
        assert_eq!(sent(&mut m), ["ship it"], "{phase}");
        assert!(look(&mut m).is_none(), "{phase}: still waiting");
    }
}

// With no dictation coming, enter sends at once, as ever.
#[test]
fn without_a_dictation_enter_sends_at_once() {
    let (mut m, _d) = window();
    m.input.set("ship it");
    let _ = press(&mut m, "enter");
    assert_eq!(sent(&mut m), ["ship it"]);
    // Nor after one: yap's said hidden.
    m.input.set("and tag it");
    let _ = on_yap(&mut m, yap("hidden", ""));
    let _ = press(&mut m, "enter");
    assert_eq!(sent(&mut m), ["ship it", "and tag it"]);
}
