//! The tests of rush-ui-social's dictation.rs, which need the whole UI.
//! A dictation in the box: Go's yap_test.go (the look, yap's paste) and
//! push-to-talk through the window, with a fake engine so nothing hears
//! the microphone. testdata/dictation_*.txt are the box and the setting as
//! drawn; UPDATE_GOLDEN=1 writes them again.

use std::sync::{Arc, Mutex};

use rush_core::text::{Line, Mods};
use rush_draw::{Key, KeyMods};

use crate::frame::{InputBox, Out, Press};
use crate::model::Model;
use crate::screens::plugin_ui::ModelPlugins;
use crate::style::*;
use rush_ui_social::dictation::*;
use rush_ui_social::parts::{LiveText, YAP_PASTE_WAIT, YapState};
use rush_ui_social::pushtalk::{Engine, Heard, Listening, Mic};

/// What the fake engine was asked, in order.
type Calls = Arc<Mutex<Vec<String>>>;

#[derive(Default)]
struct Fake {
    calls: Calls,
    /// The words the last start was told to expect.
    context: Arc<Mutex<Vec<String>>>,
    unavailable: bool,
    mics: Vec<Mic>,
}

impl Engine for Fake {
    fn available(&self) -> bool {
        !self.unavailable
    }
    fn devices(&self) -> Vec<Mic> {
        self.mics.clone()
    }
    fn start(
        &self,
        device: &str,
        context: Vec<String>,
        _out: rush_ui_social::pushtalk::Out,
    ) -> Result<Box<dyn Listening>, String> {
        self.calls.lock().unwrap().push(format!("start {device}"));
        *self.context.lock().unwrap() = context;
        Ok(Box::new(FakeListening(self.calls.clone())))
    }
}

struct FakeListening(Calls);

impl Listening for FakeListening {
    fn stop(&self) {
        self.0.lock().unwrap().push("stop".into());
    }
    fn cancel(&self) {
        self.0.lock().unwrap().push("cancel".into());
    }
}

fn airpods() -> Mic {
    Mic {
        id: "BuiltInHeadset:42".into(),
        name: "AirPods Pro".into(),
    }
}

/// A window with apple voice on, a fake engine, its clock at 0, its rush
/// folder a temp dir.
fn window(releases: bool, engine: Fake) -> (Model, Calls, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir");
    rush_state::set_thread_dir(Some(dir.path().to_path_buf()));
    let mut m = crate::new_model("0.1.0");
    m.clock = Some(0);
    m.w = 80;
    m.h = 30;
    crate::screens::settings::use_store(&mut m, rush_state::Store::load_with_env(&|_| None));
    m.plugins().bundled_on.insert(APPLE_VOICE.to_string(), true);
    let calls = engine.calls.clone();
    use_engine(&mut m, Arc::new(engine), releases);
    m.kept::<Dictation>().frame = Some(0);
    (m, calls, dir)
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press_key(&Press::of(s).unwrap_or_else(|| panic!("no key {s}")))
}

fn let_go(m: &mut Model, at: rush_core::time::Ms) {
    m.clock = Some(at);
    let _ = released(m, Key::Char(' '), KeyMods::CTRL);
}

fn calls(c: &Calls) -> Vec<String> {
    c.lock().unwrap().clone()
}

fn plain(ls: &[Line]) -> String {
    ls.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

/// got against testdata/name.txt; UPDATE_GOLDEN=1 writes it.
fn golden(name: &str, got: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/screens_tests/social/testdata")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("no {}: run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(got, want, "{name} drawn differently");
}

// --- the look (yap_test.go) ---

fn dictating(frame: usize) -> InputBox {
    InputBox {
        w: 72,
        focused: true,
        top_l: line([sp("to claude", TEXT)]),
        text: "check the".chars().collect(),
        cursor: 9,
        lead: line([sp("❯ ", ORANGE)]),
        max_rows: 6,
        live: Some(LiveText {
            settled: "migration replays every event".into(),
            guessing: "before the upgrade".into(),
            level: 0.7,
            frame,
            hint: "listening · let go to paste".into(),
        }),
        ..InputBox::default()
    }
}

// yap_test.go TestYapDictationShowsAtTheCursor
#[test]
fn dictation_shows_at_the_cursor() {
    let lines = dictating(0).lines();
    let got = plain(&lines);
    assert!(
        got.contains("check the migration replays every event before the upgrade"),
        "the dictation isn't after the typed text:\n{got}"
    );
    assert!(
        got.contains("let go to paste"),
        "no hint in the bottom edge:\n{got}"
    );
    let reversed = lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.style.mods.contains(Mods::REVERSE));
    assert!(
        !reversed,
        "the cursor is reverse video, not the dictation's"
    );
}

// yap_test.go TestYapEdgeLightMoves
#[test]
fn edge_light_moves() {
    let (a, b) = (dictating(0).lines(), dictating(12).lines());
    assert!(
        a[0] != b[0] || a[a.len() - 1] != b[b.len() - 1],
        "the edge looks the same twelve frames apart"
    );
    assert_eq!(
        a[0].plain(),
        b[0].plain(),
        "the light moved the edge's text"
    );
}

// yap_test.go TestYapGuessedWordsShimmer
#[test]
fn guessed_words_shimmer() {
    let seen: std::collections::HashSet<String> = (0..32)
        .map(|f| format!("{:?}", dictating(f).lines()[1]))
        .collect();
    assert!(
        seen.len() >= 8,
        "the text barely changes across a crossing: {} looks",
        seen.len()
    );
}

// yap_test.go TestYapBoxWithoutDictationIsUnchanged
#[test]
fn box_without_dictation_is_unchanged() {
    let mut b = dictating(0);
    b.live = None;
    b.idle = true;
    let plain_box = format!("{:?}", b.lines());
    b.live = Some(LiveText {
        settled: "hi".into(),
        ..LiveText::default()
    });
    assert_eq!(
        format!("{:?}", b.lines()),
        plain_box,
        "an idle box drew the dictation"
    );
}

// --- yap's paste (yap_test.go TestYapPasteIsTheDictation) ---

#[test]
fn yaps_paste_is_the_dictation() {
    let (mut m, _, _d) = window(true, Fake::default());
    m.clock = Some(10_000);
    let _ = on_yap(
        &mut m,
        YapState {
            phase: "done".into(),
            settled: "ship it".into(),
            ..YapState::default()
        },
    );
    let long = "ship it";
    assert_eq!(
        paste(&mut m, "ship it "),
        Out::Draw,
        "yap's own paste wasn't taken"
    );
    assert_eq!(m.input.string(), "ship it ");
    assert!(
        !m.kept::<Dictation>().yap.active(),
        "the paste didn't end the dictation's look"
    );
    assert_eq!(
        paste(&mut m, long),
        Out::Pass,
        "a second paste was taken as it too"
    );
    // Too late: an ordinary paste.
    let _ = on_yap(
        &mut m,
        YapState {
            phase: "done".into(),
            settled: "ship it".into(),
            ..YapState::default()
        },
    );
    m.clock = Some(10_000 + YAP_PASTE_WAIT + 1_000);
    assert_eq!(
        paste(&mut m, "ship it"),
        Out::Pass,
        "a paste long after was taken"
    );
}

// --- push-to-talk ---

#[test]
fn held_past_the_hold_stops_and_types_what_was_said() {
    let (mut m, c, _d) = window(true, Fake::default());
    m.input.set("deploy");
    assert_eq!(press(&mut m, "ctrl+space"), Out::Draw);
    assert_eq!(calls(&c), ["start "]);
    // The terminal saying the held key again is not a press.
    m.clock = Some(300);
    let _ = repeated(&mut m, Key::Char(' '), KeyMods::CTRL);
    assert_eq!(calls(&c), ["start "]);
    let_go(&mut m, 900);
    assert_eq!(calls(&c), ["start ", "stop"]);
    let _ = heard(
        &mut m,
        Heard {
            phase: "done".into(),
            settled: "the staging service ".into(),
            ..Heard::default()
        },
    );
    assert_eq!(
        m.input.string(),
        "deploy the staging service",
        "typed at the cursor, spaced"
    );
    assert!(look(&mut m).is_none(), "the box still shows a dictation");
}

#[test]
fn push_to_talk_expects_the_words_on_screen() {
    let fake = Fake::default();
    let context = fake.context.clone();
    let (mut m, c, _d) = window(true, fake);
    m.agents = vec![rush_core::agent::Agent {
        key: "a".into(),
        display_name: "LangWatch exporter".into(),
        branch: "fix/rushd-restart".into(),
        ..Default::default()
    }];
    m.sel = "a".into();
    let _ = press(&mut m, "ctrl+space");
    assert_eq!(calls(&c), ["start "]);
    assert_eq!(
        *context.lock().unwrap(),
        [
            "LangWatch exporter",
            "LangWatch",
            "fix/rushd-restart",
            "rushd-restart"
        ]
    );
}

#[test]
fn a_tap_listens_hands_free_until_pressed_again() {
    let (mut m, c, _d) = window(true, Fake::default());
    let _ = press(&mut m, "ctrl+space");
    let_go(&mut m, 120);
    assert_eq!(calls(&c), ["start "], "a tap stopped it");
    let l = look(&mut m).expect("listening");
    assert_eq!(l.hint, "listening · ctrl+space to stop");
    m.clock = Some(4_000);
    let _ = press(&mut m, "ctrl+space");
    assert_eq!(calls(&c), ["start ", "stop"]);
    assert_eq!(look(&mut m).expect("finishing").hint, "finishing");
}

#[test]
fn without_releases_each_press_toggles() {
    let (mut m, c, _d) = window(false, Fake::default());
    let _ = press(&mut m, "ctrl+space");
    assert_eq!(
        look(&mut m).expect("listening").hint,
        "listening · ctrl+space to stop"
    );
    let _ = press(&mut m, "ctrl+space");
    assert_eq!(calls(&c), ["start ", "stop"]);
}

#[test]
fn esc_drops_it_and_its_words_never_arrive() {
    let (mut m, c, _d) = window(true, Fake::default());
    let _ = press(&mut m, "ctrl+space");
    assert_eq!(press(&mut m, "esc"), Out::Draw);
    assert_eq!(calls(&c), ["start ", "cancel"]);
    let _ = heard(
        &mut m,
        Heard {
            phase: "done".into(),
            settled: "never".into(),
            ..Heard::default()
        },
    );
    assert_eq!(m.input.string(), "");
}

#[test]
fn off_unavailable_and_failed() {
    // The plugin off: ctrl+space is left alone.
    let (mut m, c, _d) = window(true, Fake::default());
    m.plugins()
        .bundled_on
        .insert(APPLE_VOICE.to_string(), false);
    let _ = press(&mut m, "ctrl+space");
    assert!(calls(&c).is_empty(), "started with its plugin off");
    // Before macOS 26: it says so.
    let (mut m, c, _d) = window(
        true,
        Fake {
            unavailable: true,
            ..Fake::default()
        },
    );
    let _ = press(&mut m, "ctrl+space");
    assert!(calls(&c).is_empty());
    assert_eq!(
        m.flash.as_ref().map(|f| f.0.as_str()),
        Some("push-to-talk needs macOS 26")
    );
    // A failure says why, as rush says what broke.
    let (mut m, _, _d) = window(true, Fake::default());
    let _ = press(&mut m, "ctrl+space");
    let why = "rush voice can't use the microphone";
    let _ = heard(
        &mut m,
        Heard {
            phase: "failed".into(),
            error: why.into(),
            ..Heard::default()
        },
    );
    assert_eq!(m.flash.as_ref().map(|f| f.0.as_str()), Some(why));
    assert!(look(&mut m).is_none());
}

#[test]
fn the_prompt_shows_it_listening() {
    let (mut m, _, _d) = window(true, Fake::default());
    let _ = press(&mut m, "ctrl+space");
    let _ = heard(
        &mut m,
        Heard {
            phase: "listening".into(),
            settled: "check the migration".into(),
            guessing: "replays".into(),
            level: 60,
            ..Heard::default()
        },
    );
    let screen = plain(&m.frame());
    assert!(screen.contains("check the migration replays"), "{screen}");
    assert!(screen.contains("listening · let go to stop"), "{screen}");
}

/// The box as the Agents prompt draws it while push-to-talk is where
/// steps leave it.
fn box_while(releases: bool, mic: bool, steps: impl FnOnce(&mut Model)) -> String {
    let (mut m, _, _d) = window(
        releases,
        Fake {
            mics: vec![airpods()],
            ..Fake::default()
        },
    );
    if mic {
        crate::screens::settings::config_mut(&mut m).microphone = airpods().id;
    }
    m.input.set("deploy");
    let _ = press(&mut m, "ctrl+space");
    let _ = heard(
        &mut m,
        Heard {
            phase: "listening".into(),
            settled: "the staging service".into(),
            guessing: "after the migration".into(),
            level: 50,
            ..Heard::default()
        },
    );
    steps(&mut m);
    let b = InputBox {
        w: 64,
        focused: true,
        top_l: line([sp("new agent in ", DIM), sp("rush", TEXT)]),
        text: m.input.text.clone(),
        cursor: m.input.pos,
        lead: line([sp("❯ ", ORANGE)]),
        max_rows: 6,
        live: look(&mut m),
        ..InputBox::default()
    };
    plain(&b.lines())
}

#[test]
fn the_box_while_listening_held_hands_free_and_finishing() {
    let held = box_while(true, true, |_| {});
    let free = box_while(true, false, |m| let_go(m, 100));
    let finishing = box_while(true, true, |m| {
        let_go(m, 900);
        let _ = heard(
            m,
            Heard {
                phase: "finishing".into(),
                settled: "the staging service after the migration".into(),
                ..Heard::default()
            },
        );
    });
    assert!(
        held.contains("listening · let go to stop · AirPods Pro"),
        "{held}"
    );
    assert!(free.contains("listening · ctrl+space to stop"), "{free}");
    assert!(finishing.contains("finishing · AirPods Pro"), "{finishing}");
    golden("dictation_box", &[held, free, finishing].join("\n=====\n"));
}

#[test]
fn the_microphone_setting() {
    use crate::screens::settings::form::setting_row;
    use crate::screens::settings::general::microphone;
    let (mut m, _, _d) = window(
        true,
        Fake {
            mics: vec![airpods()],
            ..Fake::default()
        },
    );
    let mut rows = Vec::new();
    let st = microphone(&m);
    assert_eq!(st.choices, ["", "BuiltInHeadset:42"]);
    assert_eq!(st.shown(), "Mac's input");
    rows.push(setting_row(true, &st, 32, 30, 80).plain());
    rows.push(st.now());
    // Chosen: kept by its id, shown by its name.
    let set = st.set.clone().expect("a setter");
    set(&mut m, "BuiltInHeadset:42");
    assert_eq!(
        crate::screens::settings::config(&m).microphone,
        "BuiltInHeadset:42"
    );
    let st = microphone(&m);
    assert_eq!(st.shown(), "AirPods Pro");
    rows.push(setting_row(false, &st, 32, 30, 80).plain());
    rows.push(st.now());
    // Unplugged since: still chosen, and it says so.
    m.kept::<Dictation>().mics.clear();
    let st = microphone(&m);
    assert_eq!(st.shown(), "a microphone not plugged in");
    rows.push(setting_row(false, &st, 32, 30, 80).plain());
    rows.push(st.now());
    golden("dictation_microphone", &rows.join("\n"));
    // Only with apple voice on is it in Settings › General.
    let has = |m: &Model| {
        crate::screens::settings::general::sections(m)
            .iter()
            .any(|s| s.title == "Push to talk")
    };
    assert!(has(&m));
    m.plugins()
        .bundled_on
        .insert(APPLE_VOICE.to_string(), false);
    assert!(!has(&m));
}
