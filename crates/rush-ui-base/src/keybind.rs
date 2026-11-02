//! The keymap in force and what it binds: rush's keys and # commands
//! (and plugins' commands, through the plugin_key_actions and plugin_keys
//! slots), the keys bound to an action, a chord waiting. Turning a key
//! pressed through it is the command bar's (screens/cmdbar/keybind.rs).
//! Port of internal/ui/keybind.go:15-102 (keyState :15, chordWait :28,
//! keyActions :32, loadKeys :42, setKeys :54, keyMap :59) and
//! fleetslash.go:22 (fleetCommands).

use std::time::{Duration, Instant};

use rush_keymap::{Action, Context, File, Map, Seq};

use crate::frame::Out;
use crate::model::Model;

/// rush's # commands: name, what it does, and its argument's hint (<>
/// needs one, [] can go without).
pub const FLEET_COMMANDS: &[(&str, &str, &str)] = &[
    (
        "expand",
        "open all conversation turns; tool details keep their own folds",
        "",
    ),
    (
        "collapse",
        "preview older turns, keeping the latest turn open",
        "",
    ),
    ("model", "choose the current session’s model", "[model]"),
    (
        "effort",
        "choose the current session’s reasoning effort",
        "[level]",
    ),
    (
        "use",
        "what the next session starts as, or in a session switch it (same row; the old one retires), then a message to send it; alone opens the sheet (Shift+Tab)",
        "[harness:account[:effort]] [message]",
    ),
    (
        "done",
        "put the agent away in Earlier (alt+d); its idle process stops",
        "",
    ),
    (
        "room",
        "a full-screen group chat: fresh agents argue a topic to a verdict, and you're in it",
        "[new|topic]",
    ),
    (
        "discuss",
        "a room on this chat: agents argue it, with its latest turns as context, and the verdict comes back here",
        "[topic]",
    ),
    (
        "intervene",
        "two agents go through this chat (or @agent's): plan, status, turns and tool calls; they tell you why it's stuck, and what the agent must do goes in its box",
        "[@agent] [concern]",
    ),
    (
        "broadcast",
        "send one message to several agents: all, @names, or alone to click them in the list",
        "[all|@agent…] [message]",
    ),
    (
        "log",
        "the last 50 things rush said: answers, what failed, and what it did on its own (clean-ups, worktrees kept, work landed)",
        "",
    ),
    (
        "feed",
        "the feed your agents share: blockers, fixes, heads-ups; on|off switches it for all of rush; open|closed lets agents cross projects",
        "[on|off|open|closed|new]",
    ),
    (
        "land",
        "tell the agent to land its work: commit it, then merge its branch or pull request (alt+p)",
        "",
    ),
    (
        "go",
        "tell the agent to keep going (alt+g); after an error, to continue",
        "",
    ),
    (
        "away",
        "leave the session to carry on while you're gone: at each check-in (every 30m unless you say) it's told to keep going, and what it asks or needs approved is saved for when you're back, until the time's up or it's all done; off, or alone, says what happened",
        "[3h [every 15m]|off]",
    ),
    (
        "loop",
        "tell the agent to keep going each time it's idle at a check-in (every 30m unless you say), till it's all done or the time you give is up; questions wait for you as ever",
        "[3h] [every 15m]|off",
    ),
    (
        "back",
        "what happened while you were away: check-ins, its last words, what it saved for you; ends away",
        "",
    ),
    ("stop", "stop the agent", ""),
    (
        "rm",
        "delete the session, and its worktree when that's safe",
        "",
    ),
    (
        "worktree",
        "whether new sessions in this project get a git worktree and branch of their own (on unless turned off); alone says which",
        "[on|off]",
    ),
    ("kill", "kill the agent and everything it started", ""),
    (
        "perm",
        "choose session permissions (also in Shift+Tab)",
        "[mode]",
    ),
    (
        "yolo",
        "explicitly enable this harness’s supported bypass permission mode",
        "",
    ),
    (
        "fork",
        "copy this conversation to another harness in a new session, from a summary, and keep both",
        "<harness>",
    ),
    (
        "compact",
        "compact this session with a model of your choosing: its own (keeps the cache), a cheaper Claude, or a local Ollama one",
        "",
    ),
    (
        "remember",
        "keep something in this project's memory: a cheap model writes the note (alt+k on a message remembers it)",
        "<what to remember>",
    ),
    (
        "slim",
        "what this session carries every request and never uses (MCP servers, subagents, skills): drop them for it alone, or compact it (#optimise)",
        "",
    ),
    (
        "clean",
        "delete the agent's temp work; all does every finished agent",
        "[all]",
    ),
    (
        "cd",
        "tell the agent to work in another folder from now on",
        "[path]",
    ),
    ("rename", "rename the agent, or type the new name", "[name]"),
    ("pin", "pin or unpin the agent in its harness", ""),
    ("pr", "open the agent's pull request", ""),
    (
        "full",
        "open a Claude Code agent full screen, in Claude Code",
        "",
    ),
    (
        "rush",
        "move the agent into rush mode (a terminal one is copied, not stopped)",
        "",
    ),
    (
        "new",
        "start an agent on any harness, provider, model and effort, once; defaults stay as they are",
        "[harness@provider[:account]] [model] [effort] [task]",
    ),
    (
        "on",
        "start an agent on another machine's rush (rush remote add): its folder, its agent; shows here while rush remote attach runs",
        "MACHINE [folder] [agent=KIND] task",
    ),
    (
        "with",
        "what the next session starts as, once: #new without a task",
        "[harness@provider[:account]] [model] [effort]",
    ),
    (
        "statusline",
        "build the top bar, the agent header and Claude Code's status line",
        "",
    ),
    (
        "why",
        "what rush did to this session in the last hour and why: woken, held, moved on, stopped (rush why)",
        "",
    ),
    (
        "network",
        "whether the API answers, the network rush is on and how fast it moves, and what waits for it (#net)",
        "",
    ),
    (
        "view",
        "Agents and the Session side by side, the agent's Session alone, or Agents alone (shift+← →)",
        "<split|agent|list>",
    ),
    (
        "stash",
        "what you set aside, sent, cleared and replaced, to put back in the box (ctrl+r · ctrl+s sets what's typed aside)",
        "",
    ),
    (
        "guide",
        "ask the guide, a fast model that sees your sessions: what's next, which one needs you, where something is; it opens the one it means",
        "[question]",
    ),
    (
        "ask",
        "ask rush about itself, or have it change a setting for you: an agent in rush's own folder, with its guide",
        "[question]",
    ),
    ("help", "a short guide to rush", ""),
    (
        "update",
        "install the newest rush (brew, or its GitHub release), then restart onto it; sessions move to it in place (alt+u, or click the header's ↑)",
        "",
    ),
    (
        "reload",
        "run the installed rush in this one's place, where you were; sessions carry on and move onto it between turns; all reloads the other rush views first",
        "[all]",
    ),
    (
        "tips",
        "Getting started and tips from the top; off puts them away",
        "[off]",
    ),
    ("quit", "leave rush", ""),
];

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
    pub loading: bool,
    /// A key handed on, remapped, through every binding again: it isn't
    /// turned a second time.
    pub passing: bool,
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
pub fn actions_of(m: &Model) -> Vec<Action> {
    let mut out = key_actions();
    let plugin_actions = m.ext.calls.plugin_key_actions();
    out.extend(plugin_actions(m));
    out
}

/// Puts f in force (setKeys), with the keys plugins suggest.
pub fn set_keys(m: &mut Model, f: File) {
    let map = Map::build(&actions_of(m), &f, &(m.ext.calls.plugin_keys())(m));
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
            &(m.ext.calls.plugin_keys())(m),
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

/// A chord is waiting for its next key (len(m.keys.chord) > 0).
pub fn chord_waiting(m: &mut Model) -> bool {
    let st = m.kept::<KeyState>();
    !st.chord.is_empty() && st.chord_at.is_some_and(|t| t.elapsed() <= CHORD_WAIT)
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
