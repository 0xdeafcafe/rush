//! Names what rush's keys do, so you can move them.
//!
//! Ported from internal/keymap/keymap.go:1-533 (this file),
//! internal/keymap/defaults.go (`defaults`) and internal/keymap/file.go
//! (`file`).
//!
//! Every key rush answers to in the Agents list, a Session or anywhere is an
//! [`Action`] with the keys it has by default. keybindings.json says which
//! keys you gave an action instead, and those can be chords: "ctrl+x d" is
//! ctrl+x then d. A plugin's commands and rush's own # commands are actions
//! too, with no key until you (or, where the key is free, the plugin) give
//! one.
//!
//! rush's key handling stays written against the default keys. A [`Map`]
//! sits in front of it and turns each key you press into the default key of
//! the action you bound it to: bind session.send to ctrl+x s and ctrl+x s
//! arrives as ctrl+enter. A default key you moved away from does nothing,
//! unless it types a character. An action with nothing to stand in for, a
//! command, is handed back to be run, as is one that Runs.

mod defaults;
mod file;

pub use defaults::{command_id, defaults, plugin_id};
pub use file::{File, load, load_from, path, save, save_to, state_dir};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

/// Where a key applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Context {
    /// Global keys work from anywhere, over sheets and the command bar too.
    Global,
    /// List is the Agents list and the Prompt under it.
    List,
    /// Prompt is the Prompt with something typed in it: its keys come
    /// before the list's, which have them back once it's empty.
    Prompt,
    /// Session is an agent's Session, its conversation and message box.
    Session,
    /// Pages is a place's pages where there's no list and Session to go
    /// between: Efficiency, Settings, Projects and the Wall.
    Pages,
    /// Any is for commands: they run the same from the list or a Session.
    Any,
}

/// Contexts in the order Settings shows them.
pub const CONTEXTS: [Context; 6] = [
    Context::Global,
    Context::List,
    Context::Prompt,
    Context::Session,
    Context::Pages,
    Context::Any,
];

impl Context {
    /// The context's name as Go writes it: "global", "list", …
    pub fn as_str(self) -> &'static str {
        match self {
            Context::Global => "global",
            Context::List => "list",
            Context::Prompt => "prompt",
            Context::Session => "session",
            Context::Pages => "pages",
            Context::Any => "any",
        }
    }

    /// The context named s, as [`Context::as_str`] writes it.
    pub fn from_name(s: &str) -> Option<Context> {
        CONTEXTS.into_iter().find(|c| c.as_str() == s)
    }

    /// How Settings names a context.
    pub fn title(self) -> &'static str {
        match self {
            Context::Global => "Everywhere",
            Context::List => "Agents and the Prompt",
            Context::Prompt => "The Prompt, with something typed",
            Context::Session => "A Session",
            Context::Pages => "Pages: Efficiency, Settings, Projects, Wall",
            Context::Any => "Commands",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Something a key can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// list.open, session.send, command:stash, plugin:haven.open
    pub id: String,
    pub context: Context,
    /// What it does, in a few words.
    pub title: String,
    /// Its default keys. The first that isn't a chord is the one a key bound
    /// to it stands in for; the rest are other keys rush already takes for
    /// it.
    pub keys: Vec<String>,
    /// Who added it: "" for rush, else the plugin's name.
    pub source: String,
    /// Handed back to be run, as a command is, though it has keys: for one
    /// whose key is another's in a context below it, or whose keys are all
    /// chords, so no key can stand in for it.
    pub runs: bool,
}

impl Action {
    /// An action of rush's own with no title, keys or source.
    pub fn new(id: impl Into<String>, context: Context) -> Action {
        Action {
            id: id.into(),
            context,
            title: String::new(),
            keys: Vec::new(),
            source: String::new(),
            runs: false,
        }
    }

    /// Whether running it means calling back rather than standing in for a
    /// default key.
    pub fn command(&self) -> bool {
        self.keys.is_empty() || self.runs
    }

    /// The default key its other keys arrive as: the first that isn't a
    /// chord, as rush's handling is written against one key.
    fn stand_in(&self) -> &str {
        self.keys
            .iter()
            .find(|k| !k.contains(' '))
            .map_or("", String::as_str)
    }

    fn has_key(&self, k: &str) -> bool {
        self.keys.iter().any(|x| x == k)
    }
}

/// A key sequence: one key, or a chord of several.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Seq(pub Vec<String>);

impl Seq {
    /// The keys, as a slice.
    pub fn keys(&self) -> &[String] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join(" "))
    }
}

impl<S: Into<String>> FromIterator<S> for Seq {
    fn from_iter<I: IntoIterator<Item = S>>(it: I) -> Seq {
        Seq(it.into_iter().map(Into::into).collect())
    }
}

/// Why a key sequence couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// s quoted as Go's %q (strconv.Quote) quotes it: printable characters as
/// they are, Go's short escapes, \xNN for other ASCII controls, and \uNNNN
/// or \UNNNNNNNN for other characters that don't print.
fn quote(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c if prints(c) => out.push(c),
            c if (c as u32) < 0x10000 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => {
                let _ = write!(out, "\\U{:08x}", c as u32);
            }
        }
    }
    out.push('"');
    out
}

/// Whether Go's unicode.IsPrint holds for c, near enough: no controls, no
/// space but ' ', no format characters or unassigned private use.
fn prints(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    !matches!(c as u32,
        0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x180e | 0x200b..=0x200f
        | 0x2028..=0x202e | 0x2060..=0x206f | 0xd800..=0xf8ff | 0xfeff
        | 0xfff9..=0xfffb | 0xfffe | 0xffff | 0xe0000..=0xe007f
        | 0xf0000..=0x10ffff)
}

/// Keys joined as a sequence is written: "ctrl+x d".
fn joined(s: &[String]) -> String {
    s.join(" ")
}

/// Reads a key sequence as keybindings.json writes it: keys by bubbletea's
/// names (ctrl+x, alt+shift+up, enter, space), a space between the keys of
/// a chord. cmd+ and meta+ are read as super+.
pub fn parse(s: &str) -> Result<Seq, ParseError> {
    let f: Vec<&str> = s.split_whitespace().collect();
    if f.is_empty() {
        return Err(ParseError("an empty key".into()));
    }
    if f.len() > 3 {
        return Err(ParseError(format!(
            "{}: a chord of at most 3 keys",
            quote(s)
        )));
    }
    let mut out = Vec::with_capacity(f.len());
    for k in f {
        match normal(k) {
            Ok(n) => out.push(n),
            Err(e) => return Err(ParseError(format!("{}: {e}", quote(s)))),
        }
    }
    Ok(Seq(out))
}

const MODS: [&str; 4] = ["ctrl", "alt", "shift", "super"];

/// Puts a key's modifiers in bubbletea's order, ctrl+alt+shift+super.
fn normal(k: &str) -> Result<String, String> {
    if k == "+" {
        return Ok(k.to_string());
    }
    let mut parts: Vec<&str> = k.split('+').collect();
    let mut base = parts[parts.len() - 1].to_string();
    if base.is_empty() {
        // "ctrl++" is ctrl and the + key.
        if k.ends_with("++") {
            parts.truncate(parts.len() - 2);
            base = "+".into();
        } else {
            return Err(format!("key {} has no key after its modifiers", quote(k)));
        }
    } else {
        parts.truncate(parts.len() - 1);
    }
    let mut has = [false; 4];
    for p in parts {
        let mut p = p.to_lowercase();
        match p.as_str() {
            "cmd" | "meta" | "command" => p = "super".into(),
            "option" | "opt" => p = "alt".into(),
            "control" => p = "ctrl".into(),
            _ => {}
        }
        let Some(i) = MODS.iter().position(|m| *m == p) else {
            return Err(format!(
                "key {}: {} is not a modifier (use ctrl, alt, shift or cmd)",
                quote(k),
                quote(&p)
            ));
        };
        has[i] = true;
    }
    if base.chars().count() > 1 {
        base = base.to_lowercase();
    }
    let mut b = String::new();
    for (i, m) in MODS.iter().enumerate() {
        if has[i] {
            b.push_str(m);
            b.push('+');
        }
    }
    b.push_str(&base);
    Ok(b)
}

/// Whether a key on its own types a character: one you can't take for an
/// action as the first key of a sequence, as the box would lose it.
pub fn types(k: &str) -> bool {
    if k == "space" {
        return true;
    }
    if k.contains('+') && k != "+" && !k.starts_with("shift+") {
        return false;
    }
    k.strip_prefix("shift+").unwrap_or(k).chars().count() == 1
}

/// A binding in the file that couldn't be used, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub action: String,
    pub key: String,
    pub why: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}: {}", self.action, self.why)
        } else {
            write!(f, "{} on {}: {}", self.action, self.key, self.why)
        }
    }
}

/// What a key does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    /// The key to hand on: the one pressed, or the default key of the action
    /// it's bound to. "" means nothing: it's dropped.
    pub key: String,
    /// A command action to run instead, when set.
    pub run: String,
    /// A chord begun: the keys so far, waiting for the next.
    pub pending: Seq,
    /// A chord that was begun but that no action finishes.
    pub missed: Seq,
}

/// The keys in force.
#[derive(Clone, Debug)]
pub struct Map {
    actions: HashMap<String, Action>,
    order: Vec<String>,
    /// Per context, each sequence's action.
    bound: [HashMap<String, String>; 6],
    /// Each action's sequences in force.
    keys: HashMap<String, Vec<Seq>>,
    /// Per context, default keys no longer bound to anything.
    freed: [HashSet<String>; 6],
    prefixes: [HashSet<String>; 6],
    /// The actions keybindings.json sets.
    yours: HashSet<String>,
    problems: Vec<Problem>,
}

/// The contexts an action's keys are looked up in.
fn scope(c: Context) -> &'static [Context] {
    match c {
        Context::Any => &[Context::List, Context::Session],
        Context::Global => &[Context::Global],
        Context::List => &[Context::List],
        Context::Prompt => &[Context::Prompt],
        Context::Session => &[Context::Session],
        Context::Pages => &[Context::Pages],
    }
}

/// The contexts whose keys an action's keys would shadow or be shadowed by.
fn clash(c: Context) -> &'static [Context] {
    match c {
        Context::Global => &[
            Context::Global,
            Context::List,
            Context::Prompt,
            Context::Session,
        ],
        Context::Any => &[Context::List, Context::Session, Context::Global],
        Context::List => &[Context::List, Context::Global],
        Context::Prompt => &[Context::Prompt, Context::Global],
        Context::Session => &[Context::Session, Context::Global],
        Context::Pages => &[],
    }
}

impl Map {
    /// Makes the Map from the actions, what you set, and what plugins
    /// suggest. Your bindings win over rush's defaults; a plugin's
    /// suggestion is taken only where the key is free.
    pub fn build(
        actions: &[Action],
        file: &File,
        suggested: &BTreeMap<String, Vec<String>>,
    ) -> Map {
        let mut m = Map {
            actions: HashMap::new(),
            order: Vec::new(),
            bound: Default::default(),
            keys: HashMap::new(),
            freed: Default::default(),
            prefixes: Default::default(),
            yours: HashSet::new(),
            problems: Vec::new(),
        };
        for a in actions {
            if m.actions.contains_key(&a.id) {
                continue;
            }
            m.actions.insert(a.id.clone(), a.clone());
            m.order.push(a.id.clone());
        }
        m.bind_defaults(file);
        m.bind_yours(file);
        m.bind_suggested(file, suggested);
        m.mark_freed();
        m
    }

    /// Binds rush's keys, except for actions you set.
    fn bind_defaults(&mut self, file: &File) {
        for i in 0..self.order.len() {
            let id = &self.order[i];
            if file.bindings.contains_key(id) {
                continue;
            }
            let a = self.actions[id].clone();
            for k in &a.keys {
                match parse(k) {
                    Ok(s) => self.bind(&a, s),
                    Err(e) => self.problems.push(Problem {
                        action: a.id.clone(),
                        key: k.clone(),
                        why: e.0,
                    }),
                }
            }
        }
    }

    /// Binds keybindings.json's keys, in a steady order, each taking its key
    /// from a default that had it.
    fn bind_yours(&mut self, file: &File) {
        // A BTreeMap is in name order already.
        for (id, keys) in &file.bindings {
            let Some(a) = self.actions.get(id).cloned() else {
                // A plugin that's gone, say. Kept in the file, not used.
                self.problems.push(Problem {
                    action: id.clone(),
                    key: String::new(),
                    why: "no such action".into(),
                });
                continue;
            };
            self.yours.insert(id.clone());
            for k in keys {
                self.bind_your(&a, k);
            }
        }
    }

    fn bind_your(&mut self, a: &Action, k: &str) {
        let s = match parse(k) {
            Ok(s) => s,
            Err(e) => {
                self.problems.push(Problem {
                    action: a.id.clone(),
                    key: k.to_string(),
                    why: e.0,
                });
                return;
            }
        };
        let (why, take) = self.cannot(a, &s, true);
        if !why.is_empty() {
            self.problems.push(Problem {
                action: a.id.clone(),
                key: s.to_string(),
                why,
            });
            return;
        }
        for (other, key) in take {
            self.unbind(&other, &key);
        }
        self.bind(a, s);
    }

    /// Binds what plugins suggest for their own commands, only where nothing
    /// else is.
    fn bind_suggested(&mut self, file: &File, suggested: &BTreeMap<String, Vec<String>>) {
        for (id, keys) in suggested {
            let Some(a) = self.actions.get(id).cloned() else {
                continue;
            };
            if a.source.is_empty() || file.bindings.contains_key(id) {
                continue;
            }
            for k in keys {
                if let Ok(s) = parse(k)
                    && self.cannot(&a, &s, false).0.is_empty()
                {
                    self.bind(&a, s);
                }
            }
        }
    }

    /// Notes the default keys that no longer do anything.
    fn mark_freed(&mut self) {
        for id in &self.order {
            let a = &self.actions[id];
            for k in &a.keys {
                let Ok(s) = parse(k) else { continue };
                if s.len() != 1 {
                    continue;
                }
                for c in scope(a.context) {
                    let i = c.index();
                    if !self.bound[i].contains_key(&s.0[0]) && !self.prefixes[i].contains(&s.0[0]) {
                        self.freed[i].insert(s.0[0].clone());
                    }
                }
            }
        }
    }

    /// Why a can't have s, or "". A binding of yours takes the key from a
    /// default that had it, so what it would take is returned to unbind.
    fn cannot(&self, a: &Action, s: &Seq, yours: bool) -> (String, Vec<(String, String)>) {
        if types(&s.0[0]) && a.context != Context::Global && !a.has_key(&s.0[0]) {
            return (
                "it types a character; start a chord with a key that doesn't, ctrl+x say".into(),
                Vec::new(),
            );
        }
        let mut take = Vec::new();
        let key = s.to_string();
        for c in clash(a.context) {
            let i = c.index();
            if let Some(other) = self.bound[i].get(&key)
                && *other != a.id
            {
                if !yours || self.yours.contains(other) {
                    return (format!("already {other}"), Vec::new());
                }
                take.push((other.clone(), key.clone()));
            }
            // A chord and a key that starts it can't both be bound.
            for n in 1..s.len() {
                let pre = joined(&s.0[..n]);
                if let Some(other) = self.bound[i].get(&pre)
                    && *other != a.id
                {
                    if !yours || self.yours.contains(other) {
                        return (
                            format!("{pre} is already {other}, so no chord can start with it"),
                            Vec::new(),
                        );
                    }
                    take.push((other.clone(), pre));
                }
            }
            if self.prefixes[i].contains(&key) {
                return (format!("a chord already starts with {key}"), Vec::new());
            }
        }
        (String::new(), take)
    }

    fn bind(&mut self, a: &Action, s: Seq) {
        for c in scope(a.context) {
            let i = c.index();
            self.bound[i].insert(s.to_string(), a.id.clone());
            for n in 1..s.len() {
                self.prefixes[i].insert(joined(&s.0[..n]));
            }
        }
        self.keys.entry(a.id.clone()).or_default().push(s);
    }

    fn unbind(&mut self, id: &str, key: &str) {
        let ctx = self.actions[id].context;
        for c in scope(ctx) {
            let i = c.index();
            if self.bound[i].get(key).is_some_and(|x| x == id) {
                self.bound[i].remove(key);
            }
        }
        if let Some(ks) = self.keys.get_mut(id) {
            ks.retain(|s| s.to_string() != key);
        }
    }

    /// The bindings in the file that aren't in force.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// Every action, in the order given.
    pub fn actions(&self) -> Vec<&Action> {
        self.order.iter().map(|id| &self.actions[id]).collect()
    }

    /// The action with id.
    pub fn action(&self, id: &str) -> Option<&Action> {
        self.actions.get(id)
    }

    /// The key sequences an action has now.
    pub fn keys(&self, id: &str) -> &[Seq] {
        self.keys.get(id).map_or(&[], Vec::as_slice)
    }

    /// An action's keys for a hint: "ctrl+s" or "ctrl+s · ctrl+x s", or ""
    /// for none.
    pub fn key_text(&self, id: &str) -> String {
        self.keys(id)
            .iter()
            .map(Seq::to_string)
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// Whether keybindings.json sets the action's keys.
    pub fn changed(&self, id: &str) -> bool {
        self.yours.contains(id)
    }

    /// What key does in ctxs (the most specific first; Global is always
    /// looked at last), after the keys of a chord pending.
    pub fn resolve(&self, ctxs: &[Context], pending: &[String], key: &str) -> Resolved {
        let mut seq = pending.to_vec();
        seq.push(key.to_string());
        let s = seq.join(" ");
        let all = || ctxs.iter().copied().chain(std::iter::once(Context::Global));
        for c in all() {
            if let Some(id) = self.bound[c.index()].get(&s) {
                let a = &self.actions[id];
                if a.command() {
                    return Resolved {
                        run: id.clone(),
                        ..Default::default()
                    };
                }
                // One of its own keys arrives as it is: rush's handling may
                // tell them apart (ctrl+\ moves even from a box).
                if seq.len() == 1 && a.has_key(&s) {
                    return Resolved {
                        key: s,
                        ..Default::default()
                    };
                }
                return Resolved {
                    key: a.stand_in().to_string(),
                    ..Default::default()
                };
            }
        }
        for c in all() {
            if self.prefixes[c.index()].contains(&s) {
                return Resolved {
                    pending: Seq(seq),
                    ..Default::default()
                };
            }
        }
        if !pending.is_empty() {
            return Resolved {
                missed: Seq(seq),
                ..Default::default()
            };
        }
        for c in all() {
            if self.freed[c.index()].contains(key) && !types(key) {
                return Resolved::default();
            }
        }
        Resolved {
            key: key.to_string(),
            ..Default::default()
        }
    }

    /// Whether any chord in ctxs starts with key, so a caller can tell a key
    /// that begins one from any other.
    pub fn starts(&self, ctxs: &[Context], key: &str) -> bool {
        ctxs.iter()
            .copied()
            .chain(std::iter::once(Context::Global))
            .any(|c| self.prefixes[c.index()].contains(key))
    }

    /// What a new binding of s to id would clash with, for Settings to show
    /// before it's saved.
    pub fn conflicts(&self, id: &str, s: &Seq) -> Vec<String> {
        let Some(a) = self.actions.get(id) else {
            return Vec::new();
        };
        if s.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if types(&s.0[0]) && a.context != Context::Global && !a.has_key(&s.0[0]) {
            out.push("types a character".to_string());
        }
        let key = s.to_string();
        let chord = format!("{key} ");
        for c in clash(a.context) {
            let bound = &self.bound[c.index()];
            if let Some(other) = bound.get(&key)
                && other != id
            {
                out.push(other.clone());
            }
            for n in 1..s.len() {
                if let Some(other) = bound.get(&joined(&s.0[..n]))
                    && other != id
                {
                    out.push(other.clone());
                }
            }
            for (k, other) in bound {
                if other != id && k.starts_with(&chord) {
                    out.push(other.clone());
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests;
