//! Settings › Plugins: rush's plugins, the bundled ones and those
//! installed in the plugins folder: which run, which don't and why, and
//! what each needs to run here. enter opens one: on or off, its settings,
//! and its commands' keys. After Go's internal/ui/settings_plugins.go
//! (pluginsPage :26, pluginRow :48, readPlugins :61, notOnPath :113,
//! needCells :126, askCells :152, pluginNeeds :172, pluginSections :208,
//! pluginListRow :271, pluginPage :295, capWords :424, joinAnd :443,
//! pluginSetting :455) and pluginui.go (setBundled :54, setPluginDev :75).
//!
//! The plugins running now, and changing one's setting, are the broker's,
//! which the plugin module (screens/plugin_ui) holds: it hands them over
//! through [`provide`]. Until then nothing reads as running.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::OnceLock;

use rush_core::text::Line;
use rush_plugin::{CATEGORIES, Manifest, Requires, SettingSpec, UiPlugin};

use super::form::{Section, Setting};
use super::paint::{first_non_empty, trunc};
use super::{PAGE_KEYS, PAGE_PLUGINS, Page, State, dialog, dialog_mut, set_settings_page};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// What the plugin module hands Settings.
#[derive(Clone, Copy)]
pub struct Broker {
    /// The plugins running now, as the broker last said.
    pub running: fn(&Model) -> Vec<UiPlugin>,
    /// Changes one of a running plugin's settings.
    pub set_setting: fn(&mut Model, &str, &str, &str) -> Out,
}

static PROVIDED: OnceLock<Broker> = OnceLock::new();

/// The plugin module's part, once.
pub fn provide(b: Broker) {
    let _ = PROVIDED.set(b);
}

fn broker() -> Broker {
    PROVIDED.get().copied().unwrap_or(Broker {
        running: |_| Vec::new(),
        set_setting: |m, name, _, _| {
            m.flash_err(&format!("{name}: rush isn't talking to its plugins yet"));
            Out::Draw
        },
    })
}

/// One plugin as Plugins lists it, read off the UI (pluginRow).
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub m: Manifest,
    pub bundled: bool,
    /// On, and able to run here.
    pub runs: bool,
    /// How it stands: on, off, not approved, why it can't run.
    pub status: String,
    /// The programs it requires that aren't on PATH.
    pub missing: Vec<String>,
    /// Where it came from.
    pub from: String,
    /// Installed from a repository, so it can be in development mode.
    pub repo: bool,
    pub dev: bool,
    /// A bundled one's: off until you turn it on.
    pub optional: bool,
}

/// What Plugins read: its rows, and which bundled plugins are on.
#[derive(Clone, Debug, Default)]
pub struct Read {
    pub rows: Vec<Row>,
    pub bundled_on: BTreeMap<String, bool>,
}

/// The page.
pub fn page() -> Page {
    Page {
        name: "Plugins",
        head: Some(|m, w| {
            if !dialog(m).plugin.is_empty() {
                return Vec::new();
            }
            vec![trunc(
                line([sp(
                    format!(
                        "Installed in {} · rush plugin approve <name> runs a new one",
                        tildify(&rush_plugin::root().to_string_lossy())
                    ),
                    FAINT,
                )]),
                w,
            )]
        }),
        form: Some(sections),
        pre: Some(|m, s| {
            let d = dialog_mut(m);
            if !d.plugin.is_empty() && (s == "esc" || s == "q") {
                d.plugin.clear();
                d.cursor = 0;
                return Out::Draw;
            }
            Out::Pass
        }),
        ..Page::default()
    }
}

/// Reads every plugin and how it stands, off the UI (readPlugins).
pub fn read(m: &mut Model) {
    m.kept::<State>().plugins_reading = true;
    m.later(read_plugins, |m, r| {
        let st = m.kept::<State>();
        st.plugins_reading = false;
        st.plugins = Some(r);
        Out::Draw
    });
}

/// Every plugin and how it stands. It reads their folders, so never on
/// the UI (readPlugins).
pub fn read_plugins() -> Read {
    let mut out = Vec::new();
    let on = rush_plugin::bundles_on();
    for b in rush_plugin::bundles() {
        let mut r = Row {
            missing: not_on_path(&b.manifest.requires.bin),
            m: b.manifest.clone(),
            bundled: true,
            runs: true,
            status: "bundled, on".into(),
            optional: b.optional,
            ..Row::default()
        };
        let why = b.manifest.unmet();
        if !why.is_empty() {
            r.runs = false;
            r.status = format!("won't run: {why}");
        } else if !on.get(&b.manifest.name).copied().unwrap_or(false) {
            r.runs = false;
            r.status = "bundled, off".into();
        }
        out.push(r);
    }
    let (installed, bad) = rush_plugin::installed();
    let approved = rush_plugin::approvals();
    for p in installed {
        if rush_plugin::bundle_named(&p.manifest.name).is_some() {
            continue; // rush's own of that name runs instead
        }
        let mut r = Row {
            m: p.manifest.clone(),
            runs: true,
            status: "on".into(),
            missing: not_on_path(&p.manifest.requires.bin),
            from: p.from(),
            repo: !p.origin.project.is_empty(),
            dev: p.origin.dev,
            ..Row::default()
        };
        let d = rush_plugin::digest(std::path::Path::new(&p.dir)).unwrap_or_default();
        if p.origin.changed_since_install(&d) {
            r.from.push_str(", and changed since");
        }
        let why = p.manifest.unmet();
        match approved.get(&p.manifest.name) {
            _ if !why.is_empty() => {
                r.runs = false;
                r.status = format!("won't run: {why}");
            }
            None => {
                r.runs = false;
                r.status = "not approved".into();
            }
            Some(a) if a.digest != d => {
                r.runs = false;
                r.status = "changed since approved".into();
            }
            Some(_) => {}
        }
        out.push(r);
    }
    for (n, err) in bad {
        out.push(Row {
            m: Manifest {
                name: n,
                description: err,
                ..Manifest::default()
            },
            status: "not a valid plugin".into(),
            ..Row::default()
        });
    }
    Read {
        rows: out,
        bundled_on: on,
    }
}

/// Those of bins not found on PATH (notOnPath).
fn not_on_path(bins: &[String]) -> Vec<String> {
    bins.iter()
        .filter(|b| rush_plugin::look_path(b).is_none())
        .cloned()
        .collect()
}

/// What a plugin requires, a cell each: ✓ met here, – not (needCells).
pub fn need_cells(r: &Requires, missing: &[String]) -> Line {
    let mut cells: Vec<Line> = Vec::new();
    let mut cell = |words: &str, met: bool| {
        cells.push(if met {
            line([sp("✓ ", GREEN), sp(words.to_string(), TEXT)])
        } else {
            line([sp(format!("– {words}"), FAINT)])
        });
    };
    for o in &r.os {
        cell(
            rush_plugin::os_name(o).unwrap_or(o.as_str()),
            o == rush_plugin::GOOS,
        );
    }
    for a in &r.arch {
        cell(a, a == rush_plugin::GOARCH);
    }
    for b in &r.bin {
        cell(&format!("{b} on PATH"), !missing.contains(b));
    }
    if cells.is_empty() {
        return line([sp("nothing; it runs anywhere", FAINT)]);
    }
    let mut out = Line::new();
    for (i, c) in cells.into_iter().enumerate() {
        if i > 0 {
            out.spans.push(plain("  "));
        }
        join(&mut out, c);
    }
    out
}

/// What a plugin's UI capability does, in words (uiCapWords).
fn cap_word(c: &str) -> &'static str {
    match c {
        rush_plugin::UI_EVENTS => "hears what happens",
        rush_plugin::UI_INPUT => "sees what you type",
        rush_plugin::UI_INTERCEPT => "is asked before a message goes",
        rush_plugin::UI_OVERVIEW => "adds to overviews",
        rush_plugin::UI_NOTIFY => "shows notices",
        rush_plugin::UI_SEND => "sends to sessions",
        rush_plugin::UI_VIEWS => "draws views",
        _ => "",
    }
}

/// What a plugin asks rush for (askCells).
pub fn ask_cells(p: &UiPlugin, mf: &Manifest) -> Line {
    let mut cells: Vec<String> = p.ui.iter().map(|c| cap_word(c).to_string()).collect();
    if mf.has_tools() {
        cells.push("offers tools".into());
    }
    for c in &mf.sessions {
        cells.push(format!("sessions: {c}"));
    }
    if cells.is_empty() {
        return line([sp("commands and settings only", FAINT)]);
    }
    line([sp(cells.join("   "), DIM)])
}

/// What a plugin needs to run, and where it starts sessions
/// (pluginNeeds).
pub fn plugin_needs(mf: &Manifest) -> String {
    let mut parts = Vec::new();
    let n = mf.requires.needs();
    if !n.is_empty() {
        parts.push(format!("needs {n}"));
    }
    if !mf.workspaces.is_empty() {
        let ws: Vec<String> = mf.workspaces.iter().map(|w| tildify(w)).collect();
        parts.push(format!("sessions in {}", ws.join(", ")));
    }
    if parts.is_empty() {
        return "runs anywhere".into();
    }
    parts.join(" · ")
}

/// The running plugin called name, as the broker last said (uiPlugin).
fn ui_plugin(m: &Model, name: &str) -> Option<UiPlugin> {
    (broker().running)(m).into_iter().find(|p| p.name == name)
}

/// The rows as read; None while they're being read.
fn rows(m: &Model) -> Option<Vec<Row>> {
    let st = m.kept.peek::<State>()?;
    match &st.plugins {
        Some(r) => Some(r.rows.clone()),
        None if st.plugins_reading => None,
        None => Some(Vec::new()),
    }
}

/// The page's sections: a plugin open, else each kind of plugin, those
/// running first (pluginSections).
fn sections(m: &Model) -> Vec<Section> {
    let d = dialog(m);
    if !d.plugin.is_empty() {
        return vec![plugin_page(m, &d.plugin)];
    }
    let Some(mut rows) = rows(m) else {
        return vec![Section {
            rows: vec![Setting {
                label: "reading".into(),
                line: Some(Rc::new(|_| line([sp("reading plugins…", DIM)]))),
                ..Setting::default()
            }],
            ..Section::default()
        }];
    };
    // One the broker runs that wasn't read from disk still shows.
    for p in (broker().running)(m) {
        if !rows.iter().any(|r| r.m.name == p.name) {
            rows.push(Row {
                m: Manifest {
                    name: p.name.clone(),
                    ui: p.ui.clone(),
                    ..Manifest::default()
                },
                runs: true,
                status: "on".into(),
                ..Row::default()
            });
        }
    }
    // A section for each kind of plugin, by what they do; in each, the
    // ones running first.
    rows.sort_by(|a, b| b.runs.cmp(&a.runs).then_with(|| a.m.name.cmp(&b.m.name)));
    let mut out = Vec::new();
    for c in &CATEGORIES {
        let mut s = Section {
            title: c.title.into(),
            note: c.about.into(),
            ..Section::default()
        };
        for r in &rows {
            if r.m.category_of().id == c.id {
                s.rows.push(list_row(r.clone()));
            }
        }
        if !s.rows.is_empty() {
            out.push(s);
        }
    }
    if out.is_empty() {
        let root = tildify(&rush_plugin::root().to_string_lossy());
        return vec![Section {
            title: "Plugins".into(),
            rows: vec![Setting {
                label: "none".into(),
                line: Some(Rc::new(move |_| {
                    line([
                        sp("No plugins. ", DIM),
                        sp(
                            format!("Put one in {root}/<name>, then rush plugin approve <name>."),
                            FAINT,
                        ),
                    ])
                })),
                about: Some(Rc::new(|| {
                    (
                        "Plugins".into(),
                        "rush's own plugins, not your agent's: those are under a Session's Settings tab.".into(),
                        "None installed.".into(),
                    )
                })),
                ..Setting::default()
            }],
            ..Section::default()
        }];
    }
    out
}

/// A plugin on the list; enter opens it (pluginListRow).
fn list_row(r: Row) -> Setting {
    let name = r.m.name.clone();
    let needs = plugin_needs(&r.m);
    let mark = if r.runs {
        sp("●", GREEN)
    } else {
        sp("○", DIM)
    };
    let (n1, n2, s1, nd) = (name.clone(), name.clone(), r.status.clone(), needs.clone());
    let desc = r.m.description.clone();
    Setting {
        label: name.clone(),
        line: Some(Rc::new(move |w| {
            let mut l = line([
                mark.clone(),
                plain(" "),
                spb(fit_str(&n1, 22), TEXT),
                plain(" "),
                sp(fit_str(&s1, 30), DIM),
                plain(" "),
            ]);
            join(
                &mut l,
                trunc(line([sp(nd.clone(), FAINT)]), w.saturating_sub(56)),
            );
            l
        })),
        about: Some(Rc::new(move || {
            (
                name.clone(),
                first_non_empty(&[&desc, "no description"]).to_string(),
                format!("{} · {needs}", r.status),
            )
        })),
        key: Some(Rc::new(move |m, s| {
            if matches!(s, "enter" | "right" | "l") {
                let d = dialog_mut(m);
                d.plugin = n2.clone();
                d.cursor = 0;
                return Out::Draw;
            }
            Out::Pass
        })),
        keys: vec!["enter".into(), "open".into()],
        ..Setting::default()
    }
}

/// What a running plugin may do, in words (capWords).
fn cap_words(p: &UiPlugin) -> String {
    let w: Vec<String> = p.ui.iter().map(|c| cap_word(c).to_string()).collect();
    if w.is_empty() {
        return "commands and settings only".into();
    }
    join_and(&w)
}

/// "a, b and c" (joinAnd).
pub fn join_and(xs: &[String]) -> String {
    match xs.len() {
        0 => String::new(),
        1 => xs[0].clone(),
        n => format!("{} and {}", xs[..n - 1].join(", "), xs[n - 1]),
    }
}

/// "label" fitted to 16, then the rest of a row truncated to w.
fn labelled(label: &str, rest: Line, w: usize) -> Line {
    let mut l = line([plain(fit_str(label, 16))]);
    join(&mut l, trunc(rest, w.saturating_sub(16)));
    l
}

/// One plugin open: on or off, what it needs, then its settings and
/// commands while it runs (pluginPage).
fn plugin_page(m: &Model, name: &str) -> Section {
    let st = m.kept.peek::<State>();
    let r = st
        .and_then(|s| s.plugins.as_ref())
        .and_then(|p| p.rows.iter().find(|r| r.m.name == name).cloned())
        .unwrap_or_default();
    let running = ui_plugin(m, name);
    let up = running.clone().unwrap_or_else(|| UiPlugin {
        ui: r.m.ui.clone(),
        ..UiPlugin::default()
    });
    let mut sec = Section {
        title: name.into(),
        note: first_non_empty(&[&r.status, "on"]).into(),
        ..Section::default()
    };
    if r.bundled {
        let on = st
            .and_then(|s| s.plugins.as_ref())
            .and_then(|p| p.bundled_on.get(name).copied())
            .unwrap_or(!r.optional);
        let what = if r.optional {
            "Comes with rush, and is off until you turn it on."
        } else {
            "Comes with rush, and is on until you turn it off."
        };
        let mut means = HashMap::new();
        means.insert("on".into(), format!("it runs, and {}", cap_words(&up)));
        means.insert("off".into(), "it doesn't run".into());
        let n = name.to_string();
        sec.rows.push(Setting {
            label: "Runs".into(),
            value: if on { "on" } else { "off" }.into(),
            choices: vec!["on".into(), "off".into()],
            what: first_non_empty(&[&r.m.description, what]).into(),
            means,
            run: Some(Rc::new(move |m, v| set_bundled(m, &n, v == "on"))),
            ..Setting::default()
        });
    }
    if !r.from.is_empty() {
        let from = r.from.clone();
        sec.rows.push(Setting {
            label: "from".into(),
            line: Some(Rc::new(move |w| {
                labelled(
                    "From",
                    line([plain(from.trim_start_matches("From ").to_string())]),
                    w,
                )
            })),
            what: format!(
                "{}. A plugin a project ships keeps the repository, its origin and the commit it was installed at.",
                r.from
            ),
            ..Setting::default()
        });
    }
    if r.repo {
        let n = name.to_string();
        let mut means = HashMap::new();
        means.insert("off".into(), "kept at the commit installed".into());
        means.insert("on".into(), "rebuilt as its files change".into());
        sec.rows.push(Setting {
            label: "Development".into(),
            value: if r.dev { "on" } else { "off" }.into(),
            choices: vec!["off".into(), "on".into()],
            what: "Off, it stays at the commit it was installed at, and rush offers an update when a newer commit changes it. On, whenever a file in its sources changes, rush runs its install.sh again, as you and outside the sandbox, and runs what it built without asking, as long as it asks for no more than you approved.".into(),
            means,
            run: Some(Rc::new(move |m, v| set_dev(m, &n, v == "on"))),
            ..Setting::default()
        });
    }
    let needs = plugin_needs(&r.m);
    let (req, missing) = (r.m.requires.clone(), r.missing.clone());
    sec.rows.push(Setting {
        label: "requires".into(),
        line: Some(Rc::new(move |w| {
            labelled("Requires", need_cells(&req, &missing), w)
        })),
        about: Some(Rc::new(move || {
            (
                "Requires".into(),
                "What the system must offer for it to run, and the folders sessions it starts may run in: ✓ met here, – not.".into(),
                needs.clone(),
            )
        })),
        ..Setting::default()
    });
    let (up2, mf, cw) = (up.clone(), r.m.clone(), cap_words(&up));
    sec.rows.push(Setting {
        label: "asks for".into(),
        line: Some(Rc::new(move |w| {
            labelled("Asks for", ask_cells(&up2, &mf), w)
        })),
        about: Some(Rc::new(move || {
            (
                "Asks for".into(),
                "What it may do in rush's screen and with sessions, as its manifest asks.".into(),
                cw.clone(),
            )
        })),
        ..Setting::default()
    });
    match r.status.as_str() {
        "not approved" | "changed since approved" => {
            let (status, n) = (r.status.clone(), name.to_string());
            sec.rows.push(Setting {
                label: "approve".into(),
                line: Some(Rc::new(move |_| {
                    line([
                        sp(format!("{status}: "), YELLOW),
                        sp(
                            format!("rush plugin approve {n} shows what it may do, and runs it"),
                            FAINT,
                        ),
                    ])
                })),
                what: "A plugin runs only once you've approved it as it is: approving again is asked for whenever its files change.".into(),
                ..Setting::default()
            });
        }
        "not a valid plugin" => {
            let desc = r.m.description.clone();
            sec.rows.push(Setting {
                label: "error".into(),
                line: Some(Rc::new(move |w| trunc(line([sp(desc.clone(), RED)]), w))),
                what: r.m.description.clone(),
                ..Setting::default()
            });
        }
        _ => {}
    }
    for spec in &up.settings {
        sec.rows.push(plugin_setting(&up, spec));
    }
    let km = super::keys::key_map(m);
    for c in &up.commands {
        let id = rush_keymap::plugin_id(name, &c.name);
        let k = km.key_text(&id);
        let k = if k.is_empty() { "no key".into() } else { k };
        let (cn, cd) = (c.name.clone(), c.description.clone());
        sec.rows.push(Setting {
            label: format!("#{}", c.name),
            line: Some(Rc::new(move |_| {
                line([
                    plain(fit_str(&format!("command {cn}"), 32)),
                    sp(k.clone(), TEXT),
                    sp(format!("  {cd}"), FAINT),
                ])
            })),
            what: format!("{} Settings, Keys changes its key.", c.description),
            key: Some(Rc::new(move |m, s| {
                if s != "enter" {
                    return Out::Pass;
                }
                set_settings_page(m, PAGE_KEYS);
                super::keys::show_key(m, &id);
                Out::Draw
            })),
            keys: vec!["enter".into(), "its key".into()],
            ..Setting::default()
        });
    }
    if !up.skipped.is_empty() {
        let s = up.skipped.clone();
        sec.rows.push(Setting {
            label: "skipped".into(),
            line: Some(Rc::new(move |_| {
                line([sp(format!("intercepts skipped: {s}"), YELLOW)])
            })),
            what:
                "It stopped answering in time, so messages go without asking it until it restarts."
                    .into(),
            ..Setting::default()
        });
    }
    if running.is_none() && r.runs && (!r.m.settings.is_empty() || !r.m.commands.is_empty()) {
        sec.rows.push(Setting {
            label: "starting".into(),
            line: Some(Rc::new(|_| {
                line([sp(
                    "its settings and commands show once it's running",
                    FAINT,
                )])
            })),
            what: "The broker starts it in the background.".into(),
            ..Setting::default()
        });
    }
    sec
}

/// One of a plugin's settings as a row; changing it asks the broker, off
/// the UI (pluginSetting).
fn plugin_setting(p: &UiPlugin, spec: &SettingSpec) -> Setting {
    let mut st = Setting {
        label: spec.title.clone(),
        value: p.values.get(&spec.key).cloned().unwrap_or_default(),
        what: spec.description.clone(),
        unset: spec.default.clone(),
        ..Setting::default()
    };
    match spec.kind.as_str() {
        "bool" => {
            st.choices = vec!["true".into(), "false".into()];
            st.means.insert("true".into(), "on".into());
            st.means.insert("false".into(), "off".into());
        }
        "choice" => st.choices = spec.choices.clone(),
        "text" => st.typed = true,
        _ => {}
    }
    let (name, key) = (p.name.clone(), spec.key.clone());
    st.run = Some(Rc::new(move |m, v| {
        (broker().set_setting)(m, &name, &key, v)
    }));
    st
}

/// Turns a bundled plugin on or off, off the UI, and reads the page again
/// (setBundled).
fn set_bundled(m: &mut Model, name: &str, on: bool) -> Out {
    let n = name.to_string();
    m.later(
        move || {
            rush_plugin::set_bundled(&n, on)?;
            rush_plugin::client::reload()
        },
        {
            let n = name.to_string();
            move |m, r| {
                if let Err(e) = r {
                    m.flash_err(&format!("{n}: {e}"));
                }
                if dialog(m).page == PAGE_PLUGINS {
                    read(m);
                }
                Out::Draw
            }
        },
    );
    Out::Draw
}

/// Turns development mode on or off for an installed plugin
/// (setPluginDev).
fn set_dev(m: &mut Model, name: &str, on: bool) -> Out {
    let n = name.to_string();
    m.later(
        move || {
            rush_plugin::set_dev(&n, on)?;
            rush_plugin::client::reload()
        },
        {
            let n = name.to_string();
            move |m, r| {
                if dialog(m).page == PAGE_PLUGINS {
                    read(m);
                }
                match r {
                    Err(e) => m.flash_err(&format!("{n}: {e}")),
                    Ok(()) if on => m.flash(&format!(
                        "{n} is in development mode · rebuilt as its files change"
                    )),
                    Ok(()) => {}
                }
                Out::Draw
            }
        },
    );
    Out::Draw
}
