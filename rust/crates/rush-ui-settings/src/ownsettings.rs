//! A harness's own settings on its Harnesses page, whatever harness it is:
//! what its HarnessSettings offers, each row with its value and where it
//! comes from, changed through the harness. A harness without one shows
//! nothing here. Rush-only: Go folded a few of Claude Code's keys under
//! Advanced (settings_files.go:338 settingsFileSection, :406 envSection),
//! which files.rs still does for an agent that offers only a SettingsPage.
//!
//! Reading and writing go off the UI: the settings are read as the page
//! shows the harness (again at most every few seconds as keys come), and
//! a change is written and read back before the row shows it.

use std::rc::Rc;
use std::time::{Duration, Instant};

use rush_core::adapter::{AdapterRef, OwnEdit, OwnKind, OwnSetting, OwnSettings, Profile};

use super::common::{profile_of, reg};
use super::files::edit_file;
use super::form::{Section, Setting};
use super::{State, ask, dialog, dialog_mut};
use crate::frame::Out;
use crate::frame::confirm::Confirm;
use crate::model::Model;
use crate::style::*;

/// A harness's settings as last read.
pub struct Read {
    pub kind: String,
    pub settings: OwnSettings,
}

/// A harness with settings of its own, the profile its page is about,
/// and the environment its sessions start in.
type Source = (AdapterRef, Profile, Vec<(String, String)>);

thread_local! {
    /// What a test's page reads in place of the registry's: the harness,
    /// the profile and the environment. Only tests (the ones in rush-ui)
    /// set it.
    #[doc(hidden)]
    pub static TEST: std::cell::RefCell<Option<Source>> =
        const { std::cell::RefCell::new(None) };
}

/// Harness k, the profile its page is about, and the environment its
/// sessions start in; none when it offers no settings of its own.
fn source(m: &Model, k: &str) -> Option<Source> {
    if let Some(t) = TEST.with(|t| t.borrow().clone()) {
        return Some(t);
    }
    let a = reg().get(k)?;
    a.harness_settings()?;
    let p = profile_of(m, &a)?;
    Some((a, p, std::env::vars().collect()))
}

/// The key a read is noted under in what's been asked for.
fn asked(k: &str) -> String {
    format!("own settings/{k}")
}

/// Reads harness k's settings again, off the UI, unless they were read a
/// moment ago.
pub fn want(m: &mut Model, k: &str) {
    if k.is_empty() {
        return;
    }
    let key = asked(k);
    let st = m.kept::<State>();
    if st
        .reads
        .asked
        .get(&key)
        .is_some_and(|at| at.elapsed() < Duration::from_secs(3))
    {
        return;
    }
    st.reads.asked.insert(key, Instant::now());
    read(m, k, Vec::new(), String::new());
}

/// Writes edits (none: only reads), then reads k's settings, off the UI;
/// said once it's done.
fn read(m: &mut Model, k: &str, edits: Vec<OwnEdit>, said: String) {
    let Some((a, p, env)) = source(m, k) else {
        return;
    };
    let k = k.to_string();
    m.later(
        move || {
            let hs = a
                .harness_settings()
                .ok_or("it has no settings of its own")?;
            // One write at a time, in the order they were made.
            static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _one = WRITES.lock();
            let wrote = if edits.is_empty() {
                Ok(())
            } else {
                hs.set_own_settings(&p, &edits).map_err(|e| e.to_string())
            };
            let now = hs.own_settings(&p, &env).map_err(|e| e.to_string());
            Ok::<_, String>((wrote, now))
        },
        move |m, r| {
            let (wrote, now) = match r {
                Ok(x) => x,
                Err(e) => return fail(m, &e),
            };
            match wrote {
                Err(e) => m.fail(&format!("couldn't save: {e}")),
                Ok(()) if !said.is_empty() => m.flash(&said),
                Ok(()) => {}
            }
            match now {
                Ok(settings) => {
                    if m.kept.peek::<State>().is_some_and(|s| s.dialog.is_some()) {
                        dialog_mut(m).own = Some(Read { kind: k, settings });
                    }
                }
                Err(e) => m.fail(&format!("couldn't read its settings: {e}")),
            }
            Out::Draw
        },
    );
}

fn fail(m: &mut Model, e: &str) -> Out {
    m.fail(e);
    Out::Draw
}

/// Sets key to value in harness k: checked first, then written.
fn change(m: &mut Model, k: &str, it: &OwnSetting, file: &str, value: &str) -> Out {
    let Some((a, _, _)) = source(m, k) else {
        return Out::Idle;
    };
    if let Some(Err(why)) = a
        .harness_settings()
        .map(|h| h.check_setting(&it.key, value))
    {
        m.flash_err(&why);
        return Out::Draw;
    }
    let said = if it.from.is_env() {
        format!("saved · {} still wins over it", it.from.said(file))
    } else {
        String::new()
    };
    read(m, k, vec![OwnEdit::new(&it.key, value)], said);
    Out::Draw
}

/// e on any row: the settings file in your editor.
fn open_key(m: &mut Model, s: &str) -> Out {
    if s == "e"
        && let Some(r) = &dialog(m).own
        && !r.settings.path.is_empty()
    {
        let p = r.settings.path.clone();
        return edit_file(m, &p);
    }
    Out::Pass
}

/// One of the harness's settings as a row of its page.
fn row(k: &str, it: &OwnSetting, file: &str, path: &str) -> Setting {
    let mut st = Setting {
        label: it.label.clone(),
        value: it.value.clone(),
        what: it.what.clone(),
        unset: it.unset.clone(),
        source: it.from.word(file),
        key: Some(Rc::new(open_key)),
        ..Setting::default()
    };
    if !path.is_empty() {
        st.keys = vec!["e".into(), format!("open {file}")];
    }
    for (v, means) in &it.choices {
        st.choices.push(v.clone());
        st.means.insert(v.clone(), means.clone());
    }
    st.typed = matches!(
        it.kind,
        OwnKind::Open | OwnKind::Text | OwnKind::Number | OwnKind::Tokens | OwnKind::Var
    );
    let (k1, it1, f1) = (k.to_string(), it.clone(), file.to_string());
    st.run = Some(Rc::new(move |m: &mut Model, v: &str| {
        change(m, &k1, &it1, &f1, v)
    }));
    match it.kind {
        OwnKind::Var => {
            // x takes a variable out.
            let (k2, it2, f2) = (k.to_string(), it.clone(), file.to_string());
            st.key = Some(Rc::new(move |m, s| match s {
                "x" | "delete" | "backspace" if !it2.value.is_empty() => {
                    let (k3, it3, f3) = (k2.clone(), it2.clone(), f2.clone());
                    m.ask(
                        Confirm::new(format!("Remove {}?", it2.label), move |m| {
                            change(m, &k3, &it3, &f3, "")
                        })
                        .detail(format!("from {f2}")),
                    );
                    Out::Draw
                }
                _ => open_key(m, s),
            }));
            st.keys
                .splice(0..0, ["x".to_string(), "remove it".to_string()]);
            st.source.clear();
        }
        OwnKind::NewVar => {
            let (k2, it2, f2) = (k.to_string(), it.clone(), file.to_string());
            st.line = Some(Rc::new(|_| line([sp("+ add a variable", FAINT)])));
            st.key = Some(Rc::new(move |m, s| {
                if !matches!(s, "enter" | "right" | "a") {
                    return open_key(m, s);
                }
                let (k3, it3, f3) = (k2.clone(), it2.clone(), f2.clone());
                ask(
                    m,
                    "NAME=value",
                    "",
                    Rc::new(move |m: &mut Model, v: &str| change(m, &k3, &it3, &f3, v)),
                );
                Out::Draw
            }));
            st.keys = vec!["enter".into(), "add NAME=value".into()];
            st.source.clear();
        }
        OwnKind::Plugin => st.source.clear(),
        _ => {}
    }
    let (title, what, shown, said) = (
        it.label.clone(),
        it.what.clone(),
        st.clone(),
        it.from.said(file),
    );
    st.about = Some(Rc::new(move || {
        let mut now = shown.now();
        if !now.is_empty() {
            now.push_str(&format!(" From {said}."));
        }
        (title.clone(), what.clone(), now)
    }));
    st
}

/// Harness k's own settings as sections of its page: none when it offers
/// none, one line while they're read.
pub fn sections(m: &Model, k: &str) -> Vec<Section> {
    if k.is_empty() || !has(k) {
        return Vec::new();
    }
    let Some(r) = dialog(m).own.as_ref().filter(|r| r.kind == k) else {
        return vec![Section {
            title: "Its own settings".into(),
            rows: vec![Setting {
                label: "reading".into(),
                line: Some(Rc::new(|_| line([sp("reading its settings…", FAINT)]))),
                ..Setting::default()
            }],
            ..Section::default()
        }];
    };
    let s = &r.settings;
    let file = std::path::Path::new(&s.file)
        .file_name()
        .map_or(s.file.clone(), |n| n.to_string_lossy().into_owned());
    let mut out = Vec::new();
    for (i, g) in s.groups.iter().enumerate() {
        let mut note = g.note.clone();
        if i == 0 {
            note = [s.file.as_str(), s.note.as_str(), note.as_str()]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
        }
        let rows: Vec<Setting> = g
            .items
            .iter()
            .map(|it| row(k, it, &file, &s.path))
            .collect();
        out.push(Section {
            title: g.title.clone(),
            note,
            rows,
            ..Section::default()
        });
    }
    out
}

/// Whether harness k offers settings of its own (files.rs leaves its
/// generic section out then).
pub fn offers(k: &str) -> bool {
    reg().get(k).is_some_and(|a| a.harness_settings().is_some())
}

/// offers, or a test's harness.
fn has(k: &str) -> bool {
    if TEST.with(|t| t.borrow().is_some()) {
        return true;
    }
    offers(k)
}
