//! The picker over a session's box as a command is typed: rush's own /
//! commands, the Claude Code screens rush draws or opens, the session's
//! commands, the custom commands and skills on disk, and claude: ones,
//! for the /word at the cursor; a command's values after a space (/model
//! so); rush's # commands; and @ mentions. ↑↓ choose, tab completes, enter
//! runs (or completes, mid-message).
//!
//! After Go's internal/ui/sessionviews.go:465-473 (claudeCloudPicks),
//! :541-551 (openableScreens), :555-575 (slashWord), :577-645
//! (slashMatches), :647-665 (loadLocal), :677-701 (argMatches), :708-751
//! (slashLines), :785-843 (slashKey), caps.go:67-76 (sessionCommands),
//! choicesheet.go:17-29 (argOptions) and fleetslash.go:317-353
//! (paneHashKey). The rows are cmdbar's pickerRows (sessionviews.go:753).
//! The commands on disk are read off the UI thread, at most every 30
//! seconds while a / word is typed (commandsOf's cache).

use rush_core::adapter::Registry;
use rush_core::event::Command;
use rush_core::text::Line;

use super::{agent, conn, keys, open_key};
use crate::frame::caps::{can_run, can_screen, own_screens};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::accounts::choices::{arg_now, arg_options};
use crate::screens::cmdbar::fleetslash::{
    completed, hash_matches, needs_arg, picker_move, picker_rows, round_move,
};
use crate::screens::session_cards::claudescreen::{
    ALIASES, CLAUDE_CLOUD, CLAUDE_SCREENS, RUSH_COMMANDS, RUSH_SCREENS, acct_of, claude_cloud_name,
    claude_screen, kind_of, local_commands, off_command,
};
use crate::screens::session_cards::mention::{complete_mention, mention_matches, mention_rows};
use crate::style::{BLUE, ORANGE, SUB, line, sp};

/// How long the commands on disk are kept before they're read again.
const LOCAL_FOR: i64 = 30_000;

fn cmd(name: &str, description: &str, hint: &str) -> Command {
    Command {
        name: name.into(),
        description: description.into(),
        argument_hint: hint.into(),
        ..Command::default()
    }
}

/// rush's own session commands (rushCommands).
pub fn rush_commands() -> Vec<Command> {
    RUSH_COMMANDS.iter().map(|(n, d, h)| cmd(n, d, h)).collect()
}

/// Claude Code's own screens (claudeScreens).
pub fn claude_screens() -> Vec<Command> {
    CLAUDE_SCREENS.iter().map(|(n, d)| cmd(n, d, "")).collect()
}

/// Claude Code's account and cloud screens as the picker offers them:
/// claude:login (claudeCloudPicks).
pub fn claude_cloud_picks() -> Vec<Command> {
    CLAUDE_CLOUD
        .iter()
        .map(|(n, d)| cmd(&format!("claude:{n}"), d, ""))
        .collect()
}

/// rush's commands an agent of kind can do (sessionCommands).
pub fn session_commands(reg: &Registry, kind: &str) -> Vec<Command> {
    rush_commands()
        .into_iter()
        .filter(|c| can_run(reg, kind, &c.name, claude_screen(&c.name)))
        .collect()
}

/// The screens an agent of kind can open (openableScreens).
pub fn openable_screens(reg: &Registry, kind: &str) -> Vec<Command> {
    claude_screens()
        .into_iter()
        .filter(|c| can_screen(reg, kind, &c.name))
        .collect()
}

/// The /word being typed at the cursor pos, at the start of the message
/// or after a space: where it starts and ends, and what's typed of it
/// lower-cased. A word with a second slash is a path (slashWord).
pub fn slash_word(input: &[char], pos: usize) -> Option<(usize, usize, String)> {
    let pos = pos.min(input.len());
    let mut start = pos;
    while start > 0 && !input[start - 1].is_whitespace() {
        start -= 1;
    }
    if start >= input.len() || input[start] != '/' || start == pos {
        return None;
    }
    let mut end = pos;
    while end < input.len() && !input[end].is_whitespace() {
        end += 1;
    }
    let q: String = input[start + 1..pos].iter().collect();
    if q.contains('/') {
        return None;
    }
    Some((start, end, q.to_lowercase()))
}

/// What the picker reads of a session: its agent's kind, the commands
/// the session said it has, and those on disk.
pub struct Seen<'a> {
    pub reg: &'a Registry,
    pub kind: &'a str,
    pub said: &'a [Command],
    pub local: &'a [Command],
}

fn alias(name: &str) -> Option<&'static str> {
    ALIASES.iter().find(|(a, _)| *a == name).map(|(_, n)| *n)
}

/// What the picker offers for the /word at the cursor: rush's own
/// commands and the screens (at the start of a message only), the
/// session's, then the custom commands and skills found on disk, and for
/// an agent with screens of its own, claude: ones; what's typed exactly
/// first, then names starting with it, then the rest (slashMatches).
pub fn slash_matches(s: &Seen, input: &[char], pos: usize) -> Vec<Command> {
    let Some((start, _, q)) = slash_word(input, pos) else {
        return Vec::new();
    };
    let mut lists: Vec<Vec<Command>> = Vec::new();
    if start == 0 {
        // Claude Code's own screens aren't another agent's; the ones rush
        // draws are, when its agent has what they show.
        lists.push(session_commands(s.reg, s.kind));
        lists.push(openable_screens(s.reg, s.kind));
    }
    lists.push(s.said.to_vec());
    lists.push(s.local.to_vec());
    if start == 0 && own_screens(s.reg, s.kind) {
        lists.push(claude_cloud_picks());
    }
    let mut out: Vec<Command> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Another name for one of Claude Code's screens (/plugins) finds it
    // first.
    if let Some(name) = alias(&q)
        && start == 0
        && can_screen(s.reg, s.kind, name)
        && let Some((n, d)) = CLAUDE_SCREENS.iter().find(|(n, _)| *n == name)
    {
        seen.insert(n.to_string());
        out.push(cmd(n, d, ""));
    }
    for list in lists {
        for c in list {
            // What rush leaves out or leaves to Claude Code isn't offered
            // under Claude Code's own name.
            let cloud = claude_cloud_name(&c.name).is_some();
            if seen.contains(&c.name)
                || off_command(&c.name).is_some()
                || cloud && !c.name.starts_with("claude:")
            {
                continue;
            }
            // claude:login is found by login, or by claude: itself.
            let mut name = c.name.to_lowercase();
            if !q.starts_with("claude:") {
                name = name.strip_prefix("claude:").unwrap_or(&name).to_string();
            }
            if name.contains(&q) {
                seen.insert(c.name.clone());
                out.push(c);
            }
        }
    }
    let rank = |c: &Command| {
        let lower = c.name.to_lowercase();
        let n = lower.strip_prefix("claude:").unwrap_or(&lower);
        if n == q || alias(&q) == Some(c.name.as_str()) {
            0
        } else if n.starts_with(&q) {
            1
        } else {
            2
        }
    };
    out.sort_by_key(rank);
    out
}

/// The picker for a command's value: "/model so" offers the models
/// starting with so, the one it runs with marked; nothing when the box
/// isn't a command with a list and a space (argMatches). picked is the
/// value last picked here, running what it runs with.
pub fn arg_matches(
    reg: &Registry,
    kind: &str,
    input: &[char],
    pos: usize,
    picked: Option<&str>,
    running: &str,
) -> Vec<Command> {
    let text: String = input.iter().collect();
    if pos != input.len() || !text.starts_with('/') || text.contains('\n') {
        return Vec::new();
    }
    let Some((name, q)) = text[1..].split_once(' ') else {
        return Vec::new();
    };
    let Some(mut opts) = arg_options(reg, kind, name) else {
        return Vec::new();
    };
    if q.contains(' ') {
        return Vec::new();
    }
    let (now, known) = arg_now(picked, running, &opts);
    opts.push(rush_core::adapter::Choice {
        id: "default".into(),
        note: "the agent's own".into(),
        ..Default::default()
    });
    let q = q.to_lowercase();
    let mut out = Vec::new();
    for o in opts {
        if !o.id.starts_with(&q) {
            continue;
        }
        let mut d = o.note.clone();
        if known && (now == o.id || now.is_empty() && o.id == "default") {
            d = format!("{d} · now");
            d = d.strip_prefix(" · ").map(str::to_string).unwrap_or(d);
        }
        out.push(cmd(&format!("{name} {}", o.id), &d, ""));
    }
    out
}

/// The box of session key, as the picker reads it: its text and cursor.
fn typed(m: &mut Model, key: &str) -> (Vec<char>, usize) {
    let c = conn(m, key);
    (c.input.text.clone(), c.input.pos)
}

/// The values session key's box offers for the command typed (paneArgs).
fn pane_args(m: &mut Model, key: &str) -> Vec<Command> {
    let (input, pos) = typed(m, key);
    if !input.first().is_some_and(|&c| c == '/') {
        return Vec::new();
    }
    let kind = kind_of(m, key);
    let name: String = input[1..].iter().take_while(|c| **c != ' ').collect();
    let picked = conn(m, key).picked.get(&name).cloned();
    let run = super::rushcmd::running(m, key, &name);
    arg_matches(&m.reg, &kind, &input, pos, picked.as_deref(), &run)
}

/// The / picker's matches for session key's box.
fn matches(m: &mut Model, key: &str) -> Vec<Command> {
    let (input, pos) = typed(m, key);
    if slash_word(&input, pos).is_none() {
        return Vec::new();
    }
    let kind = kind_of(m, key);
    let said = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .map(|v| v.sess.command_list.clone())
        .unwrap_or_default();
    let local = conn(m, key).local.clone();
    let s = Seen {
        reg: &m.reg,
        kind: &kind,
        said: &said,
        local: &local,
    };
    slash_matches(&s, &input, pos)
}

/// Reads the commands and skills on disk for session key's agent, off
/// the UI thread, when they were last read over 30 seconds ago
/// (loadLocal, commandsOf).
fn load_local(m: &mut Model, key: &str) {
    let now = m.now;
    if conn(m, key).local_at.is_some_and(|at| now - at < LOCAL_FOR) {
        return;
    }
    let Some(a) = agent(m, key).cloned() else {
        return;
    };
    let kind = kind_of(m, key);
    let Some(ad) = m.reg.get(&kind).filter(|ad| ad.commander().is_some()) else {
        return;
    };
    conn(m, key).local_at = Some(now);
    let cwd = m
        .session
        .as_ref()
        .filter(|v| v.key == key && !v.sess.cwd.is_empty())
        .map_or(a.job.cwd.clone(), |v| v.sess.cwd.clone());
    let (reg, p, key) = (m.reg.clone(), acct_of(m, &a), key.to_string());
    m.later(
        move || local_commands(&reg, &ad, p, &cwd),
        move |m, found| {
            let c = conn(m, &key);
            c.skills = found
                .iter()
                .filter(|f| f.skill)
                .map(|f| f.name.clone())
                .collect();
            c.local = found
                .into_iter()
                .map(|f| cmd(&f.name, &f.description, &f.argument_hint))
                .collect();
            Out::Draw
        },
    );
}

/// What's drawn after a command's name: rush's own, Claude Code's (↗,
/// it opens Claude Code), or a skill.
fn tag(name: &str, start: usize, skills: &std::collections::HashSet<String>) -> Option<Line> {
    let rush = || Some(line([sp(" rush", ORANGE)]));
    if RUSH_COMMANDS.iter().any(|(n, _, _)| *n == name) {
        return rush();
    }
    if start == 0 && RUSH_SCREENS.contains(&name) {
        return rush();
    }
    if start == 0 && (name.starts_with("claude:") || CLAUDE_SCREENS.iter().any(|(n, _)| *n == name))
    {
        return Some(line([sp(" claude code ↗", SUB)]));
    }
    if skills.contains(name) {
        return Some(line([sp(" skill", BLUE)]));
    }
    None
}

/// The picker over session key's box, w wide: # commands, @ mentions,
/// a command's values, else the / commands (slashLines). Nothing when
/// nothing's being typed that it offers.
pub fn slash_lines(m: &mut Model, key: &str, w: usize) -> Vec<Line> {
    let (input, pos) = typed(m, key);
    let back = input.len() - pos.min(input.len());
    let cmds = hash_matches(m, &input, back, &[]);
    if !cmds.is_empty() {
        let c = conn(m, key);
        c.slash_sel = c.slash_sel.min(cmds.len() - 1);
        return picker_rows(
            &cmds,
            c.slash_sel,
            w,
            "#",
            |_| None,
            "rush's, for this agent · ↑↓ · tab completes · enter runs",
        );
    }
    let n = mention_matches(m, &input, back).len();
    if n > 0 {
        let c = conn(m, key);
        c.slash_sel = c.slash_sel.min(n - 1);
        let sel = c.slash_sel;
        return mention_rows(m, &input, back, sel, w);
    }
    let mut cmds = pane_args(m, key);
    if cmds.is_empty() {
        if slash_word(&input, pos).is_none() {
            return Vec::new();
        }
        load_local(m, key);
        cmds = matches(m, key);
    }
    if cmds.is_empty() {
        return Vec::new();
    }
    let st = slash_word(&input, pos).map_or(0, |(s, _, _)| s);
    let c = conn(m, key);
    c.slash_sel = c.slash_sel.min(cmds.len() - 1);
    let skills = c.skills.clone();
    let how = if st > 0 {
        "↑↓ choose · tab or enter completes"
    } else {
        "↑↓ choose · tab completes · enter runs"
    };
    picker_rows(&cmds, c.slash_sel, w, "/", |n| tag(n, st, &skills), how)
}

/// Whether the Session's box has the keys: no box over it, no row picked.
fn typing(m: &Model) -> bool {
    m.confirm.is_none()
        && m.sheet.is_none()
        && !m.on_place()
        && m.session.as_ref().is_some_and(|v| v.sel.is_empty())
}

/// The picker's keys while a command is typed in the open session's box
/// (slashKey): ↑↓ choose, tab completes, enter runs what's typed as the
/// whole message, or completes it mid-message.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    if !matches!(s, "up" | "down" | "tab" | "enter") || !typing(m) {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if crate::screens::session_cards::conn(m, &key).card_focus {
        return Out::Pass;
    }
    let o = hash_key(m, &key, s);
    if o.used() {
        return o;
    }
    let args = pane_args(m, &key);
    if !args.is_empty() {
        let c = conn(m, &key);
        c.slash_sel = c.slash_sel.min(args.len() - 1);
        match s {
            "up" => c.slash_sel = round_move(c.slash_sel, -1, args.len()),
            "down" => c.slash_sel = round_move(c.slash_sel, 1, args.len()),
            "tab" => c.input.set(&format!("/{}", args[c.slash_sel].name)),
            _ => {
                c.input.set(&format!("/{}", args[c.slash_sel].name));
                return keys::send(m, &key, false);
            }
        }
        return Out::Draw;
    }
    let cmds = matches(m, &key);
    if cmds.is_empty() {
        return Out::Pass;
    }
    let (input, pos) = typed(m, &key);
    let Some((start, end, _)) = slash_word(&input, pos) else {
        return Out::Pass;
    };
    let c = conn(m, &key);
    c.slash_sel = c.slash_sel.min(cmds.len() - 1);
    let pick = cmds[c.slash_sel].name.clone();
    match s {
        "up" | "down" => {
            let d = if s == "up" { -1 } else { 1 };
            c.slash_sel = round_move(c.slash_sel, d, cmds.len());
            return Out::Draw;
        }
        _ => {}
    }
    let complete = |m: &mut Model, tail: &str| {
        let rest = &input[end..];
        let tail = if rest.first().is_some_and(|c| c.is_whitespace()) {
            ""
        } else {
            tail
        };
        let mut text: Vec<char> = input[..start].to_vec();
        text.extend(format!("/{pick}{tail}").chars());
        let at = text.len();
        text.extend_from_slice(rest);
        let c = conn(m, &key);
        c.input.text = text;
        c.input.pos = at;
        c.input.anchor = None;
    };
    match s {
        "tab" => complete(m, " "),
        _ => {
            // A command that is the whole message runs; one mid-message
            // is completed, and enter again sends the message.
            let rest: String = input[end..].iter().collect();
            if start == 0 && rest.trim().is_empty() {
                conn(m, &key).input.set(&format!("/{pick}"));
                return keys::send(m, &key, false);
            }
            // Typed in full: its name, without claude:, or another name
            // for it.
            let typed: String = input[start + 1..end]
                .iter()
                .collect::<String>()
                .to_lowercase();
            let name = pick.to_lowercase();
            let full = typed == name
                || format!("claude:{typed}") == name
                || alias(&typed) == Some(pick.as_str());
            if full
                && crate::screens::settings::store(m)
                    .config
                    .enter_sends_command
            {
                return Out::Pass; // typed in full, there's nothing to complete
            }
            complete(m, " ");
        }
    }
    Out::Draw
}

/// The # and @ pickers' keys in session key's box (paneHashKey): a #
/// command picked runs, unless it needs a value first; a mention is
/// completed.
fn hash_key(m: &mut Model, key: &str, s: &str) -> Out {
    let (input, pos) = typed(m, key);
    let back = input.len() - pos.min(input.len());
    let hash = hash_matches(m, &input, back, &[]);
    let (cmds, lead) = if hash.is_empty() {
        let names = mention_matches(m, &input, back);
        let cmds: Vec<Command> = names.iter().map(|n| cmd(n, "", "")).collect();
        (cmds, "@")
    } else {
        (hash, "#")
    };
    if cmds.is_empty() {
        return Out::Pass;
    }
    let c = conn(m, key);
    c.slash_sel = c.slash_sel.min(cmds.len() - 1);
    let pick = cmds[c.slash_sel].clone();
    match s {
        "up" | "down" => {
            c.slash_sel = picker_move(c.slash_sel, cmds.len(), s);
            Out::Draw
        }
        "tab" | "enter" if lead == "@" => {
            let (text, back) = complete_mention(&input, back, &pick.name);
            c.input.pos = text.len() - back;
            c.input.text = text;
            c.input.anchor = None;
            c.slash_sel = 0;
            Out::Draw
        }
        "tab" => {
            set_chars(c, completed(lead, &pick, true));
            Out::Draw
        }
        _ => {
            set_chars(c, completed(lead, &pick, false));
            if needs_arg(&pick) {
                return Out::Draw;
            }
            let text = c.input.string();
            c.input.clear();
            m.command(&text)
        }
    }
}

/// The box's text set to text, the cursor at its end and the picker on
/// its first row.
fn set_chars(c: &mut super::Conn, text: Vec<char>) {
    c.input.pos = text.len();
    c.input.text = text;
    c.input.anchor = None;
    c.slash_sel = 0;
}
