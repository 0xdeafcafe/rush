//! Renaming an agent in the Agents box, after Go's keys.go startRename,
//! renameStep and submit's inRename, and view.go's labels for it.

use rush_core::agent::State;
use rush_core::text::Line;

use super::rename::renaming;
use super::tests::{agent, model_of};
use crate::frame::{Out, Press};
use crate::model::Model;

fn fleet() -> Model {
    let ask = agent("a1", "fix the flaky tax rounding test", State::Blocked, 101);
    let pay = agent("b2", "add Apple Pay to checkout", State::Working, 102);
    let idle = agent("d4", "set up caching for CI", State::Idle, 104);
    model_of(vec![ask, pay, idle], 120, 40)
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press_key(&Press::of(s).unwrap_or_else(|| panic!("no key {s:?}")))
}

fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        let k = if c == ' ' {
            "space".to_string()
        } else {
            c.to_string()
        };
        press(m, &k);
    }
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

fn name(m: &Model, key: &str) -> String {
    m.agents
        .iter()
        .find(|a| a.key == key)
        .map(|a| a.display_name.clone())
        .unwrap_or_default()
}

/// ctrl+r puts the name in the box, all of it selected, the box's edge
/// and keys saying so; typing replaces it, and tab saves it and goes on
/// to the next agent, past the heading between.
#[test]
fn ctrl_r_renames_and_tab_goes_on() {
    let mut m = fleet();
    m.sel = "default/b2".into();
    press(&mut m, "ctrl+r");
    assert_eq!(renaming(&m), Some("default/b2"));
    assert_eq!(m.input.string(), "add Apple Pay to checkout");
    assert_eq!(m.input.selection(), Some((0, 25)));
    let f = screen(&mut m);
    let n = f.len();
    assert!(
        f[n - 4].starts_with("╭─ rename add Apple Pay to checkout · enter saves ─"),
        "{:?}",
        f[n - 4]
    );
    assert_eq!(
        f[n - 1].trim_end(),
        "  enter save  ·  tab · ↑↓ save, rename the next  ·  esc cancel"
    );
    typed(&mut m, "pay");
    assert_eq!(m.input.string(), "pay");
    press(&mut m, "tab");
    assert_eq!(name(&m, "default/b2"), "pay");
    assert_eq!(renaming(&m), Some("default/d4"));
    assert_eq!(m.sel, "default/d4");
    assert_eq!(m.input.string(), "set up caching for CI");
    // Kept in rush's state, for the list's next reading.
    let s = rush_state::Store::load();
    assert_eq!(
        s.overlay.names.get("default/b2").map(String::as_str),
        Some("pay")
    );
    // esc leaves the name as it was, and the box starts agents again.
    press(&mut m, "esc");
    assert_eq!(renaming(&m), None);
    assert!(m.input.is_empty());
    assert_eq!(name(&m, "default/d4"), "set up caching for CI");
    let f = screen(&mut m);
    assert!(
        f[f.len() - 4].starts_with("╭─ new agent in checkout ─"),
        "{:?}",
        f[f.len() - 4]
    );
}

/// enter saves; past the last agent, the rename is done. Empty puts the
/// agent's own name back.
#[test]
fn enter_saves_and_empty_resets() {
    let mut m = fleet();
    m.agents.iter_mut().for_each(|a| a.job.name = "own".into());
    m.sel = "default/d4".into();
    press(&mut m, "ctrl+r");
    typed(&mut m, "caching");
    press(&mut m, "enter");
    assert_eq!(name(&m, "default/d4"), "caching");
    assert_eq!(renaming(&m), None);
    assert!(m.input.is_empty());
    press(&mut m, "ctrl+r");
    press(&mut m, "backspace");
    press(&mut m, "down");
    assert_eq!(name(&m, "default/d4"), "own");
    assert_eq!(renaming(&m), None, "nothing past the last");
    assert!(
        !rush_state::Store::load()
            .overlay
            .names
            .contains_key("default/d4")
    );
}

/// ctrl+r needs an agent picked and an empty box (keys.go:405; with
/// something typed, it's the box's history, prompt.history, first).
#[test]
fn ctrl_r_needs_an_agent_and_an_empty_box() {
    let mut m = fleet();
    m.sel = "default/b2".into();
    typed(&mut m, "fix it");
    press(&mut m, "ctrl+r");
    assert_eq!(renaming(&m), None);
    m.input.clear();
    m.sel = "§Working".into();
    press(&mut m, "ctrl+r");
    assert_eq!(
        m.flash.as_ref().map(|f| f.0.as_str()),
        Some("select an agent first")
    );
}

/// #rename with a name renames the agent picked at once; alone, it puts
/// the name in the box.
#[test]
fn hash_rename() {
    let mut m = fleet();
    m.sel = "default/a1".into();
    typed(&mut m, "#rename rounding");
    press(&mut m, "enter");
    assert_eq!(name(&m, "default/a1"), "rounding");
    assert_eq!(renaming(&m), None);
    typed(&mut m, "#rename");
    press(&mut m, "enter");
    assert_eq!(renaming(&m), Some("default/a1"));
    assert_eq!(m.input.string(), "rounding");
}

/// enter on an agent asks what it should do the first time, then does
/// what you chose: rename it, or open it (keys.go:388-403).
#[test]
fn enter_does_what_you_chose() {
    let mut m = fleet();
    m.sel = "default/b2".into();
    crate::screens::settings::use_store(&mut m, rush_state::Store::default());
    press(&mut m, "enter");
    assert!(
        m.sheet_as::<crate::screens::session_cards::enter::EnterSheet>()
            .is_some(),
        "asked"
    );
    m.sheet = None;
    let mut s = rush_state::Store::default();
    s.config.enter_on = "rename".into();
    crate::screens::settings::use_store(&mut m, s);
    press(&mut m, "enter");
    assert_eq!(renaming(&m), Some("default/b2"));
    press(&mut m, "esc");
    let mut s = rush_state::Store::default();
    s.config.enter_on = "open".into();
    crate::screens::settings::use_store(&mut m, s);
    press(&mut m, "enter");
    assert_eq!(renaming(&m), None);
    assert!(m.sheet.is_none());
}
