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
fn rush_open(id: &str, w: usize, h: usize, views: Vec<View>) -> Model {
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

fn view(name: &str, title: &str, summary: &str) -> View {
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
    let (sx, sy) = (side.x, side.y + i - (side.rows.len() - side.shown));
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
