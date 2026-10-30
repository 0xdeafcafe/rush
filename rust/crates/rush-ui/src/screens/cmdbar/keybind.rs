//! The keymap in force, and every key pressed turned through it into the
//! key rush's handling is written against: a key you moved arrives as the
//! default it stands in for, a chord waits for its next key, and a command
//! bound to a key runs. Port of internal/ui/keybind.go:15-196 (keyState
//! :15, chordWait :28, keyActions :32, loadKeys :42, setKeys :54, keyMap
//! :59, keyContexts :68, onPages :87, remapKey :106, runAction :147,
//! plugin: :151). Plugins' commands are actions too, with the keys they
//! suggest (pluginActions, pluginKeys: plugin_ui/hooks.rs).
//! Actions that belong to screens not in this group (the grid, the quick
//! ask, the stash, a session's depth) do nothing until those screens
//! register them.

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

/// key_actions and what plugins offer (keyActions' pluginActions).
fn actions_of(m: &Model) -> Vec<Action> {
    let mut out = key_actions();
    out.extend(crate::screens::plugin_ui::hooks::plugin_key_actions(m));
    out
}

/// Puts f in force (setKeys), with the keys plugins suggest.
pub fn set_keys(m: &mut Model, f: File) {
    let map = Map::build(
        &actions_of(m),
        &f,
        &crate::screens::plugin_ui::hooks::plugin_keys(m),
    );
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
    if m.kept::<KeyState>().map.is_none() {
        let map = Map::build(
            &actions_of(m),
            &File::default(),
            &crate::screens::plugin_ui::hooks::plugin_keys(m),
        );
        m.kept::<KeyState>().map = Some(map);
    }
    m.kept::<KeyState>().map.as_ref().expect("built")
}

/// The keymap put in force again from the file it was built from, once
/// plugins' commands come or go (onHooks' setKeys); none is built until
/// it's needed.
pub fn rebuild(m: &mut Model) {
    let Some(f) = m
        .kept
        .peek::<KeyState>()
        .filter(|k| k.map.is_some())
        .map(|k| k.file.clone())
    else {
        return;
    };
    set_keys(m, f);
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
    if let Some(rest) = id.strip_prefix("plugin:") {
        let a = m.focused().map(|i| m.agents[i].clone());
        crate::screens::plugin_ui::hooks::run_plugin_command(m, rest, a.as_ref());
        return Out::Draw;
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
        // keybind.go:181: hide, stop, switch or delete the agent in view.
        "list.close" | "session.close" => {
            let a = m.selected_row();
            super::stops::ask_close(m, a)
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
