//! rush-ui-social's `register`, as the whole UI has it: the group's #
//! commands.

use super::feed::feed_model;
use crate::frame::Out;
use rush_ui_social::feed::FeedSheet;
use rush_ui_social::feed::ModelFeed;

// The group's # commands, as Go's keys.go:1040-1080 names them.
#[test]
fn commands() {
    let m = feed_model();
    for name in ["feed", "room", "broadcast", "update", "reload"] {
        assert!(
            m.ext.commands.iter().any(|c| c.name == name),
            "#{name} isn't registered"
        );
    }
}

// The feed sheet's keys: what each does, as community.go:523 has them.
#[test]
fn feed_keys() {
    let mut m = feed_model();
    m.open_feed("");
    type Check = fn(&FeedSheet) -> bool;
    let table: &[(&str, Check)] = &[
        ("g", |s| s.graph),
        ("g", |s| !s.graph),
        ("n", |s| s.composing && s.reply_to.is_empty()),
        ("esc", |s| !s.composing),
        ("enter", |s| s.composing && !s.reply_to.is_empty()),
        ("esc", |s| !s.composing),
        ("r", |s| s.composing),
        ("esc", |s| !s.composing),
        ("up", |s| !s.follow),
        ("down", |s| s.follow),
    ];
    for (k, ok) in table {
        let out = m.press(k);
        assert_ne!(out, Out::Pass, "{k} isn't the sheet's");
        let s = m.sheet_as::<FeedSheet>().expect("still open");
        assert!(ok(s), "after {k}: {s:?}");
    }
    assert_eq!(m.press("esc"), Out::Close);
    assert!(m.sheet.is_none());
    // Opened again, it's where it was.
    m.open_feed("");
    assert!(m.sheet_as::<FeedSheet>().is_some());
}
