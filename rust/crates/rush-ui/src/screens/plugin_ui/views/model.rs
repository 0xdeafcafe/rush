//! What a session's views are, as data: a panel an agent (or a plugin, or
//! your script) keeps beside its conversation with `rush view set`. After
//! Go's internal/host/views.go:29-175 (View, Block, Item, Stat, Progress,
//! Group, Row) and viewacts.go:22-46, :103-117 (Action, Change, Yours,
//! ItemKey, BlockKey, GroupKey).
//!
//! The files, their checks and the changes a click makes to them are the
//! host's: until rush-host is merged they're reached through
//! [`ViewStore`], which the host implements.

use rush_state::Time;
use serde::{Deserialize, Serialize};

/// One panel: a title, a few numbers, rows in groups, blocks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub name: String,
    pub title: String,
    /// One line, under the session in the list.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<Stat>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<Block>,
    /// The view's own colour: one of ACCENTS; "" is rush's orange.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub accent: String,
    /// The plugin that drew it; "" is the session's agent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub by: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    pub updated: Time,
    /// When it was put away, for a past view.
    #[serde(skip_serializing_if = "zero")]
    pub gone: Time,
    /// Its file, for a past view.
    #[serde(skip)]
    pub file: String,
}

/// A part of a view drawn in one style, its kind (BLOCK_KINDS).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Block {
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub folded: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// A hero's number.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// A chart's: columns or line.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub style: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trend: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<Stat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
}

/// One thing in a block: a row, a table's row, a card, an event, a step,
/// a node.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub meta: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub link: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

/// The styles a block may be drawn in.
pub const BLOCK_KINDS: [&str; 19] = [
    "table",
    "kv",
    "checklist",
    "log",
    "list",
    "bars",
    "chart",
    "heatmap",
    "compare",
    "steps",
    "timeline",
    "board",
    "tree",
    "hero",
    "callout",
    "code",
    "text",
    "stats",
    "progress",
];

/// The colours a view may take as its own.
pub const ACCENTS: [&str; 6] = ["orange", "blue", "green", "yellow", "purple", "red"];

/// A number along a view's top: "8" "ready".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stat {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trend: Vec<f64>,
}

/// How far along something is: done of total.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub done: f64,
    pub total: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
}

/// A titled run of rows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Group {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub folded: bool,
    pub rows: Vec<Row>,
}

/// One line of a view's group.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Row {
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub meta: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub link: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

/// A button: what it says, and what a click on it does.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    /// A message to the agent, sent at once.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub send: String,
    /// A web link.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub open: String,
    /// Words to copy.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub copy: String,
    /// A change to its row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<Change>,
}

/// What a click changes on a row: None leaves it as it is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Change {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<String>,
}

/// The key of a row under prefix: its id, else its text, else its first
/// cell (ItemKey).
pub fn item_key(prefix: &str, id: &str, text: &str, cells: &[String]) -> String {
    if !id.is_empty() {
        format!("{prefix}/{id}")
    } else if !text.is_empty() {
        format!("{prefix}/{text}")
    } else if let Some(c) = cells.first() {
        format!("{prefix}/{c}")
    } else {
        String::new()
    }
}

/// The prefix of a block's rows (BlockKey).
pub fn block_key(bi: usize) -> String {
    format!("b{bi}")
}

/// The prefix of a group's rows (GroupKey).
pub fn group_key(gi: usize) -> String {
    format!("g{gi}")
}

/// Where a session's views are kept: the host's folder of them (Go's
/// host.Views, PastViews, ViewsChanged, DropView, BringBack, ChangeRow).
/// rush-host implements it; with none set no views show.
pub trait ViewStore: Send + Sync {
    /// The session's views, the one updated last first, and when its
    /// folder last changed.
    fn views(&self, id: &str) -> (Vec<View>, Time);
    /// The views put away, the one gone last first.
    fn past_views(&self, id: &str) -> Vec<View>;
    /// When the session's views last changed: read them again only then.
    fn changed(&self, id: &str) -> Time;
    /// Puts the view of name away.
    fn drop_view(&self, id: &str, name: &str) -> Result<(), String>;
    /// Brings the past view in file back.
    fn bring_back(&self, id: &str, file: &str) -> Result<(), String>;
    /// Changes the row of key in the view of name, as a click did.
    fn change_row(&self, id: &str, name: &str, key: &str, c: &Change) -> Result<(), String>;
}

fn zero(t: &Time) -> bool {
    t.is_zero()
}
