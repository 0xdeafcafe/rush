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
use crate::screens::plugin_ui::ModelPlugins;
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
        assert_eq!(
            intercept::message(crate::screens::cmdbar::fleetslash::is_hash_cmd, raw, now),
            want,
            "{raw:?} now={now}"
        );
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
    assert_eq!(flash(&m), "sent · ctrl+r lists what you've sent");
    typed(&mut m, "three");
    press(&mut m, "esc");
    assert_eq!(flash(&m), "cleared · cmd+z or ctrl+/ brings it back");
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
        "sent · what you kept as a draft is back in the box · ctrl+r lists what you've sent"
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
                key: "alt+x".into(),
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
    let mut m = crate::new_model("test");
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
    assert_eq!(km.key_text("plugin:haven.open"), "alt+x");
}

/// A plugin's suggested key runs its command, on the agent in view
/// (runAction's plugin:).
#[test]
fn plugin_key_runs_its_command() {
    let (mut m, _) = hosted("steer", Vec::new());
    with(&mut m, haven());
    hooks::rebuild_keys(&mut m);
    press(&mut m, "alt+x");
    let (said, _, bad) = m.flash.clone().unwrap();
    // No broker in a test: the command says so.
    assert!(said.starts_with("haven open: ") && bad, "{said}");
}

/// esc in the Agents box clears it, kept to undo and in the stash's
/// history (wipePrompt).
#[test]
fn prompt_wipe() {
    let mut m = crate::new_model("test");
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
    assert_eq!(flash(&m), "cleared · cmd+z or ctrl+/ brings it back");
}

/// A plugin's note for the Prompt sits on its bottom edge (view.go:2722).
#[test]
fn note_on_the_prompt() {
    let mut m = crate::new_model("test");
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

/// ctrl+p and ctrl+r in the Session's box run the stash's commands on
/// that box, or say the stash isn't running; the stash sees the box and
/// sets it (rushmode.go:4337-4340 stashCommand, pluginui.go:414 boxState,
/// :425 setBox).
#[test]
fn the_stash_from_the_session() {
    let (mut m, _) = hosted("steer", Vec::new());
    typed(&mut m, "hello draft");
    for k in ["ctrl+p", "ctrl+r"] {
        m.flash = None;
        press(&mut m, k);
        assert_eq!(
            flash(&m),
            "Stash isn't running · Settings, Plugins has it, as drafts",
            "{k}"
        );
    }
    with(
        &mut m,
        UiState {
            plugins: vec![drafts("Drafts")],
            ..Default::default()
        },
    );
    m.flash = None;
    press(&mut m, "ctrl+p");
    let said = flash(&m);
    // No broker in a test: the command was asked of it, and says so.
    assert!(said.starts_with("Drafts: "), "{said}");
    let (b, who) = hooks::box_state(&m).expect("the Session's box");
    assert_eq!(
        (b.text.as_str(), b.cursor, who.as_str()),
        ("hello draft", 11, KEY)
    );
    hooks::plugin_do(
        &mut m,
        rush_plugin::UiDo {
            kind: "input.set".into(),
            session: KEY.into(),
            text: String::new(),
            ..Default::default()
        },
    );
    assert!(conn(&mut m, KEY).input.is_empty(), "set aside");
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("a message for this agent · ctrl+r history"),
        "{f}"
    );
    press(&mut m, "ctrl+/");
    assert_eq!(
        conn(&mut m, KEY).input.string(),
        "hello draft",
        "kept to undo"
    );
}

/// A turn under way in the hosted session: you said text and it works.
fn at_work(m: &mut Model) {
    let now = m.now;
    super::tests::sess(m).sent("go".into(), Vec::new(), now);
}

/// Every way text leaves a Session's box says where it went and the one
/// key that gets it back, in the box's words (keepSent, wipeBox).
#[test]
fn each_way_out_of_the_box_says_where() {
    // (what, working, ctrl+t presses first, key, the flash)
    let cases: &[(&str, bool, usize, &str, &str)] = &[
        (
            "sent",
            false,
            0,
            "enter",
            "sent · ctrl+r lists what you've sent",
        ),
        (
            "queued while it works",
            true,
            0,
            "enter",
            "queued for when it's done · ↑ picks it to change",
        ),
        (
            "guided while it works",
            true,
            1,
            "enter",
            "sent into the turn · it reads it at its next step · ctrl+r lists what you've sent",
        ),
        (
            "stopped and sent",
            true,
            2,
            "enter",
            "sent · ctrl+r lists what you've sent",
        ),
        (
            "sent now while it works",
            true,
            0,
            "ctrl+enter",
            "sent · ctrl+r lists what you've sent",
        ),
        (
            "cleared with esc",
            false,
            0,
            "esc",
            "cleared · cmd+z or ctrl+/ brings it back",
        ),
        (
            "cleared with ctrl+c",
            true,
            0,
            "ctrl+c",
            "cleared · cmd+z or ctrl+/ brings it back",
        ),
    ];
    for (what, work, modes, key, want) in cases {
        let (mut m, _) = hosted("steer", Vec::new());
        with(
            &mut m,
            UiState {
                plugins: vec![drafts("")],
                ..Default::default()
            },
        );
        if *work {
            at_work(&mut m);
        }
        for _ in 0..*modes {
            press(&mut m, "ctrl+t");
        }
        typed(&mut m, "a message");
        press(&mut m, key);
        assert_eq!(
            conn(&mut m, KEY).input.string(),
            "",
            "{what}: the box empties"
        );
        assert_eq!(flash(&m), *want, "{what}");
    }
}

/// The row under a Session's box, state by state: what enter does, the
/// key or two that matter now, f1, and what esc does now.
#[test]
fn the_session_box_hint_row() {
    // (what, stash on, working, typed, the row has, the row hasn't)
    let cases: &[(&str, bool, bool, &str, &[&str], &[&str])] = &[
        (
            "typed, idle, stash on",
            true,
            false,
            "draft",
            &[
                "enter send  ·  ctrl+p set aside  ·  ctrl+r history",
                "f1 more",
                "esc clear",
            ],
            &["esc back"],
        ),
        (
            "typed, idle, no stash",
            false,
            false,
            "draft",
            &["enter send", "f1 more", "esc clear"],
            &["ctrl+p", "ctrl+r"],
        ),
        (
            "empty, idle, stash on",
            true,
            false,
            "",
            &["ctrl+r history", "f1 more", "esc back"],
            &["enter"],
        ),
        (
            "typed, working",
            true,
            true,
            "draft",
            &[
                "enter queue it  ·  ctrl+enter send now  ·  ctrl+t what enter does while it works",
                "esc clear",
            ],
            &[],
        ),
        (
            "empty, working",
            true,
            true,
            "",
            &["esc stop the turn"],
            &["ctrl+x", "esc back"],
        ),
    ];
    for (what, on, work, text, has, hasnt) in cases {
        let (mut m, _) = hosted("steer", Vec::new());
        if *on {
            with(
                &mut m,
                UiState {
                    plugins: vec![drafts("")],
                    ..Default::default()
                },
            );
        }
        if *work {
            at_work(&mut m);
        }
        typed(&mut m, text);
        let f = screen(&mut m);
        let row = f.last().unwrap();
        for h in *has {
            assert!(row.contains(h), "{what}: {h:?} in {row:?}");
        }
        for h in *hasnt {
            assert!(!row.contains(h), "{what}: no {h:?} in {row:?}");
        }
    }
}

/// The Agents box takes the same words: f1 for the keys (? is a letter
/// once something's typed), ctrl+p sets aside, undo brings back what
/// was cleared, and esc lets go of a selection before it clears.
#[test]
fn the_agents_box_hint_row() {
    let mut m = crate::new_model("test");
    m.w = 120;
    m.h = 30;
    m.now = 1_000_000;
    with(
        &mut m,
        UiState {
            plugins: vec![drafts("")],
            ..Default::default()
        },
    );
    typed(&mut m, "fix it");
    let last = |m: &mut Model| screen(m).last().unwrap().trim_end().to_string();
    let row = last(&mut m);
    assert!(
        row.starts_with("  enter start  ·  shift+tab model  ·  ctrl+p set aside"),
        "{row:?}"
    );
    assert!(row.ends_with("esc clear  ·  f1 more"), "{row:?}");
    // A selection: esc lets go of it, then clears.
    press(&mut m, "shift+left");
    assert!(m.input.selection().is_some());
    press(&mut m, "esc");
    assert_eq!(m.input.string(), "fix it", "the first esc keeps the text");
    assert!(m.input.selection().is_none());
    press(&mut m, "esc");
    assert!(m.input.is_empty());
    assert_eq!(flash(&m), "cleared · cmd+z or ctrl+/ brings it back");
    m.flash = None;
    let row = last(&mut m);
    assert!(
        row.starts_with("  cmd+z or ctrl+/ bring back what you cleared"),
        "{row:?}"
    );
    assert!(row.ends_with("f1 more"), "{row:?}");
    // Two minutes on, the list's own keys again.
    m.kept::<hooks::PromptCleared>().0 -= 120_000;
    assert!(!last(&mut m).contains("bring back"));
}
