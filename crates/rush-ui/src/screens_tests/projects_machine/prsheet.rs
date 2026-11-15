//! The tests of rush-ui-projects-machine's prsheet.rs that need the whole UI.

use crate::frame::sheet::Sheet;
use crate::frame::{Out, Press};
use rush_core::text::Line;
use rush_gh::ghpr::{Comment, PR};
use rush_ui_projects_machine::data::*;
use rush_ui_projects_machine::prsheet::*;

use rush_gh::ghpr::{Check, Review};

fn pr() -> PR {
    PR {
        number: 42,
        title: "Fix the parser".into(),
        url: "https://github.com/o/r/pull/42".into(),
        state: "OPEN".into(),
        author: "sam".into(),
        head: "fix/parser".into(),
        base: "main".into(),
        decision: "REVIEW_REQUIRED".into(),
        waiting: vec!["you".into(), "kim".into()],
        reviews: vec![Review {
            who: "lee".into(),
            state: "CHANGES_REQUESTED".into(),
            body: "<!-- bot -->tabs please".into(),
        }],
        checks: vec![
            Check {
                name: "test".into(),
                workflow: "CI".into(),
                state: "passed".into(),
                ..Check::default()
            },
            Check {
                name: "lint".into(),
                state: "failed".into(),
                ..Check::default()
            },
        ],
        body: "It fixes **the** parser.".into(),
        added: 10,
        removed: 2,
        files: 3,
        comments: 1,
        ..PR::default()
    }
}

fn sheet(read: bool) -> PrSheet {
    PrSheet {
        root: "/src/r".into(),
        num: 42,
        url: String::new(),
        top: 0,
        last: pr(),
        comments: vec![Comment {
            who: "kim".into(),
            body: "looks good".into(),
            at: 0,
        }],
        said: String::new(),
        read,
        writing: String::new(),
        input: Vec::new(),
        pos: 0,
        sending: String::new(),
    }
}

fn text(l: &[Line]) -> String {
    l.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

#[test]
fn says_it_whole() {
    let mut m = crate::new_model("t");
    m.w = 160;
    m.now = 2 * 3_600_000;
    let mut s = sheet(true);
    let t = text(&s.body(&m, 120, 60));
    for want in [
        "#42 Fix the parser  open",
        "@sam wants fix/parser into main",
        "REVIEW",
        "✗ lee asked for changes · tabs please",
        "◌ you are asked to review it",
        "◌ kim asked, not reviewed yet",
        "CHECKS  1 of 2 passed · 1 failed",
        "✗ lint",
        "✓ CI / test",
        "DESCRIPTION",
        "the",
        "CONVERSATION  1",
        "kim · 2h ago",
        "looks good",
        "c comment",
        "a approve",
        "m merge…",
    ] {
        assert!(t.contains(want), "lacks {want:?}:\n{t}");
    }
    assert!(
        t.find("✗ lint").unwrap() < t.find("✓ CI / test").unwrap(),
        "failed first"
    );
    assert!(!t.contains("bot"), "what bots hide stays hidden");
    let t = text(&sheet(false).body(&m, 120, 60));
    assert!(t.contains("asking GitHub…"));
    // a asks first.
    s.key(&mut m, &Press::of("a").unwrap());
    assert_eq!(
        m.confirm.as_ref().map(|c| c.question.as_str()),
        Some("Approve #42 on GitHub, as you?")
    );
}

#[test]
fn your_own_isnt_reviewed_and_a_closed_one_isnt_acted_on() {
    let mut m = crate::new_model("t");
    m.w = 160;
    let mut s = sheet(true);
    s.last.mine = true;
    let t = text(&s.body(&m, 120, 60));
    assert!(t.contains("you want fix/parser into main"));
    assert!(!t.contains("a approve") && t.contains("m merge…"));
    assert_eq!(s.key(&mut m, &Press::of("x").unwrap()), Out::Draw);
    assert!(s.writing.is_empty(), "no asking for changes on your own");
    s.last.state = "MERGED".into();
    assert!(!text(&s.body(&m, 120, 60)).contains("c comment"));
    s.key(&mut m, &Press::of("c").unwrap());
    assert!(s.writing.is_empty());
}

#[test]
fn writing_a_comment() {
    let mut m = crate::new_model("t");
    m.w = 160;
    let mut s = sheet(true);
    s.key(&mut m, &Press::of("c").unwrap());
    assert_eq!(s.writing, "comment");
    for c in ["h", "i"] {
        s.key(&mut m, &Press::of(c).unwrap());
    }
    assert_eq!(s.input, ['h', 'i']);
    let t = text(&s.body(&m, 120, 60));
    assert!(
        t.contains("comment ❯ hi") && t.contains("enter post it"),
        "{t}"
    );
    s.key(&mut m, &Press::of("esc").unwrap());
    assert!(s.writing.is_empty() && s.input.is_empty());
    assert_eq!(s.key(&mut m, &Press::of("esc").unwrap()), Out::Close);
}

#[test]
fn merging_says_what_it_does() {
    assert_eq!(
        merge_detail(&pr()),
        "fix/parser → main\n+10 −2 · 3 files · 1 comment\n\
         checks: 1 failed, 0 still running, 1 passed · review: needed, not given\n\n\
         squash: one commit on main, titled \"Fix the parser\"\n\
         rebase: each of its commits put on main in turn\n\
         merge commit: its commits as they are, joined by a merge"
    );
    let mut m = crate::new_model("t");
    ask_merge(&mut m, sheet(true).at(), &pr());
    let c = m.confirm.as_ref().expect("asks");
    assert_eq!(c.question, "Merge #42 on GitHub?");
    assert_eq!(c.yes_text, "squash");
    assert_eq!(
        c.more
            .iter()
            .map(|(k, t, _)| format!("{k} {t}"))
            .collect::<Vec<_>>(),
        ["r rebase", "M merge commit"]
    );
    m.kept::<Data>()
        .merge_failed
        .insert("/src/r#42".into(), ("rules".into(), true));
    ask_merge(&mut m, sheet(true).at(), &pr());
    let c = m.confirm.as_ref().unwrap();
    assert!(c.detail.ends_with("\n\nlast try failed: rules"));
    assert_eq!(c.more.len(), 3, "squash as admin");
}

/// A link to a pull request in any repository opens its popover, not its
/// whole sheet: the sheet is In rush's PR sheet, read from the link, and
/// merging from there only asks (TestPRLinkOpensInRush, Rust-only since
/// the popover). Nothing asks GitHub: lookups are off, so what the link
/// would say is given.
#[test]
fn a_pr_link_opens_in_rush() {
    use crate::screens::cmdbar::linkkinds::open_known_link;
    use crate::screens::cmdbar::linkpop::{Info, LinkPop, open_link_pop_with};
    let u = "https://github.com/acme/linkopens/pull/33";
    let p = PR {
        number: 33,
        title: "Emoji fix".into(),
        url: u.into(),
        state: "OPEN".into(),
        head: "emoji".into(),
        base: "main".into(),
        ..PR::default()
    };
    let mut m = crate::new_model("t");
    (m.w, m.h) = (160, 50);
    m.kept::<Data>().frozen = true;
    assert_eq!(open_known_link(&mut m, u), Some(Out::Draw));
    assert!(m.sheet_as::<LinkPop>().is_some(), "its popover");
    assert!(open_link_pop_with(&mut m, u, Some(Info::Pr(p))));
    // In rush, then its PR sheet.
    m.sheet_key(&Press::of("i").unwrap());
    m.sheet_key(&Press::of("enter").unwrap());
    let s = m.sheet_as::<PrSheet>().expect("its sheet");
    assert_eq!((s.url.as_str(), s.num), (u, 33));
    let s = m.sheet.take().unwrap();
    let s: &dyn std::any::Any = s.as_ref();
    let s = s.downcast_ref::<PrSheet>().unwrap();
    let pr = s.last.clone();
    assert_eq!(pr.title, "Emoji fix");
    assert!(
        text(&[s.keys(&pr, 200)]).contains("m merge…"),
        "a linked open pull request can be merged"
    );
    // Any other link is left to the terminal.
    assert!(open_known_link(&mut m, "https://example.com/x").is_none());
}
