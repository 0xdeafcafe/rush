//! Finding the savers set up in an account. Port of
//! internal/efficiency/detect.go.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use rush_core::adapter::Profile;
use rush_state::Time;
use rush_state::settingsfile;
use serde::Deserialize;

use crate::catalog::{Recipe, Saver, Setting};
use crate::source::source;

/// How far a saver is set up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "i64", into = "i64")]
pub enum Status {
    #[default]
    Off,
    /// Found, but not doing anything yet: rtk's binary without its hook.
    Partial,
    On,
}

impl From<i64> for Status {
    fn from(n: i64) -> Status {
        match n {
            1 => Status::Partial,
            2 => Status::On,
            _ => Status::Off,
        }
    }
}

impl From<Status> for i64 {
    fn from(s: Status) -> i64 {
        s as i64
    }
}

/// What detection found of a saver.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub status: Status,
    /// What was found, in words: "hook in settings.json".
    pub parts: Vec<String>,
    /// What's missing when Partial.
    pub wants: String,
    /// When it was installed, when that's known.
    pub since: Time,
    /// A setting's value.
    pub value: String,
}

/// A home's setup as detection needs it, read once per look.
pub struct Env {
    pub profile: Profile,
    /// Its own settings.
    pub settings: settingsfile::File,
    /// Every hook's command.
    pub hooks: Vec<String>,
    /// Plugins turned on, by id.
    pub enabled: HashMap<String, bool>,
    /// Plugins installed, by id: when, if known.
    pub plugins: HashMap<String, Time>,
    /// MCP servers, by name.
    pub mcp: HashMap<String, bool>,
    /// The files an install may change, copied first.
    pub backup: Vec<String>,
    /// The environment its CLI runs in for that home.
    pub env: Vec<String>,
}

impl Env {
    /// A home's settings, plugins and MCP servers, through the agent's
    /// Source.
    pub fn load(p: &Profile) -> Env {
        let s = source().setup(p);
        Env {
            profile: p.clone(),
            settings: s
                .settings
                .or_else(|| settingsfile::File::load("/dev/null").ok())
                .unwrap_or_else(|| {
                    // A file in a folder that isn't there reads as empty.
                    settingsfile::File::load(
                        std::env::temp_dir()
                            .join(".rush-no-settings-folder")
                            .join(format!("{}", std::process::id()))
                            .join("settings.json"),
                    )
                    .expect("a missing settings file reads as empty")
                }),
            hooks: s.hooks,
            enabled: s.enabled,
            plugins: s.plugins,
            mcp: s.mcp,
            backup: s.backup,
            env: s.env,
        }
    }
}

thread_local! {
    /// This thread's own search path, over PATH and package managers'
    /// folders: for tests, which mustn't see what's installed here.
    static SEARCH: RefCell<Option<Vec<PathBuf>>> = const { RefCell::new(None) };
}

/// Sets this thread's search path; None goes back to the PATH. For tests.
pub fn set_thread_search_path(dirs: Option<Vec<PathBuf>>) {
    SEARCH.with(|s| *s.borrow_mut() = dirs);
}

/// Where programs are looked for: the PATH, and where package managers
/// put them, which a GUI-started rush's PATH may miss.
pub fn search_path() -> Vec<PathBuf> {
    if let Some(d) = SEARCH.with(|s| s.borrow().clone()) {
        return d;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".local").join("bin"),
        home.join(".cargo").join("bin"),
        home.join("go").join("bin"),
    ]);
    dirs
}

/// Finds a program the way detection does; "" when it isn't anywhere.
pub fn look_path(name: &str) -> String {
    if name.contains('/') {
        return if is_exec(Path::new(name)) {
            name.to_string()
        } else {
            String::new()
        };
    }
    for d in search_path() {
        if d.as_os_str().is_empty() {
            continue;
        }
        let p = d.join(name);
        if is_exec(&p) {
            return p.to_string_lossy().into_owned();
        }
    }
    String::new()
}

fn is_exec(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|st| !st.is_dir() && st.permissions().mode() & 0o111 != 0)
}

impl Env {
    /// Looks for a saver in this account.
    pub fn detect(&self, s: &Saver) -> Found {
        if let Some(st) = &s.setting {
            return self.detect_setting(st);
        }
        let mut f = Found::default();
        let d = &s.detect;
        let (mut bin, mut hook, mut plugin, mut mcp) = (false, false, false, false);
        for b in &d.bins {
            let p = look_path(b);
            if !p.is_empty() {
                bin = true;
                f.parts.push(format!("{b} installed"));
                let t = brew_time(&p);
                if !t.is_zero() {
                    f.since = t;
                }
            }
        }
        for h in &d.hooks {
            for c in &self.hooks {
                if c.contains(h) && !hook {
                    hook = true;
                    f.parts.push("hook in settings.json".into());
                }
            }
        }
        for id in &d.plugins {
            if let Some(t) = self.plugins.get(*id) {
                if self.enabled.get(*id).copied().unwrap_or(false) {
                    plugin = true;
                    f.parts.push(format!("plugin {id} on"));
                } else {
                    f.parts.push(format!("plugin {id} off"));
                }
                if !t.is_zero() {
                    f.since = *t;
                }
            }
        }
        let mut servers: Vec<&String> = self.mcp.keys().collect();
        servers.sort();
        let mut ids: Vec<&String> = self.enabled.keys().collect();
        ids.sort();
        for m in &d.mcp {
            for name in &servers {
                if name.as_str() == *m || name.contains(m) {
                    mcp = true;
                    f.parts.push(format!("MCP server {name}"));
                }
            }
            for id in &ids {
                if id.contains(m) && self.enabled[*id] && !plugin {
                    mcp = true;
                    f.parts.push(format!("plugin {id}"));
                }
            }
        }
        let mut file = false;
        for p in &d.files {
            if Path::new(&self.profile.dir).join(p).exists() {
                file = true;
                f.parts.push(p.to_string());
            }
        }
        let any = bin || hook || plugin || mcp || file;
        if !any {
            f.status = Status::Off;
        } else if d.need == "hook" && !hook {
            f.status = Status::Partial;
            f.wants = "its hook isn't in settings.json".into();
        } else if d.need == "mcp" && !mcp {
            f.status = Status::Partial;
            f.wants = "it isn't added as an MCP server".into();
        } else if !d.plugins.is_empty() && !plugin && !hook {
            f.status = Status::Partial;
            f.wants = "the plugin is off".into();
        } else {
            f.status = Status::On;
        }
        f
    }

    fn detect_setting(&self, st: &Setting) -> Found {
        let mut f = Found::default();
        let value = if st.env {
            self.settings.env().get(st.key).cloned()
        } else {
            self.settings
                .get::<serde_json::Value>(st.key)
                .map(|v| match v {
                    serde_json::Value::String(s) => s,
                    v => v.to_string(),
                })
        };
        let Some(value) = value else {
            return f;
        };
        f.value = value;
        if st.exact && f.value != st.value.to_string() {
            return f;
        }
        f.status = Status::On;
        f.parts = vec![format!("{} = {}", st.key, f.value)];
        f
    }
}

/// When Homebrew installed the program at p, from its receipt.
fn brew_time(p: &str) -> Time {
    let Ok(real) = std::fs::canonicalize(p) else {
        return Time::default();
    };
    if !real.to_string_lossy().contains("/Cellar/") {
        return Time::default();
    }
    // …/Cellar/<name>/<version>/bin/<name>
    let Some(dir) = real.parent().and_then(|d| d.parent()) else {
        return Time::default();
    };
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Receipt {
        time: i64,
    }
    match std::fs::read(dir.join("INSTALL_RECEIPT.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Receipt>(&b).ok())
    {
        Some(r) if r.time != 0 => Time::from_unix(r.time, 0),
        _ => Time::default(),
    }
}

/// The recipe to use: the first whose program is found; else why none.
pub fn pick(rs: &[Recipe]) -> Result<&Recipe, String> {
    let mut missing = Vec::new();
    for r in rs {
        if r.needs.is_empty() || !look_path(r.needs).is_empty() {
            return Ok(r);
        }
        missing.push(r.needs);
    }
    if missing.is_empty() {
        return Err("nothing to run".into());
    }
    Err(format!("needs {}", missing.join(" or ")))
}

impl Recipe {
    /// The commands it runs here, steps already done left out.
    pub fn plan(&self) -> Vec<Vec<String>> {
        self.steps
            .iter()
            .filter(|s| s.skip.is_empty() || look_path(s.skip).is_empty())
            .map(|s| s.argv.iter().map(|a| a.to_string()).collect())
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::catalog::find;

    /// An account in a folder of its own, on a machine with no programs:
    /// what's installed here mustn't change what tests see.
    pub fn test_account(dir: &Path, settings: &str) -> Profile {
        set_thread_search_path(Some(Vec::new()));
        if !settings.is_empty() {
            std::fs::write(dir.join("settings.json"), settings).unwrap();
        }
        Profile {
            kind: crate::AGENT.into(),
            name: "test".into(),
            dir: dir.to_string_lossy().into_owned(),
        }
    }

    #[test]
    fn detects() {
        let tmp = tempfile::tempdir().unwrap();
        let a = test_account(
            tmp.path(),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude"}]}]},
		"enabledPlugins":{"caveman@caveman":true},
		"bashOutputMaxChars":15000,
		"env":{"CLAUDE_CODE_SUBAGENT_MODEL":"sonnet"}}"#,
        );
        std::fs::create_dir_all(tmp.path().join("plugins")).unwrap();
        std::fs::write(
            tmp.path().join("plugins").join("installed_plugins.json"),
            r#"{"version":2,"plugins":{"caveman@caveman":[{"scope":"user","installedAt":"2026-09-01T10:00:00Z"}]}}"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join(".claude.json"),
            r#"{"mcpServers":{"serena":{"command":"serena"}}}"#,
        )
        .unwrap();
        let e = Env::load(&a);
        let f = e.detect(find("rtk").unwrap());
        assert_ne!(
            f.status,
            Status::Off,
            "rtk's hook is in settings.json: {f:?}"
        );
        let cave = e.detect(find("caveman").unwrap());
        assert!(
            cave.status == Status::On && !cave.since.is_zero(),
            "{cave:?}"
        );
        assert_eq!(e.detect(find("serena").unwrap()).status, Status::On);
        let f = e.detect(find("bash-output").unwrap());
        assert!(f.status == Status::On && f.value == "15000", "{f:?}");
        let f = e.detect(find("subagent-model").unwrap());
        assert!(f.status == Status::On && f.value == "sonnet", "{f:?}");
        assert_eq!(e.detect(find("autocompact").unwrap()).status, Status::Off);
    }
}
