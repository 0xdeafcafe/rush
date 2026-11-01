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

use crate::frame::Out;
use crate::frame::caps::agent_can_run;
use crate::model::Model;
use crate::screens::social::broadcast::ModelBroadcast;
use crate::style::*;

// rush's # commands are the base's: the keymap's actions name them
// (crate::keybind).
pub use crate::keybind::FLEET_COMMANDS;

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

/// The / commands a new session from the Agents box would have: its
/// agent's own and its skills, read from disk (newSessionCommands).
fn new_session_commands(m: &Model) -> Vec<rush_core::adapter::Command> {
    let kind = crate::screens::accounts::rows::start_kind(m);
    let Some(ad) = m.reg.get(&kind) else {
        return Vec::new();
    };
    let p = rush_core::adapter::Profile {
        kind: rush_core::adapter::Kind::from(kind.as_str()),
        ..Default::default()
    };
    let dir = crate::startdir::start_dir(m);
    crate::screens::session_cards::claudescreen::local_commands(&m.reg, &ad, p, &dir)
}

/// A Prompt command typed the old way, /stop for #stop, run as the #
/// command when the new session's agent has no command of that name,
/// saying what it's called now; None when it isn't one (legacyCommand).
pub fn legacy_command(m: &mut Model, text: &str) -> Option<Out> {
    let first = text.split_whitespace().next()?;
    let name = first.strip_prefix('/')?.to_lowercase();
    if !is_fleet_command(&name) || new_session_commands(m).iter().any(|c| c.name == name) {
        return None;
    }
    let before = m.flash.clone();
    m.input.clear_sent();
    let o = m.command(&format!("#{}", &text[1..]));
    if m.flash == before {
        m.flash(&format!(
            "rush's commands start with # now: #{name} · / starts a session with one of Claude's"
        ));
    }
    Some(o)
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

/// What a command offers once you've typed a space, and which of them is
/// in use now (fleetArgs): #on's machines, #broadcast's names and #view's
/// now as well as the fixed ones. #with's are newcmd's.
pub fn fleet_args_of(m: &Model, name: &str) -> (Vec<String>, String) {
    match name {
        "on" => {
            let cfg = rush_remote::load_config().unwrap_or_default();
            (
                cfg.peers.into_iter().map(|p| p.name).collect(),
                String::new(),
            )
        }
        "broadcast" => {
            let mut opts = vec!["all".to_string()];
            for i in m.broadcast_all() {
                opts.push(format!(
                    "@{}",
                    crate::screens::social::broadcast::mention_name(&m.agents[i])
                ));
            }
            (opts, String::new())
        }
        "view" => {
            let now = match crate::split::widths(m) {
                (0, _) => "agent",
                (_, 0) => "list",
                _ => "split",
            };
            (
                fleet_args(name).into_iter().map(str::to_string).collect(),
                now.to_string(),
            )
        }
        _ => (
            fleet_args(name).into_iter().map(str::to_string).collect(),
            String::new(),
        ),
    }
}

/// The choices a command offers that need nothing but its name.
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
/// (hashMatches; #new and #with's are newcmd's, #use's setup.rs's
/// setup_args, plugins' commands pluginui.go's pluginHashCommands).
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
    if has_arg && name == "use" {
        let now = crate::screens::accounts::setup::hash_start(m);
        return crate::screens::accounts::setup::setup_args(m, &text, &now);
    }
    if !has_arg {
        let mut all = available_fleet_commands(m);
        all.extend(extra.iter().cloned());
        // Plugins' commands as # commands: #haven.open.
        all.extend(
            crate::screens::plugin_ui::hooks::plugin_hash_commands(m)
                .into_iter()
                .map(|(name, description)| Command {
                    name,
                    description,
                    ..Command::default()
                }),
        );
        return filter_commands(&name.to_lowercase(), &[&all]);
    }
    if q.contains(' ') {
        return Vec::new();
    }
    let (opts, now) = fleet_args_of(m, name);
    opts.into_iter()
        .filter(|o| o.to_lowercase().starts_with(&q.to_lowercase()))
        .map(|o| Command {
            description: if o == now {
                "now".into()
            } else {
                String::new()
            },
            name: format!("{name} {o}"),
            ..Command::default()
        })
        .collect()
}

/// The commands that make sense for the agent in view, the open Session's
/// or the list's pick: those that act on one only once one is, and only
/// those its agent can run; #fork only in its open Session, #slim only
/// while rush hosts it and has counted its context (availableFleetCommands).
pub fn available_fleet_commands(m: &Model) -> Vec<Command> {
    let open = m.session.as_ref().map(|v| v.key.as_str());
    let a = open
        .and_then(|k| m.agents.iter().find(|a| a.key == k))
        .or_else(|| m.selected().map(|i| &m.agents[i]));
    // rush runs it and has counted what fills its context (c.client,
    // c.sess.Usage): #slim has nothing to show before.
    let counted =
        |k: &str| crate::sess::peek_conn(m, k).is_some_and(|c| c.hosted() && c.context.is_some());
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
            if c.name == "full" && (a.rush || a.interactive || a.past) {
                return false;
            }
            if c.name == "pin"
                && m.reg
                    .get(a.kind.as_str())
                    .is_none_or(|ad| ad.pinner().is_none())
            {
                return false;
            }
            let mine = open == Some(a.key.as_str());
            match c.name.as_str() {
                "fork" => mine,
                "slim" => mine && counted(&a.key),
                _ => true,
            }
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
