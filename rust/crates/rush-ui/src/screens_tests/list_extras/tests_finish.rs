//! The finishing card, after Go's internal/ui/finish_test.go
//! (TestFinishHeadline, TestFinishCard, TestFinishStepsSayWhereItIs,
//! TestFinishTabs; mergeDetail is the merge sheet's, not here).

use std::collections::{HashMap, HashSet};

use rush_core::text::Line;
use rush_gh::ghpr::{Check, PR, Status};

use super::finish::*;

fn check(name: &str, state: &str) -> Check {
    Check {
        name: name.into(),
        state: state.into(),
        ..Check::default()
    }
}

fn finish_pr() -> PR {
    let mut unit = check("unit", "failed");
    unit.workflow = "ci".into();
    PR {
        number: 118,
        title: "fix the flaky tax rounding test".into(),
        state: "OPEN".into(),
        url: "https://github.com/o/r/pull/118".into(),
        added: 18,
        removed: 6,
        files: 2,
        comments: 3,
        checks: vec![
            check("lint", "passed"),
            check("typecheck", "passed"),
            check("build", "passed"),
            check("e2e", "passed"),
            unit,
        ],
        ..PR::default()
    }
}

fn card(pr: PR) -> FinishCard {
    FinishCard {
        id: format!("finish:{}", pr.number),
        root: "/src/r".into(),
        pr,
        ..FinishCard::default()
    }
}

fn text(rows: &[Line]) -> String {
    rows.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

#[test]
fn headline_says_what_stands_in_the_way() {
    type Edit = fn(&mut PR);
    let cases: [(Edit, &str); 6] = [
        (|_| {}, "PR #118 can't merge yet: the unit check fails"),
        (
            |p| p.checks[0].state = "failed".into(),
            "PR #118 can't merge yet: 2 checks fail",
        ),
        (
            |p| p.checks[4].state = "pending".into(),
            "PR #118 is waiting on its checks: 4 of 5 done",
        ),
        (
            |p| {
                p.checks[4].state = "passed".into();
                p.decision = "REVIEW_REQUIRED".into();
            },
            "PR #118 passes its checks and waits for a review",
        ),
        (
            |p| {
                p.checks[4].state = "passed".into();
                p.decision = "APPROVED".into();
            },
            "PR #118 is ready to merge",
        ),
        (|p| p.state = "MERGED".into(), "PR #118 is merged"),
    ];
    for (edit, want) in cases {
        let mut f = card(finish_pr());
        edit(&mut f.pr);
        assert_eq!(f.headline().0, want);
    }
}

#[test]
fn card_draws_and_w_sets_it_aside() {
    let mut f = card(finish_pr());
    let mut hits = Vec::new();
    let rows = rows(&f, false, 100, &mut hits);
    assert!(fits(&rows, 100));
    for r in &rows {
        assert_eq!(r.width(), 100, "every row fills the card: {:?}", r.plain());
    }
    let t = text(&rows);
    for want in [
        "#118 ↗",
        "fix the flaky tax rounding test",
        "+18 −6",
        "2 files",
        "3 comments",
        "can't merge yet: the unit check fails",
        "✗ unit",
        "✓ pushed",
        "✗ checks 4 of 5",
        "└─ ✗ unit",
        "f fix it with this agent",
        "m merge anyway",
        "↑ gives it the keys",
    ] {
        assert!(t.contains(want), "no {want:?} in\n{t}");
    }
    let (mut gone, none) = (HashSet::new(), HashMap::new());
    assert_eq!(
        key(&mut f, false, "w", &mut gone, &none, &none),
        FinishAct::Pass,
        "w answered the card without its keys"
    );
    assert_eq!(
        key(&mut f, true, "w", &mut gone, &none, &none),
        FinishAct::Gone
    );
    assert!(gone.contains("finish:118"));
    let got = fix_text(&finish_pr());
    assert!(
        got.contains("The unit check fails") && got.contains("PR #118"),
        "{got}"
    );
}

#[test]
fn steps_say_where_it_is() {
    let p = PR {
        number: 8,
        title: "tags".into(),
        state: "OPEN".into(),
        head: "rush/x".into(),
        base: "main".into(),
        added: 3,
        removed: 1,
        files: 2,
        ..PR::default()
    };
    let mut f = card(p);
    let mut hits = Vec::new();
    let t = text(&rows(&f, false, 100, &mut hits));
    for want in ["– no checks", "– no review needed", "▸ merge"] {
        assert!(t.contains(want), "no {want:?} in\n{t}");
    }
    f.pr.decision = "REVIEW_REQUIRED".into();
    let t = text(&rows(&f, false, 100, &mut hits));
    assert!(t.contains("▸ review") && t.contains("○ merge"), "{t}");
    f.merging = "squash".into();
    f.asking = true;
    let t = text(&rows(&f, false, 100, &mut hits));
    assert!(
        t.contains("is merging…")
            && t.contains("◌ merging (squash)…")
            && t.contains("◌ asking GitHub")
            && !t.contains("m merge"),
        "a merge under way says so, and can't be asked again:\n{t}"
    );
}

#[test]
fn tabs_open_on_the_neediest_and_move() {
    let mut failing = finish_pr();
    failing.number = 16;
    let prs = vec![
        (
            "/src/alpha".to_string(),
            PR {
                number: 15,
                title: "A calmer conversation".into(),
                state: "MERGED".into(),
                ..PR::default()
            },
        ),
        ("/src/alpha".to_string(), failing),
        (
            "/src/alpha".to_string(),
            PR {
                number: 17,
                title: "Pending one".into(),
                state: "OPEN".into(),
                checks: vec![check("ci", "pending"), check("lint", "passed")],
                ..PR::default()
            },
        ),
    ];
    let (merged, none, mut gone) = (HashSet::new(), HashMap::new(), HashSet::new());
    let pick = |at: u64, gone: &HashSet<String>, old: Option<&FinishCard>| {
        build(&Pick {
            prs: &prs,
            merged: &merged,
            merging: &none,
            merge_failed: &none,
            gone: &gone.clone(),
            at,
            old,
            focus: true,
            ..Pick::default()
        })
    };
    let mut f = pick(0, &gone, None).expect("a card");
    assert_eq!((f.tabs.len(), f.pr.number), (3, 16));
    let mut hits = Vec::new();
    let t = text(&rows(&f, false, 120, &mut hits));
    for want in [
        "#15 merged",
        "#16 ✗ 1 failing",
        "#17 ◌ 1/2",
        "PR #16 can't merge yet",
        "← → the others",
    ] {
        assert!(t.contains(want), "no {want:?} in\n{t}");
    }
    assert_eq!(hits.len(), 3, "tabs to click: {hits:?}");
    key(&mut f, true, "right", &mut gone, &none, &none);
    assert_eq!(f.pr.number, 17);
    key(&mut f, true, "pr:15", &mut gone, &none, &none);
    assert_eq!(f.pr.number, 15);
    // Kept on the next draw.
    let mut f = pick(15, &gone, Some(&f)).unwrap();
    assert_eq!(f.pr.number, 15);
    key(&mut f, true, "w", &mut gone, &none, &none);
    assert!(f.tabs.len() == 2 && f.pr.number != 15, "{f:?}");
    let f = pick(0, &gone, None).unwrap();
    assert_eq!(f.tabs.len(), 2, "set aside stays aside");
}

#[test]
fn a_branch_status_stands_in() {
    let s = Status {
        number: 9,
        title: "t".into(),
        state: "draft".into(),
        checks: "fail".into(),
        failing: "lint".into(),
        review: "changes".into(),
        ..Status::default()
    };
    let p = from_status(&s);
    assert_eq!(
        (p.state.as_str(), p.draft, p.decision.as_str()),
        ("OPEN", true, "CHANGES_REQUESTED")
    );
    assert_eq!(failed_checks(&p), ["lint"]);
    let (merged, none, gone) = (HashSet::new(), HashMap::new(), HashSet::new());
    let f = build(&Pick {
        status: Some(&s),
        merged: &merged,
        merging: &none,
        merge_failed: &none,
        gone: &gone,
        ..Pick::default()
    })
    .unwrap();
    assert_eq!(f.pr.number, 9);
}

#[test]
fn narrow_cards_never_spill() {
    let mut f = card(finish_pr());
    f.tabs = (1..6)
        .map(|n| {
            let mut pr = finish_pr();
            pr.number = n;
            FinishTab {
                id: n.to_string(),
                root: String::new(),
                pr,
            }
        })
        .collect();
    let mut hits = Vec::new();
    for w in [0, 1, 4, 5, 12, 30, 61] {
        let r = rows(&f, true, w, &mut hits);
        assert!(fits(&r, w), "spilled at {w}");
    }
}
