//! Claude Code as a source of efficiency figures: its transcripts, read
//! line by line into whatever counts them, and its setup, for savers to be
//! found in. Port of internal/adapters/claude/efficiency.go.
//!
//! The efficiency module itself (Go's internal/efficiency) isn't ported
//! yet: what a line says goes to a [`Figures`], which its File will be.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use rush_core::adapter::Profile;
use rush_core::agent::TokenUsage;
use rush_core::time::{Ms, parse_rfc3339};
use rush_core::tool;
use rush_state::settingsfile::File as Settings;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::account::Account;
use crate::commands::installed_plugins;
use crate::json::{Str, contains, nd};
use crate::tools;

/// What reads a transcript's figures: Go's efficiency.File.
pub trait Figures {
    /// A line of session's, in cwd, at at.
    fn saw(&mut self, at: Ms, session: &str, cwd: &str);
    /// One request to the model, its usage as written.
    fn request(&mut self, id: &str, model: &str, u: TokenUsage, think: i64, fast: bool, at: Ms);
    fn call(&mut self, id: &str, kind: tool::Kind, name: &str, input: &tool::Input, at: Ms);
    /// A tool's result, n bytes of text.
    fn result(&mut self, id: &str, n: i64, at: Ms);
    fn skill(&mut self, name: &str, at: Ms);
    fn mcp(&mut self, server: &str, at: Ms);
    fn command(&mut self, name: &str, at: Ms);
    fn hook(&mut self, cmd: &str, at: Ms);
    fn compacted(&mut self, c: Compact);
}

/// A compaction, as its transcript tells it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compact {
    pub at: Ms,
    pub auto: bool,
    pub pre: i64,
    pub post: i64,
}

/// A profile's setup: its settings, plugins and MCP servers.
pub struct Setup {
    /// Its own settings.
    pub settings: Settings,
    /// Every hook's command.
    pub hooks: Vec<String>,
    /// Plugins turned on, by id.
    pub enabled: BTreeMap<String, bool>,
    /// Plugins installed, and when.
    pub plugins: BTreeMap<String, Ms>,
    /// MCP servers, by name.
    pub mcp: BTreeMap<String, bool>,
    /// The files an install may change, copied first.
    pub backup: Vec<String>,
    /// The environment its CLI runs in for that home.
    pub env: Vec<String>,
}

/// p's transcripts: sessions' and their subagents'.
pub fn transcripts(p: &Profile) -> Vec<String> {
    let dir = Account::at(&p.name, &p.dir).projects_dir();
    let mut main = Vec::new();
    let mut subs = Vec::new();
    for proj in sorted(&dir) {
        for f in sorted(&proj) {
            if f.extension().is_some_and(|x| x == "jsonl") && f.is_file() {
                main.push(f.to_string_lossy().into_owned());
            } else if f.is_dir() {
                for s in sorted(&f.join("subagents")) {
                    if s.extension().is_some_and(|x| x == "jsonl") {
                        subs.push(s.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    main.extend(subs);
    main
}

fn sorted(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// p's settings, plugins and MCP servers.
pub fn setup(p: &Profile) -> Setup {
    let a = Account::at(&p.name, &p.dir);
    // Settings that can't be read are none: a file that isn't there.
    let settings = crate::settings::load_settings(&a).unwrap_or_else(|_| {
        Settings::load("/nonexistent/settings.json").expect("a missing file is empty")
    });
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Hook {
        #[serde(deserialize_with = "nd")]
        command: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Group {
        #[serde(deserialize_with = "nd")]
        hooks: Vec<Hook>,
    }
    let mut hooks = Vec::new();
    if let Some(all) = settings.get::<BTreeMap<String, Vec<Group>>>("hooks") {
        for groups in all.into_values() {
            for g in groups {
                hooks.extend(g.hooks.into_iter().map(|h| h.command));
            }
        }
    }
    let enabled = settings.get::<BTreeMap<String, bool>>("enabledPlugins").unwrap_or_default();
    let mut plugins: BTreeMap<String, Ms> = BTreeMap::new();
    for (id, list) in installed_plugins(&a.config_dir) {
        for pl in list {
            if pl.scope == "user" || plugins.get(&id).is_none_or(|&t| t == 0) {
                plugins.insert(id.clone(), parse_rfc3339(&pl.installed_at));
            }
        }
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct St {
        #[serde(rename = "mcpServers")]
        mcp: Option<BTreeMap<String, Box<RawValue>>>,
    }
    let mcp = fs::read(a.state_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<St>(&b).ok())
        .and_then(|s| s.mcp)
        .unwrap_or_default()
        .into_keys()
        .map(|k| (k, true))
        .collect();
    Setup {
        backup: vec![
            a.config_dir.join("settings.json").to_string_lossy().into_owned(),
            a.config_dir.join("CLAUDE.md").to_string_lossy().into_owned(),
            a.state_path().to_string_lossy().into_owned(),
        ],
        env: a.env(),
        settings,
        hooks,
        enabled,
        plugins,
        mcp,
    }
}

/// The lines worth decoding carry one of these; the rest (most of the
/// bytes: snapshots, progress, queue operations) are skipped unread.
const MARKERS: &[&[u8]] = &[
    br#""type":"assistant""#,
    br#""tool_result""#,
    br#""hook_success""#,
    br#""compact_boundary""#,
    br#""invoked_skills""#,
    b"<command-name>",
];

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawLine<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(borrow)]
    subtype: Str<'a>,
    #[serde(borrow)]
    timestamp: Str<'a>,
    #[serde(borrow)]
    cwd: Str<'a>,
    #[serde(rename = "sessionId", borrow)]
    session_id: Str<'a>,
    #[serde(borrow, deserialize_with = "nd")]
    message: Msg<'a>,
    attachment: Option<Attachment>,
    #[serde(rename = "compactMetadata")]
    compact: Option<CompactMeta>,
    /// A system line's.
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Msg<'a> {
    #[serde(borrow)]
    id: Str<'a>,
    #[serde(borrow)]
    model: Str<'a>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Usage {
    #[serde(rename = "input_tokens", deserialize_with = "nd")]
    input: i64,
    #[serde(rename = "output_tokens", deserialize_with = "nd")]
    output: i64,
    #[serde(rename = "cache_read_input_tokens", deserialize_with = "nd")]
    cache_read: i64,
    #[serde(rename = "cache_creation_input_tokens", deserialize_with = "nd")]
    cache_create: i64,
    #[serde(deserialize_with = "nd")]
    speed: String,
    #[serde(rename = "output_tokens_details")]
    details: Option<Details>,
    #[serde(rename = "cache_creation")]
    breakup: Option<Breakup>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Details {
    #[serde(rename = "thinking_tokens", deserialize_with = "nd")]
    thinking: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Breakup {
    #[serde(rename = "ephemeral_5m_input_tokens", deserialize_with = "nd")]
    m5: i64,
    #[serde(rename = "ephemeral_1h_input_tokens", deserialize_with = "nd")]
    h1: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Attachment {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    #[serde(deserialize_with = "nd")]
    command: String,
    #[serde(deserialize_with = "nd")]
    skills: Vec<Named>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Named {
    #[serde(deserialize_with = "nd")]
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CompactMeta {
    #[serde(deserialize_with = "nd")]
    trigger: String,
    #[serde(rename = "preTokens", deserialize_with = "nd")]
    pre: i64,
    #[serde(rename = "postTokens", deserialize_with = "nd")]
    post: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Block<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(borrow)]
    id: Str<'a>,
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    #[serde(borrow)]
    tool_use_id: Str<'a>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

/// Reads one transcript line into f.
pub fn read_line(f: &mut dyn Figures, b: &[u8]) {
    if !MARKERS.iter().any(|m| contains(b, m)) {
        return;
    }
    let Ok(l) = serde_json::from_slice::<RawLine>(b) else {
        return;
    };
    let at = parse_rfc3339(&l.timestamp);
    if at == 0 {
        return;
    }
    f.saw(at, &l.session_id, &l.cwd);
    match &*l.typ {
        "assistant" => assistant(f, &l.message, at),
        "user" => user(f, l.message.content, at),
        "attachment" => {
            let Some(a) = &l.attachment else { return };
            match a.typ.as_str() {
                "hook_success" if !a.command.is_empty() => f.hook(&a.command, at),
                "invoked_skills" => {
                    for s in &a.skills {
                        f.skill(&s.name, at);
                    }
                }
                _ => {}
            }
        }
        "system" => {
            if &*l.subtype == "compact_boundary"
                && let Some(c) = &l.compact
            {
                f.compacted(Compact {
                    at,
                    auto: c.trigger == "auto",
                    pre: c.pre,
                    post: c.post,
                });
            }
            if let Some(s) = crate::json::str_of(l.content) {
                slash(f, &s, at);
            }
        }
        _ => {}
    }
}

fn slash(f: &mut dyn Figures, s: &str, at: Ms) {
    let Some((_, rest)) = s.split_once("<command-name>") else {
        return;
    };
    if let Some((name, _)) = rest.split_once("</command-name>")
        && !name.is_empty()
        && name.len() < 64
    {
        f.command(name, at);
    }
}

fn assistant(f: &mut dyn Figures, m: &Msg, at: Ms) {
    if let Some(u) = &m.usage
        && !m.model.is_empty()
        && &*m.model != "<synthetic>"
    {
        let mut tu = TokenUsage {
            input: u.input,
            output: u.output,
            cache_read: u.cache_read,
            ..TokenUsage::default()
        };
        match &u.breakup {
            Some(cb) if cb.m5 + cb.h1 > 0 => (tu.cache_write_5m, tu.cache_write_1h) = (cb.m5, cb.h1),
            _ => tu.cache_write_5m = u.cache_create,
        }
        let think = u.details.as_ref().map_or(0, |d| d.thinking);
        f.request(&m.id, &m.model, tu, think, u.speed == "fast", at);
    }
    let Some(c) = m.content else { return };
    if !c.get().contains(r#""tool_use""#) {
        return;
    }
    let Ok(blocks) = serde_json::from_str::<Vec<Block>>(c.get()) else {
        return;
    };
    for bl in blocks {
        if &*bl.typ != "tool_use" {
            continue;
        }
        if &*bl.name == "Skill" {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct S {
                #[serde(deserialize_with = "nd")]
                skill: String,
            }
            if let Some(s) = bl.input.and_then(|v| serde_json::from_str::<S>(v.get()).ok())
                && !s.skill.is_empty()
            {
                f.skill(&s.skill, at);
            }
        } else if let Some(rest) = bl.name.strip_prefix("mcp__") {
            let server = rest.split_once("__").map_or(rest, |(s, _)| s);
            f.mcp(server, at);
        }
        let c = tools::call(&bl.id, &bl.name, bl.input);
        f.call(&bl.id, tools::kind_of(&bl.name), &bl.name, &c.input, at);
    }
}

fn user(f: &mut dyn Figures, c: Option<&RawValue>, at: Ms) {
    let Some(c) = c else { return };
    if c.get().starts_with('"') {
        if let Some(s) = crate::json::str_of(Some(c)) {
            slash(f, &s, at);
        }
        return;
    }
    if !c.get().contains(r#""tool_result""#) {
        return;
    }
    let Ok(blocks) = serde_json::from_str::<Vec<Block>>(c.get()) else {
        return;
    };
    for bl in blocks {
        if &*bl.typ == "tool_result" {
            f.result(&bl.tool_use_id, result_size(bl.content), at);
        }
    }
}

/// How much text a tool result gave back; images count as nothing, being
/// tokens of another kind.
fn result_size(raw: Option<&RawValue>) -> i64 {
    let Some(raw) = raw else { return 0 };
    let s = raw.get();
    if s.is_empty() {
        return 0;
    }
    if s.starts_with('"') {
        return s.len() as i64 - 2;
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Part {
        #[serde(deserialize_with = "nd")]
        text: String,
    }
    match serde_json::from_str::<Vec<Part>>(s) {
        Ok(parts) => parts.iter().map(|p| p.text.len() as i64).sum(),
        Err(_) => s.len() as i64,
    }
}

/// A Figures that keeps what it's told, in words: for tests, and to look
/// at a transcript by hand.
#[derive(Default, Debug)]
pub struct Told(pub Vec<String>, pub HashMap<String, i64>);

impl Figures for Told {
    fn saw(&mut self, _: Ms, session: &str, _: &str) {
        *self.1.entry(format!("session:{session}")).or_default() += 1;
    }
    fn request(&mut self, id: &str, model: &str, u: TokenUsage, think: i64, fast: bool, _: Ms) {
        self.0.push(format!(
            "request {id} {model} in={} out={} read={} w5m={} w1h={} think={think} fast={fast}",
            u.input, u.output, u.cache_read, u.cache_write_5m, u.cache_write_1h
        ));
    }
    fn call(&mut self, id: &str, kind: tool::Kind, name: &str, _: &tool::Input, _: Ms) {
        self.0.push(format!("call {id} {name} {kind:?}"));
    }
    fn result(&mut self, id: &str, n: i64, _: Ms) {
        self.0.push(format!("result {id} {n}"));
    }
    fn skill(&mut self, name: &str, _: Ms) {
        self.0.push(format!("skill {name}"));
    }
    fn mcp(&mut self, server: &str, _: Ms) {
        self.0.push(format!("mcp {server}"));
    }
    fn command(&mut self, name: &str, _: Ms) {
        self.0.push(format!("command {name}"));
    }
    fn hook(&mut self, cmd: &str, _: Ms) {
        self.0.push(format!("hook {cmd}"));
    }
    fn compacted(&mut self, c: Compact) {
        self.0.push(format!("compacted auto={} {}→{}", c.auto, c.pre, c.post));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_counts() {
        let lines = [
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","sessionId":"s","cwd":"/w","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":9,"cache_creation":{"ephemeral_5m_input_tokens":4,"ephemeral_1h_input_tokens":5},"speed":"fast","output_tokens_details":{"thinking_tokens":6}},"content":[{"type":"tool_use","id":"t1","name":"Skill","input":{"skill":"pdf"}},{"type":"tool_use","id":"t2","name":"mcp__linear__get_issue","input":{}}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-01T10:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"hello"},{"type":"image"}]},{"type":"tool_result","tool_use_id":"t2","content":"abc"}]}}"#,
            r#"{"type":"attachment","timestamp":"2026-10-01T10:00:02Z","attachment":{"type":"hook_success","command":"rtk hook"}}"#,
            r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-10-01T10:00:03Z","compactMetadata":{"trigger":"auto","preTokens":100,"postTokens":10},"content":"Conversation compacted"}"#,
            r#"{"type":"user","timestamp":"2026-10-01T10:00:04Z","message":{"content":"<command-name>/review</command-name>"}}"#,
            r#"{"type":"assistant","message":{"model":"x","usage":{}}}"#,
            r#"{"type":"progress","timestamp":"2026-10-01T10:00:05Z"}"#,
        ];
        let mut t = Told::default();
        for l in lines {
            read_line(&mut t, l.as_bytes());
        }
        assert_eq!(
            t.0,
            vec![
                "request m1 claude-opus-5-5 in=1 out=2 read=3 w5m=4 w1h=5 think=6 fast=true",
                "skill pdf",
                "call t1 Skill Other",
                "mcp linear",
                "call t2 mcp__linear__get_issue Mcp",
                "result t1 5",
                "result t2 3",
                "hook rtk hook",
                "compacted auto=true 100→10",
                "command /review",
            ]
        );
    }

    #[test]
    fn lists_transcripts_and_setup() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path();
        fs::create_dir_all(cfg.join("projects/-w/s1/subagents")).unwrap();
        fs::write(cfg.join("projects/-w/s1.jsonl"), "").unwrap();
        fs::write(cfg.join("projects/-w/s1/subagents/agent-a.jsonl"), "").unwrap();
        fs::write(
            cfg.join("settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"command":"say done"}]}]},"enabledPlugins":{"p@m":true}}"#,
        )
        .unwrap();
        fs::write(cfg.join(".claude.json"), r#"{"mcpServers":{"linear":{}}}"#).unwrap();
        let p = Profile {
            kind: "claude".into(),
            name: String::new(),
            dir: cfg.to_string_lossy().into_owned(),
        };
        let ts = transcripts(&p);
        assert_eq!(ts.len(), 2);
        assert!(ts[0].ends_with("s1.jsonl") && ts[1].ends_with("agent-a.jsonl"));
        let s = setup(&p);
        assert_eq!(s.hooks, vec!["say done"]);
        assert_eq!(s.enabled.get("p@m"), Some(&true));
        assert!(s.mcp.contains_key("linear"));
        assert_eq!(s.backup.len(), 3);
    }
}
