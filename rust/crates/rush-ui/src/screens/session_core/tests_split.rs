//! Agents and an opened Session side by side, at the widths Go's tests
//! and a dogfooding terminal use (80, 120, 160, 200): which layout each
//! view setting gives, what enter, esc, ←, { }, tab, ctrl+6 and shift+← →
//! do to it, and the header's look beside the list and alone. After Go's
//! internal/ui/view.go (widths :881, chatAlone :985, listOnly :991,
//! splitAgain :1000, sessionOnly :1042, stepSplit :951), hosted.go:134
//! (toggleList), rushmode.go:5207 (leavePane, leaveChat), keys.go:356
//! (esc in the list) and :86 (a Session filling a narrow screen has the
//! keys), and rushmode.go:2855 paneHeader with headerwash.go.

use rush_core::agent::State;
use rush_core::text::{Color, Line, Span, Style};

use super::host::Fake;
use super::tests::{agent, open, press, screen};
use crate::model::Model;
use crate::screens::list_extras::split;
use crate::style::*;

/// One agent, its Session opened, w×h, view your layout ("" as split).
fn opened(w: usize, h: usize, view: &str) -> Model {
    let fake = Fake::new();
    let mut m = open(agent("plain", State::Idle), &fake, w, h);
    m.full = false; // opened as enter opens it
    if !view.is_empty() {
        m.cfg.set_view(view);
    }
    m.roots = vec![String::new()];
    m.rebuild();
    m
}

/// The frame's rows as text, the rows' ends trimmed.
fn rows(m: &mut Model) -> Vec<String> {
    screen(m)
        .into_iter()
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// Whether column x of every row under the header is the divider.
fn divider_at(m: &mut Model, x: usize) -> bool {
    let f = rows(m);
    f[7..f.len() - 1]
        .iter()
        .all(|l| l.chars().nth(x) == Some('│'))
}

/// Moves the clock on, past esc twice's second.
fn later(m: &mut Model) {
    m.now += 2_000;
    m.clock = Some(m.now);
}

/// Opened, a Session goes beside the list wherever there's room for
/// both, and has the screen where there isn't, or when the Session alone
/// is your layout (widths, chatAlone).
#[test]
fn opened_beside_the_list_by_view() {
    // (view, width, side share, (list, pane))
    let cases: &[(&str, usize, f64, (usize, usize))] = &[
        ("split", 80, 0.0, (0, 80)),
        ("split", 120, 0.0, (33, 86)),
        ("split", 160, 0.0, (44, 115)),
        ("split", 200, 0.0, (56, 143)),
        // Dogfooding's config: the list a quarter.
        ("split", 80, 0.25, (0, 80)),
        ("split", 120, 0.25, (30, 89)),
        ("split", 160, 0.25, (40, 119)),
        ("split", 200, 0.25, (50, 149)),
        // No layout picked yet is the split.
        ("", 120, 0.0, (33, 86)),
        // The Session alone: the screen, at every width.
        ("agent", 80, 0.0, (0, 80)),
        ("agent", 120, 0.0, (0, 120)),
        ("agent", 200, 0.0, (0, 200)),
        // Agents alone: a Session opened from it comes beside it.
        ("list", 80, 0.0, (0, 80)),
        ("list", 120, 0.0, (33, 86)),
        ("list", 200, 0.0, (56, 143)),
    ];
    for &(view, w, share, want) in cases {
        let mut m = opened(w, 40, view);
        m.cfg.side_width = share;
        assert_eq!(split::widths(&m), want, "{view} at {w}, share {share}");
        if want.0 > 0 {
            assert!(divider_at(&mut m, want.0), "{view} at {w}: the divider");
        }
    }
}

/// Not opened, the list has the screen under 170 columns; at 170 and
/// wider the picked agent's Session stands beside it, unless Agents
/// alone is your layout (autoSplit).
#[test]
fn not_opened_the_list() {
    for (view, w, want) in [
        ("split", 80, (80, 0)),
        ("split", 120, (120, 0)),
        ("split", 160, (160, 0)),
        ("split", 200, (56, 143)),
        ("list", 200, (200, 0)),
        ("agent", 200, (200, 0)),
    ] {
        let mut m = opened(w, 40, view);
        m.session = None;
        assert_eq!(split::widths(&m), want, "{view} at {w}");
    }
}

/// Opened beside the list, the Session has the keys: the list's box
/// waits and says how back, its keys say esc · ←, and the Session's rule
/// is lit orange. esc gives the list the keys, the Session staying beside
/// it as it was, its box saying how to give it them again; enter does.
/// esc in the list then lets it go (leavePane, leaveChat, keys.go:370).
#[test]
fn keys_between_the_two_sides() {
    let mut m = opened(120, 40, "split");
    let f = rows(&mut m);
    let foot = f.last().unwrap();
    assert!(foot.starts_with("  esc · ← back to Agents"), "{foot:?}");
    assert!(
        f.iter().any(|l| l.contains("esc or ← to come back")),
        "the list's box says how back:\n{}",
        f.join("\n")
    );
    assert!(
        !f.iter().any(|l| l.contains("back to the list")),
        "beside the list nothing says it's behind"
    );

    press(&mut m, "esc");
    assert!(m.session.is_none(), "the list has the keys");
    assert!(m.pane.is_some(), "the Session stays beside it");
    assert_eq!(split::widths(&m), (33, 86));
    let f = rows(&mut m);
    assert!(
        f.iter()
            .any(|l| l.contains("enter or → to talk to this agent")),
        "{}",
        f.join("\n")
    );
    assert!(f.last().unwrap().contains("enter · → type here"));
    assert!(f.last().unwrap().starts_with("  enter open"));

    later(&mut m);
    press(&mut m, "enter");
    assert!(m.session.is_some(), "enter gives it the keys again");
    assert_eq!(split::widths(&m), (33, 86));

    later(&mut m);
    press(&mut m, "esc");
    later(&mut m);
    press(&mut m, "esc");
    assert!(m.pane.is_none() || split::widths(&m).1 == 0);
    assert_eq!(split::widths(&m), (120, 0), "the list alone again");
}

/// ← with nothing typed, { } and tab go back to the list as esc does
/// (switchFocus, paneKey's left).
#[test]
fn back_to_the_list_keys() {
    for k in ["left", "{", "}", "tab"] {
        let mut m = opened(120, 40, "split");
        press(&mut m, k);
        assert!(m.session.is_none(), "{k}");
        assert!(m.pane.is_some(), "{k}: beside the list still");
        press(&mut m, "tab");
        assert!(m.session.is_some(), "tab goes back in");
    }
}

/// ctrl+o (reply) and ⌘↓ go from the list into the picked agent's
/// Session box; ctrl+o with text typed too (keys.go:421, :456).
#[test]
fn ctrl_o_and_cmd_down_go_in() {
    for (k, typed) in [("ctrl+o", ""), ("ctrl+o", "half"), ("super+down", "")] {
        let mut m = opened(120, 40, "split");
        press(&mut m, "esc");
        assert!(m.session.is_none());
        m.input.set(typed);
        press(&mut m, k);
        assert!(m.session.is_some(), "{k} with {typed:?}");
    }
}

/// At 170 and wider the Session beside the list stays when the list
/// takes the keys and esc is pressed there: it's beside the list by
/// itself (autoSplit).
#[test]
fn wide_keeps_the_pane() {
    let mut m = opened(200, 50, "split");
    press(&mut m, "esc");
    later(&mut m);
    press(&mut m, "esc");
    assert!(m.session.is_none());
    assert_eq!(split::widths(&m), (56, 143));
}

/// Filling a narrow screen, esc goes back to the list alone.
#[test]
fn narrow_esc_is_the_list() {
    let mut m = opened(80, 24, "split");
    assert_eq!(split::widths(&m), (0, 80));
    let f = rows(&mut m);
    assert!(
        f.iter().any(|l| l.contains("esc back to the list")),
        "alone, the header says the list is behind:\n{}",
        f.join("\n")
    );
    press(&mut m, "esc");
    assert!(m.session.is_none() && m.pane.is_none());
    assert_eq!(split::widths(&m), (80, 0));
}

/// ctrl+6: from the split the Session takes the screen; again, the two
/// side by side, the list with the keys; the layout kept isn't changed.
/// Too narrow for both, the list takes the screen (toggleList).
#[test]
fn ctrl_6_between_split_and_alone() {
    let mut m = opened(120, 40, "split");
    press(&mut m, "ctrl+6");
    assert!(m.full && m.session.is_some());
    assert_eq!(split::widths(&m), (0, 120));
    press(&mut m, "ctrl+6");
    assert!(!m.full && m.session.is_none() && m.pane.is_some());
    assert_eq!(split::widths(&m), (33, 86));
    assert_eq!(m.cfg.view, "split", "the layout kept");

    let mut m = opened(80, 24, "split");
    press(&mut m, "ctrl+6");
    assert!(m.session.is_none());
    assert_eq!(split::widths(&m), (80, 0), "no room: the list");
}

/// The Session alone as your layout: opened, it has the screen; ctrl+6
/// gives it to the list, as a peek, esc going back to it (chatAlone,
/// leaveChat's peekFrom).
#[test]
fn session_alone_layout() {
    let mut m = opened(160, 45, "agent");
    assert_eq!(split::widths(&m), (0, 160));
    press(&mut m, "ctrl+6");
    assert!(m.session.is_none());
    assert_eq!(split::widths(&m), (160, 0));
    assert!(crate::screens::list_extras::peek::peeking(&m));
    let f = rows(&mut m);
    assert!(
        f.last().unwrap().contains("back to tidy the parser"),
        "{}",
        f.join("\n")
    );
    later(&mut m);
    press(&mut m, "esc");
    // The peek is over: the Session you came from opens again (here
    // there's nothing to read it from, so the list stays).
    assert!(!crate::screens::list_extras::peek::peeking(&m));
}

/// shift+← past the list's least share gives the Session the screen, and
/// keeps it so; shift+→ from there brings the list back beside it; within
/// the ends they move the edge two columns (stepSplit, setSideWidth).
#[test]
fn shift_arrows_step_the_split() {
    let mut m = opened(120, 40, "split");
    m.cfg.side_width = 0.25; // 30 columns: the least
    press(&mut m, "shift+left");
    assert!(m.full);
    assert_eq!(m.cfg.view, "agent");
    assert_eq!(split::widths(&m), (0, 120));
    press(&mut m, "shift+right");
    assert!(!m.full);
    assert_eq!(m.cfg.view, "split");
    assert_eq!(split::widths(&m), (30, 89));
    assert!(m.session.is_some(), "still open, beside the list");

    press(&mut m, "esc");
    press(&mut m, "shift+right");
    assert_eq!(split::widths(&m), (32, 87), "the edge two columns right");
    // Past the most the list can have (min(¾, w-1-84) = 35): the list
    // alone, kept.
    press(&mut m, "shift+right");
    press(&mut m, "shift+right");
    assert_eq!(m.cfg.view, "list");
    assert_eq!(split::widths(&m), (120, 0));
}

/// The header's rule is in the harness's grey, lit under the tab orange
/// while the Session has the keys, its own grey beside the list without
/// them; the side without the keys fades (paneHeader's rule, fadeRow).
#[test]
fn rule_and_fade_follow_the_keys() {
    let mut m = opened(120, 40, "split");
    let lit = |m: &mut Model| {
        let f = m.frame();
        let rule = f
            .iter()
            .find(|l| l.plain().contains('━'))
            .expect("the rule")
            .clone();
        rule.spans
            .iter()
            .find(|s| s.text.contains('━'))
            .map(|s| s.style.fg)
    };
    assert_eq!(lit(&mut m), Some(ORANGE));
    press(&mut m, "esc");
    let faded = split::fade_row(Line::from_spans(vec![Span::new("━", Style::fg(SUB))]));
    assert_eq!(lit(&mut m), faded.spans[0].style.fg.into());
    assert_ne!(lit(&mut m), Some(ORANGE));
}

/// The side without the keys fades every colour 30% toward the ground,
/// the terminal's own text too, and drops faint (fadeCode).
#[test]
fn fade_row_colours() {
    let l = Line::from_spans(vec![
        Span::new("a", Style::fg(Color::Rgb(217, 119, 87))),
        Span::new("b", Style::default()),
        Span::new(
            "c",
            Style {
                mods: rush_core::text::Mods::DIM,
                ..Style::fg(Color::Rgb(122, 117, 108))
            },
        ),
        Span::new("d", Style::default().on(Color::Rgb(30, 28, 26))),
    ]);
    let f = split::fade_row(l);
    let fg: Vec<Color> = f.spans.iter().map(|s| s.style.fg).collect();
    // Go's mix(c, (17,16,14), .3), rounded.
    assert_eq!(fg[0], Color::Rgb(157, 88, 65));
    assert_eq!(fg[1], Color::Rgb(163, 160, 152));
    assert_eq!(fg[2], Color::Rgb(91, 87, 80));
    assert!(!f.spans[2].style.mods.contains(rush_core::text::Mods::DIM));
    assert_eq!(f.spans[3].style.bg, Color::Rgb(30, 28, 26), "grounds stay");
}

/// The header's rows, washed as Go draws a Claude Code session's at 80
/// columns alone: plain chrome to 60%, then four-cell blocks warming
/// toward the right edge, the right gutter in the last; alternate pairs
/// of rows offset (headerColours, cells read off Go's --render).
#[test]
fn header_wash_as_go() {
    let reg = rush_core::adapter::Registry::new();
    let chrome = || {
        let mut l = Line::from_spans(vec![Span::new(blanks(80), Style::default().on(CHROME_BG))]);
        l.bg = CHROME_BG;
        l
    };
    let mut head = vec![chrome(), chrome(), chrome(), chrome()];
    crate::frame::headerwash::wash_head(&reg, rush_state::LOGINS_KIND, &mut head, 80);
    let bg_at = |l: &Line, x: usize| {
        let mut at = 0;
        for s in &l.spans {
            let n = s.width();
            if x < at + n {
                return s.style.bg;
            }
            at += n;
        }
        Color::Default
    };
    let rgb = |r, g, b| Color::Rgb(r, g, b);
    // Rows 0-1, then 2-3: (from, to, ground) as Go's cells.
    let bands: [&[(usize, usize, Color)]; 2] = [
        &[
            (0, 53, rgb(30, 28, 26)),
            (53, 57, rgb(37, 31, 28)),
            (57, 61, rgb(52, 39, 33)),
            (61, 80, rgb(57, 39, 32)),
        ],
        &[
            (0, 53, rgb(30, 28, 26)),
            (53, 57, rgb(34, 30, 27)),
            (57, 61, rgb(46, 36, 31)),
            (61, 65, rgb(53, 39, 32)),
            (65, 80, rgb(57, 39, 32)),
        ],
    ];
    for (r, row) in head.iter().enumerate() {
        for &(from, to, want) in bands[r / 2] {
            for x in from..to {
                assert_eq!(bg_at(row, x), want, "row {r}, column {x}");
            }
        }
    }
}

/// Pressed on the list's edge and dragged, the list follows the mouse;
/// dropped against the right edge, Agents alone; against the left, the
/// Session alone; the mouse over it lights it (model.go:1528-1560, :1658;
/// view.go:1055 dragSplit, :1469 divider).
#[test]
fn dragging_the_edge() {
    use crate::frame::Mouse;
    use rush_draw::{Button, KeyMods, MouseKind};
    let ev = |m: &mut Model, kind, x: u16| {
        m.mouse_event(&Mouse {
            kind,
            x,
            y: 20,
            mods: KeyMods::NONE,
        })
    };
    let mut m = opened(200, 50, "split");
    let _ = rows(&mut m);
    assert_eq!(split::widths(&m), (56, 143));
    ev(&mut m, MouseKind::Moved, 56);
    assert!(split::divider_lit(&m), "the mouse on the edge lights it");
    ev(&mut m, MouseKind::Moved, 40);
    assert!(!split::divider_lit(&m));

    ev(&mut m, MouseKind::Down(Button::Left), 56);
    ev(&mut m, MouseKind::Drag(Button::Left), 80);
    ev(&mut m, MouseKind::Drag(Button::Left), 90);
    assert_eq!(split::widths(&m), (90, 109), "the list follows the mouse");
    assert!(divider_at(&mut m, 90));
    ev(&mut m, MouseKind::Up(Button::Left), 90);
    assert!(!split::divider_lit(&m), "let go");
    assert!((m.cfg.side_width - 0.45).abs() < 1e-9);
    // Moving with no button held after the release drags nothing.
    ev(&mut m, MouseKind::Moved, 100);
    assert_eq!(split::widths(&m).0, 90);

    // Dropped against the right edge: Agents alone.
    ev(&mut m, MouseKind::Down(Button::Left), 90);
    ev(&mut m, MouseKind::Drag(Button::Left), 199);
    ev(&mut m, MouseKind::Up(Button::Left), 199);
    assert_eq!(split::widths(&m), (200, 0));

    // Against the left: the Session alone.
    let mut m = opened(200, 50, "split");
    let _ = rows(&mut m);
    ev(&mut m, MouseKind::Down(Button::Left), 57); // the column after it too
    ev(&mut m, MouseKind::Drag(Button::Left), 1);
    assert_eq!(split::widths(&m), (0, 200));
}

/// The list goes down to 30 columns on any screen, by shift+← or by
/// dragging its edge; no wide screen pushes it wider. One more shift+←
/// from 30 gives the Session the screen.
#[test]
fn list_down_to_30_on_wide_screens() {
    use crate::frame::Mouse;
    use rush_draw::{Button, KeyMods, MouseKind};
    for w in [200, 300] {
        let mut m = opened(w, 50, "split");
        let first = split::widths(&m).0;
        let mut n = 0;
        while split::widths(&m).0 > 30 {
            press(&mut m, "shift+left");
            n += 1;
            assert!(n < 100, "at {w}: the list never got to 30");
        }
        assert_eq!(
            split::widths(&m),
            (30, w - 31),
            "at {w}: 30 columns, from {first}"
        );
        assert!(divider_at(&mut m, 30), "at {w}: the divider at 30");
        assert!((m.cfg.side_width - 30.0 / w as f64).abs() < 1e-9);
        press(&mut m, "shift+left");
        assert_eq!(
            split::widths(&m),
            (0, w),
            "at {w}: past 30, the Session alone"
        );

        // Dragged: the edge stops at 30 too.
        let mut m = opened(w, 50, "split");
        let _ = rows(&mut m);
        let at = split::widths(&m).0 as u16;
        let ev = |m: &mut Model, kind, x: u16| {
            m.mouse_event(&Mouse {
                kind,
                x,
                y: 20,
                mods: KeyMods::NONE,
            })
        };
        ev(&mut m, MouseKind::Down(Button::Left), at);
        ev(&mut m, MouseKind::Drag(Button::Left), 40);
        assert_eq!(split::widths(&m), (40, w - 41), "at {w}: dragged to 40");
        ev(&mut m, MouseKind::Drag(Button::Left), 10);
        ev(&mut m, MouseKind::Up(Button::Left), 10);
        assert_eq!(split::widths(&m), (30, w - 31), "at {w}: dragged past 30");
    }
}

/// With no share of your own, the list takes 28% of the screen, 30
/// columns at least and 64 at most; nothing makes it wider to keep the
/// Session under 128 columns.
#[test]
fn first_run_width() {
    for (w, want) in [
        (120, 33),
        (160, 44),
        (200, 56),
        (229, 64),
        (300, 64),
        (400, 64),
    ] {
        let m = opened(w, 40, "split");
        assert_eq!(split::widths(&m), (want, w - want - 1), "at {w}");
    }
}

/// A share you set is kept, at any width, between 30 columns and three
/// quarters of the screen, never so wide the Session goes under 84.
#[test]
fn saved_share_kept() {
    for (w, share, want) in [
        (300, 0.1, 30),
        (300, 0.2, 60),
        (300, 0.5, 150),
        (300, 0.05, 30), // under 30 columns: 30
        (120, 0.1, 30),
        (200, 0.5, 100),
    ] {
        let mut m = opened(w, 40, "split");
        m.cfg.side_width = share;
        assert_eq!(split::widths(&m), (want, w - want - 1), "{share} at {w}");
    }
    // ¾ of 200 leaves the Session 49: no split, the Session alone.
    let mut m = opened(200, 40, "split");
    m.cfg.side_width = 0.75;
    assert_eq!(split::widths(&m), (0, 200));
}

/// No split when the Session would be under 84 columns beside the
/// list's 30: 115 columns is the least.
#[test]
fn no_split_under_84() {
    assert!(split::can_split(115));
    assert!(!split::can_split(114));
    for (w, want) in [(114, (0, 114)), (115, (30, 84))] {
        let mut m = opened(w, 40, "split");
        m.cfg.side_width = 0.1; // the list at its least
        assert_eq!(split::widths(&m), want, "at {w}");
    }
    // rush's own 28% at 115 is 32 columns, leaving the Session 82.
    let m = opened(115, 40, "split");
    assert_eq!(split::widths(&m), (0, 115));
}
