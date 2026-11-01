//! The tests of rush-ui-social's dock.rs that need the whole UI.

use crate::frame::{Mouse, Out};
use crate::model::Model;
use crate::style::*;
use rush_draw::{Button, MouseKind};
use rush_ui_social::dock::*;
use rush_ui_social::ink::cells;
use rush_ui_social::stream::Stream;

use super::feed::feed_model;

fn rows(m: &mut Model, w: usize, room: usize) -> Vec<String> {
    m.stream_lines(w, room)
        .iter()
        .map(|(l, _)| l.plain())
        .collect()
}

// Under a title rule, oldest first, newest at the bottom, each row
// keyed to its post; nothing while the feed is off, and nothing in
// too little room (stream_test.go TestStreamLines).
#[test]
fn lines() {
    let mut m = feed_model();
    m.now = m.clock.unwrap();
    let ls = m.stream_lines(60, 40);
    assert!(
        ls[0].0.plain().starts_with("── the feed 🐓 ──"),
        "{:?}",
        ls[0].0.plain()
    );
    assert_eq!(ls.last().unwrap().1, format!("{KEY_PREFIX}a/1"));
    assert_eq!(ls.len(), 4);
    for (l, _) in &ls[1..] {
        assert_eq!(l.width(), 59, "{:?}", l.plain());
    }
    let text: Vec<String> = ls.iter().map(|(l, _)| l.plain()).collect();
    assert!(
        text[1].contains("go test hangs") && text[3].contains("-race"),
        "{text:?}"
    );
    // Room for two rows: the title and the newest.
    let two = rows(&mut m, 60, 2);
    assert!(two.len() == 2 && two[1].contains("-race"), "{two:?}");
    assert!(rows(&mut m, 60, 1).is_empty());
    assert!(rows(&mut m, 23, 10).is_empty());
    m.kept::<Stream>().on = Some(false);
    assert!(rows(&mut m, 60, 10).is_empty());
}

// Quiet, it's faded; a new post or the pointer lights it.
#[test]
fn fades_when_quiet() {
    let mut m = feed_model();
    let now = m.clock.unwrap();
    for p in &mut m.kept::<Stream>().posts {
        p.at = now - 3_600_000;
    }
    m.now = now;
    let fg = |m: &mut Model| {
        let ls = m.stream_lines(60, 8);
        ls[0].0.spans[1].style.fg
    };
    let quiet = fg(&mut m);
    assert_ne!(quiet, TEXT);
    m.hover = format!("{KEY_PREFIX}a/0");
    assert_eq!(fg(&mut m), TEXT);
    m.hover.clear();
    m.kept::<Stream>().posts[2].at = now - 1000;
    assert_eq!(fg(&mut m), TEXT);
}

// The ticker stops once the end is in view, and never splits a wide
// char (stream_test.go TestTicker).
#[test]
fn ticker_steps() {
    let cases: &[(&str, usize, i64, &str, bool)] = &[
        ("abcdefghij", 4, 0, "abcd", true),
        ("abcdefghij", 4, 3, "defg", true),
        ("abcdefghij", 4, 6, "ghij", false),
        ("abcdefghij", 4, 60, "ghij", false),
        ("日本語のテキスト", 5, 1, "本語", true),
        ("short", 12, 5, "short", false),
    ];
    for &(text, w, step, want, more) in cases {
        let (got, m) = ticker(text, w, step);
        assert_eq!((got.as_str(), m), (want, more), "{text} {w} {step}");
        assert!(cells(&got) <= w);
    }
}

// Docked at the very foot of the list with the list behind it; a
// click on a post opens the feed at it.
#[test]
fn docks_and_clicks() {
    let mut m = feed_model();
    let n = 12;
    let agents: Vec<rush_core::agent::Agent> = (0..n)
        .map(|i| rush_core::agent::Agent {
            key: format!("k{i}"),
            display_name: format!("agent {i}"),
            ..Default::default()
        })
        .collect();
    m.set_fleet(crate::worker::Fleet {
        agents,
        roots: vec![String::new(); n],
        ..Default::default()
    });
    let f: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    let y = f
        .iter()
        .position(|l| l.contains("the feed 🐓"))
        .expect("docked");
    let last = f.iter().rposition(|l| l.contains("-race")).expect("newest");
    assert!(last > y, "{f:#?}");
    assert_eq!(m.row_keys[y - m.list_top], KEY_PREFIX);
    let e = Mouse {
        kind: MouseKind::Down(Button::Left),
        x: 5,
        y: last as u16,
        mods: rush_draw::KeyMods::NONE,
    };
    assert_ne!(mouse(&mut m, &e), Out::Pass);
    let s = m
        .sheet_as::<rush_ui_social::feed::FeedSheet>()
        .expect("the feed opened");
    assert_eq!(s.picked, "a/1");
}
