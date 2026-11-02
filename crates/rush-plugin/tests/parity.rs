//! What Rust rush writes is what Go rush writes, byte for byte, so the two
//! share approvals, origins and the broker's wire. fixtures/go_marshal.txt
//! is Go's jsonx output for fixtures/manifest_in.json read as a Manifest,
//! then an Approval of it, origins, a UI event, a UI state, a pick and a
//! CLI run, each written as Go rush writes it.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::collections::BTreeMap;

use rush_plugin::origin::Origin;
use rush_plugin::{
    Approval, CliRun, Line, Manifest, Pick, PickAction, PickItem, Section, Status, TextBox, UiDo,
    UiEvent, UiPlugin, UiSection, UiSession, UiState, UiStatus,
};
use rush_state::Time;
use rush_state::json::{to_vec, to_vec_indent};

fn s(b: Vec<u8>) -> String {
    String::from_utf8(b).unwrap()
}

#[test]
fn writes_what_go_writes() {
    let want = include_str!("fixtures/go_marshal.txt");
    let m = Manifest::parse(include_bytes!("fixtures/manifest_in.json"), true).unwrap();
    let at = Time::parse("2026-10-29T10:00:00.5+01:00").unwrap();

    let ap = BTreeMap::from([(
        "p".to_string(),
        Approval {
            digest: "d".into(),
            manifest: m,
            approved_at: at,
        },
    )]);
    let o = BTreeMap::from([
        (
            "p".to_string(),
            Origin {
                project: "/r".into(),
                commit: "c".into(),
                installed_at: at,
                sources: vec!["a".into()],
                dev: true,
                ..Default::default()
            },
        ),
        ("q".to_string(), Origin::default()),
    ]);
    let ev = UiEvent {
        kind: "input.changed".into(),
        ui: "w".into(),
        at,
        session: Some(UiSession {
            id: "s".into(),
            hosted: true,
            ..Default::default()
        }),
        input_box: Some(TextBox {
            text: "hi".into(),
            cursor: 2,
            pastes: BTreeMap::from([(1, "x".into()), (10, "y".into()), (2, "z".into())]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let st = UiState {
        plugins: vec![UiPlugin {
            name: "a".into(),
            ..Default::default()
        }],
        sections: BTreeMap::from([(
            "s".to_string(),
            vec![UiSection {
                plugin: "a".into(),
                section: Section {
                    id: "i".into(),
                    title: "T".into(),
                    lines: vec![Line {
                        text: "l".into(),
                        tone: "dim".into(),
                        ..Default::default()
                    }],
                },
            }],
        )]),
        notes: BTreeMap::from([(
            String::new(),
            vec![UiStatus {
                plugin: "a".into(),
                status: Status {
                    text: "n".into(),
                    ..Default::default()
                },
            }],
        )]),
        ..Default::default()
    };
    let do_ = UiDo {
        plugin: "a".into(),
        kind: "pick".into(),
        pick: Some(Pick {
            id: "p".into(),
            title: "T".into(),
            items: vec![PickItem {
                id: "1".into(),
                text: "x".into(),
                ..Default::default()
            }],
            actions: vec![PickAction {
                key: "enter".into(),
                name: "go".into(),
                stay: false,
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let cli = CliRun {
        command: "c".into(),
        ..Default::default()
    };
    let got = [
        s(to_vec_indent(&ap).unwrap()),
        s(to_vec_indent(&o).unwrap()),
        s(to_vec(&ev).unwrap()),
        s(to_vec(&st).unwrap()),
        s(to_vec(&do_).unwrap()),
        s(to_vec(&cli).unwrap()),
    ]
    .join("\n---\n");
    let (got_parts, want_parts): (Vec<&str>, Vec<&str>) = (
        got.split("\n---\n").collect(),
        want.split("\n---\n").collect(),
    );
    for (g, w) in got_parts.iter().zip(&want_parts) {
        assert_eq!(g, w);
    }
    assert_eq!(got, want);

    // And Go's file reads back to what writes it again.
    let back: BTreeMap<String, Approval> =
        rush_state::json::from_slice(want_parts[0].as_bytes()).unwrap();
    assert_eq!(s(to_vec_indent(&back).unwrap()), want_parts[0]);
}
