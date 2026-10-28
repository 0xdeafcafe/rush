//! What a plugin may do in rush's own screen, and what it draws there, as
//! data: the UI capabilities and their checks, the events it hears, the
//! sections, statuses, notes, notices and picks it adds, each made safe to
//! draw. The drawing itself is rush-ui's.
//! Ported from internal/plugin/ui.go.
//!
//! A plugin never runs on rush's UI thread. The UI hands the broker events
//! without waiting, draws what plugins added from a copy it already holds,
//! and waits for an intercept only in the background, and only so long,
//! before sending the message as it was.

use std::collections::BTreeMap;
use std::time::Duration;

use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::manifest::{Manifest, PROTO_RUSH, name_ok};
use crate::text::{clip, go_q, printable};

// UI capabilities.

/// Hears what happens in rush's screen: a Session opened or left, a turn
/// started or ended, a session stopped by an error and of what kind, the
/// API going away and coming back. What the agent list shows of a
/// session, never what was said.
pub const UI_EVENTS: &str = "events";
/// Sees what you type in a message box (as it changes, and when it's sent
/// or cleared), may set it, and may put a note on its edge.
pub const UI_INPUT: &str = "input";
/// Is asked before a message you send goes, and may change it or hold it
/// back, with a reason shown. It needs "input".
pub const UI_INTERCEPT: &str = "intercept";
/// Adds sections to a Session's overview, and a short status to its row.
pub const UI_OVERVIEW: &str = "overview";
/// Shows a short message at the bottom of rush's screen.
pub const UI_NOTIFY: &str = "notify";
/// Sends a message to a session as if you'd typed it, or continues one an
/// error stopped. It needs "events", and reaches only sessions in the
/// manifest's workspaces that ask before acting.
pub const UI_SEND: &str = "send";
/// Draws views beside a session's conversation, named as its own.
pub const UI_VIEWS: &str = "views";

pub(crate) const UI_CAPS: [&str; 7] = [
    UI_EVENTS,
    UI_INPUT,
    UI_INTERCEPT,
    UI_OVERVIEW,
    UI_NOTIFY,
    UI_SEND,
    UI_VIEWS,
];

/// How many views one plugin keeps on a session, so the agent's own
/// always have room.
pub const MAX_PLUGIN_VIEWS: usize = 2;

/// How long rush waits for all intercepts of one message together.
pub const INTERCEPT_BUDGET: Duration = Duration::from_millis(400);
/// Each plugin's share of it.
pub const INTERCEPT_EACH: Duration = Duration::from_millis(250);
/// Timeouts in a row before its intercepts are skipped until restarted.
pub const INTERCEPT_STRIKE_OUT: u32 = 3;

// Limits on what a plugin adds to the screen.
pub const MAX_COMMANDS: usize = 32;
pub const MAX_SETTINGS: usize = 32;
pub const MAX_SECTIONS: usize = 4; // per session, per plugin
pub const MAX_SECTION_LINES: usize = 24;
pub const MAX_LINE_LEN: usize = 200; // characters
pub const MAX_STATUS_LEN: usize = 24;
pub const MAX_NOTE_LEN: usize = 60; // on a message box's edge
pub const MAX_NOTIFY_LEN: usize = 200;
pub const MAX_SESSIONS_TRACKED: usize = 2000;

/// A command a plugin adds: to the # commands, the command bar and the
/// keymap, as plugin:<name>.<command>.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandSpec {
    pub name: String,
    pub description: String,
    /// A key it suggests, taken only if it's free.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key: String,
}

/// A setting a plugin offers, shown under Settings, Plugins. rush keeps
/// its value, in a file the plugin can't write, and hands it the values at
/// initialize and when you change one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingSpec {
    pub key: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// bool, choice or text
    #[serde(rename = "type")]
    pub kind: String,
    /// for choice
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// "true"/"false" for bool
    #[serde(skip_serializing_if = "String::is_empty")]
    pub default: String,
}

impl Manifest {
    /// Whether it was given UI capability c.
    pub fn can_ui(&self, c: &str) -> bool {
        self.ui.iter().any(|u| u == c)
    }

    /// Checks the manifest's ui, commands and settings.
    pub(crate) fn validate_ui(&self) -> Result<(), String> {
        if (!self.ui.is_empty() || !self.commands.is_empty() || !self.settings.is_empty())
            && self.proto() != PROTO_RUSH
        {
            return Err(
                "an MCP plugin cannot take part in rush's screen: it has no way to hear from it"
                    .into(),
            );
        }
        for c in &self.ui {
            if !UI_CAPS.contains(&c.as_str()) {
                return Err(format!(
                    "ui capability {}: use one of {}",
                    go_q(c),
                    UI_CAPS.join(", ")
                ));
            }
        }
        if self.can_ui(UI_INTERCEPT) && !self.can_ui(UI_INPUT) {
            return Err(
                r#""intercept" needs "input": it's shown the message it's asked about"#.into(),
            );
        }
        if self.can_ui(UI_SEND) {
            if !self.can_ui(UI_EVENTS) {
                return Err(r#""send" needs "events", to know which session it sends to"#.into());
            }
            if self.workspaces.is_empty() {
                return Err(
                    r#""send" needs at least one workspace whose sessions it may send to"#.into(),
                );
            }
        }
        self.validate_commands()?;
        self.validate_settings()
    }

    fn validate_commands(&self) -> Result<(), String> {
        if self.commands.len() > MAX_COMMANDS {
            return Err(format!("over {MAX_COMMANDS} commands"));
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.commands {
            if !name_ok(&c.name) {
                return Err(format!(
                    "command {}: use lowercase letters, digits and dashes",
                    go_q(&c.name)
                ));
            }
            if !seen.insert(c.name.as_str()) {
                return Err(format!("command {} twice", go_q(&c.name)));
            }
            if c.description.is_empty() || c.description.chars().count() > 200 {
                return Err(format!(
                    "command {} needs a description of at most 200 characters",
                    go_q(&c.name)
                ));
            }
            if !c.key.is_empty() && (c.key.contains(['\n', '\t']) || c.key.len() > 40) {
                return Err(format!(
                    "command {}: key {} is not a key",
                    go_q(&c.name),
                    go_q(&c.key)
                ));
            }
        }
        Ok(())
    }

    fn validate_settings(&self) -> Result<(), String> {
        if self.settings.len() > MAX_SETTINGS {
            return Err(format!("over {MAX_SETTINGS} settings"));
        }
        let mut seen = std::collections::HashSet::new();
        for s in &self.settings {
            if !name_ok(&s.key) {
                return Err(format!(
                    "setting {}: use lowercase letters, digits and dashes",
                    go_q(&s.key)
                ));
            }
            if !seen.insert(s.key.as_str()) {
                return Err(format!("setting {} twice", go_q(&s.key)));
            }
            s.validate()?;
        }
        Ok(())
    }
}

impl SettingSpec {
    fn validate(&self) -> Result<(), String> {
        let k = go_q(&self.key);
        if self.title.is_empty() || self.title.chars().count() > 60 {
            return Err(format!(
                "setting {k} needs a title of at most 60 characters"
            ));
        }
        match self.kind.as_str() {
            "bool" => {
                if !self.default.is_empty() && self.default != "true" && self.default != "false" {
                    return Err(format!("setting {k}: a bool's default is true or false"));
                }
            }
            "choice" => {
                if self.choices.len() < 2 || self.choices.len() > 16 {
                    return Err(format!("setting {k}: a choice needs 2 to 16 choices"));
                }
                if !self.default.is_empty() && !self.choices.contains(&self.default) {
                    return Err(format!(
                        "setting {k}: default {} is not one of its choices",
                        go_q(&self.default)
                    ));
                }
            }
            "text" => {
                if self.default.len() > 1000 {
                    return Err(format!("setting {k}: default is too long"));
                }
            }
            _ => {
                return Err(format!(
                    "setting {k}: type {}, use bool, choice or text",
                    go_q(&self.kind)
                ));
            }
        }
        Ok(())
    }
}

/// What a UI event says about a session: what the list shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSession {
    /// rush's key for it
    pub id: String,
    /// Claude Code's (or the agent's) id
    #[serde(rename = "sessionId", skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// claude, codex, …
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub repo: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    /// a rush-mode session
    #[serde(skip_serializing_if = "is_false")]
    pub hosted: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

// UI event kinds.

/// One rush shows: each recent one when a plugin connects, and each new one.
pub const EV_SESSION_SEEN: &str = "session.seen";
/// Its Session came into view.
pub const EV_SESSION_OPENED: &str = "session.opened";
/// Its Session went out of view.
pub const EV_SESSION_LEFT: &str = "session.left";
pub const EV_TURN_STARTED: &str = "turn.started";
pub const EV_TURN_ENDED: &str = "turn.ended";
/// Error says why, when an error did it.
pub const EV_SESSION_STOPPED: &str = "session.stopped";
/// The API stopped answering.
pub const EV_NETWORK_DOWN: &str = "network.down";
/// And answers again.
pub const EV_NETWORK_UP: &str = "network.up";
/// "input" only; Text is the box.
pub const EV_INPUT_CHANGED: &str = "input.changed";
/// "input" only.
pub const EV_INPUT_SENT: &str = "input.sent";
/// "input" only.
pub const EV_INPUT_CLEARED: &str = "input.cleared";
/// Not sent: commands come as ui.command requests.
pub const EV_COMMAND: &str = "command";

/// One thing that happened in rush's screen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiEvent {
    pub kind: String,
    /// which rush window
    pub ui: String,
    pub at: Time,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<UiSession>,
    /// Why a session stopped: limit, auth, offline, retryable, too-long or
    /// other, and what it said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<UiError>,
    /// The message box, for input events, to plugins with "input".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// The whole box, for input events: Text with where the cursor is and
    /// what its chips stand for.
    #[serde(rename = "box", skip_serializing_if = "Option::is_none")]
    pub input_box: Option<TextBox>,
}

/// A message box as a plugin with "input" sees it, and may set it (Go's
/// plugin.Box). Text is as the box shows it: a paste as
/// [#N L lines: start…end], an image as [Image #N]. Setting one puts all
/// of it back as it was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextBox {
    pub text: String,
    /// in characters from the start
    pub cursor: i64,
    /// each paste chip's text, by N
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub pastes: BTreeMap<i64, String>,
    /// Each [Image #N]'s file. The Prompt, whose images are attachments
    /// rather than in its text, numbers them 1, 2, 3.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub images: BTreeMap<i64, String>,
}

impl TextBox {
    /// About how many bytes the box holds, to check against limits.
    pub fn size(b: Option<&TextBox>) -> usize {
        let Some(b) = b else { return 0 };
        b.text.len()
            + b.pastes.values().map(String::len).sum::<usize>()
            + b.images.values().map(String::len).sum::<usize>()
    }
}

/// Why a session stopped.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiError {
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// rush telling the session to continue itself, as it does after an
    /// offline or retryable error; a plugin needn't too.
    #[serde(skip_serializing_if = "is_false")]
    pub retrying: bool,
}

impl UiEvent {
    /// Whether the event carries what you typed.
    pub fn input_event(&self) -> bool {
        self.kind.starts_with("input.")
    }

    /// The event as plugin m may see it, or None if it may not.
    pub fn for_plugin(&self, m: &Manifest) -> Option<UiEvent> {
        if self.input_event() {
            return m.can_ui(UI_INPUT).then(|| self.clone());
        }
        if !m.can_ui(UI_EVENTS) {
            return None;
        }
        let mut e = self.clone();
        e.text.clear();
        e.input_box = None;
        Some(e)
    }
}

/// A line a plugin draws, with a tone rush colours it by.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Line {
    pub text: String,
    /// "", dim, good, warn, bad, accent
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    /// opened with enter, if http(s)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
}

const TONES: [&str; 6] = ["", "dim", "good", "warn", "bad", "accent"];

fn known_tone(t: &str) -> String {
    if TONES.contains(&t) {
        t.to_string()
    } else {
        String::new()
    }
}

/// A titled block a plugin adds to a Session's overview.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Section {
    pub id: String,
    pub title: String,
    #[serde(serialize_with = "nil_if_empty")]
    pub lines: Vec<Line>,
}

fn nil_if_empty<S: serde::Serializer, T: Serialize>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        return s.serialize_none();
    }
    v.serialize(s)
}

/// A short word or two a plugin puts on a session's row.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
}

/// Makes a line safe to draw: no escapes or control characters, no longer
/// than MAX_LINE_LEN, a known tone, and a URL only if it's http(s).
pub fn clean_line(mut l: Line) -> Line {
    l.text = clip(&printable(&l.text), MAX_LINE_LEN);
    l.tone = known_tone(&l.tone);
    if !(l.url.starts_with("https://") || l.url.starts_with("http://"))
        || l.url.len() > 2000
        || l.url.contains([' ', '\n', '\t', '\x1b'])
    {
        l.url.clear();
    }
    l
}

/// Makes a section safe to draw, or says why it can't be.
pub fn clean_section(mut s: Section) -> Result<Section, String> {
    if !name_ok(&s.id) {
        return Err(format!(
            "section id {}: use lowercase letters, digits and dashes",
            go_q(&s.id)
        ));
    }
    s.title = clip(&printable(&s.title), 60);
    s.lines.truncate(MAX_SECTION_LINES);
    s.lines = s.lines.into_iter().map(clean_line).collect();
    Ok(s)
}

/// Makes a status safe to draw.
pub fn clean_status(mut s: Status) -> Status {
    s.text = clip(&printable(&s.text), MAX_STATUS_LEN);
    s.tone = known_tone(&s.tone);
    s
}

/// Makes a note on a message box's edge safe to draw.
pub fn clean_note(mut s: Status) -> Status {
    s.text = clip(&printable(&s.text), MAX_NOTE_LEN);
    s.tone = known_tone(&s.tone);
    s
}

/// Makes a notice safe to draw.
pub fn clean_notice(s: &str) -> String {
    clip(&printable(s), MAX_NOTIFY_LEN)
}

/// Contributions a UI draws from, as the broker holds them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    #[serde(serialize_with = "nil_if_empty")]
    pub plugins: Vec<UiPlugin>,
    /// Each session's overview sections, by rush session id, each plugin's
    /// in the order it gave them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub sections: BTreeMap<String, Vec<UiSection>>,
    /// Each session's row statuses, by rush session id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub statuses: BTreeMap<String, Vec<UiStatus>>,
    /// What plugins put on the edge of a message box: a session's, by rush
    /// session id, or the Prompt's, by "".
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub notes: BTreeMap<String, Vec<UiStatus>>,
}

/// What the UI needs of a plugin taking part in its screen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPlugin {
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CommandSpec>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<SettingSpec>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ui: Vec<String>,
    /// Why its intercepts are skipped for now, if they are.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub skipped: String,
}

/// A plugin's section.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSection {
    pub plugin: String,
    #[serde(flatten)]
    pub section: Section,
}

/// A plugin's status.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiStatus {
    pub plugin: String,
    #[serde(flatten)]
    pub status: Status,
}

/// Something a plugin asks one rush window (or, with no UI, every one) to
/// do now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiDo {
    pub plugin: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ui: String,
    /// notify, input.set, send, pick
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    /// For input.set, the whole box to put back.
    #[serde(rename = "box", skip_serializing_if = "Option::is_none")]
    pub input_box: Option<TextBox>,
    /// When set, the text the box must hold for it to be set at all, so
    /// what's typed meanwhile is never lost.
    #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
    pub if_text: Option<String>,
    /// For pick, the list to show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick: Option<Pick>,
}

/// What a plugin is asked before a message goes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Intercept {
    /// before-send
    pub hook: String,
    pub ui: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<UiSession>,
    pub text: String,
}

/// A plugin's answer: let it go as it is, go changed, or be held back.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InterceptResult {
    /// allow, rewrite, block
    pub action: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// Who changed or held it, filled in by the broker.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plugin: String,
}

/// A list a plugin asks a window to show, for you to choose from: in tabs,
/// filtered as you type, each action on its own key. It's shown only in
/// answer to one of the plugin's commands, soon after you ran it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pick {
    /// the plugin's name for it, given back with the choice
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// none is one list
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<String>,
    /// the tab it opens on
    #[serde(skip_serializing_if = "is_zero")]
    pub tab: i64,
    #[serde(serialize_with = "nil_if_empty")]
    pub items: Vec<PickItem>,
    #[serde(serialize_with = "nil_if_empty")]
    pub actions: Vec<PickAction>,
    /// What each tab says with nothing in it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub empty: Vec<String>,
    /// Whose message box it's about: a session's id, or "" for the Prompt.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session: String,
}

/// One row of a Pick.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PickItem {
    pub id: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub tab: i64,
    /// its first line is the row, the next two a preview
    pub text: String,
    /// on the row's right, faint
    #[serde(skip_serializing_if = "String::is_empty")]
    pub meta: String,
}

/// What a key does to the chosen item. Stay keeps the list open, without
/// the item, as for forgetting one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PickAction {
    pub key: String,
    pub name: String,
    #[serde(skip_serializing_if = "is_false")]
    pub stay: bool,
}

/// What was chosen from a Pick, and how.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Picked {
    pub plugin: String,
    pub pick: String,
    pub item: String,
    /// its key
    pub action: String,
    pub ui: String,
    /// Whose box, as in ui.command.
    #[serde(rename = "box")]
    pub box_id: String,
    /// The box whole, for "input" only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<TextBox>,
}

// Limits on a Pick.
pub const MAX_PICK_ITEMS: usize = 500;
pub const MAX_PICK_TABS: usize = 6;
pub const MAX_PICK_ACTIONS: usize = 6;
pub const MAX_PICK_TEXT: usize = 4000;

/// ^(enter|(ctrl|alt)\+(shift\+)?[a-z0-9])$: a key an action may take,
/// since what's typed filters the list.
fn pick_key_ok(k: &str) -> bool {
    if k == "enter" {
        return true;
    }
    let Some(rest) = k.strip_prefix("ctrl+").or_else(|| k.strip_prefix("alt+")) else {
        return false;
    };
    let rest = rest.strip_prefix("shift+").unwrap_or(rest);
    let b = rest.as_bytes();
    b.len() == 1 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
}

/// Makes a pick safe to draw, or says why it can't be.
pub fn clean_pick(mut p: Pick) -> Result<Pick, String> {
    if !name_ok(&p.id) {
        return Err(format!(
            "pick id {}: use lowercase letters, digits and dashes",
            go_q(&p.id)
        ));
    }
    if p.items.len() > MAX_PICK_ITEMS
        || p.tabs.len() > MAX_PICK_TABS
        || p.actions.is_empty()
        || p.actions.len() > MAX_PICK_ACTIONS
    {
        return Err(format!(
            "a pick has 1 to {MAX_PICK_ACTIONS} actions, at most {MAX_PICK_TABS} tabs and {MAX_PICK_ITEMS} items"
        ));
    }
    p.title = clip(&printable(&p.title), 40);
    p.about = clip(&printable(&p.about), MAX_LINE_LEN);
    for t in &mut p.tabs {
        *t = clip(&printable(t), 20);
    }
    for e in &mut p.empty {
        *e = clip(&printable(e), MAX_LINE_LEN);
    }
    // max(0, min(tab, len(tabs)-1))
    p.tab = p.tab.min(p.tabs.len() as i64 - 1).max(0);
    let mut seen = std::collections::HashSet::new();
    for a in &mut p.actions {
        if !pick_key_ok(&a.key) || a.key == "ctrl+c" || !seen.insert(a.key.clone()) {
            return Err(format!(
                "action key {}: enter, or ctrl+ or alt+ a letter or digit, once each",
                go_q(&a.key)
            ));
        }
        a.name = clip(&printable(&a.name), 30);
    }
    let tabs = p.tabs.len().max(1) as i64;
    for (i, it) in p.items.iter_mut().enumerate() {
        if it.id.is_empty() || it.id.len() > 200 || it.tab < 0 || it.tab >= tabs {
            return Err(format!("item {i}: needs an id, and a tab it has"));
        }
        // The text keeps its lines, for the preview; each is made safe.
        it.text = clip(&it.text, MAX_PICK_TEXT)
            .split('\n')
            .map(printable)
            .collect::<Vec<_>>()
            .join("\n");
        it.meta = clip(&printable(&it.meta), 40);
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_picks_are_made_safe() {
        let l = clean_line(Line {
            text: "a\x1b[2Jb".into(),
            tone: "loud".into(),
            url: "javascript:x".into(),
        });
        assert_eq!(
            (l.text.as_str(), l.tone.as_str(), l.url.as_str()),
            ("a[2Jb", "", "")
        );
        let l = clean_line(Line {
            url: "https://x.y/z".into(),
            tone: "good".into(),
            ..Default::default()
        });
        assert_eq!((l.tone.as_str(), l.url.as_str()), ("good", "https://x.y/z"));
        assert!(
            clean_section(Section {
                id: "Bad".into(),
                ..Default::default()
            })
            .is_err()
        );

        let ok = Pick {
            id: "drafts".into(),
            tab: 5,
            tabs: vec!["a".into(), "b".into()],
            items: vec![PickItem {
                id: "1".into(),
                tab: 1,
                text: "one\x07\ntwo".into(),
                ..Default::default()
            }],
            actions: vec![PickAction {
                key: "enter".into(),
                name: "use".into(),
                stay: false,
            }],
            ..Default::default()
        };
        let p = clean_pick(ok.clone()).unwrap();
        assert_eq!((p.tab, p.items[0].text.as_str()), (1, "one\ntwo"));
        for key in ["ctrl+c", "x", "ctrl+X", "alt+shift+1x"] {
            let mut bad = ok.clone();
            bad.actions[0].key = key.into();
            assert!(clean_pick(bad).is_err(), "{key}");
        }
        let mut good = ok.clone();
        good.actions[0].key = "alt+shift+9".into();
        assert!(clean_pick(good).is_ok());
        let mut no_tab = ok;
        no_tab.items[0].tab = 2;
        assert!(clean_pick(no_tab).is_err());
    }

    #[test]
    fn events_as_a_plugin_may_see_them() {
        let typed = UiEvent {
            kind: EV_INPUT_CHANGED.into(),
            text: "secret".into(),
            ..Default::default()
        };
        let opened = UiEvent {
            kind: EV_SESSION_OPENED.into(),
            text: "x".into(),
            ..Default::default()
        };
        let events = Manifest {
            ui: vec![UI_EVENTS.into()],
            ..Default::default()
        };
        let input = Manifest {
            ui: vec![UI_INPUT.into()],
            ..Default::default()
        };
        assert!(typed.for_plugin(&events).is_none());
        assert_eq!(typed.for_plugin(&input).unwrap().text, "secret");
        assert_eq!(opened.for_plugin(&events).unwrap().text, "");
        assert!(opened.for_plugin(&input).is_none());
    }

    #[test]
    fn wire_shapes() {
        let s = UiSection {
            plugin: "p".into(),
            section: Section {
                id: "a".into(),
                title: "A".into(),
                lines: vec![],
            },
        };
        assert_eq!(
            rush_state::json::to_vec(&s)
                .map(String::from_utf8)
                .unwrap()
                .unwrap(),
            r#"{"plugin":"p","id":"a","title":"A","lines":null}"#
        );
        let d: UiDo = serde_json::from_str(r#"{"plugin":"p","kind":"input.set","box":{"text":"hi","cursor":2,"pastes":{"1":"x"}},"if":""}"#).unwrap();
        assert_eq!(d.if_text.as_deref(), Some(""));
        assert_eq!(d.input_box.unwrap().pastes[&1], "x");
    }
}
