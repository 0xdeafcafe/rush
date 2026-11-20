//! Settings › Disk: the switch, the rules with their numbers and runs,
//! and proposals accepted only once seen whole. Every test keeps rush's
//! folder in a temp dir of its own; work handed off runs at once.

use rush_fleet::disk::{
    Done, Kept, Kind, Level, Outcome, Params, Proposal, Rule, RulesFile, built_in_rules,
    event_fields, load_rules, update_rules,
};
use rush_state::Store;
use rush_ui_settings::disk::{self, Num, Target};
use rush_ui_settings::form::flat;

use super::*;
use crate::frame::Press;

fn model(w: usize, h: usize) -> (Model, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir");
    rush_state::set_thread_dir(Some(dir.path().to_path_buf()));
    rush_state::set_thread_cache_dir(Some(dir.path().join("cache")));
    let mut m = crate::new_model("0.1.0");
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

fn body(m: &Model, w: usize) -> String {
    let mut g = workspace::Geometry::default();
    dialog_body(m, &mut g, w, settings_height(m))
        .iter()
        .map(Line::plain)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The cursor onto the row labelled label.
fn to_row(m: &mut Model, label: &str) {
    let rows = flat(&disk::sections(m));
    let i = rows
        .iter()
        .position(|s| s.label == label)
        .unwrap_or_else(|| panic!("no row {label:?}"));
    dialog_mut(m).cursor = i;
}

const H: u64 = 3600;
const D: u64 = 24 * H;

/// A while typed reads as seconds, and shows in words and short.
#[test]
fn spans_read_and_show() {
    for (typed, want) in [
        ("30m", Ok(30 * 60)),
        ("6h", Ok(6 * H)),
        ("2d", Ok(2 * D)),
        ("1w", Ok(7 * D)),
        ("90s", Ok(90)),
        ("1d12h", Ok(36 * H)),
        ("2 days", Ok(2 * D)),
        ("1 hour and 30 minutes", Ok(H + 30 * 60)),
        (" 45 MIN ", Ok(45 * 60)),
        ("1.5h", Ok(H + 30 * 60)),
        ("0", Ok(0)),
        ("", Err(())),
        ("12", Err(())),
        ("soon", Err(())),
        ("3 fortnights", Err(())),
    ] {
        assert_eq!(disk::parse_span(typed).map_err(|_| ()), want, "{typed:?}");
    }
    for (secs, words, short) in [
        (0, "0", "0"),
        (90, "90 seconds", "90s"),
        (60, "1 minute", "1m"),
        (30 * 60, "30 minutes", "30m"),
        (H, "1 hour", "1h"),
        (36 * H, "36 hours", "36h"),
        (D, "1 day", "1d"),
        (2 * D, "2 days", "2d"),
    ] {
        assert_eq!(disk::span(secs), words, "{secs}");
        assert_eq!(disk::span_short(secs), short, "{secs}");
    }
}

/// Each built-in rule's numbers at each level, as its row says them.
#[test]
fn built_in_rules_read_short() {
    let r = |id: &str| {
        built_in_rules()
            .into_iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no built-in {id}"))
    };
    for (id, normal, low) in [
        ("cargo-target", "2d", "30m"),
        ("cargo-artifacts", "2d, 3 past 1d", "6h, 3 past 1h"),
        ("worktree", "3d", "3d"),
        ("kache", "–", "runs"),
        ("go-mod-cache", "–", "0"),
    ] {
        assert_eq!(disk::level_short(&r(id), Level::Normal), normal, "{id}");
        assert_eq!(disk::level_short(&r(id), Level::Low), low, "{id}");
    }
    assert_eq!(
        disk::level_text(&r("go-mod-cache"), Level::Low),
        "whenever it runs"
    );
    assert_eq!(
        disk::what_it_does(&r("kache")),
        "kache's store; builds fill it again. Runs: kache clean --cache --yes"
    );
    let rm = Rule {
        kind: Kind::ToolCommand {
            command: vec!["rm".into(), "-rf".into(), "/".into()],
            why: "everything".into(),
        },
        ..r("kache")
    };
    assert!(
        disk::what_it_does(&rm).ends_with("It never runs: its program isn't one rush runs (kache, go, cargo, pnpm, npm, yarn, bun)."),
        "{}",
        disk::what_it_does(&rm)
    );
    assert_eq!(
        disk::command_line(&["sh".into(), "-c".into(), "rm -rf it's".into()]),
        r"sh -c 'rm -rf it'\''s'"
    );
}

/// What the runs did, read back from the events event_fields writes:
/// freed summed per rule, and what the newest run that left any left.
#[test]
fn runs_tally_by_rule() {
    let older = event_fields(
        15 << 30,
        Some(Level::Low),
        &Outcome {
            did: vec![Done {
                rule: "cargo-target".into(),
                path: "/w/a/target".into(),
                what: "removed".into(),
                freed: 2 << 30,
            }],
            left: vec![Kept {
                path: "/w/old".into(),
                rule: "worktree".into(),
                why: "commits not in main".into(),
            }],
            freed: 2 << 30,
        },
        "",
    );
    let newer = event_fields(
        30 << 30,
        Some(Level::Normal),
        &Outcome {
            did: vec![
                Done {
                    rule: "cargo-target".into(),
                    path: "/w/b (copy)/target".into(),
                    what: "removed".into(),
                    freed: 1 << 30,
                },
                Done {
                    rule: "kache".into(),
                    path: "".into(),
                    what: "ran kache clean".into(),
                    freed: 0,
                },
            ],
            left: vec![
                Kept {
                    path: "/w/x".into(),
                    rule: "worktree".into(),
                    why: "2 uncommitted files".into(),
                },
                Kept {
                    path: "/w/y".into(),
                    rule: "worktree".into(),
                    why: "locked".into(),
                },
            ],
            freed: 1 << 30,
        },
        "",
    );
    let runs = disk::tally(&[(2_000, newer), (1_000, older)]);
    assert_eq!(runs.n, 2);
    let last = runs.last.clone().expect("a last run");
    assert_eq!(
        (last.at, last.level.as_str(), last.freed),
        (2_000, "normal", 1 << 30)
    );
    let t = |id: &str| runs.by_rule.get(id).cloned().unwrap_or_default();
    assert_eq!(
        (t("cargo-target").freed, t("cargo-target").did),
        (3 << 30, 2)
    );
    assert_eq!((t("kache").did, t("kache").freed), (1, 0));
    assert_eq!(
        (t("worktree").left, t("worktree").why.as_str()),
        (2, "2 uncommitted files")
    );
    let idle = event_fields(80 << 30, None, &Outcome::default(), "enough free");
    let runs = disk::tally(&[(3_000, idle)]);
    assert_eq!(runs.last.map(|l| l.why), Some("enough free".into()));
}

fn proposal(id: &str, kind: Kind) -> Proposal {
    Proposal {
        rule: Rule {
            id: id.into(),
            title: format!("{id}'s rule"),
            kind,
            enabled: false,
            normal: None,
            low: Some(Params::default()),
        },
        evidence: "/tmp/x is 4G, made by pnpm".into(),
        by: "cleanup".into(),
        at: 1_000,
    }
}

/// The changes the page makes to the rules file, each on the merged
/// rules as update_rules hands them.
#[test]
fn changes_to_the_rules() {
    let base = || RulesFile {
        rules: built_in_rules(),
        proposed: vec![proposal(
            "pnpm",
            Kind::ToolCommand {
                command: vec!["pnpm".into(), "store".into(), "prune".into()],
                why: "pnpm's store".into(),
            },
        )],
        ..RulesFile::default()
    };
    // Accepting moves it into rules, on.
    let mut f = base();
    assert!(disk::accept(&mut f, "pnpm"));
    assert!(f.proposed.is_empty());
    let r = f.rules.iter().find(|r| r.id == "pnpm").expect("accepted");
    assert!(r.enabled);
    assert!(!disk::accept(&mut f, "pnpm"), "nothing left to accept");
    // Dismissing drops it.
    let mut f = base();
    assert!(disk::dismiss(&mut f, "pnpm"));
    assert!(f.proposed.is_empty() && !f.rules.iter().any(|r| r.id == "pnpm"));
    // A built-in turned off and changed shows it differs, and resets.
    let mut f = base();
    assert_eq!(disk::switch(&mut f, "worktree"), Some(false));
    assert!(disk::set_num(
        &mut f,
        &Target::Rule("worktree".into()),
        Level::Low,
        Num::Idle,
        D
    ));
    let w = f.rules.iter().find(|r| r.id == "worktree").unwrap();
    assert_eq!(w.low.as_ref().and_then(|p| p.idle_secs), Some(D));
    assert!(disk::changed_built_in(w).is_some());
    assert!(disk::reset(&mut f, "worktree"));
    let w = f.rules.iter().find(|r| r.id == "worktree").unwrap();
    assert!(disk::changed_built_in(w).is_none() && w.enabled);
    // A built-in can't be removed; one of yours can.
    assert!(!disk::remove(&mut f, "worktree"));
    assert!(disk::accept(&mut f, "pnpm"));
    assert!(disk::remove(&mut f, "pnpm"));
    // A level it didn't run at starts from the other's numbers; stopped,
    // it has none.
    let mut f = base();
    let t = Target::Rule("cargo-target".into());
    assert!(disk::run_at(&mut f, &t, Level::Normal, false));
    assert!(t.rule(&f).unwrap().normal.is_none());
    assert!(disk::run_at(&mut f, &t, Level::Normal, true));
    assert_eq!(
        t.rule(&f)
            .unwrap()
            .normal
            .as_ref()
            .and_then(|p| p.idle_secs),
        Some(30 * 60)
    );
    // A proposal's numbers change before it's accepted.
    let p = Target::Proposal("pnpm".into());
    assert!(disk::run_at(&mut f, &p, Level::Normal, true));
    assert!(t.rule(&f).is_some() && p.rule(&f).unwrap().normal.is_some());
}

/// The page: the switch, both levels, Research now (session_core's
/// start::install gives it the cleanup agent's research), every rule,
/// and nothing proposed.
#[test]
fn disk_page_shows_switch_levels_and_rules() {
    // As start::install does once hosts can start sessions (the process's
    // first; set here so the test doesn't hang on another's order).
    disk::on_research(rush_ui_projects_machine::research::research_now);
    let (mut m, _d) = model(240, 60);
    set_settings_page(&mut m, PAGE_DISK);
    let b = body(&m, 230);
    for want in [
        "Clean on its own when disk is low",
        "under 50G free",
        "under 40G free",
        "Last run",
        "none yet",
        "● Cargo targets no build touched",
        "● kache's store",
        "Proposed",
        "none waiting",
        "Research now",
    ] {
        assert!(b.contains(want), "Disk lacks {want:?}:\n{b}");
    }
    // The switch is off until turned on, and saved to config.json.
    assert!(!config(&m).disk.auto);
    to_row(&mut m, "Clean on its own when disk is low");
    press(&mut m, "space");
    assert!(config(&m).disk.auto);
    let raw = std::fs::read_to_string(rush_state::dir().join("config.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(saved["disk"]["auto"], true, "the scheduler's switch: {raw}");
}

/// A rule turned off from its row is written to disk-rules.json; its row
/// says it's off and changed, and r puts it back.
#[test]
fn disk_rule_switched_and_reset() {
    let (mut m, _d) = model(240, 60);
    set_settings_page(&mut m, PAGE_DISK);
    to_row(&mut m, "kache's store");
    press(&mut m, "space");
    let f = load_rules().unwrap();
    let k = f.rules.iter().find(|r| r.id == "kache").unwrap();
    assert!(!k.enabled);
    let b = body(&m, 230);
    let row = b
        .lines()
        .find(|l| l.contains("○ kache's store"))
        .unwrap_or_else(|| panic!("no kache row:\n{b}"));
    assert!(
        row.contains("○ kache's store") && row.contains("changed"),
        "{row}"
    );
    to_row(&mut m, "kache's store");
    press(&mut m, "r");
    let f = load_rules().unwrap();
    assert!(f.rules.iter().find(|r| r.id == "kache").unwrap().enabled);
}

/// A proposal is accepted only through a question that shows its command
/// in full; dismissing takes x twice.
#[test]
fn disk_proposal_accepted_after_seeing_its_command() {
    let (mut m, _d) = model(240, 60);
    let long = "--older-than-thirty-days-and-not-in-any-lockfile";
    update_rules(|f| {
        f.proposed.push(proposal(
            "pnpm",
            Kind::ToolCommand {
                command: vec!["pnpm".into(), "store".into(), "prune".into(), long.into()],
                why: "pnpm's store".into(),
            },
        ));
        f.proposed.push(proposal(
            "junk",
            Kind::PluginCandidate {
                kind: "node-modules".into(),
            },
        ));
    })
    .unwrap();
    set_settings_page(&mut m, PAGE_DISK);
    let b = body(&m, 230);
    assert!(b.contains("pnpm's rule") && b.contains("by cleanup"), "{b}");
    to_row(&mut m, "pnpm's rule");
    press(&mut m, "enter");
    let c = m.confirm.as_ref().expect("a question before accepting");
    assert!(
        c.detail.contains(&format!("Runs: pnpm store prune {long}")),
        "{}",
        c.detail
    );
    assert!(c.detail.contains("Evidence: /tmp/x is 4G, made by pnpm"));
    press(&mut m, "y");
    let f = load_rules().unwrap();
    assert!(f.rules.iter().any(|r| r.id == "pnpm" && r.enabled));
    assert_eq!(f.proposed.len(), 1);
    to_row(&mut m, "junk's rule");
    press(&mut m, "x");
    assert_eq!(load_rules().unwrap().proposed.len(), 1, "x once only asks");
    press(&mut m, "x");
    assert!(load_rules().unwrap().proposed.is_empty());
}

/// Everything about the disk is on this page: the rows that were
/// General's "Disk and memory" sit under When space runs short, keep
/// their config.json keys, and General keeps only the memory row.
#[test]
fn disk_rows_moved_from_general() {
    let (mut m, _d) = model(240, 60);
    set_settings_page(&mut m, PAGE_DISK);
    let secs = disk::sections(&m);
    let titles: Vec<&str> = secs.iter().map(|s| s.title.as_str()).collect();
    let mut want = vec![
        "Disk",
        "When space runs short",
        "Spotlight and Time Machine",
    ];
    if cfg!(target_os = "macos") {
        want.push("Scratch volume");
    }
    want.extend(["Rules", "Proposed"]);
    assert_eq!(
        titles, want,
        "the moved rows come after the switch and levels"
    );
    let short: Vec<&str> = secs[1].rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        short,
        [
            "Offer a cleanup when disk is under",
            "When nearly full, clean anyway under",
            "Trim a running session's temp files",
        ]
    );
    // The nearly-full clean says it's a switch of its own and what it
    // does, with the Low level's number as the page shows it; the switch
    // says the same from its side.
    let dire = &secs[1].rows[1];
    for want in [
        "runs even with that off",
        "the rules below, at their low numbers (under 40G free)",
        "only until there's room again",
    ] {
        assert!(dire.what.contains(want), "lacks {want:?}: {}", dire.what);
    }
    assert!(body(&m, 230).contains("under 40G free"));
    let auto = &secs[0].rows[0];
    assert!(
        auto.what.contains("nearly full"),
        "the switch points at the nearly-full clean: {}",
        auto.what
    );

    // General has none of them, and its section is Memory now.
    let general = (pages()[PAGE_GENERAL].form.unwrap())(&m);
    assert!(general.iter().all(|s| s.title != "Disk and memory"));
    let memory = general
        .iter()
        .find(|s| s.title == "Memory")
        .expect("General's Memory section");
    let labels: Vec<&str> = memory.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Offer a cleanup when memory is full for"]);
    for gone in short {
        assert!(
            flat(&general).iter().all(|r| r.label != gone),
            "{gone} is on Disk now"
        );
    }

    // Each row still writes the key it always did.
    for (label, key, want) in [
        ("Offer a cleanup when disk is under", "cleanupDiskGB", "10"),
        ("When nearly full, clean anyway under", "cleanupDireGB", "2"),
        (
            "Trim a running session's temp files",
            "trimRunningTmp",
            "true",
        ),
    ] {
        let row = flat(&disk::sections(&m))
            .into_iter()
            .find(|r| r.label == label)
            .unwrap_or_else(|| panic!("no row {label:?}"));
        // As a pick does: set, then saved.
        rush_ui_settings::form::cycle(&mut m, &row, 1);
        rush_ui_settings::changed(&mut m);
        let raw = std::fs::read_to_string(rush_state::dir().join("config.json")).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(saved[key].to_string(), want, "{label}: {raw}");
    }
    let c = config(&m);
    assert_eq!(
        (c.cleanup_disk_gb, c.cleanup_dire_gb, c.trim_running_tmp),
        (10, 2, true)
    );
}

/// Scratch is kept out of Spotlight and Time Machine unless turned off;
/// projects only once opted in, and each switch says what it costs.
#[test]
fn spotlight_and_time_machine_rows() {
    let (mut m, _d) = model(240, 60);
    set_settings_page(&mut m, PAGE_DISK);
    let secs = disk::sections(&m);
    let quiet = secs
        .iter()
        .find(|s| s.title == "Spotlight and Time Machine")
        .expect("the section");
    let rows: Vec<(&str, &str)> = quiet
        .rows
        .iter()
        .map(|r| (r.label.as_str(), r.value.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("Keep scratch out of Spotlight and Time Machine", "on"),
            ("Keep your projects out of them too", "off"),
        ]
    );
    assert!(quiet.rows[0].what.contains("tmutil addexclusion"));
    assert!(quiet.rows[1].what.contains("won't back it up"));
    for (label, key, want) in [
        (
            "Keep scratch out of Spotlight and Time Machine",
            "quietScratch",
            "false",
        ),
        (
            "Keep your projects out of them too",
            "quietProjects",
            "true",
        ),
    ] {
        let row = flat(&disk::sections(&m))
            .into_iter()
            .find(|r| r.label == label)
            .unwrap_or_else(|| panic!("no row {label:?}"));
        rush_ui_settings::form::cycle(&mut m, &row, 1);
        rush_ui_settings::changed(&mut m);
        let raw = std::fs::read_to_string(rush_state::dir().join("config.json")).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(saved[key].to_string(), want, "{label}: {raw}");
    }
    let c = config(&m);
    assert!(!c.quiets_scratch() && c.quiet_projects);
}

/// The scratch volume is off until made here, only through a question
/// that says it shares the disk's space; a test never runs diskutil, so
/// yes comes back refused and nothing is made.
#[cfg(target_os = "macos")]
#[test]
fn scratch_volume_made_only_on_yes() {
    let (mut m, _d) = model(240, 60);
    set_settings_page(&mut m, PAGE_DISK);
    let secs = disk::sections(&m);
    let sec = secs
        .iter()
        .find(|s| s.title == "Scratch volume")
        .expect("the section");
    let row = &sec.rows[0];
    assert_eq!(row.label, disk::VOLUME_ROW);
    assert_eq!(row.value, "off · scratch goes in /tmp/rush");
    assert_eq!(row.keys, ["enter", "make it"]);
    for want in [
        "sharing its free space",
        "fseventsd",
        "password once",
        "running ones keep theirs",
    ] {
        assert!(row.what.contains(want), "no {want:?}: {}", row.what);
    }
    to_row(&mut m, disk::VOLUME_ROW);
    press(&mut m, "enter");
    let c = m.confirm.as_ref().expect("a question before making it");
    assert_eq!(c.question, "Make a volume for rush's scratch?");
    assert!(
        c.detail.contains("shares the disk's free space"),
        "{}",
        c.detail
    );
    assert!(
        !c.detail.contains("encrypted"),
        "a test has no FileVault: {}",
        c.detail
    );
    assert_eq!(c.yes_text, "make it");
    press(&mut m, "y");
    let l = m.said.last().expect("rush said something");
    assert_eq!(l.text, "couldn't change the scratch volume: not in a test");
    let b = body(&m, 230);
    assert!(b.contains("off · scratch goes in /tmp/rush"), "{b}");
}

/// Removing asks too, and says it deletes what's on the volume.
#[test]
fn scratch_volume_removal_asks() {
    let c = disk::remove_volume_confirm("disk3s7".into());
    assert_eq!(c.question, "Remove the scratch volume?");
    assert!(
        c.detail
            .contains("Refused while a running session's scratch is on it")
    );
    assert_eq!(c.yes_text, "remove it");
}

/// With FileVault on, making it says it's encrypted like the disk.
#[test]
fn scratch_volume_encrypted_like_the_disk() {
    let c = disk::make_volume_confirm(true);
    assert!(
        c.detail.contains("It's encrypted, like your disk"),
        "{}",
        c.detail
    );
    assert!(c.detail.contains("login keychain"), "{}", c.detail);
    let c = disk::make_volume_confirm(false);
    assert!(!c.detail.contains("encrypted"), "{}", c.detail);
    let c = disk::remove_volume_confirm("disk3s7".into());
    assert!(c.detail.contains("passphrase goes from your keychain"));
}
