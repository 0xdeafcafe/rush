//! Claude Code's own screens (/plugin, /mcp, …) typed in a session's box:
//! rush's own take on one where it has one (rushScreen), else the
//! terminal handed to a fresh Claude Code on it, in the session's folder
//! and account; leaving it (esc, then ctrl+c twice) brings rush back, and
//! a session rush hosts reloads its plugins after /plugin. A / command
//! neither rush nor the session knows asks whether it's one of Claude
//! Code's screens; a claude: one says first that it opens a real Claude
//! Code.
//!
//! After Go's internal/ui/sessionviews.go:417-440 (rushCommands,
//! rushAliases), :446-467 (claudeScreens, screenAliases), :472-523
//! (claudeCloud, cloudAliases, claudeCloudName), :535-546
//! (offCommands), :553 (claudeScreen), :847-866 (runRushCommand's head:
//! aliases, commands left out, what the agent can't do, claude:),
//! :1001-1006 (a screen typed), :1009-1046 (cmdName, askUnknown),
//! :1050-1078 (rushScreen), :1094-1137 (openScreen, screenDoneMsg,
//! onScreenDone), rushmode.go:4710-4720 (sendPane's sendRaw),
//! infosheet.go:572-626 (unknownSheet), :628-668 (claudeSheet) and
//! infotabs.go:333 (the info sheet's MCP servers). Commands are made and
//! the disk read off the UI thread; programs run through Model::exec
//! (frame/outside.rs).

use std::rc::Rc;

use rush_core::adapter::{AdapterRef, Command, Kind, Profile};
use rush_core::text::Line;

use crate::frame::caps::{can_run, can_screen, own_screens};
use crate::frame::sheet::{Sheet, sheet_row, sheet_title};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::accounts::info as infosheet;
use crate::screens::session_core;
use crate::screens::settings::{sheets, statusline};
use crate::session::harness_name;
use crate::style::{CHROME_BG, FAINT, ORANGE, SUB, TEXT, keys_fit, line, plain, sp, spb, wrap};

/// Claude Code's screens rush can open (claudeScreens' names).
pub const CLAUDE_SCREENS: &[&str] = &[
    "status",
    "context",
    "usage",
    "stats",
    "skills",
    "plugin",
    "mcp",
    "hooks",
    "permissions",
    "memory",
    "config",
    "statusline",
];

/// rush's own session commands, by name (rushCommands): one of them is
/// never asked about.
pub const RUSH_COMMANDS: &[&str] = &[
    "clear",
    "fork",
    "rewind",
    "model",
    "effort",
    "plan",
    "diff",
    "tasks",
    "copy",
    "rename",
    "cd",
    "stop",
    "background",
    "resume",
    "help",
    "btw",
    "export",
    "subtask",
];

/// Claude Code's other names for commands rush does (rushAliases).
pub const RUSH_ALIASES: &[(&str, &str)] = &[
    ("bashes", "tasks"),
    ("bg", "background"),
    ("continue", "resume"),
    ("name", "rename"),
    ("branch", "fork"),
    ("checkpoint", "rewind"),
    ("undo", "rewind"),
    ("reset", "clear"),
    ("new", "clear"),
];

/// Claude Code's screens for your account and Claude's cloud, which rush
/// leaves to Claude Code: named claude:<name> in the picker, they open a
/// real Claude Code after saying so (claudeCloud).
pub const CLAUDE_CLOUD: &[(&str, &str)] = &[
    ("login", "sign in to an Anthropic account"),
    ("logout", "sign out"),
    ("upgrade", "upgrade the plan for higher limits"),
    (
        "usage-credits",
        "usage credits past the limits, or ask your admin for them",
    ),
    ("rate-limit-options", "what to do when a limit is reached"),
    ("privacy-settings", "privacy settings"),
    ("setup-bedrock", "Amazon Bedrock sign-in, region and models"),
    (
        "setup-vertex",
        "Google Vertex AI sign-in, project, region and models",
    ),
    (
        "teleport",
        "send a session to the cloud, or bring one back from claude.ai",
    ),
    (
        "remote-control",
        "drive a session from claude.ai or the app",
    ),
    ("session", "a cloud session's URL and QR code"),
    ("remote-env", "the default environment for cloud agents"),
    ("web-setup", "cloud sessions with your GitHub account"),
    (
        "cloud-plugins",
        "whether cloud sessions use this machine's plugins",
    ),
    ("desktop", "carry on in Claude Desktop"),
    ("mobile", "a QR code for the Claude app"),
    ("ultraplan", "plan in the cloud"),
    ("ultrareview", "a deep multi-agent review in the cloud"),
    ("autofix-pr", "watch a pull request and fix what fails"),
    ("install-github-app", "Claude on GitHub Actions for a repo"),
    ("install-slack-app", "the Claude Slack app"),
    ("design-login", "design-system access for /design-sync"),
    ("design-consent", "let Claude reach your Design projects"),
    ("design-revoke", "take that back"),
    ("artifacts", "browse your published and shared artifacts"),
    ("workflows", "browse running and completed workflows"),
    ("daemon", "Claude Code's background services and routines"),
];

/// Claude Code's other names for claudeCloud's (cloudAliases).
const CLOUD_ALIASES: &[(&str, &str)] = &[
    ("extra-usage", "usage-credits"),
    ("tp", "teleport"),
    ("rc", "remote-control"),
    ("remote", "session"),
    ("app", "desktop"),
    ("ios", "mobile"),
    ("android", "mobile"),
];

/// The claudeCloud command name is, by any of its names (login,
/// claude:login, tp) (claudeCloudName).
pub fn claude_cloud_name(name: &str) -> Option<&'static str> {
    let name = name.strip_prefix("claude:").unwrap_or(name);
    let name = CLOUD_ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, n)| n);
    CLAUDE_CLOUD.iter().map(|(n, _)| *n).find(|n| *n == name)
}

/// Why one of Claude Code's commands does nothing in rush: its own
/// terminal's, the odds and ends, and the ones rush hasn't done yet
/// (offCommands). None for the rest.
pub fn off_command(name: &str) -> Option<&'static str> {
    const TERMINAL: &[&str] = &[
        "theme",
        "color",
        "tui",
        "scroll-speed",
        "focus",
        "brief",
        "terminal-setup",
        "voice",
        "keybindings",
        "ide",
        "chrome",
        "exit",
        "quit",
        "vim",
    ];
    const LEFT_OUT: &[&str] = &[
        "release-notes",
        "feedback",
        "bug",
        "share",
        "update",
        "install",
        "import",
        "powerup",
        "wellbeing",
        "breaks",
        "break-reminder",
        "downtime",
        "stickers",
        "radio",
        "heapdump",
        "passes",
        "agents",
        "plugin-types",
        "workflow-launch-exec",
    ];
    if TERMINAL.contains(&name) {
        return Some("it's for Claude Code's own terminal");
    }
    if LEFT_OUT.contains(&name) {
        return Some("rush leaves it out");
    }
    (name == "loops").then_some("Claude Code has it switched off; ask Claude to /loop instead")
}

/// Other names Claude Code takes for the same screens (screenAliases).
const ALIASES: &[(&str, &str)] = &[
    ("plugins", "plugin"),
    ("marketplace", "plugin"),
    ("settings", "config"),
    ("cost", "usage"),
    ("allowed-tools", "permissions"),
    ("version", "status"),
];

/// The screen a / command names, through its aliases, if it's one of
/// Claude Code's (claudeScreen).
pub fn claude_screen(name: &str) -> Option<&'static str> {
    let name = ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, n)| n);
    CLAUDE_SCREENS.iter().copied().find(|s| *s == name)
}

/// Hands the terminal to the agent's program on one of its own screens,
/// for session key's account and folder (openScreen).
pub fn open_screen(m: &mut Model, key: &str, screen: &str) -> Out {
    let Some(a) = session_core::agent(m, key).cloned() else {
        return Out::Idle;
    };
    let harness = harness_name(&m.reg, &a.kind);
    let Some(ad) = m
        .reg
        .get(a.kind.as_str())
        .filter(|ad| ad.screener().is_some())
    else {
        m.flash_err(&format!("{harness} has no screens of its own"));
        return Out::Draw;
    };
    let hint = format!(
        "\x1b[2m  rush · {harness}'s /{screen} · when you're done: esc, then ctrl+c twice to come back\x1b[0m"
    );
    let cwd = a.job.cwd.clone();
    let acct = if a.acct.name.is_empty() && a.acct.dir.is_empty() {
        crate::screens::cmdbar::outside::profile_of(&m.reg, &a)
    } else {
        a.acct.clone()
    };
    let (key, screen) = (key.to_string(), screen.to_string());
    m.later(
        {
            let screen = screen.clone();
            // It reads the account's settings: not on the UI thread.
            move || {
                ad.screener()
                    .expect("filtered")
                    .screen(&acct, &cwd, &screen, &hint)
            }
        },
        move |m, cmd| {
            m.exec(cmd, Box::new(move |m, r| screen_done(m, &key, &screen, r)));
            Out::Draw
        },
    );
    Out::Draw
}

/// Rush coming back from one of Claude Code's screens: after /plugin, a
/// session rush hosts that isn't mid-turn reloads its plugins
/// (onScreenDone).
pub fn screen_done(m: &mut Model, key: &str, screen: &str, r: crate::frame::outside::Ran) -> Out {
    if let Err(e) = r {
        m.fail(&format!("Claude Code's /{screen}: {e}"));
        return Out::Draw;
    }
    if screen != "plugin"
        || session_core::open_key(m).as_deref() != Some(key)
        || !session_core::conn(m, key).hosted()
    {
        return Out::Draw;
    }
    // A running session only sees newly enabled plugins once it reloads
    // them; mid-turn, that's left to you.
    if session_core::conn(m, key).info.state == "working" {
        let name = session_core::agent(m, key)
            .map(|a| a.display_name.clone())
            .unwrap_or_default();
        m.flash(&format!(
            "plugins change for {name} after /reload-plugins, once this turn ends"
        ));
        return Out::Draw;
    }
    session_core::keys::send_as(m, key, "/reload-plugins");
    Out::Draw
}

/// The kind of agent session key runs: its host's word, else the list's
/// (sessionAgent).
fn kind_of(m: &mut Model, key: &str) -> String {
    let k = session_core::conn(m, key).info.began.kind.clone();
    if !k.is_empty() {
        return k;
    }
    session_core::agent(m, key)
        .map(|a| a.kind.as_str().to_string())
        .unwrap_or_default()
}

/// The profile an agent runs on (a.Acct), else the one its agent knows
/// by its account's name.
fn acct_of(m: &Model, a: &rush_core::agent::Agent) -> Profile {
    if a.acct.name.is_empty() && a.acct.dir.is_empty() {
        crate::screens::cmdbar::outside::profile_of(&m.reg, a)
    } else {
        a.acct.clone()
    }
}

/// The head of runRushCommand, for a / command typed in session key's
/// box: Claude Code's other name for one of rush's is taken as rush's
/// (rushAliases), one rush leaves out or the agent can't do says so, and
/// a claude: one asks first. Used when it did; the name, as rush's, goes
/// on to rush's other commands, then [`screen_command`].
pub fn command(m: &mut Model, key: &str, name: &str, arg: &str) -> (Out, String) {
    let name = RUSH_ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, n)| n)
        .to_string();
    if let Some(why) = off_command(&name) {
        m.flash_err(&format!("/{name} isn't in rush: {why}"));
        return (Out::Draw, name);
    }
    let kind = kind_of(m, key);
    let cloud = claude_cloud_name(&name);
    if !can_run(&m.reg, &kind, &name, claude_screen(&name))
        || cloud.is_some() && !own_screens(&m.reg, &kind)
    {
        let h = harness_name(&m.reg, &Kind::new(&kind));
        m.flash_err(&format!("/{name} isn't something {h} can do"));
        return (Out::Draw, name);
    }
    if let Some(cloud) = cloud
        && session_core::agent(m, key).is_some()
    {
        m.open_sheet(Box::new(ClaudeSheet {
            conn: key.to_string(),
            line: format!("{cloud} {arg}").trim().to_string(),
        }));
        return (Out::Draw, name);
    }
    (Out::Pass, name)
}

/// One of Claude Code's screens typed in session key's box
/// (sessionviews.go:1001): rush's own take on it, else Claude Code's.
/// Given arguments, /mcp and /config go to the agent as usual. Pass when
/// it isn't one.
pub fn screen_command(m: &mut Model, key: &str, name: &str, arg: &str) -> Out {
    let Some(screen) = claude_screen(name) else {
        return Out::Pass;
    };
    if !arg.is_empty() && (screen == "mcp" || screen == "config")
        || session_core::agent(m, key).is_none()
    {
        return Out::Pass;
    }
    match rush_screen(m, key, screen) {
        Out::Pass => open_screen(m, key, screen),
        out => out,
    }
}

/// rush's own take on one of Claude Code's screens for session key,
/// where it has one; Pass for the rest, which open Claude Code's
/// (rushScreen).
pub fn rush_screen(m: &mut Model, key: &str, screen: &str) -> Out {
    let Some(a) = session_core::agent(m, key).cloned() else {
        return Out::Pass;
    };
    let kind = kind_of(m, key);
    let cwd = a.job.cwd.clone();
    let p = acct_of(m, &a);
    let tab = match screen {
        "status" => infosheet::STATUS,
        "context" => infosheet::CONTEXT,
        "usage" => infosheet::USAGE,
        "stats" => infosheet::HISTORY,
        "plugin" => {
            if !can_screen(&m.reg, &kind, "plugin") {
                let h = harness_name(&m.reg, &Kind::new(&kind));
                m.flash_err(&format!("{h} has no plugins rush manages"));
                return Out::Draw;
            }
            crate::screens::plugin_ui::sheet::open(m, &a, &cwd);
            return Out::Draw;
        }
        "memory" if session_core::views::show_view(m, key, "memory") => return Out::Draw,
        "config" => {
            crate::screens::settings::providers::open_agent_settings(m, &kind);
            return Out::Draw;
        }
        "statusline" => {
            let model = m
                .session
                .as_ref()
                .filter(|v| v.key == key)
                .map(|v| v.sess.model.clone())
                .unwrap_or_default();
            let input = statusline::preview_input(&a.job.session_id, &cwd, &model, a.spend.cost);
            statusline::open_status_line(m, &kind, p, key, input);
            return Out::Draw;
        }
        "skills" => {
            open_skills(m, key, &kind, p, cwd);
            return Out::Draw;
        }
        "permissions" => {
            sheets::open_permissions(m, &kind, &p, &cwd);
            return Out::Draw;
        }
        "hooks" => {
            sheets::open_hooks(m, &kind, &p, &cwd);
            return Out::Draw;
        }
        _ => return Out::Pass,
    };
    infosheet::open_info(m, tab)
}

/// /skills: the session's skills and commands, read off the UI thread,
/// then the sheet; using one puts "/name " in the session's box
/// (openSkills, sessionCommands).
fn open_skills(m: &mut Model, key: &str, kind: &str, p: Profile, cwd: String) {
    let ad = m.reg.get(kind).filter(|a| a.commander().is_some());
    let Some(ad) = ad.filter(|_| can_screen(&m.reg, kind, "skills")) else {
        let h = harness_name(&m.reg, &Kind::new(kind));
        m.flash_err(&format!("{h} has no skills or commands rush lists"));
        return;
    };
    let reg = m.reg.clone();
    let (key, kind) = (key.to_string(), kind.to_string());
    m.later(
        move || local_commands(&reg, &ad, p, &cwd),
        move |m, all| {
            let use_it: sheets::UseIt = Rc::new(move |m, name| {
                session_core::conn(m, &key).input.set(&format!("/{name} "));
            });
            sheets::open_skills(m, &kind, all, use_it);
            Out::Draw
        },
    );
}

/// The commands and skills on disk for an agent's account and folder:
/// its first profile's when the account names no folder
/// (sessionCommands). It reads the disk.
fn local_commands(
    reg: &rush_core::adapter::Registry,
    ad: &AdapterRef,
    mut p: Profile,
    cwd: &str,
) -> Vec<Command> {
    let Some(cm) = ad.commander() else {
        return Vec::new();
    };
    if p.dir.is_empty()
        && let Some(first) = reg.profiles_of(ad).into_iter().next()
    {
        p = first;
    }
    cm.commands(&p, cwd)
}

/// A / command's name as Claude Code takes it: not a path (cmdName).
fn cmd_name(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && cs.all(|c| c.is_ascii_alphanumeric() || "_:.-".contains(c))
}

/// What's kept while a / command is asked about.
#[derive(Default)]
pub struct Screens {
    /// The box's text is being sent as typed, not asked about
    /// (c.sendRaw).
    pub send_raw: bool,
}

/// Asks what to do with the / command text typed in session key's box
/// that neither rush nor the session knows, which its agent would only
/// answer with "isn't available in this environment": most likely it's
/// one of Claude Code's own screens. Open it in Claude Code, or send it
/// as typed (now as it was sent). False when it isn't such a command, or
/// while it's being sent as typed (askUnknown). The commands on disk are
/// read off the UI thread first: one of those is sent as typed at once.
pub fn ask_unknown(m: &mut Model, key: &str, text: &str, now: bool) -> bool {
    if m.kept::<Screens>().send_raw {
        return false;
    }
    let Some(a) = session_core::agent(m, key).cloned() else {
        return false;
    };
    // Claude Code sessions are typed into, and know their own commands;
    // until the session has said which it has, there's nothing to go by.
    // An agent with no screens of its own has none to open.
    let said: Vec<String> = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .map(|v| v.sess.commands.clone())
        .unwrap_or_default();
    let kind = kind_of(m, key);
    if !session_core::conn(m, key).hosted() || said.is_empty() || !own_screens(&m.reg, &kind) {
        return false;
    }
    let t = text.trim();
    let line = t.strip_prefix('/').unwrap_or(t).trim().to_string();
    let name = line.split(' ').next().unwrap_or("").to_string();
    if line.contains('\n') || !cmd_name(&name) {
        return false;
    }
    let aliased = |l: &[(&str, &str)]| l.iter().any(|(a, _)| *a == name);
    if claude_cloud_name(&name).is_some()
        || off_command(&name).is_some()
        || aliased(RUSH_ALIASES)
        || aliased(ALIASES)
    {
        return false;
    }
    let known = |n: &str| n.eq_ignore_ascii_case(&name);
    if RUSH_COMMANDS.iter().any(|n| known(n))
        || CLAUDE_SCREENS.iter().any(|n| known(n))
        || said.iter().any(|n| known(n))
    {
        return false;
    }
    let Some(ad) = m.reg.get(&kind).filter(|ad| ad.commander().is_some()) else {
        return open_unknown(m, key, line, now);
    };
    let (reg, p, cwd) = (m.reg.clone(), acct_of(m, &a), a.job.cwd.clone());
    let key = key.to_string();
    m.later(
        move || local_commands(&reg, &ad, p, &cwd),
        move |m, local| {
            if local.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
                // One of the agent's own on disk: it goes as typed.
                return send_raw(m, &key, now);
            }
            open_unknown(m, &key, line, now);
            Out::Draw
        },
    );
    true
}

/// Opens the sheet asking what to do with line.
fn open_unknown(m: &mut Model, key: &str, line: String, now: bool) -> bool {
    m.open_sheet(Box::new(UnknownSheet {
        conn: key.to_string(),
        line,
        now,
        cur: 0,
    }));
    true
}

/// Sends what's in session key's box as typed, asking nothing about it
/// (sendRaw).
fn send_raw(m: &mut Model, key: &str, now: bool) -> Out {
    m.kept::<Screens>().send_raw = true;
    let out = session_core::keys::send(m, key, now);
    m.kept::<Screens>().send_raw = false;
    out
}

/// What a sheet's enter does once it's done: a Quit goes on, anything
/// else closes it.
fn closed(out: Out) -> Out {
    if out == Out::Quit { out } else { Out::Close }
}

/// Asks what to do with a / command rush doesn't know (askUnknown): open
/// Claude Code on it, or send it to Claude after all (unknownSheet).
pub struct UnknownSheet {
    pub conn: String,
    pub line: String,
    /// It was sent to go now, stopping the turn.
    pub now: bool,
    /// 0 opens it in Claude Code, 1 sends it as a message.
    pub cur: usize,
}

impl Sheet for UnknownSheet {
    fn width(&self, _: &Model) -> usize {
        72
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" => Out::Close,
            "up" | "down" | "tab" | "shift+tab" => {
                self.cur = 1 - self.cur;
                Out::Draw
            }
            "enter" => {
                if session_core::agent(m, &self.conn).is_none() {
                    return Out::Close;
                }
                if self.cur == 1 {
                    return closed(send_raw(m, &self.conn, self.now));
                }
                session_core::conn(m, &self.conn).input.clear();
                closed(open_screen(m, &self.conn, &self.line))
            }
            _ => Out::Idle,
        }
    }

    fn body(&mut self, _: &Model, w: usize, _: usize) -> Vec<Line> {
        let name = self.line.split(' ').next().unwrap_or("");
        let mut out = vec![
            sheet_title(&format!("/{name}"), "isn't a command rush knows", w),
            Line::new(),
        ];
        let why = "rush has no view of its own for it, and this session didn't list it among its commands, so it's most likely one of Claude Code's own screens.";
        for l in wrap(why, w.saturating_sub(4)) {
            out.push(line([plain("  "), sp(l, SUB)]));
        }
        out.push(Line::new());
        let choices = [
            (
                "Open it in Claude Code",
                "in this agent's folder and account · esc, then ctrl+c twice, comes back",
            ),
            ("Send it to Claude as a message", "as you typed it"),
        ];
        for (i, (name, about)) in choices.iter().enumerate() {
            out.push(sheet_row(line([spb(*name, TEXT)]), i == self.cur, w));
            let about = rush_draw::truncate(about, w.saturating_sub(6), "…").into_owned();
            out.push(line([plain("    "), sp(about, FAINT)]));
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[("↑↓", "choose"), ("enter", "go"), ("esc", "cancel")],
        ));
        out
    }
}

/// Says, before it does, that a claude: command opens a real Claude Code:
/// a fresh one, in this agent's folder and on its account, not this
/// conversation (claudeSheet).
pub struct ClaudeSheet {
    pub conn: String,
    pub line: String,
}

impl Sheet for ClaudeSheet {
    fn width(&self, _: &Model) -> usize {
        72
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" => Out::Close,
            "enter" => {
                if session_core::agent(m, &self.conn).is_none() {
                    return Out::Close;
                }
                session_core::conn(m, &self.conn).input.clear();
                closed(open_screen(m, &self.conn, &self.line))
            }
            _ => Out::Idle,
        }
    }

    fn body(&mut self, _: &Model, w: usize, _: usize) -> Vec<Line> {
        let name = self.line.split(' ').next().unwrap_or("");
        let about = CLAUDE_CLOUD
            .iter()
            .find(|(n, _)| *n == name)
            .map_or("", |(_, d)| d);
        let mut out = vec![
            sheet_title(&format!("claude:{name}"), about, w),
            Line::new(),
        ];
        let banner = line([
            plain("  "),
            sp("↗ ", ORANGE),
            spb("This opens a real Claude Code", TEXT),
        ]);
        out.push(super::card::on_bg(banner, CHROME_BG, w));
        out.push(Line::new());
        let say = format!(
            "/{name} is Claude Code's own, for your account or Claude's cloud, and rush leaves it to Claude Code. rush hands the terminal to a fresh Claude Code in this agent's folder, on its account, and opens /{} there. It isn't this conversation.",
            self.line
        );
        for l in wrap(&say, w.saturating_sub(4)) {
            out.push(line([plain("  "), sp(l, SUB)]));
        }
        out.push(Line::new());
        let back = "When you're done: esc, then ctrl+c twice, and you're back here.";
        for l in wrap(back, w.saturating_sub(4)) {
            out.push(line([plain("  "), sp(l, SUB)]));
        }
        out.push(Line::new());
        out.push(keys_fit(
            w,
            &[("enter", "open Claude Code"), ("esc", "cancel")],
        ));
        out
    }
}
