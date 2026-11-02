//! `plugin.json`: what a plugin is, and everything it asks for. rush shows
//! all of it at approval, and the plugin runs with only what it says.
//!
//! [`Manifest::parse`] reads one as rush does, refusing fields it doesn't
//! know, and [`Manifest::check`] makes the checks of rush's that need
//! nothing but the file. `rush plugin check <name>` is the whole check.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The categories rush lists plugins under, in its order.
pub const CATEGORIES: &[&str] = &[
    "messages",
    "voice",
    "agents",
    "code",
    "pull-requests",
    "machine",
    "views",
    "memory",
    "integrations",
    "other",
];

/// What `sessions` may ask for.
pub const SESSIONS: &[&str] = &[
    "list", "start", "read", "send", "control", "queue", "queued",
];

/// What `ui` may ask for.
pub const UI: &[&str] = &[
    "events",
    "input",
    "intercept",
    "overview",
    "notify",
    "send",
    "views",
];

/// A plugin's manifest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Manifest {
    /// `^[a-z][a-z0-9-]{0,30}$`, and its folder's name.
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// One of [`CATEGORIES`]; none is `other`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// The program, a path in the plugin's folder or absolute, and its
    /// arguments.
    pub command: Vec<String>,
    /// `rush` (none) or `mcp`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub protocol: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Offer its tools to every rush-mode session.
    #[serde(skip_serializing_if = "is_false")]
    pub tools: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<String>,
    /// `host:port` pairs it may reach, through rush's proxy.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub write: Vec<String>,
    /// Programs rush runs for it, outside the sandbox, by name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub exec: BTreeMap<String, Vec<String>>,
    /// Its memory limit; 0 is rush's default, 256.
    #[serde(rename = "memoryMB", skip_serializing_if = "is_zero")]
    pub memory_mb: i64,
    /// Subagents, as Claude Code's `--agents` takes them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, Value>,
    /// Added to every rush-mode session's system prompt.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    #[serde(skip_serializing_if = "is_false")]
    pub sidebar: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ui: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<Command>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<Setting>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cli: Vec<Cli>,
    #[serde(skip_serializing_if = "Requires::is_empty")]
    pub requires: Requires,
    /// Paths, relative to its folder, whose changes mean an update.
    #[serde(rename = "builtFrom", skip_serializing_if = "Vec::is_empty")]
    pub built_from: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub uses: Vec<String>,
}

/// One of its commands, run as a `ui.command` request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Command {
    pub name: String,
    pub description: String,
    /// A key it suggests, taken only if free.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key: String,
}

/// One of its settings, under Settings, Plugins.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Setting {
    pub key: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// `bool`, `choice` or `text`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub default: String,
}

/// One of its commands on rush's CLI, `rush <plugin> <name>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cli {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub usage: String,
    pub description: String,
}

/// What it needs to run at all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Requires {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub arch: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bin: Vec<String>,
}

impl Requires {
    pub fn is_empty(&self) -> bool {
        self.os.is_empty() && self.arch.is_empty() && self.bin.is_empty()
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Manifest {
    /// Reads a manifest, refusing a field rush doesn't know: a typo must
    /// not drop a restriction.
    pub fn parse(s: &str) -> Result<Manifest, String> {
        serde_json::from_str(s).map_err(|e| format!("plugin.json: {e}"))
    }

    /// Reads dir's plugin.json, and checks it, and that it's named after
    /// dir.
    pub fn load(dir: &Path) -> Result<Manifest, String> {
        let path = dir.join("plugin.json");
        let s = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let m = Manifest::parse(&s)?;
        m.check()?;
        let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if m.name != folder {
            return Err(format!("named {:?} in a folder named {folder:?}", m.name));
        }
        Ok(m)
    }

    /// Whether it speaks MCP on stdio rather than rush's protocol on fd 3.
    pub fn is_mcp(&self) -> bool {
        self.protocol == "mcp"
    }

    /// The checks of rush's that need only the manifest. The first thing
    /// wrong, in words.
    pub fn check(&self) -> Result<(), String> {
        if !name_ok(&self.name, 31) {
            return Err(format!(
                "name {:?} isn't lowercase letters, digits and dashes, starting with a letter, at most 31",
                self.name
            ));
        }
        let Some(prog) = self.command.first().filter(|p| !p.is_empty()) else {
            return Err("command names no program".into());
        };
        if !prog.starts_with('/') && prog.split('/').any(|p| p == "..") {
            return Err(format!("command {prog:?} reaches out of its folder"));
        }
        if !matches!(self.protocol.as_str(), "" | "rush" | "mcp") {
            return Err(format!("protocol {:?}: rush or mcp", self.protocol));
        }
        if !self.category.is_empty() && !CATEGORIES.contains(&self.category.as_str()) {
            return Err(format!(
                "category {:?}: one of {}",
                self.category,
                CATEGORIES.join(", ")
            ));
        }
        for s in &self.sessions {
            if !SESSIONS.contains(&s.as_str()) {
                return Err(format!("sessions: no such capability {s:?}"));
            }
        }
        let needs_workspace = ["start", "queue", "queued"]
            .iter()
            .any(|c| self.sessions.iter().any(|s| s == c))
            || self.ui.iter().any(|u| u == "send");
        if needs_workspace && self.workspaces.is_empty() {
            return Err("it starts or reaches agents, so it needs workspaces".into());
        }
        for w in &self.workspaces {
            if w == "/" || !(w.starts_with('/') || w.starts_with("~/")) {
                return Err(format!("workspace {w:?}: an absolute path or ~/…, not /"));
            }
        }
        for u in &self.ui {
            if !UI.contains(&u.as_str()) {
                return Err(format!("ui: no such capability {u:?}"));
            }
        }
        let has = |u: &str| self.ui.iter().any(|x| x == u);
        if has("intercept") && !has("input") {
            return Err("ui intercept needs input".into());
        }
        if has("send") && !has("events") {
            return Err("ui send needs events".into());
        }
        if self.memory_mb != 0 && !(1..=8192).contains(&self.memory_mb) {
            return Err(format!("memoryMB {}: 1 to 8192", self.memory_mb));
        }
        for n in self.exec.keys() {
            if !name_ok(n, 41) {
                return Err(format!("exec name {n:?}"));
            }
        }
        for n in self.agents.keys() {
            if !name_ok(n, 41) {
                return Err(format!("agent name {n:?}"));
            }
        }
        if self.commands.len() > 32 {
            return Err("more than 32 commands".into());
        }
        let mut seen = HashSet::new();
        for c in &self.commands {
            if !name_ok(&c.name, 31) || !seen.insert(c.name.as_str()) {
                return Err(format!("command name {:?}", c.name));
            }
            if c.description.chars().count() > 200 {
                return Err(format!(
                    "command {}: description over 200 characters",
                    c.name
                ));
            }
        }
        if self.settings.len() > 32 {
            return Err("more than 32 settings".into());
        }
        let mut seen = HashSet::new();
        for s in &self.settings {
            if !name_ok(&s.key, 31) || !seen.insert(s.key.as_str()) {
                return Err(format!("setting key {:?}", s.key));
            }
            if s.title.is_empty() || s.title.chars().count() > 60 {
                return Err(format!("setting {}: a title of 1 to 60 characters", s.key));
            }
            match s.kind.as_str() {
                "bool" if matches!(s.default.as_str(), "" | "true" | "false") => {}
                "choice"
                    if (2..=16).contains(&s.choices.len())
                        && (s.default.is_empty() || s.choices.contains(&s.default)) => {}
                "text" if s.default.len() <= 1000 => {}
                k => return Err(format!("setting {}: type {k:?} with that default", s.key)),
            }
        }
        if self.is_mcp() {
            let rush_only = [
                ("sessions", !self.sessions.is_empty()),
                ("exec", !self.exec.is_empty()),
                ("sidebar", self.sidebar),
                ("ui", !self.ui.is_empty()),
                ("commands", !self.commands.is_empty()),
                ("settings", !self.settings.is_empty()),
                ("cli", !self.cli.is_empty()),
                ("provides", !self.provides.is_empty()),
            ];
            if let Some((what, _)) = rush_only.iter().find(|(_, set)| *set) {
                return Err(format!("an mcp plugin can't have {what}"));
            }
        }
        if self.prompt.len() > 16 << 10 {
            return Err("prompt over 16 KB".into());
        }
        Ok(())
    }
}

/// `^[a-z][a-z0-9-]{0,max-1}$`.
fn name_ok(s: &str, max: usize) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= max
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_checks() {
        let m = Manifest::parse(
            r#"{"name": "notes", "command": ["notes"], "tools": true, "sessions": ["list"],
                "memoryMB": 32, "settings": [{"key": "keep", "title": "Keep", "type": "choice",
                "choices": ["10", "50"], "default": "50"}]}"#,
        )
        .unwrap();
        m.check().unwrap();
        assert_eq!(m.memory_mb, 32);
        assert!(!m.is_mcp());
    }

    #[test]
    fn refuses_what_rush_refuses() {
        for (json, want) in [
            (
                r#"{"name": "x", "command": ["x"], "tool": true}"#,
                "unknown field",
            ),
            (r#"{"name": "X", "command": ["x"]}"#, "name"),
            (r#"{"name": "x", "command": []}"#, "no program"),
            (r#"{"name": "x", "command": ["../x"]}"#, "out of its folder"),
            (
                r#"{"name": "x", "command": ["x"], "sessions": ["start"]}"#,
                "workspaces",
            ),
            (
                r#"{"name": "x", "command": ["x"], "ui": ["intercept"]}"#,
                "needs input",
            ),
            (
                r#"{"name": "x", "command": ["x"], "category": "toys"}"#,
                "category",
            ),
            (
                r#"{"name": "x", "command": ["x"], "protocol": "mcp", "sessions": ["list"]}"#,
                "mcp",
            ),
        ] {
            let e = Manifest::parse(json).and_then(|m| m.check()).unwrap_err();
            assert!(e.contains(want), "{json}: {e}");
        }
    }
}
