//! The cleanup agent's research mode (rush-ui-projects-machine's
//! research.rs): what it's told, and what it brought said.

use rush_fleet::disk::{
    Candidate, KNOWN_TOOLS, PROPOSALS_FILE, Proposed, RulesFile, add_proposals, merge_built_in,
    parse_proposals,
};
use rush_plugin::CleanupReport;
use rush_ui_projects_machine::research::*;

const G: i64 = 1 << 30;
const DAY: i64 = 24 * 3_600_000;

fn found(rules: Result<RulesFile, String>) -> Found {
    Found {
        survey: None,
        map: Some(rush_fleet::HomeMap {
            at: 100 * DAY - 3_600_000,
            tops: vec![("/Users/me/Library".into(), 80 * G)],
            untouched: vec![rush_fleet::Untouched {
                path: "/Users/me/.zed/old".into(),
                size: 3 * G,
                newest: 60 * DAY,
                kind: "logs".into(),
            }],
        }),
        candidates: vec![
            Candidate {
                path: "/Users/me/w/node_modules".into(),
                kind: "js.node-modules".into(),
                why: "w's packages".into(),
                plugin: "js-clean".into(),
                bytes_on_disk: 2 << 30,
                ..Candidate::default()
            },
            Candidate {
                path: "/Users/me/Library/pnpm/store".into(),
                kind: "js.pnpm-store".into(),
                plugin: "js-clean".into(),
                bytes_on_disk: 14 << 30,
                ..Candidate::default()
            },
        ],
        reports: Vec::new(),
        rules,
    }
}

// The agent is told it removes nothing, how a rule is written and where,
// which tools a command may run, and that pnpm's store is pruned; its
// example is one rush takes.
#[test]
fn research_prompt_proposes_and_removes_nothing() {
    let r = CleanupReport {
        title: "Haven".into(),
        report: "3 stacks idle".into(),
        rules: String::new(),
    };
    let p = research_prompt(&["Disk: 9G free of 500G.".into()], &[r]);
    for want in [
        "Research what fills this disk and propose rules for rush's disk engine",
        "Delete, remove, stop, prune and run nothing that changes the disk",
        "No rule you propose runs until the user accepts it.",
        &format!("$TMPDIR/{PROPOSALS_FILE}"),
        &KNOWN_TOOLS.join(", "),
        "pnpm's store is always this, pnpm store prune, never a folder removed whole",
        "Never js.pnpm-store",
        "what made them and how you know",
        "What rush knows:\nDisk: 9G free of 500G.\n",
    ] {
        assert!(p.contains(want), "the prompt lacks {want:?}:\n{p}");
    }
    assert!(p.ends_with("\nWhat Haven reports:\n3 stacks idle\n"), "{p}");
    let example = p
        .split_once("For example:\n")
        .and_then(|(_, rest)| rest.split_once("\nPropose nothing"))
        .map(|(e, _)| e)
        .expect("an example");
    let (props, refused) = parse_proposals(example);
    assert!(refused.is_empty(), "{refused:?}");
    let mut f = merge_built_in(RulesFile::default());
    let got = add_proposals(&mut f, props, "/Users/me");
    assert_eq!(
        got.added,
        ["cache:Library/Caches/com.spotify.client", "pnpm-store"],
        "{got:?}"
    );
}

// What rush found, past care_known: where the disk goes, what's big and
// untouched, the plugins' kinds biggest first, the rules and proposals as
// JSON; or that the rules can't be read.
#[test]
fn research_known_says_what_rush_found() {
    let mut rules = merge_built_in(RulesFile::default());
    let (props, _) = parse_proposals(
        r#"[{"rule": {"id": "pnpm-store", "title": "t", "kind": {"type": "tool-command", "command": ["pnpm", "store", "prune"], "why": "w"}, "low": {}}, "evidence": "e"}]"#,
    );
    rules.proposed = props;
    let t = research_known(&found(Ok(rules)), "/Users/me", 100 * DAY).join("\n");
    for want in [
        "as rush walked it 1h ago (folder · size):\n- ~/Library · 80.0G",
        "- ~/.zed/old · 3.0G · logs · ",
        "js.node-modules: 2.0G in 1\n- ~/w/node_modules · 2.0G · w's packages · js-clean",
        "js.pnpm-store: 14.0G in 1\n- ~/Library/pnpm/store · 14.0G · ? · js-clean",
        "The engine's rules, as JSON (propose none of these again):\n- {\"id\":\"cargo-target\"",
        "Proposed already, waiting for the user (propose none of these again):\n- {\"id\":\"pnpm-store\"",
    ] {
        assert!(t.contains(want), "lacks {want:?}:\n{t}");
    }
    let t = research_known(&found(Err("bad json".into())), "/Users/me", 100 * DAY).join("\n");
    assert!(
        t.contains("The engine's rules can't be read (bad json)"),
        "{t}"
    );
}

#[test]
fn proposed_words_say_what_came() {
    let p = |added: &[&str], refused: &[(&str, &str)], already: bool| Proposed {
        added: added.iter().map(|s| s.to_string()).collect(),
        refused: refused
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        already,
        ..Proposed::default()
    };
    for (learned, research, want) in [
        (0, None, ""),
        (
            1,
            None,
            "disk: 1 rule proposed · accept or dismiss in Settings › Disk",
        ),
        (
            1,
            Some(p(&["a", "b"], &[], false)),
            "disk: 3 rules proposed · accept or dismiss in Settings › Disk",
        ),
        (0, Some(p(&["a"], &[("x", "why")], true)), ""),
        (
            0,
            Some(p(&[], &[("rm", "rm isn't a known tool")], false)),
            "disk: 1 proposal left out (rm: rm isn't a known tool)",
        ),
        (
            0,
            Some(p(&["a"], &[("x", "why"), ("y", "why")], false)),
            "disk: 1 rule proposed · accept or dismiss in Settings › Disk · 2 proposals left out (x: why)",
        ),
    ] {
        assert_eq!(proposed_words(learned, research.as_ref()), want, "{want}");
    }
}
