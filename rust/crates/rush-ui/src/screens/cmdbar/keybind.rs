//! The keymap in force, and every key pressed turned through it into the
//! key rush's handling is written against: a key you moved arrives as the
//! default it stands in for, a chord waits for its next key, and a command
//! bound to a key runs. Port of internal/ui/keybind.go:15-196 (keyState
//! :15, chordWait :28, keyActions :32, loadKeys :42, setKeys :54, keyMap
//! :59, keyContexts :68, onPages :87, remapKey :106, runAction :147).
//! Actions that belong to screens not in this group (the grid, the quick
//! ask, the stash, a session's depth) do nothing until those screens
//! register them.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use rush_keymap::{Action, Context, File, Map, Seq};

use super::fleetslash::FLEET_COMMANDS;
use super::{bar, keyhelp};
use crate::frame::{Out, Press};
use crate::model::Model;

/// How long a chord waits for its next key (chordWait).
pub const CHORD_WAIT: Duration = Duration::from_secs(2);

/// The keymap in force and a chord being typed (keyState).
#[derive(Default)]
pub struct KeyState {
    pub map: Option<Map>,
    pub file: File,
    /// The keys of a chord typed so far, and when the last came.
    pub chord: Seq,
    pub chord_at: Option<Instant>,
    /// Asked for keybindings.json already.
    loading: bool,
    /// A key handed on, remapped, through every binding again: it isn't
    /// turned a second time.
    passing: bool,
}

/// Every action a key can be bound to: rush's keys and its # commands
/// (keyActions). Plugins' come with the plugin screens.
pub fn key_actions() -> Vec<Action> {
    let mut out = rush_keymap::defaults();
    for (name, about, _) in FLEET_COMMANDS {
        let mut a = Action::new(rush_keymap::command_id(name), Context::Any);
        a.title = format!("#{name}: {about}");
        out.push(a);
    }
    out
}

/// Puts f in force (setKeys).
pub fn set_keys(m: &mut Model, f: File) {
    let map = Map::build(&key_actions(), &f, &BTreeMap::new());
    let k = m.kept::<KeyState>();
    k.file = f;
    k.map = Some(map);
}

/// The keymap in force; before keybindings.json is read, rush's own
/// (keyMap). With a loop running, the first call reads the file off the
/// UI thread (loadKeys).
pub fn key_map(m: &mut Model) -> &Map {
    if m.sender().is_some() && !m.kept::<KeyState>().loading {
        m.kept::<KeyState>().loading = true;
        m.later(rush_keymap::load, |m, r| {
            match r {
                Ok(f) => set_keys(m, f),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => m.flash_err(&format!("keybindings.json: {e}")),
            }
            Out::Draw
        });
    }
    let k = m.kept::<KeyState>();
    if k.map.is_none() {
        k.map = Some(Map::build(
            &key_actions(),
            &File::default(),
            &BTreeMap::new(),
        ));
    }
    k.map.as_ref().expect("built")
}

/// Where a key lands now, beyond Global: nothing more while a box over the
/// screen has the keys (keyContexts).
pub fn key_contexts(m: &Model) -> Vec<Context> {
    if on_pages(m) {
        return vec![Context::Pages];
    }
    if keyhelp::is_open(m) || bar::is_open(m) || m.confirm.is_some() || m.sheet.is_some() {
        return Vec::new();
    }
    if m.on_place() {
        return Vec::new(); // a place asking for text
    }
    if m.session.is_some() {
        return vec![Context::Session];
    }
    if !m.input.is_empty() {
        return vec![Context::Prompt, Context::List];
    }
    vec![Context::List]
}

/// Whether a place's pages have the keys: another place or the Wall, with
/// nothing over them and nothing being typed (onPages).
pub fn on_pages(m: &Model) -> bool {
    if bar::is_open(m) || m.confirm.is_some() || m.sheet.is_some() {
        return false;
    }
    m.on_place() && !m.place_asking()
}

/// What the keymap makes of k (remapKey): Pass hands it on as it is,
/// anything else is the key used up (a chord begun, a command run, a key
/// that does nothing now, or another key pressed in its place).
pub fn remap(m: &mut Model, k: &Press) -> Out {
    if m.kept::<KeyState>().passing {
        return Out::Pass;
    }
    let st = m.kept::<KeyState>();
    if !st.chord.is_empty() && st.chord_at.is_none_or(|t| t.elapsed() > CHORD_WAIT) {
        st.chord = Seq::default();
    }
    let chord = std::mem::take(&mut st.chord);
    let ctxs = key_contexts(m);
    let r = key_map(m).resolve(&ctxs, chord.keys(), &k.s);
    if !r.pending.is_empty() {
        let st = m.kept::<KeyState>();
        st.chord = r.pending.clone();
        st.chord_at = Some(Instant::now());
        m.flash(&format!("{} …", r.pending));
        return Out::Draw;
    }
    if !r.missed.is_empty() {
        m.flash_err(&format!("{} does nothing", r.missed));
        return Out::Draw;
    }
    if !r.run.is_empty() {
        return run_action(m, &r.run).or_idle();
    }
    if r.key.is_empty() {
        return Out::Idle;
    }
    if r.key == k.s {
        return Out::Pass;
    }
    let Some(nk) = Press::of(&r.key) else {
        return Out::Pass;
    };
    // The key it stands for, through every binding, not turned again.
    m.kept::<KeyState>().passing = true;
    let o = m.press_key(&nk);
    m.kept::<KeyState>().passing = false;
    o.or_idle()
}

/// Runs a command action: a # command on the agent in view, or one of
/// rush's own that runs rather than stands in for a key (runAction).
pub fn run_action(m: &mut Model, id: &str) -> Out {
    if let Some(name) = id.strip_prefix("command:") {
        return m.command(&format!("#{name}"));
    }
    match id {
        "guide.open" => {
            if keyhelp::is_open(m) {
                keyhelp::close(m);
            } else {
                keyhelp::open(m);
            }
            Out::Draw
        }
        "agent.new" => {
            bar::to_prompt(m);
            Out::Draw
        }
        // The session's own: quick ask, recall, discuss (keybind.go:188-211),
        // then its turns, depth, switch sheet, mode and box (:155-189).
        _ => match crate::screens::session_cards::run_action(m, id) {
            Out::Pass => crate::screens::session_core::actions::run_action(m, id),
            o => o,
        },
    }
}

/// The keys bound to an action, the first of them, for a hint: "" for
/// none (boundKey).
pub fn bound_key(m: &mut Model, id: &str) -> String {
    key_map(m)
        .keys(id)
        .first()
        .map(Seq::to_string)
        .unwrap_or_default()
}
