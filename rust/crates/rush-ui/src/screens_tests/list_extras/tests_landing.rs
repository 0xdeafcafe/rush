//! What's left before an agent's work lands: Go's landing_test.go
//! (TestLanding, TestLandingQuiet).

use rush_core::agent::{Agent, PrStatus, State};
use rush_fleet::{Folder, GitState};

use super::landing::*;
use super::tests::{MIN, NOW, agent, model_of};
use crate::list::TO_LAND;
use crate::model::Model;

const TREE: &str = "/src/checkout/.claude/worktrees/tax";

/// One stopped agent in a worktree, git having said st of it.
fn land_model(st: Option<GitState>, pr: Option<PrStatus>) -> Model {
    let mut a: Agent = agent("tax", "fix tax rounding", State::Done, 0);
    a.interactive = false;
    a.root = "/src/checkout".into();
    a.repo = TREE.into();
    a.pr = pr;
    a.seen = true; // looked at: Finished no longer has it
    a.job.updated_at = NOW - 20 * MIN;
    let mut m = model_of(vec![a], 120, 40);
    if let Some(st) = st {
        let mut f = Folder {
            root: "/src/checkout".into(),
            ..Folder::default()
        };
        f.git.branch = "main".into();
        f.trees.insert(TREE.into(), st);
        m.kept::<ListFolders>()
            .by_root
            .insert("/src/checkout".into(), f);
    }
    m.rebuild();
    m
}

fn pr(state: &str, checks: &str) -> Option<PrStatus> {
    Some(PrStatus {
        number: 41,
        state: state.into(),
        checks: checks.into(),
        ..PrStatus::default()
    })
}

/// Each row in a worktree says what's left before its work lands, worst
/// first, and only what's yours to merge goes under To land.
#[test]
fn landing() {
    let base = GitState {
        branch: "rush/tax".into(),
        base: "main".into(),
        ..GitState::default()
    };
    let with = |changed: i64, ahead: i64| GitState {
        changed,
        base_ahead: ahead,
        ..base.clone()
    };
    let open = || pr("open", "pass");
    for (name, st, pr, kind, text, to_land) in [
        (
            "uncommitted",
            with(3, 2),
            open(),
            LandKind::Uncommitted,
            "±3 not committed",
            false,
        ),
        (
            "commits, no pull request",
            with(0, 2),
            None,
            LandKind::Unmerged,
            "2 commits not on main",
            true,
        ),
        (
            "one commit",
            with(0, 1),
            None,
            LandKind::Unmerged,
            "1 commit not on main",
            true,
        ),
        (
            "ready",
            with(0, 2),
            open(),
            LandKind::PrReady,
            "PR #41 ready to merge",
            true,
        ),
        (
            "failing",
            base.clone(),
            pr("open", "fail"),
            LandKind::PrFailing,
            "PR #41 ✗ checks",
            false,
        ),
        (
            "draft",
            base.clone(),
            pr("draft", ""),
            LandKind::PrWaiting,
            "PR #41 draft",
            false,
        ),
        (
            "merged",
            with(0, 2),
            pr("merged", ""),
            LandKind::Landed,
            "landed · Earlier soon",
            false,
        ),
        (
            "nothing done",
            base.clone(),
            None,
            LandKind::Nothing,
            "nothing to land",
            false,
        ),
    ] {
        let mut m = land_model(Some(st), pr);
        let a = m.agents[0].clone();
        let l = land_of(&m, &a);
        assert_eq!((l.kind, l.text.as_str()), (kind, text), "{name}");
        let section = m.group_of.get(&a.key).cloned().unwrap_or_default();
        assert_eq!(section == TO_LAND, to_land, "{name}: in {section:?}");
        let screen: String = m
            .frame()
            .iter()
            .map(|l| l.plain())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            screen.contains(text),
            "{name}: no {text:?} under the name:\n{screen}"
        );
        if !matches!(kind, LandKind::Landed | LandKind::Nothing) {
            assert!(!land_text(&l).is_empty(), "{name}: nothing to tell it");
        }
    }
}

/// The main checkout, an agent at work, or git not having said yet: no
/// tag.
#[test]
fn landing_quiet() {
    let st = GitState {
        branch: "rush/tax".into(),
        base_ahead: 2,
        ..GitState::default()
    };
    let m = land_model(Some(st), None);
    let mut a = m.agents[0].clone();
    a.repo = a.root.clone();
    assert_eq!(land_of(&m, &a).kind, LandKind::None, "main checkout");
    a.repo = TREE.into();
    a.job.state = State::Working;
    a.pid = 7;
    assert_eq!(land_of(&m, &a).kind, LandKind::None, "at work");
    let m = land_model(None, None);
    assert_eq!(
        land_of(&m, &m.agents[0]).kind,
        LandKind::None,
        "git not asked yet"
    );
}

/// alt+p says why it can't, or tells the agent.
#[test]
fn land_key_says_why() {
    let mut m = land_model(None, None);
    m.sel = m.agents[0].key.clone();
    m.press_key(&crate::frame::Press::of("alt+p").unwrap());
    let said = m.flash.clone().unwrap().0;
    assert!(
        said.starts_with("git hasn't said yet what fix tax rounding has to land"),
        "{said}"
    );
}
