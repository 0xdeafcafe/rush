//! What passes between the screen and its plugins: what they ask the
//! window to do, what it tells them changed, their commands, and what
//! they put on rows, boxes and overviews. After Go's
//! internal/ui/pluginui.go (readBundled :45, setBundled :54,
//! setPluginDev :75, onHooks :95, pluginDo :113, pluginActions :175,
//! pluginKeys :192, pluginHashCommands :208, runPluginCommand :235,
//! uiSession :253, haltKind :262, emitHooks :285, emitChange :324,
//! emitFocusAndNet :340, recent :370, hookState :375, boxNow :387,
//! emitInput :398, boxState :414, setBox :427, boxNote :472, emitBox
//! :484, pluginOverview :560, pluginStatus :576), the keymap's share of
//! plugins (keybind.go:37 keyActions, :55 setKeys with pluginKeys, :151
//! runAction's plugin:, onHooks :97 rebuilding it) and the stash as a
//! flash says it (stash.go:101 stash, :133 wentTo, :141 hasStash, :173
//! wipeBox, :187 wipePrompt, :199 keepSent). Asking plugins before a
//! message goes is intercept.rs.
//!
//! Every call into the client here only queues (emit) or reads the state
//! it holds; what waits on the broker runs through Model::later.

use std::collections::{BTreeMap, HashMap};

use rush_core::agent::Agent;
use rush_core::failure;
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_fleet::Predicates;
use rush_plugin::{
    EV_INPUT_CHANGED, EV_NETWORK_DOWN, EV_NETWORK_UP, EV_SESSION_LEFT, EV_SESSION_OPENED,
    EV_SESSION_SEEN, EV_SESSION_STOPPED, EV_TURN_ENDED, EV_TURN_STARTED, TextBox, UiDo, UiError,
    UiEvent, UiSession, clean_notice,
};
use rush_plugind::hooks::Msg;

use super::{STASH_PLUGIN, toned};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// What the last look at the fleet found, to tell what changed
/// (hookSeen).
#[derive(Clone, Debug, Default)]
pub struct HookSeen {
    /// Each agent's state, and whether halted; None before the first
    /// look.
    pub states: Option<HashMap<String, String>>,
    /// The Session in view.
    pub focus: String,
    pub offline: bool,
    /// The box being typed in, as it was, and whose.
    pub input: String,
    pub input_at: String,
    /// The connection it was told on.
    pub attach: u64,
}

/// How many agents plugins hear of at once, well inside the queue events
/// wait in (maxSeen).
pub const MAX_SEEN: usize = 150;

/// Reads which bundled plugins are on, off the UI (readBundled).
pub fn read_bundled(m: &mut Model) {
    let disk = m.plugins().disk.clone();
    m.later(
        move || disk.bundles_on(),
        |m, on| {
            m.plugins().bundled_on = on;
            Out::Draw
        },
    );
}

/// Turns a bundled plugin on or off, off the UI, and has the broker
/// start or stop it (setBundled).
pub fn set_bundled(m: &mut Model, name: &str, on: bool) {
    let disk = m.plugins().disk.clone();
    let name = name.to_string();
    m.later(
        {
            let name = name.clone();
            move || match disk.set_bundled(&name, on) {
                Err(e) => (None, Some(e)),
                Ok(()) => (Some(disk.bundles_on()), disk.reload().err()),
            }
        },
        move |m, (st, err)| {
            if let Some(st) = st {
                m.plugins().bundled_on = st;
            }
            if let Some(e) = err {
                m.fail(&format!("{name}: {e}"));
            }
            Out::Draw
        },
    );
}

/// Turns development mode on or off for an installed plugin
/// (setPluginDev).
pub fn set_plugin_dev(m: &mut Model, name: &str, on: bool) {
    let disk = m.plugins().disk.clone();
    let name = name.to_string();
    m.later(
        {
            let name = name.clone();
            move || disk.set_dev(&name, on).and_then(|_| disk.reload())
        },
        move |m, r| {
            match r {
                Err(e) => m.fail(&format!("{name}: {e}")),
                Ok(()) if on => m.flash(&format!(
                    "{name} is in development mode · rebuilt as its files change"
                )),
                Ok(()) => {}
            }
            Out::Draw
        },
    );
}

/// Takes what the broker sent (onHooks).
pub fn on_hooks(m: &mut Model, msg: Msg) -> Out {
    match msg {
        Msg::State => {
            // Commands came or went: the keymap has them.
            rebuild_keys(m);
            let n = m.hooks().map(|h| h.attaches()).unwrap_or(0);
            if n != m.plugins().seen.attach {
                // A broker new to this window knows nothing of it: say
                // again what's here and which Session is open.
                m.plugins().seen = HookSeen {
                    attach: n,
                    ..HookSeen::default()
                };
                emit_hooks(m);
            }
            Out::Draw
        }
        Msg::Do(d) => plugin_do(m, *d),
    }
}

/// Does what a plugin asked, as far as rush lets it (pluginDo).
pub fn plugin_do(m: &mut Model, mut d: UiDo) -> Out {
    match d.kind.as_str() {
        "notify" => {
            let said = clean_notice(&d.text);
            let bad = d.tone == "bad";
            if d.plugin == STASH_PLUGIN {
                // rush's own, in its words: no name before it.
                m.flash_tone(&stash_said(&said), bad); // ui.go flashTone
                return Out::Draw;
            }
            let mut who = d.plugin.clone();
            if !d.session.is_empty()
                && let Some(a) = agent_by_key(m, &d.session)
            {
                who += &format!(" · {}", a.display_name);
            }
            m.flash_tone(&format!("{who}: {said}"), bad);
            Out::Draw
        }
        "input.set" => {
            set_box(m, &d);
            Out::Draw
        }
        "pick" => {
            if d.plugin == STASH_PLUGIN
                && let Some(p) = d.pick.as_mut()
            {
                p.about = stash_said(&p.about);
                for e in &mut p.empty {
                    *e = stash_said(e);
                }
            }
            if let Some(p) = d.pick.take() {
                super::pick::open(m, &d.plugin, p);
            }
            Out::Draw
        }
        // A message to a session goes through its own box, which the
        // session screens keep: until they take one from here, nothing is
        // sent on a plugin's word.
        _ => Out::Idle,
    }
}

/// The stash's keys filled in as rush has them (stash.go:126 stashSaid).
pub fn stash_said(s: &str) -> String {
    s.replace("{aside}", "ctrl+p")
        .replace("{history}", "#stash")
}

/// The agent with key k.
pub fn agent_by_key<'a>(m: &'a Model, k: &str) -> Option<&'a Agent> {
    if k.is_empty() {
        return None;
    }
    m.agents.iter().find(|a| a.key == k)
}

/// Plugins' commands, as (id, title, plugin) for the keymap: id is
/// plugin:<name>.<command> (pluginActions).
pub fn plugin_actions(m: &Model) -> Vec<(String, String, String)> {
    let Some(h) = m.hooks() else {
        return Vec::new();
    };
    let st = h.state();
    let mut out = Vec::new();
    for p in &st.plugins {
        if p.name == STASH_PLUGIN {
            continue; // rush's own stash and history keys run these
        }
        for c in &p.commands {
            out.push((
                format!("plugin:{}.{}", p.name, c.name),
                c.description.clone(),
                p.name.clone(),
            ));
        }
    }
    out
}

/// The keys plugins suggest for their commands, by action id
/// (pluginKeys).
pub fn plugin_keys(m: &Model) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let Some(h) = m.hooks() else { return out };
    for p in &h.state().plugins {
        for c in &p.commands {
            if !c.key.is_empty() {
                out.insert(format!("plugin:{}.{}", p.name, c.name), vec![c.key.clone()]);
            }
        }
    }
    out
}

/// Plugins' commands as # commands, (name, description): #haven.open
/// (pluginHashCommands).
pub fn plugin_hash_commands(m: &Model) -> Vec<(String, String)> {
    let Some(h) = m.hooks() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in &h.state().plugins {
        if p.name == STASH_PLUGIN {
            continue; // #stash is rush's own
        }
        for c in &p.commands {
            out.push((
                format!("{}.{}", p.name, c.name),
                format!("{} ({})", c.description, p.name),
            ));
        }
    }
    out
}

/// Whether name is a plugin's command, as plugin.command
/// (isPluginCommand).
pub fn is_plugin_command(m: &Model, name: &str) -> bool {
    plugin_hash_commands(m).iter().any(|(n, _)| n == name)
}

/// A #plugin.command typed in the Agents box: run on the agent picked
/// (keys.go's command, for plugins' names).
pub fn hash_command(m: &mut Model, text: &str) -> Out {
    let first = text.split_whitespace().next().unwrap_or("");
    let name = first.trim_start_matches(['#', '/']).to_lowercase();
    if !is_plugin_command(m, &name) {
        return Out::Pass;
    }
    m.input.clear();
    let a = m.focused().map(|i| m.agents[i].clone());
    run_plugin_command(m, &name, a.as_ref());
    Out::Draw
}

/// Runs name.command on a, off the UI (runPluginCommand).
pub fn run_plugin_command(m: &mut Model, id: &str, a: Option<&Agent>) {
    let Some((name, cmd)) = id.split_once('.') else {
        return;
    };
    let Some(h) = m.hooks() else { return };
    m.flash(&format!("{name} {cmd}…"));
    let (input, who) = match box_state(m) {
        Some((b, who)) => (Some(b), who),
        None => (None, String::new()),
    };
    let s = a.map(ui_session);
    let (name, cmd) = (name.to_string(), cmd.to_string());
    m.later(
        {
            let (name, cmd) = (name.clone(), cmd.clone());
            move || h.command(&name, &cmd, s.as_ref(), &who, input.as_ref())
        },
        move |m, r| {
            if let Err(e) = r {
                m.fail(&format!("{name} {cmd}: {e}"));
                return Out::Draw;
            }
            Out::Idle
        },
    );
}

/// What a plugin is told of an agent: what its row shows (uiSession).
pub fn ui_session(a: &Agent) -> UiSession {
    UiSession {
        id: a.key.clone(),
        session_id: a.job.session_id.clone(),
        name: a.display_name.clone(),
        agent: a.kind.to_string(),
        cwd: a.job.cwd.clone(),
        repo: a.repo.clone(),
        branch: a.branch.clone(),
        state: a.job.state.as_str().to_string(),
        hosted: false,
    }
}

/// Why an agent stopped, sorted for a plugin (haltKind).
pub fn halt_kind(a: &Agent, now: Ms) -> Option<UiError> {
    let h = a.spend.halt.as_ref()?;
    let k = failure::kind_of(&h.text);
    let kind = if h.kind == "rate_limit" {
        "limit"
    } else if h.kind == "authentication_failed" {
        "auth"
    } else if a.offline() {
        "offline"
    } else if matches!(k, failure::Kind::Transient | failure::Kind::Capacity) {
        "retryable"
    } else {
        "other"
    };
    // rush continues the rest for a day.
    let retrying = matches!(kind, "offline" | "retryable") && a.continues(now);
    Some(UiError {
        kind: kind.into(),
        message: clean_notice(&h.text),
        retrying,
    })
}

/// An agent worth telling plugins of: open, or at work today (recent).
pub fn recent(a: &Agent, now: Ms) -> bool {
    !a.past && (a.job.live() || a.busy() || now - a.job.updated_at < 24 * 3_600_000)
}

/// An agent's state as events tell it (hookState).
pub fn hook_state(a: &Agent) -> String {
    if a.halted() {
        return "halted".into();
    }
    if a.busy() {
        return "working".into();
    }
    a.job.state.as_str().into()
}

/// Tells plugins what changed since the last look: all from what's in
/// memory, and emit never waits (emitHooks).
pub fn emit_hooks(m: &mut Model) {
    let Some(h) = m.hooks() else { return };
    if !h.wants(EV_TURN_ENDED) {
        return;
    }
    let now = m.now;
    let offline = m.plugins().disk.offline();
    let agents: Vec<(String, String, bool, UiSession, Option<UiError>)> = m
        .agents
        .iter()
        .map(|a| {
            (
                a.key.clone(),
                hook_state(a),
                recent(a, now),
                ui_session(a),
                halt_kind(a, now),
            )
        })
        .collect();
    let seen = &mut m.plugins().seen;
    let Some(states) = seen.states.as_mut() else {
        // The first look is a baseline: nothing changed yet, but plugins
        // hear of each recent agent.
        let mut states = HashMap::new();
        let mut told = 0;
        for (key, st, rec, s, _) in agents {
            states.insert(key, st);
            if told < MAX_SEEN && rec {
                h.emit(event(EV_SESSION_SEEN, Some(s)));
                told += 1;
            }
        }
        seen.states = Some(states);
        seen.offline = offline;
        if offline {
            // A plugin just come can't know the network is already gone.
            h.emit(event(EV_NETWORK_DOWN, None));
        }
        return;
    };
    for (key, now, rec, s, err) in agents {
        let was = states.insert(key, now.clone()).unwrap_or_default();
        if was.is_empty() && rec {
            h.emit(event(EV_SESSION_SEEN, Some(s.clone())));
        }
        if now != was && !was.is_empty() {
            let kind = change_kind(&was, &now);
            if let Some(k) = kind {
                let mut e = event(k, Some(s));
                if now == "halted" {
                    e.error = err;
                }
                h.emit(e);
            }
        }
    }
    emit_focus_and_net(m, offline);
}

/// What an agent's state going from was to now tells plugins
/// (emitChange).
pub fn change_kind(was: &str, now: &str) -> Option<&'static str> {
    if now == "halted" || now == "stopped" {
        Some(EV_SESSION_STOPPED)
    } else if now == "working" && was != "blocked" {
        Some(EV_TURN_STARTED)
    } else if was == "working" || was == "blocked" {
        Some(EV_TURN_ENDED)
    } else {
        None
    }
}

fn event(kind: &str, s: Option<UiSession>) -> UiEvent {
    UiEvent {
        kind: kind.into(),
        session: s,
        ..UiEvent::default()
    }
}

/// Tells plugins the Session in view changed, or the network went or
/// came back (emitFocusAndNet).
fn emit_focus_and_net(m: &mut Model, offline: bool) {
    let Some(h) = m.hooks() else { return };
    let focus = m
        .session
        .as_ref()
        .map(|v| v.key.clone())
        .unwrap_or_default();
    let was = m.plugins().seen.focus.clone();
    if focus != was {
        if let Some(a) = agent_by_key(m, &was) {
            h.emit(event(EV_SESSION_LEFT, Some(ui_session(a))));
        }
        if let Some(a) = agent_by_key(m, &focus) {
            h.emit(event(EV_SESSION_OPENED, Some(ui_session(a))));
        }
        m.plugins().seen.focus = focus;
    }
    let seen = &mut m.plugins().seen;
    if offline != seen.offline {
        seen.offline = offline;
        h.emit(event(
            if offline {
                EV_NETWORK_DOWN
            } else {
                EV_NETWORK_UP
            },
            None,
        ));
    }
}

/// The box keys go to, whole, and whose it is: the Agents box's as ""
/// while it's on screen (boxNow, boxState). A Session's box is its own
/// screen's.
pub fn box_state(m: &Model) -> Option<(TextBox, String)> {
    if !m.ext.agents_box || m.session.is_some() || m.sheet.is_some() {
        return None;
    }
    Some((
        TextBox {
            text: m.input.string(),
            cursor: m.input.pos as i64,
            ..TextBox::default()
        },
        String::new(),
    ))
}

/// Tells plugins with "input" what's in the box, when it changed
/// (emitInput).
pub fn emit_input(m: &mut Model) {
    let Some(h) = m.hooks() else { return };
    if !h.wants(EV_INPUT_CHANGED) {
        return;
    }
    let Some((b, who)) = box_state(m) else { return };
    let seen = &mut m.plugins().seen;
    if b.text == seen.input && who == seen.input_at {
        return;
    }
    seen.input = b.text.clone();
    seen.input_at = who.clone();
    let s = agent_by_key(m, &who).map(ui_session);
    let mut e = event(EV_INPUT_CHANGED, s);
    e.text = b.text.clone();
    e.input_box = Some(b);
    h.emit(e);
}

/// Tells plugins with "input" that a box was sent or cleared (emitBox).
pub fn emit_box(m: &mut Model, kind: &str, key: &str, text: &str) {
    let Some(h) = m.hooks() else { return };
    if text.is_empty() {
        return;
    }
    let seen = &mut m.plugins().seen;
    seen.input.clear();
    seen.input_at = key.to_string();
    let mut e = event(kind, agent_by_key(m, key).map(ui_session));
    e.text = text.to_string();
    h.emit(e);
}

/// A plugin setting the Agents box, when it has the keys. With If, only
/// a box still holding that text is set, so nothing typed meanwhile is
/// lost (setBox).
pub fn set_box(m: &mut Model, d: &UiDo) {
    if !d.session.is_empty() || !m.ext.agents_box {
        return;
    }
    if d.if_text.as_ref().is_some_and(|t| *t != m.input.string()) {
        return;
    }
    let b = d.input_box.clone().unwrap_or_else(|| TextBox {
        text: d.text.clone(),
        ..TextBox::default()
    });
    let text: Vec<char> = b.text.chars().collect();
    let pos = if d.input_box.is_none() {
        text.len() // text alone: the cursor at its end, as before
    } else {
        (b.cursor.max(0) as usize).min(text.len())
    };
    let e = &mut m.input;
    e.undo.save(&e.text, e.pos, false);
    e.text = text;
    e.pos = pos;
    e.anchor = None;
}

/// Plugins' commands as the keymap's actions: plugin:<name>.<command>,
/// everywhere, by the plugin (pluginActions).
pub fn plugin_key_actions(m: &Model) -> Vec<rush_keymap::Action> {
    plugin_actions(m)
        .into_iter()
        .map(|(id, title, plugin)| {
            let mut a = rush_keymap::Action::new(id, rush_keymap::Context::Any);
            a.title = title;
            a.source = plugin;
            a
        })
        .collect()
}

/// Puts the keymaps built already in force again, from the file each was
/// built from, with plugins' commands as they are now (onHooks' setKeys).
pub fn rebuild_keys(m: &mut Model) {
    crate::screens::cmdbar::keybind::rebuild(m);
    crate::screens::settings::keys::rebuild(m);
}

/// The stash's words, and whether it's running (stash.go:101 stash).
pub fn stash(m: &Model) -> (rush_bundled::drafts::Words, bool) {
    if let Some(h) = m.hooks()
        && let Some(p) = h.state().plugins.iter().find(|p| p.name == STASH_PLUGIN)
    {
        let called = p.values.get("called").map_or("", String::as_str);
        return (rush_bundled::drafts::words_for(called), true);
    }
    (rush_bundled::drafts::words_for(""), false)
}

/// What a flash says of text taken out of a box: what happened, and the
/// history's tab that has it while the stash runs to keep it (wentTo).
pub fn went_to(m: &Model, did: &str, tab: &str) -> String {
    if !stash(m).1 {
        return did.to_string();
    }
    format!(
        "{did}{}",
        stash_said(&format!(" · {{history}} › {tab} has it"))
    )
}

/// Whether a box has a stash waiting, as its edge says (hasStash).
pub fn has_stash(m: &Model, key: &str) -> bool {
    m.hooks().is_some_and(|h| {
        h.state()
            .notes
            .get(key)
            .is_some_and(|ns| ns.iter().any(|n| n.plugin == STASH_PLUGIN))
    })
}

/// A message gone from session key's box: the stash told, for its
/// history, and the flash saying so, with the box's stash coming back
/// into it if it has one (keepSent).
pub fn keep_sent(m: &mut Model, key: &str, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let did = if has_stash(m, key) {
        format!("sent · your {} is back in the box", stash(m).0.noun)
    } else {
        "sent".to_string()
    };
    emit_box(m, rush_plugin::EV_INPUT_SENT, key, text);
    let said = went_to(m, &did, "Sent");
    m.flash(&said);
}

/// A box wiped: what was in it told to the stash for its history, and
/// the flash saying how to bring it back (wipeBox's and wipePrompt's
/// emitBox and flash). key is the session's, "" the Agents box's.
pub fn cleared(m: &mut Model, key: &str, text: &str) {
    emit_box(m, rush_plugin::EV_INPUT_CLEARED, key, text);
    let did = format!(
        "cleared · {} brings it back",
        crate::screens::social::parts::UNDO_HINT
    );
    let said = went_to(m, &did, "Cleared");
    m.flash(&said);
}

/// What plugins put on the edge of a message box: a session's by its
/// agent, the Agents box's by "" (boxNote).
pub fn box_note(m: &Model, key: &str) -> Line {
    let mut l = Line::new();
    let Some(h) = m.hooks() else { return l };
    let st = h.state();
    for (i, n) in st.notes.get(key).into_iter().flatten().enumerate() {
        if i > 0 {
            l.spans.push(sp(" · ", DIM));
        }
        let tone = if n.status.tone.is_empty() {
            "dim"
        } else {
            &n.status.tone
        };
        l.spans.push(toned(tone, &n.status.text));
    }
    l
}

/// What plugins add to a Session's overview (pluginOverview).
pub fn plugin_overview(m: &Model, key: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let Some(h) = m.hooks() else { return out };
    let st = h.state();
    for s in st.sections.get(key).into_iter().flatten() {
        out.push(Line::new());
        out.push(line([
            plain("  "),
            sp(s.section.title.clone(), SUB),
            sp(format!("  · {}", s.plugin), DIM),
        ]));
        for l in &s.section.lines {
            out.push(line([plain("    "), toned(&l.tone, &l.text)]));
        }
    }
    out
}

/// What plugins put on an agent's row, short (pluginStatus).
pub fn plugin_status(m: &Model, key: &str) -> Line {
    let mut l = Line::new();
    let Some(h) = m.hooks() else { return l };
    let st = h.state();
    for (i, s) in st.statuses.get(key).into_iter().flatten().enumerate() {
        if i > 0 {
            l.spans.push(plain(" "));
        }
        l.spans.push(toned(&s.status.tone, &s.status.text));
    }
    l
}
