//! Which of rush's session commands and screens an agent can do, by what
//! its adapter says it supports. After Go's internal/ui/caps.go
//! (commandNeeds :10, screenNeeds :21, agentCanRun :41, canRun :48,
//! canScreen :56, ownScreens :67). The session's own (sessionAgent, the
//! commands list) are the Session's to add.

use rush_core::adapter::{Feature, Registry};

/// rush's session commands only some agents can do, and the feature each
/// needs.
pub const COMMAND_NEEDS: &[(&str, Feature)] = &[
    ("fork", Feature::Fork),
    ("rewind", Feature::Rewind),
    ("effort", Feature::Effort),
    ("plan", Feature::Plan),
    ("tasks", Feature::Background),
    ("subtask", Feature::Subagents),
    ("btw", Feature::SideQuestion),
    ("cd", Feature::Dirs),
    ("model", Feature::Model),
    ("compact", Feature::Compact),
];

/// The screens a session opens and the features each needs. A screen rush
/// doesn't draw (mcp) is the agent's own, so it needs its screen too.
pub const SCREEN_NEEDS: &[(&str, &[Feature])] = &[
    ("context", &[Feature::Context]),
    ("usage", &[Feature::Quota]),
    ("stats", &[Feature::Stats]),
    ("skills", &[Feature::Commands]),
    ("plugin", &[Feature::Plugins]),
    ("hooks", &[Feature::Hooks]),
    ("permissions", &[Feature::Settings]),
    ("memory", &[Feature::Memory]),
    ("statusline", &[Feature::StatusLine]),
    ("mcp", &[Feature::Mcp, Feature::Screen]),
];

/// Whether an agent of kind can do the command rush runs for name: what
/// can_run checks for a session, and the command bar with none open.
pub fn agent_can_run(reg: &Registry, kind: &str, name: &str) -> bool {
    COMMAND_NEEDS
        .iter()
        .find(|(n, _)| *n == name)
        .is_none_or(|&(_, f)| reg.supports(kind, f))
}

/// Whether an agent of kind can open screen.
pub fn can_screen(reg: &Registry, kind: &str, screen: &str) -> bool {
    SCREEN_NEEDS
        .iter()
        .find(|(n, _)| *n == screen)
        .is_none_or(|(_, fs)| fs.iter().all(|&f| reg.supports(kind, f)))
}

/// Whether kind can do the command for name, or open the screen it
/// names (screen: the agent's own screen name opens, as claudeScreen
/// says).
pub fn can_run(reg: &Registry, kind: &str, name: &str, screen: Option<&str>) -> bool {
    if let Some(s) = screen
        && !can_screen(reg, kind, s)
    {
        return false;
    }
    agent_can_run(reg, kind, name)
}

/// Whether kind has screens of its own, which rush shows or hands the
/// terminal to.
pub fn own_screens(reg: &Registry, kind: &str) -> bool {
    reg.supports(kind, Feature::Screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ungated_commands_run_anywhere() {
        let reg = Registry::default();
        // An agent nobody registered supports nothing gated.
        assert!(agent_can_run(&reg, "nobody", "status"));
        assert!(!agent_can_run(&reg, "nobody", "fork"));
        assert!(can_screen(&reg, "nobody", "config"));
        assert!(!can_screen(&reg, "nobody", "mcp"));
        assert!(!can_run(&reg, "nobody", "status", Some("context")));
        assert!(!own_screens(&reg, "nobody"));
    }
}
