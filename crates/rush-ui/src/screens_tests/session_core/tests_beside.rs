//! The session's views beside its conversation, in a whole pane: the room
//! they take, a click on ×, ⟲ and a past view, a button's copy and row
//! change, the views folded into the column and rush's tasks at the
//! panel's top. After Go's agentviews_test.go (TestViewsPutAwayAndBack),
//! viewacts_test.go (TestViewRowsWork), taskrow_test.go (the folded views'
//! part) and panelsubs_test.go, through the pane rather than the panel.

use crate::screens::plugin_ui::views::{Action, Block, Change, Item, View};
use rush_core::event::{Event, TaskKind, TaskStarted};
use rush_draw::{KeyMods, MouseKind};

use super::*;
use crate::frame::Mouse;
use crate::screens::plugin_ui::views::read;

/// A rush session of host id, its views folder made, open w×h.
pub(super) fn rush_open(id: &str, w: usize, h: usize, views: Vec<View>) -> Model {
    std::fs::create_dir_all(rush_host::paths::dir(id)).expect("the session's folder");
    for v in views {
        rush_host::views::save_view(id, v).expect("saved");
    }
    let mut a = agent("steer", State::Idle);
    a.rush = true;
    a.job.id = id.into();
    let fake = Fake::new();
    let mut m = open(a, &fake, w, h);
    let now = m.now;
    sess(&mut m).sent("fix the parser".into(), Vec::new(), now - 60_000);
    m
}

pub(super) fn view(name: &str, title: &str, summary: &str) -> View {
    View {
        name: name.into(),
        title: title.into(),
        summary: summary.into(),
        ..View::default()
    }
}

fn click(m: &mut Model, x: usize, y: usize) -> Out {
    m.mouse_event(&Mouse {
        kind: MouseKind::Down(rush_draw::Button::Left),
        x: x as u16,
        y: y as u16,
        mods: KeyMods::NONE,
    })
}

/// Where the panel was drawn: its first column and row on screen, its
/// width and height.
fn panel(m: &Model) -> (usize, usize, usize, usize) {
    let st = read::state_of(m, KEY).expect("drawn");
    (st.x, st.y, st.w, st.h)
}

/// The views sit right of the conversation where there's room, its text
/// narrowed by them; too narrow, they don't show.
#[test]
fn views_sit_beside_the_conversation() {
    let mut m = rush_open(
        "vb-beside",
        160,
        40,
        vec![view("prs", "Pull requests", "3 ready")],
    );
    let f = screen(&mut m);
    let (x, y, w, _) = panel(&m);
    let room = 160 - 3 - crate::screens::session_core::minimap::MINI_WIDTH;
    assert_eq!(w, room * 3 / 8, "3/8 of the conversation's room");
    assert!(
        f[y].contains("Pull requests"),
        "no tab on the panel's top edge:\n{}",
        f.join("\n")
    );
    let edge = f[y].chars().take(x).collect::<String>();
    assert!(
        !edge.contains("Pull requests"),
        "the panel starts at column {x}"
    );

    let mut m = rush_open(
        "vb-narrow",
        100,
        40,
        vec![view("prs", "Pull requests", "3 ready")],
    );
    let f = screen(&mut m).join("\n");
    assert!(!f.contains("Pull requests"), "100 wide has no room:\n{f}");
    assert_eq!(panel(&m).2, 0);
}

/// × puts the shown view away among the past ones; the strip lists them,
/// and a click on one brings it back, pinned (TestViewsPutAwayAndBack).
#[test]
fn views_put_away_and_back() {
    let id = "vb-away";
    let mut m = rush_open(id, 160, 40, vec![view("prs", "Pull requests", "3 ready")]);
    screen(&mut m);
    let (x, y, w, _) = panel(&m);
    click(&mut m, x + w - 4, y); // ×
    assert!(
        rush_host::views::views(id).0.is_empty(),
        "× didn't put it away"
    );
    assert_eq!(rush_host::views::past_views(id).len(), 1);

    let f = screen(&mut m);
    let (x, y, w, h) = panel(&m);
    assert_eq!(w, 3, "only past ones: the strip");
    assert!(
        f[y..y + h].iter().any(|l| l.contains('⟲')),
        "no strip:\n{}",
        f.join("\n")
    );
    click(&mut m, x + 1, y + 3); // the strip
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("Past views"),
        "the past ones aren't listed:\n{f}"
    );
    let (x, y, _, _) = panel(&m);
    click(&mut m, x + 5, y + 2); // the past view's row, under a blank
    let vs = rush_host::views::views(id).0;
    assert!(
        vs.len() == 1 && vs[0].pinned,
        "not brought back pinned: {vs:?}"
    );
    assert!(!read::state_of(&m, KEY).expect("drawn").past);
}

/// A button changes its row in the view's file, copies its words, and a
/// click on a checklist's mark ticks it (TestViewRowsWork).
#[test]
fn view_buttons_work() {
    let id = "vb-acts";
    let good = Some("good".to_string());
    let v = View {
        name: "prs".into(),
        title: "PRs".into(),
        blocks: vec![
            Block {
                kind: "list".into(),
                items: vec![Item {
                    text: "#3 the archive".into(),
                    actions: vec![
                        Action {
                            label: "approve".into(),
                            set: Some(Change {
                                tone: good,
                                ..Change::default()
                            }),
                            ..Action::default()
                        },
                        Action {
                            label: "copy it".into(),
                            copy: "gh pr view 3".into(),
                            ..Action::default()
                        },
                    ],
                    ..Item::default()
                }],
                ..Block::default()
            },
            Block {
                kind: "checklist".into(),
                items: vec![Item {
                    text: "run the tests".into(),
                    ..Item::default()
                }],
                ..Block::default()
            },
        ],
        ..View::default()
    };
    let mut m = rush_open(id, 160, 40, vec![v]);
    let row_of = |m: &mut Model, s: &str| -> (usize, String) {
        let f = screen(m);
        let i = f
            .iter()
            .position(|l| l.contains(s))
            .unwrap_or_else(|| panic!("no {s:?} in:\n{}", f.join("\n")));
        (i, f[i].clone())
    };
    // Its row opened shows its buttons.
    let (y, _) = row_of(&mut m, "#3 the archive");
    let (x, ..) = panel(&m);
    click(&mut m, x + 8, y);
    let (y, _) = row_of(&mut m, "approve");
    let (x, py, ..) = panel(&m);
    let btn = |m: &Model, label: &str| {
        let st = read::state_of(m, KEY).expect("drawn");
        st.acts[y - py - 1]
            .btns
            .iter()
            .find(|b| b.a.label == label)
            .map(|b| b.from)
            .expect("the button")
    };
    let from = btn(&m, "approve");
    click(&mut m, x + from, y);
    let vs = rush_host::views::views(id).0;
    assert_eq!(
        vs[0].blocks[0].items[0].tone, "good",
        "approve didn't change its row"
    );
    let (y, _) = row_of(&mut m, "copy it");
    let from = {
        let st = read::state_of(&m, KEY).expect("drawn");
        st.acts[y - py - 1]
            .btns
            .iter()
            .find(|b| b.a.label == "copy it")
            .map(|b| b.from)
            .expect("the button")
    };
    click(&mut m, x + from, y);
    assert_eq!(m.pending_copy.as_deref(), Some("gh pr view 3"));

    let (y, _) = row_of(&mut m, "run the tests");
    click(&mut m, x + 2, y);
    let vs = rush_host::views::views(id).0;
    assert_eq!(
        vs[0].blocks[1].items[0].done,
        Some(true),
        "a click on its mark didn't tick it"
    );
}

/// Folded beside the column, the views are its last part rather than a
/// strip: how many, and each one's title and summary; a click on one
/// opens them at it. Too narrow for the column, the strip as before
/// (taskrow_test.go).
#[test]
fn folded_views_in_the_column() {
    let mut m = rush_open(
        "vb-column",
        220,
        50,
        vec![
            view("tests", "Tests", ""),
            view("prs", "Pull requests", "2 failing"),
        ],
    );
    read::state(&mut m, KEY).shut = true;
    let f = screen(&mut m).join("\n");
    assert_eq!(
        panel(&m).2,
        0,
        "folded beside a column, the views take room"
    );
    for want in [
        "Views  2",
        "open ›",
        "▸ Pull requests  2 failing",
        "▸ Tests",
    ] {
        assert!(f.contains(want), "column missing {want:?}:\n{f}");
    }
    let side = &conn(&mut m, KEY).side;
    let i = side
        .refs
        .iter()
        .position(|r| r == "side:views:tests")
        .expect("Tests' row");
    // The column runs from the top: its rows are where they're drawn.
    assert!(i < side.shown, "Tests' row shows");
    let (sx, sy) = (side.x, side.y + i);
    click(&mut m, sx + 4, sy);
    let st = read::state_of(&m, KEY).expect("drawn");
    assert!(!st.shut && st.tab == "tests", "{} {:?}", st.shut, st.tab);
    screen(&mut m);
    assert!(panel(&m).2 > 3, "opened, the views take no room");

    let mut m = rush_open("vb-strip", 160, 40, vec![view("prs", "Pull requests", "")]);
    read::state(&mut m, KEY).shut = true;
    screen(&mut m);
    assert_eq!(panel(&m).2, 3, "160 wide, folded views take the strip");
}

/// While the panel is open, the shells the session runs show at its top
/// under their heading (panelsubs.go).
#[test]
fn tasks_at_the_panels_top() {
    let mut m = rush_open("vb-subs", 160, 40, vec![view("prs", "Pull requests", "")]);
    let now = m.now;
    sess(&mut m).apply(
        &Event::TaskStarted(TaskStarted {
            id: "b2".into(),
            call_id: "t-b2".into(),
            kind: TaskKind::Shell,
            label: "npm run dev".into(),
            background: true,
            ..TaskStarted::default()
        }),
        now - 60_000,
    );
    let f = screen(&mut m);
    let (x, y, _, h) = panel(&m);
    let inside: Vec<String> = f[y..y + h]
        .iter()
        .map(|l| l.chars().skip(x).collect())
        .collect();
    let inside = inside.join("\n");
    assert!(inside.contains("Shells"), "no heading:\n{inside}");
    assert!(inside.contains("npm run dev"), "no shell:\n{inside}");
}

/// The list's second line ends with what the session's own view says of
/// where it stands (rail.go:121).
#[test]
fn list_says_the_views_summary() {
    let mut m = rush_open(
        "vb-list",
        160,
        40,
        vec![view("prs", "Pull requests", "8 milestones, 1 running")],
    );
    screen(&mut m);
    m.roots = vec![String::new()];
    let a = m.agents[0].clone();
    let l = m.agent_where(&a, 0, 120).plain();
    assert!(
        l.trim_end().ends_with("• 8 milestones · 1 running"),
        "the second line: {l:?}"
    );
}

/// Rush's own column and the agent's views stand in the same place and
/// are as wide, so the conversation doesn't move when one gives way to
/// the other.
#[test]
fn the_column_and_the_views_are_one_width() {
    for (i, w) in [200usize, 240, 300].into_iter().enumerate() {
        let mut m = rush_open(&format!("vb-same-a{i}"), w, 40, Vec::new());
        screen(&mut m);
        let col = {
            let side = &conn(&mut m, KEY).side;
            (side.x, side.w)
        };
        assert!(col.1 > 0, "{w}: no column");
        let mut m = rush_open(
            &format!("vb-same-b{i}"),
            w,
            40,
            vec![view("prs", "Pull requests", "3 ready")],
        );
        screen(&mut m);
        let (x, _, pw, _) = panel(&m);
        assert_eq!((x, pw), col, "{w}: the views stand where the column does");
    }
}

/// The column's width as last drawn.
fn column_w(m: &mut Model) -> usize {
    screen(m);
    conn(m, KEY).side.w
}

/// alt+← and alt+→ with nothing typed make the column beside the
/// conversation two columns narrower or wider, between 44 and all the
/// room left of the text, and the width is kept in the config.
#[test]
fn alt_arrows_resize_the_column() {
    let mut m = rush_open("ak-column", 240, 40, Vec::new());
    let first = column_w(&mut m);
    let room = conn(&mut m, KEY).side.room;
    assert!(room > 44, "room for a column: {room}");
    assert_eq!(
        first,
        room * 3 / 4,
        "rush's own: three quarters of the room"
    );
    press(&mut m, "alt+right");
    assert_eq!(column_w(&mut m), first + 2);
    assert_eq!(m.cfg.column_width, first as i64 + 2, "kept");
    press(&mut m, "alt+left");
    press(&mut m, "alt+left");
    assert_eq!(column_w(&mut m), first - 2);
    // The most: all the room left of the text.
    for _ in 0..100 {
        press(&mut m, "alt+right");
    }
    assert_eq!(column_w(&mut m), room);
    press(&mut m, "alt+right");
    assert_eq!(column_w(&mut m), room);
    assert_eq!(m.cfg.column_width, room as i64);
    // The least: 44.
    for _ in 0..100 {
        press(&mut m, "alt+left");
    }
    assert_eq!(column_w(&mut m), 44);
    press(&mut m, "alt+left");
    assert_eq!(column_w(&mut m), 44);
    assert_eq!(m.cfg.column_width, 44);
    // The width kept is the width drawn next time.
    let mut again = rush_open("ak-column-again", 240, 40, Vec::new());
    again.cfg.column_width = 60;
    assert_eq!(column_w(&mut again), 60);
}

/// shift+← and shift+→ in a Session still move the list's edge, not the
/// column.
#[test]
fn shift_arrows_leave_the_column() {
    let mut m = rush_open("ak-shift", 240, 40, Vec::new());
    let first = column_w(&mut m);
    press(&mut m, "shift+left");
    assert_eq!(column_w(&mut m), first);
    assert_eq!(m.cfg.column_width, 0);
}

/// With something typed, alt+← and alt+→ jump a word and leave the column.
#[test]
fn alt_arrows_with_text_jump_a_word() {
    let mut m = rush_open("ak-word", 240, 40, Vec::new());
    let first = column_w(&mut m);
    typed(&mut m, "one two");
    press(&mut m, "alt+left");
    typed(&mut m, "X");
    assert_eq!(conn(&mut m, KEY).input.string(), "one Xtwo");
    press(&mut m, "alt+right");
    typed(&mut m, "Y");
    assert_eq!(conn(&mut m, KEY).input.string(), "one XtwoY");
    assert_eq!(column_w(&mut m), first);
    assert_eq!(m.cfg.column_width, 0);
}

/// Under the box, the hint names the key to the conversation's view, and
/// it is the first to go when the row is too narrow.
#[test]
fn the_box_hints_the_conversation_view_key() {
    let mut m = rush_open("ak-hint", 120, 40, Vec::new());
    let f = screen(&mut m);
    let last = f[f.len() - 1].clone();
    assert!(
        last.contains("⌥s conversation view  ·  esc back"),
        "{last:?}"
    );
    let mut m = rush_open("ak-hint-narrow", 60, 24, Vec::new());
    let f = screen(&mut m);
    let last = f[f.len() - 1].clone();
    assert!(!last.contains("conversation view"), "{last:?}");
    assert!(last.contains("esc back"), "{last:?}");
}

/// The pointer over the side column, level with a turn of the
/// conversation, lights nothing in the conversation; over the turn's own
/// text it lights it as before.
#[test]
fn hover_stays_out_of_the_side_column() {
    let mut m = rush_open("vb-hover", 220, 50, Vec::new());
    let f = screen(&mut m);
    let (sx, sy, shown) = {
        let side = &conn(&mut m, KEY).side;
        (side.x, side.y, side.shown)
    };
    assert!(shown > 0, "220 wide has the side column:\n{}", f.join("\n"));
    let y = f
        .iter()
        .enumerate()
        .skip(sy)
        .find(|(_, l)| l.contains("fix the parser"))
        .map(|(y, _)| y)
        .expect("the turn's row");
    let moved = |m: &mut Model, x: usize, y: usize| {
        m.mouse_event(&Mouse {
            kind: MouseKind::Moved,
            x: x as u16,
            y: y as u16,
            mods: KeyMods::NONE,
        });
    };
    moved(&mut m, 6, y);
    assert!(
        !conn(&mut m, KEY).vs.hover_ref.is_empty(),
        "over the turn's text: it lights"
    );
    moved(&mut m, sx + 2, y);
    assert_eq!(
        conn(&mut m, KEY).vs.hover_ref,
        "",
        "over the side column: the conversation isn't lit"
    );
    moved(&mut m, 6, y);
    assert!(!conn(&mut m, KEY).vs.hover_ref.is_empty(), "and back");
}

/// Two sessions drawn side by side (a grid's tiles) each keep their own
/// follower, frame after frame: neither is let go and read again because
/// the other was drawn between. One not drawn in the last frame is.
#[test]
fn each_session_drawn_keeps_its_follower() {
    let mut m = rush_open("vb-grid-a", 160, 40, vec![view("a", "A", "first")]);
    std::fs::create_dir_all(rush_host::paths::dir("vb-grid-b")).expect("b's folder");
    rush_host::views::save_view("vb-grid-b", view("b", "B", "first")).expect("saved");
    let summary = |m: &Model, id: &str| {
        read::views_of(m, id)
            .first()
            .map(|v| v.summary.clone())
            .unwrap_or_default()
    };
    for id in ["vb-grid-a", "vb-grid-b"] {
        read::follow(&mut m, id);
        assert_eq!(summary(&m, id), "first", "{id} read as it's followed");
    }
    // Changed on disk; with no loop a follower reads once, as it starts,
    // so a reading now would be a follower started again.
    rush_host::views::save_view("vb-grid-a", view("a", "A", "second")).expect("saved");
    rush_host::views::save_view("vb-grid-b", view("b", "B", "second")).expect("saved");
    for frame in 0..3 {
        m.frames += 1;
        for id in ["vb-grid-a", "vb-grid-b"] {
            read::follow(&mut m, id);
            assert_eq!(
                summary(&m, id),
                "first",
                "frame {frame}: {id} followed again"
            );
        }
    }
    // b not drawn for a frame: let go, and read again when it's back.
    m.frames += 2;
    read::follow(&mut m, "vb-grid-a");
    read::follow(&mut m, "vb-grid-b");
    assert_eq!(summary(&m, "vb-grid-a"), "first", "a, drawn now, kept");
    assert_eq!(summary(&m, "vb-grid-b"), "second", "b followed anew");
}
