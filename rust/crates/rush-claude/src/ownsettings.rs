//! Claude Code's HarnessSettings: what its Harnesses page offers of
//! settings.json, the env block and its plugins, each with its value and
//! where that comes from, and what a change to each writes. Rush-only:
//! Go folded a few of these keys under Advanced (settingspage.go). The
//! reading and writing is usersettings'.

use std::io;
use std::path::{Path, PathBuf};

use rush_core::adapter::{OwnEdit, OwnGroup, OwnKind, OwnSetting, OwnSettings, ValueFrom};
use serde_json::Value;

use crate::usersettings::{self, Edit, Shown, Snapshot};

/// The key of the auto-compact row: a window, off or on.
pub const AUTO_COMPACT: &str = "autoCompact";
/// The key of the status line's command.
pub const STATUS_LINE: &str = "statusLine";
/// A variable's key is this and its name; a new one's is this alone.
pub const VAR: &str = "env:";
/// A plugin's key is this and its id.
pub const PLUGIN: &str = "plugin:";
/// disableAllHooks, shown the right way round: on runs them.
const HOOKS: &str = "hooks";

/// The keys set as strings, "" removing them.
const STRINGS: [&str; 4] = ["model", "effortLevel", "permissions.defaultMode", "theme"];
/// The keys set as true or false.
const FLAGS: [&str; 3] = [
    "includeCoAuthoredBy",
    "agentPushNotifEnabled",
    "alwaysThinkingEnabled",
];
/// The keys set as whole numbers.
const NUMBERS: [&str; 1] = ["cleanupPeriodDays"];

/// The other config folders a change goes to: each rush login's home.
pub fn homes() -> Vec<PathBuf> {
    rush_state::Store::load()
        .config
        .logins
        .iter()
        .map(|l| crate::home::home_of(&l.id).config_dir)
        .collect()
}

/// Claude Code's settings in dir, with env the environment rush runs in
/// and homes the other folders written to.
pub fn read(dir: &Path, homes: &[PathBuf], env: &[(String, String)]) -> io::Result<OwnSettings> {
    let s = Snapshot::read(dir, homes, env.iter().cloned())?;
    Ok(page(&s))
}

/// Writes edits into the settings.json of dir and every home, each
/// checked first.
pub fn write(dir: &Path, homes: &[PathBuf], edits: &[OwnEdit]) -> io::Result<()> {
    let mut all = Vec::new();
    for e in edits {
        all.extend(
            edits_for(&e.key, &e.value)
                .map_err(|why| io::Error::new(io::ErrorKind::InvalidInput, why))?,
        );
    }
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(homes.iter().cloned());
    usersettings::write(&usersettings::settings_paths(&dirs), &all)
}

/// What setting key to value writes; Err says why it can't be.
pub fn edits_for(key: &str, value: &str) -> Result<Vec<Edit>, String> {
    let v = value.trim();
    if key == AUTO_COMPACT {
        return usersettings::compact_edits(v);
    }
    if key == STATUS_LINE {
        return Ok(if v.is_empty() || v == "none" {
            vec![Edit::delete("statusLine")]
        } else {
            vec![
                Edit::set("statusLine.type", "command"),
                Edit::set("statusLine.command", v),
            ]
        });
    }
    if key == VAR {
        let (name, val) = v
            .split_once('=')
            .map(|(a, b)| (a.trim(), b.trim()))
            .unwrap_or(("", ""));
        if name.is_empty() || name.contains([' ', '\t', '.']) {
            return Err("type it as NAME=value".into());
        }
        return edits_for(&format!("{VAR}{name}"), val);
    }
    if let Some(name) = key.strip_prefix(VAR) {
        return Ok(vec![if v.is_empty() {
            Edit::delete_in("env", name)
        } else {
            Edit::set_in("env", name, Value::String(v.into()))
        }]);
    }
    if let Some(id) = key.strip_prefix(PLUGIN) {
        return match v {
            "on" | "off" => Ok(vec![Edit::set_in("enabledPlugins", id, v == "on")]),
            _ => Err(format!("{id} is on or off")),
        };
    }
    let flag = |k: &str, on: bool| match v {
        "on" => Ok(vec![Edit::set(k, on)]),
        "off" => Ok(vec![Edit::set(k, !on)]),
        "" => Ok(vec![Edit::delete(k)]),
        _ => Err(format!(
            "{v:?}: on, off, or nothing for Claude Code's default"
        )),
    };
    if key == HOOKS {
        return flag("disableAllHooks", false);
    }
    if FLAGS.contains(&key) {
        return flag(key, true);
    }
    if NUMBERS.contains(&key) {
        if v.is_empty() {
            return Ok(vec![Edit::delete(key)]);
        }
        return v
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= 0)
            .map(|n| vec![Edit::set(key, n)])
            .ok_or_else(|| format!("{v:?} isn't a whole number"));
    }
    if STRINGS.contains(&key) {
        return Ok(vec![if v.is_empty() {
            Edit::delete(key)
        } else {
            Edit::set(key, v)
        }]);
    }
    Err(format!("rush doesn't change {key}"))
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn item(
    key: &str,
    label: &str,
    kind: OwnKind,
    shown: Shown,
    what: &str,
    choices: &[(&str, &str)],
    unset: &str,
) -> OwnSetting {
    OwnSetting {
        key: key.into(),
        label: label.into(),
        what: what.into(),
        kind,
        value: shown.value,
        from: shown.from,
        choices: pairs(choices),
        unset: unset.into(),
    }
}

const ON_OFF: [(&str, &str); 3] = [("", "Claude Code's own default."), ("on", ""), ("off", "")];

/// What the page shows of s.
pub fn page(s: &Snapshot) -> OwnSettings {
    let settings = vec![
        item(
            AUTO_COMPACT,
            "Auto-compact",
            OwnKind::Tokens,
            s.auto_compact(),
            "When a conversation nears this many tokens, Claude Code compacts it: summarises what came before so the session can go on. Type any window from 100k to 1M (400k and 400000 are the same), off, or on for the model's own window. Writes autoCompactWindow and autoCompactEnabled; CLAUDE_CODE_AUTO_COMPACT_WINDOW or DISABLE_AUTO_COMPACT in the environment win over them.",
            &[
                ("", "on, at the model's own context window."),
                ("off", "never compacts by itself; /compact still does."),
                ("200k", "compacts near 200k tokens."),
                ("300k", "compacts near 300k tokens."),
                ("400k", "compacts near 400k tokens."),
                ("500k", "compacts near 500k tokens."),
                ("600k", "compacts near 600k tokens."),
                ("800k", "compacts near 800k tokens."),
                (
                    "1M",
                    "compacts near 1M tokens, on a model with a 1M window.",
                ),
            ],
            "on · model's window",
        ),
        item(
            "model",
            "Default model",
            OwnKind::Open,
            s.string("model", Some("ANTHROPIC_MODEL")),
            "The model every session starts with unless rush or the session picks one. ANTHROPIC_MODEL in the environment wins over it.",
            &[
                ("", "Claude Code's own default for your plan."),
                ("opus", ""),
                ("opus[1m]", "Opus with a 1M-token window."),
                ("sonnet", ""),
                ("haiku", ""),
                ("fable", ""),
            ],
            "your plan's default",
        ),
        item(
            "effortLevel",
            "Effort",
            OwnKind::Choice,
            s.string("effortLevel", Some("CLAUDE_CODE_EFFORT_LEVEL")),
            "How hard sessions think by default; /effort changes one session. CLAUDE_CODE_EFFORT_LEVEL in the environment wins over it, and rush's own Effort for new sessions (Every harness) over both.",
            &[
                ("", "the model's own default."),
                ("low", "quick, for small changes."),
                ("medium", ""),
                ("high", ""),
                ("xhigh", "the most thinking a model offers below max."),
            ],
            "model's default",
        ),
        item(
            "permissions.defaultMode",
            "Permission mode",
            OwnKind::Choice,
            s.string("permissions.defaultMode", None),
            "What sessions may do without asking. Your allow and deny rules are kept.",
            &[
                ("", "asks as Claude Code does by default."),
                (
                    "default",
                    "asks before edits and commands not allowed already.",
                ),
                (
                    "acceptEdits",
                    "edits files without asking; commands still ask.",
                ),
                ("plan", "plans first and changes nothing until you agree."),
                (
                    "auto",
                    "decides for itself what's safe to do without asking.",
                ),
                ("dontAsk", "never asks: what isn't allowed is refused."),
                ("bypassPermissions", "does everything without asking."),
            ],
            "default · asks",
        ),
        item(
            "theme",
            "Theme",
            OwnKind::Open,
            s.string("theme", None),
            "Claude Code's own colours, in its terminal screen (rush's are Settings › General's). A custom theme is typed as custom:<name>.",
            &[
                ("", "Claude Code picks."),
                ("auto", "follows the terminal's light or dark."),
                ("dark", ""),
                ("light", ""),
                ("dark-daltonized", "dark, for colour blindness."),
                ("light-daltonized", "light, for colour blindness."),
                ("dark-ansi", "dark, in the terminal's own 16 colours."),
                ("light-ansi", "light, in the terminal's own 16 colours."),
            ],
            "not set",
        ),
        item(
            "includeCoAuthoredBy",
            "Co-authored-by in commits",
            OwnKind::OnOff,
            s.flag("includeCoAuthoredBy", false),
            "Whether Claude adds a Co-Authored-By line to the commits it makes. Claude Code reads attribution first now; Credit in commits and PRs (Every harness) sets both for every agent.",
            &[
                ("", "Claude Code's own default: it adds the line."),
                ("on", "it adds the line."),
                ("off", "no line."),
            ],
            "on",
        ),
        item(
            "agentPushNotifEnabled",
            "Push to your phone",
            OwnKind::OnOff,
            s.flag("agentPushNotifEnabled", true),
            "Whether Claude may push notifications to the Claude app on your phone on its own (agentPushNotifEnabled).",
            &[
                ("", "Claude Code's own default."),
                ("on", "it may."),
                ("off", "it doesn't."),
            ],
            "not set",
        ),
        {
            let cmd = s.status_line();
            let from = if cmd.is_empty() {
                ValueFrom::Default
            } else {
                ValueFrom::File
            };
            item(
                STATUS_LINE,
                "Status line",
                OwnKind::Text,
                Shown::new(cmd, from),
                "The command whose output Claude Code shows under its prompt (statusLine.command). #statusline builds one from rush's segments. Empty takes it out.",
                &[],
                "none",
            )
        },
        item(
            "alwaysThinkingEnabled",
            "Always think",
            OwnKind::OnOff,
            s.flag("alwaysThinkingEnabled", false),
            "Extended thinking on every request (alwaysThinkingEnabled).",
            &ON_OFF,
            "not set",
        ),
        item(
            "cleanupPeriodDays",
            "Keep transcripts for",
            OwnKind::Number,
            s.int("cleanupPeriodDays"),
            "How many days Claude Code keeps old transcripts before cleaning them up (cleanupPeriodDays).",
            &[
                ("", "Claude Code's own default: 30 days."),
                ("7", ""),
                ("30", ""),
                ("90", ""),
                ("365", ""),
            ],
            "30",
        ),
        {
            let mut h = s.flag("disableAllHooks", false);
            if !h.value.is_empty() {
                h.value = if h.value == "on" { "off" } else { "on" }.into();
            }
            item(
                HOOKS,
                "Hooks",
                OwnKind::OnOff,
                h,
                "Whether the hooks in your settings run. Off stops every hook without deleting them (disableAllHooks).",
                &[
                    ("", "Claude Code's own default: they run."),
                    ("on", "they run."),
                    ("off", "none runs; they're kept."),
                ],
                "on",
            )
        },
    ];
    let path = s.file.path.to_string_lossy().into_owned();
    let note = match s.paths.len() {
        0 | 1 => "every account reads it".to_string(),
        n => format!("written to {n} files: an account has its own copy"),
    };
    OwnSettings {
        file: tilde(&path),
        path,
        note,
        groups: vec![
            OwnGroup {
                title: "Claude Code's settings".into(),
                note: String::new(),
                items: settings,
            },
            OwnGroup {
                title: "Environment".into(),
                note: "settings.json's env: every session starts with these".into(),
                items: env_items(s),
            },
            OwnGroup {
                title: "Plugins".into(),
                note: "settings.json's enabledPlugins".into(),
                items: plugin_items(s),
            },
        ],
    }
}

/// The variables Claude Code's page offers with choices, then the rest
/// of the env block, then a row to add one.
fn env_items(s: &Snapshot) -> Vec<OwnSetting> {
    let env = s.env_block();
    let page = crate::settings::settings_page();
    let mut out = Vec::new();
    for e in &page.env {
        let mut choices = vec![(String::new(), "not set: Claude Code's own default.".into())];
        for c in &e.choices {
            choices.push((c.clone(), e.means.get(c).cloned().unwrap_or_default()));
        }
        let (value, from) = match env.get(&e.name) {
            Some(v) => (v.clone(), ValueFrom::EnvFile(e.name.clone())),
            None => (String::new(), ValueFrom::Default),
        };
        out.push(OwnSetting {
            key: format!("{VAR}{}", e.name),
            label: e.label.clone(),
            what: format!("{} (env {}).", e.what, e.name),
            kind: OwnKind::Var,
            value,
            from,
            choices,
            unset: "not set".into(),
        });
    }
    for (name, value) in &env {
        if page.env.iter().any(|e| e.name == *name) {
            continue;
        }
        out.push(OwnSetting {
            key: format!("{VAR}{name}"),
            label: name.clone(),
            what: "A variable in settings.json's env block: every Claude Code session starts with it, subagents included. Empty takes it out.".into(),
            kind: OwnKind::Var,
            value: value.clone(),
            from: ValueFrom::EnvFile(name.clone()),
            ..OwnSetting::default()
        });
    }
    out.push(OwnSetting {
        key: VAR.into(),
        label: "+ add a variable".into(),
        what: "Adds a variable to settings.json's env block: every Claude Code session starts with it, subagents included.".into(),
        kind: OwnKind::NewVar,
        ..OwnSetting::default()
    });
    out
}

/// The plugins settings.json names, each on or off.
fn plugin_items(s: &Snapshot) -> Vec<OwnSetting> {
    s.plugins()
        .into_iter()
        .map(|(id, on)| OwnSetting {
            key: format!("{PLUGIN}{id}"),
            label: id.clone(),
            what: format!(
                "The plugin {id}, as settings.json's enabledPlugins has it. Off keeps it installed but out of every session."
            ),
            kind: OwnKind::Plugin,
            value: if on { "on" } else { "off" }.into(),
            from: ValueFrom::File,
            choices: pairs(&[
                ("on", "every session has it."),
                ("off", "installed, but no session loads it."),
            ]),
            unset: String::new(),
        })
        .collect()
}

/// path with the home folder as ~.
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && path.starts_with(&h) => format!("~{}", &path[h.len()..]),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn find<'a>(o: &'a OwnSettings, key: &str) -> &'a OwnSetting {
        o.groups
            .iter()
            .flat_map(|g| g.items.iter())
            .find(|i| i.key == key)
            .unwrap_or_else(|| panic!("no {key}"))
    }

    #[test]
    fn edits_table() {
        for (key, v, ok) in [
            (AUTO_COMPACT, "400k", true),
            (AUTO_COMPACT, "50k", false),
            ("model", "", true),
            ("includeCoAuthoredBy", "maybe", false),
            ("cleanupPeriodDays", "x", false),
            ("cleanupPeriodDays", "14", true),
            ("env:", "FOO=bar", true),
            ("env:", "FOO", false),
            ("plugin:a@b.c", "on", true),
            ("plugin:a@b.c", "yes", false),
            ("somethingElse", "1", false),
        ] {
            assert_eq!(edits_for(key, v).is_ok(), ok, "{key} {v}");
        }
        assert_eq!(
            edits_for(HOOKS, "off").unwrap(),
            vec![Edit::set("disableAllHooks", true)]
        );
        assert_eq!(
            edits_for("env:", "FOO = bar").unwrap(),
            vec![Edit::set_in("env", "FOO", Value::String("bar".into()))]
        );
    }

    // Read and write through the generic shape: values with where they
    // come from, a write that keeps the rest and the link.
    #[test]
    fn read_and_write() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real");
        let home = d.path().join("home");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            real.join("settings.json"),
            r#"{"unknown":{"x":[1]},"disableAllHooks":true,"env":{"MINE":"1"}}"#,
        )
        .unwrap();
        symlink(real.join("settings.json"), home.join("settings.json")).unwrap();
        let env = vec![("ANTHROPIC_MODEL".to_string(), "sonnet".to_string())];
        let o = read(&home, &[], &env).unwrap();
        let m = find(&o, "model");
        assert_eq!(
            (m.value.as_str(), &m.from),
            ("sonnet", &ValueFrom::Env("ANTHROPIC_MODEL".into()))
        );
        assert_eq!(find(&o, HOOKS).value, "off");
        assert_eq!(find(&o, "env:MINE").from, ValueFrom::EnvFile("MINE".into()));
        assert_eq!(find(&o, AUTO_COMPACT).from, ValueFrom::Default);

        write(
            &home,
            &[],
            &[
                OwnEdit::new(AUTO_COMPACT, "400k"),
                OwnEdit::new("env:MINE", ""),
            ],
        )
        .unwrap();
        let got: Value =
            serde_json::from_str(&fs::read_to_string(real.join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            got,
            serde_json::json!({"unknown":{"x":[1]},"disableAllHooks":true,"autoCompactWindow":400000})
        );
        assert!(
            fs::symlink_metadata(home.join("settings.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(write(&home, &[], &[OwnEdit::new(AUTO_COMPACT, "2M")]).is_err());
    }
}
