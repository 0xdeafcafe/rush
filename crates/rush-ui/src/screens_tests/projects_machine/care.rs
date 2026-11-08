//! The tests of rush-ui-projects-machine's care.rs that need the whole UI.

use crate::frame::sheet::Sheet;
use crate::frame::{Out, Press};
use crate::style::*;
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_plugin::CleanupReport;
use rush_ui_projects_machine::care::*;
use rush_ui_projects_machine::data::*;

#[test]
fn reasons() {
    const G: u64 = 1 << 30;
    let cases: &[(u64, u64, i64, Ms, i64, &str)] = &[
        (
            10 * G,
            500 * G,
            0,
            0,
            0,
            "the disk has 10.0G free, under the 20G you set",
        ),
        (30 * G, 500 * G, 0, 0, 0, ""),
        (10 * G, 500 * G, -1, 0, 0, ""),
        (10 * G, 0, 0, 0, 0, ""),
        (
            30 * G,
            500 * G,
            0,
            16 * 60_000,
            0,
            "the memory has been under pressure for 16 minutes",
        ),
        (30 * G, 500 * G, 0, 16 * 60_000, -1, ""),
        (30 * G, 500 * G, 0, 14 * 60_000, 0, ""),
        (
            30 * G,
            500 * G,
            40,
            0,
            0,
            "the disk has 30.0G free, under the 40G you set",
        ),
    ];
    for &(free, total, gb, pressed, min, want) in cases {
        assert_eq!(care_reason(free, total, gb, pressed, min), want);
    }
}

// The cleanup agent, which tidies worktrees, is offered only by the
// rush that leads: one that follows never even reads the machine for
// it.
#[test]
fn only_the_rush_that_leads_watches_for_a_cleanup() {
    for follows in [true, false] {
        let lp: rush_draw::Loop<crate::worker::Note> = rush_draw::Loop::new();
        let mut m = crate::new_model_with_workers("t", lp.sender());
        m.follows = follows;
        m.now = CARE_EVERY * 2;
        watch_machine(&mut m);
        let c = m.kept::<Care>();
        assert_eq!(c.reading, !follows, "follows {follows}");
        assert_eq!(c.read_at, if follows { 0 } else { CARE_EVERY * 2 });
    }
}

fn cleanup_agent(state: rush_core::agent::State) -> rush_core::agent::Agent {
    rush_core::agent::Agent {
        key: "c".into(),
        display_name: "Cleanup".into(),
        rush: true,
        started_by: CARE_PLUGIN.into(),
        job: rush_core::agent::Job {
            id: "c1".into(),
            state,
            ..Default::default()
        },
        ..Default::default()
    }
}

// machinecare.go:80-115: a cleanup agent at work (one started before
// a reload is found again by its StartedBy) holds off the offer; once
// its turn is over and settled, it's put in Earlier, its host stopped
// and the news says so. A rush that follows does none of it.
#[test]
fn a_finished_cleanup_agent_is_put_away_by_the_lead() {
    use rush_core::agent::State;
    for follows in [false, true] {
        let lp: rush_draw::Loop<crate::worker::Note> = rush_draw::Loop::new();
        let mut m = crate::new_model_with_workers("t", lp.sender());
        let h = crate::host::Fake::new();
        crate::host::set_hosts(&mut m, std::sync::Arc::new(h.clone()));
        m.follows = follows;
        m.agents = vec![cleanup_agent(State::Working)];
        m.roots = vec![String::new()];
        m.now = 1_000_000;
        watch_machine(&mut m);
        let c = m.kept::<Care>();
        assert_eq!(
            c.busy, !follows,
            "follows {follows}: at work holds the offer"
        );
        assert_eq!(
            c.agents.len(),
            usize::from(!follows),
            "found again after a reload"
        );
        m.agents[0].job.state = State::Done;
        m.now += 60_000;
        watch_machine(&mut m);
        assert!(!m.agents[0].done, "it settles first");
        m.now += CARE_SETTLE;
        watch_machine(&mut m);
        assert_eq!(m.agents[0].done, !follows, "follows {follows}");
        let said = m
            .said
            .iter()
            .any(|l| l.text == "machine cleanup finished: its report is in Earlier");
        assert_eq!(said, !follows);
        assert!(m.kept::<Care>().agents.is_empty());
    }
}

// One finished before this rush saw it, or brought back by you, is
// yours: left alone.
#[test]
fn a_cleanup_agent_finished_before_is_left_alone() {
    let mut m = crate::new_model("t");
    m.agents = vec![cleanup_agent(rush_core::agent::State::Done)];
    let (done, busy) = finished_care(&mut m, 1_000_000);
    assert!(done.is_empty() && !busy);
}

// machinecare.go:313-322: it starts as rush's own, named Cleanup, with
// why on its row, and is watched from then.
#[test]
fn starts_as_rush_s_own() {
    let mut m = crate::new_model("t");
    let h = crate::host::Fake::new();
    crate::host::set_hosts(&mut m, std::sync::Arc::new(h.clone()));
    start_care(
        &mut m,
        "the disk has 9G free",
        "9.0G free on disk",
        &[],
        &[],
        false,
    );
    let starts = h.starts.lock().unwrap().clone();
    assert_eq!(starts.len(), 1);
    let s = &starts[0];
    assert_eq!(
        (s.name.as_str(), s.started_by.as_str(), s.why.as_str()),
        ("Cleanup", CARE_PLUGIN, "9.0G free on disk")
    );
    assert!(s.prompt.contains("the disk has 9G free"), "{}", s.prompt);
    // Beyond Go: never auto or bypass, whatever new sessions start in.
    assert_eq!(
        rush_core::adapter::trust(&s.permission_mode),
        0,
        "{}",
        s.permission_mode
    );
    assert_eq!(m.kept::<Care>().agents[0].0, "fake-1");
}

// The cleanup agent's mode is one that asks: default where there is one,
// else the first asking mode but plan, else the agent's own.
#[test]
fn asking_modes() {
    let ms = |ids: &[&str]| -> Vec<rush_core::adapter::Choice> {
        ids.iter()
            .map(|id| rush_core::adapter::Choice {
                id: (*id).into(),
                ..Default::default()
            })
            .collect()
    };
    for (modes, want) in [
        (
            ms(&[
                "default",
                "acceptEdits",
                "plan",
                "auto",
                "bypassPermissions",
            ]),
            "default",
        ),
        (ms(&["yolo", "plan", "suggest", "full-access"]), "suggest"),
        (ms(&["auto", "yolo"]), ""),
        (ms(&[]), ""),
    ] {
        assert_eq!(asking_mode(&modes), want, "{modes:?}");
    }
}

// Its plan's turn ends on a decisions block: it waits on you (Blocked,
// under Needs you), still at work for the watch, not finished.
#[test]
fn a_cleanup_agent_waiting_on_its_plan_isnt_finished() {
    let mut m = crate::new_model("t");
    m.agents = vec![cleanup_agent(rush_core::agent::State::Working)];
    let (done, busy) = finished_care(&mut m, 1_000_000);
    assert!(done.is_empty() && busy);
    m.agents[0].job.state = rush_core::agent::State::Blocked;
    let (done, busy) = finished_care(&mut m, 1_000_000 + 10 * CARE_SETTLE);
    assert!(done.is_empty(), "the plan's turn taken as its end");
    assert!(busy, "a cleanup waiting on you holds the offer");
}

#[test]
fn lists() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(and_list(&s(&[])), "");
    assert_eq!(and_list(&s(&["a"])), "a");
    assert_eq!(and_list(&s(&["a", "b"])), "a and b");
    assert_eq!(and_list(&s(&["a", "b", "c"])), "a, b and c");
}

#[test]
fn prompt_has_rules_known_and_reports() {
    let r = CleanupReport {
        title: "Haven".into(),
        report: " 3 stacks idle \n".into(),
        rules: "Stop only idle stacks.".into(),
    };
    let p = care_prompt(
        "the disk is low",
        &["Disk: 1G free of 2G.".into()],
        &[r],
        false,
    );
    assert!(!p.contains("Look further"));
    assert!(p.starts_with("Free disk space and memory on this machine, but only what the user approves. rush started you because the disk is low. "));
    assert!(p.contains("\nHaven, as its plugin says:\nStop only idle stacks.\n"));
    assert!(p.contains("What rush knows:\nDisk: 1G free of 2G.\n"));
    assert!(!p.contains("Nobody is watching"), "{p}");
    // Approve first (Rust-only): a read-only plan of every path with its
    // size, why and whether it's conservative, totals, a decisions block of
    // all, some or none; then only what's approved, in one script.
    for want in [
        "Never delete, remove or stop anything on your own",
        "Phase 1, now, is read-only",
        "delete, remove or stop nothing. End your turn with a plan",
        "every path you would delete, each with its size and why it's safe to go",
        "conservative: yes when it's only something made or fetched again",
        "each group's total, and a grand total",
        "what you'd leave, and why",
        "$TMPDIR/cleanup-plan.tsv",
        "(a) delete all of it; (b) only some, the user names which; (c) none.",
        "Phase 2 starts only once the user answers. Delete exactly what they approved, nothing new",
        "one script, $TMPDIR/cleanup.sh",
        "run it once, so the user sees and approves one command",
        "Run df -h before and after",
        "one that changed since the plan (touched since, or now in use) stays out of the script, and you say so",
        // Rush's own scratch, by whether its session is done, stopped or
        // running, and what no session owns.
        "- Rush's scratch folders (each session's $TMPDIR, listed below): a done session's; a stopped one's only once nothing has touched it for days",
        "a running one's never.",
        "no session's once nothing has touched it for days and no running session uses it",
        "never the folders that hold them",
    ] {
        assert!(p.contains(want), "the rules lack {want:?}:\n{p}");
    }
    assert!(p.ends_with("\nWhat Haven reports:\n3 stacks idle\n"));
}

fn sheet(off: bool) -> CareSheet {
    let s = CareSheet {
        why: "the disk has 10.0G free, under the 20G you set".into(),
        short: "10G free on disk".into(),
        cur: 0,
        busy: false,
        looking: false,
        off,
        from: vec!["Haven".into(), "go-clean".into()],
        asked: false,
    };
    CareSheet {
        cur: s.not_now(),
        ..s
    }
}

#[test]
fn prompt_looks_further_when_told() {
    let p = care_prompt("you asked for one", &[], &[], true);
    assert!(p.contains("\nLook further, too:"), "{p}");
    assert!(p.contains("never its volumes or containers"), "{p}");
    assert!(p.contains("com.google.Chrome.code_sign_clone"), "{p}");
    assert!(p.contains("the biggest things you found but left"), "{p}");
}

#[test]
fn asked_sheet_starts_on_start_and_snoozes_nothing() {
    let m = crate::new_model("t");
    let mut s = CareSheet {
        why: "you asked for one, with 3.0G free on disk".into(),
        asked: true,
        cur: 0,
        ..sheet(false)
    };
    let t = text(&s.body(&m, 100, 30));
    assert!(t.starts_with("Cleanup  you asked for one"), "{t}");
    assert!(!t.contains("rush noticed"), "{t}");
    assert!(t.contains("▍ Start a cleanup agent"), "{t}");
    assert!(
        t.contains("Start one that looks further  also hunts caches"),
        "{t}"
    );
    assert!(t.contains("Not now  starts nothing"), "{t}");
}

#[test]
fn cleanup_command_is_registered() {
    let m = crate::new_model("t");
    assert!(
        m.ext.commands.iter().any(|c| c.name == "cleanup"),
        "no #cleanup"
    );
}

fn text(l: &[Line]) -> String {
    l.iter().map(text_of).collect::<Vec<_>>().join("\n")
}

#[test]
fn sheet_says_what_it_reads() {
    let m = crate::new_model("t");
    let mut s = sheet(false);
    let t = text(&s.body(&m, 100, 30));
    assert!(t.starts_with("Cleanup  the disk has 10.0G free"), "{t}");
    let flat = t.replace("\n  ", " ");
    assert!(
        flat.contains("your worktrees, rush's scratch folders, what agents left in /tmp and what Haven and go-clean report, deleting nothing, and propose"),
        "{t}"
    );
    assert!(t.contains("Start a cleanup agent  asks before it deletes anything"));
    assert!(
        flat.contains("propose a list of what can go without losing work"),
        "{t}"
    );
    assert!(!t.contains("auto mode"), "{t}");
    assert!(t.contains("Start one that looks further"));
    assert!(t.contains("▍ Not now  asks again in 6 hours if it's still so"));
    assert!(t.contains("↑↓ choose"));
    let t = text(&sheet(true).body(&m, 100, 30));
    assert!(t.contains("Turn on the cleanup plugin  recommended"));
    assert!(!t.contains("looks further"), "{t}");
    assert!(
        t.replace("\n  ", " ")
            .contains("The cleanup plugin is off, and recommended now")
    );
}

#[test]
fn sheet_keys() {
    let mut m = crate::new_model("t");
    let mut s = sheet(false);
    assert_eq!(s.cur, 2);
    for (k, cur) in [("down", 0), ("tab", 1), ("up", 0), ("shift+tab", 2)] {
        assert_eq!(s.key(&mut m, &Press::of(k).unwrap()), Out::Draw);
        assert_eq!(s.cur, cur, "{k}");
    }
    let mut s = sheet(true);
    assert_eq!(s.cur, 1);
    assert_eq!(s.key(&mut m, &Press::of("down").unwrap()), Out::Draw);
    assert_eq!(s.cur, 0);
    assert_eq!(s.key(&mut m, &Press::of("f").unwrap()), Out::Idle);
    for k in ["n", "esc", "ctrl+c"] {
        let mut s = sheet(false);
        s.cur = 0;
        assert_eq!(s.key(&mut m, &Press::of(k).unwrap()), Out::Close, "{k}");
        assert_eq!(s.cur, 2);
    }
    let mut s = sheet(false);
    s.busy = true;
    assert_eq!(s.key(&mut m, &Press::of("enter").unwrap()), Out::Idle);
}

#[test]
fn busy_says_what_its_doing() {
    let m = crate::new_model("t");
    let last = |s: &mut CareSheet| text_of(s.body(&m, 100, 30).last().unwrap());
    let mut s = sheet(false);
    s.busy = true;
    assert_eq!(
        last(&mut s),
        "⋯ asking Haven and go-clean for their reports"
    );
    s.looking = true;
    assert_eq!(
        last(&mut s),
        "⋯ looking at your worktrees and asking Haven and go-clean for their reports"
    );
    s.from.clear();
    assert_eq!(last(&mut s), "⋯ looking at your worktrees");
    s.looking = false;
    assert_eq!(last(&mut s), "⋯ starting");
}

#[test]
fn short_why() {
    let g = 18 << 30;
    assert_eq!(
        care_short("the disk has 18.0G free, under the 20G you set", g),
        "18.0G free on disk"
    );
    assert_eq!(
        care_short("the memory has been under pressure for 16 minutes", g),
        "memory under pressure"
    );
}

#[test]
fn start_waits_on_scan_and_reports() {
    let wait = std::time::Duration::from_secs(5);
    let r = || {
        vec![CleanupReport {
            title: "Haven".into(),
            ..Default::default()
        }]
    };
    let st = care_start(Some(|| 7), r, wait);
    assert_eq!(st.scan, Some(7));
    assert!(st.late.is_none());
    assert_eq!(st.reports.len(), 1);
    let st = care_start(None::<fn() -> i32>, r, wait);
    assert_eq!((st.scan, st.reports.len()), (None, 1));
}

#[test]
fn start_goes_on_past_the_wait() {
    let (go, hold) = std::sync::mpsc::channel::<()>();
    let st = care_start(
        Some(move || {
            let _ = hold.recv();
            7
        }),
        Vec::new,
        std::time::Duration::from_millis(20),
    );
    assert_eq!(st.scan, None);
    let late = st.late.expect("the scan lands later");
    go.send(()).unwrap();
    assert_eq!(late.recv().ok(), Some(7));
}

/// The brief names rush's own scratch (Rust-only): sessions' folders of
/// 100M or more, biggest first, each done, stopped or running, and what's
/// in rush's scratch root that no session owns, kept while a running
/// session's id starts its name.
#[test]
fn known_lists_rush_scratch() {
    use rush_core::agent::{Agent, State};
    const DAY: Ms = 24 * 3_600_000;
    let now: Ms = 1_793_190_896_789;
    let mut m = crate::new_model("t");
    m.now = now;
    let ses = |id: &str, name: &str, pid: i32, ago: Ms| {
        let mut a = Agent {
            key: format!("default/{id}"),
            display_name: name.into(),
            rush: true,
            pid,
            ..Agent::default()
        };
        a.job.id = id.into();
        a.job.state = if pid != 0 {
            State::Working
        } else {
            State::Idle
        };
        a.job.updated_at = now - ago;
        a
    };
    m.agents = vec![
        ses("aaaa1111", "done work", 0, 2 * DAY),
        ses("bbbb2222", "stopped work", 0, 3 * 3_600_000),
        ses("cccc3333", "running work", 9, 0),
        ses("dddd4444", "small work", 0, DAY),
    ];
    m.group_of
        .insert("default/aaaa1111".into(), crate::list::FINISHED.into());
    let d = m.kept::<Data>();
    for (id, mb) in [
        ("aaaa1111", 150),
        ("bbbb2222", 300),
        ("cccc3333", 200),
        ("dddd4444", 10),
    ] {
        d.temp.insert(
            format!("default/{id}"),
            TempWork {
                dirs: vec![rush_core::adapter::TempDir {
                    path: format!("/cfg/tmp/app-1a2b3c4d/{id}"),
                    keep: true,
                }],
                bytes: mb << 20,
                known: true,
            },
        );
    }
    let o = |p: &str, session: &str, size: i64, ago: Ms| rush_fleet::TempOrphan {
        path: format!("/cfg/tmp/{p}"),
        size,
        newest: now - ago,
        session: session.into(),
        gone: false,
    };
    d.scratch_orphans = vec![
        o("os27-builds", "", 2 << 30, 5 * DAY),
        o("app-1a2b3c4d/cccc3333browse", "cccc3333", 5 << 20, 4 * DAY),
    ];
    let mut out = Vec::new();
    care_known_with(&m, &mut out, -1);
    assert_eq!(
        out[1..],
        [
            "",
            "Sessions' scratch folders (their $TMPDIR) of 100M or more (path · session · size · done/stopped/running · last touched):",
            "- /cfg/tmp/app-1a2b3c4d/bbbb2222 · stopped work · 300M · stopped · last touched 3h ago",
            "- /cfg/tmp/app-1a2b3c4d/cccc3333 · running work · 200M · running · last touched just now",
            "- /cfg/tmp/app-1a2b3c4d/aaaa1111 · done work · 150M · done · last touched 2 days ago",
            "",
            "In rush's scratch folders but no session's (path · size · last touched · the session it probably came from):",
            "- /cfg/tmp/os27-builds · 2.0G · last touched 5 days ago",
            "- /cfg/tmp/app-1a2b3c4d/cccc3333browse · 5M · last touched 4 days ago · from running work · running work is running: keep it",
        ]
    );
    assert!(out[0].starts_with("Worktrees: rush hasn't looked yet"));
}

#[test]
fn known_before_and_after_a_look() {
    let mut m = crate::new_model("t");
    let mut out = Vec::new();
    care_known_with(&m, &mut out, 40);
    assert_eq!(out[0], "Memory: 40% available.");
    assert!(out[1].starts_with("Worktrees: rush hasn't looked yet"));
    m.now = 10 * 60_000;
    let d = m.kept::<Data>();
    d.checked = 5 * 60_000;
    let mut w = rush_fleet::Worktree::default();
    w.path = "/src/r/.claude/worktrees/a".into();
    w.branch = "rush/a".into();
    d.wts.push(w);
    d.tmp.stale_items = 2;
    d.tmp.stale = 3 << 20;
    let mut out = Vec::new();
    care_known_with(&m, &mut out, -1);
    assert_eq!(
        out,
        [
            "",
            "Worktrees, as rush checked them 5m ago (path · branch · size · verdict):",
            "- /src/r/.claude/worktrees/a · rush/a · – · would-lose (not looked at with git yet)",
            "",
            "/tmp: 2 of your items untouched for days, 3M.",
        ]
    );
}
