//! The views panel against Go's: testdata/*.txt are what Go's
//! viewBody and viewsPanel draw of testdata/all.json (every block kind),
//! plain, written by a throwaway Go test at the widths below.

use std::collections::HashMap;

use super::panel::view_body;
use super::*;

fn all() -> View {
    serde_json::from_str(include_str!("testdata/all.json")).expect("all.json")
}

fn state() -> ViewsState {
    let mut st = ViewsState::default();
    for k in [
        "tests|b0/core",
        "tests|b1/commit",
        "tests|b2/rerun ui",
        "tests|g0/auth/login_test.go: TestExpired",
        "tests|b13/Doing/fix",
    ] {
        st.open.insert(k.into(), true);
    }
    st.sorts = HashMap::from([("tests/b0".to_string(), -2)]);
    st.fold = HashMap::from([("tests/b14/t0.1".to_string(), true)]);
    st
}

fn same(got: Vec<String>, want: &str, what: &str) {
    let want: Vec<&str> = want
        .strip_suffix('\n')
        .unwrap_or(want)
        .split('\n')
        .collect();
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "{what}: row {i}\n got: {got:#?}");
    }
    assert_eq!(got.len(), want.len(), "{what}: rows\n{got:#?}");
}

#[test]
fn body_matches_go() {
    let v = all();
    for (w, want) in [
        (44, include_str!("testdata/body_44.txt")),
        (60, include_str!("testdata/body_60.txt")),
        (30, include_str!("testdata/body_30.txt")),
    ] {
        let got = view_body(&v, w, &state())
            .into_iter()
            .map(|l| l.s.plain())
            .collect();
        same(got, want, &format!("body at {w}"));
    }
}

#[test]
fn panel_matches_go() {
    let v = all();
    let small = View {
        name: "prs".into(),
        title: "Pull requests".into(),
        summary: "2 open".into(),
        updated: v.updated,
        ..Default::default()
    };
    let past = vec![View {
        name: "old".into(),
        title: "Old one".into(),
        summary: "was here".into(),
        gone: rush_state::Time::from_ms(v.updated.to_ms() - 3_600_000),
        file: "old.json".into(),
        ..Default::default()
    }];
    let now = v.updated.to_ms() + 90_000;
    let vs = vec![v, small];
    for (w, want) in [
        (48, include_str!("testdata/panel_48.txt")),
        (64, include_str!("testdata/panel_64.txt")),
    ] {
        let mut st = state();
        let mut got: Vec<String> = Vec::new();
        let mut draw = |st: &mut ViewsState, h| {
            got.extend(
                views_panel(&vs, &past, w, h, now, st)
                    .iter()
                    .map(|l| l.plain()),
            );
        };
        draw(&mut st, 24);
        st.scroll = 30;
        draw(&mut st, 24);
        st.tab = "prs".into();
        draw(&mut st, 10);
        st.past = true;
        draw(&mut st, 10);
        st.past = false;
        st.shut = true;
        draw(&mut st, 6);
        same(got, want, &format!("panel at {w}"));
    }
}

/// A click on the panel: tabs, folding, opening, sorting, buttons, the
/// strip, the past views and the bottom edge, as clickViews does them.
#[test]
fn clicks() {
    let v = all();
    let vs = vec![v.clone()];
    let past = vec![View {
        name: "old".into(),
        title: "Old".into(),
        file: "old.json".into(),
        ..Default::default()
    }];
    let mut st = ViewsState::default();
    let draw = |st: &mut ViewsState| views_panel(&vs, &past, 48, 30, 0, st);
    draw(&mut st);
    // The first group's rule folds it.
    let row = st.acts.iter().position(|a| a.fold == "tests/0").unwrap() + 1;
    assert_eq!(st.click(5, row as i64), Click::Drew);
    assert_eq!(st.fold.get("tests/0"), Some(&true));
    draw(&mut st);
    // A row opens, showing its buttons; a button does what it says.
    let row = st
        .acts
        .iter()
        .position(|a| a.open == "tests|g1/db")
        .unwrap()
        + 1;
    st.click(5, row as i64);
    assert_eq!(st.open.get("tests|g1/db"), Some(&true));
    draw(&mut st);
    let row = st.acts.iter().position(|a| !a.btns.is_empty()).unwrap() + 1;
    let b = st.acts[row - 1].btns[0].clone();
    assert_eq!(
        st.click(b.from as i64, row as i64),
        Click::Act {
            key: "g1/db".into(),
            a: b.a.clone()
        }
    );
    assert_eq!(b.a.label, "copy");
    // × on the top edge puts the view away; ⟲ shows the past ones.
    assert_eq!(st.click(44, 0), Click::Drop("tests".into()));
    assert_eq!(st.click(40, 0), Click::Drew);
    assert!(st.past);
    draw(&mut st);
    assert_eq!(
        st.click(5, 2),
        Click::BringBack {
            file: "old.json".into(),
            name: "old".into()
        }
    );
    st.click(5, 0);
    assert!(!st.past);
    // Elsewhere on the top edge folds it to a strip, which opens again.
    draw(&mut st);
    st.click(30, 0);
    assert!(st.shut);
    draw(&mut st);
    st.click(1, 2);
    assert!(!st.shut);
    // The bottom edge scrolls a page, and from the end back to the top.
    draw(&mut st);
    assert!(st.most > 0);
    st.click(5, 29);
    assert_eq!(st.scroll, 27);
    st.scroll = st.most;
    st.click(5, 29);
    assert_eq!(st.scroll, 0);
    // A table's header sorts by the column under it, then the other way,
    // then as it was.
    draw(&mut st);
    st.scroll = 0;
    let mut st2 = ViewsState::default();
    let body = view_body(&vs[0], 44, &st2);
    let hi = body.iter().position(|l| l.act.sort == "tests/b0").unwrap();
    st2.acts = body.iter().map(|l| l.act.clone()).collect();
    st2.w = 48;
    st2.h = body.len() + 2;
    let col = st2.acts[hi].cols[1] as i64;
    for want in [Some(2), Some(-2), None] {
        st2.click(col, hi as i64 + 1);
        assert_eq!(st2.sorts.get("tests/b0").copied(), want);
    }
}

/// The pointer lights a row a click does something on, and a button
/// under it.
#[test]
fn hover() {
    let v = View {
        name: "v".into(),
        title: "V".into(),
        actions: vec![Action {
            label: "go".into(),
            ..Default::default()
        }],
        groups: vec![Group {
            title: "G".into(),
            rows: vec![Row {
                text: "row".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let vs = vec![v];
    let mut st = ViewsState::default();
    views_panel(&vs, &[], 48, 12, 0, &mut st);
    let row = st.acts.iter().position(|a| !a.btns.is_empty()).unwrap() + 1;
    assert!(st.hover_at(3, row as i64));
    assert!(st.hot);
    let lit = views_panel(&vs, &[], 48, 12, 0, &mut st);
    assert!(
        lit[row]
            .spans
            .iter()
            .any(|s| s.text == " go " && s.style.bg == BTN_LIT_BG)
    );
    assert!(st.hover_at(20, row as i64));
    assert!(!st.hot, "beside the button");
    assert!(st.hover_at(5, 2));
    assert!(st.hot);
}

#[test]
fn numbers_read_short() {
    for (f, want) in [(14.0, "14"), (0.5, "0.5"), (212.25, "212.25"), (-3.0, "-3")] {
        assert_eq!(num(f), want);
    }
    assert_eq!(views_width(100), 0);
    assert_eq!(views_width(120), 45);
    assert_eq!(views_width(200), 64);
}

/// A plugin's view shows only while the plugin runs allowed to draw it,
/// and names the plugin; the agent's own gives the session its line
/// (agentviews_test.go TestViewsOfPlugins).
#[test]
fn views_of_plugins() {
    let mine = View {
        name: "mine".into(),
        title: "Mine".into(),
        summary: "the agent's".into(),
        ..Default::default()
    };
    let ci = View {
        name: "p-ci--ci".into(),
        title: "Checks".into(),
        summary: "the plugin's".into(),
        by: "ci".into(),
        ..Default::default()
    };
    let list = vec![ci.clone(), mine.clone()];
    let mut m = crate::model::Model::new("test");
    let shown = shown_views(&list, |p| plugin_draws_views(&m, p));
    assert_eq!(shown.len(), 1, "a plugin that isn't running drew");
    assert_eq!(shown[0].name, "mine");
    // Running, but not allowed views: still nothing.
    let plugin = |ui: Vec<String>| rush_plugin::ui::UiPlugin {
        name: "ci".into(),
        ui,
        ..Default::default()
    };
    m.plugins().client = Some(rush_plugind::hooks::Client::fixed(
        rush_plugin::ui::UiState {
            plugins: vec![plugin(vec!["notify".into()])],
            ..Default::default()
        },
    ));
    assert_eq!(shown_views(&list, |p| plugin_draws_views(&m, p)).len(), 1);
    m.plugins().client = Some(rush_plugind::hooks::Client::fixed(
        rush_plugin::ui::UiState {
            plugins: vec![plugin(vec![rush_plugin::ui::UI_VIEWS.into()])],
            ..Default::default()
        },
    ));
    let shown = shown_views(&list, |p| plugin_draws_views(&m, p));
    assert_eq!(shown.len(), 2);
    // The agent's own summary first, though the plugin's is newer.
    assert_eq!(view_summary(&shown), "the agent's");
    assert_eq!(view_summary(std::slice::from_ref(&ci)), "the plugin's");
    assert_eq!(view_summary(&[]), "");
    let mut st = ViewsState::default();
    let rows = views_panel(&[ci], &[], 48, 10, 0, &mut st);
    assert!(
        rows.last().unwrap().plain().contains("made by plugin ci"),
        "the edge doesn't name the plugin: {:?}",
        rows.last().unwrap().plain()
    );
}

/// Numbers share a bar only as parts of one whole: each with a tone
/// (agentviews_test.go TestShareBar).
#[test]
fn share_bars() {
    let st = |v: &str, label: &str, tone: &str| Stat {
        value: v.into(),
        label: label.into(),
        tone: tone.into(),
        trend: Vec::new(),
    };
    let parts = [
        st("3", "", "good"),
        st("1", "", "bad"),
        st("2", "", "quiet"),
    ];
    let bar = share_bar(&parts, 20);
    assert_eq!(bar.width(), 20, "parts of a whole drew no bar");
    assert!(
        share_bar(&[st("41", "uses", ""), st("2", "unsure", "warn")], 20)
            .spans
            .is_empty(),
        "a number with no tone shared a bar"
    );
}

/// The wheel scrolls the panel only over it; a click on a checklist's
/// mark ticks it (agentviews_test.go TestViewsMouse, viewacts_test.go
/// TestViewRowsWork).
#[test]
fn wheel_and_tick() {
    let rows: Vec<Row> = (0..30)
        .map(|i| Row {
            text: format!("row {i}"),
            ..Default::default()
        })
        .collect();
    let v = View {
        name: "a".into(),
        title: "Alpha".into(),
        groups: vec![Group {
            title: "Rows".into(),
            rows,
            ..Default::default()
        }],
        blocks: vec![Block {
            kind: "checklist".into(),
            items: vec![Item {
                text: "run the tests".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let vs = vec![v];
    let mut st = ViewsState::default();
    let lines = views_panel(&vs, &[], 40, 12, 0, &mut st);
    assert_eq!(lines.len(), 12);
    assert!(st.wheel(5, 3, false));
    assert_eq!(st.scroll, 3, "the wheel didn't scroll it");
    assert!(!st.wheel(41, 3, false), "the wheel beside it scrolled it");
    assert!(st.wheel(5, 3, true));
    assert_eq!(st.scroll, 0);
    // The checklist's row, at the end.
    let lines = views_panel(&vs, &[], 40, 80, 0, &mut st);
    let at = lines
        .iter()
        .position(|l| l.plain().contains("run the tests"))
        .unwrap();
    match st.click(2, at as i64) {
        Click::Act { a, .. } => {
            assert_eq!(a.set.and_then(|c| c.done), Some(true), "not ticked")
        }
        c => panic!("a click on its mark: {c:?}"),
    }
}
