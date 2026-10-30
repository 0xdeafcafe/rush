//! What an agent itself reads, on its page: the keys of its settings file
//! its adapter offers (defaults, permissions, hooks), its env block, and
//! the definitions its sessions can start as. All of it is read off the
//! UI: none until it's in, read again (at most every few seconds) as keys
//! come while the page is open. After Go's internal/ui/settings_files.go
//! (agentProfile :25, agentDefs :38, defsKey :55, definitionsSection :82,
//! sameDefsOnce :117, definitionRow :140, fileSections :218,
//! agentSettings :238, saveSettings :308, openSettingsKey :331,
//! settingsFileSection :338, envSection :406) and rulesheet.go:30
//! (settingsFiles).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use rush_core::adapter::{AgentDef, Profile, SettingType, SettingsPage};
use rush_state::settingsfile::File;

use super::common::{LOGINS_KIND, agent_name, model_word, profile_of, reg};
use super::form::{Section, Setting};
use super::paint::on_off;
use super::{State, ask, config, config_mut, dialog, dialog_mut};
use crate::frame::Out;
use crate::frame::confirm::Confirm;
use crate::model::Model;
use crate::style::*;

/// How long a settings file read stays fresh (settingsRead's memo).
const FRESH: Duration = Duration::from_secs(3);

/// A settings file as last read: the file, and when it was last written.
pub struct Loaded {
    pub file: File,
    pub kind: String,
    pub modified: Option<SystemTime>,
}

/// What the agents' pages have read off the UI, by agent.
#[derive(Default)]
pub struct Reads {
    pub defs: HashMap<String, Vec<AgentDef>>,
    /// When each agent's things were last asked for.
    pub(crate) asked: HashMap<String, Instant>,
}

/// The profile k's page is about: the one its sessions run in
/// (agentProfile).
pub fn agent_profile(m: &Model, k: &str) -> Option<Profile> {
    let a = reg().get(k)?;
    profile_of(m, &a)
}

/// Opens path in $VISUAL or $EDITOR, vi without either, and reads what
/// the page shows again once it's closed (dialog.go:64 editFile).
pub fn edit_file(m: &mut Model, path: &str) -> Out {
    crate::screens::session_core::memedit::edit_then(m, path, Box::new(dialog_reload))
}

/// Back from the editor: the page open reads its files again, the open
/// Session's memory view too, and a failure is said (model.go:1153
/// dialogReload, settings.go:133 loadDialog).
pub fn dialog_reload(m: &mut Model, r: Result<(), String>) -> Out {
    if m.kept.peek::<State>().is_some_and(|s| s.dialog.is_some()) {
        m.kept::<State>().reads.asked.clear();
        let page = dialog(m).page;
        if page <= super::PAGE_CONFIG {
            reg().recheck();
            super::common::read_settings_models(m);
        }
        if page == super::PAGE_PLUGINS {
            super::plugins::read(m);
        }
        super::prep(m);
    }
    if let Some(key) = crate::screens::session_core::open_key(m) {
        crate::screens::session_core::memory::forget(m, &key);
    }
    if let Err(e) = r {
        m.fail(&e);
    }
    Out::Draw
}

/// Reads agent k's definitions and settings file again, off the UI,
/// unless they were read a moment ago.
pub fn want(m: &mut Model, k: &str) {
    if k.is_empty() {
        return;
    }
    let st = m.kept::<State>();
    if st.reads.asked.get(k).is_some_and(|at| at.elapsed() < FRESH) {
        return;
    }
    st.reads.asked.insert(k.to_string(), Instant::now());
    read(m, k);
}

/// Reads k's definitions and settings file now, off the UI.
fn read(m: &mut Model, k: &str) {
    let Some(a) = reg().get(k) else {
        return;
    };
    let p = profile_of(m, &a).unwrap_or_default();
    let mut roots = vec![m.launch_dir.clone()];
    roots.extend(m.agents.iter().map(|a| a.repo.clone()));
    let k1 = k.to_string();
    let (a1, p1) = (a.clone(), p.clone());
    if a.definer().is_some() {
        m.later(
            move || {
                a1.definer()
                    .map(|d| d.agent_defs(&p1, &roots))
                    .unwrap_or_default()
            },
            move |m, defs| {
                m.kept::<State>().reads.defs.insert(k1, defs);
                Out::Draw
            },
        );
    }
    if a.settings_pager().is_none() || a.settings_filer().is_none() {
        return;
    }
    let k2 = k.to_string();
    m.later(
        move || {
            let files = a.settings_filer()?.settings_files(&p, "");
            let path = files.first()?.path.clone();
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            let r = File::load(&path).map_err(|e| e.to_string());
            Some((path, modified, r))
        },
        move |m, got| {
            let Some((path, modified, r)) = got else {
                return Out::Idle;
            };
            let d = dialog(m);
            if let Some(l) = &d.settings
                && l.kind == k2
                && l.file.path == Path::new(&path)
                && l.modified == modified
            {
                return Out::Idle;
            }
            let file = match r {
                Ok(f) => f,
                Err(e) => {
                    m.fail(&format!("couldn't read {}: {e}", tildify(&path)));
                    let dir = Path::new(&path).parent().unwrap_or(Path::new("."));
                    let name = Path::new(&path).file_name().unwrap_or_default();
                    match File::load(dir.join(".rush-unreadable").join(name)) {
                        Ok(f) => f,
                        Err(_) => return Out::Draw,
                    }
                }
            };
            if m.kept.peek::<State>().is_some_and(|s| s.dialog.is_some()) {
                dialog_mut(m).settings = Some(Loaded {
                    file,
                    kind: k2,
                    modified,
                });
            }
            Out::Draw
        },
    );
}

/// The definitions k's sessions can start as, as read (agentDefs).
pub fn agent_defs(m: &Model, k: &str) -> Vec<AgentDef> {
    m.kept
        .peek::<State>()
        .and_then(|s| s.reads.defs.get(k).cloned())
        .unwrap_or_default()
}

/// Has k's definitions read again (defsChanged).
fn defs_changed(m: &mut Model, k: &str) {
    m.kept::<State>().reads.asked.remove(k);
    read(m, k);
}

/// A definition listed once however many places have it: each worktree
/// of a repository has the same files. The first place stays its own,
/// the rest are counted (sameDefsOnce).
pub fn same_defs_once(defs: Vec<AgentDef>) -> Vec<AgentDef> {
    let mut at: HashMap<(String, String, String), usize> = HashMap::new();
    let mut more: BTreeMap<usize, usize> = BTreeMap::new();
    let mut out: Vec<AgentDef> = Vec::new();
    for a in defs {
        let key = (a.name.clone(), a.model.clone(), a.desc.clone());
        if let Some(&i) = at.get(&key) {
            *more.entry(i).or_default() += 1;
            continue;
        }
        at.insert(key, out.len());
        out.push(a);
    }
    for (i, n) in more {
        out[i].scope = format!("{} +{n}", out[i].scope);
    }
    out
}

/// The definitions k's new sessions can start as, the one they do
/// marked; none if k has none (definitionsSection).
pub fn definitions_section(m: &Model, k: &str) -> Option<Section> {
    reg().get(k)?.definer()?;
    let mut sec = Section {
        advanced: true,
        note: "what new sessions start as (--agent)".into(),
        ..Section::default()
    };
    for a in same_defs_once(agent_defs(m, k)) {
        sec.rows.push(definition_row(m, k, a));
    }
    sec.title = format!("Agent definitions ({})", sec.rows.len());
    Some(sec)
}

/// Asks for a new definition's name, makes it, and opens it.
fn new_agent(m: &mut Model, k: &str) {
    let k = k.to_string();
    ask(
        m,
        "new agent name",
        "",
        Rc::new(move |m: &mut Model, v: &str| {
            let name = v
                .split_whitespace()
                .collect::<Vec<_>>()
                .join("-")
                .to_lowercase();
            let Some(a) = reg().get(&k) else {
                return Out::Idle;
            };
            let p = agent_profile(m, &k).unwrap_or_default();
            let (k2, n2) = (k.clone(), name.clone());
            m.later(
                move || {
                    a.definer()
                        .map(|d| d.new_agent_def(&p, &name).map_err(|e| e.to_string()))
                        .unwrap_or_else(|| Err("it has no definitions".into()))
                },
                move |m, r| match r {
                    Err(e) => {
                        m.fail(&format!("couldn't make {n2}: {e}"));
                        Out::Draw
                    }
                    Ok(path) => {
                        defs_changed(m, &k2);
                        edit_file(m, &path)
                    }
                },
            );
            Out::Draw
        }),
    );
}

/// One definition: enter starts new sessions as it (definitionRow).
fn definition_row(m: &Model, k: &str, a: AgentDef) -> Setting {
    let cfg = config(m);
    let used = a.name == cfg.dispatch.agent || cfg.dispatch.agent.is_empty() && a.path.is_empty();
    let (al, ak, aa) = (a.clone(), a.clone(), a.clone());
    let k = k.to_string();
    Setting {
        label: a.name.clone(),
        line: Some(Rc::new(move |w| {
            let a = &al;
            let mark = if used {
                sp("●", ORANGE)
            } else {
                sp("○", FAINT)
            };
            let model = if !a.model.is_empty() && a.model != "inherit" {
                format!(" · {}", model_word(LOGINS_KIND, &a.model))
            } else {
                String::new()
            };
            line([
                mark,
                plain(" "),
                sp(fit_str(&a.name, 22), TEXT),
                sp(fit_str(&format!("{}{model}", a.scope), 24), FAINT),
                sp(fit_str(&a.desc, 10.max(w as isize - 48) as usize), DIM),
            ])
        })),
        key: Some(Rc::new(move |m, s| {
            let a = &ak;
            match s {
                "enter" => {
                    let name = if a.path.is_empty() {
                        String::new()
                    } else {
                        a.name.clone()
                    };
                    config_mut(m).dispatch.agent = name;
                    super::changed(m);
                    m.flash(&format!("new sessions start as {}", a.name));
                    Out::Draw
                }
                "e" if !a.path.is_empty() => edit_file(m, &a.path),
                "e" => Out::Draw,
                "n" => {
                    new_agent(m, &k);
                    Out::Draw
                }
                "x" | "d" => {
                    if !a.path.is_empty() {
                        let (path, name, k) = (a.path.clone(), a.name.clone(), k.clone());
                        m.ask(Confirm::new(
                            format!("Delete the agent {name} ({})?", tildify(&path)),
                            move |m| {
                                m.later(
                                    move || std::fs::remove_file(&path).map_err(|e| e.to_string()),
                                    move |m, r| {
                                        if let Err(e) = r {
                                            m.fail(&format!("couldn't delete {name}: {e}"));
                                            return Out::Draw;
                                        }
                                        defs_changed(m, &k);
                                        let d = dialog_mut(m);
                                        d.cursor = d.cursor.saturating_sub(1);
                                        Out::Draw
                                    },
                                );
                                Out::Draw
                            },
                        ));
                    }
                    Out::Draw
                }
                _ => Out::Pass,
            }
        })),
        keys: [
            "enter",
            "start as it",
            "e",
            "edit",
            "n",
            "new",
            "d",
            "delete",
        ]
        .map(String::from)
        .to_vec(),
        about: Some(Rc::new(move || {
            let a = &aa;
            let mut meta = a.scope.clone();
            if !a.model.is_empty() {
                meta.push_str(&format!(" · model {}", a.model));
            }
            if !a.path.is_empty() {
                meta.push_str(&format!(" · {}", tildify(&a.path)));
            }
            let now = if used {
                "New sessions start as it."
            } else {
                "enter makes new sessions start as it."
            };
            (a.name.clone(), format!("{meta}. {}", a.desc), now.into())
        })),
        ..Setting::default()
    }
}

/// What k itself reads, if its adapter says: its settings file, then its
/// env block (fileSections).
pub fn file_sections(m: &Model, k: &str) -> Vec<Section> {
    let Some(a) = reg().get(k) else {
        return Vec::new();
    };
    let Some(pg) = a.settings_pager() else {
        return Vec::new();
    };
    if !reg().supports(k, rush_core::adapter::Feature::Settings) {
        return Vec::new();
    }
    let Some(s) = dialog(m).settings.as_ref().filter(|l| l.kind == k) else {
        return Vec::new();
    };
    let page = pg.settings_page();
    let mut secs = vec![settings_file_section(&page, &s.file)];
    if !page.env_key.is_empty() {
        secs.push(env_section(k, &page, &s.file));
    }
    secs
}

/// Changes the settings file being shown and saves it off the UI, one
/// save after another; said when it worked (saveSettings).
fn save_settings(
    m: &mut Model,
    change: impl FnOnce(&mut File) -> std::io::Result<()>,
    said: &str,
) -> Out {
    let Some(l) = dialog_mut(m).settings.as_mut() else {
        return Out::Idle;
    };
    if let Err(e) = change(&mut l.file) {
        m.fail(&format!("couldn't save: {e}"));
        return Out::Draw;
    }
    let mut d = l.file.detach();
    let said = said.to_string();
    m.later(
        move || {
            // One save at a time, in the order they were made.
            static SAVES: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _one = SAVES.lock();
            let name = d
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            d.save().map_err(|e| format!("couldn't save {name}: {e}"))
        },
        move |m, r| {
            match r {
                Err(e) => m.fail(&e),
                Ok(()) if !said.is_empty() => m.flash(&said),
                Ok(()) => {}
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// e on any settings file or env row (openSettingsKey).
fn open_settings_key(m: &mut Model, s: &str) -> Out {
    if s == "e"
        && let Some(l) = &dialog(m).settings
    {
        let p = l.file.path.to_string_lossy().into_owned();
        return edit_file(m, &p);
    }
    Out::Pass
}

fn file_name(f: &File) -> String {
    f.path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The keys of the settings file the adapter offers (settingsFileSection).
fn settings_file_section(page: &SettingsPage, s: &File) -> Section {
    let file = file_name(s);
    let unset_note = format!("not set: {}.", page.unset);
    let mut rows = Vec::new();
    for r in &page.rows {
        let key = r.key.clone();
        let mut choices: Vec<String> = Vec::new();
        let mut st = Setting {
            label: r.label.clone(),
            what: format!("{} ({file} {})", r.what, r.key),
            unset: "not set".into(),
            key: Some(Rc::new(open_settings_key)),
            keys: vec!["e".into(), format!("open {file}")],
            ..Setting::default()
        };
        st.means.insert(String::new(), unset_note.clone());
        let run: super::form::RunFn = match r.kind {
            SettingType::Flag | SettingType::Off => {
                let invert = r.kind == SettingType::Off;
                choices = vec!["".into(), "on".into(), "off".into()];
                if let Some(b) = s.get::<bool>(&key) {
                    st.value = on_off(b != invert).into();
                }
                Rc::new(move |m, v| {
                    let v = v.to_string();
                    let key = key.clone();
                    save_settings(
                        m,
                        move |f| {
                            if v.is_empty() {
                                f.delete(&key);
                                Ok(())
                            } else {
                                f.set(&key, &((v == "on") != invert))
                            }
                        },
                        "",
                    )
                })
            }
            SettingType::Int => {
                choices.push(String::new());
                choices.extend(r.choices.iter().cloned());
                if let Some(n) = s.get::<i64>(&key) {
                    st.value = n.to_string();
                }
                Rc::new(move |m, v| {
                    let n = v.parse::<i64>().ok();
                    let key = key.clone();
                    save_settings(
                        m,
                        move |f| match n {
                            Some(n) => f.set(&key, &n),
                            None => {
                                f.delete(&key);
                                Ok(())
                            }
                        },
                        "",
                    )
                })
            }
            SettingType::String => {
                choices.push(String::new());
                choices.extend(r.choices.iter().cloned());
                st.value = s.string(&key);
                Rc::new(move |m, v| {
                    let v = v.to_string();
                    let key = key.clone();
                    save_settings(
                        m,
                        move |f| {
                            if v.is_empty() {
                                f.delete(&key);
                                Ok(())
                            } else {
                                f.set(&key, &v)
                            }
                        },
                        "",
                    )
                })
            }
        };
        st.choices = choices;
        let r2 = run.clone();
        st.run = Some(run);
        st.set = Some(Rc::new(move |m, v| {
            r2(m, v);
        }));
        rows.push(st);
    }
    Section {
        title: file,
        advanced: true,
        note: format!("{} · {}", tildify(&s.path.to_string_lossy()), page.note),
        rows,
    }
}

/// Sets env variable v in the settings file shown; "" removes it.
fn set_env(m: &mut Model, env_key: &str, v: &str, value: &str) -> Out {
    let path = dialog(m)
        .settings
        .as_ref()
        .map(|l| tildify(&l.file.path.to_string_lossy()))
        .unwrap_or_default();
    let (k, value) = (format!("{env_key}.{v}"), value.to_string());
    save_settings(
        m,
        move |f| {
            if value.is_empty() {
                f.delete(&k);
                Ok(())
            } else {
                f.set(&k, &value)
            }
        },
        &format!("saved to {path}"),
    )
}

/// The variables every session on the account starts with (envSection).
fn env_section(k: &str, page: &SettingsPage, s: &File) -> Section {
    let name = agent_name(k);
    let file = file_name(s);
    let env: BTreeMap<String, String> = s.get(&page.env_key).unwrap_or_default();
    let mut sec = Section {
        title: "Environment".into(),
        advanced: true,
        note: "every session on this account starts with these, subagents included".into(),
        ..Section::default()
    };
    let env_key = page.env_key.clone();
    for e in &page.env {
        let mut means: HashMap<String, String> = HashMap::new();
        means.insert(String::new(), format!("not set: {}.", page.unset));
        means.extend(e.means.iter().map(|(a, b)| (a.clone(), b.clone())));
        let mut choices = vec![String::new()];
        choices.extend(e.choices.iter().cloned());
        let (n1, n2, k1, k2) = (
            e.name.clone(),
            e.name.clone(),
            env_key.clone(),
            env_key.clone(),
        );
        sec.rows.push(Setting {
            label: e.label.clone(),
            value: env.get(&e.name).cloned().unwrap_or_default(),
            choices,
            unset: "not set".into(),
            what: format!("{} (env {}).", e.what, e.name),
            means,
            set: Some(Rc::new(move |m, v| {
                set_env(m, &k1, &n1, v);
            })),
            run: Some(Rc::new(move |m, v| set_env(m, &k2, &n2, v))),
            key: Some(Rc::new(open_settings_key)),
            keys: vec!["e".into(), format!("open {file}")],
            ..Setting::default()
        });
    }
    let known: Vec<&String> = page.env.iter().map(|e| &e.name).collect();
    for (v, value) in &env {
        if known.contains(&v) {
            continue;
        }
        let (v1, v2, v3) = (v.clone(), v.clone(), v.clone());
        let (k1, k2, k3) = (env_key.clone(), env_key.clone(), env_key.clone());
        let f2 = file.clone();
        sec.rows.push(Setting {
            label: v.clone(),
            value: value.clone(),
            typed: true,
            what: format!(
                "An environment variable in this account's {file} env block. Every {name} session on the account starts with it."
            ),
            set: Some(Rc::new(move |m, x| {
                set_env(m, &k1, &v1, x);
            })),
            run: Some(Rc::new(move |m, x| set_env(m, &k2, &v2, x))),
            key: Some(Rc::new(move |m, s| match s {
                "x" | "delete" | "backspace" => {
                    let (v, k) = (v3.clone(), k3.clone());
                    m.ask(
                        Confirm::new(format!("Remove {v}?"), move |m| {
                            m.flash(&format!("removed {v}"));
                            set_env(m, &k, &v, "")
                        })
                        .detail(format!("from this account's {f2}")),
                    );
                    Out::Draw
                }
                _ => open_settings_key(m, s),
            })),
            keys: vec![
                "x".into(),
                "remove it".into(),
                "e".into(),
                format!("open {file}"),
            ],
            ..Setting::default()
        });
    }
    let (f3, n3) = (file.clone(), name.clone());
    sec.rows.push(Setting {
        label: "+ add a variable".into(),
        line: Some(Rc::new(|_| line([sp("+ add a variable", FAINT)]))),
        key: Some(Rc::new(move |m, s| {
            if matches!(s, "enter" | "right" | "a") {
                let k = env_key.clone();
                ask(
                    m,
                    "NAME=value",
                    "",
                    Rc::new(move |m: &mut Model, input: &str| {
                        let (v, value) = match input.split_once('=') {
                            Some((v, value)) => (v.trim(), value.trim()),
                            None => ("", ""),
                        };
                        if v.is_empty() || !input.contains('=') || v.contains([' ', '\t']) {
                            m.flash_err("type it as NAME=value");
                            return Out::Draw;
                        }
                        set_env(m, &k, v, value)
                    }),
                );
                return Out::Draw;
            }
            open_settings_key(m, s)
        })),
        keys: vec!["enter".into(), "add NAME=value".into()],
        about: Some(Rc::new(move || {
            (
                "Add a variable".into(),
                format!(
                    "Adds an environment variable to this account's {f3} env block. Every {n3} session on the account starts with it, subagents included."
                ),
                "Press enter and type NAME=value.".into(),
            )
        })),
        ..Setting::default()
    });
    sec
}
