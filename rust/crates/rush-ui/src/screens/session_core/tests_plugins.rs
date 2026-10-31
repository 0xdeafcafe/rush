//! Plugins in a Session: asked before a message goes (Go's
//! pluginui.go:493-558, intercept_test.go), told what was sent and
//! cleared for the stash's history (stash.go:173-210), their notes on the
//! box's edge (rushmode.go:3728) and their sections in the overview
//! (rushmode.go:2452), and their commands on keys (keybind.go:37, :55,
//! :151).

use std::collections::BTreeMap;

use rush_plugin::{
    CommandSpec, InterceptResult, Line as PluginLine, Section, Status, UiPlugin, UiSection,
    UiState, UiStatus,
};
use rush_plugind::hooks::Client;

use super::host::Op;
use super::tests::{KEY, flash, hosted, press, screen, typed};
use super::{conn, views};
use crate::model::Model;
use crate::screens::plugin_ui::{hooks, intercept};

fn drafts(called: &str) -> UiPlugin {
    UiPlugin {
        name: "drafts".into(),
        values: BTreeMap::from([("called".to_string(), called.to_string())]),
        ..Default::default()
    }
}

fn note(plugin: &str, text: &str) -> UiStatus {
    UiStatus {
        plugin: plugin.into(),
        status: Status {
            text: text.into(),
            ..Default::default()
        },
    }
}

fn with(m: &mut Model, st: UiState) {
    m.plugins().client = Some(Client::fixed(st));
}

fn send(text: &str) -> Op {
    Op::Send {
        text: text.into(),
        images: Vec::new(),
        now: false,
    }
}

fn answer(action: &str, text: &str, reason: &str) -> InterceptResult {
    InterceptResult {
        action: action.into(),
        text: text.into(),
        reason: reason.into(),
        plugin: "guard".into(),
    }
}

/// What's in a box goes past plugins only as a message: not a # or /
/// command, nor a line carried on with a trailing backslash
/// (wantsIntercept).
#[test]
fn what_goes_past_plugins() {
    for (raw, now, want) in [
        ("fix the bug", false, true),
        ("  fix it  ", false, true),
        ("", false, false),
        ("   ", false, false),
        ("#stash", false, false),
        ("# Plan", false, true),
        ("#123 is broken", false, true),
        ("/compact", false, false),
        ("carry on\\", false, false),
        ("carry on\\", true, true),
    ] {
        assert_eq!(intercept::message(raw, now), want, "{raw:?} now={now}");
    }
    assert_eq!(intercept::clip_log("short"), "short");
    let long = "é".repeat(301);
    assert_eq!(intercept::clip_log(&long), format!("{}…", "é".repeat(300)));
}

/// A plugin's rewrite goes in the box's place and is sent; a block holds
/// it with the plugin's why; a box changed meanwhile isn't sent
/// (onIntercepted).
#[test]
fn intercept_answers() {
    let (mut m, fake) = hosted("steer", Vec::new());
    typed(&mut m, "push to main");
    let o = intercept::answered(
        &mut m,
        KEY,
        "push to main",
        false,
        answer("block", "", "pushes\x1b[31m wait for CI"),
    );
    assert!(o.used());
    assert!(fake.ops().is_empty());
    assert_eq!(conn(&mut m, KEY).input.string(), "push to main");
    let (said, _, bad) = m.flash.clone().unwrap();
    assert!(!bad, "a refusal, as Go's flash(…, true)");
    assert_eq!(said, "guard: pushes[31m wait for CI");

    // Held back with no why.
    intercept::answered(&mut m, KEY, "push to main", false, answer("block", "", ""));
    assert_eq!(flash(&m), "guard: held back");

    // The box changed while plugins looked.
    intercept::answered(&mut m, KEY, "push", false, answer("allow", "", ""));
    assert_eq!(
        flash(&m),
        "the message changed while plugins looked at it: send it again"
    );
    assert!(fake.ops().is_empty());

    // Rewritten, then sent: the box empties.
    intercept::answered(
        &mut m,
        KEY,
        "push to main",
        false,
        answer("rewrite", "open a PR instead", "no pushes to main"),
    );
    assert_eq!(fake.ops(), [send("open a PR instead")]);
    assert_eq!(conn(&mut m, KEY).input.string(), "");
    assert!(!conn(&mut m, KEY).intercepted);
    assert!(!conn(&mut m, KEY).intercepting);

    // Allowed: sent as it was.
    typed(&mut m, "go on");
    intercept::answered(&mut m, KEY, "go on", false, answer("allow", "", ""));
    assert_eq!(fake.ops().last(), Some(&send("go on")));
}

/// A Session left while plugins looked keeps nothing: the answer is
/// dropped.
#[test]
fn intercept_after_leaving() {
    let (mut m, fake) = hosted("steer", Vec::new());
    typed(&mut m, "hello");
    let o = intercept::answered(
        &mut m,
        "default/other",
        "hello",
        false,
        answer("allow", "", ""),
    );
    assert_eq!(o, crate::frame::Out::Idle);
    assert!(fake.ops().is_empty());
}

/// While plugins look at it, enter sends nothing and the box stays.
#[test]
fn intercepting_holds_the_box() {
    let (mut m, fake) = hosted("steer", Vec::new());
    typed(&mut m, "hello");
    conn(&mut m, KEY).intercepting = true;
    press(&mut m, "enter");
    assert!(fake.ops().is_empty());
    assert_eq!(conn(&mut m, KEY).input.string(), "hello");
    conn(&mut m, KEY).intercepting = false;
    press(&mut m, "enter");
    assert_eq!(fake.ops(), [send("hello")]);
}

/// With the stash running, what was sent and cleared is in its history,
/// and the flash says where; a box with a stash waiting says it's back
/// (keepSent, wipeBox, wentTo).
#[test]
fn sent_and_cleared_say_where_they_went() {
    let (mut m, fake) = hosted("steer", Vec::new());
    typed(&mut m, "one");
    press(&mut m, "enter");
    assert_eq!(flash(&m), "sent");
    with(
        &mut m,
        UiState {
            plugins: vec![drafts("Drafts")],
            ..Default::default()
        },
    );
    typed(&mut m, "two");
    press(&mut m, "enter");
    assert_eq!(flash(&m), "sent · #stash › Sent has it");
    typed(&mut m, "three");
    press(&mut m, "esc");
    assert_eq!(
        flash(&m),
        "cleared · cmd+z or ctrl+/ brings it back · #stash › Cleared has it"
    );
    with(
        &mut m,
        UiState {
            plugins: vec![drafts("Drafts")],
            notes: BTreeMap::from([(KEY.to_string(), vec![note("drafts", "a draft waits")])]),
            ..Default::default()
        },
    );
    typed(&mut m, "four");
    press(&mut m, "enter");
    assert_eq!(
        flash(&m),
        "sent · your draft is back in the box · #stash › Sent has it"
    );
    assert_eq!(fake.ops(), [send("one"), send("two"), send("four")]);
}

/// A plugin's note sits on the box's bottom edge, at its right
/// (rushmode.go:3728 boxNote).
#[test]
fn note_on_the_box() {
    let (mut m, _) = hosted("steer", Vec::new());
    with(
        &mut m,
        UiState {
            notes: BTreeMap::from([(
                KEY.to_string(),
                vec![note("drafts", "stashed · alt+s brings it back")],
            )]),
            ..Default::default()
        },
    );
    let f = screen(&mut m);
    let row = f
        .iter()
        .find(|r| r.trim_start().starts_with('╰'))
        .unwrap_or_else(|| panic!("no box\n{f:#?}"));
    assert!(row.contains("stashed · alt+s brings it back ─╯"), "{row}");
}

/// Plugins' sections follow the session's own in the overview, titled
/// with who added them (pluginOverview).
#[test]
fn sections_in_the_overview() {
    let (mut m, _) = hosted("steer", Vec::new());
    with(
        &mut m,
        UiState {
            sections: BTreeMap::from([(
                KEY.to_string(),
                vec![UiSection {
                    plugin: "ci".into(),
                    section: Section {
                        title: "Checks".into(),
                        lines: vec![
                            PluginLine {
                                text: "build passed".into(),
                                tone: "good".into(),
                                ..Default::default()
                            },
                            PluginLine {
                                text: "lint failed".into(),
                                tone: "bad".into(),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    },
                }],
            )]),
            ..Default::default()
        },
    );
    assert!(views::show_view(&mut m, KEY, "overview"));
    let rows: Vec<String> = super::overview::lines(&m, KEY, 120)
        .into_iter()
        .map(|(l, _)| l.plain())
        .collect();
    let at = rows
        .iter()
        .position(|r| r == "  Checks  · ci")
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert_eq!(rows[at - 1], "");
    assert_eq!(rows[at + 1], "    build passed");
    assert_eq!(rows[at + 2], "    lint failed");
}

fn haven() -> UiState {
    UiState {
        plugins: vec![UiPlugin {
            name: "haven".into(),
            commands: vec![CommandSpec {
                name: "open".into(),
                description: "open the haven".into(),
                key: "alt+j".into(),
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Plugins' commands are on Settings › Keys, everywhere, with the key
/// they suggest, and come in once plugins do (keyActions, setKeys,
/// onHooks).
#[test]
fn plugin_commands_on_keys() {
    let mut m = Model::new("test");
    crate::screens::settings::keys::set_keys(&mut m, Default::default());
    let km = crate::screens::settings::keys::key_map(&m).into_owned();
    assert!(km.action("plugin:haven.open").is_none());
    with(&mut m, haven());
    hooks::rebuild_keys(&mut m);
    let km = crate::screens::settings::keys::key_map(&m).into_owned();
    let a = km.action("plugin:haven.open").expect("listed");
    assert_eq!(a.title, "open the haven");
    assert_eq!(a.source, "haven");
    assert_eq!(a.context, rush_keymap::Context::Any);
    assert_eq!(km.key_text("plugin:haven.open"), "alt+j");
}

/// A plugin's suggested key runs its command, on the agent in view
/// (runAction's plugin:).
#[test]
fn plugin_key_runs_its_command() {
    let (mut m, _) = hosted("steer", Vec::new());
    with(&mut m, haven());
    hooks::rebuild_keys(&mut m);
    press(&mut m, "alt+j");
    let (said, _, bad) = m.flash.clone().unwrap();
    // No broker in a test: the command says so.
    assert!(said.starts_with("haven open: ") && bad, "{said}");
}

/// esc in the Agents box clears it, kept to undo and in the stash's
/// history (wipePrompt).
#[test]
fn prompt_wipe() {
    let mut m = Model::new("test");
    with(
        &mut m,
        UiState {
            plugins: vec![drafts("")],
            ..Default::default()
        },
    );
    m.input.set("a task");
    m.wipe_prompt();
    assert!(m.input.is_empty());
    assert_eq!(
        flash(&m),
        "cleared · cmd+z or ctrl+/ brings it back · #stash › Cleared has it"
    );
}

/// A plugin's note for the Prompt sits on its bottom edge (view.go:2722).
#[test]
fn note_on_the_prompt() {
    let mut m = Model::new("test");
    m.w = 120;
    m.h = 30;
    with(
        &mut m,
        UiState {
            notes: BTreeMap::from([(String::new(), vec![note("drafts", "a draft waits")])]),
            ..Default::default()
        },
    );
    let f = screen(&mut m);
    let row = f
        .iter()
        .find(|r| r.trim_start().starts_with('╰'))
        .unwrap_or_else(|| panic!("no box\n{f:#?}"));
    assert!(row.ends_with("a draft waits ─╯"), "{row}");
}
