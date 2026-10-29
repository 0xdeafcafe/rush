//! rush's own keys, as its key handling is written.
//!
//! Ported from internal/keymap/defaults.go:1-148 (list.land as of 2e4662ff; session.remember as of b2a1a0b3).

use crate::{Action, Context};

use Context::{Global as G, List as L, Pages as Pg, Prompt as P, Session as S};

/// One default action: id, context, title, keys, and whether it Runs.
type Row = (
    &'static str,
    Context,
    &'static str,
    &'static [&'static str],
    bool,
);

/// rush's own keys, as its key handling is written. Keys that only edit text
/// (arrows in a box, ctrl+a, backspace) aren't here: they belong to the box,
/// not to an action. None needs ⌥: where no ctrl or shift key is free,
/// ctrl+] then the letter is the chord for its ⌥ key. ⌥d, ⌥f, ⌥c and ⌥b are
/// the box's word keys, so their commands are the chord alone.
#[rustfmt::skip]
const DEFAULTS: &[Row] = &[
    ("guide.open", G, "these keys, from anywhere", &["f1"], true),
    ("bar.open", G, "the command bar", &["ctrl+k", "super+k"], false),
    ("bar.scope", G, "the command bar, next scope", &["ctrl+f"], false),
    ("quit", G, "quit rush", &["ctrl+q"], false),
    ("zen", G, "Zen on and off", &["ctrl+z"], false),
    ("place.prev", G, "the place before", &[",", "<"], false),
    ("place.next", G, "the next place", &[".", ">", "ctrl+\\"], false),
    ("chat.next", G, "the next open chat", &["ctrl+tab", "ctrl+] ]"], true),
    ("chat.prev", G, "the open chat before", &["ctrl+shift+tab", "ctrl+] ["], true),
    ("agent.new", G, "a new agent: the keys to Agents' Prompt", &["ctrl+shift+n", "ctrl+] ="], true),
    ("grid.beside", G, "pin the open agent as a tile beside the Session, and pick the next", &["ctrl+] |", "ctrl+] \\"], true),
    ("grid.below", G, "pin the open agent as a tile under the Session, and pick the next", &["ctrl+] -"], true),
    ("grid.unpin", G, "take the newest tile off the grid", &["ctrl+] k"], true),
    ("quick.ask", G, "the guide: ask rush what's next, where something is, or anything quick; again moves the keys", &["alt+/", "ctrl+] q"], true),
    ("profile.pick", G, "switch the default profile or provider", &["ctrl+] w", "alt+w"], false),

    ("list.up", L, "the agent above", &["up"], false),
    ("list.down", L, "the agent below", &["down"], false),
    ("list.pageup", L, "ten agents up", &["pgup"], false),
    ("list.pagedown", L, "ten agents down", &["pgdown"], false),
    ("list.first", L, "the first agent", &["home", "shift+up"], false),
    ("list.last", L, "the last agent", &["end", "shift+down"], false),
    ("list.enter", L, "open or rename the agent, or send what's typed", &["enter"], false),
    ("list.open", L, "open the agent's Session", &["ctrl+o", "super+down"], false),
    ("list.right", L, "into the agent, or unfold a heading", &["right"], false),
    ("list.left", L, "fold the heading", &["left"], false),
    ("list.back", L, "clear, go back, or (twice) quit", &["esc"], false),
    ("list.rename", L, "rename the agent", &["ctrl+r"], false),
    ("list.start", L, "pick the next session's agent, model and effort", &["shift+tab", "alt+m"], false),
    ("list.worktree", L, "new sessions from a worktree agent: its worktree or main checkout", &["ctrl+] l", "alt+l"], false),
    ("list.folder", L, "choose the folder, or move the agent", &["ctrl+l"], false),
    ("list.pr", L, "open the agent's pull request", &["ctrl+y"], false),
    ("list.pin", L, "pin the agent", &["ctrl+t"], false),
    ("list.groupby", L, "group agents by the next", &["ctrl+s"], false),
    ("list.split", L, "split each section by project, or stop", &["ctrl+p"], false),
    ("list.stop", L, "stop the agent with all it started, and hide it", &["ctrl+x"], false),
    ("list.close", L, "hide, stop, restart, switch or delete the agent", &["ctrl+] x"], true),
    ("list.done", L, "stop the agent and put it away in Earlier, or bring it back", &["ctrl+d", "ctrl+] d"], false),
    ("list.go", L, "tell the agent to keep going", &["ctrl+b", "alt+g"], false),
    ("list.land", L, "tell the agent to land its work: commit, then merge its branch or pull request", &["ctrl+] p", "alt+p"], false),
    ("list.claudeview", L, "a Claude Code agent's screen or its summary, with nothing typed", &["v"], false),
    ("list.view.prev", L, "the agent's view before", &["["], false),
    ("list.view.next", L, "the agent's next view", &["]"], false),
    ("list.needs", L, "the next agent that needs you", &["ctrl+n"], false),
    ("list.split.less", L, "less of the screen for the list", &["shift+left", "alt+left"], false),
    ("list.split.more", L, "more of the screen for the list", &["shift+right", "alt+right"], false),
    ("list.help", L, "the keys, with nothing typed", &["?"], false),
    ("list.editor", L, "write what's typed in your editor; with nothing typed, bring a Done agent back", &["ctrl+g"], false),
    ("list.image", L, "paste an image", &["ctrl+v", "super+v"], false),
    ("list.undo", L, "undo in the Prompt", &["super+z", "ctrl+_", "ctrl+/"], false),
    ("list.redo", L, "redo in the Prompt", &["shift+super+z"], false),
    ("list.focus", L, "into the Session, and back", &["tab", "{", "}"], false),
    ("list.filter", L, "filter agents by name or what's said", &["ctrl+] f"], true),
    ("list.toggle", L, "Agents beside the Session", &["ctrl+6", "ctrl+^", "ctrl+shift+6"], false),

    // The same keys as the list's split and rename, which have them back
    // once the Prompt is empty.
    ("prompt.stash", P, "set what's typed aside; it comes back after you send", &["ctrl+p"], true),
    ("prompt.append", P, "add what's typed to the open agent's box", &["ctrl+] a"], true),
    ("prompt.replace", P, "put what's typed in the open agent's box, in place of its draft", &["ctrl+] o"], true),
    ("prompt.history", P, "what you set aside, sent, cleared and replaced", &["ctrl+r"], true),

    ("page.prev", Pg, "the page before", &["[", "shift+tab"], false),
    ("page.next", Pg, "the next page", &["]", "tab"], false),
    ("page.go", Pg, "tell the agent to keep going", &["ctrl+b", "alt+g"], false),
    ("page.pr", Pg, "open the agent's pull request (Projects)", &["ctrl+y"], false),

    ("session.send", S, "send now, the queue first", &["ctrl+enter", "ctrl+s"], false),
    ("session.enter", S, "send the message from the composer", &["enter"], false),
    ("session.right", S, "open what's picked", &["right"], false),
    ("session.back", S, "back: off a row, out of a subagent, to Agents", &["left"], false),
    ("session.esc", S, "clear, stop the turn, or leave; twice asks to close it", &["esc"], false),
    ("session.up", S, "the row above", &["up"], false),
    ("session.down", S, "the row below", &["down"], false),
    ("session.pageup", S, "scroll up", &["pgup"], false),
    ("session.pagedown", S, "scroll down", &["pgdown"], false),
    ("session.first", S, "first message, keeping your draft", &["ctrl+home"], false),
    ("session.last", S, "latest message, keeping your draft", &["ctrl+end"], false),
    ("session.message", S, "read the full user message and attachments", &["ctrl+] enter", "alt+enter"], false),
    ("session.bottom", S, "back to the latest", &["end"], false),
    ("session.toggle", S, "open or close the selected row; type a space in the composer", &["space"], false),
    ("session.stash", S, "set what's typed aside, or bring it back; it comes back after you send", &["ctrl+p"], false),
    ("session.history", S, "what you set aside, sent, cleared and replaced", &["ctrl+r"], false),
    ("session.done", S, "stop the agent and put it away in Earlier, or bring it back", &["ctrl+d", "ctrl+] d"], false),
    ("session.hold", S, "hold or release the queue", &["ctrl+] h", "alt+h"], false),
    ("session.rewind", S, "rewind to the turn, or mark a file reviewed", &["ctrl+] r", "alt+r"], false),
    ("session.full", S, "the picked changed file in full", &["ctrl+] v", "alt+v"], false),
    ("session.fork", S, "fork from the turn", &["ctrl+] f"], true),
    ("session.remember", S, "remember the message: a cheap model writes a memory note from it", &["ctrl+] M", "alt+k"], false),
    ("session.copy", S, "copy the answer or drawing", &["ctrl+] c"], true),
    ("session.sub.next", S, "the next subagent run", &["shift+down", "alt+down"], false),
    ("session.sub.prev", S, "the subagent run before", &["shift+up", "alt+up"], false),
    ("session.view.prev", S, "the view before", &["["], false),
    ("session.view.next", S, "the next view", &["]"], false),
    ("session.history.open", S, "open all conversation turns", &["ctrl+] e"], true),
    ("session.history.close", S, "close older turns, keeping the latest open", &["ctrl+] s"], true),
    ("session.depth.0", S, "depth m0: only what's said, a row per run of steps", &["ctrl+] 0"], true),
    ("session.depth.1", S, "depth m1: every run of steps folded to a row", &["ctrl+] 1"], true),
    ("session.depth.2", S, "depth m2: as usual", &["ctrl+] 2"], true),
    ("session.depth.3", S, "depth m3: everything open", &["ctrl+] 3"], true),
    ("session.verbose", S, "more or less detail", &["ctrl+o"], false),
    ("session.stop", S, "stop it; again puts it away in Earlier", &["ctrl+x"], false),
    ("session.close", S, "hide, stop, restart, switch or delete it", &["ctrl+] x"], true),
    ("session.carry", S, "move what's typed to another session: pick it in the list, enter puts it in its box, esc brings it back", &["ctrl+] up"], true),
    ("session.toprompt", S, "move what's typed to Agents' Prompt, to start a new agent with", &["ctrl+] u"], true),
    ("session.go", S, "tell the agent to keep going", &["ctrl+b"], false),
    ("session.btw", S, "a side question in a panel, and back to the chat", &["ctrl+] b"], true),
    ("session.quickask.insert", S, "the quick ask's thread into the box, as one paste", &["ctrl+] i"], true),
    ("session.recall", S, "recall on and off: a cheap model's notes on the open chat, at the foot of the list", &["ctrl+] z"], true),
    ("session.background", S, "background what the turn waits on", &["ctrl+] j"], true),
    ("session.discuss", S, "discuss this chat in a room of agents; its verdict comes back here", &["ctrl+] t"], true),
    ("session.sendmode", S, "how enter sends: queue, guide, or stop and send while it works; or discuss it in a room first", &["ctrl+t"], false),
    ("session.mode", S, "choose session permissions", &["ctrl+] p"], true),
    ("session.setup", S, "choose this session’s model, effort and harness", &["shift+tab"], true),
    ("session.start", S, "pick the next session's agent, model and effort, or merge the queued message", &["ctrl+] m", "alt+m"], false),
    ("session.separate", S, "send the queue as one message or separately", &["ctrl+] o", "alt+o"], false),
    ("session.allow", S, "answer the card yes: allow it, or continue at the reset", &["ctrl+] y", "alt+y"], false),
    ("session.always", S, "allow it, and don't ask again", &["ctrl+] a", "alt+a"], false),
    ("session.deny", S, "answer the card no", &["ctrl+] n", "alt+n"], false),
    ("session.look", S, "the picked step's output as text, pretty or hex", &["v"], false),
    ("session.link", S, "what to do with the picked step's file or link", &["o"], false),
    ("session.folder", S, "choose where the agent works", &["ctrl+l"], false),
    ("session.needs", S, "the next agent that needs you", &["ctrl+n"], false),
    ("session.split.less", S, "less of the screen for the list", &["shift+left", "alt+left"], false),
    ("session.split.more", S, "more of the screen for the list", &["shift+right", "alt+right"], false),
    ("session.editor", S, "write what's typed in your editor; with nothing typed, bring a Done agent back", &["ctrl+g"], false),
    ("session.image", S, "paste an image", &["ctrl+v", "super+v"], false),
    ("session.undo", S, "undo in the box", &["super+z", "ctrl+_", "ctrl+/"], false),
    ("session.redo", S, "redo in the box", &["shift+super+z", "ctrl+y"], false),
    ("session.focus", S, "back to Agents, and in again", &["tab", "{", "}"], false),
    ("session.help", S, "the keys, with nothing typed", &["?"], false),
    ("session.list", S, "Agents beside the Session", &["ctrl+6", "ctrl+^", "ctrl+shift+6"], false),
];

/// rush's own actions with their default keys (Go's `keymap.Defaults`).
pub fn defaults() -> Vec<Action> {
    DEFAULTS
        .iter()
        .map(|&(id, context, title, keys, runs)| Action {
            id: id.into(),
            context,
            title: title.into(),
            keys: keys.iter().map(|k| k.to_string()).collect(),
            source: String::new(),
            runs,
        })
        .collect()
}

/// The action that runs a # command.
pub fn command_id(name: &str) -> String {
    format!("command:{name}")
}

/// The action that runs a plugin's command.
pub fn plugin_id(plugin: &str, command: &str) -> String {
    format!("plugin:{plugin}.{command}")
}
