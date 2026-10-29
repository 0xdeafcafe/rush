//! What rush says: Go's messages_test.go (TestSayKinds,
//! TestNewsStaysOffTheLine, TestLogSheet, TestSaidInEveryLayout) and
//! stream_test.go's TestNewsUnderTheFeed.

use rush_core::text::Line;
use rush_draw::{Button, KeyMods, MouseKind};

use super::messages::*;
use super::*;
use crate::model::Model;

fn model(w: usize, h: usize) -> Model {
    let mut m = Model::new("0.1.0");
    m.clock = Some(1_793_190_896_789);
    m.now = rush_core::time::now();
    m.w = w;
    m.h = h;
    m
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

fn plain(rows: &[(Line, String)]) -> String {
    rows.iter()
        .map(|(l, _)| l.plain())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A refusal is plain and goes; a failure is red and stays until a key.
#[test]
fn say_kinds() {
    let mut m = model(120, 40);
    m.flash_err("select an agent first");
    assert!(m.status_shown(), "a refusal shows");
    assert!(!m.flash.as_ref().unwrap().2, "and isn't a failure");
    m.now += FLASH_FOR + 1000;
    assert!(!m.status_shown(), "a refusal goes after a few seconds");
    m.fail("couldn't switch: no such account");
    m.now += 3_600_000;
    assert!(m.status_shown(), "a failure stays until a key");
    m.press_key(&Press::of("down").unwrap());
    assert!(!m.status_shown(), "a key takes a failure off the line");
    let said = m.said();
    assert_eq!(said.len(), 2, "the log keeps both");
    assert_eq!(said[1].kind, SayKind::Failed);
}

/// News never takes the line under the box; it's in the header's count
/// and under the feed until seen, and a click goes where it points.
#[test]
fn news_stays_off_the_line() {
    let mut m = model(120, 40);
    m.flash("copied");
    m.news(
        "kept “fix the status bar”'s worktree: 1 commit not pushed or in main",
        &format!("{NEWS_REF_PROJECTS}/src/rush"),
    );
    assert_eq!(m.flash.as_ref().unwrap().0, "copied", "news took the line");
    assert!(m.news_head().unwrap().plain().contains("1 from rush"));
    let rows = m.news_dock(44);
    let got = plain(&rows);
    assert!(got.contains("what rush did"), "{got}");
    assert!(
        got.contains("kept “fix the status bar”'s worktree"),
        "{got}"
    );
    m.click_news(&rows[1].1.clone());
    assert_eq!(m.place, PlaceId::Projects, "a click opens Projects");
    assert!(m.unseen_news().is_empty(), "and sees it");
    assert!(
        m.news_dock(44).is_empty(),
        "seen news leaves the foot of the list"
    );
    assert!(m.news_head().is_none());
}

/// The column beside the conversation's part: a heading, each item and
/// its reason under it, every row a click.
#[test]
fn side_news_rows() {
    let mut m = model(120, 40);
    m.news("removed the worktrees of done work: “a” · freed 2 GB", "");
    let rows = m.side_news(44, |t, meta| {
        let mut l = crate::style::line([crate::style::plain(t)]);
        crate::style::join(&mut l, meta);
        l
    });
    let got = plain(&rows);
    assert!(got.contains("What rush did1 · #log for all"), "{got}");
    assert!(
        got.contains("  removed the worktrees of done work"),
        "{got}"
    );
    assert!(got.contains("    “a” · freed 2 GB"), "{got}");
    assert_eq!(rows[0].1, format!("{SIDE_NEWS_REF}all"));
    m.click_news(&rows[1].1.clone());
    assert!(
        m.side_news(44, |t, _| crate::style::line([crate::style::plain(t)]))
            .is_empty()
    );
}

/// Noted things are in the log only.
#[test]
fn noted_is_logged_seen() {
    let mut m = model(120, 40);
    m.noted("queued messages sent: fix auth has them");
    assert!(m.unseen_news().is_empty());
    assert_eq!(m.said().len(), 1);
    assert!(m.flash.is_none(), "nor on the line");
}

/// #log opens the log, newest first; closing it sees the news; it keeps
/// the last 50.
#[test]
fn log_sheet() {
    let mut m = model(120, 40);
    m.flash("one");
    m.news("two: happened", "");
    m.command("#log");
    let body = screen(&mut m).join("\n");
    let (two, one) = (body.find("two: happened"), body.find("· one"));
    assert!(
        two.is_some() && one.is_some() && two < one,
        "newest first:\n{body}"
    );
    assert!(body.contains("the last 2 things rush said"), "{body}");
    m.press_key(&Press::of("esc").unwrap());
    assert!(m.sheet.is_none(), "esc closes it");
    assert!(m.unseen_news().is_empty(), "and sees the news");
    for _ in 0..LOG_KEPT + 5 {
        m.flash("x");
    }
    assert_eq!(m.said().len(), LOG_KEPT);
}

/// What rush says shows once, on the line under the list.
#[test]
fn said_once() {
    let mut m = model(220, 40);
    m.flash_err("select an agent first");
    let out = screen(&mut m).join("\n");
    assert_eq!(out.matches("select an agent first").count(), 1, "{out}");
}

/// What rush did on its own sits at the foot of the list, and a click on
/// one of its rows sees it.
#[test]
fn news_under_the_feed() {
    let mut m = model(200, 50);
    m.news("3 updates found: Settings › Updates has them", "");
    let rows = screen(&mut m);
    let head = rows.iter().position(|r| r.contains("── what rush did "));
    let item = rows
        .iter()
        .position(|r| r.contains("◇ 3 updates found · Settings"));
    assert!(
        head.is_some() && item == head.map(|h| h + 1),
        "{}",
        rows.join("\n")
    );
    let y = item.unwrap();
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(Button::Left),
        x: 4,
        y: y as u16,
        mods: KeyMods::NONE,
    });
    assert!(m.unseen_news().is_empty(), "a click sees it");
}
