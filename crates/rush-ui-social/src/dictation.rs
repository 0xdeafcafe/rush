//! A dictation shows in the box that has the keys, as it's heard: the
//! words it's sure of in the text's colour, the ones it's still guessing
//! dimmer with a light passing over them, a cursor that brightens as you
//! speak, and a light running round the box's edge.
//!
//! Two engines feed it. yap, followed on its socket while the yap plugin
//! is on: when you let go and yap says it's done, the words go straight
//! into the box with the keys and yap is told rush took them, so a yap
//! that waits for that doesn't paste (a window that doesn't take them
//! says it left them, so yap pastes at once). The paste of an older yap,
//! the same words into the same box a moment later, is dropped before
//! it's drawn, and so is yap saying done again. Where no box takes them,
//! yap's paste goes in as text however long it is: it's what you said,
//! not a log to fold away. And rush's own push-to-talk (pushtalk.rs), while the apple voice plugin
//! is on: ctrl+space starts it, and its words go in at the cursor as
//! typed when it's done. Both on, fn is yap's and ctrl+space is rush's;
//! the box shows whichever is under way.
//!
//! Both are told the words on the window's screen (screenwords.rs) so
//! names come out spelled as written: yap on its socket whenever they
//! change while it's followed (read after a frame, at most once a second),
//! push-to-talk as it starts. That has no Go original.
//!
//! The yap side is ported from Go's internal/ui/yap.go (yapLive :23,
//! yapSync :50, onYap :94, onYapTick :108, yapShown :124, yapPaste :131,
//! yapLook :142, yapDictated :217) and its callers (model.go:1179-1182,
//! :1388, pluginui.go:48 and :70, view.go:2719, rushmode.go:3722); the
//! look itself is rush-ui-base's frame/inputbox.rs. Its tests need the
//! whole UI: rush-ui's screens_tests/social/dictation.rs. Push-to-talk has no Go
//! original.

use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use rush_draw::{Key, KeyMods, Step};

use super::parts::{LiveText, YapState, is_dictation, yap_shown};
use super::pushtalk::{Do, Engine, Heard, Listening, Mic, Talk};
use super::screenwords;
use crate::frame::{Editor, Out, Press};
use crate::model::Model;
use crate::worker::Note;
use rush_bundled::yap::Say;

/// The yap plugin's name: its switch.
pub const YAP: &str = "yap";
/// The apple voice plugin's name: push-to-talk's switch.
pub const APPLE_VOICE: &str = rush_bundled::apple_voice::NAME;
/// push-to-talk's action, and the key it has unless you moved it.
pub const TALK_ACTION: &str = "voice.talk";
pub const TALK_KEY: &str = "ctrl+space";

/// The look's animation, a step each (yapEvery).
const EVERY: Duration = Duration::from_millis(50);

/// The window's view of its dictations (yapLive, and push-to-talk's).
#[derive(Default)]
pub struct Dictation {
    /// yap's latest, and how to stop following it; None while its plugin
    /// is off.
    pub yap: YapState,
    yap_stop: Option<mpsc::Sender<()>>,
    /// Lines for yap while it's followed: the words on screen, and that a
    /// dictation was taken.
    yap_say: Option<mpsc::Sender<Say>>,
    /// The words on screen as last read, when, what they were read from,
    /// and whether a wake is asked to read them again.
    words: Vec<String>,
    words_at: Option<Instant>,
    words_sig: Option<u64>,
    words_due: bool,
    /// A finished yap dictation's text until its paste arrives, and when
    /// yap said it.
    yap_final: String,
    yap_final_at: rush_core::time::Ms,
    /// yap's done is dealt with (its words went into a box, by rush or by
    /// a paste): another done is the same dictation said again, not shown.
    /// Until yap says another phase.
    yap_handled: bool,
    /// The dictation rush put in a box itself, when, and which box, until
    /// yap's paste of it arrives.
    yap_took: Option<(String, rush_core::time::Ms, String)>,
    /// A paste that came while yap was finishing, before its done, and
    /// when: the dictation, if done says the same words.
    yap_early: Option<(String, rush_core::time::Ms)>,
    /// What hears push-to-talk; None leaves ctrl+space alone.
    engine: Option<Arc<dyn Engine>>,
    talk: Talk,
    /// The engine's latest.
    heard: Heard,
    listening: Option<Box<dyn Listening>>,
    /// The dictation under way; changes of an older one are dropped.
    serial: u64,
    /// The terminal says when a key is let go.
    releases: bool,
    /// The key now being handled is a held key said again.
    repeat: bool,
    /// The microphone setting, read once when Settings hasn't it.
    mic: Option<String>,
    /// The microphones the engine said it has, as last asked.
    pub mics: Vec<Mic>,
    /// When the look's animation counts from.
    epoch: Option<Instant>,
    /// The look's frame, pinned (tests).
    pub frame: Option<usize>,
}

fn state(m: &mut Model) -> &mut Dictation {
    m.kept::<Dictation>()
}

/// Whether the apple voice plugin is on, as last read.
pub fn apple_voice_on(m: &Model) -> bool {
    switched_on(m, APPLE_VOICE)
}

/// The microphones the engine said it has, as last asked.
pub fn mics(m: &Model) -> Vec<Mic> {
    m.kept
        .peek::<Dictation>()
        .map(|d| d.mics.clone())
        .unwrap_or_default()
}

/// Whether a bundled plugin is on, as last read: off until it's known.
fn switched_on(m: &Model, name: &str) -> bool {
    (m.ext.calls.bundled_on())(m, name)
}

/// Whether push-to-talk works now: its plugin on and an engine to hear.
pub fn talk_on(m: &Model) -> bool {
    switched_on(m, APPLE_VOICE)
        && m.kept
            .peek::<Dictation>()
            .is_some_and(|d| d.engine.is_some())
}

/// Gives push-to-talk the engine that hears it, and asks it for its
/// microphones; releases is whether the terminal says when a key is let
/// go. Done once as the window opens; tests give a fake.
pub fn use_engine(m: &mut Model, e: Arc<dyn Engine>, releases: bool) {
    let d = state(m);
    d.engine = Some(e);
    d.releases = releases;
    read_mics(m);
}

/// Asks the engine for its microphones again, off the UI thread.
pub fn read_mics(m: &mut Model) {
    let Some(e) = state(m).engine.clone() else {
        return;
    };
    m.later(
        move || e.devices(),
        |m, mics| {
            state(m).mics = mics;
            Out::Draw
        },
    );
}

/// Follows yap while its plugin is on and stops when it's off; drops
/// push-to-talk when its plugin goes off (yapSync).
pub fn sync(m: &mut Model) {
    if !switched_on(m, APPLE_VOICE) && state(m).talk.on() {
        cancel(m);
    }
    if !switched_on(m, YAP) {
        let d = state(m);
        d.yap_stop = None; // dropped: follow stops
        d.yap_say = None;
        d.yap = YapState::default();
        d.yap_final.clear();
        d.yap_took = None;
        d.yap_early = None;
        return;
    }
    if state(m).yap_stop.is_some() {
        return;
    }
    let Some(tx) = m.sender() else {
        return;
    };
    let (stop_tx, stop) = mpsc::channel();
    let (say_tx, say) = mpsc::channel();
    let d = state(m);
    d.yap_stop = Some(stop_tx);
    d.yap_say = Some(say_tx);
    // yap hears the words on screen as it's found.
    read_words(m, Instant::now());
    say_words(m);
    let tx = tx.lane(64);
    let _ = std::thread::Builder::new()
        .name("rush-yap".into())
        .spawn(move || {
            rush_bundled::yap::follow(stop, say, move |s| {
                let s = YapState {
                    phase: s.phase,
                    settled: s.settled,
                    guessing: s.guessing,
                    level: s.level,
                    locked: s.locked,
                    app: s.app,
                };
                // The window's behind: it'll have the next.
                let _ = tx.try_send(Note::Apply(Box::new(move |m: &mut Model| on_yap(m, s))));
            });
        });
}

/// After each frame: while yap is followed, the words on screen are read
/// again once the last reading is a second old, and yap told when they
/// changed. A frame sooner asks for a wake at that second, so the last
/// change before the screen goes still isn't missed.
pub fn after_frame(m: &mut Model) {
    let Some(d) = m.kept.peek::<Dictation>() else {
        return;
    };
    if d.yap_say.is_none() {
        return;
    }
    let now = Instant::now();
    if let Some(t) = d.words_at
        && now.duration_since(t) < screenwords::EVERY
    {
        if !d.words_due {
            state(m).words_due = true;
            m.ask_wake(t + screenwords::EVERY);
        }
        return;
    }
    state(m).words_due = false;
    if read_words(m, now) {
        say_words(m);
    }
}

/// Reads the words on screen again unless what they're read from is the
/// same; whether they changed.
fn read_words(m: &mut Model, now: Instant) -> bool {
    state(m).words_at = Some(now);
    let sig = screenwords::fingerprint(m);
    if state(m).words_sig == Some(sig) {
        return false;
    }
    state(m).words_sig = Some(sig);
    let words = screenwords::collect(m);
    let d = state(m);
    if words == d.words {
        return false;
    }
    d.words = words;
    true
}

/// Tells yap the words on screen, while it's followed.
fn say_words(m: &mut Model) {
    let d = state(m);
    if let Some(tx) = &d.yap_say {
        let _ = tx.send(Say::Kept(rush_bundled::yap::context_line(
            this_app(),
            &d.words,
        )));
    }
}

/// The words on screen for a dictation starting now: as last read, read
/// again when that's a second old.
fn words_now(m: &mut Model) -> Vec<String> {
    let now = Instant::now();
    let stale = state(m)
        .words_at
        .is_none_or(|t| now.duration_since(t) >= screenwords::EVERY);
    if stale && read_words(m, now) {
        say_words(m);
    }
    state(m).words.clone()
}

/// Takes what yap says (onYap). A done going to this window, with a box
/// that has the keys, goes into that box at once, as yap's paste would
/// have put it, and yap is told it was taken; the look ends in the same
/// step, so no frame shows the words twice.
pub fn on_yap(m: &mut Model, s: YapState) -> Out {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    if s.phase != "done" {
        let d = state(m);
        d.yap_handled = false;
        if s.phase != "finishing" {
            d.yap_early = None;
        } else if d.yap_early.is_some() {
            // Its paste is in already: the look stays away.
            return Out::Idle;
        }
        d.yap = s;
        return Out::Draw;
    }
    let hidden = YapState {
        phase: "hidden".into(),
        ..YapState::default()
    };
    let d = state(m);
    // Said again after it was dealt with: nothing new.
    if d.yap_handled {
        d.yap = hidden;
        return Out::Draw;
    }
    // Its paste came first and went in as an ordinary paste.
    if let Some((text, at)) = d.yap_early.take()
        && is_dictation(&s.settled, at, now, &text)
    {
        d.yap_handled = true;
        d.yap = hidden;
        return Out::Draw;
    }
    let shown = yap_shown(&s, m.term_focused, this_app());
    let into = target_id(m);
    if shown
        && !s.settled.trim().is_empty()
        && let Some(into) = into
        && let Some(ed) = target(m)
    {
        // Just as yap pastes it: the words and a space.
        ed.insert_str(&crate::frame::editor::clean_paste(&format!(
            "{} ",
            s.settled
        )));
        let d = state(m);
        if let Some(tx) = &d.yap_say {
            let _ = tx.send(Say::Once(rush_bundled::yap::took_line(&s.settled)));
        }
        d.yap_took = Some((s.settled, now, into));
        d.yap_final.clear();
        d.yap_handled = true;
        d.yap = hidden;
        return Out::Draw;
    }
    let d = state(m);
    if let Some(tx) = &d.yap_say {
        let _ = tx.send(Say::Once(rush_bundled::yap::left_line(&s.settled)));
    }
    d.yap_final = s.settled.clone();
    d.yap_final_at = now;
    d.yap = s;
    Out::Draw
}

/// The app rush's terminal is, as macOS tells programs started in it, so
/// a dictation going to another app isn't shown here (yapApp).
fn this_app() -> &'static str {
    static APP: OnceLock<String> = OnceLock::new();
    APP.get_or_init(|| std::env::var("__CFBundleIdentifier").unwrap_or_default())
}

/// The key push-to-talk is on: its first single key, as keybindings.json
/// may have moved it.
fn talk_key(m: &mut Model) -> String {
    rush_ui_base::keybind::key_map(m)
        .keys(TALK_ACTION)
        .iter()
        .find(|s| s.len() == 1)
        .map_or_else(|| TALK_KEY.to_string(), |s| s.to_string())
}

/// The dictation as the box with the keys draws it, if one is shown: push
/// -to-talk's under way, else yap's while rush's terminal has the keys
/// and yap is pasting into it (yapShown, yapLook). Asks for the look's
/// next frame.
pub fn look(m: &mut Model) -> Option<LiveText> {
    let focused = m.term_focused;
    let own = state(m).talk.on();
    let shown = own || yap_shown(&state(m).yap, focused, this_app());
    if !shown {
        return None;
    }
    let key = if own { talk_key(m) } else { String::new() };
    let mic = if own { mic_name(m) } else { String::new() };
    let d = state(m);
    let frame = d.frame.unwrap_or_else(|| {
        let epoch = *d.epoch.get_or_insert_with(Instant::now);
        (epoch.elapsed().as_millis() / EVERY.as_millis()) as usize
    });
    let pinned = d.frame.is_some();
    let live = if own {
        let h = &d.heard;
        let mut hint = if h.phase == "finishing" {
            "finishing".to_string()
        } else {
            d.talk.hint(&key)
        };
        if !mic.is_empty() {
            hint = format!("{hint} · {mic}");
        }
        LiveText {
            settled: h.settled.clone(),
            guessing: h.guessing.clone(),
            level: h.level as f64 / 100.0,
            frame,
            hint,
        }
    } else {
        d.yap.look(frame)
    };
    if !pinned {
        m.ask_wake(Instant::now() + EVERY);
    }
    Some(live)
}

/// The chosen microphone's name, for the hint; empty for the Mac's input
/// or one the engine hasn't named.
fn mic_name(m: &mut Model) -> String {
    let id = mic(m);
    if id.is_empty() {
        return String::new();
    }
    state(m)
        .mics
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.name.clone())
        .unwrap_or_default()
}

/// The microphone setting: Settings' copy once it has read the config,
/// else config.json read once.
fn mic(m: &mut Model) -> String {
    if let Some(c) = crate::accounts::loaded_config(m) {
        return c.microphone.clone();
    }
    if let Some(v) = &state(m).mic {
        return v.clone();
    }
    let v = rush_state::Store::load().config.microphone;
    state(m).mic = Some(v.clone());
    v
}

/// Settings chose a microphone.
pub fn set_mic(m: &mut Model, id: &str) {
    state(m).mic = Some(id.to_string());
}

/// A key, before anything else has it: push-to-talk's key, and esc while
/// it listens. Everything else, and its key with its plugin off, goes on.
pub fn key(m: &mut Model, k: &Press) -> Out {
    if !talk_on(m) {
        return Out::Pass;
    }
    if k.s == "esc" && state(m).talk.on() {
        cancel(m);
        return Out::Draw;
    }
    let talk = rush_ui_base::keybind::key_map(m)
        .keys(TALK_ACTION)
        .iter()
        .any(|s| s.len() == 1 && s.keys()[0] == k.s);
    if !talk {
        return Out::Pass;
    }
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let d = state(m);
    let (repeat, releases) = (d.repeat, d.releases);
    if d.talk == Talk::Idle && !repeat {
        let available = d.engine.as_ref().is_some_and(|e| e.available());
        if !available {
            m.flash_err("push-to-talk needs macOS 26");
            return Out::Draw;
        }
    }
    match state(m).talk.press(now, repeat, releases) {
        Do::Start => start(m),
        Do::Stop => {
            if let Some(l) = &state(m).listening {
                l.stop();
            }
        }
        Do::Cancel | Do::Nothing => {}
    }
    Out::Draw
}

/// A held key said again: a press as ever, push-to-talk told it's held.
pub fn repeated(m: &mut Model, code: Key, mods: KeyMods) -> Step {
    state(m).repeat = true;
    let s = m.key(code, mods);
    state(m).repeat = false;
    s
}

/// A key let go: only push-to-talk listens. Its key is let go however
/// its modifier went first, so the key alone is matched.
pub fn released(m: &mut Model, code: Key, _mods: KeyMods) -> Step {
    // A terminal that says so after all.
    state(m).releases = true;
    if !talk_on(m) || !matches!(state(m).talk, Talk::Held { .. }) {
        return Step::Idle;
    }
    let key = talk_key(m);
    if Press::of(&key).is_none_or(|p| p.code != code) {
        return Step::Idle;
    }
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    match state(m).talk.release(now) {
        Do::Stop => {
            if let Some(l) = &state(m).listening {
                l.stop();
            }
            Step::Draw
        }
        // Let go quickly: it listens on, hands-free, and the hint says so.
        _ => Step::Draw,
    }
}

/// Starts the engine on the chosen microphone; each change it hears
/// comes back to the UI thread as a note.
fn start(m: &mut Model) {
    let device = mic(m);
    let context = words_now(m);
    let tx = m.sender().map(|t| t.lane(64));
    let d = state(m);
    d.serial += 1;
    let serial = d.serial;
    d.heard = Heard {
        phase: "listening".into(),
        ..Heard::default()
    };
    let Some(e) = d.engine.clone() else {
        d.talk.ended();
        return;
    };
    let out = Box::new(move |h: Heard| {
        if let Some(tx) = &tx {
            tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                heard_in(m, serial, h)
            })));
        }
    });
    match e.start(&device, context, out) {
        Ok(l) => state(m).listening = Some(l),
        Err(e) => {
            end(m);
            m.fail(&e);
        }
    }
}

/// esc, or its plugin turned off: drop what it heard.
fn cancel(m: &mut Model) {
    let d = state(m);
    let _ = d.talk.esc();
    if let Some(l) = d.listening.take() {
        l.cancel();
    }
    d.heard = Heard::default();
    d.serial += 1;
}

/// Forgets the dictation that ended.
fn end(m: &mut Model) {
    let d = state(m);
    d.talk.ended();
    d.listening = None;
    d.heard = Heard::default();
}

/// What the engine heard, for the dictation under way.
pub fn heard(m: &mut Model, h: Heard) -> Out {
    let serial = state(m).serial;
    heard_in(m, serial, h)
}

fn heard_in(m: &mut Model, serial: u64, h: Heard) -> Out {
    if serial != state(m).serial || !state(m).talk.on() {
        return Out::Idle; // one dropped or done
    }
    match h.phase.as_str() {
        "done" => {
            end(m);
            let said = h.settled.trim();
            if !said.is_empty() {
                type_in(m, said);
            }
        }
        "failed" => {
            end(m);
            let why = if h.error.is_empty() {
                "push-to-talk stopped"
            } else {
                h.error.as_str()
            };
            m.fail(why);
        }
        "hidden" => end(m),
        _ => {
            let d = state(m);
            if h.phase == "finishing" && matches!(d.talk, Talk::Held { .. } | Talk::Free) {
                d.talk = Talk::Finishing;
            }
            d.heard = h;
        }
    }
    Out::Draw
}

/// The box that has the keys: the open Session's, or Agents' with nothing
/// over it (yapDictated's choice).
fn target(m: &mut Model) -> Option<&mut Editor> {
    use crate::sess as sc;
    let clear = m.confirm.is_none() && m.sheet.is_none() && !m.on_place();
    if !clear {
        return None;
    }
    if let Some(key) = sc::open_key(m) {
        return Some(&mut sc::conn(m, &key).input);
    }
    if m.ext.agents_box {
        return Some(&mut m.input);
    }
    None
}

/// Which box target() is: the open Session's key, or "" for Agents'.
fn target_id(m: &mut Model) -> Option<String> {
    let key = crate::sess::open_key(m);
    target(m)?;
    Some(key.unwrap_or_default())
}

/// Push-to-talk's words, in the box at its cursor as typed, a space
/// before them as the look showed; on the clipboard when no box takes
/// them, so what you said isn't lost.
fn type_in(m: &mut Model, said: &str) {
    if let Some(ed) = target(m) {
        let pos = ed.pos.min(ed.text.len());
        let spaced = pos > 0 && !ed.text[pos - 1].is_whitespace();
        ed.insert_str(&if spaced {
            format!(" {said}")
        } else {
            said.to_string()
        });
        return;
    }
    m.copy_text(said.to_string());
    m.flash("what you said is on the clipboard: no box had the keys");
}

/// yap's paste of a dictation rush already put in the same box, a moment
/// ago, is dropped before it's drawn. Else a paste that's the dictation
/// yap just finished goes in as typed, never folded into a chip, the
/// prompt's newlines kept; any paste ends the dictation's look (yapPaste,
/// yapDictated).
pub fn paste(m: &mut Model, text: &str) -> Out {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    if let Some((took, at, into)) = state(m).yap_took.clone()
        && is_dictation(&took, at, now, text)
        && target_id(m).as_deref() == Some(into.as_str())
    {
        state(m).yap_took = None;
        return Out::Draw;
    }
    let d = state(m);
    let (final_, at) = (std::mem::take(&mut d.yap_final), d.yap_final_at);
    if d.yap.phase == "finishing" {
        d.yap_early = Some((text.to_string(), now));
    }
    if d.yap.phase == "done" || d.yap.phase == "finishing" {
        d.yap = YapState {
            phase: "hidden".into(),
            ..YapState::default()
        };
    }
    if !is_dictation(&final_, at, now, text) {
        return Out::Pass;
    }
    state(m).yap_handled = true;
    let text = crate::frame::editor::clean_paste(text);
    match target(m) {
        Some(ed) => {
            ed.insert_str(&text);
            Out::Draw
        }
        None => Out::Pass,
    }
}
