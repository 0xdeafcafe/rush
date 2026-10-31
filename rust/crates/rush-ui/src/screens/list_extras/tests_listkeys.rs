//! The list's keys on the agent and the list (ctrl+y, ctrl+t, ctrl+s,
//! ctrl+p), the Prompt's images and stash, and the preview kept while a
//! heading is picked: after Go's keys.go listKey, cycleGroupBy,
//! toggleSplit, togglePin, openPR, attachImages, stashCommand and
//! model.go's move.

use rush_core::adapter::{Checks, Pr};
use rush_core::agent::State;
use rush_core::text::Line;

use super::listkeys::{attach_images, pr_row, take_images};
use super::tests::{agent, model_of};
use crate::frame::{Out, Press};
use crate::model::Model;

/// Held by each test that saves config.json: the tests share one home.
fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn fleet() -> Model {
    let ask = agent("a1", "fix the flaky tax rounding test", State::Blocked, 101);
    let pay = agent("b2", "add Apple Pay to checkout", State::Working, 102);
    let idle = agent("d4", "set up caching for CI", State::Idle, 104);
    model_of(vec![ask, pay, idle], 120, 40)
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press_key(&Press::of(s).unwrap_or_else(|| panic!("no key {s:?}")))
}

fn flash(m: &Model) -> &str {
    m.flash.as_ref().map_or("", |f| f.0.as_str())
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

/// ctrl+s goes through status, agent and group and round again, saying
/// each; config.json keeps the last.
#[test]
fn ctrl_s_groups_by_the_next() {
    let _one = one_at_a_time();
    let mut m = fleet();
    m.cfg.group_by = "status".into();
    for want in ["agent", "group", "status", "agent"] {
        press(&mut m, "ctrl+s");
        assert_eq!(m.cfg.group_by, want);
        assert_eq!(flash(&m), format!("grouped by {want}"));
    }
    assert_eq!(rush_state::Store::load().config.group_by, "agent");
}

/// ctrl+p splits each section by project, or stops; the rail look starts
/// unsplit.
#[test]
fn ctrl_p_splits_by_project() {
    let _one = one_at_a_time();
    let mut m = fleet();
    m.cfg.split_by = String::new();
    press(&mut m, "ctrl+p");
    assert_eq!(flash(&m), "each section split by project");
    assert_eq!(m.cfg.split_by, "project");
    assert_eq!(rush_state::Store::load().config.split_by, "project");
    press(&mut m, "ctrl+p");
    assert_eq!(flash(&m), "sections no longer split by project");
    assert_eq!(m.cfg.split_by, "none");
}

/// ctrl+t on an agent whose program keeps no pins says so.
#[test]
fn ctrl_t_without_pins() {
    let mut m = fleet();
    m.sel = "default/b2".into();
    press(&mut m, "ctrl+t");
    assert_eq!(flash(&m), "add Apple Pay to checkout's agent keeps no pins");
}

fn pr(n: i64, title: &str, state: &str, checks: Checks) -> Pr {
    Pr {
        url: format!("https://github.com/acme/shop/pull/{n}"),
        title: title.into(),
        number: n,
        state: state.into(),
        review: String::new(),
        checks,
    }
}

/// ctrl+y with no pull request says so; with several, it asks which.
#[test]
fn ctrl_y_opens_the_pull_request() {
    let mut m = fleet();
    m.sel = "default/b2".into();
    press(&mut m, "ctrl+y");
    assert_eq!(flash(&m), "no pull request found for this agent");
    let i = m.selected().unwrap();
    m.agents[i].prs = vec![
        pr(12, "Apple Pay", "OPEN", Checks::default()),
        pr(9, "", "MERGED", Checks::default()),
    ];
    press(&mut m, "ctrl+y");
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("Pull requests · add Apple Pay to checkout"),
        "{f}"
    );
    assert!(f.contains("#12     Apple Pay"), "{f}");
    assert!(f.contains("#9      acme/shop/pull/9"), "{f}");
    assert!(f.contains("enter open in browser"), "{f}");
    press(&mut m, "esc");
    assert!(m.sheet.is_none());
}

/// A pull request's row: number, title fitted, state, checks.
#[test]
fn pull_request_rows() {
    let checks = Checks {
        passed: 3,
        failed: 1,
        pending: 2,
    };
    let r = pr_row(&pr(7, "Fix rounding", "OPEN", checks));
    let s = r.label.plain();
    assert!(s.starts_with("#7      Fix rounding "), "{s}");
    assert!(s.ends_with("  open      3✓ 1✗ 2…"), "{s}");
    assert_eq!(r.value, "https://github.com/acme/shop/pull/7");
}

/// An image goes into the Prompt as [Image #N], spaced off the words,
/// and leaves with the task that starts a session.
#[test]
fn prompt_images() {
    let mut m = fleet();
    m.input.text = "look at this".chars().collect();
    m.input.pos = m.input.text.len();
    attach_images(&mut m, &["/tmp/shot.png".into()]);
    assert_eq!(m.input.string(), "look at this [Image #1]");
    attach_images(&mut m, &["/tmp/two.png".into()]);
    let (text, imgs) = take_images(&mut m, "two [Image #2] one [Image #1]");
    assert_eq!(text, "two [Image #1] one [Image #2]");
    assert_eq!(imgs, ["/tmp/two.png", "/tmp/shot.png"]);
    let (text, imgs) = take_images(&mut m, "[Image #1]");
    assert_eq!((text.as_str(), imgs.len()), ("[Image #1]", 0));
}

/// With no plugin broker, ctrl+p with text typed, ctrl+r and #stash say
/// the stash isn't running.
#[test]
fn stash_not_running() {
    let mut m = fleet();
    m.input.text = "hello".chars().collect();
    m.input.pos = 5;
    let says = "Stash isn't running · Settings, Plugins has it, as drafts";
    press(&mut m, "ctrl+p");
    assert_eq!(flash(&m), says);
    assert_eq!(m.input.string(), "hello");
    m.flash = None;
    press(&mut m, "ctrl+r");
    assert_eq!(flash(&m), says);
    m.flash = None;
    m.input.clear();
    let _ = m.command("#stash");
    assert_eq!(flash(&m), says);
}

/// A heading picked keeps the preview on the agent left (model.go's
/// move and focused).
#[test]
fn heading_keeps_the_preview() {
    let mut m = fleet();
    let items = m.items();
    let first = items.iter().position(|k| !k.starts_with('§')).unwrap();
    let heading = items[first..].iter().position(|k| k.starts_with('§'));
    let Some(h) = heading.map(|h| h + first) else {
        panic!("no heading after an agent: {items:?}");
    };
    m.sel = items[h - 1].clone();
    press(&mut m, "down");
    assert_eq!(m.sel, items[h]);
    let f = m.focused().map(|i| m.agents[i].key.clone());
    assert_eq!(f.as_deref(), Some(items[h - 1].as_str()));
}

/// What the stash leaves on the Prompt sits on its bottom edge
/// (view.go:2722 boxNote("")); renaming, the box is another's and has
/// none.
#[test]
fn prompt_edge_has_plugin_notes() {
    use rush_plugin::{Status, UiStatus};
    let mut m = fleet();
    let note = UiStatus {
        plugin: "drafts".into(),
        status: Status {
            text: "stashed · back after you send".into(),
            ..Default::default()
        },
    };
    let st = rush_plugin::UiState {
        notes: std::collections::BTreeMap::from([(String::new(), vec![note])]),
        ..Default::default()
    };
    m.plugins().client = Some(rush_plugind::hooks::Client::fixed(st));
    let f = screen(&mut m);
    let edge = f.iter().rev().find(|l| l.starts_with('╰')).unwrap();
    assert!(
        edge.ends_with(" stashed · back after you send ─╯"),
        "{edge}"
    );
}
