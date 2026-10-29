//! Claude Code as efficiency's Source: its transcripts, read line by line
//! into efficiency's figures, and its setup, for savers to be found in.
//! Port of internal/adapters/claude/efficiency.go. Its tools' kinds and
//! inputs are read through the registry's native agent (Claude Code's
//! adapter), as everything else in rush reads them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rush_core::adapter::{Profile, global};
use rush_core::agent::TokenUsage;
use rush_core::tool::{Input, Kind};
use rush_state::Time;
use rush_state::settingsfile;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::scan::{Compact, File};
use crate::source::{Setup, Source};

/// Claude Code's transcripts and setup.
pub struct Claude;

/// The variables a Claude Code session started rush with, which aren't
/// another session's to inherit (claude/account.go inherited).
const INHERITED: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
    "CLAUDE_JOB_DIR",
    "CLAUDE_CODE_VERSION",
];

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn is_default(dir: &Path) -> bool {
    dir == home().join(".claude")
}

/// The account's state file: ~/.claude.json for the default, else inside
/// its folder.
fn state_path(dir: &Path) -> PathBuf {
    if is_default(dir) {
        let mut s = dir.as_os_str().to_owned();
        s.push(".json");
        return s.into();
    }
    dir.join(".claude.json")
}

/// The environment Claude Code runs in for the account in dir.
fn account_env(dir: &Path) -> Vec<String> {
    let mut env: Vec<String> = std::env::vars()
        .filter(|(k, _)| k != "CLAUDE_CONFIG_DIR" && !INHERITED.contains(&k.as_str()))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if !is_default(dir) {
        env.push(format!("CLAUDE_CONFIG_DIR={}", dir.display()));
    }
    env
}

/// The .jsonl files in dir.
fn jsonl_in(dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<String> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    found.sort();
    out.extend(found);
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

impl Source for Claude {
    /// p's transcripts: projects/*/*.jsonl, then their subagents'
    /// projects/*/*/subagents/*.jsonl.
    fn transcripts(&self, p: &Profile) -> Vec<String> {
        let dir = PathBuf::from(&p.dir).join("projects");
        let projects = subdirs(&dir);
        let mut main = Vec::new();
        for proj in &projects {
            jsonl_in(proj, &mut main);
        }
        for proj in &projects {
            for sess in subdirs(proj) {
                jsonl_in(&sess.join("subagents"), &mut main);
            }
        }
        main
    }

    fn transcripts_dir(&self, p: &Profile) -> String {
        PathBuf::from(&p.dir)
            .join("projects")
            .to_string_lossy()
            .into_owned()
    }

    fn read_line(&self, f: &mut File, b: &[u8]) {
        read_line(f, b);
    }

    /// p's settings, plugins and MCP servers.
    fn setup(&self, p: &Profile) -> Setup {
        let dir = PathBuf::from(&p.dir);
        let mut s = Setup {
            backup: vec![
                dir.join("settings.json").to_string_lossy().into_owned(),
                dir.join("CLAUDE.md").to_string_lossy().into_owned(),
                state_path(&dir).to_string_lossy().into_owned(),
            ],
            env: account_env(&dir),
            ..Setup::default()
        };
        let settings = settingsfile::File::load(dir.join("settings.json"))
            .or_else(|_| settingsfile::File::load("/dev/null"))
            .ok();
        if let Some(st) = &settings {
            type Hooks = HashMap<String, Vec<HookGroup>>;
            #[derive(Deserialize)]
            struct HookGroup {
                #[serde(default)]
                hooks: Vec<Hook>,
            }
            #[derive(Deserialize)]
            struct Hook {
                #[serde(default)]
                command: String,
            }
            let hooks: Hooks = st.get("hooks").unwrap_or_default();
            let mut events: Vec<&String> = hooks.keys().collect();
            events.sort();
            for e in events {
                for g in &hooks[e] {
                    s.hooks.extend(g.hooks.iter().map(|h| h.command.clone()));
                }
            }
            s.enabled = st.get("enabledPlugins").unwrap_or_default();
        }
        s.settings = settings;
        #[derive(Deserialize, Default)]
        struct Installed {
            #[serde(default)]
            plugins: HashMap<String, Vec<InstalledPlugin>>,
        }
        #[derive(Deserialize)]
        struct InstalledPlugin {
            #[serde(default)]
            scope: String,
            #[serde(default, rename = "installedAt")]
            installed_at: Time,
        }
        let inst: Installed = std::fs::read(dir.join("plugins").join("installed_plugins.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for (id, list) in inst.plugins {
            for pl in list {
                let have = s.plugins.get(&id).copied().unwrap_or_default();
                if pl.scope == "user" || have.is_zero() {
                    s.plugins.insert(id.clone(), pl.installed_at);
                }
            }
        }
        #[derive(Deserialize, Default)]
        struct State {
            #[serde(default, rename = "mcpServers")]
            mcp: HashMap<String, Box<RawValue>>,
        }
        let st: State = std::fs::read(state_path(&dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for name in st.mcp.into_keys() {
            s.mcp.insert(name, true);
        }
        s
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawLine<'a> {
    #[serde(rename = "type")]
    kind: String,
    subtype: String,
    timestamp: Time,
    cwd: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(borrow)]
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
    id: String,
    model: String,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Usage {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    speed: String,
    output_tokens_details: Option<Details>,
    cache_creation: Option<CacheBreakup>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Details {
    thinking_tokens: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CacheBreakup {
    ephemeral_5m_input_tokens: i64,
    ephemeral_1h_input_tokens: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Attachment {
    #[serde(rename = "type")]
    kind: String,
    command: String,
    skills: Vec<Named>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Named {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CompactMeta {
    trigger: String,
    #[serde(rename = "preTokens")]
    pre: i64,
    #[serde(rename = "postTokens")]
    post: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Block<'a> {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    name: String,
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    tool_use_id: String,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
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

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn interesting(b: &[u8]) -> bool {
    MARKERS.iter().any(|m| contains(b, m))
}

/// Reads one transcript line into f.
pub fn read_line(f: &mut File, b: &[u8]) {
    if !interesting(b) {
        return;
    }
    let Ok(l) = serde_json::from_slice::<RawLine>(b) else {
        return;
    };
    let at = l.timestamp;
    if at.is_zero() {
        return;
    }
    f.saw(at, &l.session_id, &l.cwd);
    match l.kind.as_str() {
        "assistant" => assistant(f, &l, at),
        "user" => user(f, &l, at),
        "attachment" => {
            let Some(a) = &l.attachment else { return };
            match a.kind.as_str() {
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
            if l.subtype == "compact_boundary"
                && let Some(c) = &l.compact
            {
                f.compacted(Compact {
                    at,
                    auto: c.trigger == "auto",
                    pre: c.pre,
                    post: c.post,
                });
            }
            if let Some(c) = l.content
                && let Ok(s) = serde_json::from_str::<String>(c.get())
            {
                slash(f, &s, at);
            }
        }
        _ => {}
    }
}

fn slash(f: &mut File, s: &str, at: Time) {
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

/// The tool's kind and its input as rush's, through Claude Code's
/// adapter; Other with no input when it isn't registered.
fn tool_of(id: &str, name: &str, input: Option<&RawValue>) -> (Kind, Input) {
    let Some(n) = global().get(crate::AGENT) else {
        return (Kind::Other, Input::default());
    };
    let Some(native) = n.native() else {
        return (Kind::Other, Input::default());
    };
    let empty = RawValue::from_string("{}".into()).expect("{} is JSON");
    let c = native.call(id, name, input.unwrap_or(&empty));
    (native.kind_of(name), c.input)
}

fn assistant(f: &mut File, l: &RawLine, at: Time) {
    let m = &l.message;
    if let Some(u) = &m.usage
        && !m.model.is_empty()
        && m.model != "<synthetic>"
    {
        let mut tu = TokenUsage {
            input: u.input_tokens,
            output: u.output_tokens,
            cache_read: u.cache_read_input_tokens,
            ..TokenUsage::default()
        };
        match &u.cache_creation {
            Some(cb) if cb.ephemeral_5m_input_tokens + cb.ephemeral_1h_input_tokens > 0 => {
                tu.cache_write_5m = cb.ephemeral_5m_input_tokens;
                tu.cache_write_1h = cb.ephemeral_1h_input_tokens;
            }
            _ => tu.cache_write_5m = u.cache_creation_input_tokens,
        }
        let think = u
            .output_tokens_details
            .as_ref()
            .map_or(0, |d| d.thinking_tokens);
        f.request(&m.id, &m.model, tu, think, u.speed == "fast", at);
    }
    let Some(content) = m.content else { return };
    if !content.get().contains(r#""tool_use""#) {
        return;
    }
    let Ok(blocks) = serde_json::from_str::<Vec<Block>>(content.get()) else {
        return;
    };
    for bl in blocks.iter().filter(|b| b.kind == "tool_use") {
        if bl.name == "Skill" {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct SkillIn {
                skill: String,
            }
            if let Some(i) = bl.input
                && let Ok(s) = serde_json::from_str::<SkillIn>(i.get())
                && !s.skill.is_empty()
            {
                f.skill(&s.skill, at);
            }
        } else if let Some(rest) = bl.name.strip_prefix("mcp__") {
            let server = rest.split_once("__").map_or(rest, |(s, _)| s);
            f.mcp(server, at);
        }
        let (kind, input) = tool_of(&bl.id, &bl.name, bl.input);
        f.call(&bl.id, kind, &bl.name, &input, at);
    }
}

fn user(f: &mut File, l: &RawLine, at: Time) {
    let Some(c) = l.message.content else { return };
    let raw = c.get();
    if raw.starts_with('"') {
        if let Ok(s) = serde_json::from_str::<String>(raw) {
            slash(f, &s, at);
        }
        return;
    }
    if !raw.contains(r#""tool_result""#) {
        return;
    }
    let Ok(blocks) = serde_json::from_str::<Vec<Block>>(raw) else {
        return;
    };
    for bl in blocks.iter().filter(|b| b.kind == "tool_result") {
        f.result(&bl.tool_use_id, result_size(bl.content), at);
    }
}

/// How much text a tool result gave back; images count as nothing, being
/// tokens of another kind.
fn result_size(raw: Option<&RawValue>) -> i64 {
    let Some(raw) = raw else { return 0 };
    let raw = raw.get();
    if raw.is_empty() {
        return 0;
    }
    if raw.starts_with('"') {
        return raw.len() as i64 - 2;
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Part {
        text: String,
    }
    match serde_json::from_str::<Vec<Part>>(raw) {
        Ok(parts) => parts.iter().map(|p| p.text.len() as i64).sum(),
        Err(_) => raw.len() as i64,
    }
}
