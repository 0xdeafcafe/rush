//! config.json: rush's settings, in the shape Go rush writes them. Fields
//! keep Go's order and names, and are left out exactly where Go's
//! omitempty and omitzero leave them out.
//!
//! Ported from internal/state/state.go:67-612 (Config, SignIn, Onboarding,
//! Dispatch, Start, Overlay's neighbours), :961-980 (Shows, StackPercent,
//! Everywhere), dispatch.go (OwnAgent), routes.go (StartOn, SetStartOn),
//! apikeys.go:42-57 (MarkAPIKey, HasAPIKey), vault.go:66-103 (Login,
//! Folder, home).

use crate::agents;
use crate::json::{RawJson, is_default, is_false, is_zero_f64, is_zero_i64, lenient};
use crate::profiles::{FolderRule, LOGINS_KIND, Profile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// rush's settings: config.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The feed agents share: #feed on|off, default off. Its old name,
    /// kept so the setting survives.
    #[serde(
        rename = "twatter",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub feed: bool,
    /// How little free disk, in GB, has rush offer to start a cleanup
    /// agent: 0 is 20, -1 never. CleanupRAMMinutes is how long the memory
    /// may be under pressure before it offers: 0 is 15, -1 never.
    #[serde(
        rename = "cleanupDiskGB",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_i64"
    )]
    pub cleanup_disk_gb: i64,
    #[serde(
        rename = "cleanupRAMMinutes",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_i64"
    )]
    pub cleanup_ram_minutes: i64,
    /// Hides the conversation overview rail; narrow panes hide it automatically.
    #[serde(
        rename = "hideMinimap",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub hide_minimap: bool,
    /// The agents pinned as tiles beside the Session, by key, and GridBelow
    /// whether they sit under it instead.
    #[serde(
        rename = "grid",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub grid: Vec<String>,
    /// Old configs: every tile under.
    #[serde(
        rename = "gridBelow",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub grid_below: bool,
    /// The tiles in the row under the Session; the rest are beside it.
    #[serde(
        rename = "gridBelowKeys",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub grid_below_keys: Vec<String>,
    /// How much of the pane the tiles take.
    #[serde(
        rename = "gridShare",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_f64"
    )]
    pub grid_share: f64,
    /// Draws pictures in coloured blocks even where the terminal could
    /// show them sharp.
    #[serde(
        rename = "pixelPictures",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub pixel_pictures: bool,
    /// The user's explicit #compact choice. Empty keeps native harness
    /// compaction; never an automatic override.
    #[serde(
        rename = "compactKind",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub compact_kind: String,
    #[serde(
        rename = "compactModel",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub compact_model: String,
    /// The Ollama decision model #compact prunes with when it's the
    /// default (CompactKind "ollama-prune"). Experimental.
    #[serde(
        rename = "pruneModel",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub prune_model: String,
    /// The providers whose API key rush keeps in the vault.
    #[serde(
        rename = "apiKeys",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub api_keys: Vec<String>,
    /// The Claude config folders an older rush was given: ~/.claude and
    /// ~/.claude-*. Everything is in ~/.claude now; the others are only
    /// read to take their sign-ins and past sessions in, once, and dropped
    /// after. ~/.claude's entry stays: its name is what its sessions, names
    /// and hosted agents are kept under.
    #[serde(
        rename = "accounts",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub folders: Vec<Folder>,
    /// Set once the older folders' sign-ins were taken in as logins.
    #[serde(
        rename = "foldersImported",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub folders_imported: bool,
    /// Named the folder new sessions started in, before accounts became
    /// logins; cleared once logins are found.
    #[serde(
        rename = "active",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub active: String,
    /// The Claude accounts ~/.claude can be signed in as; their sign-ins
    /// are in the vault, not here.
    #[serde(
        rename = "logins",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub logins: Vec<Login>,
    /// The accounts of agents other than Claude Code that rush keeps,
    /// several to an agent; their credentials are elsewhere.
    #[serde(
        rename = "signIns",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub sign_ins: Vec<SignIn>,
    /// The account an agent runs on where rush picks it: a SignIn's ID, by kind.
    #[serde(
        rename = "using",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub using: BTreeMap<String, String>,
    /// The profiles you made, and the folders that picked one, from before
    /// Settings › Config replaced them: read once, to move the default one
    /// in (move_profiles_in), then kept only for older rushes.
    #[serde(
        rename = "profiles",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub old_profiles: Vec<Profile>,
    /// The profile new sessions get.
    #[serde(
        rename = "defaultProfile",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub default_profile: String,
    #[serde(
        rename = "folderRules",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub old_folder_rules: Vec<FolderRule>,
    /// Set once OldProfiles' default was moved in.
    #[serde(
        rename = "profilesMovedIn",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub profiles_moved_in: bool,
    /// The harnesses switched off in Settings › Config: rush starts no
    /// session in them.
    #[serde(
        rename = "harnessesOff",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub harnesses_off: Vec<String>,
    /// The models a provider may call in a harness, by CellKey; a cell not
    /// in it may call all of them.
    #[serde(
        rename = "cellModels",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub cell_models: BTreeMap<String, Vec<String>>,
    /// What rush does when the account in use runs out: "" moves to
    /// another account of the provider, "wait" waits for the reset, "next"
    /// moves on to the next provider with room.
    #[serde(
        rename = "atLimit",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub at_limit: String,
    /// The harness each provider runs in, by provider, where it isn't its
    /// usual one: "ollama": "pi" runs Ollama in Pi.
    #[serde(
        rename = "runsIn",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub runs_in: BTreeMap<String, String>,
    /// The harnesses you use each provider in besides its default.
    #[serde(
        rename = "harnesses",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub harnesses: BTreeMap<String, Vec<String>>,
    /// Set once an older config's profiles were fitted to built-in ones.
    #[serde(
        rename = "builtinProfiles",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub builtin_profiles: bool,
    /// What rush does when the account in use is nearly out: "" (or
    /// "account") switches to another account of the same agent; "agent"
    /// does that, then starts new sessions on the next agent in AgentOrder
    /// once every account of it is out; "off" stays.
    #[serde(
        rename = "switchOnLimit",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub switch_on_limit: String,
    /// The agents new sessions move on to, in turn, when SwitchOnLimit is
    /// "agent". Installed agents not in it come after, by name.
    #[serde(
        rename = "agentOrder",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub agent_order: Vec<String>,
    /// SwitchOnLimit "off" as rushes before it read it: kept in step with it.
    #[serde(
        rename = "stayOnAccount",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub stay_on_account: bool,
    #[serde(rename = "groupBy", deserialize_with = "lenient")]
    pub group_by: String,
    /// Splits each section of the list further: "project" (and empty) by
    /// the repository agents work in, "none" not at all.
    #[serde(
        rename = "splitBy",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub split_by: String,
    /// How the Agents list is drawn: "rail" (and empty) or "classic".
    #[serde(
        rename = "listLook",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub list_look: String,
    #[serde(
        rename = "folds",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub folds: BTreeMap<String, bool>,
    #[serde(rename = "dispatch", deserialize_with = "lenient")]
    pub dispatch: Dispatch,
    #[serde(
        rename = "quiet",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub quiet: bool,
    #[serde(
        rename = "dockLines",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_i64"
    )]
    pub dock_lines: i64,
    /// The agent list's share of a split screen, 0.25 to 0.5; zero means
    /// rush's own choice.
    #[serde(
        rename = "sideWidth",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_f64"
    )]
    pub side_width: f64,
    /// The layout you picked: "split", "agent" or "list". Empty asks, the
    /// first time rush opens.
    #[serde(
        rename = "view",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub view: String,
    /// Keeps a wide screen to the list alone.
    #[serde(
        rename = "listOnly",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub list_only: bool,
    /// How a Session opens from Agents alone: the whole screen rather than
    /// beside the list.
    #[serde(
        rename = "chatFull",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub chat_full: bool,
    /// What enter does on an agent in the list: "rename" or "open". Empty
    /// asks, the first time.
    #[serde(
        rename = "enterOn",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub enter_on: String,
    #[serde(
        rename = "sortBy",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub sort_by: String,
    /// The share of the screen, in percent, at or under which the list's
    /// rows take two lines; see stack_percent.
    #[serde(
        rename = "stackAt",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_i64"
    )]
    pub stack_at: i64,
    /// The agents list's figures you've turned off.
    #[serde(
        rename = "hideColumns",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub hide_columns: Vec<String>,
    #[serde(rename = "hibernate", deserialize_with = "lenient")]
    pub hibernate: Hibernate,
    /// What rush sets in every agent's own settings, each in its own words.
    #[serde(
        rename = "everywhere",
        deserialize_with = "lenient",
        skip_serializing_if = "is_default"
    )]
    pub everywhere: Everywhere,
    /// How long a stopped agent stays in the list's Active section; 0 is
    /// the default (5), negative off.
    #[serde(
        rename = "activeMinutes",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_i64"
    )]
    pub active_minutes: i64,
    /// Bound the list's Just left: 0 is the default (2 hours, 10),
    /// negative minutes off.
    #[serde(
        rename = "justLeftMinutes",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_i64"
    )]
    pub just_left_minutes: i64,
    #[serde(
        rename = "justLeftCount",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_i64"
    )]
    pub just_left_count: i64,
    /// How long an agent must have been done and untouched before its
    /// worktree and temp work are removed; 0 is the default, negative off.
    #[serde(
        rename = "cleanupHours",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero_i64"
    )]
    pub cleanup_hours: i64,
    /// By project, whether a session started from the box gets a git
    /// worktree of its own. A project not in it does.
    #[serde(
        rename = "ownWorktrees",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub own_worktrees: BTreeMap<String, bool>,
    #[serde(
        rename = "keepOrphans",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub keep_orphans: bool,
    #[serde(
        rename = "restartOnInstall",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub restart_on_install: bool,
    #[serde(
        rename = "trimRunningTmp",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub trim_running_tmp: bool,
    #[serde(
        rename = "keepTranscriptsPlain",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub keep_transcripts_plain: bool,
    /// How see-through an agent's second line in the list is.
    #[serde(
        rename = "subLine",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub sub_line: String,
    #[serde(
        rename = "hideLogo",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub hide_logo: bool,
    #[serde(
        rename = "colorBlind",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub color_blind: bool,
    /// "dark" or "light", or, empty, whatever the terminal says.
    #[serde(
        rename = "theme",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub theme: String,
    #[serde(
        rename = "showWhitespace",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub show_whitespace: bool,
    /// Whether text dragged over goes to the clipboard as the drag ends.
    /// Unset is on.
    #[serde(
        rename = "copyOnSelect",
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub copy_on_select: Option<bool>,
    #[serde(
        rename = "searchTranscriptsOnKey",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub search_transcripts_on_key: bool,
    #[serde(
        rename = "enterSendsCommand",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub enter_sends_command: bool,
    /// The key that opens or closes a picked conversation row: "enter",
    /// "space", or empty for either.
    #[serde(
        rename = "rowKey",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub row_key: String,
    #[serde(
        rename = "menuBar",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub menu_bar: bool,
    #[serde(
        rename = "menuBarAsked",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub menu_bar_asked: bool,
    /// How far a new user has got.
    #[serde(rename = "onboarding", deserialize_with = "lenient")]
    pub onboarding: Onboarding,
    /// Lets rush's advisor look over your agents' figures now and then.
    #[serde(
        rename = "advisor",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub advisor: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hibernate {
    #[serde(rename = "afterMinutes", deserialize_with = "lenient")]
    pub after_minutes: i64,
}

/// What rush keeps set in every agent that has it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Everywhere {
    /// Whether agents credit themselves in the commits and pull requests
    /// they make: "on", "off", or "" to leave each as it's set.
    #[serde(
        rename = "attribution",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub attribution: String,
}

/// An account of an agent other than Claude Code: who it is, and what
/// rush calls it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SignIn {
    #[serde(rename = "kind", deserialize_with = "lenient")]
    pub kind: String,
    /// The agent's own id for the account.
    #[serde(rename = "id", deserialize_with = "lenient")]
    pub id: String,
    #[serde(rename = "name", deserialize_with = "lenient")]
    pub name: String,
    #[serde(
        rename = "email",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub email: String,
    #[serde(
        rename = "plan",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub plan: String,
}

/// What rush has taught you so far.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Onboarding {
    #[serde(
        rename = "steps",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub steps: Vec<String>,
    /// The optional bundled plugins rush has asked you about: each once.
    #[serde(
        rename = "plugins",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub plugins: Vec<String>,
    #[serde(
        rename = "tips",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tips: Vec<String>,
    #[serde(
        rename = "hidden",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub hidden: bool,
}

/// How new sessions start: which coding agent, model, effort and
/// permission mode. Empty means Claude Code's own default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dispatch {
    /// The agent new sessions run: empty is Claude Code.
    #[serde(
        rename = "kind",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub kind: String,
    #[serde(
        rename = "agent",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub agent: String,
    #[serde(
        rename = "model",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub model: String,
    #[serde(
        rename = "effort",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub effort: String,
    #[serde(
        rename = "permission",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub permission: String,
    /// Where new Claude sessions run: "" for rush mode, "daemon" for Claude
    /// Code's background service.
    #[serde(
        rename = "runIn",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub run_in: String,
    /// What rush-mode sessions do when a usage limit stops them: "" asks
    /// once per session, "auto" continues at the reset, "off" waits.
    #[serde(
        rename = "onLimit",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub on_limit: String,
    /// Starts rush-mode sessions without Claude Code's non-essential
    /// network traffic. Unset is on: quick_start.
    #[serde(
        rename = "lean",
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub lean: Option<bool>,
    /// What new sessions of the agents other than the built-in one start
    /// with, by kind; the built-in's are Model, Effort and Permission above.
    #[serde(
        rename = "starts",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub starts: BTreeMap<String, Start>,
    /// The effort and permissions every harness starts with, and ByHarness
    /// what one harness changes from it. Their Model is unused.
    #[serde(
        rename = "every",
        deserialize_with = "lenient",
        skip_serializing_if = "is_default"
    )]
    pub every: Start,
    #[serde(
        rename = "byHarness",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub by_harness: BTreeMap<String, Start>,
    /// Set once each route's own effort and permissions were moved into
    /// ByHarness (move_harness_starts).
    #[serde(
        rename = "harnessStarts",
        deserialize_with = "lenient",
        skip_serializing_if = "is_false"
    )]
    pub harness_starts: bool,
}

/// The model, effort and permission mode a new session starts with; empty
/// is the agent's own default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Start {
    #[serde(
        rename = "model",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub model: String,
    #[serde(
        rename = "effort",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub effort: String,
    #[serde(
        rename = "mode",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub mode: String,
}

impl Start {
    fn is_empty(&self) -> bool {
        self.model.is_empty() && self.effort.is_empty() && self.mode.is_empty()
    }
}

/// One Claude Code account rush can sign ~/.claude in as. Every session
/// shares ~/.claude; what differs between accounts is only the sign-in,
/// which rush keeps in its vault and puts in place when you switch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Login {
    #[serde(rename = "name", deserialize_with = "lenient")]
    pub name: String,
    /// The account's uuid.
    #[serde(rename = "id", deserialize_with = "lenient")]
    pub id: String,
    #[serde(
        rename = "email",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub email: String,
    #[serde(
        rename = "org",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub org: String,
    /// The oauthAccount block Claude Code keeps beside the sign-in in its
    /// state file: who the account is, not a secret. Kept as written.
    #[serde(rename = "profile", skip_serializing_if = "Option::is_none")]
    pub profile: Option<RawJson>,
    /// A team or someone else's account: rush never switches to it on its
    /// own, only when you pick it. (Go's omitempty never leaves a bool out.)
    #[serde(rename = "borrowed", deserialize_with = "lenient")]
    pub borrowed: bool,
}

impl Login {
    /// Where the login's usage readings are kept.
    pub fn usage_key(&self) -> String {
        format!("login:{}", self.id)
    }
}

/// A Claude Code config folder an older rush was given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Folder {
    #[serde(rename = "name", deserialize_with = "lenient")]
    pub name: String,
    #[serde(rename = "configDir", deserialize_with = "lenient")]
    pub config_dir: String,
}

/// The logins agent's own home (~/.claude), as its adapter says.
fn home() -> Folder {
    match agents::get(LOGINS_KIND).and_then(|a| a.home.clone()) {
        Some((name, dir)) => Folder {
            name,
            config_dir: dir,
        },
        None => Folder::default(),
    }
}

/// What SwitchOnLimit can be.
pub const ON_LIMIT_ACCOUNT: &str = ""; // another account of the same agent
pub const ON_LIMIT_AGENT: &str = "agent"; // then the next agent
pub const ON_LIMIT_OFF: &str = "off";

/// How long done work waits before it's cleaned up.
pub const DEFAULT_CLEANUP: Duration = Duration::from_secs(3 * 3600);

/// How full, in percent, the login in use may get before rush switches to
/// another.
pub const SWITCH_AT: f64 = 95.0;

impl Config {
    /// Brings a config written by an older rush up to date. It only adds
    /// and renames: nothing an older rush reads is taken away.
    pub fn migrate(&mut self) {
        if self.stay_on_account && self.switch_on_limit.is_empty() {
            self.switch_on_limit = ON_LIMIT_OFF.into();
        }
        if self.group_by == "repo" || self.group_by == "folder" {
            // Grouping by repository became a split inside the status
            // sections: see SplitBy.
            self.group_by = "status".into();
            self.split_by.clear();
        }
        if self.group_by == "account" {
            // Folders are gone; sessions group by the agent they run.
            self.group_by = "agent".into();
        }
        if self.dispatch.kind.is_empty() {
            self.dispatch.kind = agents::migrated(""); // migration: no kind was Claude Code
        }
        self.migrate_profiles();
        self.migrate_key_harness();
        self.move_profiles_in();
        self.dispatch.move_harness_starts();
    }

    /// Whether a drag copies as it ends: see copy_on_select.
    pub fn copies_on_select(&self) -> bool {
        self.copy_on_select.unwrap_or(true)
    }

    /// Sets what rush does when an account is nearly out.
    pub fn set_switch_on_limit(&mut self, v: &str) {
        self.switch_on_limit = v.into();
        self.stay_on_account = v == ON_LIMIT_OFF;
    }

    /// The agent new sessions run: its kind.
    pub fn default_agent(&self) -> String {
        if self.dispatch.kind.is_empty() {
            return LOGINS_KIND.into(); // a config made here, not read: nothing's been chosen
        }
        self.dispatch.kind.clone()
    }

    /// Records an account of another agent, named name (or after its
    /// email) when it's new, and returns what it's called.
    pub fn note_sign_in(&mut self, mut s: SignIn, name: &str) -> SignIn {
        if let Some(old) = self
            .sign_ins
            .iter_mut()
            .find(|o| o.kind == s.kind && o.id == s.id)
        {
            s.name = old.name.clone();
            if s.email.is_empty() {
                s.email = old.email.clone();
            }
            if s.plan.is_empty() {
                s.plan = old.plan.clone();
            }
            *old = s.clone();
            return s;
        }
        let mut name = name.to_string();
        if name.is_empty() {
            name = s.email.split('@').next().unwrap_or("").to_string();
        }
        if name.is_empty() {
            name = s.id.clone();
        }
        s.name = name.clone();
        let mut n = 2;
        while self.sign_in_named(&s.kind, &s.name) {
            s.name = format!("{name}-{n}");
            n += 1;
        }
        self.sign_ins.push(s.clone());
        s
    }

    fn sign_in_named(&self, kind: &str, name: &str) -> bool {
        self.sign_ins
            .iter()
            .any(|s| s.kind == kind && s.name.to_lowercase() == name.to_lowercase())
    }

    /// The accounts rush keeps for agent kind.
    pub fn sign_ins_of(&self, kind: &str) -> Vec<SignIn> {
        self.sign_ins
            .iter()
            .filter(|s| s.kind == kind)
            .cloned()
            .collect()
    }

    /// Drops an account of another agent.
    pub fn forget_sign_in(&mut self, kind: &str, id: &str) {
        self.sign_ins.retain(|s| s.kind != kind || s.id != id);
        if self.using.get(kind).is_some_and(|u| u == id) {
            self.using.remove(kind);
        }
    }

    /// Whether a session started from the box in project (a main checkout)
    /// gets its own worktree: yes unless turned off for it.
    pub fn own_worktree(&self, project: &str) -> bool {
        self.own_worktrees.get(project).copied().unwrap_or(true)
    }

    /// Keeps layout v ("split", "agent" or "list") for next time. Agents
    /// alone and the Session alone both leave the list without a Session
    /// beside it; the Session alone also opens every Session that way.
    pub fn set_view(&mut self, v: &str) {
        self.view = v.into();
        self.list_only = v != "split";
        self.chat_full = v == "agent";
    }

    /// How long done work waits before it's cleaned up; zero means never.
    pub fn cleanup_after(&self) -> Duration {
        match self.cleanup_hours {
            h if h < 0 => Duration::ZERO,
            h if h > 0 => Duration::from_secs(h as u64 * 3600),
            _ => DEFAULT_CLEANUP,
        }
    }

    /// The folders besides ~/.claude an older rush was given, whose
    /// sign-ins and past sessions are still to be taken in.
    pub fn old_folders(&self) -> Vec<Folder> {
        let root = home().config_dir;
        self.folders
            .iter()
            .filter(|a| !a.config_dir.is_empty() && a.config_dir != root)
            .cloned()
            .collect()
    }

    /// ~/.claude's entry in Folders, if it has one: what's kept when the
    /// older folders are dropped.
    pub fn root_folder(&self) -> Vec<Folder> {
        let root = home().config_dir;
        self.folders
            .iter()
            .find(|a| a.config_dir == root)
            .map(|a| vec![a.clone()])
            .unwrap_or_default()
    }

    /// Where every session runs: ~/.claude, signed in as whichever login
    /// is in use.
    pub fn active_account(&self) -> Folder {
        let mut root = home();
        for a in &self.folders {
            if a.config_dir == root.config_dir && !a.name.is_empty() {
                root.name = a.name.clone();
            }
        }
        root
    }

    /// The saved login with id.
    pub fn login(&self, id: &str) -> Option<Login> {
        self.logins.iter().find(|l| l.id == id).cloned()
    }

    /// Records who a login is, adding it named after name (or its email)
    /// when it's new; it reports whether anything changed.
    pub fn note_login(&mut self, mut l: Login, name: &str) -> bool {
        if let Some(old) = self.logins.iter_mut().find(|o| o.id == l.id) {
            let same_profile = old.profile.as_ref().map(|p| p.get()).unwrap_or("")
                == l.profile.as_ref().map(|p| p.get()).unwrap_or("");
            if old.email == l.email && old.org == l.org && same_profile {
                return false;
            }
            l.name = old.name.clone();
            *old = l;
            return true;
        }
        let mut name = name.to_string();
        if name.is_empty() {
            name = l.email.split('@').next().unwrap_or("").to_string();
        }
        if name.is_empty() {
            name = "account".into();
        }
        l.name = name.clone();
        let mut n = 2;
        while self.logins.iter().any(|x| x.name == l.name) {
            l.name = format!("{name}-{n}");
            n += 1;
        }
        self.logins.push(l);
        true
    }

    /// Whether the agents list shows column col.
    pub fn shows(&self, col: &str) -> bool {
        !self.hide_columns.iter().any(|c| c == col)
    }

    /// StackAt with its default: 42, and 0 when turned off.
    pub fn stack_percent(&self) -> i64 {
        match self.stack_at {
            s if s < 0 => 0,
            0 => 42,
            s => s,
        }
    }

    /// Says whether rush keeps an API key for provider p.
    pub fn mark_api_key(&mut self, p: &str, has: bool) {
        self.api_keys.retain(|x| x != p);
        if has {
            self.api_keys.push(p.into());
        }
    }

    /// Whether provider p has an API key: one rush keeps, or one in its
    /// variable.
    pub fn has_api_key(&self, p: &str) -> bool {
        if self.api_keys.iter().any(|x| x == p) {
            return true;
        }
        let e = agents::key_env(p);
        !e.is_empty() && std::env::var_os(&e).is_some_and(|v| !v.is_empty())
    }
}

impl Dispatch {
    /// Whether rush-mode sessions start lean: on unless turned off.
    pub fn quick_start(&self) -> bool {
        self.lean.unwrap_or(true)
    }

    /// What a new session of agent kind starts with.
    pub fn start_for(&self, kind: &str) -> Start {
        if kind == LOGINS_KIND {
            return Start {
                model: self.model.clone(),
                effort: self.effort.clone(),
                mode: self.permission.clone(),
            };
        }
        self.starts.get(kind).cloned().unwrap_or_default()
    }

    /// Sets what new sessions of agent kind start with.
    pub fn set_start_for(&mut self, kind: &str, s: Start) {
        if kind == LOGINS_KIND {
            self.model = s.model;
            self.effort = s.effort;
            self.permission = s.mode;
            return;
        }
        if s.is_empty() {
            self.starts.remove(kind);
            return;
        }
        self.starts.insert(kind.into(), s);
    }

    pub fn flags(&self) -> Vec<String> {
        let mut f = Vec::new();
        for (flag, v) in [
            ("--agent", &self.agent),
            ("--model", &self.model),
            ("--effort", &self.effort),
            ("--permission-mode", &self.permission),
        ] {
            if !v.is_empty() {
                f.push(flag.to_string());
                f.push(v.clone());
            }
        }
        f
    }

    /// Whether new sessions run the logins agent's own agent, not one of
    /// the definitions it can start as.
    pub fn own_agent(&self) -> bool {
        if self.agent.is_empty() {
            return true;
        }
        agents::get(LOGINS_KIND)
            .and_then(|a| a.builtin_def.clone())
            .is_some_and(|d| d == self.agent)
    }

    // Each route (a provider in a harness) has its own defaults: the
    // model, effort and mode its new sessions start with. Almost every
    // route is one agent kind of its own (Ollama in Pi is ollama-pi), so
    // it keeps them by kind. Only a split provider's key in its own
    // harness shares a kind with the plan (Claude Code on Anthropic's
    // key), so it keeps them under the key's id, and starts from the
    // plan's until set.

    /// What a new session on provider id, run by agent k, starts with.
    pub fn start_on(&self, id: &str, k: &str) -> Start {
        if key_in_own(id, k)
            && let Some(s) = self.starts.get(id)
        {
            return s.clone();
        }
        self.start_for(k)
    }

    /// Sets what new sessions on provider id, run by agent k, start with.
    pub fn set_start_on(&mut self, id: &str, k: &str, s: Start) {
        if !key_in_own(id, k) {
            self.set_start_for(k, s);
            return;
        }
        self.starts.insert(id.into(), s);
    }
}

/// Whether id is a split provider's key, run by its own program rather
/// than a rider.
fn key_in_own(id: &str, k: &str) -> bool {
    let (p, key) = agents::billed(id);
    key && k == p
}

#[cfg(test)]
mod tests {
    use super::*;

    // Quick start is on until it's turned off, and turning it off sticks.
    #[test]
    fn quick_start_is_on_unless_turned_off() {
        for (j, want) in [
            ("{}", true),
            (r#"{"lean":true}"#, true),
            (r#"{"lean":false}"#, false),
        ] {
            let d: Dispatch = crate::json::from_slice(j.as_bytes()).unwrap();
            assert_eq!(d.quick_start(), want, "{j}");
        }
        let b = crate::json::to_vec(&Dispatch {
            lean: Some(false),
            ..Dispatch::default()
        })
        .unwrap();
        let d: Dispatch = crate::json::from_slice(&b).unwrap();
        assert!(!d.quick_start(), "turned off, saved, read back on");
    }

    // A split provider's key in its own harness keeps its own defaults,
    // starting from the plan's; every other route keeps them by its agent.
    #[test]
    fn start_on() {
        crate::agents::fake::register_fakes();
        let mut d = Dispatch::default();
        d.set_start_on(
            "ps",
            "ps",
            Start {
                model: "big".into(),
                ..Start::default()
            },
        );
        assert_eq!(
            d.start_on("ps-key", "ps").model,
            "big",
            "the key starts from the plan's"
        );
        d.set_start_on(
            "ps-key",
            "ps",
            Start {
                model: "small".into(),
                ..Start::default()
            },
        );
        assert_eq!(d.start_on("ps-key", "ps").model, "small");
        assert_eq!(
            d.start_on("ps", "ps").model,
            "big",
            "plan and key share defaults"
        );
        d.set_start_on(
            "ps-key",
            "ps-pb",
            Start {
                effort: "high".into(),
                ..Start::default()
            },
        );
        assert_eq!(
            d.start_for("ps-pb").effort,
            "high",
            "a rider's defaults are its agent's"
        );
    }

    // A second account with the same name is numbered; forgetting drops
    // only the one named.
    #[test]
    fn sign_ins() {
        let mut c = Config::default();
        c.note_sign_in(
            SignIn {
                kind: "codex".into(),
                id: "acct-1".into(),
                email: "me@example.com".into(),
                ..SignIn::default()
            },
            "",
        );
        let got = c.note_sign_in(
            SignIn {
                kind: "codex".into(),
                id: "acct-2".into(),
                email: "Me@example.org".into(),
                ..SignIn::default()
            },
            "",
        );
        assert_eq!(got.name, "Me-2");
        c.forget_sign_in("codex", "acct-1");
        assert_eq!(c.sign_ins_of("codex").len(), 1);
        assert!(c.note_login(
            Login {
                id: "a".into(),
                email: "x@y".into(),
                ..Login::default()
            },
            ""
        ));
        assert!(!c.note_login(
            Login {
                id: "a".into(),
                email: "x@y".into(),
                ..Login::default()
            },
            ""
        ));
        assert!(c.note_login(
            Login {
                id: "b".into(),
                email: "x@z".into(),
                ..Login::default()
            },
            ""
        ));
        assert_eq!(c.logins[1].name, "x-2");
    }
}
