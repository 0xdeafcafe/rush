//! The list's grouping and the side-by-side view, after Go's internal/ui
//! listorder_test.go (Earlier by the time its rows show), folders_test.go
//! (a section split by project), subrows_test.go, machinecare_test.go
//! (the startedBy line), rail.go's prShort and statusShort, view.go's
//! widths and sideWidth, and shelf.go: the list as the frame draws it.

use rush_core::adapter::{Checks, Pr};
use rush_core::agent::{PrStatus, State, SubagentTile};
use rush_core::text::Line;

use super::split;
use super::tests::{HOUR, MIN, NOW, agent, model_of};
use crate::frame::Press;
use crate::list::{EARLIER, ListLine, PINNED, pr_short, started_by, status_short};
use crate::model::Model;

fn sections(m: &Model) -> Vec<String> {
    m.lines
        .iter()
        .filter_map(|l| match l {
            ListLine::Section { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect()
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

/// What's put away goes to Earlier however fresh, a pinned agent keeps
/// a section of its own, and Earlier lists the latest first, folded
/// at the foot with what's in it (model.go:2257, a4c94f59).
#[test]
fn put_away_and_pinned() {
    let mut away = agent("a1", "put away this morning", State::Stopped, 0);
    away.done = true;
    away.job.updated_at = NOW - 10 * MIN;
    let mut old = agent("b2", "left a week ago", State::Stopped, 0);
    old.job.updated_at = NOW - 7 * 24 * HOUR;
    let mut pin = agent("c3", "pinned for later", State::Stopped, 0);
    pin.pinned = true;
    pin.job.updated_at = NOW - 3 * HOUR;
    let mut m = model_of(vec![old, away, pin], 120, 40);
    assert_eq!(sections(&m), vec![PINNED, EARLIER]);
    let ord: Vec<&str> = m
        .order
        .iter()
        .map(|&i| m.agents[i].display_name.as_str())
        .collect();
    assert_eq!(
        ord,
        [
            "pinned for later",
            "put away this morning",
            "left a week ago"
        ]
    );
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains(" ▸ Earlier  2   put away this morning, left a week ago"),
        "{f}"
    );
}

/// Split by project, each section's rows sit under their repository's
/// heading, and the line under each row names its branch, not the
/// project its heading already names (model.go:2346, rail.go:184).
#[test]
fn split_by_project() {
    let mut a = agent("a1", "fix the tax rounding", State::Working, 1);
    a.repo = "/src/zeta".into();
    a.job.cwd = "/src/zeta".into();
    let mut b = agent("b2", "add Apple Pay", State::Working, 2);
    b.repo = "/src/alpha".into();
    b.job.cwd = "/src/alpha".into();
    b.branch = "pay".into();
    let mut m = model_of(vec![a, b], 120, 40);
    m.cfg.split_by = "project".into();
    m.rebuild();
    let f = screen(&mut m);
    let at = |s: &str| {
        f.iter()
            .position(|l| l.trim_end() == s)
            .unwrap_or_else(|| panic!("no {s:?} in\n{}", f.join("\n")))
    };
    let (alpha, zeta) = (at("┃ alpha"), at("┃ zeta"));
    assert!(alpha < zeta, "projects go by name");
    assert!(f[alpha + 1].contains("add Apple Pay"));
    assert_eq!(f[alpha + 2].trim(), "┃      pay".trim(), "{f:#?}");
    // Not split: the line under the row names the project too.
    m.cfg.split_by = "none".into();
    m.rebuild();
    let f = screen(&mut m).join("\n");
    assert!(f.contains("alpha · pay"), "{f}");
    assert!(!f.contains("┃ alpha\n"), "{f}");
}

/// An agent's running subagents are rows under it, four at most, the
/// last counting the rest; ← tucks them away (subrows.go).
#[test]
fn subagent_rows() {
    let mut a = agent("a1", "port the list", State::Working, 1);
    a.subagents = (0..6)
        .map(|i| SubagentTile {
            id: format!("s{i}"),
            kind: "opus".into(),
            description: format!("piece {i}"),
            born: NOW - 6 * MIN,
            ..SubagentTile::default()
        })
        .collect();
    let mut m = model_of(vec![a], 120, 40);
    let f = screen(&mut m).join("\n");
    for want in ["piece 0 · opus", "piece 2 · opus", "└ 3 more subagents"] {
        assert!(f.contains(want), "{want:?} in\n{f}");
    }
    assert!(!f.contains("piece 3"), "{f}");
    assert!(f.contains("6m"), "{f}");
    m.press_key(&Press::of("left").unwrap());
    let f = screen(&mut m).join("\n");
    assert!(!f.contains("more subagents"), "{f}");
}

/// A session a plugin started says who runs it and why in place of
/// where it works; rush's own says rush and the job (rail.go:130).
#[test]
fn started_by_line() {
    rush_bundled::register(); // as a model's places make them known
    let mut a = agent("a1", "tidy the disk", State::Idle, 0);
    assert!(started_by(&a).is_none());
    a.started_by = "tidy-bot".into();
    a.why = "disk is low".into();
    assert_eq!(
        started_by(&a).unwrap().plain(),
        "tidy-bot · disk is low",
        "a plugin of yours says its own name"
    );
    a.started_by = "cleanup".into();
    assert_eq!(
        started_by(&a).unwrap().plain(),
        "rush · cleanup · disk is low",
        "rush's own says rush and the job"
    );
}

/// A pull request in a few words, as its agent linked it and as GitHub
/// last said it (rail.go:155 prShort, :227 statusShort).
#[test]
fn pull_requests_in_a_few_words() {
    let pr = |state: &str, review: &str, p: i64, f: i64, q: i64| Pr {
        number: 7,
        state: state.into(),
        review: review.into(),
        checks: Checks {
            passed: p,
            failed: f,
            pending: q,
        },
        ..Pr::default()
    };
    for (p, want) in [
        (pr("MERGED", "", 0, 0, 0), "PR #7 merged"),
        (pr("CLOSED", "", 0, 0, 0), "PR #7 closed"),
        (pr("OPEN", "", 1, 2, 0), "PR #7 ✗ 2 failed"),
        (pr("OPEN", "", 1, 0, 2), "PR #7 · checks 1 of 3"),
        (pr("OPEN", "APPROVED", 3, 0, 0), "PR #7 ✓ approved"),
        (pr("DRAFT", "", 0, 0, 0), "PR #7 draft"),
        (pr("OPEN", "", 3, 0, 0), "PR #7 ✓"),
        (pr("OPEN", "", 0, 0, 0), "PR #7"),
    ] {
        assert_eq!(pr_short(&p).text, want);
    }
    let st = |state: &str, checks: &str, review: &str| PrStatus {
        number: 9,
        state: state.into(),
        checks: checks.into(),
        failing: "lint".into(),
        review: review.into(),
        ..PrStatus::default()
    };
    for (p, want) in [
        (st("merged", "", ""), "PR #9 merged"),
        (st("closed", "", ""), "PR #9 closed"),
        (st("open", "fail", ""), "PR #9 ✗ lint"),
        (st("open", "pending", ""), "PR #9 · checks running"),
        (st("open", "pass", "approved"), "PR #9 ✓ approved"),
        (st("open", "", "changes"), "PR #9 · changes asked for"),
        (st("draft", "", ""), "PR #9 draft"),
        (st("open", "pass", ""), "PR #9 ✓"),
    ] {
        assert_eq!(status_short(&p).text, want);
    }
}

/// How a wide screen splits: the list a quarter, 30 columns at least,
/// the Session never wider than it can use; too narrow, no split
/// (view.go:881 widths, :917 sideWidth, :1017 canSplit).
#[test]
fn split_widths() {
    let mut m = model_of(vec![agent("a1", "one", State::Idle, 1)], 200, 50);
    assert_eq!(
        split::widths(&m),
        (71, 128),
        "the Session takes 128 at most"
    );
    m.cfg.side_width = 0.25;
    assert_eq!(split::widths(&m), (50, 149), "a share you set wins");
    m.w = 160;
    assert_eq!(split::widths(&m), (160, 0), "under 170, no split by itself");
    assert!(split::can_split(160) && split::can_split(120));
    assert!(!split::can_split(110));
    m.w = 200;
    m.cfg.list_only = true;
    assert_eq!(split::widths(&m), (200, 0), "the list alone, kept");
}

/// At 200 columns Agents and the picked agent's Session stand side by
/// side, the divider between, the box and its keys under the list;
/// #view list gives the screen back to the list.
#[test]
fn side_by_side() {
    let mut m = model_of(vec![agent("a1", "port the list", State::Idle, 1)], 200, 50);
    m.cfg.side_width = 0.25;
    let f = screen(&mut m);
    let body = &f[7..];
    assert!(
        body.iter().all(|l| l.chars().nth(50) == Some('│')),
        "{}",
        f.join("\n")
    );
    let last = f.last().unwrap();
    assert!(last.starts_with("  enter open"), "{last:?}");
    assert!(
        f.iter()
            .any(|l| l.contains("port the list has no conversation to show yet")),
        "the pane says what it has of it:\n{}",
        f.join("\n")
    );
    split::command(&mut m, "list");
    assert!(m.cfg.list_only);
    let f = screen(&mut m);
    assert!(f[8..].iter().all(|l| l.chars().nth(50) != Some('│')));
    assert!(m.pane.is_none());
}

/// The box's top edge offers what the next agent starts as, the chosen
/// one a chip, the rest numbered ⌥1…9 (shelf.go shelfLine).
#[test]
fn shelf_on_the_box() {
    let mut m = model_of(vec![agent("a1", "one", State::Idle, 1)], 120, 40);
    let f = screen(&mut m);
    let top = f.iter().find(|l| l.starts_with("╭─ new agent in")).unwrap();
    assert!(top.ends_with(" ─╮"), "{top:?}");
    let (l, hits) = super::shelf::shelf_line(&m, "/src/checkout", 200);
    assert!(!hits.is_empty());
    assert_eq!(hits[0].x0, 0);
    assert_eq!(hits.last().unwrap().x1, l.width());
    let (narrow, kept) = super::shelf::shelf_line(&m, "/src/checkout", 1);
    assert_eq!(kept.len(), 1, "too narrow, the chosen one stays");
    assert!(narrow.width() > 1);
}

/// The classic look: a heading per project with a dotted rule, a lone
/// worktree's agent tagged with it under its project, what's left before
/// Earlier has it at the row's end, the picked row marked ▍ (view.go
/// listLines, :2336 as of 2e4662ff; folders.go projectLine, treeTag).
#[test]
fn classic_look() {
    use super::landing::ListFolders;
    use rush_fleet::{Folder, GitState};
    const TREE: &str = "/src/checkout/.claude/worktrees/tax";
    let mut a = agent("tax", "fix tax rounding", State::Idle, 9);
    a.root = "/src/checkout".into();
    a.repo = TREE.into();
    a.seen = true;
    a.job.updated_at = NOW - 20 * MIN;
    let b = agent("b2", "add Apple Pay", State::Idle, 7);
    let mut m = model_of(vec![a, b], 120, 40);
    m.roots = vec!["/src/checkout".into(), "/src/checkout".into()];
    m.cfg.list_look = "classic".into();
    let mut f = Folder {
        root: "/src/checkout".into(),
        ..Folder::default()
    };
    f.trees.insert(
        TREE.into(),
        GitState {
            branch: "rush/tax".into(),
            changed: 3,
            ..GitState::default()
        },
    );
    m.kept::<ListFolders>()
        .by_root
        .insert("/src/checkout".into(), f);
    m.rebuild();
    m.sel = m.agents[1].key.clone();
    let s = screen(&mut m);
    let row = |want: &str| {
        s.iter()
            .find(|l| l.contains(want))
            .unwrap_or_else(|| panic!("no {want:?} in\n{}", s.join("\n")))
            .clone()
    };
    assert!(row(" ▾ Idle  2 ").trim_end().ends_with('─'));
    assert!(row("  checkout ┄").trim_end().ends_with('┄'));
    let tax = row("fix tax rounding");
    assert!(tax.contains("fix tax rounding ✻  ↳ tax"), "{tax:?}");
    assert!(
        tax.contains("  ±3 not committed"),
        "at the row's end: {tax:?}"
    );
    let pay = row("add Apple Pay");
    assert!(pay.starts_with("▍ ◦ add Apple Pay"), "{pay:?}");
    assert!(!s.iter().any(|l| l.contains("┃")), "no rail");
}

/// A click on the Session beside the list opens it; one on the list
/// still picks a row there.
#[test]
fn click_beside_the_list() {
    let mut m = model_of(
        vec![
            agent("a1", "port the list", State::Idle, 1),
            agent("b2", "port the pane", State::Idle, 2),
        ],
        200,
        50,
    );
    m.cfg.side_width = 0.25;
    screen(&mut m);
    let first = m.sel.clone();
    m.mouse(rush_draw::MouseKind::Down(rush_draw::Button::Left), 120, 20);
    assert_eq!(m.sel, first, "a click on the pane picks no row");
}
