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

// An agent goes by its session's title, a long one cut with …. Hovered,
// the row keeps its layout: the name scrolls in its own room and the
// text stays where it was. Renamed, its posts show the new title.
#[test]
fn long_handles_cut_and_hover_keeps_the_row() {
    use rush_room::handle::{Names, set_known};
    let mut m = feed_model();
    m.now = m.clock.unwrap();
    set_known(Names::new([(
        "s1",
        "Terminal Decline: Shell Autocomplete Ideas",
    )]));
    let full = "@terminal-decline-shell-autocomplete-ideas";
    let cut = "@terminal-decline-shell…";
    let last = |m: &mut Model| rows(m, 120, 8).last().unwrap().clone();
    let quiet = last(&mut m);
    assert!(quiet.contains(cut) && !quiet.contains(full), "{quiet}");
    m.hover = format!("{KEY_PREFIX}a/1");
    let hovered = last(&mut m);
    assert_eq!(cells(&hovered), cells(&quiet), "{hovered}");
    assert_eq!(
        hovered.find("-race"),
        quiet.find("-race"),
        "the text stays put:\n{quiet}\n{hovered}"
    );
    assert!(!hovered.contains(full), "{hovered}");
    // The pointer leaves: cut again.
    m.hover.clear();
    assert_eq!(last(&mut m), quiet);
    set_known(Names::new([("s1", "Build cache")]));
    let renamed = last(&mut m);
    assert!(renamed.contains("@build-cache "), "{renamed}");
    set_known(Names::default());
}

// A hovered name too long for its room rests at its start, scrolls left
// about nine characters a second until its end shows, rests, then starts
// again; one that fits never moves.
#[test]
fn marquee_scrolls_a_cut_name() {
    use rush_ui_social::stream::marquee;
    use std::time::Duration;
    let name = "@terminal-decline-shell-autocomplete-ideas";
    let at = |ms: u64| marquee(name, 24, Duration::from_millis(ms));
    let ms = |n: u64| Some(Duration::from_millis(n));
    // The first second: cut, as it was.
    assert_eq!(at(0), ("@terminal-decline-shell…".to_string(), ms(1000)));
    assert_eq!(at(900), ("@terminal-decline-shell…".to_string(), ms(100)));
    // Then a character every 110ms.
    assert_eq!(at(1000), ("terminal-decline-shell-a".to_string(), ms(110)));
    assert_eq!(at(1440), ("inal-decline-shell-autoc".to_string(), ms(110)));
    // At its end, it rests: 18 steps take it there.
    let end = 1000 + 18 * 110;
    assert_eq!(at(end), ("shell-autocomplete-ideas".to_string(), ms(1000)));
    assert_eq!(at(end + 500).0, "shell-autocomplete-ideas");
    // Then back to the start, round again.
    let cycle = end + 1000;
    assert_eq!(at(cycle).0, "@terminal-decline-shell…");
    assert_eq!(at(cycle + 1440).0, "inal-decline-shell-autoc");
    // Never more than its room.
    for t in (0..cycle).step_by(37) {
        assert_eq!(cells(&at(t).0), 24, "{t}");
    }
    // A name that fits stays still and asks for no frame.
    assert_eq!(
        marquee("@docs", 24, Duration::from_millis(1440)),
        ("@docs".to_string(), None)
    );
}

// Nothing hovered and nothing new: the dock asks for no frame, so an idle
// rush stays idle. A hovered cut name asks for the next one.
#[test]
fn no_tick_unless_a_cut_name_is_hovered() {
    use rush_room::handle::{Names, set_known};
    let mut m = feed_model();
    let now = m.clock.unwrap();
    for p in &mut m.kept::<Stream>().posts {
        p.at = now - 3_600_000;
    }
    m.now = now;
    set_known(Names::new([(
        "s1",
        "Terminal Decline: Shell Autocomplete Ideas",
    )]));
    let before = m.wake_asked();
    m.stream_lines(120, 8);
    assert_eq!(m.wake_asked(), before, "idle");
    m.hover = format!("{KEY_PREFIX}a/0");
    m.stream_lines(120, 8);
    assert_eq!(m.wake_asked(), before, "a name that fits never moves");
    m.hover = format!("{KEY_PREFIX}a/1");
    m.stream_lines(120, 8);
    assert!(m.wake_asked().is_some(), "a cut name scrolls");
    if before.is_none() {
        let due = m.wake_asked().unwrap() - std::time::Instant::now();
        assert!(due <= std::time::Duration::from_millis(1000), "{due:?}");
    }
    set_known(Names::default());
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
