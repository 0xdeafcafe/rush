//! What rush can do with an agent, feature by feature, and how far its
//! support has been tried. After Go's internal/agent/feature.go.

use std::collections::HashMap;

/// One thing rush can do with a session or an agent. Every adapter says,
/// feature by feature, whether it can: the core asks through supports and
/// never by the agent's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    // Running a session.
    /// Runs headless, in rush mode: a Driver.
    Run,
    /// Picks a past session up again.
    Resume,
    /// Branches a session into a new one.
    Fork,
    /// Goes back to an earlier message, files and all.
    Rewind,
    /// Stops a turn and keeps the session.
    Interrupt,
    /// Takes a message mid-turn without stopping it.
    Guide,
    /// Changes model mid-session.
    Model,
    /// Sets how hard the model thinks.
    Effort,
    /// Permission modes, such as accept-edits.
    Modes,
    /// Plans before it acts.
    Plan,
    Images,
    /// Asks structured questions with choices.
    Questions,
    Subagents,
    /// Background tasks rush can list and stop.
    Background,
    /// A breakdown of what fills the context.
    Context,
    Compact,
    /// Slash commands and skills: a Commander.
    Commands,
    /// A question aside, outside the conversation.
    SideQuestion,
    /// cd and add-dir.
    Dirs,
    Mcp,
    Hooks,
    Plugins,
    /// Runs rush as its statusline.
    StatusLine,
    /// Its own TUI, shown beside rush's.
    Screen,
    /// Starts from another agent's conversation.
    HandoffIn,
    /// Takes another agent's conversation whole, as its own history.
    Port,
    /// Takes rush's instructions as its system prompt, not atop the first
    /// message.
    Prompt,
    /// Its settings files: permission rules, env, what the Settings tab
    /// sums up.
    Settings,
    /// Its own record of the account's use, by day and model.
    Stats,

    // Finding sessions.
    /// Sessions running outside rush: a Discoverer.
    Live,
    /// Past transcripts read back: a HistoryReader.
    History,
    /// Sessions on the agent's own servers.
    Remote,

    // Accounts and usage.
    /// Switches between accounts: Accounts.
    Switch,
    /// Signs in to another account: Accounts.
    SignIn,
    /// Reads its limits: a QuotaSource.
    Quota,
    /// Prices its tokens: a Pricer.
    Pricing,
    /// The files it reads as memory, and the memory report.
    Memory,
}

/// Every feature, its word, and what it's called, in the order they're
/// shown.
const ALL: [(Feature, &str, &str); 37] = [
    (Feature::Run, "run", "Run in rush mode"),
    (Feature::Resume, "resume", "Resume"),
    (Feature::Fork, "fork", "Fork"),
    (Feature::Rewind, "rewind", "Rewind"),
    (Feature::Interrupt, "interrupt", "Interrupt"),
    (Feature::Guide, "guide", "Guide mid-turn"),
    (Feature::Model, "model", "Switch model"),
    (Feature::Effort, "effort", "Effort"),
    (Feature::Modes, "modes", "Permission modes"),
    (Feature::Plan, "plan", "Plan mode"),
    (Feature::Images, "images", "Images"),
    (Feature::Questions, "questions", "Questions"),
    (Feature::Subagents, "subagents", "Subagents"),
    (Feature::Background, "background", "Background tasks"),
    (Feature::Context, "context", "Context breakdown"),
    (Feature::Compact, "compact", "Compact"),
    (Feature::Commands, "commands", "Commands and skills"),
    (Feature::SideQuestion, "btw", "Side questions (btw)"),
    (Feature::Dirs, "dirs", "cd and add-dir"),
    (Feature::Mcp, "mcp", "MCP"),
    (Feature::Hooks, "hooks", "Hooks"),
    (Feature::Plugins, "plugins", "Plugins"),
    (Feature::StatusLine, "statusline", "Statusline"),
    (Feature::Screen, "screen", "Its own screen"),
    (Feature::HandoffIn, "handoff", "Hand-off in"),
    (Feature::Port, "port", "Hand-off in whole"),
    (Feature::Prompt, "prompt", "rush's instructions"),
    (Feature::Settings, "settings", "Settings files"),
    (Feature::Stats, "stats", "Usage history"),
    (Feature::Live, "live", "Running sessions"),
    (Feature::History, "history", "Past sessions"),
    (Feature::Remote, "remote", "Remote sessions"),
    (Feature::Switch, "switch", "Account switching"),
    (Feature::SignIn, "signin", "Sign-in"),
    (Feature::Quota, "quota", "Limits"),
    (Feature::Pricing, "pricing", "Pricing"),
    (Feature::Memory, "memory", "Memory"),
];

impl Feature {
    /// Its word, as Go's Feature string: "run", "btw", "signin".
    pub fn as_str(self) -> &'static str {
        ALL[self as usize].1
    }

    /// What it's called.
    pub fn label(self) -> &'static str {
        ALL[self as usize].2
    }

    /// The feature with this word.
    pub fn from_word(w: &str) -> Option<Feature> {
        ALL.iter().find(|(_, s, _)| *s == w).map(|(f, _, _)| *f)
    }
}

impl std::fmt::Display for Feature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A feature and what it's called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureInfo {
    pub feature: Feature,
    pub label: &'static str,
}

/// Every feature, in the order they're shown.
pub fn all_features() -> Vec<FeatureInfo> {
    ALL.iter()
        .map(|&(feature, _, label)| FeatureInfo { feature, label })
        .collect()
}

/// Whether an agent has a feature (Go's agent.State).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Has {
    #[default]
    No,
    Yes,
    /// rush means to, and hasn't yet.
    Planned,
}

impl Has {
    pub fn as_str(self) -> &'static str {
        match self {
            Has::Yes => "yes",
            Has::Planned => "planned",
            Has::No => "no",
        }
    }
}

impl std::fmt::Display for Has {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How an agent has a feature, and a note to say more: "view only", "needs
/// a paid plan".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Support {
    pub is: Has,
    pub note: String,
}

impl Support {
    pub const YES: Support = Support {
        is: Has::Yes,
        note: String::new(),
    };
    pub const NO: Support = Support {
        is: Has::No,
        note: String::new(),
    };
    pub const PLANNED: Support = Support {
        is: Has::Planned,
        note: String::new(),
    };

    /// self with a note.
    pub fn with(mut self, note: impl Into<String>) -> Support {
        self.note = note.into();
        self
    }
}

/// What an agent declares it can do, feature by feature.
pub type Features = HashMap<Feature, Support>;

/// How far rush's support for an agent has been tried.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Built, not yet tried against the real program.
    #[default]
    Preview,
    /// Tried against the real program.
    Tested,
    /// Everything rush does, and used every day.
    Full,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Full => "full",
            Level::Tested => "tested",
            Level::Preview => "preview",
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_has_its_place_word_and_label() {
        for (i, &(f, w, l)) in ALL.iter().enumerate() {
            assert_eq!(f as usize, i, "{w} is out of place");
            assert_eq!(Feature::from_word(w), Some(f));
            assert_eq!(f.label(), l);
        }
        assert_eq!(all_features().len(), 37);
        assert_eq!(Support::YES.with("view only").note, "view only");
        assert_eq!(Has::Planned.to_string(), "planned");
        assert_eq!(Level::Tested.to_string(), "tested");
    }
}
