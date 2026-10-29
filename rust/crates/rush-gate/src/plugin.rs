//! The gate plugin's side the gate's commands need: its settings, read
//! from the plugins' files as Go's plugin package keeps them (whether it's
//! on, and each setting's value or default), the shims and node preload
//! it keeps in step, and which rush they run. Running the plugin on its
//! broker connection is the plugin module's.
//! Ported from internal/bundled/gate/plugin.go:1-283, and the parts of
//! internal/plugin/bundled.go:100-140 (BundledOn) and uisettings.go:20-95
//! (SettingValuesOf, checkSetting) and ui.go:372-386 (printable) it reads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::gate::{bin_dir, mkdir_all, write_file};
use crate::hook::quote;
use crate::node::{PRELOAD, names_path, node_dir, preload_path};
use crate::rules::{self, Rules, Settings, default_of};

/// The plugin's name.
pub const NAME: &str = "gate";

/// When set, taken as the per-program limits with the gate on, over the
/// settings: for scripts and tests.
pub const RULES_ENV: &str = "RUSH_GATE_RULES";

/// The longest a text setting's value may be, in bytes.
const MAX_SETTING_TEXT: usize = 1000;

/// One of the plugin's settings, as its manifest offers it.
struct Spec {
    key: &'static str,
    kind: Kind,
    choices: &'static [&'static str],
}

#[derive(PartialEq)]
enum Kind {
    Text,
    Choice,
}

/// The gate's settings, as its manifest has them.
const SPECS: &[Spec] = &[
    Spec {
        key: "names",
        kind: Kind::Text,
        choices: &[],
    },
    Spec {
        key: "limit",
        kind: Kind::Choice,
        choices: &["10m", "15m", "30m", "1h", "2h"],
    },
    Spec {
        key: "reserve",
        kind: Kind::Choice,
        choices: &["5%", "10%", "15%", "20%", "25%"],
    },
    Spec {
        key: "floor",
        kind: Kind::Choice,
        choices: &["off", "3%", "5%", "8%", "10%"],
    },
    Spec {
        key: "limits",
        kind: Kind::Text,
        choices: &[],
    },
];

/// Where the gate plugin's settings are: rush's folder, and the
/// environment it's read with.
#[derive(Clone, Debug)]
pub struct Plugin {
    /// rush's folder: the plugins' files are in its plugins.
    pub home: PathBuf,
    /// RUSH_GATE_RULES, when it's set.
    pub rules_env: Option<String>,
}

impl Default for Plugin {
    fn default() -> Self {
        Plugin::new()
    }
}

impl Plugin {
    /// The gate plugin as this process sees it.
    pub fn new() -> Plugin {
        Plugin {
            home: rush_state::dir(),
            rules_env: std::env::var(RULES_ENV).ok(),
        }
    }

    fn plugins(&self) -> PathBuf {
        self.home.join("plugins")
    }

    /// Whether you have the gate on: an optional bundled plugin is on when
    /// bundled-on.json lists it.
    pub fn on(&self) -> bool {
        let Ok(b) = std::fs::read(self.plugins().join("bundled-on.json")) else {
            return false;
        };
        rush_state::json::from_slice::<Vec<String>>(&b)
            .unwrap_or_default()
            .iter()
            .any(|n| n == NAME)
    }

    /// Its settings: what you set, or each one's default. A stored value
    /// its settings no longer allow is taken as unset.
    pub fn settings(&self) -> Settings {
        let stored: HashMap<String, HashMap<String, String>> =
            std::fs::read(self.plugins().join("settings.json"))
                .ok()
                .and_then(|b| rush_state::json::from_slice(&b).ok())
                .unwrap_or_default();
        let mine = stored.get(NAME);
        let mut out = Settings::new();
        for s in SPECS {
            let v = match mine.and_then(|m| m.get(s.key)) {
                Some(v) if allowed(s, v) => v.clone(),
                _ => default_setting(s).to_string(),
            };
            out.insert(s.key.to_string(), v);
        }
        out
    }

    /// The rule for each program, or None when the gate is off.
    pub fn rules(&self) -> Option<Rules> {
        if let Some(v) = &self.rules_env {
            return Some(rules::parse_rules(&Settings::from([
                ("names".to_string(), String::new()),
                ("limits".to_string(), v.clone()),
            ])));
        }
        if !self.on() {
            return None;
        }
        Some(rules::parse_rules(&self.settings()))
    }

    /// The share of memory kept free of gated runs.
    pub fn reserve(&self) -> f64 {
        if self.rules_env.is_some() || !self.on() {
            return rules::parse_reserve(&Settings::new());
        }
        rules::parse_reserve(&self.settings())
    }

    /// The memory left, in percent, at which the guard stops agents'
    /// programs; 0 when it's off or the gate is.
    pub fn floor(&self) -> i32 {
        if self.rules_env.is_some() {
            return rules::parse_floor(&Settings::new());
        }
        if !self.on() {
            return 0;
        }
        rules::parse_floor(&self.settings())
    }

    /// Makes the gate's bin folder hold a shim for each program the gate
    /// queues, and nothing else: none when it's off. It keeps the node
    /// preload in step too.
    pub fn write_shims(&self, root: &Path) {
        self.write_shims_only(root);
        self.write_node(root);
    }

    fn write_shims_only(&self, root: &Path) {
        let d = bin_dir(root);
        let exe = exe_for(&d);
        if exe.is_empty() || mkdir_all(&d).is_err() {
            return;
        }
        let rules = self.rules().unwrap_or_default();
        if let Ok(ents) = std::fs::read_dir(&d) {
            for e in ents.flatten() {
                if !rules.contains_key(&*e.file_name().to_string_lossy()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        for name in rules.keys() {
            if name.contains('/') || name.starts_with('.') {
                continue;
            }
            let path = d.join(name);
            let want = format!(
                "#!/bin/sh\nexec {} gate shim {} \"$@\"\n",
                quote(&exe),
                quote(name)
            );
            if std::fs::read(&path).is_ok_and(|b| b == want.as_bytes()) {
                continue;
            }
            let tmp = crate::gate::with_ext(&path, ".tmp");
            if write_file(&tmp, want.as_bytes(), 0o700).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// Writes the node preload, always, since a session's NODE_OPTIONS
    /// names it for its whole life, and the names it reads: none when the
    /// gate is off, so it does nothing.
    fn write_node(&self, root: &Path) {
        let d = node_dir(root);
        if mkdir_all(&d).is_err() {
            return;
        }
        write_if_changed(&preload_path(root), PRELOAD.as_bytes());
        let exe = exe_for(&d);
        let rules = match self.rules() {
            Some(r) if !exe.is_empty() => r,
            _ => {
                let _ = std::fs::remove_file(names_path(root));
                return;
            }
        };
        let names: Vec<&str> = rules.keys().map(String::as_str).collect(); // sorted: a BTreeMap
        write_if_changed(
            &names_path(root),
            format!("{exe}\n{}\n", names.join("\n")).as_bytes(),
        );
    }
}

/// NODE_OPTIONS with the preload required first, or as it is when it's
/// there already or the preload isn't written.
pub fn node_options(root: &Path, cur: &str) -> String {
    let p = preload_path(root).to_string_lossy().into_owned();
    if cur.contains(&p) || p.contains([' ', '"', '\'', '\\']) {
        return cur.to_string();
    }
    if std::fs::metadata(&p).is_err() {
        return cur.to_string();
    }
    format!("--require {p} {cur}").trim().to_string()
}

fn write_if_changed(path: &Path, want: &[u8]) {
    if std::fs::read(path).is_ok_and(|b| b == want) {
        return;
    }
    let tmp = crate::gate::with_ext(path, ".tmp");
    if write_file(&tmp, want, 0o600).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The running rush, links resolved, or "".
pub fn exe() -> String {
    let Ok(e) = std::env::current_exe() else {
        return String::new();
    };
    let e = std::fs::canonicalize(&e).unwrap_or(e);
    e.to_string_lossy().into_owned()
}

/// The rush a stand-in written into dir runs: the running one, unless
/// that's a scratch build (a test's, one in the temp folder) and dir
/// outlives it; then the rush on PATH, or "" for none.
pub fn exe_for(dir: &Path) -> String {
    let exe = exe();
    if !scratch(Path::new(&exe)) || scratch(dir) {
        return exe;
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let Some(p) = crate::cli::look_path_in("rush", &path) else {
        return String::new();
    };
    let p = std::fs::canonicalize(&p).unwrap_or(p);
    if scratch(&p) {
        return String::new();
    }
    p.to_string_lossy().into_owned()
}

/// Whether path is somewhere that comes and goes: the temp folder, go's
/// build cache, or cargo's test builds (target/<profile>/deps).
fn scratch(path: &Path) -> bool {
    let tmp = std::env::temp_dir();
    let tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
    let p = path.to_string_lossy();
    for d in [
        tmp.to_string_lossy().into_owned(),
        "/tmp".into(),
        "/private/tmp".into(),
    ] {
        let d = d.trim_end_matches('/');
        if p.starts_with(&format!("{d}/")) {
            return true;
        }
    }
    p.contains("/go-build") || p.contains("/deps/")
}

/// Claude Code's PreToolUse hook for Bash: it gates what the gate's rules
/// name, and runs a long call as a script rush can follow. "" when there's
/// no rush to run it.
pub fn hook_command() -> String {
    let e = exe();
    if e.is_empty() {
        return String::new();
    }
    format!("{} gate hook", quote(&e))
}

fn default_setting(s: &Spec) -> &'static str {
    let d = default_of(s.key);
    if !d.is_empty() {
        return d;
    }
    if s.kind == Kind::Choice {
        return s.choices.first().copied().unwrap_or("");
    }
    ""
}

/// Whether v is a value setting s allows.
fn allowed(s: &Spec, v: &str) -> bool {
    match s.kind {
        Kind::Choice => s.choices.contains(&v),
        Kind::Text => v.len() <= MAX_SETTING_TEXT && printable(v),
    }
}

/// Whether s has nothing a text setting can't hold: control characters
/// (a tab too), direction marks, or bytes that aren't text.
fn printable(s: &str) -> bool {
    !s.chars().any(|r| {
        let r = r as u32;
        r < 0x20
            || r == 0x7f
            || (0x80..0xa0).contains(&r)
            || (0x202a..=0x202e).contains(&r)
            || (0x2066..=0x2069).contains(&r)
            || r == 0x200e
            || r == 0x200f
            || r == 0xfffd
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(home: &Path, env: Option<&str>) -> Plugin {
        Plugin {
            home: home.to_path_buf(),
            rules_env: env.map(String::from),
        }
    }

    // plugin_test.go TestShimsFollowTheRules
    #[test]
    fn shims_follow_the_rules() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("gate");
        plugin(home.path(), Some("tsc=1, go")).write_shims(&root);
        let b = std::fs::read_to_string(bin_dir(&root).join("tsc")).unwrap();
        assert!(b.contains(" gate shim 'tsc' \"$@\""), "{b}");
        let names = std::fs::read_to_string(names_path(&root)).unwrap();
        assert!(names.ends_with("\ngo\ntsc\n"), "{names}");
        assert_eq!(
            std::fs::read_to_string(preload_path(&root)).unwrap(),
            PRELOAD
        );
        plugin(home.path(), Some("go")).write_shims(&root);
        assert!(
            !bin_dir(&root).join("tsc").exists(),
            "tsc's shim stayed after it left the rules"
        );
        plugin(home.path(), None).write_shims(&root);
        assert!(!bin_dir(&root).join("go").exists());
        assert!(!names_path(&root).exists());
        let opts = node_options(&root, "--max-old-space-size=4096");
        if !preload_path(&root).to_string_lossy().contains(' ') {
            assert_eq!(
                opts,
                format!(
                    "--require {} --max-old-space-size=4096",
                    preload_path(&root).display()
                )
            );
            assert_eq!(node_options(&root, &opts), opts);
        }
    }

    // plugin_test.go TestOffHasNoRules
    #[test]
    fn off_has_no_rules() {
        let home = tempfile::tempdir().unwrap();
        let p = plugin(home.path(), None);
        assert_eq!(p.rules(), None);
        assert_eq!(p.floor(), 0);
        assert_eq!(p.reserve(), 0.15);
    }

    // On, its settings are read as Go keeps them; ones it doesn't allow
    // are taken as unset.
    #[test]
    fn on_reads_the_settings() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("plugins");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bundled-on.json"), r#"["gate","watch"]"#).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"gate":{"names":"tsc","limit":"7m","reserve":"25%","floor":"off","limits":"tsc=6G"}}"#,
        )
        .unwrap();
        let p = plugin(home.path(), None);
        let rules = p.rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules["tsc"].memory, 6 << 30);
        assert_eq!(rules["tsc"].limit, std::time::Duration::from_secs(1800));
        assert_eq!(p.reserve(), 0.25);
        assert_eq!(p.floor(), 0);
        assert!(!printable("a\tb"));
        assert!(printable("tsc=6G/45m, vitest=4G"));
    }

    // plugin_test.go TestScratchBuildsKeepToScratch: a test's own build
    // runs the stand-ins it writes into a temp folder, but never the ones
    // a real folder keeps for every agent.
    #[test]
    fn scratch_builds_keep_to_scratch() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(exe_for(tmp.path()), exe());
        // A home's folder, not the test's own home: that's in /tmp
        // (rush-testenv), which is scratch.
        let real = Path::new("/Users/someone/Library/Caches/agtop/shims");
        let got = exe_for(real);
        assert!(
            got != exe() && !scratch(Path::new(&got)) || got.is_empty(),
            "{got}"
        );
    }
}
