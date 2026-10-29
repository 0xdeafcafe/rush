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
        ("split", 200, 0.0, (71, 128)),
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
        ("list", 200, 0.0, (71, 128)),
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
        ("split", 200, (71, 128)),
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
    assert_eq!(split::widths(&m), (71, 128));
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
