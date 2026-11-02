//! A harness's own settings on its page (ownsettings.rs), driven through
//! Claude Code's HarnessSettings: each value with where it comes from,
//! changed by keys and written to settings.json with the rest of it kept,
//! the links kept; and the arrow keys on the agents' pages, which never
//! change a value going back.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use rush_state::Store;
use serde_json::{Value, json};

use super::form::flat;
use super::ownsettings::TEST;
use super::*;
use crate::frame::Press;
use crate::model::Model;

fn model(w: usize, h: usize) -> (Model, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir");
    rush_state::set_thread_dir(Some(dir.path().to_path_buf()));
    rush_state::set_thread_cache_dir(Some(dir.path().join("cache")));
    let mut m = crate::new_model("0.1.0");
    m.w = w;
    m.h = h;
    use_store(&mut m, Store::load_with_env(&|_| None));
    (m, dir)
}

fn press(m: &mut Model, s: &str) -> Out {
    let p = Press::of(s).unwrap_or_else(|| panic!("no key {s:?}"));
    m.press_key(&p)
}

const K: &str = common::LOGINS_KIND;

/// Claude Code's sections drawn, w wide.
fn body(m: &Model, w: usize) -> String {
    form::form_rows(&ownsettings::sections(m, K), usize::MAX, w)
        .iter()
        .map(rush_core::text::Line::plain)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Settings open with Claude Code's settings read from dir with env,
/// through its HarnessSettings. (Its page is Harnesses' once Claude Code
/// is registered, which the binary does; here its sections are driven as
/// the page drives them.)
fn claude_page(dir: &Path, env: &[(&str, &str)]) -> (Model, tempfile::TempDir) {
    let env = env
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let a: rush_core::adapter::AdapterRef =
        std::sync::Arc::new(rush_claude::adapter::Claude::new());
    let p = rush_core::adapter::Profile {
        kind: rush_core::adapter::Kind::new(K),
        name: String::new(),
        dir: dir.to_string_lossy().into_owned(),
    };
    TEST.with(|t| *t.borrow_mut() = Some((a, p, env)));
    let (mut m, d) = model(160, 70);
    set_settings_page(&mut m, PAGE_HARNESSES);
    ownsettings::want(&mut m, K);
    assert!(dialog(&m).own.is_some(), "read");
    (m, d)
}

/// A key on the row the cursor is on, as the page hands it over.
fn key(m: &mut Model, s: &str) -> Out {
    let secs = ownsettings::sections(m, K);
    form::form_key(m, &secs, s)
}

/// Puts the cursor on the row labelled label.
fn to_row(m: &mut Model, label: &str) {
    let rows = flat(&ownsettings::sections(m, K));
    let i = rows
        .iter()
        .position(|r| r.label == label)
        .unwrap_or_else(|| panic!("no row {label}"));
    dialog_mut(m).cursor = i;
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

const BODY: &str = r#"{"zeta":{"keep":[1,{"cmd":"a && b > c"}]},"hooks":{"Stop":[]},"autoCompactWindow":200000,"enabledPlugins":{"tool@market.place":false},"env":{"MY_OWN":"1"}}"#;

/// A folder whose settings.json is a link to another file, as each
/// account's home is to ~/.claude's.
fn linked_home() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    let home = d.path().join("home");
    fs::create_dir_all(&real).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(real.join("settings.json"), BODY).unwrap();
    symlink(real.join("settings.json"), home.join("settings.json")).unwrap();
    (d, real.join("settings.json"), home)
}

/// Each row shows its value and where it comes from: settings.json, the
/// default, or an environment variable, which wins.
#[test]
fn rows_say_where_values_come_from() {
    let (_d, _real, home) = linked_home();
    let (m, _g) = claude_page(&home, &[]);
    let b = body(&m, 150);
    assert!(b.contains("Claude Code's settings"), "{b}");
    let line = |name: &str| {
        b.lines()
            .find(|l| l.contains(name) && l.contains('‹'))
            .unwrap_or_else(|| panic!("no {name} row:\n{b}"))
            .to_string()
    };
    let ac = line("Auto-compact");
    assert!(
        ac.contains("‹ 200k") && ac.contains("settings.json"),
        "{ac}"
    );
    let model = line("Default model");
    assert!(model.contains("default"), "{model}");
    assert!(
        line("Co-authored-by").contains("‹ on") && line("Co-authored-by").contains("default"),
        "an unset flag says Claude Code's default"
    );
    assert!(line("tool@market.place").contains("‹ off"), "plugins");
    assert!(line("MY_OWN").contains('1') || b.contains("MY_OWN"), "{b}");

    let (m, _g) = claude_page(
        &home,
        &[
            ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "600000"),
            ("ANTHROPIC_MODEL", "sonnet"),
        ],
    );
    let b = body(&m, 150);
    let ac = b
        .lines()
        .find(|l| l.contains("Auto-compact") && l.contains('‹'))
        .unwrap();
    assert!(ac.contains("600k") && ac.contains("env"), "{ac}");
    let model = b
        .lines()
        .find(|l| l.contains("Default model") && l.contains('‹'))
        .unwrap();
    assert!(model.contains("sonnet") && model.contains("env"), "{model}");
}

/// enter on Auto-compact opens its choices; 400k typed sets it, written
/// through the link with every other key kept.
#[test]
fn auto_compact_typed_400k() {
    let (_d, real, home) = linked_home();
    let (mut m, _g) = claude_page(&home, &[]);
    to_row(&mut m, "Auto-compact");
    key(&mut m, "enter");
    assert!(m.picker().is_some(), "enter opens its choices");
    for k in ["4", "0", "0", "k"] {
        press(&mut m, k);
    }
    press(&mut m, "enter");
    let got = read_json(&real);
    let mut want: Value = serde_json::from_str(BODY).unwrap();
    want["autoCompactWindow"] = json!(400_000);
    assert_eq!(got, want, "only the window changed");
    assert!(
        fs::symlink_metadata(home.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is still a link"
    );
    let b = body(&m, 150);
    assert!(
        b.lines()
            .any(|l| l.contains("Auto-compact") && l.contains("400k")),
        "{b}"
    );
    // A window Claude Code wouldn't take is refused, and nothing written.
    to_row(&mut m, "Auto-compact");
    key(&mut m, "enter");
    for k in ["5", "0", "k"] {
        press(&mut m, k);
    }
    press(&mut m, "enter");
    assert_eq!(read_json(&real)["autoCompactWindow"], json!(400_000));
    // Off is a choice of its own.
    to_row(&mut m, "Auto-compact");
    key(&mut m, "enter");
    press(&mut m, "o");
    press(&mut m, "f");
    press(&mut m, "f");
    press(&mut m, "enter");
    assert_eq!(read_json(&real)["autoCompactEnabled"], json!(false));
}

/// ← isn't a row's to change; space
/// switches a plugin; the env block takes a variable added, all through
/// the link.
#[test]
fn left_goes_back_space_switches() {
    let (_d, real, home) = linked_home();
    let (mut m, _g) = claude_page(&home, &[]);
    to_row(&mut m, "Theme");
    assert_eq!(key(&mut m, "left"), Out::Idle, "← isn't the row's");
    assert_eq!(
        read_json(&real),
        serde_json::from_str::<Value>(BODY).unwrap()
    );

    to_row(&mut m, "tool@market.place");
    key(&mut m, "space");
    assert_eq!(
        read_json(&real)["enabledPlugins"]["tool@market.place"],
        json!(true)
    );
    to_row(&mut m, "+ add a variable");
    key(&mut m, "enter");
    for c in "FOO=bar".chars() {
        let k = if c == '=' {
            "=".to_string()
        } else {
            c.to_string()
        };
        press(&mut m, &k);
    }
    press(&mut m, "enter");
    let got = read_json(&real);
    assert_eq!(got["env"], json!({"MY_OWN": "1", "FOO": "bar"}));
    assert_eq!(got["zeta"]["keep"][1]["cmd"], json!("a && b > c"));
}

/// Providers, Harnesses and Config: ← goes back or a column left, never
/// changing a value; esc backs out too.
#[test]
fn arrows_on_the_agents_pages() {
    let (mut m, _d) = model(140, 50);
    // Harnesses › Every harness: ← from a row goes back to the list.
    set_settings_page(&mut m, PAGE_HARNESSES);
    press(&mut m, "right");
    assert!(dialog(&m).harness_inside);
    let before = config(&m).dispatch.every.effort.clone();
    press(&mut m, "left");
    assert!(!dialog(&m).harness_inside, "← went back");
    assert_eq!(
        config(&m).dispatch.every.effort,
        before,
        "and changed nothing"
    );
    press(&mut m, "left");
    assert_eq!(m.place, PlaceId::Harnesses, "← on the list stays");
    // Providers: → goes into one, ← back out.
    set_settings_page(&mut m, PAGE_PROVIDERS);
    if !providers::items(&m).is_empty() {
        press(&mut m, "right");
        assert!(dialog(&m).inside, "→ went in");
        press(&mut m, "left");
        assert!(!dialog(&m).inside, "← came out");
        assert_eq!(m.place, PlaceId::Harnesses);
    }
    // Config's At a usage limit row: ← changes nothing, enter picks.
    set_settings_page(&mut m, PAGE_CONFIG);
    let n = configpage::harnesses(&m).len();
    if n > 0 && !providers::items(&m).is_empty() {
        dialog_mut(&mut m).cursor = n;
        let was = config(&m).at_limit.clone();
        press(&mut m, "left");
        assert_eq!(config(&m).at_limit, was);
        press(&mut m, "enter");
        assert!(m.picker().is_some(), "enter opens its choices");
        press(&mut m, "esc");
        assert!(m.picker().is_none());
        assert_eq!(config(&m).at_limit, was, "esc keeps it");
    }
}

/// A harness that offers no settings of its own shows none.
#[test]
fn a_harness_without_settings_shows_none() {
    TEST.with(|t| *t.borrow_mut() = None);
    let (mut m, _d) = model(140, 50);
    set_settings_page(&mut m, PAGE_HARNESSES);
    ownsettings::want(&mut m, "no-such-harness");
    assert!(ownsettings::sections(&m, "no-such-harness").is_empty());
    assert!(dialog(&m).own.is_none());
}
