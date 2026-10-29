//! The settings places' tests: Go's settings_test.go,
//! settings_catalog_test.go, settings_catalog_layout_test.go,
//! settings_controls_test.go, settings_updates_test.go,
//! settings_remote_test.go and settings_workspace_test.go, and frames at
//! the widths Go's `rush --render` was compared at. Every test keeps
//! rush's folder in a temp dir of its own (rush_state's thread dir), so
//! nothing here reads or writes the real one; work handed off runs at
//! once, so nothing sleeps.

use std::collections::HashMap;
use std::rc::Rc;

use rush_core::adapter::{AgentDef, Command, Feature, Has, Support, all_features};
use rush_core::text::Line;
use rush_core::usage::{Quota, Window};
use rush_harnessup::{State as Up, Status, Update};
use rush_plugin::Requires;
use rush_state::Store;

use super::acct::{self, AcctRow};
use super::form::{self, Section, Setting, choice, flat, form_key, form_keys, setting_row};
use super::paint::{civil, from_ansi};
use super::sheets::{self, PermSheet, SkillSheet};
use super::statusline::{self, StatusSheet};
use super::*;
use crate::frame::Press;
use crate::model::Model;

/// A model w×h whose rush folder is a temp dir, and a store loaded from
/// it. The dir lives as long as the guard.
fn model(w: usize, h: usize) -> (Model, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir");
    rush_state::set_thread_dir(Some(dir.path().to_path_buf()));
    rush_state::set_thread_cache_dir(Some(dir.path().join("cache")));
    let mut m = Model::new("0.1.0");
    m.clock = Some(1_793_190_896_789);
    m.now = 1_793_190_896_789;
    m.w = w;
    m.h = h;
    use_store(&mut m, Store::load_with_env(&|_| None));
    (m, dir)
}

fn press(m: &mut Model, s: &str) -> Out {
    let p = Press::of(s).unwrap_or_else(|| panic!("no key {s:?}"));
    m.press_key(&p)
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

fn text(ls: &[Line]) -> String {
    ls.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

fn body(m: &Model, w: usize) -> String {
    let mut g = workspace::Geometry::default();
    text(&dialog_body(m, &mut g, w, settings_height(m)))
}

// ---- settings_test.go ----

/// [ and ] go through a place's pages, round the ends, as tab and
/// shift+tab do (TestSettingsPagesBrackets).
#[test]
fn pages_brackets() {
    let (mut m, _d) = model(140, 50);
    set_settings_page(&mut m, PAGE_PROVIDERS);
    assert_eq!(m.place, PlaceId::Harnesses);
    press(&mut m, "]");
    assert_eq!(dialog(&m).page, PAGE_HARNESSES, "] from Providers");
    press(&mut m, "[");
    press(&mut m, "[");
    assert_eq!(dialog(&m).page, PAGE_CONFIG, "[ from Providers goes round");
    set_settings_page(&mut m, PAGE_GENERAL);
    assert_eq!(m.place, PlaceId::Settings);
    press(&mut m, "tab");
    assert_eq!(dialog(&m).page, PAGE_GENERAL + 1, "tab is the next page");
    press(&mut m, "shift+tab");
    assert_eq!(dialog(&m).page, PAGE_GENERAL, "shift+tab goes back");
    let f = screen(&mut m);
    assert!(
        f.iter().any(|l| l.contains("General") && l.contains("[ ]")),
        "the pages row says [ ]:\n{}",
        f.join("\n")
    );
    press(&mut m, "esc");
    assert_eq!(m.place, PlaceId::Agents, "esc goes back to Agents");
}

/// Every page draws, and every form row explains itself
/// (TestSettingsPagesDraw).
#[test]
fn every_page_draws_and_explains_itself() {
    let (mut m, _d) = model(140, 50);
    for (i, p) in pages().iter().enumerate() {
        set_settings_page(&mut m, i);
        let mut g = workspace::Geometry::default();
        let b = dialog_body(&m, &mut g, 130, settings_height(&m));
        // With no agent installed Providers has nothing to list.
        let least = if i == PAGE_PROVIDERS && providers::items(&m).is_empty() {
            2
        } else {
            3
        };
        assert!(b.len() >= least, "{} drew {} lines", p.name, b.len());
        for l in &b {
            assert!(
                l.width() <= 130,
                "{}: a row past 130: {:?}",
                p.name,
                l.plain()
            );
        }
        let Some(f) = p.form else {
            continue;
        };
        for st in flat(&f(&m)) {
            assert!(
                st.line.is_some() || !st.what.is_empty() || st.about.is_some(),
                "{} › {} doesn't say what it does",
                p.name,
                st.label
            );
        }
    }
}

/// A plugin's requirements are a cell each, met here or not
/// (TestNeedCells).
#[test]
fn need_cells() {
    let os = rush_plugin::GOOS;
    let r = Requires {
        os: vec![os.into(), "plan9".into()],
        arch: vec![rush_plugin::GOARCH.into()],
        bin: vec!["git".into(), "nope".into()],
    };
    let got = plugins::need_cells(&r, &["nope".into()]).plain();
    let want = format!(
        "✓ {}  – plan9  ✓ {}  ✓ git on PATH  – nope on PATH",
        rush_plugin::os_name(os).unwrap_or(os),
        rush_plugin::GOARCH
    );
    assert_eq!(got, want);
    assert_eq!(
        plugins::need_cells(&Requires::default(), &[]).plain(),
        "nothing; it runs anywhere"
    );
}

/// What a model takes: unknown until it's used, for an agent that knows
/// nothing of it (TestModelTakes).
#[test]
fn model_takes_unknown() {
    let got = common::model_takes("nobody", "m").plain();
    assert!(got.contains("unknown to rush"), "{got}");
}

/// Clicking a tab in the pages row goes to that page (TestClickingTabs).
#[test]
fn tabs_name_the_place_pages() {
    let (mut m, _d) = model(140, 50);
    set_settings_page(&mut m, PAGE_GENERAL);
    let tabs = workspace::tabs(&m, PlaceId::Settings, 140).plain();
    assert!(
        tabs.starts_with("General   Keys   Plugins   Updates   Remote"),
        "{tabs}"
    );
    assert!(tabs.trim_end().ends_with("[ ]"), "{tabs}");
    let tabs = workspace::tabs(&m, PlaceId::Harnesses, 140).plain();
    assert!(tabs.starts_with("Providers   Harnesses   Config"), "{tabs}");
    // Narrow, the page showing and its neighbours stay, with ‹ › for the
    // rest.
    set_settings_page(&mut m, PAGE_UPDATES);
    let tabs = workspace::tabs(&m, PlaceId::Settings, 24).plain();
    assert!(tabs.contains("Updates"), "{tabs}");
    assert!(tabs.contains('‹') || tabs.contains('›'), "{tabs}");
}

// ---- settings_catalog_test.go ----

/// A definition is listed once however many worktrees have it
/// (TestSameDefsOnce).
#[test]
fn same_defs_once() {
    let def = |name: &str, scope: &str, model: &str, desc: &str| AgentDef {
        name: name.into(),
        scope: scope.into(),
        model: model.into(),
        desc: desc.into(),
        path: String::new(),
    };
    let got = files::same_defs_once(vec![
        def("claude", "", "", ""),
        def("lane", "langwatch", "opus", "a lane"),
        def("lane", "feat-a", "opus", "a lane"),
        def("lane", "feat-b", "opus", "a lane"),
        def("lane", "feat-c", "sonnet", "changed here"),
    ]);
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got[1].scope, "langwatch +2");
    assert_eq!(got[2].scope, "feat-c");
}

// ---- settings_catalog_layout_test.go ----

/// What rush can do is said short: how many, then only what it can't
/// and what's planned (TestFeatureSummaryNamesOnlyWhatItCant).
#[test]
fn feature_summary_names_only_what_it_cant() {
    let all = all_features();
    let (cant, planned) = (all[0].feature, all[1].feature);
    let got = text(&common::feature_summary(
        |f| {
            if f == cant {
                Support::NO
            } else if f == planned {
                Support::PLANNED
            } else {
                Support::YES
            }
        },
        150,
    ));
    assert!(
        got.contains(&format!("{} of rush's {}", all.len() - 2, all.len())),
        "{got}"
    );
    assert!(got.contains(&format!("can't: {}", all[0].label)), "{got}");
    assert!(got.contains(&format!("planned: {}", all[1].label)), "{got}");
    assert!(
        !got.contains(all[2].label),
        "what it can do is listed:\n{got}"
    );
    assert!(got.matches('\n').count() <= 3, "{got}");
    let every = text(&common::feature_summary(|_| Support::YES, 80));
    assert!(every.contains("everything rush does"), "{every}");
    assert_ne!(
        common::pairing_support("definitely-missing", "", Feature::Run).is,
        Has::Yes,
        "an unknown harness advertised as supported"
    );
}

/// Harnesses is a list and its detail, side by side when wide and one
/// over the other when narrow; enter goes into Every harness and esc back
/// (TestHarnessCatalogShowsDetailsAndDenseFeatures).
#[test]
fn harnesses_catalog_wide_and_narrow() {
    let (mut m, _d) = model(140, 40);
    set_settings_page(&mut m, PAGE_HARNESSES);
    let wide = body(&m, 130);
    assert!(wide.contains("▍ Every harness"), "{wide}");
    assert!(wide.contains(" │ Every harness"), "{wide}");
    assert!(
        wide.contains("What every harness starts with; each one shows only what it changes."),
        "{wide}"
    );
    assert!(wide.contains("→ controls"), "{wide}");
    let narrow = body(&m, 60);
    assert!(
        !narrow.contains('│'),
        "narrow is one over the other:\n{narrow}"
    );
    assert!(narrow.contains(&"─".repeat(60)), "{narrow}");
    press(&mut m, "enter");
    assert!(dialog(&m).harness_inside, "enter goes into Every harness");
    let inside = body(&m, 130);
    assert!(inside.contains("Effort"), "{inside}");
    assert!(inside.contains("Credit in commits and PRs"), "{inside}");
    assert!(inside.contains("About Effort"), "{inside}");
    // → changes the effort every harness starts with, saved at once.
    press(&mut m, "right");
    assert_eq!(config(&m).dispatch.every.effort, "low");
    let saved = std::fs::read_to_string(rush_state::dir().join("config.json")).expect("saved");
    assert!(saved.contains("\"low\""), "{saved}");
    press(&mut m, "esc");
    assert!(!dialog(&m).harness_inside, "esc goes back to the list");
    assert_eq!(m.place, PlaceId::Harnesses, "and stays on Harnesses");
}

/// The catalog's keys: home, end and paging move the detail, not the
/// list (catalogKey).
#[test]
fn catalog_keys_page_the_detail() {
    let (mut m, _d) = model(140, 40);
    set_settings_page(&mut m, PAGE_HARNESSES);
    press(&mut m, "pgdown");
    assert!(
        dialog(&m).catalog_scroll.get() > 0,
        "pgdown scrolls the detail"
    );
    press(&mut m, "pgup");
    assert_eq!(dialog(&m).catalog_scroll.get(), 0);
    press(&mut m, "end");
    press(&mut m, "home");
    assert_eq!(dialog(&m).cursor, 0);
}

// ---- settings_controls_test.go ----

/// Actions and facts aren't drawn as choices; ←→ never runs an action,
/// enter does (TestSettingsActionsAndFactsAreNotSelectors).
#[test]
fn actions_and_facts_are_not_selectors() {
    let (mut m, _d) = model(140, 50);
    let called = Rc::new(std::cell::Cell::new(false));
    let c2 = called.clone();
    let action = Setting {
        label: "Sign in".into(),
        keys: vec!["enter".into(), "sign in".into()],
        key: Some(Rc::new(move |_, s| {
            if s == "enter" {
                c2.set(true);
                return Out::Draw;
            }
            Out::Pass
        })),
        ..Setting::default()
    };
    let fact = Setting {
        label: "Status".into(),
        value: "not checked".into(),
        ..Setting::default()
    };
    let mut fixed = choice(
        "Provider",
        "one",
        "",
        &[("one", "Only provider")],
        form::set(|_, _| panic!("a fixed choice must not change the config")),
    );
    fixed.names.insert("one".into(), "Only provider".into());
    for st in [&action, &fact, &fixed] {
        let row = setting_row(false, st, 24, 18, 100).plain();
        assert!(
            !row.contains(['‹', '›']) && !row.contains("default"),
            "a fake selector: {row:?}"
        );
    }
    let row = setting_row(false, &action, 24, 18, 100).plain();
    assert!(row.contains("[enter]"), "{row:?}");
    open_dialog(&mut m, PAGE_GENERAL);
    form_key(&mut m, &[Section::new("", vec![fixed])], "right");
    form_key(&mut m, &[Section::new("", vec![action.clone()])], "right");
    assert!(!called.get(), "→ ran an action");
    form_key(&mut m, &[Section::new("", vec![action])], "enter");
    assert!(called.get(), "enter didn't run it");
    let effort = choice(
        "Effort",
        "high",
        "",
        &[("high", "high effort"), ("low", "low effort")],
        form::set(|_, _| {}),
    );
    let row = setting_row(false, &effort, 24, 18, 100).plain();
    assert!(row.contains('‹'), "a real choice lost its arrows: {row:?}");
}

/// Rows stay within their width, and a key line names a key once
/// (TestSettingsRowsStayWithinWidthAndHintsAreUnique).
#[test]
fn rows_stay_within_width_and_hints_are_unique() {
    for w in [24, 60, 120] {
        let rows = [
            Setting {
                label: "Long status label".into(),
                value: "x".repeat(200),
                ..Setting::default()
            },
            Setting {
                line: Some(Rc::new(|_| line([plain("界".repeat(200))]))),
                ..Setting::default()
            },
            Setting {
                label: "Run action".into(),
                key: Some(Rc::new(|_, _| Out::Pass)),
                keys: vec!["enter".into(), "run".into()],
                ..Setting::default()
            },
        ];
        for st in &rows {
            for on in [false, true] {
                let row = setting_row(on, st, 32, 20, w);
                assert!(row.width() <= w, "width {} past {w}", row.width());
            }
        }
    }
    let hint = form_keys(
        &Setting {
            keys: vec!["enter".into(), "restart idle sessions".into()],
            ..Setting::default()
        },
        &[
            "enter".into(),
            "update".into(),
            "r".into(),
            "refresh".into(),
        ],
        160,
    )
    .plain();
    assert_eq!(hint.matches("enter").count(), 1, "{hint:?}");
    assert!(hint.contains("restart idle sessions"), "{hint:?}");
    assert_eq!(
        Setting {
            label: "Sign in".into(),
            ..Setting::default()
        }
        .now(),
        ""
    );
}

// ---- the form's keys: what Go's formKey does with each ----

/// The keys a form row takes, and what each does to a choice of three:
/// → l space enter forward, ← h back, round the ends; anything else
/// nothing (formKey).
#[test]
fn form_key_table() {
    let (mut m, _d) = model(140, 50);
    open_dialog(&mut m, PAGE_GENERAL);
    let seen = Rc::new(std::cell::RefCell::new(String::new()));
    let row = |v: &str| {
        let s2 = seen.clone();
        choice(
            "Pick",
            v,
            "what",
            &[("a", ""), ("b", ""), ("c", "")],
            form::set(move |_, v| *s2.borrow_mut() = v.into()),
        )
    };
    for (key, from, want) in [
        ("right", "a", "b"),
        ("l", "a", "b"),
        ("space", "a", "b"),
        ("enter", "a", "b"),
        ("right", "c", "a"),
        ("left", "a", "c"),
        ("h", "b", "a"),
        ("x", "a", ""),
        ("up", "a", ""),
    ] {
        seen.borrow_mut().clear();
        form_key(&mut m, &[Section::new("", vec![row(from)])], key);
        assert_eq!(*seen.borrow(), want, "{key} on {from}");
    }
}

/// A typed row asks for its value on enter, prefilled, and the answer is
/// set once enter takes it (ask, dialogKey).
#[test]
fn typed_rows_ask() {
    let (mut m, _d) = model(140, 50);
    open_dialog(&mut m, PAGE_GENERAL);
    let seen = Rc::new(std::cell::RefCell::new(String::new()));
    let s2 = seen.clone();
    let st = Setting {
        label: "NAME".into(),
        value: "was".into(),
        typed: true,
        set: form::set(move |_, v| *s2.borrow_mut() = v.into()),
        ..Setting::default()
    };
    form_key(&mut m, &[Section::new("", vec![st])], "enter");
    assert_eq!(dialog(&m).asking, "NAME");
    assert_eq!(dialog(&m).input.iter().collect::<String>(), "was");
    dialog_key(&mut m, &Press::of("ctrl+u").unwrap());
    for c in "now ".chars() {
        dialog_key(
            &mut m,
            &Press::new(rush_draw::Key::Char(c), rush_draw::KeyMods::NONE),
        );
    }
    dialog_key(&mut m, &Press::of("enter").unwrap());
    assert_eq!(*seen.borrow(), "now", "trimmed");
    assert!(dialog(&m).asking.is_empty());
}

// ---- settings_workspace_test.go and the render oracle ----

/// General as Go's `rush --render 140x40` draws it under the header: the
/// description, a section, the highlighted row, its explanation under the
/// rows (no room for a column beside at 136), the progress rule and the
/// keys.
#[test]
fn general_at_140_as_go_draws_it() {
    let (mut m, _d) = model(140, 40);
    m.set_place(PlaceId::Settings);
    let f = screen(&mut m);
    let at = |s: &str| f.iter().position(|l| l.trim_end() == s);
    assert!(
        at("  Sessions, notifications, background work, and how the workspace looks.").is_some(),
        "{}",
        f.join("\n")
    );
    assert!(at("  Idle and finished").is_some(), "{}", f.join("\n"));
    let hib = f
        .iter()
        .find(|l| l.contains("Hibernate finished agents") && l.contains('‹'))
        .expect("the first row");
    assert!(
        hib.starts_with("  ▍ Hibernate finished agents                "),
        "{hib:?}"
    );
    // Go's About wraps at the body's width less four.
    assert!(
        at("  A finished agent's process stays in memory (often 300–800 MB) until it is stopped. Hibernating stops it; the conversation is kept and").is_some(),
        "{}",
        f.join("\n")
    );
    assert!(at("  enter resumes it.").is_some(), "{}", f.join("\n"));
    let keys = f
        .iter()
        .find(|l| l.contains("? details"))
        .expect("the keys");
    assert_eq!(
        keys.trim_end(),
        "  ←→ change  ·  ? details  ·  ctrl+↑↓ section  ·  [ ] page  ·  esc back"
    );
    assert!(
        f.iter()
            .any(|l| l.trim_start().starts_with("1 of ") && l.contains("───")),
        "the progress rule"
    );
}

/// At 160 the explanation goes beside the rows; at 80 under them; the
/// form never draws past its width and the keys stay on screen
/// (TestSettingsWorkspaceFitsEveryPage).
#[test]
fn form_lays_out_at_every_width() {
    for (w, beside) in [(80, false), (120, true), (160, true)] {
        let (mut m, _d) = model(w, 40);
        m.set_place(PlaceId::Settings);
        press(&mut m, "]"); // Keys
        press(&mut m, "]"); // Plugins, a form without a preview
        press(&mut m, "]"); // Updates
        let f = screen(&mut m);
        assert_eq!(f.len(), 40, "w {w}");
        for l in &f {
            assert!(
                rush_core::text::width(l) <= w,
                "w {w}: past the screen: {l:?}"
            );
        }
        let has_column = f.iter().any(|l| l.contains(" │ "));
        assert_eq!(has_column, beside, "w {w}:\n{}", f.join("\n"));
        assert!(
            f.iter().any(|l| l.contains("? details")),
            "w {w}: the keys fell off:\n{}",
            f.join("\n")
        );
    }
}

/// ? opens the row's whole explanation; ctrl+↓ jumps a section; home and
/// end the ends (TestSettingsHelpAndSectionNavigation).
#[test]
fn help_and_section_navigation() {
    let (mut m, _d) = model(140, 40);
    m.set_place(PlaceId::Settings);
    press(&mut m, "ctrl+down");
    assert_eq!(
        dialog(&m).cursor,
        7,
        "ctrl+↓ to the second section's first row"
    );
    press(&mut m, "ctrl+up");
    assert_eq!(dialog(&m).cursor, 0);
    press(&mut m, "end");
    assert_eq!(dialog(&m).cursor, dialog_len(&m) - 1);
    press(&mut m, "home");
    assert_eq!(dialog(&m).cursor, 0);
    press(&mut m, "?");
    assert!(
        m.sheet_as::<workspace::HelpSheet>().is_some(),
        "? opens the details"
    );
    let f = screen(&mut m).join("\n");
    assert!(f.contains("Hibernate finished agents"), "{f}");
    assert!(f.contains("↑↓ scroll"), "{f}");
    press(&mut m, "esc");
    assert!(m.sheet.is_none());
}

// ---- settings_updates_test.go ----

/// Updates says what each harness can do, how many have an update, and
/// the sessions on an older rush (TestUpdatesPage).
#[test]
fn updates_page() {
    let (mut m, _d) = model(140, 50);
    set_settings_page(&mut m, PAGE_UPDATES);
    {
        let u = &mut m.kept::<State>().updates;
        u.checked = true;
        u.rows = vec![
            Status {
                kind: "codex".into(),
                name: "Codex".into(),
                installed: "0.155.1".into(),
                latest: "0.159.2".into(),
                update: Update {
                    label: "brew upgrade --cask codex".into(),
                    argv: vec!["brew".into()],
                },
                ..Status::default()
            },
            Status {
                kind: "gemini".into(),
                name: "Gemini CLI".into(),
                installed: "0.62.0".into(),
                latest: "0.62.0".into(),
                ..Status::default()
            },
            Status {
                kind: "vibe".into(),
                name: "Vibe".into(),
                installed: "2.25.0".into(),
                ..Status::default()
            },
        ];
        u.stale = 2;
    }
    assert_eq!(m.kept::<State>().updates.rows[0].state(), Up::Available);
    assert_eq!(updates::count(&m), 1, "Codex");
    let b = body(&m, 120);
    for want in [
        "update available",
        "up to date",
        "unknown",
        "2 sessions on an older rush",
    ] {
        assert!(b.contains(want), "Updates lacks {want:?}:\n{b}");
    }
    let l = updates::update_line(
        60,
        "Codex",
        "0.155.1 (Oct 1)",
        "0.159.2",
        line([plain("x")]),
    );
    assert_eq!(l.plain(), "Codex           0.155.1    → 0.159.2    x");
}

// ---- settings_remote_test.go ----

/// Remote's switches, with serve not running (TestRemotePageSwitches).
#[test]
fn remote_page_switches() {
    let (mut m, _d) = model(140, 50);
    set_settings_page(&mut m, PAGE_REMOTE);
    let b = body(&m, 130);
    for want in [
        "Remote mode",
        "Over Tailscale",
        "Through Cloudflare",
        "Web app",
        "Terminal (SSH)",
        "Pair a device",
    ] {
        assert!(b.contains(want), "Remote lacks {want:?}:\n{b}");
    }
    // serve isn't running here: no token in this rush folder.
    assert!(
        b.contains("not running") || b.contains("starting, or not answering"),
        "{b}"
    );
    assert_eq!(remote::pair_code("ABCDEFGH"), "ABCD-EFGH");
    assert_eq!(remote::pair_code("short"), "short");
    let now = 1_793_190_896_789;
    assert_eq!(remote::seen_ago(0, now), "never");
    assert_eq!(remote::seen_ago(now - 10_000, now), "just now");
    assert_eq!(remote::seen_ago(now - 5 * 60_000, now), "5m ago");
    assert_eq!(remote::seen_ago(now - 3 * 3_600_000, now), "3h ago");
}

/// A Web app switch flipped is written to remote.json, keeping what else
/// it says.
#[test]
fn remote_switch_writes_remote_json() {
    let (mut m, _d) = model(140, 50);
    std::fs::write(
        rush_state::dir().join("remote.json"),
        r#"{"listen":"127.0.0.1:9999","peers":[{"name":"x"}]}"#,
    )
    .unwrap();
    set_settings_page(&mut m, PAGE_REMOTE);
    let web = flat(&remote_sections(&m))
        .iter()
        .position(|s| s.label == "Web app")
        .expect("Web app");
    dialog_mut(&mut m).cursor = web;
    press(&mut m, "right");
    let raw = std::fs::read_to_string(rush_state::dir().join("remote.json")).unwrap();
    assert!(
        raw.contains("\"web\": true") || raw.contains("\"web\":true"),
        "{raw}"
    );
    assert!(raw.contains("9999"), "the rest kept: {raw}");
    let b = body(&m, 130);
    assert!(
        b.contains("Other machines"),
        "a peer shows its section:\n{b}"
    );
}

fn remote_sections(m: &Model) -> Vec<Section> {
    (remote::page().form.expect("a form"))(m)
}

// ---- Plugins ----

/// Plugins lists none with where to put one, and opening one shows its
/// switches (pluginSections, pluginPage).
#[test]
fn plugins_page() {
    let (mut m, _d) = model(140, 50);
    set_settings_page(&mut m, PAGE_PLUGINS);
    let b = body(&m, 130);
    assert!(b.contains("No plugins."), "{b}");
    assert!(b.contains("rush plugin approve <name>"), "{b}");
    m.kept::<State>().plugins = Some(plugins::Read {
        rows: vec![plugins::Row {
            m: rush_plugin::Manifest {
                name: "haven".into(),
                description: "stacks".into(),
                ..Default::default()
            },
            runs: false,
            status: "not approved".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let b = body(&m, 130);
    assert!(b.contains("○ haven"), "{b}");
    press(&mut m, "enter");
    assert_eq!(dialog(&m).plugin, "haven");
    let b = body(&m, 130);
    assert!(b.starts_with("haven · Manage extensions"), "{b}");
    assert!(b.contains("rush plugin approve haven"), "{b}");
    assert!(b.contains("Requires"), "{b}");
    press(&mut m, "esc");
    assert_eq!(dialog(&m).plugin, "", "esc goes back to the list");
    assert_eq!(m.place, PlaceId::Settings);
    assert_eq!(
        plugins::join_and(&["a".into(), "b".into(), "c".into()]),
        "a, b and c"
    );
}

// ---- Config ----

/// With no provider installed, Config says where to add one, and its keys
/// do nothing (configBody, configKey).
#[test]
fn config_with_nothing_installed() {
    let (mut m, _d) = model(140, 50);
    set_settings_page(&mut m, PAGE_CONFIG);
    let b = body(&m, 130);
    assert!(
        b.contains("No provider is installed yet: Settings › Providers says how to add one."),
        "{b}"
    );
    assert_eq!(press(&mut m, "space"), Out::Idle);
}

// ---- accounts as Settings shows them ----

/// The accounts' windows added up: each averaged over those with a
/// reading, resetting when the first does (together).
#[test]
fn accounts_together() {
    let win = |label: &str, pct: f64, at: i64| Window {
        label: label.into(),
        percent: pct,
        resets_at: at,
        ..Window::default()
    };
    let row = |ws: Vec<Window>| AcctRow {
        q: Quota {
            windows: ws,
            ..Quota::default()
        },
        ..AcctRow::default()
    };
    let (all, n) = acct::together(&[
        row(vec![win("5h", 20.0, 2000), win("7d", 50.0, 9000)]),
        row(vec![win("5h", 60.0, 1000)]),
        row(Vec::new()),
    ]);
    assert_eq!(n, 2);
    assert_eq!(all.windows[0].percent, 40.0);
    assert_eq!(all.windows[0].resets_at, 1000);
    assert_eq!(all.windows[1].percent, 50.0);
    assert_eq!(acct::roughly(30 * 60_000), "30m");
    assert_eq!(acct::roughly(5 * 3_600_000), "5h");
    assert_eq!(acct::roughly(72 * 3_600_000), "3d");
}

/// An account's limits: a meter for each of its first two windows, or
/// why there are none (limits).
#[test]
fn account_limits() {
    let (m, _d) = model(140, 50);
    let mut r = AcctRow {
        kind: rush_state::LOGINS_KIND.into(),
        ..AcctRow::default()
    };
    assert_eq!(acct::limits(&m, &r, 26, 26).plain().trim_end(), "fetching…");
    r.q.fetched_at = m.now - 60_000;
    r.q.windows = vec![Window {
        label: "5h".into(),
        percent: 15.0,
        ..Window::default()
    }];
    let l = acct::limits(&m, &r, 26, 26).plain();
    assert!(l.starts_with("5h ▰▰▱▱▱▱▱▱▱▱  15%"), "{l:?}");
    assert_eq!(rush_core::text::width(&l), 52);
}

// ---- sheets ----

/// A skill's size as tokens, where it comes from, and the network's rate
/// short (approxTokens, sourceTitle, netRate).
#[test]
fn sheet_words() {
    assert_eq!(sheets::approx_tokens(0), "");
    assert_eq!(sheets::approx_tokens(400), "~100 tok");
    assert_eq!(sheets::approx_tokens(6000), "~1.5k tok");
    assert_eq!(sheets::source_title("yours"), "Yours");
    assert_eq!(sheets::source_title("project"), "This project's");
    assert_eq!(sheets::source_title("superpowers"), "Plugin · superpowers");
    assert_eq!(sheets::net_rate(0.0), "0B");
    assert_eq!(sheets::net_rate(40.0 * 1024.0), "40K");
    assert_eq!(sheets::net_rate(1.2 * 1024.0 * 1024.0), "1.2M");
    assert_eq!(sheets::net_rate(20.0 * 1024.0 * 1024.0), "20M");
}

/// /skills: yours first, typing narrows, enter uses one.
#[test]
fn skills_sheet() {
    let (mut m, _d) = model(140, 50);
    let used = Rc::new(std::cell::RefCell::new(String::new()));
    let u2 = used.clone();
    let cmd = |name: &str, source: &str, skill: bool| Command {
        name: name.into(),
        description: format!("does {name}"),
        source: source.into(),
        skill,
        size: 4000,
        ..Command::default()
    };
    m.open_sheet(Box::new(SkillSheet {
        all: vec![
            cmd("zeta", "superpowers", true),
            cmd("alpha", "yours", true),
            cmd("beta", "project", true),
            cmd("deploy", "yours", false),
        ],
        tab: 0,
        cur: [0; 2],
        query: Vec::new(),
        query_pos: 0,
        use_it: Rc::new(move |_, n| *u2.borrow_mut() = n.into()),
    }));
    let f = screen(&mut m).join("\n");
    assert!(f.contains("Skills 3  ·  Commands 1"), "{f}");
    let ya = f.find("Yours").unwrap();
    let pp = f.find("This project's").unwrap();
    let pl = f.find("Plugin · superpowers").unwrap();
    assert!(
        ya < pp && pp < pl,
        "yours, then the project's, then plugins:\n{f}"
    );
    for c in "bet".chars() {
        m.press_key(&Press::new(
            rush_draw::Key::Char(c),
            rush_draw::KeyMods::NONE,
        ));
    }
    let names: Vec<String> = m
        .sheet_as::<SkillSheet>()
        .unwrap()
        .shown()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["beta"]);
    press(&mut m, "enter");
    assert_eq!(*used.borrow(), "beta");
    assert!(m.sheet.is_none(), "enter closes it");
}

/// /permissions reads every file's rules, and a rule added goes into the
/// file picked, which is then read again (readRules, change).
#[test]
fn permissions_sheet_adds_a_rule() {
    let (mut m, d) = model(140, 50);
    let user = d.path().join("settings.json");
    std::fs::write(
        &user,
        r#"{"permissions":{"allow":["Read"],"defaultMode":"plan"}}"#,
    )
    .unwrap();
    let files = vec![sheets::SettingsFile {
        label: "yours".into(),
        path: user.to_string_lossy().into_owned(),
    }];
    let mut p = PermSheet::default();
    p.adopt(sheets::read_rules(files));
    assert_eq!(p.rules[0].len(), 1);
    assert_eq!(p.mode, "plan (yours)");
    m.open_sheet(Box::new(p));
    let f = screen(&mut m).join("\n");
    assert!(f.contains("Allow 1  ·  Ask 0  ·  Deny 0"), "{f}");
    assert!(f.contains("default mode: plan (yours)"), "{f}");
    press(&mut m, "a");
    for c in "Bash(ls)".chars() {
        m.press_key(&Press::new(
            rush_draw::Key::Char(c),
            rush_draw::KeyMods::NONE,
        ));
    }
    press(&mut m, "enter");
    let saved = std::fs::read_to_string(&user).unwrap();
    assert!(saved.contains("Bash(ls)"), "{saved}");
    assert_eq!(m.sheet_as::<PermSheet>().unwrap().rules[0].len(), 2);
}

/// Hooks as settings files write them: by event, a command or prompt each
/// (parseHooks).
#[test]
fn parse_hooks() {
    let raw: serde_json::Value = serde_json::from_str(
        r#"{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}],
            "PreToolUse":[{"matcher":"Bash","hooks":[{"type":"prompt","prompt":"check it"}]}]}"#,
    )
    .unwrap();
    let f = sheets::SettingsFile {
        label: "yours".into(),
        path: "/x".into(),
    };
    let hs = sheets::parse_hooks(&raw, &f);
    assert_eq!(hs.len(), 2);
    assert_eq!(hs[0].event, "PreToolUse");
    assert_eq!(hs[0].matcher, "Bash");
    assert_eq!(hs[0].command, "check it");
    assert_eq!(hs[1].command, "say done");
}

/// The layout sheet: ↓ then enter keeps the Session alone, and the menu
/// bar is offered after on a Mac (viewSheet, askMenuBar).
#[test]
fn view_sheet_keeps_the_layout() {
    let (mut m, _d) = model(140, 50);
    sheets::start_view(&mut m);
    assert!(
        m.sheet_as::<sheets::ViewSheet>().is_some(),
        "asked the first time"
    );
    let f = screen(&mut m).join("\n");
    assert!(f.contains("Layout  how should rush open?"), "{f}");
    assert!(
        f.contains("▍ Side by side  Agents, and the picked agent's Session beside them"),
        "{f}"
    );
    press(&mut m, "down");
    press(&mut m, "enter");
    assert_eq!(config(&m).view, "agent");
    if cfg!(target_os = "macos") {
        assert!(
            m.sheet_as::<sheets::MenuBarSheet>().is_some(),
            "the menu bar is offered"
        );
        press(&mut m, "n");
        assert!(config(&m).menu_bar_asked && !config(&m).menu_bar);
    }
    assert!(m.sheet.is_none());
}

// ---- /statusline ----

fn status_sheet(m: &mut Model) {
    statusline::open_status_line(
        m,
        "nobody",
        rush_core::adapter::Profile::default(),
        "",
        statusline::preview_input("", "/tmp", "", 0.0),
    );
}

/// The builder's slots, rows, and keys that move a segment: space hides
/// and shows, shift+↑↓ move it, a number puts it on that line
/// (slots, rows, place, key).
#[test]
fn status_line_builder_keys() {
    let (mut m, _d) = model(140, 50);
    status_sheet(&mut m);
    let st = m.sheet_as::<StatusSheet>().expect("open");
    assert_eq!(
        st.tab_names(),
        ["Agent header", "Top bar"],
        "no line of its own"
    );
    let first = st.slots()[0].clone();
    assert_eq!(first.line, Some(0));
    // space takes it off its line.
    press(&mut m, "space");
    let st = m.sheet_as::<StatusSheet>().unwrap();
    let at = st.slots().iter().position(|s| s.id == first.id).unwrap();
    assert_eq!(st.slots()[at].line, None, "space hides it");
    assert_eq!(st.cur, at, "the cursor goes with it");
    // 2 puts it on the second line.
    press(&mut m, "2");
    let st = m.sheet_as::<StatusSheet>().unwrap();
    let s = st.slots().into_iter().find(|s| s.id == first.id).unwrap();
    assert_eq!(s.line, Some(1));
    // shift+↑ moves it up its line, then onto the end of the one above.
    for _ in 0..30 {
        let st = m.sheet_as::<StatusSheet>().unwrap();
        if st
            .slots()
            .into_iter()
            .any(|s| s.id == first.id && s.line == Some(0))
        {
            break;
        }
        press(&mut m, "shift+up");
    }
    let st = m.sheet_as::<StatusSheet>().unwrap();
    let line0 = &st.bars.agent.lines[0];
    assert_eq!(
        line0.last(),
        Some(&first.id),
        "at the end of line 1: {line0:?}"
    );
    let rows = st.rows();
    assert!(rows[0].head, "a line's heading first");
    assert!(rows.last().is_some_and(|r| r.slot.is_some() || r.head));
    // ] goes to the top bar; s changes its separator.
    press(&mut m, "]");
    let st = m.sheet_as::<StatusSheet>().unwrap();
    assert_eq!(st.tab, 1);
    let was = st.bars.top.sep.clone();
    press(&mut m, "s");
    assert_ne!(m.sheet_as::<StatusSheet>().unwrap().bars.top.sep, was);
    let f = screen(&mut m).join("\n");
    assert!(f.contains("Status lines  the top right of rush"), "{f}");
    assert!(f.contains("Line 1"), "{f}");
    assert!(f.contains("Not shown"), "{f}");
    // enter saves the bars and closes.
    press(&mut m, "enter");
    assert!(m.sheet.is_none(), "saved and closed");
    let saved = rush_bundled::statusline::load_bars();
    assert_eq!(saved.top.sep, m_sep_after(&was));
}

fn m_sep_after(was: &str) -> String {
    let seps = rush_bundled::statusline::SEPS;
    let i = seps.iter().position(|s| *s == was);
    seps[i.map_or(0, |i| (i + 1) % seps.len())].to_string()
}

/// A layout as saved has no empty lines at its end (trimmed).
#[test]
fn trimmed_layout() {
    let l = rush_bundled::statusline::Layout {
        lines: vec![vec!["a".into()], vec![], vec![]],
        ..Default::default()
    };
    assert_eq!(statusline::trimmed(&l).lines.len(), 1);
}

// ---- small pieces ----

/// The status line's colours read back as spans (from_ansi).
#[test]
fn ansi_reads_back() {
    let l = from_ansi("\x1b[38;2;217;119;87mOpus\x1b[0m · \x1b[1mmain\x1b[0m");
    assert_eq!(l.plain(), "Opus · main");
    assert_eq!(
        l.spans[0].style.fg,
        rush_core::text::Color::Rgb(217, 119, 87)
    );
    assert!(l.spans[2].style.mods.contains(rush_core::text::Mods::BOLD));
}

/// Local time's parts, from Unix ms (civil).
#[test]
fn civil_parts() {
    // A whole number of days after the epoch, at local midnight or near.
    let c = civil(0);
    assert!(c.year == 1970 || c.year == 1969);
    let c = civil(1_700_000_000_000 - crate::model::local_zone(1_700_000_000_000) * 1000);
    assert_eq!((c.year, c.month, c.day), (2023, 11, 14));
    assert_eq!((c.hour, c.minute), (22, 13));
    assert_eq!(c.weekday_name(), "Tue");
    assert_eq!(c.month_name(), "Nov");
}

/// The rows of a page where the Keys page sits: its own keys go to Keys
/// (Keys' own table is in keys.rs's tests); here only that Keys draws in
/// the shell.
#[test]
fn keys_page_in_the_shell() {
    let (mut m, _d) = model(140, 50);
    set_settings_page(&mut m, PAGE_KEYS);
    let b = body(&m, 130);
    assert!(
        b.starts_with("Find, practice and customize your shortcuts."),
        "{b}"
    );
}

/// A setting's means and names: how its value is shown (shown, now).
#[test]
fn setting_shown_and_now() {
    let mut st = choice("X", "", "what", &[("", "nothing"), ("a", "an a")], None);
    assert_eq!(st.shown(), "default");
    assert_eq!(st.now(), "default: nothing");
    st.unset = "as it was".into();
    assert_eq!(st.shown(), "as it was");
    st.value = "a".into();
    st.names = HashMap::from([("a".to_string(), "Ay".to_string())]);
    assert_eq!(st.now(), "Ay: an a");
}
