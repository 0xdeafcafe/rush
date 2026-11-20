//! The tests of rush-ui-projects-machine's kititems.rs that need the
//! whole UI: the kits' cleanup.report items, asked and cleaned into Data.
//! Nothing asks the broker: the answers and cleans are the tests'.

use rush_plugin::{Answer, CleanupDone, CleanupItem, CleanupReport};
use rush_ui_projects_machine::data::*;
use rush_ui_projects_machine::kititems::*;

fn item(id: &str, worktree: &str) -> CleanupItem {
    CleanupItem {
        id: id.into(),
        title: id.into(),
        bytes: 1 << 20,
        worktree: worktree.into(),
        can_go: true,
        comes_back: true,
        does: "deletes it".into(),
        ..Default::default()
    }
}

fn answer(plugin: &str, r: Option<CleanupReport>, error: &str) -> Answer {
    Answer {
        plugin: plugin.into(),
        result: r.map(|r| rush_plugin::rpc::to_raw(&r).unwrap()),
        error: error.into(),
    }
}

fn answers() -> Vec<Answer> {
    vec![
        answer(
            "rust-clean",
            Some(CleanupReport {
                title: "rust-clean".into(),
                report: "- target in wt".into(),
                items: vec![item("target:/w/wt", "/w/wt"), item("crates", "")],
                ..Default::default()
            }),
            "",
        ),
        // An old report, text only: the cleanup agent's, no rows.
        answer(
            "haven",
            Some(CleanupReport {
                title: "Haven".into(),
                report: "3 stacks idle".into(),
                ..Default::default()
            }),
            "",
        ),
        answer("go-clean", None, "timed out"),
        answer(
            "git-clean",
            Some(CleanupReport {
                items: vec![item("gc:/w", "")],
                ..Default::default()
            }),
            "",
        ),
    ]
}

// Only reports with items become rows, by plugin; one with no title is
// named for its plugin.
#[test]
fn items_of_answers() {
    let got = kit_items_of(answers());
    assert_eq!(
        got,
        vec![
            KitItems {
                plugin: "git-clean".into(),
                title: "git-clean".into(),
                items: vec![item("gc:/w", "")],
            },
            KitItems {
                plugin: "rust-clean".into(),
                title: "rust-clean".into(),
                items: vec![item("target:/w/wt", "/w/wt"), item("crates", "")],
            },
        ]
    );
}

// Asked, they land in Data's kits: what's in a worktree under it, the
// rest the machine's. A clean lands what it did, and drops the row; one
// refused keeps it and says why.
#[test]
fn asked_and_cleaned_into_data() {
    let mut m = crate::new_model("0.1.0");
    ask_kits_with(&mut m, answers);
    let k = &data(&m).kits;
    assert!(!k.asking && k.asked != 0 && k.found.len() == 2);
    let wt: Vec<&str> = k.in_worktree("/w/wt").map(|(_, i)| i.id.as_str()).collect();
    assert_eq!(wt, ["target:/w/wt"]);
    let mac: Vec<(&str, &str)> = k.machine().map(|(p, i)| (p, i.id.as_str())).collect();
    assert_eq!(mac, [("git-clean", "gc:/w"), ("rust-clean", "crates")]);

    clean_kit_item_with(&mut m, KitRef::new("rust-clean", "nope"), |r| {
        Err(format!("no such item: {}; measure again and look", r.id))
    });
    clean_kit_item_with(&mut m, KitRef::new("rust-clean", "target:/w/wt"), |_| {
        Ok(CleanupDone {
            said: "target in wt: freed 1 MB.".into(),
            freed: 1 << 20,
        })
    });
    let k = &data(&m).kits;
    assert!(k.cleaning.is_empty());
    assert_eq!(
        k.cleaned[&KitRef::new("rust-clean", "nope")],
        Err("no such item: nope; measure again and look".to_string())
    );
    assert_eq!(
        k.cleaned[&KitRef::new("rust-clean", "target:/w/wt")],
        Ok(CleanupDone {
            said: "target in wt: freed 1 MB.".into(),
            freed: 1 << 20,
        })
    );
    assert_eq!(k.in_worktree("/w/wt").count(), 0, "cleaned, it's gone");
    assert_eq!(k.all().count(), 2);
}
