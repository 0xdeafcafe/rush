//! Harnesses › Claude Code's own settings: what settings.json says (and
//! what overrides it), each row with where its value comes from, changed
//! in place for every account; the variables every session starts with;
//! the plugins it names. Rush-only: Go folded a few of these keys under
//! Advanced (settings_files.go:338 settingsFileSection, :406 envSection),
//! which files.rs still does for an agent that offers a settings page.
//! The reading and writing is rush_claude::usersettings'.

use std::rc::Rc;

use rush_claude::usersettings::{self, Edit, From, Shown, Snapshot};
use serde_json::Value;

use super::common::{LOGINS_KIND, agent_name};
use super::files::{agent_profile, edit_file};
use super::form::{RunFn, Section, Setting};
use super::{State, ask, config, dialog, dialog_mut};
use crate::frame::Out;
use crate::frame::confirm::Confirm;
use crate::model::Model;
use crate::style::*;

/// The key the read is noted under in what's been asked for.
const ASKED: &str = "claude/settings.json";

/// Reads Claude Code's settings again, off the UI, unless they were read a
/// moment ago: when k, the agent the page shows, is Claude Code.
pub fn want(m: &mut Model, k: &str) {
    if k != LOGINS_KIND {
        return;
    }
    let st = m.kept::<State>();
    if st
        .reads
        .asked
        .get(ASKED)
        .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(3))
    {
        return;
    }
    st.reads
        .asked
        .insert(ASKED.into(), std::time::Instant::now());
    read(m);
}

/// The config folders whose settings.json a change goes to besides the
/// one in use: each login's home.
fn homes(m: &Model) -> Vec<std::path::PathBuf> {
    config(m)
        .logins
        .iter()
        .map(|l| rush_claude::home::home_of(&l.id).config_dir)
        .collect()
}

#[cfg(test)]
thread_local! {
    /// The folder a test's page reads in place of the account's, and
    /// the environment it runs in.
    pub(crate) static TEST_DIR: std::cell::RefCell<Option<(std::path::PathBuf, Vec<(String, String)>)>> =
        const { std::cell::RefCell::new(None) };
}

/// The folder whose settings.json the page shows, and the environment
/// sessions start in.
fn source(m: &Model) -> Option<(std::path::PathBuf, Vec<(String, String)>)> {
    #[cfg(test)]
    if let Some(t) = TEST_DIR.with(|t| t.borrow().clone()) {
        return Some(t);
    }
    let p = agent_profile(m, LOGINS_KIND)?;
    Some((p.dir.into(), std::env::vars().collect()))
}

/// Reads them now, off the UI.
fn read(m: &mut Model) {
    let Some((dir, env)) = source(m) else {
        return;
    };
    let homes = homes(m);
    m.later(
        move || Snapshot::read(&dir, &homes, env).map_err(|e| e.to_string()),
        |m, r| {
            match r {
                Ok(s) => {
                    if m.kept.peek::<State>().is_some_and(|s| s.dialog.is_some()) {
                        dialog_mut(m).claude = Some(s);
                    }
                }
                Err(e) => m.fail(&format!("couldn't read Claude Code's settings.json: {e}")),
            }
            Out::Draw
        },
    );
}

/// Makes edits to what's shown at once, then writes them to every
/// settings.json off the UI, one write after another; said once done.
fn save(m: &mut Model, edits: Vec<Edit>, said: String) -> Out {
    let Some(s) = dialog_mut(m).claude.as_mut() else {
        return Out::Idle;
    };
    if let Err(e) = s.apply(&edits) {
        m.fail(&format!("couldn't change settings.json: {e}"));
        return Out::Draw;
    }
    let paths = s.paths.clone();
    m.later(
        move || {
            static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _one = WRITES.lock();
            usersettings::write(&paths, &edits).map_err(|e| e.to_string())
        },
        move |m, r| {
            match r {
                Err(e) => m.fail(&format!("couldn't save settings.json: {e}")),
                Ok(()) if !said.is_empty() => m.flash(&said),
                Ok(()) => {}
            }
            // Read again on the next key: what else changed shows.
            m.kept::<State>().reads.asked.remove(ASKED);
            Out::Draw
        },
    );
    Out::Draw
}

/// What a change said when something set in the environment still wins
/// over it.
fn still_wins(from: &From) -> String {
    format!("saved · {} still wins over it", from.said())
}

/// e on any row: settings.json in your editor.
fn open_key(m: &mut Model, s: &str) -> Out {
    if s == "e"
        && let Some(sn) = &dialog(m).claude
    {
        let p = sn.file.path.to_string_lossy().into_owned();
        return edit_file(m, &p);
    }
    Out::Pass
}

/// A row of Claude Code's settings: shown with where it comes from, and
/// what setting it to v writes (Err says why it can't be).
#[allow(clippy::too_many_arguments)]
fn row(
    label: &str,
    shown: &Shown,
    what: &str,
    choices: &[(&str, &str)],
    unset: &str,
    typed: bool,
    edits: impl Fn(&str) -> Result<Vec<Edit>, String> + 'static,
) -> Setting {
    let mut st = super::form::choice(label, &shown.value, what, choices, None);
    st.unset = unset.into();
    st.typed = typed;
    st.source = shown.from.word().into();
    st.keys = vec!["e".into(), "open settings.json".into()];
    st.key = Some(Rc::new(open_key));
    let from = shown.from.clone();
    let run: RunFn = Rc::new(move |m: &mut Model, v: &str| match edits(v) {
        Err(e) => {
            m.flash_err(&e);
            Out::Draw
        }
        Ok(ed) => {
            let said = if from.is_env() {
                still_wins(&from)
            } else {
                String::new()
            };
            save(m, ed, said)
        }
    });
    st.run = Some(run);
    let (title, what2, shown2) = (label.to_string(), what.to_string(), st.clone());
    let from2 = shown.from.clone();
    st.about = Some(Rc::new(move || {
        let mut now = shown2.now();
        now.push_str(&format!(" From {}.", from2.said()));
        (title.clone(), what2.clone(), now)
    }));
    st
}

/// Sets string key to v, or removes it for "".
fn string_edits(key: &str) -> impl Fn(&str) -> Result<Vec<Edit>, String> + 'static {
    let key = key.to_string();
    move |v: &str| {
        Ok(vec![if v.is_empty() {
            Edit::delete(&key)
        } else {
            Edit::set(&key, v)
        }])
    }
}

/// Sets flag key on or off, or removes it for "".
fn flag_edits(key: &str, invert: bool) -> impl Fn(&str) -> Result<Vec<Edit>, String> + 'static {
    let key = key.to_string();
    move |v: &str| {
        Ok(vec![match v {
            "on" => Edit::set(&key, !invert),
            "off" => Edit::set(&key, invert),
            _ => Edit::delete(&key),
        }])
    }
}

/// The settings.json rows.
fn settings_rows(s: &Snapshot) -> Vec<Setting> {
    let mut rows = Vec::new();
    let ac = s.auto_compact();
    rows.push(row(
        "Auto-compact",
        &ac,
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
            ("1M", "compacts near 1M tokens, on a model with a 1M window."),
        ],
        "on · model's window",
        true,
        usersettings::compact_edits,
    ));
    rows.push(row(
        "Default model",
        &s.string("model", Some("ANTHROPIC_MODEL")),
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
        true,
        string_edits("model"),
    ));
    rows.push(row(
        "Effort",
        &s.string("effortLevel", Some("CLAUDE_CODE_EFFORT_LEVEL")),
        "How hard sessions think by default; /effort changes one session. CLAUDE_CODE_EFFORT_LEVEL in the environment wins over it, and rush's own Effort for new sessions (Every harness) over both.",
        &[
            ("", "the model's own default."),
            ("low", "quick, for small changes."),
            ("medium", ""),
            ("high", ""),
            ("xhigh", "the most thinking a model offers below max."),
        ],
        "model's default",
        false,
        string_edits("effortLevel"),
    ));
    rows.push(row(
        "Permission mode",
        &s.string("permissions.defaultMode", None),
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
        false,
        string_edits("permissions.defaultMode"),
    ));
    rows.push(row(
        "Theme",
        &s.string("theme", None),
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
        true,
        string_edits("theme"),
    ));
    let on_off = |on: &str, off: &str| -> Vec<(String, String)> {
        vec![
            (String::new(), "Claude Code's own default.".into()),
            ("on".into(), on.into()),
            ("off".into(), off.into()),
        ]
    };
    let flag = |label: &str,
                key: &str,
                shown: Shown,
                what: &str,
                means: Vec<(String, String)>,
                invert: bool,
                unset: &str| {
        let pairs: Vec<(&str, &str)> = means
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        row(
            label,
            &shown,
            what,
            &pairs,
            unset,
            false,
            flag_edits(key, invert),
        )
    };
    rows.push(flag(
        "Co-authored-by in commits",
        "includeCoAuthoredBy",
        s.flag("includeCoAuthoredBy", false),
        "Whether Claude adds a Co-Authored-By line to the commits it makes. Claude Code reads attribution first now; Credit in commits and PRs (Every harness) sets both for every agent.",
        on_off("it adds the line.", "no line."),
        false,
        "on",
    ));
    rows.push(flag(
        "Push to your phone",
        "agentPushNotifEnabled",
        s.flag("agentPushNotifEnabled", true),
        "Whether Claude may push notifications to the Claude app on your phone on its own (agentPushNotifEnabled).",
        on_off("it may.", "it doesn't."),
        false,
        "not set",
    ));
    let cmd = s.status_line();
    let shown = Shown {
        value: cmd.clone(),
        from: if cmd.is_empty() {
            From::Default
        } else {
            From::File
        },
    };
    let mut sl = row(
        "Status line",
        &shown,
        "The command whose output Claude Code shows under its prompt (statusLine.command). #statusline builds one from rush's segments. Empty takes it out.",
        &[],
        "none",
        true,
        |v: &str| {
            Ok(if v.is_empty() || v == "none" {
                vec![Edit::delete("statusLine")]
            } else {
                vec![
                    Edit::set("statusLine.type", "command"),
                    Edit::set("statusLine.command", v),
                ]
            })
        },
    );
    sl.choices.clear();
    rows.push(sl);
    // The rest Claude Code's page offers, as its adapter says.
    let page = rush_claude::settings::settings_page();
    let mine = [
        "model",
        "effortLevel",
        "permissions.defaultMode",
        "includeCoAuthoredBy",
    ];
    for r in page.rows.iter().filter(|r| !mine.contains(&r.key.as_str())) {
        let key = r.key.as_str();
        use rush_core::adapter::SettingType as T;
        let st = match r.kind {
            T::Flag | T::Off => {
                let invert = r.kind == T::Off;
                let mut shown = s.flag(key, false);
                if invert && !shown.value.is_empty() {
                    shown.value = if shown.value == "on" { "off" } else { "on" }.into();
                }
                // A key that turns a thing off leaves it on unset.
                let unset = if invert { "on" } else { "not set" };
                flag(
                    &r.label,
                    key,
                    shown,
                    &format!("{} ({key}).", r.what),
                    on_off("", ""),
                    invert,
                    unset,
                )
            }
            T::Int => {
                let mut pairs: Vec<(&str, &str)> = vec![("", "Claude Code's own default.")];
                pairs.extend(r.choices.iter().map(|c| (c.as_str(), "")));
                let k = key.to_string();
                let unset = if key == "cleanupPeriodDays" {
                    "30"
                } else {
                    "not set"
                };
                row(
                    &r.label,
                    &s.int(key),
                    &format!("{} ({key}).", r.what),
                    &pairs,
                    unset,
                    true,
                    move |v: &str| {
                        if v.is_empty() {
                            return Ok(vec![Edit::delete(&k)]);
                        }
                        v.trim()
                            .parse::<i64>()
                            .map(|n| vec![Edit::set(&k, n)])
                            .map_err(|_| format!("{v:?} isn't a whole number"))
                    },
                )
            }
            T::String => {
                let mut pairs: Vec<(&str, &str)> = vec![("", "Claude Code's own default.")];
                pairs.extend(r.choices.iter().map(|c| (c.as_str(), "")));
                row(
                    &r.label,
                    &s.string(key, None),
                    &format!("{} ({key}).", r.what),
                    &pairs,
                    "not set",
                    true,
                    string_edits(key),
                )
            }
        };
        rows.push(st);
    }
    rows
}

/// Sets variable v in settings.json's env block; "" removes it.
fn set_var(m: &mut Model, v: &str, value: &str) -> Out {
    let e = if value.is_empty() {
        Edit::delete_in("env", v)
    } else {
        Edit::set_in("env", v, Value::String(value.into()))
    };
    let said = if value.is_empty() {
        format!("{v} taken out of settings.json's env")
    } else {
        format!("{v} saved to settings.json's env")
    };
    save(m, vec![e], said)
}

/// The variables every session starts with: the ones Claude Code's page
/// offers, then the rest of the env block, then a row to add one.
fn env_rows(s: &Snapshot) -> Vec<Setting> {
    let name = agent_name(LOGINS_KIND);
    let page = rush_claude::settings::settings_page();
    let env = s.env_block();
    let mut rows = Vec::new();
    for e in &page.env {
        let mut pairs: Vec<(String, String)> =
            vec![(String::new(), "not set: Claude Code's own default.".into())];
        for c in &e.choices {
            pairs.push((c.clone(), e.means.get(c).cloned().unwrap_or_default()));
        }
        let pairs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let var = e.name.clone();
        let shown = Shown {
            value: env.get(&e.name).cloned().unwrap_or_default(),
            from: if env.contains_key(&e.name) {
                From::EnvBlock(e.name.clone())
            } else {
                From::Default
            },
        };
        let mut st = row(
            &e.label,
            &shown,
            &format!("{} (env {}).", e.what, e.name),
            &pairs,
            "not set",
            true,
            |_| Ok(Vec::new()),
        );
        st.source.clear();
        st.run = Some(Rc::new(move |m: &mut Model, v: &str| set_var(m, &var, v)));
        rows.push(st);
    }
    let known: Vec<&String> = page.env.iter().map(|e| &e.name).collect();
    for (v, value) in &env {
        if known.contains(&v) {
            continue;
        }
        let (v1, v2) = (v.clone(), v.clone());
        rows.push(Setting {
            label: v.clone(),
            value: value.clone(),
            typed: true,
            what: format!(
                "A variable in settings.json's env block: every {name} session starts with it, subagents included. Empty takes it out."
            ),
            run: Some(Rc::new(move |m: &mut Model, x: &str| set_var(m, &v1, x))),
            key: Some(Rc::new(move |m, s| match s {
                "x" | "delete" | "backspace" => {
                    let v = v2.clone();
                    m.ask(
                        Confirm::new(format!("Remove {v}?"), move |m| set_var(m, &v, ""))
                            .detail("from settings.json's env block, for every account"),
                    );
                    Out::Draw
                }
                _ => open_key(m, s),
            })),
            keys: ["x", "remove it", "e", "open settings.json"]
                .map(String::from)
                .to_vec(),
            ..Setting::default()
        });
    }
    rows.push(Setting {
        label: "+ add a variable".into(),
        line: Some(Rc::new(|_| line([sp("+ add a variable", FAINT)]))),
        what: format!(
            "Adds a variable to settings.json's env block: every {name} session starts with it, subagents included."
        ),
        key: Some(Rc::new(|m, s| {
            if matches!(s, "enter" | "right" | "a") {
                ask(
                    m,
                    "NAME=value",
                    "",
                    Rc::new(|m: &mut Model, input: &str| {
                        let (v, value) = input.split_once('=').unwrap_or(("", ""));
                        let (v, value) = (v.trim(), value.trim());
                        if v.is_empty() || v.contains([' ', '\t', '.']) {
                            m.flash_err("type it as NAME=value");
                            return Out::Draw;
                        }
                        set_var(m, v, value)
                    }),
                );
                return Out::Draw;
            }
            open_key(m, s)
        })),
        keys: vec!["enter".into(), "add NAME=value".into()],
        ..Setting::default()
    });
    rows
}

/// The plugins settings.json names, each on or off.
fn plugin_rows(s: &Snapshot) -> Vec<Setting> {
    let mut rows = Vec::new();
    for (id, on) in s.plugins() {
        let id2 = id.clone();
        let mut st = super::form::choice(
            &id,
            if on { "on" } else { "off" },
            &format!(
                "The plugin {id}, as settings.json's enabledPlugins has it. Off keeps it installed but out of every session."
            ),
            &[
                ("on", "every session has it."),
                ("off", "installed, but no session loads it."),
            ],
            None,
        );
        st.run = Some(Rc::new(move |m: &mut Model, v: &str| {
            save(
                m,
                vec![Edit::set_in("enabledPlugins", &id2, v == "on")],
                String::new(),
            )
        }));
        st.key = Some(Rc::new(open_key));
        st.keys = vec!["e".into(), "open settings.json".into()];
        rows.push(st);
    }
    if rows.is_empty() {
        rows.push(Setting {
            label: "no plugins".into(),
            line: Some(Rc::new(|_| {
                line([sp(
                    "settings.json names no plugins: /plugin in Claude Code installs them.",
                    FAINT,
                )])
            })),
            ..Setting::default()
        });
    }
    rows
}

/// Claude Code's settings, its env block and its plugins, as sections of
/// its page; one line while they're read.
pub fn sections(m: &Model) -> Vec<Section> {
    let Some(s) = dialog(m).claude.as_ref() else {
        return vec![Section {
            title: "Claude Code's settings".into(),
            rows: vec![Setting {
                label: "reading".into(),
                line: Some(Rc::new(|_| line([sp("reading settings.json…", FAINT)]))),
                ..Setting::default()
            }],
            ..Section::default()
        }];
    };
    let path = tildify(&s.file.path.to_string_lossy());
    let accounts = match s.paths.len() {
        0 | 1 => "every account reads it".to_string(),
        n => format!("written to {n} files: an account has its own copy"),
    };
    vec![
        Section {
            title: "Claude Code's settings".into(),
            note: format!("{path} · {accounts}"),
            rows: settings_rows(s),
            ..Section::default()
        },
        Section {
            title: "Environment".into(),
            note: "settings.json's env: every session starts with these".into(),
            rows: env_rows(s),
            ..Section::default()
        },
        Section {
            title: "Plugins".into(),
            note: "settings.json's enabledPlugins".into(),
            rows: plugin_rows(s),
            ..Section::default()
        },
    ]
}
