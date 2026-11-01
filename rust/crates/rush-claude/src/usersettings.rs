//! Claude Code's own settings as rush's Settings shows and changes them:
//! each value with where it comes from (the environment, settings.json,
//! Claude Code's state file, or its default), and changes written to every
//! settings.json the accounts read. Rush-only: Go showed a few of these
//! keys folded under Advanced, and none of auto-compact.
//!
//! Every account's home links its settings.json to ~/.claude's, so one
//! file serves them all; a home whose settings.json isn't a link (Claude
//! Code rewrote it in place of the link) is written too, so a value set
//! here holds for every account. Writes keep everything else in the file
//! (rush_state's settingsfile: keys in order, unknown keys as written),
//! go through a temp file and a rename onto the link's target, so the
//! links stay links, and leave the file readable by you alone.
//!
//! Which wins is as Claude Code and compaction() in settings.rs read it: a
//! variable in settings.json's env block, then one in the environment rush
//! runs in (each session starts with both, the block over the rest), then
//! settings.json, then the state file, then the default.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rush_state::settingsfile::File as Settings;
use serde_json::Value;

pub use rush_core::adapter::ValueFrom as From;

/// The state file's name, as a value from it says.
pub const STATE: &str = "~/.claude.json";

/// A value as shown, and where it comes from. "" is unset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub value: String,
    pub from: From,
}

impl Shown {
    pub fn new(value: impl Into<String>, from: From) -> Shown {
        Shown {
            value: value.into(),
            from,
        }
    }
}

/// The variables that turn auto-compact off; DISABLE_COMPACT turns
/// /compact off too.
pub const COMPACT_OFF_VARS: [&str; 2] = ["DISABLE_AUTO_COMPACT", "DISABLE_COMPACT"];
/// The variable that sets the auto-compact window over the setting.
pub const COMPACT_WINDOW_VAR: &str = "CLAUDE_CODE_AUTO_COMPACT_WINDOW";
/// The smallest and largest window Claude Code takes from settings.
pub const MIN_WINDOW: i64 = 100_000;
pub const MAX_WINDOW: i64 = 1_000_000;

/// Claude Code's settings as last read: settings.json, the state file,
/// and the environment rush runs in.
pub struct Snapshot {
    /// The settings.json shown: the account in use's.
    pub file: Settings,
    pub state: Option<Settings>,
    env: HashMap<String, String>,
    /// Every distinct settings.json a change is written to.
    pub paths: Vec<PathBuf>,
}

impl Snapshot {
    /// Reads dir's settings.json and state file, with env the
    /// environment rush runs in; homes are the other config folders
    /// whose settings.json a change goes to as well.
    pub fn read(
        dir: &Path,
        homes: &[PathBuf],
        env: impl IntoIterator<Item = (String, String)>,
    ) -> io::Result<Snapshot> {
        let main = dir.join("settings.json");
        let file = Settings::load(&main)?;
        let state = Settings::load(crate::settings::claude_json(&dir.to_string_lossy())).ok();
        let mut dirs = vec![dir.to_path_buf()];
        dirs.extend(homes.iter().cloned());
        Ok(Snapshot {
            file,
            state,
            env: env.into_iter().filter(|(k, _)| wanted_var(k)).collect(),
            paths: settings_paths(&dirs),
        })
    }

    /// A variable a session starts with, and where it's set: the env
    /// block over the environment.
    pub fn var(&self, name: &str) -> Option<Shown> {
        if let Some(v) = self.file.get::<Value>(&format!("env.{name}")) {
            return Some(Shown::new(env_string(&v), From::EnvFile(name.to_string())));
        }
        self.env
            .get(name)
            .map(|v| Shown::new(v.clone(), From::Env(name.to_string())))
    }

    /// Auto-compact: "off", a window ("400k"), or "" for on at the
    /// model's own window.
    pub fn auto_compact(&self) -> Shown {
        for name in COMPACT_OFF_VARS {
            if let Some(v) = self.var(name)
                && truthy(&v.value)
            {
                return Shown::new("off", v.from);
            }
        }
        match self.file.get::<bool>("autoCompactEnabled") {
            Some(false) => return Shown::new("off", From::File),
            Some(true) => {}
            None => {
                if self
                    .state
                    .as_ref()
                    .and_then(|s| s.get::<bool>("autoCompactEnabled"))
                    == Some(false)
                {
                    return Shown::new("off", From::State(STATE.into()));
                }
            }
        }
        if let Some(v) = self.var(COMPACT_WINDOW_VAR)
            && let Some(n) = leading_int(&v.value).filter(|n| *n > 0)
        {
            return Shown::new(tokens_word(n.clamp(MIN_WINDOW, MAX_WINDOW)), v.from);
        }
        match self.file.get::<f64>("autoCompactWindow") {
            Some(n) if n.fract() == 0.0 => Shown::new(tokens_word(n as i64), From::File),
            _ => Shown::new("", From::Default),
        }
    }

    /// A string key, unless variable var (when given) sets it.
    pub fn string(&self, key: &str, var: Option<&str>) -> Shown {
        if let Some(v) = var.and_then(|n| self.var(n))
            && !v.value.is_empty()
        {
            return v;
        }
        match self.file.get::<String>(key) {
            Some(v) if !v.is_empty() => Shown::new(v, From::File),
            _ => Shown::new("", From::Default),
        }
    }

    /// A key that's true or false, as "on", "off" or "" for unset; the
    /// state file's value when settings.json has none and state says.
    pub fn flag(&self, key: &str, state_too: bool) -> Shown {
        if let Some(b) = self.file.get::<bool>(key) {
            return Shown::new(on_off(b), From::File);
        }
        if state_too && let Some(b) = self.state.as_ref().and_then(|s| s.get::<bool>(key)) {
            return Shown::new(on_off(b), From::State(STATE.into()));
        }
        Shown::new("", From::Default)
    }

    /// A whole number key, "" when unset.
    pub fn int(&self, key: &str) -> Shown {
        match self.file.get::<f64>(key) {
            Some(n) if n.fract() == 0.0 => Shown::new((n as i64).to_string(), From::File),
            _ => Shown::new("", From::Default),
        }
    }

    /// The status line's command, "" when there's none.
    pub fn status_line(&self) -> String {
        self.file.string("statusLine.command")
    }

    /// settings.json's env block, each value as a session sees it.
    pub fn env_block(&self) -> BTreeMap<String, String> {
        self.file
            .get::<BTreeMap<String, Value>>("env")
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, env_string(&v)))
            .collect()
    }

    /// The plugins settings.json names, and whether each is on.
    pub fn plugins(&self) -> Vec<(String, bool)> {
        self.file
            .get::<BTreeMap<String, Value>>("enabledPlugins")
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| {
                let on = !matches!(v, Value::Bool(false) | Value::Null);
                (k, on)
            })
            .collect()
    }

    /// Makes edits to the file shown, so it shows them before they're
    /// written.
    pub fn apply(&mut self, edits: &[Edit]) -> io::Result<()> {
        apply_to(&mut self.file, edits)
    }
}

/// Only these variables are asked of the environment.
fn wanted_var(k: &str) -> bool {
    COMPACT_OFF_VARS.contains(&k)
        || matches!(
            k,
            COMPACT_WINDOW_VAR | "ANTHROPIC_MODEL" | "CLAUDE_CODE_EFFORT_LEVEL"
        )
}

/// One change to settings.json: a key, by the keys on the way to it, set
/// to a JSON value or removed.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Set(Vec<String>, Value),
    Delete(Vec<String>),
}

impl Edit {
    /// Sets the key at a dotted path ("permissions.defaultMode").
    pub fn set(path: &str, v: impl Into<Value>) -> Edit {
        Edit::Set(path.split('.').map(String::from).collect(), v.into())
    }
    /// Removes the key at a dotted path, and objects it leaves empty.
    pub fn delete(path: &str) -> Edit {
        Edit::Delete(path.split('.').map(String::from).collect())
    }
    /// Sets key in object parent, whatever key has in it (a plugin's
    /// name may have a dot).
    pub fn set_in(parent: &str, key: &str, v: impl Into<Value>) -> Edit {
        Edit::Set(vec![parent.into(), key.into()], v.into())
    }
    /// Removes key from object parent.
    pub fn delete_in(parent: &str, key: &str) -> Edit {
        Edit::Delete(vec![parent.into(), key.into()])
    }
}

fn apply_to(f: &mut Settings, edits: &[Edit]) -> io::Result<()> {
    for e in edits {
        match e {
            Edit::Set(k, v) => {
                let keys: Vec<&str> = k.iter().map(String::as_str).collect();
                f.set_keys(&keys, v)?
            }
            Edit::Delete(k) => {
                let keys: Vec<&str> = k.iter().map(String::as_str).collect();
                f.delete_keys(&keys)
            }
        }
    }
    Ok(())
}

/// The settings.json each of dirs reads, once each: a link and its
/// target are one file. The first is written whether it's there or not;
/// another only when it's there (a login with no home yet, or one whose
/// link leads nowhere, gets no file of rush's making).
pub fn settings_paths(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for (i, d) in dirs.iter().enumerate() {
        let p = d.join("settings.json");
        if i > 0 && fs::metadata(&p).is_err() {
            continue;
        }
        let real = fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        if seen.contains(&real) {
            continue;
        }
        seen.push(real);
        out.push(p);
    }
    out
}

/// Writes edits into each settings.json in paths: the rest of each file
/// kept as it is on disk then, through a temp file renamed onto the
/// link's target, readable by you alone. The first failure is said.
pub fn write(paths: &[PathBuf], edits: &[Edit]) -> io::Result<()> {
    for p in paths {
        let mut f = Settings::load(p)?;
        apply_to(&mut f, edits)?;
        f.save()?;
        let real = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// A number of tokens as typed: "400k", "400000", "1m", "0.5M", with
/// commas or underscores between digits. None when it isn't one.
pub fn parse_tokens(s: &str) -> Option<i64> {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '_' | ' '))
        .collect::<String>()
        .to_lowercase();
    let t = t.strip_suffix("tokens").unwrap_or(&t);
    let (num, mul) = match t.chars().last()? {
        'k' => (&t[..t.len() - 1], 1_000.0),
        'm' => (&t[..t.len() - 1], 1_000_000.0),
        _ => (t, 1.0),
    };
    let n: f64 = num.parse().ok()?;
    let v = n * mul;
    (v.is_finite() && v > 0.0 && v.fract() == 0.0).then_some(v as i64)
}

/// n tokens said short: 400000 is "400k", 1000000 "1M".
pub fn tokens_word(n: i64) -> String {
    if n >= 1_000_000 && n % 1_000_000 == 0 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 && n % 1_000 == 0 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// What a typed or picked auto-compact value writes: "off", "on" (or
/// "" / "default") for the model's own window, or a window in tokens.
/// Err says why it can't be.
pub fn compact_edits(v: &str) -> Result<Vec<Edit>, String> {
    let v = v.trim().to_lowercase();
    match v.as_str() {
        "off" => Ok(vec![Edit::set("autoCompactEnabled", false)]),
        "" | "on" | "default" | "auto" => Ok(vec![
            Edit::delete("autoCompactEnabled"),
            Edit::delete("autoCompactWindow"),
        ]),
        _ => {
            let n = parse_tokens(&v).ok_or_else(|| {
                format!("{v:?} isn't a number of tokens: type it as 400k or 400000")
            })?;
            if !(MIN_WINDOW..=MAX_WINDOW).contains(&n) {
                return Err(format!(
                    "Claude Code takes a window from {} to {} tokens",
                    tokens_word(MIN_WINDOW),
                    tokens_word(MAX_WINDOW)
                ));
            }
            Ok(vec![
                Edit::delete("autoCompactEnabled"),
                Edit::set("autoCompactWindow", n),
            ])
        }
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The digits v starts with, as parseInt reads them.
fn leading_int(v: &str) -> Option<i64> {
    let v = v.trim_start();
    let end = v
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .map_or(v.len(), |(i, _)| i);
    v[..end].parse().ok()
}

/// An env value as the string a session's environment has.
fn env_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn env(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn tokens_read_as_typed() {
        for (s, want) in [
            ("400k", Some(400_000)),
            ("400K", Some(400_000)),
            ("400000", Some(400_000)),
            ("400,000", Some(400_000)),
            ("1m", Some(1_000_000)),
            ("1M", Some(1_000_000)),
            ("0.5m", Some(500_000)),
            ("250k tokens", Some(250_000)),
            ("lots", None),
            ("", None),
            ("-5k", None),
        ] {
            assert_eq!(parse_tokens(s), want, "{s:?}");
        }
        assert_eq!(tokens_word(400_000), "400k");
        assert_eq!(tokens_word(1_000_000), "1M");
        assert_eq!(tokens_word(123_456), "123456");
    }

    #[test]
    fn compact_edits_table() {
        assert_eq!(
            compact_edits("400k").unwrap(),
            vec![
                Edit::delete("autoCompactEnabled"),
                Edit::set("autoCompactWindow", 400_000)
            ]
        );
        assert_eq!(
            compact_edits("off").unwrap(),
            vec![Edit::set("autoCompactEnabled", false)]
        );
        assert!(compact_edits("on").unwrap().len() == 2);
        assert!(compact_edits("50k").unwrap_err().contains("100k to 1M"));
        assert!(compact_edits("soon").unwrap_err().contains("400k"));
    }

    // An env override wins over settings.json, the env block over the
    // environment rush runs in; settings.json over the state file.
    #[test]
    fn where_each_value_comes_from() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        fs::write(
            dir.join("settings.json"),
            r#"{"autoCompactWindow":400000,"model":"opus","env":{"MY_OWN":"1"}}"#,
        )
        .unwrap();
        let s = Snapshot::read(dir, &[], env(&[])).unwrap();
        assert_eq!(s.auto_compact(), Shown::new("400k", From::File));
        assert_eq!(s.string("model", Some("ANTHROPIC_MODEL")).from, From::File);
        assert_eq!(s.string("theme", None), Shown::new("", From::Default));

        let s = Snapshot::read(
            dir,
            &[],
            env(&[
                ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "600000"),
                ("ANTHROPIC_MODEL", "sonnet"),
                ("UNRELATED", "x"),
            ]),
        )
        .unwrap();
        assert_eq!(
            s.auto_compact(),
            Shown::new("600k", From::Env(COMPACT_WINDOW_VAR.into()))
        );
        assert_eq!(
            s.string("model", Some("ANTHROPIC_MODEL")),
            Shown::new("sonnet", From::Env("ANTHROPIC_MODEL".into()))
        );
        assert!(s.var("UNRELATED").is_none(), "only what's asked is kept");

        fs::write(
            dir.join("settings.json"),
            r#"{"autoCompactWindow":400000,"env":{"DISABLE_AUTO_COMPACT":"1"}}"#,
        )
        .unwrap();
        let s = Snapshot::read(dir, &[], env(&[(COMPACT_WINDOW_VAR, "600000")])).unwrap();
        assert_eq!(
            s.auto_compact(),
            Shown::new("off", From::EnvFile("DISABLE_AUTO_COMPACT".into()))
        );

        // The state file says off; settings.json saying on wins.
        fs::write(dir.join(".claude.json"), r#"{"autoCompactEnabled":false}"#).unwrap();
        fs::write(dir.join("settings.json"), "{}").unwrap();
        let s = Snapshot::read(dir, &[], env(&[])).unwrap();
        assert_eq!(
            s.auto_compact(),
            Shown::new("off", From::State(STATE.into()))
        );
        fs::write(dir.join("settings.json"), r#"{"autoCompactEnabled":true}"#).unwrap();
        let s = Snapshot::read(dir, &[], env(&[])).unwrap();
        assert_eq!(s.auto_compact(), Shown::new("", From::Default));
    }

    // A write keeps every other key exactly, unknown ones included, keeps
    // a home's link a link, writes a home of its own too, and leaves the
    // file readable by you alone.
    #[test]
    fn writes_keep_the_rest_and_the_links() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("claude-root");
        let linked = d.path().join("home-a");
        let own = d.path().join("home-b");
        for p in [&root, &linked, &own] {
            fs::create_dir_all(p).unwrap();
        }
        let body = r#"{"zeta":{"keep":[1,2,{"x":"a && b > c"}]},"hooks":{"Stop":[]},"enabledPlugins":{"p@m":false},"autoCompactWindow":200000}"#;
        fs::write(root.join("settings.json"), body).unwrap();
        fs::set_permissions(
            root.join("settings.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        symlink(root.join("settings.json"), linked.join("settings.json")).unwrap();
        fs::write(own.join("settings.json"), r#"{"mine":true}"#).unwrap();

        let none = d.path().join("home-none");
        let dangling = d.path().join("home-dangling");
        fs::create_dir_all(&dangling).unwrap();
        symlink(d.path().join("gone.json"), dangling.join("settings.json")).unwrap();
        let homes = vec![linked.clone(), own.clone(), none.clone(), dangling.clone()];
        let paths = settings_paths(&[
            root.clone(),
            linked.clone(),
            own.clone(),
            none.clone(),
            dangling.clone(),
        ]);
        assert_eq!(
            paths.len(),
            2,
            "a link and its target are one file; a home with none, or a link to nothing, isn't written"
        );
        let s = Snapshot::read(&root, &homes, env(&[])).unwrap();
        assert_eq!(s.paths, paths);
        write(&s.paths, &compact_edits("400k").unwrap()).unwrap();

        let got: Value =
            serde_json::from_str(&fs::read_to_string(root.join("settings.json")).unwrap()).unwrap();
        let mut want: Value = serde_json::from_str(body).unwrap();
        want["autoCompactWindow"] = 400_000.into();
        assert_eq!(got, want, "only the window changed");
        let text = fs::read_to_string(root.join("settings.json")).unwrap();
        assert!(text.contains("a && b > c"), "kept as written: {text}");
        assert!(
            text.find("\"zeta\"").unwrap() < text.find("\"hooks\"").unwrap(),
            "keys keep their order"
        );
        assert!(
            fs::symlink_metadata(linked.join("settings.json"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the home's link is still a link"
        );
        let mode = fs::metadata(root.join("settings.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let theirs: Value =
            serde_json::from_str(&fs::read_to_string(own.join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            theirs,
            serde_json::json!({"mine": true, "autoCompactWindow": 400000})
        );
        assert!(!none.exists(), "no home made for a login without one");
        assert!(!d.path().join("gone.json").exists());
        // No temp file left behind.
        for dir in [&root, &own] {
            let names: Vec<_> = fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert!(
                names
                    .iter()
                    .all(|n| !n.to_string_lossy().starts_with(".settings-")),
                "{names:?}"
            );
        }
    }

    #[test]
    fn env_block_and_plugins() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("settings.json"),
            r#"{"env":{"A":"1","N":5},"enabledPlugins":{"a@m":true,"b@m.x":false}}"#,
        )
        .unwrap();
        let mut s = Snapshot::read(d.path(), &[], env(&[])).unwrap();
        assert_eq!(
            s.env_block(),
            BTreeMap::from([("A".into(), "1".into()), ("N".into(), "5".into())])
        );
        assert_eq!(
            s.plugins(),
            vec![("a@m".into(), true), ("b@m.x".into(), false)]
        );
        s.apply(&[
            Edit::set_in("enabledPlugins", "b@m.x", true),
            Edit::delete_in("env", "A"),
        ])
        .unwrap();
        assert_eq!(s.plugins()[1], ("b@m.x".into(), true), "a dot in a name");
        assert!(!s.env_block().contains_key("A"));
    }
}
