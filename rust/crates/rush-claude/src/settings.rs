//! Claude Code's settings files: which a session reads, its status line,
//! where it compacts, whether it credits itself in commits, and what
//! rush's Settings page offers of them; its agent definitions. Port of
//! internal/adapters/claude/settings.go, attribution.go, settingspage.go,
//! claude/settings.go and claude/compact.go.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use rush_core::adapter::{
    AgentDef, Compaction, EnvSetting, Profile, SettingRow, SettingType, SettingsFile, SettingsPage,
};
use rush_state::settingsfile::File as Settings;
use serde_json::{Value, json};

use crate::account::Account;
use crate::adapter::KIND;

/// acct's settings.json; a missing file is an empty one.
pub fn load_settings(acct: &Account) -> io::Result<Settings> {
    Settings::load(acct.config_dir.join("settings.json"))
}

/// Yours, then the project's (shared through git) and the project's
/// just-for-you one, where Claude Code saves "don't ask again".
pub fn settings_files(p: &Profile, cwd: &str) -> Vec<SettingsFile> {
    let mut files = vec![SettingsFile {
        label: "yours".into(),
        path: join(&p.dir, "settings.json"),
    }];
    let mut root = rush_sys::actions::repo_root(cwd);
    if root.is_empty() {
        root = cwd.to_string();
    }
    if !root.is_empty() {
        let dot = Path::new(&root).join(".claude");
        files.push(SettingsFile {
            label: "project".into(),
            path: dot.join("settings.json").to_string_lossy().into_owned(),
        });
        files.push(SettingsFile {
            label: "project · just you".into(),
            path: dot
                .join("settings.local.json")
                .to_string_lossy()
                .into_owned(),
        });
    }
    files
}

fn join(dir: &str, name: &str) -> String {
    Path::new(dir).join(name).to_string_lossy().into_owned()
}

/// The command in p's settings.json.
pub fn status_line(p: &Profile) -> io::Result<String> {
    Ok(Settings::load(join(&p.dir, "settings.json"))?.string("statusLine.command"))
}

/// Writes command into p's settings.json, flush to the left; "" takes it
/// out.
pub fn set_status_line(p: &Profile, command: &str) -> io::Result<()> {
    let mut s = Settings::load(join(&p.dir, "settings.json"))?;
    if command.is_empty() {
        s.delete("statusLine");
    } else {
        s.set(
            "statusLine",
            &json!({"type": "command", "command": command, "padding": 0}),
        )?;
    }
    s.save()
}

/// The folder Claude Code ran the status line for: CLAUDE_CONFIG_DIR's,
/// else ~/.claude.
pub fn status_line_profile() -> Profile {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(dir) if !dir.is_empty() => Profile {
            kind: KIND.into(),
            name: String::new(),
            dir,
        },
        _ => crate::adapter::profile(&Account::home()),
    }
}

/// Claude Code's state file: ~/.claude.json for the default config folder,
/// or .claude.json inside any other.
pub fn claude_json(cfg: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && Path::new(cfg) == Path::new(&home).join(".claude") {
        return Path::new(&home).join(".claude.json");
    }
    Path::new(cfg).join(".claude.json")
}

/// Where a session on p in cwd compacts, from its process's env and the
/// settings files it reads: yours, then the project's and its
/// just-for-you one.
pub fn compaction_for(p: &Profile, cwd: &str, env: &[String]) -> Compaction {
    let mut src = CompactSource {
        env: env.to_vec(),
        state: read_settings_cached(&claude_json(&p.dir)),
        settings: Vec::new(),
    };
    let mut paths = vec![PathBuf::from(join(&p.dir, "settings.json"))];
    let root = project_root(cwd);
    if !root.is_empty() {
        let dot = Path::new(&root).join(".claude");
        paths.push(dot.join("settings.json"));
        paths.push(dot.join("settings.local.json"));
    }
    for path in paths {
        src.settings.push(read_settings_cached(&path));
    }
    compaction(&src)
}

/// The checkout cwd is in, found by its .git without running git (the list
/// asks for every row); else cwd itself.
pub fn project_root(cwd: &str) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    let mut dir = Path::new(cwd);
    loop {
        if fs::symlink_metadata(dir.join(".git")).is_ok() {
            return dir.to_string_lossy().into_owned();
        }
        match dir.parent() {
            Some(up) if up != dir => dir = up,
            _ => return cwd.to_string(),
        }
    }
}

// Claude Code's auto-compact window: CLAUDE_CODE_AUTO_COMPACT_WINDOW, else
// the settings' autoCompactWindow, else the model's own window. Compaction
// starts short of it by the room kept for a reply (the model's output
// limit, at most OUTPUT_ROOM) and a buffer of COMPACT_BUFFER more.
const MIN_COMPACT_WINDOW: i64 = 100_000;
const MAX_COMPACT_WINDOW: i64 = 1_000_000;
const OUTPUT_ROOM: i64 = 20_000;
const COMPACT_BUFFER: i64 = 13_000;

/// What a session's auto-compact is read from: its process's
/// environment, then each settings file it reads in order (a later one
/// wins), and Claude Code's state file for whether it's on.
#[derive(Default)]
pub struct CompactSource {
    pub env: Vec<String>,
    pub settings: Vec<Option<Arc<Settings>>>,
    pub state: Option<Arc<Settings>>,
}

/// Where Claude Code compacts a session with src, as it works it out: none
/// when auto-compact is off or no window is set apart from the model's.
pub fn compaction(src: &CompactSource) -> Compaction {
    // Only the variables below are asked of it.
    let mut env: HashMap<String, String> = HashMap::new();
    for e in &src.env {
        if let Some((k, v)) = e.split_once('=')
            && compact_var(k)
        {
            env.insert(k.to_string(), v.to_string());
        }
    }
    // A settings file's env block goes into the session's environment
    // over what it was started with.
    let mut setting = 0i64;
    for s in src.settings.iter().flatten() {
        if let Some(block) = s.get::<BTreeMap<String, Value>>("env") {
            for (k, v) in block {
                if compact_var(&k) {
                    env.insert(k, env_string(&v));
                }
            }
        }
        if let Some(n) = s.get::<f64>("autoCompactWindow")
            && n == (n as i64) as f64
            && (MIN_COMPACT_WINDOW as f64..=MAX_COMPACT_WINDOW as f64).contains(&n)
        {
            setting = n as i64;
        }
    }
    let var = |k: &str| env.get(k).map_or("", String::as_str);
    if truthy(var("DISABLE_COMPACT")) || truthy(var("DISABLE_AUTO_COMPACT")) {
        return Compaction::default();
    }
    if let Some(st) = &src.state
        && st.get::<bool>("autoCompactEnabled") == Some(false)
    {
        return Compaction::default();
    }
    let mut room = OUTPUT_ROOM;
    if let Some(n) = leading_int(var("CLAUDE_CODE_MAX_OUTPUT_TOKENS"))
        && n > 0
    {
        room = room.min(n);
    }
    let mut window = setting;
    let v = var("CLAUDE_CODE_AUTO_COMPACT_WINDOW");
    if !v.is_empty()
        && let Some(n) = leading_int(v)
        && n > 0
    {
        window = n.clamp(MIN_COMPACT_WINDOW, MAX_COMPACT_WINDOW);
    }
    if window == 0 {
        return Compaction::default();
    }
    Compaction {
        window,
        headroom: room + COMPACT_BUFFER,
    }
}

/// Whether compaction reads environment variable k.
fn compact_var(k: &str) -> bool {
    matches!(
        k,
        "DISABLE_COMPACT"
            | "DISABLE_AUTO_COMPACT"
            | "CLAUDE_CODE_MAX_OUTPUT_TOKENS"
            | "CLAUDE_CODE_AUTO_COMPACT_WINDOW"
    )
}

/// JavaScript's parseInt(v, 10): the digits v starts with, after spaces
/// and a sign.
fn leading_int(v: &str) -> Option<i64> {
    let v = v.trim_start_matches([' ', '\t', '\n', '\r']);
    let b = v.as_bytes();
    let mut end = 0;
    if end < b.len() && (b[end] == b'-' || b[end] == b'+') {
        end += 1;
    }
    let digits = end;
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
    }
    if end == digits {
        return None;
    }
    v[..end].parse().ok()
}

fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// A settings env value as the string the session's environment has.
fn env_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{}", f as i64),
            Some(f) => f.to_string(),
            None => n.to_string(),
        },
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Settings files as last read, by path, read again only once they
/// change: the list asks for every row on every reading.
static SETTINGS_MEMO: LazyLock<Mutex<HashMap<PathBuf, MemoFile>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct MemoFile {
    modified: Option<SystemTime>,
    size: u64,
    file: Option<Arc<Settings>>,
}

/// A settings file, kept while it doesn't change. Missing or unreadable
/// is None.
pub fn read_settings_cached(path: &Path) -> Option<Arc<Settings>> {
    let fi = fs::metadata(path).ok()?;
    let modified = fi.modified().ok();
    let mut memo = SETTINGS_MEMO.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = memo.get(path)
        && m.modified == modified
        && m.size == fi.len()
    {
        return m.file.clone();
    }
    let file = Settings::load(path).ok().map(Arc::new);
    memo.insert(
        path.to_path_buf(),
        MemoFile {
            modified,
            size: fi.len(),
            file: file.clone(),
        },
    );
    file
}

// Claude Code credits itself in commits and pull requests unless its
// settings' attribution says otherwise: an empty commit and pr text adds
// nothing. includeCoAuthoredBy is the older word for the commit's, still
// read.

/// Whether Claude Code with its config in dir credits itself, and
/// whether its settings say.
pub fn attribution_in(dir: &str) -> (bool, bool) {
    let Ok(s) = Settings::load(join(dir, "settings.json")) else {
        return (false, false);
    };
    let commit = s.get::<String>("attribution.commit");
    let pr = s.get::<String>("attribution.pr");
    if commit.is_some() || pr.is_some() {
        let said = |v: Option<String>| v.is_some_and(|v| !v.is_empty());
        return (said(commit) || said(pr), true);
    }
    if let Some(co) = s.get::<bool>("includeCoAuthoredBy") {
        return (co, true);
    }
    (false, false)
}

/// Says in dir's settings whether Claude Code credits itself: off empties
/// its texts, on removes them so it adds its own.
pub fn set_attribution_in(dir: &str, on: bool) -> io::Result<()> {
    let mut s = Settings::load(join(dir, "settings.json"))?;
    if on {
        s.delete("attribution.commit");
        s.delete("attribution.pr");
        s.delete("includeCoAuthoredBy");
    } else {
        s.set("attribution.commit", "")?;
        s.set("attribution.pr", "")?;
        s.set("includeCoAuthoredBy", &false)?;
    }
    s.save()
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// What settings.json and its env block offer: the keys people change
/// most, and the variables worth a choice.
pub fn settings_page() -> SettingsPage {
    let models = strs(&["opus", "opus[1m]", "sonnet", "haiku", "fable"]);
    let row =
        |label: &str, key: &str, what: &str, kind: SettingType, choices: Vec<String>| SettingRow {
            label: label.into(),
            key: key.into(),
            what: what.into(),
            kind,
            choices,
        };
    let env = |label: &str, name: &str, what: &str, choices: &[&str], means: &[(&str, &str)]| {
        EnvSetting {
            label: label.into(),
            name: name.into(),
            what: what.into(),
            choices: strs(choices),
            means: means
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    };
    use SettingType::{Flag, Int, Off};
    let text = SettingType::String;
    SettingsPage {
        note: "every Claude Code session reads it, not only rush's".into(),
        unset: "Claude Code's own default".into(),
        rows: vec![
            row(
                "Default model",
                "model",
                "The model every session on this account starts with, unless a session or rush picks one",
                text,
                models,
            ),
            row(
                "Default effort",
                "effortLevel",
                "How hard sessions think by default",
                text,
                strs(&["low", "medium", "high", "xhigh", "max"]),
            ),
            row(
                "Permission mode",
                "permissions.defaultMode",
                "What sessions may do without asking. Your allow and deny rules are kept",
                text,
                strs(&["default", "acceptEdits", "plan", "auto"]),
            ),
            row(
                "Always think",
                "alwaysThinkingEnabled",
                "Extended thinking on every request",
                Flag,
                Vec::new(),
            ),
            row(
                "Co-authored-by in commits",
                "includeCoAuthoredBy",
                "Whether Claude adds a Co-authored-by line to commits it makes",
                Flag,
                Vec::new(),
            ),
            row(
                "Keep transcripts for",
                "cleanupPeriodDays",
                "How many days Claude Code keeps old transcripts before cleaning them up",
                Int,
                strs(&["7", "30", "90", "365"]),
            ),
            row(
                "Hooks",
                "disableAllHooks",
                "Whether the hooks in your settings run. Turning them off stops every hook without deleting them",
                Off,
                Vec::new(),
            ),
        ],
        env_key: "env".into(),
        env: vec![
            env(
                "Subagent model",
                "CLAUDE_CODE_SUBAGENT_MODEL",
                "The model subagents use, whatever the main session runs",
                &["haiku", "sonnet", "opus"],
                &[],
            ),
            env(
                "Max subagents",
                "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS",
                "How many subagents a session may run at once; rush picks by the machine when unset",
                &["20", "40", "80"],
                &[],
            ),
            env(
                "Max output tokens",
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                "The longest single reply Claude may write",
                &["32000", "64000", "128000"],
                &[],
            ),
            env(
                "Bash timeout",
                "BASH_DEFAULT_TIMEOUT_MS",
                "How long a shell command may run before it's stopped",
                &["120000", "300000", "600000"],
                &[
                    ("120000", "2 minutes."),
                    ("300000", "5 minutes."),
                    ("600000", "10 minutes."),
                ],
            ),
            env(
                "Telemetry",
                "DISABLE_TELEMETRY",
                "Whether Claude Code sends usage telemetry",
                &["1"],
                &[("1", "off: no telemetry is sent.")],
            ),
            env(
                "Non-essential traffic",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
                "Whether Claude Code makes non-essential network calls such as update checks",
                &["1"],
                &[("1", "off: only the calls a session needs.")],
            ),
        ],
    }
}

/// Claude Code's own agent, then the markdown definitions in p's agents
/// folder and every project's .claude/agents.
pub fn agent_defs(p: &Profile, roots: &[String]) -> Vec<AgentDef> {
    let mut dirs: Vec<(String, PathBuf)> = vec![("user".into(), Path::new(&p.dir).join("agents"))];
    let mut seen = std::collections::HashSet::new();
    for r in roots {
        if r.is_empty() || !seen.insert(r.as_str()) {
            continue;
        }
        let d = Path::new(r).join(".claude").join("agents");
        if d.is_dir() {
            let base = Path::new(r)
                .file_name()
                .map_or_else(|| r.clone(), |b| b.to_string_lossy().into_owned());
            dirs.push((base, d));
        }
    }
    let mut out = vec![AgentDef {
        name: "claude".into(),
        scope: "built-in".into(),
        desc: "Claude Code's default agent".into(),
        ..AgentDef::default()
    }];
    for (scope, d) in dirs {
        for f in glob_md(&d) {
            let mut def = read_agent_def(&f);
            def.scope = scope.clone();
            out.push(def);
        }
    }
    out
}

/// The .md files in dir, by name, as filepath.Glob(dir/*.md).
fn glob_md(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    out.sort();
    out
}

const AGENT_TEMPLATE: &str = "---
name: {name}
description: When to use this agent, in one or two sentences.
model: inherit
---

What this agent does, and how.
";

/// p's agents/name.md, from a template if it isn't there.
pub fn new_agent_def(p: &Profile, name: &str) -> io::Result<String> {
    let dir = Path::new(&p.dir).join("agents");
    let path = dir.join(format!("{name}.md"));
    let ps = path.to_string_lossy().into_owned();
    if path.exists() {
        return Ok(ps);
    }
    fs::create_dir_all(&dir)?;
    fs::write(&path, AGENT_TEMPLATE.replace("{name}", name))?;
    Ok(ps)
}

/// Reads a definition's front matter.
pub fn read_agent_def(path: &Path) -> AgentDef {
    let mut def = AgentDef {
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_string_lossy().into_owned(),
        ..AgentDef::default()
    };
    let Ok(f) = fs::File::open(path) else {
        return def;
    };
    let (mut in_front, mut in_desc) = (false, false);
    for l in BufReader::new(f).lines().take(60) {
        let Ok(l) = l else { break };
        if l.trim() == "---" {
            if in_front {
                break;
            }
            in_front = true;
            continue;
        }
        if !in_front {
            continue;
        }
        if let Some(v) = l.strip_prefix("name:") {
            def.name = v.trim().to_string();
            in_desc = false;
        } else if let Some(v) = l.strip_prefix("model:") {
            def.model = v.trim().to_string();
            in_desc = false;
        } else if let Some(v) = l.strip_prefix("description:") {
            let v = v.trim();
            in_desc = v == "|" || v == ">" || v.is_empty();
            if !in_desc {
                def.desc = v.trim_matches(['"', '\'']).to_string();
            }
        } else if in_desc && l.starts_with(' ') {
            if def.desc.is_empty() {
                def.desc = l.trim().to_string();
            }
        } else {
            in_desc = false;
        }
    }
    def
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_of(dir: &Path, n: usize, body: &str) -> Option<Arc<Settings>> {
        let path = dir.join(format!("settings-{n}.json"));
        fs::write(&path, body).unwrap();
        Some(Arc::new(Settings::load(path).unwrap()))
    }

    // compact_test.go:26 (TestCompaction)
    #[test]
    fn compaction_table() {
        let c = |w: i64, h: i64| Compaction {
            window: w,
            headroom: h,
        };
        let none = Compaction::default();
        type Case<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a str, Compaction);
        let cases: &[Case] = &[
            ("nothing set is the model's window", &[], &[], "", none),
            (
                "the environment",
                &["CLAUDE_CODE_AUTO_COMPACT_WINDOW=400000"],
                &[],
                "",
                c(400_000, 33_000),
            ),
            (
                "a settings env block",
                &[],
                &[r#"{"env":{"CLAUDE_CODE_AUTO_COMPACT_WINDOW":"400000"}}"#],
                "",
                c(400_000, 33_000),
            ),
            (
                "a settings env block over the process's",
                &["CLAUDE_CODE_AUTO_COMPACT_WINDOW=200000"],
                &[r#"{"env":{"CLAUDE_CODE_AUTO_COMPACT_WINDOW":"300000"}}"#],
                "",
                c(300_000, 33_000),
            ),
            (
                "the project's just-yours file last",
                &[],
                &[
                    r#"{"autoCompactWindow":500000}"#,
                    r#"{"autoCompactWindow":300000}"#,
                    r#"{"autoCompactWindow":250000}"#,
                ],
                "",
                c(250_000, 33_000),
            ),
            (
                "the environment over autoCompactWindow",
                &["CLAUDE_CODE_AUTO_COMPACT_WINDOW=600000"],
                &[r#"{"autoCompactWindow":300000}"#],
                "",
                c(600_000, 33_000),
            ),
            (
                "an invalid environment leaves autoCompactWindow",
                &["CLAUDE_CODE_AUTO_COMPACT_WINDOW=big"],
                &[r#"{"autoCompactWindow":300000}"#],
                "",
                c(300_000, 33_000),
            ),
            (
                "a setting out of range is dropped",
                &[],
                &[r#"{"autoCompactWindow":50000}"#],
                "",
                none,
            ),
            (
                "too small is raised to the least",
                &["CLAUDE_CODE_AUTO_COMPACT_WINDOW=50000"],
                &[],
                "",
                c(100_000, 33_000),
            ),
            (
                "too big is capped",
                &["CLAUDE_CODE_AUTO_COMPACT_WINDOW=5000000"],
                &[],
                "",
                c(1_000_000, 33_000),
            ),
            (
                "parsed as parseInt does",
                &["CLAUDE_CODE_AUTO_COMPACT_WINDOW=400k"],
                &[],
                "",
                c(100_000, 33_000),
            ),
            (
                "a smaller output limit leaves less room",
                &[
                    "CLAUDE_CODE_AUTO_COMPACT_WINDOW=400000",
                    "CLAUDE_CODE_MAX_OUTPUT_TOKENS=8000",
                ],
                &[],
                "",
                c(400_000, 21_000),
            ),
            (
                "auto-compact off",
                &[
                    "CLAUDE_CODE_AUTO_COMPACT_WINDOW=400000",
                    "DISABLE_AUTO_COMPACT=1",
                ],
                &[],
                "",
                none,
            ),
            (
                "compaction off",
                &[],
                &[r#"{"env":{"DISABLE_COMPACT":"true"},"autoCompactWindow":300000}"#],
                "",
                none,
            ),
            (
                "auto-compact off in /config",
                &["CLAUDE_CODE_AUTO_COMPACT_WINDOW=400000"],
                &[],
                r#"{"autoCompactEnabled":false}"#,
                none,
            ),
        ];
        for (name, env, settings, state, want) in cases {
            let dir = tempfile::tempdir().unwrap();
            let mut src = CompactSource {
                env: env.iter().map(|s| s.to_string()).collect(),
                ..CompactSource::default()
            };
            for (i, s) in settings.iter().enumerate() {
                src.settings.push(settings_of(dir.path(), i, s));
            }
            if !state.is_empty() {
                src.state = settings_of(dir.path(), 99, state);
            }
            assert_eq!(compaction(&src), *want, "{name}");
        }
    }

    // compact_test.go:67 (TestReadSettingsCached)
    #[test]
    fn read_settings_cached_reads_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert!(read_settings_cached(&path).is_none(), "a missing file");
        fs::write(&path, r#"{"autoCompactWindow":300000}"#).unwrap();
        let first = read_settings_cached(&path).unwrap();
        assert!(
            Arc::ptr_eq(&first, &read_settings_cached(&path).unwrap()),
            "an unchanged file is read once"
        );
        fs::write(&path, r#"{"autoCompactWindow":400000,"model":"opus"}"#).unwrap();
        let s = read_settings_cached(&path).unwrap();
        assert_eq!(
            s.get::<i64>("autoCompactWindow"),
            Some(400_000),
            "a changed file is read again"
        );
    }

    // attribution_test.go
    #[test]
    fn attribution_off_and_on_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_string_lossy().into_owned();
        let p = dir.path().join("settings.json");
        fs::write(
            &p,
            r#"{"model":"opus[1m]","attribution":{"sessionUrl":false}}"#,
        )
        .unwrap();
        assert!(!attribution_in(&d).1, "nothing said yet");
        set_attribution_in(&d, false).unwrap();
        assert_eq!(attribution_in(&d), (false, true));
        let b = fs::read_to_string(&p).unwrap();
        assert!(
            b.contains(r#""sessionUrl": false"#) && b.contains(r#""opus[1m]""#),
            "the rest is kept:\n{b}"
        );
        set_attribution_in(&d, true).unwrap();
        assert!(
            !attribution_in(&d).1,
            "on leaves Claude Code to its default:\n{}",
            fs::read_to_string(&p).unwrap()
        );
    }

    // settings_test.go (TestSettingsKeepUnknownKeys, TestSettingsMissingFile)
    #[test]
    fn settings_keep_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let orig = r#"{
  "model": "opus",
  "statusLine": {"type": "command", "command": "~/bin/status"},
  "permissions": {"allow": ["Bash(go test:*)"], "defaultMode": "default"},
  "env": {"FOO": "bar"},
  "enabledPlugins": {"design@x": true}
}"#;
        fs::write(&path, orig).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let a = Account::at("", dir.path());
        let mut s = load_settings(&a).unwrap();
        assert_eq!(s.string("model"), "opus");
        assert_eq!(s.string("permissions.defaultMode"), "default");
        assert_eq!(s.env().get("FOO").map(String::as_str), Some("bar"));
        s.set("model", "sonnet").unwrap();
        s.set("permissions.defaultMode", "plan").unwrap();
        s.set_env("CLAUDE_CODE_SUBAGENT_MODEL", "haiku").unwrap();
        s.set_env("FOO", "").unwrap();
        s.set("effortLevel", "high").unwrap();
        s.save().unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "mode changed"
        );
        let got: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(got["model"], "sonnet");
        assert_eq!(got["effortLevel"], "high");
        assert_eq!(got["permissions"]["defaultMode"], "plan");
        assert_eq!(got["permissions"]["allow"], json!(["Bash(go test:*)"]));
        assert!(!got["statusLine"].is_null() && !got["enabledPlugins"].is_null());
        assert!(got["env"].get("FOO").is_none());
        assert_eq!(got["env"]["CLAUDE_CODE_SUBAGENT_MODEL"], "haiku");
        s.set_env("CLAUDE_CODE_SUBAGENT_MODEL", "").unwrap();
        s.save().unwrap();
        let got: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            got.get("env").is_none(),
            "empty env should be removed: {got}"
        );

        let empty = tempfile::tempdir().unwrap();
        let mut s = load_settings(&Account::at("", empty.path())).unwrap();
        assert_eq!(s.string("model"), "");
        s.set("model", "haiku").unwrap();
        s.save().unwrap();
    }

    #[test]
    fn status_line_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = Profile {
            kind: KIND.into(),
            name: String::new(),
            dir: dir.path().to_string_lossy().into_owned(),
        };
        assert_eq!(status_line(&p).unwrap(), "");
        set_status_line(&p, "rush statusline").unwrap();
        assert_eq!(status_line(&p).unwrap(), "rush statusline");
        let got: Value =
            serde_json::from_slice(&fs::read(dir.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            got["statusLine"],
            json!({"type": "command", "command": "rush statusline", "padding": 0})
        );
        set_status_line(&p, "").unwrap();
        assert_eq!(status_line(&p).unwrap(), "");
    }

    #[test]
    fn agent_defs_read_front_matter() {
        let dir = tempfile::tempdir().unwrap();
        let p = Profile {
            kind: KIND.into(),
            name: String::new(),
            dir: dir.path().join("cfg").to_string_lossy().into_owned(),
        };
        let made = new_agent_def(&p, "tidy").unwrap();
        assert!(made.ends_with("agents/tidy.md"));
        let proj = dir.path().join("app");
        fs::create_dir_all(proj.join(".claude/agents")).unwrap();
        fs::write(
            proj.join(".claude/agents/rev.md"),
            "---\nname: reviewer\ndescription: |\n  reviews code\nmodel: opus\n---\nbody\n",
        )
        .unwrap();
        let roots = vec![proj.to_string_lossy().into_owned(), String::new()];
        let defs = agent_defs(&p, &roots);
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].scope, "built-in");
        assert_eq!(
            (
                defs[1].name.as_str(),
                defs[1].scope.as_str(),
                defs[1].model.as_str()
            ),
            ("tidy", "user", "inherit")
        );
        assert_eq!(
            defs[1].desc,
            "When to use this agent, in one or two sentences."
        );
        assert_eq!(
            (
                defs[2].name.as_str(),
                defs[2].scope.as_str(),
                defs[2].desc.as_str()
            ),
            ("reviewer", "app", "reviews code")
        );
    }

    #[test]
    fn project_root_by_its_git() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("a/b")).unwrap();
        let sub = repo.join("a/b").to_string_lossy().into_owned();
        assert_eq!(project_root(&sub), repo.to_string_lossy());
        let lone = dir.path().join("lone").to_string_lossy().into_owned();
        assert_eq!(project_root(&lone), lone);
        assert_eq!(project_root(""), "");
    }
}
