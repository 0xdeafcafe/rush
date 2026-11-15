//! rush-ui-cmdbar's linkpop.rs tests: a GitHub link's popover drawn over
//! a whole frame, its keys and its mouse. testdata/linkpop_*.txt are the
//! box as drawn; UPDATE_GOLDEN=1 writes them. Nothing asks GitHub: what a
//! link's lookup would say is given (open_link_pop_with).

use super::linkmenu::open_link_menu;
use super::linkpop::*;
use crate::frame::dialog::cells;
use crate::frame::{Mouse, Out, Picker, Press};
use crate::model::Model;
use rush_draw::{Button, KeyMods, MouseKind};
use rush_gh::ghpr::{Check, Issue, Job, PR, Run};
use rush_ui_projects_machine::data::Data;
use rush_ui_projects_machine::prsheet::PrSheet;

const PR_URL: &str = "https://github.com/0xdeafcafe/rush/pull/482";
const RUN_URL: &str = "https://github.com/0xdeafcafe/rush/actions/runs/1234567890";

fn pr(state: &str) -> PR {
    PR {
        number: 482,
        title: "Fix the login redirect".into(),
        url: PR_URL.into(),
        state: state.into(),
        author: "sam".into(),
        head: "rush/login".into(),
        base: "main".into(),
        checks: vec![Check {
            name: "test".into(),
            workflow: "CI".into(),
            state: "passed".into(),
            ..Check::default()
        }],
        ..PR::default()
    }
}

fn model(w: usize) -> Model {
    let mut m = crate::new_model("t");
    (m.w, m.h) = (w, 30);
    m.kept::<Data>().frozen = true;
    m
}

/// Opens u's popover as a click on the cell (x, y) would.
fn click_open(m: &mut Model, u: &str, info: Option<Info>, at: (usize, usize)) {
    m.click_at = Some(at);
    assert!(open_link_pop_with(m, u, info), "no popover for {u}");
    m.click_at = None;
}

fn press(m: &mut Model, k: &str) -> Out {
    m.sheet_key(&Press::of(k).expect("a key"))
}

fn click(m: &mut Model, x: usize, y: usize) -> Out {
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(Button::Left),
        x: x as u16,
        y: y as u16,
        mods: KeyMods::NONE,
    })
}

/// The popover's box as the frame draws it.
fn the_box(m: &mut Model) -> String {
    let rows = m.frame();
    let o = m.over;
    assert!(o.on, "no box drawn");
    rows[o.y0..o.y1]
        .iter()
        .map(|l| cells(l, o.x0, o.x1).plain())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// got against testdata/name.txt; UPDATE_GOLDEN=1 writes it.
fn golden(name: &str, got: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/screens_tests/cmdbar/testdata")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no {}: run with UPDATE_GOLDEN=1; got:\n{got}\n",
            path.display()
        )
    });
    assert_eq!(got, want, "{name} drawn differently; got:\n{got}\n");
}

fn pop(m: &mut Model) -> &mut LinkPop {
    m.sheet_as::<LinkPop>().expect("the popover")
}

/// An open pull request's popover hangs under the link clicked: one line
/// of what it is, one of facts, its links out, then its actions folded;
/// r unfolds Review into rows to pick from, and esc folds it again.
#[test]
fn an_open_pr_with_review_unfolded() {
    let mut m = model(100);
    click_open(&mut m, PR_URL, Some(Info::Pr(pr("OPEN"))), (6, 4));
    assert_eq!(press(&mut m, "r"), Out::Draw);
    assert_eq!(pop(&mut m).open, Some(0), "r unfolds Review");
    let got = the_box(&mut m);
    assert_eq!((m.over.x0, m.over.y0), (4, 5), "under the link clicked");
    golden("linkpop_pr_open", &got);
    // esc folds it; esc again closes.
    assert_eq!(press(&mut m, "esc"), Out::Draw);
    assert_eq!(pop(&mut m).open, None);
    press(&mut m, "esc");
    assert!(m.sheet.is_none(), "esc with nothing unfolded closes it");
}

#[test]
fn a_merged_pr() {
    let mut m = model(100);
    let mut p = pr("MERGED");
    p.number = 480;
    p.title = "Tidy the list".into();
    p.head = "tidy".into();
    p.checks.clear();
    let u = "https://github.com/0xdeafcafe/rush/pull/480";
    click_open(&mut m, u, Some(Info::Pr(p)), (6, 4));
    golden("linkpop_pr_merged", &the_box(&mut m));
}

#[test]
fn a_failed_actions_run() {
    let mut m = model(100);
    let job = |id: u64, name: &str, conclusion: &str| Job {
        id,
        name: name.into(),
        status: "completed".into(),
        conclusion: conclusion.into(),
        url: format!("{RUN_URL}/job/{id}"),
    };
    let r = Run {
        id: 1234567890,
        repo: "0xdeafcafe/rush".into(),
        title: "Fix the login redirect".into(),
        workflow: "CI".into(),
        branch: "rush/login".into(),
        status: "completed".into(),
        conclusion: "failure".into(),
        url: RUN_URL.into(),
        jobs: vec![job(41, "lint", "success"), job(42, "test", "failure")],
    };
    click_open(&mut m, RUN_URL, Some(Info::Run(r)), (6, 4));
    golden("linkpop_run_failed", &the_box(&mut m));
}

#[test]
fn an_issue() {
    let mut m = model(100);
    let i = Issue {
        number: 12,
        title: "Links open the whole sheet".into(),
        url: "https://github.com/0xdeafcafe/rush/issues/12".into(),
        state: "OPEN".into(),
        reason: String::new(),
    };
    let u = "https://github.com/0xdeafcafe/rush/issues/12";
    click_open(&mut m, u, Some(Info::Issue(i)), (6, 4));
    golden("linkpop_issue", &the_box(&mut m));
}

/// On a narrow screen the facts are cut first (whole parts), then the
/// hints; the links flow onto another row.
#[test]
fn a_pr_on_a_narrow_screen() {
    let mut m = model(34);
    click_open(&mut m, PR_URL, Some(Info::Pr(pr("OPEN"))), (20, 4));
    golden("linkpop_pr_narrow", &the_box(&mut m));
}

/// From a key, with no click to hang under, it sits in the middle.
#[test]
fn from_a_key_it_is_centred() {
    let mut m = model(100);
    assert!(open_link_pop_with(&mut m, PR_URL, None));
    let b = the_box(&mut m);
    assert_eq!(m.over.x0, (100 - POP_WIDTH) / 2);
    assert!(b.starts_with("╭ #482 pull request ─"), "{b}");
    assert!(b.contains("▸ In rush   PR sheet"), "{b}");
    assert!(
        !b.contains("Merge"),
        "nothing to merge until GitHub says it's open"
    );
}

/// Merging and approving still ask, through the PR sheet's questions:
/// nothing goes to GitHub from the popover without a key there.
#[test]
fn merge_and_approve_ask_first() {
    let mut m = model(100);
    click_open(&mut m, PR_URL, Some(Info::Pr(pr("OPEN"))), (6, 4));
    press(&mut m, "m");
    assert_eq!(pop(&mut m).open, Some(1), "m unfolds Merge");
    assert!(m.confirm.is_none(), "unfolding acts on nothing");
    // squash is its first row, the cursor on it.
    press(&mut m, "enter");
    assert!(m.sheet.is_none(), "the popover gives way to the question");
    let c = m.confirm.take().expect("a question");
    assert_eq!(
        (c.question.as_str(), c.yes_text.as_str()),
        ("Merge #482 on GitHub?", "squash")
    );
    assert!(m.kept::<Data>().merging.is_empty(), "nothing merged");

    // rebase, by its letter once Merge is open, asks with rebase first.
    click_open(&mut m, PR_URL, Some(Info::Pr(pr("OPEN"))), (6, 4));
    press(&mut m, "m");
    press(&mut m, "r");
    let c = m.confirm.take().expect("a question");
    assert_eq!(c.yes_text, "rebase");
    let more: Vec<&str> = c.more.iter().map(|x| x.1.as_str()).collect();
    assert_eq!(more, ["squash", "merge commit"]);
    assert!(m.kept::<Data>().merging.is_empty(), "nothing merged");

    // approve asks too.
    click_open(&mut m, PR_URL, Some(Info::Pr(pr("OPEN"))), (6, 4));
    press(&mut m, "r");
    press(&mut m, "a");
    let c = m.confirm.take().expect("a question");
    assert_eq!(c.question, "Approve #482 on GitHub, as you?");
}

/// The whole PR sheet is one of its actions, under In rush.
#[test]
fn the_pr_sheet_is_under_in_rush() {
    let mut m = model(100);
    click_open(&mut m, PR_URL, Some(Info::Pr(pr("OPEN"))), (6, 4));
    press(&mut m, "i");
    press(&mut m, "enter");
    let s = m.sheet_as::<PrSheet>().expect("its sheet");
    assert_eq!((s.num, s.url.as_str()), (482, PR_URL));
}

/// A click on a group's row unfolds it; one outside the box closes it,
/// though esc would only fold.
#[test]
fn the_mouse_unfolds_and_a_click_outside_closes() {
    let mut m = model(100);
    click_open(&mut m, PR_URL, Some(Info::Pr(pr("OPEN"))), (6, 4));
    the_box(&mut m);
    let (bx, by) = m.sheet_at;
    // The rows: facts, blank, links, copy, then Review.
    assert_eq!(click(&mut m, bx + 1, by + 4), Out::Draw);
    assert_eq!(pop(&mut m).open, Some(0), "a click on Review unfolds it");
    the_box(&mut m);
    click(&mut m, 90, 25);
    assert!(m.sheet.is_none(), "a click outside closes it");
    assert!(m.confirm.is_none());
}

/// Any other link keeps the link menu.
#[test]
fn other_links_keep_the_menu() {
    let mut m = model(100);
    assert!(open_link_menu(&mut m, "https://example.com/x"));
    assert!(m.sheet_as::<Picker>().is_some());
    assert!(open_link_menu(&mut m, PR_URL));
    assert!(m.sheet_as::<LinkPop>().is_some(), "a GitHub link's popover");
}
