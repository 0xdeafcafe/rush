//! Rush's gate hook and context guard as Codex's own hooks. Codex runs a
//! user's hooks from hooks.json in its home, each only once config.toml
//! trusts it (hooks.state."<key>".trusted_hash, the hash hooks/list
//! reports for it); one passed for a session with -c is skipped. So,
//! with your OK (Settings › Harnesses › Codex), rush writes its hook into
//! that hooks.json once and trusts it through Codex's own app-server,
//! and every Codex session after, rush's or not, runs it. Off takes it
//! out of both, your other hooks kept as they were. Rush-only: Go had no
//! such step.
//!
//! What was checked against codex-cli 0.169.0: the key is hooks.json's
//! real path, the event in snake case, the group's index and the hook's
//! (".../hooks.json:pre_tool_use:0:0"); the hash is the hook's own, the
//! same at any index; config/batchWrite splits a key path at dots, so
//! the state is written as a table under hooks.state.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rush_core::adapter::{OwnGroup, OwnKind, OwnSetting, ValueFrom};
use serde_json::{Map, Value, json};

use crate::rpc::Client;

/// The events rush's hook answers in Codex, each with its matcher
/// ("" for none): before a shell call, the gate and the context guard;
/// after one and before a compaction, the context guard. Codex names its
/// shell tool Bash in a hook's input, as Claude Code does.
pub const EVENTS: &[(&str, &str)] = &[
    ("PreToolUse", "Bash"),
    ("PostToolUse", "Bash"),
    ("PreCompact", ""),
];

/// The seconds Codex gives a run of it, as Claude Code's.
const TIMEOUT: u64 = 10;

/// How a command of rush's hook ends, whichever rush it names: rush_gate's
/// codex_hook_command, `'<rush>' gate hook --codex`.
const MARK: &str = " gate hook --codex";

/// The key its row on Codex's settings page edits.
pub const KEY: &str = "rush:hooks";

/// The longest a call to the app-server is waited on.
const WITHIN: Duration = Duration::from_secs(30);

static MAKE: OnceLock<fn() -> String> = OnceLock::new();
static COMMAND: OnceLock<String> = OnceLock::new();

/// Says what makes the command that runs rush's hook for Codex
/// (rush_gate::plugin::codex_hook_command), once, as rush starts; it's
/// made when it's first needed.
pub fn use_command(make: fn() -> String) {
    let _ = MAKE.set(make);
}

/// The command use_command's maker makes; none without one, or for no
/// rush.
pub fn command() -> Option<&'static str> {
    let make = MAKE.get()?;
    Some(COMMAND.get_or_init(*make).as_str()).filter(|c| !c.is_empty())
}

/// Whether a Codex home runs rush's hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not there.
    Off,
    /// There for every event, as this rush runs it, and trusted.
    On,
    /// There, but for another rush, or not trusted: on again trusts it.
    Stale,
}

/// home's hooks.json, where a user's hooks are.
pub fn hooks_file(home: &Path) -> PathBuf {
    home.join("hooks.json")
}

/// Whether home runs rush's hook as command; read from its files alone.
pub fn state(home: &Path, command: &str) -> State {
    let doc = read_doc(&hooks_file(home)).unwrap_or(Value::Null);
    let found = ours(&doc);
    if found.is_empty() {
        return State::Off;
    }
    let trusted = trusted_keys(home);
    let base = key_base(home);
    let on = found.len() == EVENTS.len()
        && EVENTS.iter().all(|(event, matcher)| {
            found.iter().any(|h| {
                h.event == *event
                    && h.command == command
                    && h.matcher == *matcher
                    && trusted.contains(&key(&base, event, h.group, h.index))
            })
        });
    if on { State::On } else { State::Stale }
}

/// What Codex's app-server is asked; Err is in words to show.
pub trait Server {
    fn call(&self, method: &str, params: Value) -> Result<Value, String>;
}

/// Codex's app-server for one home, started at its first call and ended
/// when dropped.
pub struct AppServer {
    home: String,
    client: Mutex<Option<Arc<Client>>>,
}

impl AppServer {
    pub fn new(home: &Path) -> AppServer {
        AppServer {
            home: home.to_string_lossy().into_owned(),
            client: Mutex::new(None),
        }
    }

    fn client(&self) -> Result<Arc<Client>, String> {
        let mut c = self.client.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = c.as_ref() {
            return Ok(c.clone());
        }
        let started = crate::rpc::spawn("", &self.home, &[], &[], None)
            .map_err(|e| format!("couldn't start Codex's app-server: {e}"))?;
        if let Err(e) = started.initialize(Some(WITHIN)) {
            started.close();
            return Err(format!("Codex's app-server didn't answer: {e}"));
        }
        *c = Some(started.clone());
        Ok(started)
    }
}

impl Server for AppServer {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let r = self
            .client()?
            .call(method, params, Some(WITHIN))
            .map_err(|e| e.to_string())?;
        serde_json::from_str(r.get()).map_err(|e| format!("{method}: {e}"))
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        if let Some(c) = self.client.lock().unwrap_or_else(|e| e.into_inner()).take() {
            c.close();
        }
    }
}

/// Puts rush's hook, run as command, into home's hooks.json for each of
/// EVENTS (a hook of rush's already there takes command, so a new rush
/// replaces an old one's), then trusts each through s. Nothing else in
/// hooks.json or config.toml changes. Says what it did.
pub fn install(home: &Path, command: &str, s: &dyn Server) -> Result<String, String> {
    if command.is_empty() {
        return Err("there's no rush to run the hook".into());
    }
    if hooks_off(home) {
        return Err("Codex's hooks are off in its config.toml ([features] hooks = false)".into());
    }
    let path = hooks_file(home);
    let mut doc = read_doc(&path).map_err(|e| format!("couldn't read {}: {e}", shown(&path)))?;
    if put(&mut doc, command)? {
        write_doc(&path, &doc).map_err(|e| format!("couldn't write {}: {e}", shown(&path)))?;
    }
    let mine = |listed: &[Listed]| -> Vec<Listed> {
        listed
            .iter()
            .filter(|h| h.source == "user" && h.command == command)
            .cloned()
            .collect()
    };
    let found = mine(&list(s, home)?);
    if found.len() < EVENTS.len() {
        return Err(format!(
            "Codex lists {} of rush's {} hooks in {}",
            found.len(),
            EVENTS.len(),
            shown(&path)
        ));
    }
    let trust: Map<String, Value> = found
        .iter()
        .filter(|h| h.trust != "trusted")
        .map(|h| (h.key.clone(), json!({ "trusted_hash": h.hash })))
        .collect();
    if !trust.is_empty() {
        s.call(
            "config/batchWrite",
            json!({"edits": [{"keyPath": "hooks.state", "mergeStrategy": "upsert", "value": trust}]}),
        )?;
    }
    let after = mine(&list(s, home)?);
    if after.iter().any(|h| h.trust != "trusted") {
        return Err("Codex didn't take rush's trust in its hook".into());
    }
    Ok(format!(
        "every Codex session runs rush's hook now: {} hooks in {}, trusted in config.toml",
        after.len(),
        shown(&path)
    ))
}

/// Takes rush's hook out of home's hooks.json and its trust out of
/// config.toml; a hook of yours after it in the same event moves up, so
/// its trust moves with it. Says what it did.
pub fn remove(home: &Path, s: &dyn Server) -> Result<String, String> {
    let path = hooks_file(home);
    let mut doc = read_doc(&path).map_err(|e| format!("couldn't read {}: {e}", shown(&path)))?;
    let base = key_base(home);
    let moved = take_out(&mut doc, &base);
    if moved.is_empty() {
        return Ok(format!("rush's hook isn't in {}", shown(&path)));
    }
    let state = trust_table(home);
    let mut next = state.clone();
    let mut kept = Vec::new();
    for (old, new) in &moved {
        if let Some(v) = next.remove(old)
            && let Some(new) = new
        {
            kept.push((new.clone(), v));
        }
    }
    next.extend(kept);
    if next != state {
        s.call(
            "config/batchWrite",
            json!({"edits": [{"keyPath": "hooks.state", "mergeStrategy": "replace", "value": next}]}),
        )?;
    }
    let empty = doc.as_object().is_some_and(|o| {
        o.len() == 1
            && o.get("hooks")
                .is_some_and(|h| h.as_object().is_some_and(Map::is_empty))
    });
    let wrote = if empty {
        fs::remove_file(&path)
    } else {
        write_doc(&path, &doc)
    };
    wrote.map_err(|e| format!("couldn't write {}: {e}", shown(&path)))?;
    Ok(format!(
        "rush's hook is out of {} and config.toml; Codex's other hooks are as they were",
        shown(&path)
    ))
}

/// Trusts rush's hook again in home when it's there for another rush (a
/// new path after an update): the OK given for it still holds. Nothing
/// when it's off, or already this rush's.
pub fn refresh(home: &Path) {
    let Some(command) = command() else {
        return;
    };
    let doc = read_doc(&hooks_file(home)).unwrap_or(Value::Null);
    let found = ours(&doc);
    if found.is_empty() || found.iter().all(|h| h.command == command) {
        return;
    }
    let _ = install(home, command, &AppServer::new(home));
}

/// Its row on Codex's settings page, for home, run as command.
pub fn group(home: &Path, command: &str) -> OwnGroup {
    let st = state(home, command);
    let item = OwnSetting {
        key: KEY.into(),
        label: "rush's gate and context guard".into(),
        what: "Whether every Codex session, rush's and those you start yourself, runs rush's hook before and after each shell command: the search and context guards refuse wasteful searches and repeated commands, and in sessions rush runs the gate shares out heavy builds and tests and runs long commands as scripts rush can follow. On writes it into hooks.json in Codex's folder and trusts it in config.toml; off takes it out of both, your other hooks as they were.".into(),
        kind: OwnKind::OnOff,
        value: if st == State::On { "on" } else { "" }.into(),
        from: if st == State::Off {
            ValueFrom::Default
        } else {
            ValueFrom::State("hooks.json".into())
        },
        choices: vec![
            (String::new(), "off: Codex runs only its own hooks.".into()),
            ("on".into(), "every Codex session runs rush's hook.".into()),
        ],
        unset: if st == State::Stale {
            "off · rush's hook needs trusting again"
        } else {
            "off"
        }
        .into(),
        ask: "Give Codex rush's gate and context guard?".into(),
    };
    OwnGroup {
        title: "rush in every Codex session".into(),
        note: "Codex's own hooks, trusted once".into(),
        items: vec![item],
    }
}

/// Whether value can be the row's.
pub fn check(value: &str) -> Result<(), String> {
    match value {
        "" | "on" | "off" => Ok(()),
        _ => Err(format!("{KEY} is on or off")),
    }
}

/// Sets the row to value in home: on installs and trusts, off removes.
pub fn set(home: &Path, value: &str) -> Result<String, String> {
    check(value)?;
    let s = AppServer::new(home);
    if value == "on" {
        let command = command().ok_or("there's no rush to run the hook")?;
        return install(home, command, &s);
    }
    remove(home, &s)
}

/// A hook of rush's in hooks.json.
#[derive(Debug)]
struct Found {
    event: String,
    matcher: String,
    group: usize,
    index: usize,
    command: String,
}

/// A hook as hooks/list reports it.
#[derive(Clone, Debug)]
struct Listed {
    key: String,
    command: String,
    source: String,
    trust: String,
    hash: String,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Every hook home's app-server lists.
fn list(s: &dyn Server, home: &Path) -> Result<Vec<Listed>, String> {
    let r = s.call("hooks/list", json!({ "cwds": [home.to_string_lossy()] }))?;
    let mut out: Vec<Listed> = Vec::new();
    for d in r
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for h in d
            .get("hooks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = self::s(h, "key").to_string();
            if out.iter().any(|o| o.key == key) {
                continue;
            }
            out.push(Listed {
                key,
                command: self::s(h, "command").into(),
                source: self::s(h, "source").into(),
                trust: self::s(h, "trustStatus").into(),
                hash: self::s(h, "currentHash").into(),
            });
        }
    }
    Ok(out)
}

/// The hooks of rush's in a hooks.json.
fn ours(doc: &Value) -> Vec<Found> {
    let mut out = Vec::new();
    let Some(events) = doc.get("hooks").and_then(Value::as_object) else {
        return out;
    };
    for (event, groups) in events {
        for (g, group) in groups.as_array().into_iter().flatten().enumerate() {
            for (i, h) in group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                if s(h, "command").ends_with(MARK) {
                    out.push(Found {
                        event: event.clone(),
                        matcher: s(group, "matcher").into(),
                        group: g,
                        index: i,
                        command: s(h, "command").into(),
                    });
                }
            }
        }
    }
    out
}

/// Puts rush's hook into doc for each event; whether doc changed.
fn put(doc: &mut Value, command: &str) -> Result<bool, String> {
    if doc.is_null() {
        *doc = json!({});
    }
    let bad = || "hooks.json isn't hooks as Codex reads them".to_string();
    let top = doc.as_object_mut().ok_or_else(bad)?;
    let events = top
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(bad)?;
    let mut changed = false;
    for (event, matcher) in EVENTS {
        let groups = events
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(bad)?;
        let at = groups.iter_mut().find_map(|g| {
            g.get_mut("hooks")
                .and_then(Value::as_array_mut)?
                .iter_mut()
                .find(|h| s(h, "command").ends_with(MARK))
        });
        match at {
            Some(h) => {
                if s(h, "command") != command {
                    h["command"] = Value::String(command.into());
                    changed = true;
                }
            }
            None => {
                let mut g =
                    json!({"hooks": [{"type": "command", "command": command, "timeout": TIMEOUT}]});
                if !matcher.is_empty() {
                    g["matcher"] = Value::String((*matcher).into());
                }
                groups.push(g);
                changed = true;
            }
        }
    }
    Ok(changed)
}

/// Takes rush's hooks out of doc: each one's key with the key it has
/// after, or none for rush's own, for every hook that moved or went.
fn take_out(doc: &mut Value, base: &str) -> Vec<(String, Option<String>)> {
    let mut moved = Vec::new();
    let Some(events) = doc.get_mut("hooks").and_then(Value::as_object_mut) else {
        return moved;
    };
    let mut emptied = Vec::new();
    for (event, groups) in events.iter_mut() {
        let Some(list) = groups.as_array_mut() else {
            continue;
        };
        if !list.iter().any(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hs| hs.iter().any(|h| s(h, "command").ends_with(MARK)))
        }) {
            continue;
        }
        let mut next = Vec::new();
        for (g, mut group) in std::mem::take(list).into_iter().enumerate() {
            let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                next.push(group);
                continue;
            };
            let mut kept = Vec::new();
            for (i, h) in std::mem::take(hooks).into_iter().enumerate() {
                let old = key(base, event, g, i);
                if s(&h, "command").ends_with(MARK) {
                    moved.push((old, None));
                } else {
                    let new = key(base, event, next.len(), kept.len());
                    if new != old {
                        moved.push((old, Some(new)));
                    }
                    kept.push(h);
                }
            }
            if kept.is_empty() {
                continue;
            }
            *hooks = kept;
            next.push(group);
        }
        if next.is_empty() {
            emptied.push(event.clone());
        }
        *list = next;
    }
    for e in emptied {
        events.remove(&e);
    }
    moved
}

/// hooks.json at path; a missing one is null.
fn read_doc(path: &Path) -> io::Result<Value> {
    match fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(e),
    }
}

/// Writes doc to path (a link's target) through a temp file renamed
/// onto it, readable by you alone.
fn write_doc(path: &Path, doc: &Value) -> io::Result<()> {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = real.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut b = serde_json::to_vec_pretty(doc)?;
    b.push(b'\n');
    let tmp = dir.join(format!(".hooks-{}.json", std::process::id()));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&b)?;
        f.sync_all()?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        fs::rename(&tmp, &real)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// home's config.toml as a table; empty when it's missing or unreadable.
fn config(home: &Path) -> toml::Table {
    fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

/// Whether config.toml turns Codex's hooks off.
fn hooks_off(home: &Path) -> bool {
    config(home)
        .get("features")
        .and_then(|f| f.get("hooks"))
        .and_then(toml::Value::as_bool)
        == Some(false)
}

/// config.toml's hooks.state, as JSON.
fn trust_table(home: &Path) -> Map<String, Value> {
    config(home)
        .get("hooks")
        .and_then(|h| h.get("state"))
        .and_then(|s| serde_json::to_value(s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// The keys config.toml trusts.
fn trusted_keys(home: &Path) -> HashSet<String> {
    trust_table(home)
        .into_iter()
        .filter(|(_, v)| !s(v, "trusted_hash").is_empty())
        .map(|(k, _)| k)
        .collect()
}

/// How Codex's keys for home's hooks.json begin: its real path.
fn key_base(home: &Path) -> String {
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    hooks_file(&home).to_string_lossy().into_owned()
}

/// Codex's key for a hook: PreToolUse's first group's first hook is
/// "<base>:pre_tool_use:0:0".
fn key(base: &str, event: &str, group: usize, index: usize) -> String {
    let mut snake = String::new();
    for (i, c) in event.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                snake.push('_');
            }
            snake.push(c.to_ascii_lowercase());
        } else {
            snake.push(c);
        }
    }
    format!("{base}:{snake}:{group}:{index}")
}

/// path with the home folder as ~.
fn shown(path: &Path) -> String {
    let p = path.to_string_lossy().into_owned();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Codex's app-server as codex-cli 0.169.0 answered in a throwaway
    /// CODEX_HOME: hooks/list keys a hook by hooks.json's real path, its
    /// event in snake case and its indices, and hashes the hook alone;
    /// config/batchWrite merges or replaces a table in config.toml.
    struct Fake {
        home: PathBuf,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(home: &Path) -> Fake {
            Fake {
                home: home.to_path_buf(),
                calls: RefCell::new(Vec::new()),
            }
        }
        fn writes(&self) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|c| *c == "config/batchWrite")
                .count()
        }
    }

    fn snake(event: &str) -> &'static str {
        match event {
            "PreToolUse" => "pre_tool_use",
            "PostToolUse" => "post_tool_use",
            "PreCompact" => "pre_compact",
            "Stop" => "stop",
            e => panic!("no snake for {e}"),
        }
    }

    impl Server for Fake {
        fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            self.calls.borrow_mut().push(method.into());
            let cfg_path = self.home.join("config.toml");
            let mut cfg: toml::Table = fs::read_to_string(&cfg_path)
                .ok()
                .map(|t| toml::from_str(&t).unwrap())
                .unwrap_or_default();
            match method {
                "hooks/list" => {
                    let real = fs::canonicalize(&self.home).unwrap().join("hooks.json");
                    let doc: Value = fs::read(&real)
                        .ok()
                        .map(|b| serde_json::from_slice(&b).unwrap())
                        .unwrap_or(json!({}));
                    let state = cfg
                        .get("hooks")
                        .and_then(|h| h.get("state"))
                        .cloned()
                        .unwrap_or(toml::Value::Table(Default::default()));
                    let mut hooks = Vec::new();
                    for (event, groups) in doc["hooks"].as_object().into_iter().flatten() {
                        for (g, group) in groups.as_array().unwrap().iter().enumerate() {
                            for (i, h) in group["hooks"].as_array().unwrap().iter().enumerate() {
                                let key = format!("{}:{}:{g}:{i}", real.display(), snake(event));
                                let hash = format!(
                                    "sha256:{:x}",
                                    sha2::Sha256::digest(
                                        format!("{event}|{}|{h}", group["matcher"]).as_bytes()
                                    )
                                );
                                let trusted = state
                                    .get(&key)
                                    .and_then(|s| s.get("trusted_hash"))
                                    .and_then(toml::Value::as_str);
                                let trust = match trusted {
                                    None => "untrusted",
                                    Some(t) if t == hash => "trusted",
                                    Some(_) => "modified",
                                };
                                hooks.push(
                                    json!({"key": key, "command": h["command"], "source": "user",
                                    "currentHash": hash, "trustStatus": trust}),
                                );
                            }
                        }
                    }
                    Ok(json!({"data": [{"cwd": params["cwds"][0], "hooks": hooks}]}))
                }
                "config/batchWrite" => {
                    for e in params["edits"].as_array().unwrap() {
                        assert_eq!(e["keyPath"], "hooks.state");
                        let value: toml::Value = toml::Value::try_from(e["value"].clone()).unwrap();
                        let hooks = cfg
                            .entry("hooks")
                            .or_insert(toml::Value::Table(Default::default()))
                            .as_table_mut()
                            .unwrap();
                        let state = hooks
                            .entry("state")
                            .or_insert(toml::Value::Table(Default::default()))
                            .as_table_mut()
                            .unwrap();
                        let toml::Value::Table(t) = value else {
                            panic!("not a table")
                        };
                        match e["mergeStrategy"].as_str().unwrap() {
                            "upsert" => state.extend(t),
                            "replace" => *state = t,
                            s => panic!("{s}"),
                        }
                    }
                    fs::write(&cfg_path, toml::to_string(&cfg).unwrap()).unwrap();
                    Ok(json!({"status": "ok"}))
                }
                m => Err(format!("no {m}")),
            }
        }
    }

    use sha2::Digest;

    const RUSH: &str = "'/opt/rush/bin/rush' gate hook --codex";
    const MOVED: &str = "'/usr/local/bin/rush' gate hook --codex";

    fn doc(home: &Path) -> Value {
        serde_json::from_slice(&fs::read(hooks_file(home)).unwrap()).unwrap()
    }

    fn trust(home: &Path) -> toml::Table {
        let t: toml::Table =
            toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
        t.get("hooks")
            .and_then(|h| h.get("state"))
            .and_then(toml::Value::as_table)
            .cloned()
            .unwrap_or_default()
    }

    /// What the app-server lists: (key, command, trust, hash).
    fn listed(s: &Fake) -> Vec<(String, String, String, String)> {
        let r = s.call("hooks/list", json!({"cwds": ["/"]})).unwrap();
        r["data"][0]["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                let f = |k: &str| h[k].as_str().unwrap().to_string();
                (f("key"), f("command"), f("trustStatus"), f("currentHash"))
            })
            .collect()
    }

    // On writes rush's hook for each event and trusts it; again, nothing
    // changes; your settings and other trust stay as they were.
    #[test]
    fn install_trusts_once() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        fs::write(
            home.join("config.toml"),
            "model = \"gpt-6\"\n\n[hooks.state.\"warp:hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:w\"\ntrusted_content_hash = \"sha256:c\"\n",
        )
        .unwrap();
        assert_eq!(state(home, RUSH), State::Off);
        let s = Fake::new(home);
        let said = install(home, RUSH, &s).unwrap();
        assert!(said.contains("3 hooks"), "{said}");
        assert_eq!(state(home, RUSH), State::On);
        assert_eq!(
            doc(home),
            json!({"hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": RUSH, "timeout": 10}]}],
                "PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": RUSH, "timeout": 10}]}],
                "PreCompact": [{"hooks": [{"type": "command", "command": RUSH, "timeout": 10}]}],
            }})
        );
        let t = trust(home);
        assert_eq!(t.len(), 4, "{t:?}");
        assert_eq!(
            t["warp:hooks.json:stop:0:0"]["trusted_content_hash"].as_str(),
            Some("sha256:c")
        );
        let cfg: toml::Table =
            toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
        assert_eq!(cfg["model"].as_str(), Some("gpt-6"));
        assert_eq!(s.writes(), 1);

        let before = fs::read(hooks_file(home)).unwrap();
        install(home, RUSH, &s).unwrap();
        assert_eq!(fs::read(hooks_file(home)).unwrap(), before);
        assert_eq!(s.writes(), 1, "nothing left to trust");
    }

    // Your hooks before and beside rush's keep their places and trust; a
    // new rush takes the old one's place, and is trusted as itself.
    #[test]
    fn install_keeps_yours_and_moves_with_rush() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let yours = json!({"hooks": {
            "PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "~/bin/audit"}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "say done"}]}],
        }});
        fs::write(hooks_file(home), yours.to_string()).unwrap();
        let s = Fake::new(home);
        install(home, RUSH, &s).unwrap();
        let got = doc(home);
        assert_eq!(got["hooks"]["Stop"], yours["hooks"]["Stop"]);
        assert_eq!(
            got["hooks"]["PreToolUse"][0],
            yours["hooks"]["PreToolUse"][0]
        );
        assert_eq!(got["hooks"]["PreToolUse"][1]["hooks"][0]["command"], RUSH);
        let t = trust(home);
        assert!(
            t.keys().all(|k| k.contains(":pre_tool_use:1:0")
                || k.contains(":post_tool_use:0:0")
                || k.contains(":pre_compact:0:0")),
            "only rush's are trusted by it: {t:?}"
        );

        install(home, MOVED, &s).unwrap();
        assert_eq!(state(home, MOVED), State::On);
        assert_eq!(state(home, RUSH), State::Stale);
        let got = doc(home);
        assert_eq!(got["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(got["hooks"]["PreToolUse"][1]["hooks"][0]["command"], MOVED);
    }

    // Off takes rush's hook and its trust out; a hook of yours added after
    // it moves up with its trust; nothing of rush's left, no hooks.json.
    #[test]
    fn remove_keeps_yours_trusted() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let s = Fake::new(home);
        install(home, RUSH, &s).unwrap();
        let mut got = doc(home);
        let audit = json!({"hooks": [{"type": "command", "command": "~/bin/audit"}]});
        got["hooks"]["PreToolUse"]
            .as_array_mut()
            .unwrap()
            .push(audit.clone());
        fs::write(hooks_file(home), got.to_string()).unwrap();
        install(home, RUSH, &s).unwrap(); // trusts nothing new of yours
        // You trust yours, as Codex would.
        let all = listed(&s);
        let (key, _, _, hash) = all.iter().find(|h| h.1 == "~/bin/audit").unwrap();
        assert!(key.ends_with(":pre_tool_use:1:0"), "{key}");
        let mut value = Map::new();
        value.insert(key.clone(), json!({ "trusted_hash": hash }));
        s.call(
            "config/batchWrite",
            json!({"edits": [{"keyPath": "hooks.state", "mergeStrategy": "upsert", "value": value}]}),
        )
        .unwrap();

        let said = remove(home, &s).unwrap();
        assert!(said.contains("out of"), "{said}");
        assert_eq!(state(home, RUSH), State::Off);
        assert_eq!(doc(home), json!({"hooks": {"PreToolUse": [audit]}}));
        let after = listed(&s);
        assert_eq!(after.len(), 1);
        assert!(after[0].0.ends_with(":pre_tool_use:0:0"), "{after:?}");
        assert_eq!(after[0].2, "trusted", "its trust moved with it");
        assert_eq!(trust(home).len(), 1);

        // Only rush's: the file goes.
        let d = tempfile::tempdir().unwrap();
        let s = Fake::new(d.path());
        install(d.path(), RUSH, &s).unwrap();
        remove(d.path(), &s).unwrap();
        assert!(!hooks_file(d.path()).exists());
        assert!(trust(d.path()).is_empty());
        assert!(remove(d.path(), &s).unwrap().contains("isn't in"));
    }

    #[test]
    fn refuses_what_it_cant_do() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let s = Fake::new(home);
        fs::write(home.join("config.toml"), "[features]\nhooks = false\n").unwrap();
        assert!(
            install(home, RUSH, &s)
                .unwrap_err()
                .contains("hooks are off")
        );
        fs::write(home.join("config.toml"), "").unwrap();
        fs::write(hooks_file(home), "{not json").unwrap();
        assert!(install(home, RUSH, &s).is_err());
        assert_eq!(fs::read_to_string(hooks_file(home)).unwrap(), "{not json");
        assert!(install(home, "", &s).is_err());
        for (v, ok) in [("on", true), ("", true), ("off", true), ("yes", false)] {
            assert_eq!(check(v).is_ok(), ok, "{v}");
        }
    }

    // The row shows what's there, and asks before it's turned on.
    #[test]
    fn its_row() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let row = |c: &str| group(home, c).items.remove(0);
        let off = row(RUSH);
        assert_eq!(
            (off.key.as_str(), off.value.as_str(), off.unset.as_str()),
            (KEY, "", "off")
        );
        assert!(!off.ask.is_empty());
        install(home, RUSH, &Fake::new(home)).unwrap();
        assert_eq!(row(RUSH).value, "on");
        let stale = row(MOVED);
        assert_eq!(stale.value, "");
        assert!(stale.unset.contains("trusting again"), "{}", stale.unset);
    }
}
