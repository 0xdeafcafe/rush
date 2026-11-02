//! The panel worked as a live thing (Rust-only): scrolled and kept where
//! you left it when the agent sets it again, its rows moved through with
//! the keys and opened by enter or a click, their buttons pressed, a
//! table too wide for it giving up its last column, and the host's grey.

use super::*;

const NOW: i64 = 1_800_000_000_000;

/// A view of n rows, each with a detail and a button.
fn rows_view(n: usize) -> View {
    View {
        name: "fix".into(),
        title: "Flaky tests".into(),
        updated: rush_state::Time::from_ms(NOW - 60_000),
        blocks: vec![Block {
            kind: "list".into(),
            items: (0..n)
                .map(|i| Item {
                    text: format!("row {i}"),
                    tone: "bad".into(),
                    detail: format!("why {i} failed"),
                    actions: vec![Action {
                        label: "fix it".into(),
                        send: format!("Fix row {i}"),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn plain(rows: &[rush_core::text::Line]) -> Vec<String> {
    rows.iter().map(|l| l.plain()).collect()
}

/// The screen row the cursor's first line is on.
fn cursor_line(st: &ViewsState) -> usize {
    st.cursor - st.scroll + 1
}

#[test]
fn scroll_is_kept_when_the_view_is_set_again() {
    let mut st = ViewsState::default();
    let v = rows_view(30);
    let rows = views_panel(std::slice::from_ref(&v), &[], 48, 12, NOW, &mut st);
    let edge = rows[11].plain();
    // 32 body rows (a blank one first and last), 10 in sight.
    assert!(edge.contains("↓ 22 more"), "{edge}");
    assert!(st.wheel(5, 3, false));
    assert_eq!(st.scroll, 3);
    // Set again, a row changed: where you scrolled stays.
    let mut again = rows_view(30);
    again.blocks[0].items[4].tone = "good".into();
    again.updated = rush_state::Time::from_ms(NOW);
    let rows = views_panel(std::slice::from_ref(&again), &[], 48, 12, NOW, &mut st);
    assert_eq!(st.scroll, 3);
    let edge = rows[11].plain();
    assert!(edge.contains("↑ 3 · ↓ 19 more"), "{edge}");
    assert!(rows[1].plain().contains("✗ row 2"), "{:#?}", plain(&rows));
    // Shrunk below where you were: as much as it still has, its end at
    // the bottom; shrunk to fit: from the top, nothing more below.
    st.scroll = 20;
    views_panel(&[rows_view(25)], &[], 48, 12, NOW, &mut st);
    assert_eq!(st.scroll, 17);
    let rows = views_panel(&[rows_view(5)], &[], 48, 12, NOW, &mut st);
    assert_eq!(st.scroll, 0);
    assert!(!plain(&rows).concat().contains("more"));
}

#[test]
fn keys_move_through_rows_open_them_and_press_their_buttons() {
    let mut st = ViewsState::default();
    let vs = vec![rows_view(30)];
    let draw = |st: &mut ViewsState| views_panel(&vs, &[], 48, 12, NOW, st);
    draw(&mut st);
    assert_eq!(st.key("down"), None, "keys while the panel hasn't them");
    assert!(st.take_focus());
    let rows = draw(&mut st);
    assert!(rows[cursor_line(&st)].plain().contains("row 0"));
    assert!(
        rows[cursor_line(&st)]
            .spans
            .iter()
            .any(|s| s.style.bg == SEL_BG),
        "the cursor isn't lit"
    );
    assert_eq!(st.key("down"), Some(Click::Drew));
    let rows = draw(&mut st);
    assert!(rows[cursor_line(&st)].plain().contains("row 1"));
    // enter opens it: its detail and its buttons under it.
    assert_eq!(st.key("enter"), Some(Click::Drew));
    let rows = draw(&mut st);
    let text = plain(&rows).join("\n");
    assert!(
        text.contains("why 1 failed") && text.contains(" fix it "),
        "{text}"
    );
    // ↓ to its buttons, enter presses the first, → picks the next.
    st.key("down");
    let rows = draw(&mut st);
    let lit = &rows[cursor_line(&st)];
    assert!(
        lit.spans
            .iter()
            .any(|s| s.text == " fix it " && s.style.bg == BTN_LIT_BG),
        "{:?}",
        lit.plain()
    );
    match st.key("enter") {
        Some(Click::Act { a, .. }) => assert_eq!(a.send, "Fix row 1"),
        c => panic!("enter on the buttons: {c:?}"),
    }
    st.key("right");
    match st.key("enter") {
        Some(Click::Act { a, .. }) => assert_eq!(a.copy, "row 1"),
        c => panic!("enter on copy: {c:?}"),
    }
    // pgdn scrolls a page and brings the cursor into sight.
    st.key("pgdown");
    draw(&mut st);
    assert!(st.scroll > 0 && (st.scroll..st.scroll + 10).contains(&st.cursor));
    // ← off the buttons, then ← again gives the keys back; so does any
    // key that isn't the panel's.
    st.key("left");
    assert!(!st.focus);
    assert!(st.take_focus());
    assert_eq!(st.key("x"), None);
    assert!(!st.focus);
}

#[test]
fn a_click_opens_a_row_and_its_button_runs() {
    let mut st = ViewsState::default();
    let vs = vec![rows_view(3)];
    let rows = views_panel(&vs, &[], 48, 20, NOW, &mut st);
    let at = rows
        .iter()
        .position(|l| l.plain().contains("row 2"))
        .unwrap();
    assert_eq!(st.click(6, at as i64), Click::Drew);
    let rows = views_panel(&vs, &[], 48, 20, NOW, &mut st);
    assert!(plain(&rows).join("\n").contains("why 2 failed"));
    let at = rows
        .iter()
        .rposition(|l| l.plain().contains(" fix it "))
        .unwrap();
    let b = st.acts[at - 1].btns[0].clone();
    match st.click(b.from as i64 + 1, at as i64) {
        Click::Act { key, a } => {
            assert_eq!(a.send, "Fix row 2");
            assert!(!key.is_empty(), "the row it's on");
        }
        c => panic!("a click on its button: {c:?}"),
    }
}

#[test]
fn a_table_too_wide_drops_its_last_column_into_the_detail() {
    let cells = |c: &[&str]| Item {
        cells: c.iter().map(|s| s.to_string()).collect(),
        tone: "good".into(),
        ..Default::default()
    };
    let v = View {
        name: "br".into(),
        title: "Branches".into(),
        blocks: vec![Block {
            kind: "table".into(),
            columns: ["branch", "CI", "ahead", "age"].map(String::from).to_vec(),
            items: vec![
                cells(&["rush/menubar", "failing", "12", "3 days"]),
                cells(&["rush/views", "green", "3", "1 hour"]),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    for (w, gone, kept) in [
        // inner 34: the 38 it needs is too wide, age goes.
        (
            38,
            vec!["age", "3 days"],
            vec!["CI", "ahead", "+1", "rush/menubar"],
        ),
        // inner 60: all of it.
        (64, vec!["+1"], vec!["age", "3 days", "ahead"]),
        // inner 22: down to two columns, then cells cut; no room for +2.
        (26, vec!["ahead", "age", "3 days"], vec!["CI"]),
    ] {
        let mut st = ViewsState::default();
        let rows = views_panel(std::slice::from_ref(&v), &[], w, 20, 0, &mut st);
        let text = plain(&rows).join("\n");
        // The header and the rows, not the edge (whose "agent" has "age").
        let table: String = plain(&rows)
            .iter()
            .filter(|r| r.contains("branch") || r.contains("rush/") || r.contains("days"))
            .cloned()
            .collect();
        for g in gone {
            assert!(!table.contains(g), "at {w}, {g:?} shows:\n{text}");
        }
        for k in kept {
            assert!(table.contains(k), "at {w}, {k:?} is missing:\n{text}");
        }
        assert!(rows.iter().all(|r| r.width() <= w), "at {w}:\n{text}");
    }
    // Opened, a row shows what the column held.
    let mut st = ViewsState::default();
    let rows = views_panel(std::slice::from_ref(&v), &[], 38, 20, 0, &mut st);
    let at = rows
        .iter()
        .position(|l| l.plain().contains("rush/menubar"))
        .unwrap();
    st.click(6, at as i64);
    let rows = views_panel(std::slice::from_ref(&v), &[], 38, 20, 0, &mut st);
    let text = plain(&rows).join("\n");
    assert!(text.contains("│ age     3 days"), "{text}");
}

#[test]
fn a_view_the_host_greyed_says_why() {
    let mut v = rows_view(2);
    v.greyed = rush_host::viewstale::WHY_DONE.into();
    let sess = rush_convo::Session::default();
    let mut st = ViewsState::default();
    st.quiet = st.quiet_why(Some(&sess), &v, NOW);
    assert_eq!(st.quiet, "all done");
    let rows = views_panel(std::slice::from_ref(&v), &[], 48, 20, NOW, &mut st);
    let text = plain(&rows).join("\n");
    assert!(text.contains("all done · 1m"), "{text}");
    v.pinned = true;
    assert_eq!(
        st.quiet_why(Some(&sess), &v, NOW),
        "",
        "one you brought back"
    );
}

/// The branches example as a user meets it: scrolled, then a row opened
/// with the keys and its first button pressed. The drawings are
/// testdata/live_*.txt.
#[test]
fn branches_scrolled_and_opened() {
    let (_, ex) = rush_host::viewwords::VIEW_EXAMPLES
        .iter()
        .find(|(n, _)| *n == "branches")
        .unwrap();
    let mut v: View = serde_json::from_str(ex).unwrap();
    v.name = "branches".into();
    v.updated = rush_state::Time::from_ms(NOW - 60_000);
    let vs = vec![v];
    let mut st = ViewsState::default();
    let draw = |st: &mut ViewsState| views_panel(&vs, &[], 48, 12, NOW, st);
    draw(&mut st);
    st.wheel(5, 3, false);
    let scrolled = plain(&draw(&mut st));
    assert!(st.take_focus());
    let mut rows = draw(&mut st);
    for _ in 0..10 {
        if rows[cursor_line(&st)].plain().contains("rush/perf") {
            break;
        }
        st.key("down");
        rows = draw(&mut st);
    }
    assert_eq!(st.key("enter"), Some(Click::Drew));
    let opened = plain(&draw(&mut st));
    st.key("down");
    draw(&mut st);
    match st.key("enter") {
        Some(Click::Act { a, .. }) => assert_eq!(a.send, "Show me rush/perf's failing CI log"),
        c => panic!("enter on its first button: {c:?}"),
    }
    let bad: Vec<String> = [
        super::tests::differs(
            scrolled,
            include_str!("testdata/live_scrolled.txt"),
            "scrolled",
        ),
        super::tests::differs(opened, include_str!("testdata/live_opened.txt"), "opened"),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n\n"));
}
