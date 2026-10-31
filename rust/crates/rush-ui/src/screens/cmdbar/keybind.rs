//! The keymap in force, and every key pressed turned through it into the
//! key rush's handling is written against: a key you moved arrives as the
//! default it stands in for, a chord waits for its next key, and a command
//! bound to a key runs. Port of internal/ui/keybind.go:15-196 (keyState
//! :15, chordWait :28, keyActions :32, loadKeys :42, setKeys :54, keyMap
//! :59, keyContexts :68, onPages :87, remapKey :106, runAction :147,
//! plugin: :151, promptToAgent :333), and the next or previous open chat,
//! ctrl+tab and ctrl+] ] [ (keys.go:614 stepChat, model.go:2175 inPlay).
//! Plugins' commands are actions too, with the keys they suggest
//! (pluginActions, pluginKeys: plugin_ui/hooks.rs). Actions that belong
//! to screens not in this group (the quick ask, the stash) do nothing
//! until those screens register them.

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
    // The Prompt's keys while it's the Prompt with text, not a name being
    // renamed (m.inKind == inPrompt).
    if !m.input.is_empty() && crate::screens::list_extras::rename::renaming(m).is_none() {
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

/// A chord is waiting for its next key (len(m.keys.chord) > 0).
pub fn chord_waiting(m: &mut Model) -> bool {
    let st = m.kept::<KeyState>();
    !st.chord.is_empty() && st.chord_at.is_some_and(|t| t.elapsed() <= CHORD_WAIT)
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
    // A chord that ended draws again even when what it ran says nothing,
    // or its "ctrl+] …" would stay on screen.
    let ended = !chord.is_empty() && r.pending.is_empty() && end_chord_flash(m, &chord);
    let redraw = |o: Out| {
        if ended && o == Out::Idle {
            Out::Draw
        } else {
            o
        }
    };
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
        return redraw(run_action(m, &r.run).or_idle());
    }
    if r.key.is_empty() {
        return redraw(Out::Idle);
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
    redraw(o.or_idle())
}

/// A chord that ended takes its "ctrl+] …" off the line under the box,
/// so it doesn't say it's still waiting while whatever ran says nothing;
/// true when it did.
fn end_chord_flash(m: &mut Model, chord: &Seq) -> bool {
    let waiting = rush_convo::render::key_word(&format!("{chord} …"));
    let shown = m.flash.as_ref().is_some_and(|f| f.0 == waiting);
    if shown {
        m.flash = None;
    }
    shown
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
        "chat.next" | "chat.prev" => step_chat(m, if id == "chat.next" { 1 } else { -1 }),
        "grid.beside" | "grid.below" => {
            crate::screens::list_extras::grid::pin(m, id == "grid.below");
            Out::Draw
        }
        "grid.unpin" => {
            crate::screens::list_extras::grid::unpin(m, false);
            Out::Draw
        }
        "prompt.append" | "prompt.replace" => prompt_to_agent(m, id == "prompt.replace"),
        // ctrl+] f: the list's filter, as alt+f opens it (keys.go:513,
        // listfilter.go:13); Go's runAction has no case for it yet.
        "list.filter" => {
            if m.input.is_empty() && !m.on_place() {
                crate::screens::list_extras::filter::start(m);
            }
            Out::Draw
        }
        // keybind.go:181: hide, stop, switch or delete the agent in view.
        "list.close" | "session.close" => {
            let a = m.selected_row();
            super::stops::ask_close(m, a)
        }
        // The Prompt's stash (keybind.go:208-211).
        "prompt.stash" => crate::screens::list_extras::stash::command(m, "stash"),
        "prompt.history" => crate::screens::list_extras::stash::command(m, "history"),
        // The session's own: quick ask, recall, discuss (keybind.go:188-211),
        // then its turns, depth, switch sheet, mode and box (:155-189).
        _ => match crate::screens::session_cards::run_action(m, id) {
            Out::Pass => crate::screens::session_core::actions::run_action(m, id),
            o => o,
        },
    }
}

/// Whether a section's agents are chats in play: what needs you, is at
/// work, or waits on you (inPlay).
fn in_play(title: &str) -> bool {
    use crate::list::{FINISHED, IDLE, NEEDS, STUCK, TO_LAND, WORKING};
    [NEEDS, FINISHED, STUCK, WORKING, TO_LAND, IDLE].contains(&title)
}

/// ctrl+tab: the next open chat (Active's, in the list's order), shown
/// where you are: in its Session if you were in one (stepChat).
pub fn step_chat(m: &mut Model, d: isize) -> Out {
    let mut chats: Vec<usize> = Vec::new();
    let mut at: Option<usize> = None;
    for &i in &m.order {
        let a = &m.agents[i];
        if a.key == m.sel {
            at = Some(chats.len());
        } else if !m.group_of.get(&a.key).is_some_and(|g| in_play(g)) {
            continue;
        }
        chats.push(i);
    }
    if chats.len() < 2 && at.is_some() || chats.is_empty() {
        m.flash("no other chat open");
        return Out::Draw;
    }
    let n = chats.len() as isize;
    // From outside Active, the first; going back, the last.
    let at = at.map_or(if d > 0 { -1 } else { 0 }, |a| a as isize);
    let i = chats[((at + d) % n + n) as usize % chats.len()];
    let key = m.agents[i].key.clone();
    m.select(key);
    m.rebuild();
    if m.session.is_some() {
        m.open(i);
    }
    Out::Draw
}

/// What's typed in Agents' Prompt put in the open agent's box, after its
/// draft or in place of it, and the keys go there (promptToAgent): only
/// into a session rush runs, as Go's m.host is.
pub fn prompt_to_agent(m: &mut Model, replace: bool) -> Out {
    // Go's m.host stays the agent last opened once the keys go back to
    // the Prompt (ctrl+] u): here, the picked agent, if it was opened.
    let sel = m.sel.clone();
    let opened = crate::screens::session_core::core(m)
        .conns
        .contains_key(&sel);
    let picked = (opened && !sel.starts_with('§')).then_some(sel);
    let Some(key) = m
        .session
        .as_ref()
        .or(m.pane.as_ref())
        .map(|v| v.key.clone())
        .or(picked)
        // Go's m.host is only ever a session rush runs: one outside rush
        // has no box rush sends from (keybind.go:333).
        .filter(|k| m.agents.iter().any(|a| a.key == *k && a.rush))
    else {
        m.flash_err("no agent open to send it to");
        return Out::Draw;
    };
    if m.input.is_empty() {
        m.flash("nothing typed to move");
        return Out::Draw;
    }
    let text = std::mem::take(&mut m.input.text);
    let pastes = std::mem::take(&mut m.input.pastes);
    m.input.clear();
    let c = &mut crate::screens::session_core::conn(m, &key).input;
    if replace || c.is_empty() {
        c.text = text;
        c.pastes = pastes;
    } else {
        // Its pasted chips come along, numbered again in the box's own.
        let s: String = text.iter().collect();
        let moved = c.pastes.unfold(&pastes.expand(&s, Some(0)));
        c.text.push('\n');
        c.text.extend(moved);
    }
    c.pos = c.text.len();
    c.anchor = None;
    if let Some(i) = m.agents.iter().position(|a| a.key == key) {
        m.open(i);
    }
    Out::Draw
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
