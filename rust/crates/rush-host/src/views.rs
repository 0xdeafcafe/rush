//! An agent can keep views of its own work: a panel rush draws beside its
//! conversation, in rush's own parts, and a line for the list. The agent
//! says what to show (`rush view set prs`, JSON on stdin) and rush draws
//! it; the same name again updates it in place. Plugins and your scripts
//! show one the same way. Each lives in the session's folder, one file a
//! view, so any rush that has the session open reads it.
//!
//! Ported from internal/host/views.go.

use std::path::{Path, PathBuf};

use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::paths::dir;
use crate::viewacts::{Action, Yours, check_actions, clip_detail, keep_yours};
use crate::wire::zero_time;

/// Tells every agent it can keep a view: kept short, as every session
/// carries it.
pub const VIEWS_PROMPT: &str = "rush draws a panel beside this conversation that you fill with your `view` tool (or `rush view set <name>`, JSON on stdin). Use it on your own, without being asked, whenever the user would otherwise follow a list, numbers or progress through your messages: a list you were handed to work through, a long search across the code, pull requests, a test run, a migration, services, comparisons, an incident. Pick the styles that fit (table, chart, timeline, board, hero number and more), keep it current, and clear it once it's no use: rush keeps it among the past views the user can bring back, and puts away one you haven't touched in 3 turns. The user can click a row open to its detail and press buttons you put on it (actions: send you a message, open a link, copy, or change the row): give rows the next steps they'd take, so a click does it rather than typing. `rush view guide` is the style guide.";

/// Numbers as Go writes a float64: 9, not 9.0.
pub(crate) mod floats {
    use serde::{Serialize, Serializer};

    struct F(f64);

    impl Serialize for F {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            rush_core::wire::float::serialize(&self.0, s)
        }
    }

    pub fn serialize<S: Serializer>(v: &[f64], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(v.iter().map(|&x| F(x)))
    }

    pub mod opt {
        use super::*;

        pub fn serialize<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
            match v {
                Some(x) => F(*x).serialize(s),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
            <Option<f64> as serde::Deserialize>::deserialize(d)
        }
    }
}

fn nullv<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    rush_core::wire::nullvec::deserialize(d)
}

fn floats_de<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
    rush_core::wire::nullvec::deserialize(d)
}

/// One panel: a title, a few numbers, and rows in groups.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub name: String,
    pub title: String,
    /// One line, under the session in the list.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Its title's colour.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    /// A bar under the title: done of total, and a word on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub stats: Vec<Stat>,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub groups: Vec<Group>,
    /// A quiet line at the end.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The view's parts after those above, each in a style of its own.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub blocks: Vec<Block>,
    /// The view's own colour: one of ACCENTS; "" is rush's orange.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub accent: String,
    /// The plugin that drew it; "" is the session's agent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub by: String,
    /// A view you brought back from the past ones: rush doesn't put it
    /// away by itself.
    pub pinned: bool,
    /// Buttons along the view's foot.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub actions: Vec<Action>,
    /// The changes you made to its rows, by a click: kept over the agent's,
    /// until it changes that itself.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub yours: Vec<Yours>,
    pub updated: Time,
    /// When it was put away, for a past view.
    #[serde(skip_serializing_if = "zero_time")]
    pub gone: Time,
    /// Its file, for a past view.
    #[serde(skip)]
    pub file: String,
}

/// A part of a view drawn in one style, its kind; which of its fields it
/// reads depends on that (see BLOCK_KINDS and views.go's table).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Block {
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    pub folded: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// A hero's number.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// A chart's: columns or line.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub style: String,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub columns: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", serialize_with = "floats::serialize", deserialize_with = "floats_de")]
    pub values: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", serialize_with = "floats::serialize", deserialize_with = "floats_de")]
    pub trend: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub items: Vec<Item>,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub stats: Vec<Stat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
}

/// One thing in a block: a row, a cell's row, a card, an event, a step, a
/// node.
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
    #[serde(skip_serializing_if = "Option::is_none", with = "floats::opt")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "floats::opt")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub cells: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", serialize_with = "floats::serialize", deserialize_with = "floats_de")]
    pub values: Vec<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub items: Vec<Item>,
    /// Names it for the changes you make to it, its text if none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// More about it, shown when a click opens it, with its actions.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub actions: Vec<Action>,
}

/// The styles a block may be drawn in.
pub const BLOCK_KINDS: [&str; 19] = [
    "table", "kv", "checklist", "log", "list", "bars", "chart", "heatmap", "compare", "steps",
    "timeline", "board", "tree", "hero", "callout", "code", "text", "stats", "progress",
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
    /// How the number has gone, oldest first: a sparkline under it.
    #[serde(skip_serializing_if = "Vec::is_empty", serialize_with = "floats::serialize", deserialize_with = "floats_de")]
    pub trend: Vec<f64>,
}

/// How far along something is: done of total.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    #[serde(with = "rush_core::wire::float")]
    pub done: f64,
    #[serde(with = "rush_core::wire::float")]
    pub total: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
}

/// A titled run of rows; its tone colours its title. text is a few words
/// under its title; folded starts it closed, a click opens it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Group {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    pub folded: bool,
    #[serde(with = "rush_core::wire::nullvec")]
    pub rows: Vec<Row>,
}

/// One line of a view: what it is, a quiet word on it, and a link. tone
/// colours its mark: good, bad, warn, busy or quiet.
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
    /// 0 to 1: a small bar at the row's end.
    #[serde(skip_serializing_if = "Option::is_none", with = "floats::opt")]
    pub progress: Option<f64>,
    /// A number drawn as a bar against the group's largest.
    #[serde(skip_serializing_if = "Option::is_none", with = "floats::opt")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "nullv")]
    pub actions: Vec<Action>,
}

/// Tones a view may use; anything else reads as plain.
pub const TONES: [&str; 5] = ["good", "bad", "warn", "busy", "quiet"];

const PAST_MOST: usize = 40; // past views a session keeps
const BLOCKS_MOST: usize = 16; // blocks in a view
const ITEMS_MOST: usize = 200; // items in a block, all its levels
const CELLS_MOST: usize = 8; // a table's columns, a heatmap's are 40
const VALUES_MOST: usize = 120; // a chart's or heatmap row's numbers
const CODE_MOST: usize = 60; // a code block's lines
pub const VIEWS_MOST: usize = 6; // views a session may keep
const VIEW_ROWS_MOST: usize = 200; // rows across a view's groups
pub const VIEW_TEXT_MOST: usize = 300; // characters in any one text

/// Whether name is a view's name: lowercase letters, digits and dashes.
pub fn name_ok(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn views_dir(id: &str) -> PathBuf {
    dir(id).join("views")
}

/// s quoted as Go's %q does, near enough for an error.
fn go_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// Keeps v as the session's view of its name, in place of any before it,
/// checked and cut to size.
pub fn save_view(id: &str, mut v: View) -> Result<(), String> {
    if !name_ok(&v.name) {
        return Err(
            "a view's name is lowercase letters, digits and dashes, up to 64: like prs or tests"
                .into(),
        );
    }
    if std::fs::metadata(dir(id)).is_err() {
        return Err(format!("no rush session {id}"));
    }
    v.check()?;
    let d = views_dir(id);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let names: Vec<String> = std::fs::read_dir(&d)
        .map(|es| {
            es.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default();
    let mine = format!("{}.json", v.name);
    if names.len() >= VIEWS_MOST && !names.contains(&mine) {
        return Err(
            "a session keeps at most 6 views: clear one first (rush view clear <name>)".into(),
        );
    }
    // One you brought back stays yours to put away, and the changes you
    // made to it stay until the agent changes the same.
    let mut yours = std::mem::take(&mut v.yours);
    if let Ok(b) = std::fs::read(d.join(&mine))
        && let Ok(was) = serde_json::from_slice::<View>(&b)
    {
        v.pinned = v.pinned || was.pinned;
        yours = was.yours;
    }
    v.yours = keep_yours(&mut v, yours);
    v.updated = Time::now();
    v.gone = Time::default();
    let b = crate::json::line(&v);
    crate::json::write_atomic(&d.join(&mine), &b).map_err(|e| e.to_string())
}

/// The rules every text in a view is held to: one line each, cut to
/// VIEW_TEXT_MOST.
pub(crate) fn clip(s: &mut String) {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    *s = cut(&one, VIEW_TEXT_MOST);
}

/// s cut to most characters, the last an ellipsis when cut.
pub(crate) fn cut(s: &str, most: usize) -> String {
    match s.char_indices().nth(most) {
        None => s.to_string(),
        Some(_) => {
            let i = s.char_indices().nth(most - 1).map_or(s.len(), |(i, _)| i);
            format!("{}…", &s[..i])
        }
    }
}

/// A tone a view has, else none.
pub(crate) fn tone(t: &mut String) {
    if !TONES.contains(&t.as_str()) {
        t.clear();
    }
}

fn web(link: &mut String) {
    if !link.starts_with("https://") && !link.starts_with("http://") {
        link.clear(); // only web links: a click opens it
    }
}

fn finite(vs: &mut [f64]) {
    for v in vs {
        if !v.is_finite() {
            *v = 0.0;
        }
    }
}

fn last<T>(v: &mut Vec<T>, n: usize) {
    if v.len() > n {
        v.drain(..v.len() - n);
    }
}

impl View {
    /// v cut to what a panel can show, or why it can't be.
    pub fn check(&mut self) -> Result<(), String> {
        clip(&mut self.title);
        clip(&mut self.summary);
        clip(&mut self.note);
        tone(&mut self.tone);
        check_actions(&mut self.actions);
        if !ACCENTS.contains(&self.accent.as_str()) {
            self.accent.clear();
        }
        self.blocks.truncate(BLOCKS_MOST);
        for (i, b) in self.blocks.iter_mut().enumerate() {
            b.check().map_err(|e| format!("block {}: {e}", i + 1))?;
        }
        if let Some(p) = &mut self.progress {
            clip(&mut p.label);
            tone(&mut p.tone);
            if p.total <= 0.0 {
                self.progress = None;
            }
        }
        if self.title.is_empty() {
            return Err(r#"a view needs a title: {"title": "Pull requests", …}"#.into());
        }
        self.stats.truncate(4);
        for s in &mut self.stats {
            clip(&mut s.value);
            clip(&mut s.label);
            tone(&mut s.tone);
            last(&mut s.trend, 40);
        }
        let mut left = VIEW_ROWS_MOST;
        for g in &mut self.groups {
            clip(&mut g.title);
            clip(&mut g.text);
            tone(&mut g.tone);
            g.rows.truncate(left);
            for r in &mut g.rows {
                clip(&mut r.text);
                clip(&mut r.meta);
                tone(&mut r.tone);
                if let Some(p) = &mut r.progress {
                    *p = p.clamp(0.0, 1.0);
                }
                if r.value.is_some_and(|p| p < 0.0 || !p.is_finite()) {
                    r.value = None; // a bar is a size: nought or more
                }
                web(&mut r.link);
                clip_detail(&mut r.detail);
                clip(&mut r.id);
                check_actions(&mut r.actions);
            }
            left -= g.rows.len();
        }
        Ok(())
    }
}

impl Block {
    /// b cut to what a panel can show, or why it can't be.
    fn check(&mut self) -> Result<(), String> {
        if !BLOCK_KINDS.contains(&self.kind.as_str()) {
            return Err(format!(
                "kind {}: use one of {}",
                go_quote(&self.kind),
                BLOCK_KINDS.join(", ")
            ));
        }
        clip(&mut self.title);
        tone(&mut self.tone);
        clip(&mut self.value);
        if self.style != "line" {
            self.style.clear();
        }
        if self.kind == "code" {
            let t = self.text.replace('\t', "    ");
            let lines: Vec<String> = t
                .split('\n')
                .take(CODE_MOST)
                .map(|l| {
                    let l: String = l.chars().filter(|&r| r >= ' ' && r != '\x7f').collect();
                    l.chars().take(VIEW_TEXT_MOST).collect()
                })
                .collect();
            self.text = lines.join("\n");
        } else {
            clip(&mut self.text);
        }
        self.columns.truncate(40);
        if self.kind == "table" {
            self.columns.truncate(CELLS_MOST);
        }
        self.columns.iter_mut().for_each(clip);
        self.labels.truncate(VALUES_MOST);
        self.labels.iter_mut().for_each(clip);
        last(&mut self.values, VALUES_MOST);
        finite(&mut self.values);
        last(&mut self.trend, 40);
        finite(&mut self.trend);
        self.stats.truncate(4);
        for s in &mut self.stats {
            clip(&mut s.value);
            clip(&mut s.label);
            tone(&mut s.tone);
            last(&mut s.trend, 40);
            finite(&mut s.trend);
        }
        if let Some(p) = &mut self.progress {
            clip(&mut p.label);
            tone(&mut p.tone);
            if p.total <= 0.0 {
                self.progress = None;
            }
        }
        let mut left = ITEMS_MOST;
        check_items(&mut self.items, 0, &mut left);
        Ok(())
    }
}

/// Items cut to size, down to 4 levels and left items in all, each
/// clipped and its link a web one.
fn check_items(items: &mut Vec<Item>, depth: usize, left: &mut usize) {
    if depth >= 4 || *left == 0 {
        items.clear();
        return;
    }
    items.truncate(*left);
    *left -= items.len();
    for it in items.iter_mut() {
        clip(&mut it.text);
        clip(&mut it.meta);
        clip(&mut it.time);
        clip(&mut it.from);
        clip(&mut it.to);
        tone(&mut it.tone);
        web(&mut it.link);
        if let Some(p) = &mut it.progress {
            *p = p.clamp(0.0, 1.0);
        }
        if it.value.is_some_and(|p| p < 0.0 || !p.is_finite()) {
            it.value = None;
        }
        it.cells.truncate(40);
        it.cells.iter_mut().for_each(clip);
        it.values.truncate(VALUES_MOST);
        finite(&mut it.values);
        clip_detail(&mut it.detail);
        clip(&mut it.id);
        check_actions(&mut it.actions);
        check_items(&mut it.items, depth + 1, left);
    }
}

fn read_view(p: &Path) -> Option<View> {
    let b = std::fs::read(p).ok()?;
    let v: View = serde_json::from_slice(&b).ok()?;
    (!v.title.is_empty()).then_some(v)
}

/// Puts the session's view of name away among its past ones, where you
/// can bring it back; none is no error.
pub fn drop_view(id: &str, name: &str) -> Result<(), String> {
    let from = views_dir(id).join(format!("{name}.json"));
    let b = match std::fs::read(&from) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
        Ok(b) => b,
    };
    if let Ok(mut v) = serde_json::from_slice::<View>(&b)
        && !v.title.is_empty()
    {
        v.gone = Time::now();
        v.pinned = false;
        let past = views_dir(id).join("past");
        std::fs::create_dir_all(&past).map_err(|e| e.to_string())?;
        let _ = crate::json::write_private(
            &past.join(format!("{}-{name}.json", v.gone.unix_nanos())),
            &crate::json::line(&v),
        );
        prune_past(&past);
    }
    std::fs::remove_file(&from).map_err(|e| e.to_string())
}

/// Keeps the newest PAST_MOST past views.
fn prune_past(past: &Path) {
    let mut names: Vec<String> = std::fs::read_dir(past)
        .map(|es| {
            es.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    if names.len() <= PAST_MOST {
        return;
    }
    // Named by when they went, so their names sort oldest first.
    names.sort_by_key(|n| past_key(n));
    for n in &names[..names.len() - PAST_MOST] {
        let _ = std::fs::remove_file(past.join(n));
    }
}

/// A past view's file name with its time padded, to sort by.
fn past_key(name: &str) -> String {
    let (t, rest) = name.split_once('-').unwrap_or((name, ""));
    format!("{t:0>20}-{rest}")
}

/// The session's views put away, the one gone last first.
pub fn past_views(id: &str) -> Vec<View> {
    let past = views_dir(id).join("past");
    let mut vs: Vec<View> = std::fs::read_dir(&past)
        .map(|es| {
            es.flatten()
                .filter_map(|e| {
                    let mut v = read_view(&e.path())?;
                    v.file = e.file_name().to_string_lossy().into_owned();
                    Some(v)
                })
                .collect()
        })
        .unwrap_or_default();
    vs.sort_by(|a, b| b.gone.cmp(&a.gone));
    vs
}

/// Shows a past view again, pinned: rush won't put it away by itself. It
/// takes the place of a view of its name showing now, which goes among
/// the past ones.
pub fn bring_back(id: &str, file: &str) -> Result<(), String> {
    if file.contains('/') || file.is_empty() || !file.ends_with(".json") {
        return Err(format!("no past view {file}"));
    }
    let from = views_dir(id).join("past").join(file);
    let b = std::fs::read(&from).map_err(|e| e.to_string())?;
    let mut v: View = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
    drop_view(id, &v.name)?;
    v.pinned = true;
    save_view(id, v)?;
    std::fs::remove_file(&from).map_err(|e| e.to_string())
}

/// Puts away the session's views the agent hasn't touched since since:
/// the start of its third turn back. Pinned ones stay.
pub fn put_away_stale(id: &str, since: Time) {
    let (vs, _) = views(id);
    for v in vs {
        if !v.pinned && v.by.is_empty() && v.updated.before(since) {
            let _ = drop_view(id, &v.name);
        }
    }
}

/// The session's views, the one updated last first, and when its folder
/// last changed, to read them again only then.
pub fn views(id: &str) -> (Vec<View>, Option<std::time::SystemTime>) {
    let d = views_dir(id);
    let Ok(st) = std::fs::metadata(&d) else {
        return (Vec::new(), None);
    };
    let mut vs: Vec<View> = std::fs::read_dir(&d)
        .map(|es| {
            es.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .filter_map(|e| {
                    let mut v = read_view(&e.path())?;
                    v.apply_yours();
                    Some(v)
                })
                .collect()
        })
        .unwrap_or_default();
    vs.sort_by(|a, b| b.updated.cmp(&a.updated));
    (vs, st.modified().ok())
}

/// When the session's views last changed: a view saved or cleared renames
/// a file in its folder, which the folder's time shows.
pub fn views_changed(id: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(views_dir(id)).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    // views_test.go:11: a view is kept, read back newest first, cut to
    // size, and cleared.
    #[test]
    fn views() {
        let _h = Home::new();
        std::fs::create_dir_all(dir("s1")).unwrap();
        let named = |n: &str, t: &str| View {
            name: n.into(),
            title: t.into(),
            ..View::default()
        };
        assert!(save_view("s1", named("Bad Name", "x")).is_err());
        assert!(save_view("s1", named("prs", "")).is_err());
        assert!(save_view("nope", named("prs", "x")).is_err());
        let v = View {
            name: "prs".into(),
            title: "Pull\nrequests".into(),
            summary: "8 ready · 2 failing".into(),
            stats: vec![
                Stat { value: "8".into(), label: "ready".into(), tone: "good".into(), ..Stat::default() },
                Stat { value: "2".into(), label: "failing".into(), tone: "purple".into(), ..Stat::default() },
            ],
            groups: vec![
                Group {
                    title: "Failing".into(),
                    rows: vec![
                        Row { text: format!("#8947 {}", "long ".repeat(100)), tone: "bad".into(), link: "https://github.com/o/r/pull/8947".into(), ..Row::default() },
                        Row { text: "#8945".into(), link: "file:///etc/passwd".into(), ..Row::default() },
                    ],
                    ..Group::default()
                },
                Group {
                    title: "Time".into(),
                    text: "per\ncheck".into(),
                    folded: true,
                    rows: vec![
                        Row { text: "e2e".into(), value: Some(9.0), ..Row::default() },
                        Row { text: "lint".into(), value: Some(-1.0), ..Row::default() },
                    ],
                    ..Group::default()
                },
            ],
            ..View::default()
        };
        save_view("s1", v).unwrap();
        save_view("s1", named("tests", "Tests")).unwrap();
        let (vs, _) = super::views("s1");
        assert_eq!(vs.len(), 2);
        assert_eq!((vs[0].name.as_str(), vs[1].name.as_str()), ("tests", "prs"));
        let p = &vs[1];
        assert_eq!(p.title, "Pull requests");
        assert_eq!(p.stats[1].tone, "");
        assert_eq!(p.groups[0].rows[0].text.chars().count(), VIEW_TEXT_MOST);
        let g = &p.groups[1];
        assert!(g.text == "per check" && g.folded && g.rows[0].value == Some(9.0) && g.rows[1].value.is_none());
        assert!(!p.groups[0].rows[0].link.is_empty() && p.groups[0].rows[1].link.is_empty());
        drop_view("s1", "tests").unwrap();
        assert_eq!(super::views("s1").0.len(), 1);
        for i in 0..VIEWS_MOST - 1 {
            save_view("s1", named(&format!("v{}", (b'a' + i as u8) as char), "x")).unwrap();
        }
        assert!(save_view("s1", named("toomany", "x")).is_err(), "a seventh view was kept");
        save_view("s1", named("prs", "again")).expect("updating one in place");
    }

    // views_test.go:74: cleared, a view goes among the past ones; one
    // untouched since a time is put away unless pinned; a past one comes
    // back pinned.
    #[test]
    fn past_views() {
        let _h = Home::new();
        std::fs::create_dir_all(dir("s1")).unwrap();
        save_view("s1", View { name: "old".into(), title: "Old".into(), ..View::default() }).unwrap();
        // A time between the two saves, rather than a sleep: the old one
        // is older than it, the new one newer.
        let since = Time::now();
        let mut stale = std::fs::read(views_dir("s1").join("old.json")).unwrap();
        let mut ov: View = serde_json::from_slice(&stale).unwrap();
        ov.updated = since.add_nanos(-1_000_000);
        stale = crate::json::line(&ov);
        std::fs::write(views_dir("s1").join("old.json"), stale).unwrap();
        let bad = View {
            name: "new".into(),
            title: "New".into(),
            blocks: vec![
                Block { kind: "hero".into(), value: "41".into(), ..Block::default() },
                Block { kind: "nope".into(), ..Block::default() },
            ],
            ..View::default()
        };
        assert!(save_view("s1", bad).is_err(), "a block of no kind was taken");
        assert_eq!(super::views("s1").0.len(), 1);
        put_away_stale("s1", since);
        let (vs, _) = super::views("s1");
        let past = past_views("s1");
        assert!(vs.is_empty() && past.len() == 1 && past[0].name == "old" && !past[0].gone.is_zero(), "{vs:?} {past:?}");
        bring_back("s1", &past[0].file).unwrap();
        let (vs, _) = super::views("s1");
        assert!(vs.len() == 1 && vs[0].pinned);
        assert!(past_views("s1").is_empty());
        put_away_stale("s1", Time::now().add_nanos(1_000_000_000));
        assert_eq!(super::views("s1").0.len(), 1, "a pinned view was put away");
    }

    #[test]
    fn writes_as_go_does() {
        for name in ["view.json", "view_empty.json"] {
            let want = std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
            let v: View = serde_json::from_str(&want).unwrap();
            assert_eq!(String::from_utf8(crate::json::line(&v)).unwrap(), want, "{name}");
        }
    }

    #[test]
    fn checks_as_go_does() {
        // The fixture is Go's check of a view with every part: Rust's check
        // of the same view writes the same.
        let want = std::fs::read_to_string(format!("{}/tests/fixtures/view.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let mut v: View = serde_json::from_str(&want).unwrap();
        v.title = "Pull\nrequests".into();
        v.groups[0].text = "a\nb".into();
        v.groups[0].rows[0].detail = "why\n  it failed".into();
        v.blocks[2].text = "a\tb\nc".into();
        v.check().unwrap();
        assert_eq!(String::from_utf8(crate::json::line(&v)).unwrap(), want);
    }
}
