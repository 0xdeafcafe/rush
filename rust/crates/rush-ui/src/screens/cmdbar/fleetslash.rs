//! rush's own # commands, and the picker that offers them as one is
//! typed: their names, aliases, which act on an agent, what they offer
//! after a space, and how a pick completes the box. / is always the
//! agent's. Port of internal/ui/fleetslash.go:1-440 (fleetCommands :22,
//! fleetAliases :77, fleetNeedsAgent :81, isHashCmd :89, filterCommands
//! :145, hashMatches :174, pickerMove/roundMove :354, needsArg :373,
//! completed :377, availableFleetCommands :405; #log and #land as of
//! 2dcf46c6 and 2e4662ff) and sessionviews.go:753
//! (pickerRows). What a command does is the screen's that owns it, each
//! registered with `Ext::command`.

use rush_core::event::Command;
use rush_core::text::Line;

use crate::frame::caps::agent_can_run;
use crate::model::Model;
use crate::style::*;

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
        "mackeys",
        "send Terminal.app's ⌘← → ⌘⌫ ⌘⌦ ⌘Z on to rush through Hammerspoon, installed with brew if need be; alone says whether it's on",
        "[on|off]",
    ),
    (
        "ghostty",
        "put Ghostty on LangWatch's light and dark themes, following the system; off puts its own colours back; alone says whether it's on",
        "[on|off]",
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
        "install the newest rush, with go install; #reload runs it",
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

/// Other names a command answers to.
pub const FLEET_ALIASES: &[(&str, &str)] = &[
    ("agent", "use"),
    ("handoff", "fork"),
    ("profile", "use"),
    ("preset", "use"),
    ("permissions", "perm"),
    ("optimise", "slim"),
    ("optimize", "slim"),
    ("trim", "slim"),
    ("bloat", "slim"),
    ("undone", "done"),
    ("delete", "rm"),
    ("move", "cd"),
    ("exit", "quit"),
    ("history", "stash"),
    ("drafts", "stash"),
    ("net", "network"),
    ("afk", "away"),
    ("flight", "away"),
    ("twotter", "feed"),
    ("twatter", "feed"),
    ("twitter", "feed"),
    ("twattr", "feed"),
    ("community", "feed"),
    ("chirp", "feed"),
    ("chirps", "feed"),
    ("memorise", "remember"),
    ("memorize", "remember"),
];

/// # commands that act on the selected or focused agent: offered only
/// once one's in view. The rest are rush-wide.
pub const FLEET_NEEDS_AGENT: &[&str] = &[
    "done", "go", "land", "away", "loop", "back", "stop", "rm", "kill", "clean", "cd", "rename",
    "pin", "pr", "full", "rush", "compact", "slim", "fork",
];

/// The command a name stands for: itself, or what it's an alias of.
pub fn alias_of(name: &str) -> &str {
    FLEET_ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, n)| n)
}

/// Whether a # command acts on an agent.
pub fn needs_agent(name: &str) -> bool {
    FLEET_NEEDS_AGENT.contains(&name)
}

/// rush's # commands as Commands.
pub fn fleet_commands() -> Vec<Command> {
    FLEET_COMMANDS
        .iter()
        .map(|(n, d, h)| Command {
            name: n.to_string(),
            description: d.to_string(),
            argument_hint: h.to_string(),
            aliases: Vec::new(),
        })
        .collect()
}

/// Whether text is a # command: # and a letter, so a Markdown heading
/// (# Plan) or an issue (#123) is still a message.
pub fn is_hash_cmd(text: &str) -> bool {
    let mut c = text.chars();
    c.next() == Some('#') && c.next().is_some_and(char::is_alphabetic)
}

/// Whether a # command is being typed, # alone included.
pub fn typing_hash(text: &str) -> bool {
    text == "#" || is_hash_cmd(text)
}

/// Whether name (without its prefix) is one of rush's.
pub fn is_fleet_command(name: &str) -> bool {
    FLEET_ALIASES.iter().any(|(a, _)| *a == name) || FLEET_COMMANDS.iter().any(|c| c.0 == name)
}

/// The commands whose name contains q, those starting with it first; an
/// argument hint leads the description (filterCommands).
pub fn filter_commands(q: &str, lists: &[&[Command]]) -> Vec<Command> {
    let mut out: Vec<Command> = Vec::new();
    for list in lists {
        for c in *list {
            if out.iter().any(|o| o.name == c.name) || !c.name.to_lowercase().contains(q) {
                continue;
            }
            let mut c = c.clone();
            if !c.argument_hint.is_empty() {
                c.description = format!("{}  {}", c.argument_hint, c.description)
                    .trim()
                    .to_string();
            }
            out.push(c);
        }
    }
    // A plugin's plugin:name starts with q when its name does.
    let starts = |name: &str| {
        let name = name.to_lowercase();
        let short = name.split_once(':').map_or("", |x| x.1).to_string();
        name.starts_with(q) || short.starts_with(q)
    };
    out.sort_by_key(|c| !starts(&c.name));
    out
}

/// What a command offers once you've typed a space, and which is in use
/// now: the ones that need nothing but the list (fleetArgs). The rest
/// (#on's machines, #with's agents, #broadcast's names, #view's now) are
/// their screens'.
pub fn fleet_args(name: &str) -> Vec<&'static str> {
    match name {
        "clean" => vec!["all"],
        "away" => vec!["1h", "2h", "3h", "5h", "8h", "off"],
        "loop" => vec!["every 15m", "every 30m", "every 1h", "off"],
        "view" => vec!["split", "agent", "list"],
        _ => Vec::new(),
    }
}

/// What the picker offers for a # command being typed: the commands
/// containing what's typed, or after a space, the command's choices
/// (hashMatches; #new and #with's are newcmd's, without #use's own).
pub fn hash_matches(m: &Model, input: &[char], back: usize, extra: &[Command]) -> Vec<Command> {
    let text: String = input.iter().collect();
    if back != 0
        || !text.starts_with('#')
        || text.contains('\n')
        || input.len() > 1 && !input[1].is_alphabetic()
    {
        return Vec::new();
    }
    let (name, q, has_arg) = match text[1..].split_once(' ') {
        Some((n, q)) => (n, q, true),
        None => (&text[1..], "", false),
    };
    if has_arg && (name == "new" || name == "with") {
        return crate::screens::accounts::newcmd::new_args(m, name, q);
    }
    let name = alias_of(name);
    if !has_arg {
        let mut all = available_fleet_commands(m);
        all.extend(extra.iter().cloned());
        return filter_commands(&name.to_lowercase(), &[&all]);
    }
    if q.contains(' ') {
        return Vec::new();
    }
    fleet_args(name)
        .into_iter()
        .filter(|o| o.to_lowercase().starts_with(&q.to_lowercase()))
        .map(|o| Command {
            name: format!("{name} {o}"),
            ..Command::default()
        })
        .collect()
}

/// The commands that make sense for the agent in view: those that act on
/// one only once one is, and only those its agent can run
/// (availableFleetCommands; what needs a hosted session is left out until
/// rush hosts them).
pub fn available_fleet_commands(m: &Model) -> Vec<Command> {
    let a = m.focused().map(|i| &m.agents[i]);
    fleet_commands()
        .into_iter()
        .filter(|c| {
            if needs_agent(&c.name) && a.is_none() {
                return false;
            }
            let Some(a) = a else { return true };
            if !agent_can_run(&m.reg, a.kind.as_str(), &c.name) {
                return false;
            }
            if (c.name == "native" || c.name == "full")
                && !m
                    .reg
                    .supports(a.kind.as_str(), rush_core::adapter::Feature::Screen)
            {
                return false;
            }
            if c.name == "full" && (a.interactive || a.past) {
                return false;
            }
            if c.name == "pin"
                && m.reg
                    .get(a.kind.as_str())
                    .is_none_or(|ad| ad.pinner().is_none())
            {
                return false;
            }
            // #fork and #slim need the session hosted here.
            !matches!(c.name.as_str(), "fork" | "slim")
        })
        .collect()
}

/// A menu's cursor moved d rows round: ↑ on the first row goes to the
/// last, ↓ on the last to the first.
pub fn round_move(cur: usize, d: isize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let n = n as isize;
    (((cur as isize + d) % n + n) % n) as usize
}

/// up or down as a move round the picker.
pub fn picker_move(sel: usize, n: usize, s: &str) -> usize {
    round_move(sel, if s == "up" { -1 } else { 1 }, n)
}

/// Whether a picked command can't run without an argument.
pub fn needs_arg(c: &Command) -> bool {
    c.argument_hint.starts_with('<')
}

/// The box's text once a command is picked: with a space to type its
/// argument after when it takes one, or always with tab.
pub fn completed(lead: &str, c: &Command, tab: bool) -> Vec<char> {
    let mut out: Vec<char> = format!("{lead}{}", c.name).chars().collect();
    if needs_arg(c) || tab && !c.argument_hint.is_empty() {
        out.push(' ');
    }
    out
}

/// The picker's rows, at most six round the one picked, and a line of
/// how to use it under them (sessionviews.go:753 pickerRows).
pub fn picker_rows(
    cmds: &[Command],
    sel: usize,
    w: usize,
    lead: &str,
    tag: impl Fn(&str) -> Option<Line>,
    how: &str,
) -> Vec<Line> {
    let start = sel.saturating_sub(5);
    let end = cmds.len().min(start + 6);
    let name_w = cmds[start..end]
        .iter()
        .map(|c| c.name.chars().count() + 1)
        .max()
        .unwrap_or(0)
        .min(28);
    let mut out = Vec::new();
    for (i, c) in cmds.iter().enumerate().take(end).skip(start) {
        let name = spb(fit_str(&format!("{lead}{}", c.name), name_w + 1), BRIGHT);
        let t = tag(&c.name);
        let tw = t.as_ref().map_or(0, Line::width);
        let desc = rush_convo::render::key_word(&one_line(&c.description));
        let desc =
            rush_draw::truncate(&desc, 10.max(w.saturating_sub(name_w + 7 + tw)), "…").into_owned();
        let mut row = if i == sel {
            line([sp(" ▸ ", ORANGE), name, plain("  "), sp(desc, DIM)])
        } else {
            line([plain("   "), name, plain("  "), sp(desc, DIM)])
        };
        if let Some(t) = t {
            join(&mut row, t);
        }
        out.push(on_ground(row, w, if i == sel { SEL_BG } else { CHROME_BG }));
    }
    let more = if cmds.len() > end - start {
        format!("{} of {} · ", sel + 1, cmds.len())
    } else {
        String::new()
    };
    out.push(on_ground(
        line([plain("   "), sp(format!("{more}{how}"), DIM)]),
        w,
        CHROME_BG,
    ));
    out
}
