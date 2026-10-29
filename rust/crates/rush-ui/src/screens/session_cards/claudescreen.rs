//! Claude Code's own screens (/plugin, /mcp, …), with the terminal handed
//! to a fresh Claude Code on one, in the session's folder and account;
//! leaving it (esc, then ctrl+c twice) brings rush back, and a session
//! rush hosts reloads its plugins after /plugin. After Go's
//! internal/ui/sessionviews.go:446-467 (claudeScreens, screenAliases),
//! :553 (claudeScreen), :1094-1137 (openScreen, screenDoneMsg,
//! onScreenDone) and infotabs.go:333 (the info sheet's MCP servers). The
//! command is made off the UI thread (it reads the account's settings),
//! then run through Model::exec (frame/outside.rs).

use crate::frame::Out;
use crate::model::Model;
use crate::screens::session_core;
use crate::session::harness_name;

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
