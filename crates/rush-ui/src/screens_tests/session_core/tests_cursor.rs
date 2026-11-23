//! Where the cursor goes in an open Session, as one map: from each place
//! it can be (the message box, the task list's head on it, a card, a
//! queued message, a row of the conversation, the column beside a wide
//! conversation, the Agents list), what each key does, in the Session
//! alone at 80 wide, beside the list, and wide with its column.
//!
//! The rules the map holds everywhere (DESIGN.md, "Where the cursor
//! goes"): ↑↓ go through what's on screen in the order it's drawn, ←→
//! only shut and open, tab and { } change pane, esc steps out a level at
//! a time, enter and space act on what the cursor is on. Rust-only: Go's
//! keys differed from place to place.

use super::views_tests::opened;
use super::*;

/// Where the cursor is, as the map names it.
fn place(m: &mut Model) -> String {
    let Some(sel) = m.session.as_ref().map(|v| v.sel.clone()) else {
        return "list".into();
    };
    if let Some(i) = conn(m, KEY).side_pick {
        let r = conn(m, KEY).side.refs[i].clone();
        return r.split(':').take(2).collect::<Vec<_>>().join(":");
    }
    if m.cards().conns.get(KEY).is_some_and(|c| c.card_focus) {
        return "card".into();
    }
    match sel.as_str() {
        "" => "box".into(),
        "tasks" => "tasks".into(),
        s if s.starts_with("q:") => s.replace("q:", "queue:"),
        _ => "row".into(),
    }
}

/// Presses keys, drawing a frame after each, as the screen would.
fn go(m: &mut Model, keys: &[&str]) {
    screen(m);
    for k in keys {
        press(m, k);
        screen(m);
    }
}

/// Runs a table of (how to get there from the box, the key, where it
/// lands) over a fresh model for each row.
fn map(name: &str, make: &dyn Fn() -> Model, cases: &[(&[&str], &str, &str)]) {
    for (from, key, want) in cases {
        let mut m = make();
        go(&mut m, from);
        let was = place(&mut m);
        go(&mut m, &[key]);
        assert_eq!(
            place(&mut m),
            *want,
            "{name}: {key} from {was} (after {from:?})"
        );
    }
}

/// The Session alone, 80 wide: the box, the task list's head on it, the
/// conversation's rows.
#[test]
fn the_move_map_narrow() {
    let cases: &[(&[&str], &str, &str)] = &[
        // The box.
        (&[], "up", "tasks"),
        (&[], "down", "box"),
        (&[], "left", "box"),
        (&[], "right", "box"),
        (&[], "enter", "box"),
        (&[], "esc", "list"),
        (&[], "tab", "list"),
        (&[], "{", "list"),
        (&[], "}", "list"),
        (&[], "ctrl+n", "list"),
        // The task list's head.
        (&["up"], "up", "row"),
        (&["up"], "down", "box"),
        (&["up"], "left", "tasks"),
        (&["up"], "right", "tasks"),
        (&["up"], "enter", "tasks"),
        (&["up"], "space", "tasks"),
        (&["up"], "esc", "box"),
        // A row of the conversation.
        (&["up", "up"], "down", "tasks"),
        (&["up", "up"], "left", "row"),
        (&["up", "up"], "right", "row"),
        (&["up", "up"], "enter", "row"),
        (&["up", "up"], "space", "row"),
        (&["up", "up"], "esc", "box"),
        (&["up", "up"], "tab", "list"),
    ];
    map("narrow", &|| opened(80, 34), cases);
}

/// ↑ goes on up through the conversation a row at a time and ↓ comes
/// back the same way, through the task list's head, to the box.
#[test]
fn up_and_down_are_each_others_way_back() {
    let mut m = opened(80, 34);
    let mut seen = vec![place(&mut m)];
    let mut refs = Vec::new();
    for _ in 0..4 {
        go(&mut m, &["up"]);
        seen.push(place(&mut m));
        refs.push(m.session.as_ref().unwrap().sel.clone());
    }
    assert_eq!(seen[..3], ["box", "tasks", "row"], "{seen:?}");
    // Back down: the same refs, last first, then the box.
    refs.dedup();
    let mut back = Vec::new();
    for _ in 0..refs.len() {
        back.push(m.session.as_ref().unwrap().sel.clone());
        go(&mut m, &["down"]);
    }
    back.reverse();
    assert_eq!(back, refs, "↓ undoes ↑");
    assert_eq!(place(&mut m), "box");
}

/// The Session beside the list: tab, { } and esc move between the two,
/// enter and → open the agent picked in the list, ← never leaves the
/// Session.
#[test]
fn the_move_map_beside_the_list() {
    let make = || {
        let mut m = opened(200, 40);
        m.full = false;
        m.roots = vec![String::new()];
        m.rebuild();
        m
    };
    let cases: &[(&[&str], &str, &str)] = &[
        (&[], "left", "box"),
        (&[], "up", "tasks"),
        (&[], "esc", "list"),
        (&[], "tab", "list"),
        (&[], "{", "list"),
        (&[], "}", "list"),
        (&["up", "up"], "esc", "box"),
        (&["up", "up"], "left", "row"),
        // The list.
        (&["esc"], "tab", "box"),
        (&["esc"], "enter", "box"),
        (&["esc"], "right", "box"),
        (&["esc"], "{", "box"),
        (&["esc"], "}", "box"),
        (&["esc"], "left", "list"),
    ];
    map("split", &make, cases);
}

/// Wide, the Session alone with the column beside its conversation: tab
/// goes into the column and on to the list, { and esc come back, ←→
/// don't change pane, and the column's pick stays put through frames.
#[test]
fn the_move_map_wide() {
    let cases: &[(&[&str], &str, &str)] = &[
        // The box: the tasks are in the column, so ↑ is the conversation.
        (&[], "up", "row"),
        (&[], "right", "box"),
        (&[], "left", "box"),
        (&[], "tab", "side:tasks"),
        (&[], "}", "side:tasks"),
        (&[], "{", "list"),
        (&[], "esc", "list"),
        // The column.
        (&["tab"], "down", "side:changes"),
        (&["tab"], "up", "side:tasks"),
        (&["tab"], "left", "side:tasks"),
        (&["tab"], "esc", "box"),
        (&["tab"], "{", "box"),
        (&["tab"], "tab", "list"),
        (&["tab"], "}", "list"),
        (&["tab", "down"], "up", "side:tasks"),
        // A row of the conversation keeps its pick while the column has
        // the keys, and gets them back.
        (&["up"], "right", "row"),
        (&["up"], "left", "row"),
        (&["up"], "tab", "side:tasks"),
        (&["up", "tab"], "esc", "row"),
        (&["up"], "esc", "box"),
        (&["up"], "down", "box"),
    ];
    map("wide", &|| opened(200, 40), cases);
}

/// enter, space and → in the column open what the cursor is on: the
/// tasks' part the Tasks view, a changed file Changes with it picked.
#[test]
fn the_column_opens_what_is_picked() {
    for k in ["enter", "space", "right"] {
        let mut m = opened(200, 40);
        go(&mut m, &["tab", k]);
        assert_eq!(
            crate::screens::session_core::views::view_name(&m, KEY),
            "tasks",
            "{k}"
        );
        let mut m = opened(200, 40);
        go(&mut m, &["tab", "down", "down", k]);
        assert_eq!(
            crate::screens::session_core::views::view_name(&m, KEY),
            "changes",
            "{k}"
        );
        assert_eq!(
            m.session.as_ref().unwrap().sel,
            "chg:/src/checkout/src/parse.rs"
        );
    }
}

/// The pane with the keys is the lit one: in the column, its picked item
/// has the orange bar on the selection's ground, the box says the keys
/// are beside it and the row picked in the conversation has the faint
/// bar; back in the conversation the row's bar is the cursor's.
#[test]
fn the_pane_with_the_keys_is_the_lit_one() {
    let bar_of = |m: &mut Model, text: &str| {
        let f = m.frame();
        let l = f
            .iter()
            .find(|l| l.plain().contains(text))
            .unwrap_or_else(|| panic!("no row with {text:?}"));
        // The column's is the last on the row, right of the conversation's.
        l.spans
            .iter()
            .rev()
            .find(|s| s.text.contains('▍'))
            .map(|s| s.style.fg)
    };
    let mut m = opened(200, 40);
    go(&mut m, &["up"]);
    let picked = m.session.as_ref().unwrap().sel.clone();
    assert!(!picked.is_empty());
    let f = screen(&mut m).join("\n");
    assert!(!f.contains('▏'), "the bar, not Go's thin one:\n{f}");
    go(&mut m, &["tab"]);
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("picking beside · typing or esc returns here"),
        "{f}"
    );
    assert!(
        f.contains("▍ Tasks  1 of 3"),
        "the column's item is lit:\n{f}"
    );
    assert_eq!(
        bar_of(&mut m, "▍ Tasks  1 of 3"),
        Some(crate::style::ORANGE),
        "the column has the keys"
    );
    let last = screen(&mut m).pop().unwrap();
    assert!(
        last.contains("↑↓ pick")
            && last.contains("tab Agents")
            && last.contains("esc back to message"),
        "{last}"
    );
    go(&mut m, &["esc"]);
    let f = screen(&mut m).join("\n");
    assert!(f.contains("▌ Tasks  1 of 3"), "its own edge again:\n{f}");
    assert_eq!(m.session.as_ref().unwrap().sel, picked);
}

/// The hints under the box say the moves from where the cursor is.
#[test]
fn the_hints_say_the_moves() {
    let last = |m: &mut Model| screen(m).pop().unwrap();
    let mut m = opened(120, 40);
    go(&mut m, &["up"]);
    let h = last(&mut m);
    for w in [
        "enter open",
        "→ open",
        "← shut",
        "↑↓ move",
        "esc back to message",
    ] {
        assert!(h.contains(w), "the task list's head: {w:?} in {h}");
    }
    go(&mut m, &["enter"]);
    let h = last(&mut m);
    assert!(h.contains("enter shut") && h.contains("→ all tasks"), "{h}");
    go(&mut m, &["up"]);
    let h = last(&mut m);
    assert!(h.contains("→ ← open · shut"), "a row: {h}");
    assert!(!h.contains("the side"), "{h}");
    let mut m = opened(200, 40);
    go(&mut m, &[]);
    let h = last(&mut m);
    assert!(h.contains("tab the side"), "the box, wide: {h}");
}

/// A queue and a card over the box, as drawn: the card nearest the box,
/// the queue over it. ↑ from the box goes through them in that order and
/// on into the conversation, and ↓ comes back through each (Go's ↑ from
/// the card went past the queue, and ↓ from the queue past the card).
#[test]
fn the_move_map_with_a_queue_and_a_card() {
    let make = || {
        let (mut m, _) = hosted(
            "steer",
            vec![queue_of(&["a", "b"]), Said::Event(approval("r1"))],
        );
        m.cards().host = Some(crate::screens::session_core::cards_host::host());
        m
    };
    // An approval's card has the keys from the start, the box empty
    // (its letters answer it), so the walk starts on it.
    let cases: &[(&[&str], &str, &str)] = &[
        (&[], "up", "queue:1"),
        (&["up"], "down", "card"),
        (&["up"], "up", "queue:0"),
        (&["up", "up"], "down", "queue:1"),
        (&["up", "up"], "up", "row"),
        (&["up", "up", "up"], "down", "queue:0"),
    ];
    map("queue and card", &make, cases);
    // And only a queue: its last message first.
    let make = || hosted("steer", vec![queue_of(&["a", "b"])]).0;
    let cases: &[(&[&str], &str, &str)] = &[
        (&[], "up", "queue:1"),
        (&["up"], "up", "queue:0"),
        (&["up"], "down", "box"),
        (&["up", "up"], "down", "queue:1"),
        (&["up", "up"], "up", "queue:0"),
    ];
    map("queue", &make, cases);
}

/// A column taller than its room scrolls: ↓ through its items keeps the
/// one picked in sight, the rows at its ends say how many more there
/// are, no row is drawn twice, and ↑ comes back to its top. The wheel
/// over it scrolls it, never past either end.
#[test]
fn a_tall_column_scrolls_with_its_pick() {
    use rush_core::event::{TaskKind, TaskStarted};
    use rush_draw::{KeyMods, MouseKind};
    let mut m = opened(200, 24);
    for i in 0..30 {
        sess(&mut m).apply(
            &Event::TaskStarted(TaskStarted {
                id: format!("b{i}"),
                call_id: format!("t{i}"),
                kind: TaskKind::Subagent,
                label: format!("watch-{i:02}"),
                background: true,
                ..TaskStarted::default()
            }),
            NOW - 60_000,
        );
    }
    let column = |m: &mut Model| -> Vec<String> {
        let f = screen(m);
        let side = &conn(m, KEY).side;
        let (x, y, n) = (side.x, side.y, side.shown);
        f[y..y + n]
            .iter()
            .map(|l| l.chars().skip(x).collect::<String>().trim_end().to_string())
            .collect()
    };
    let first = column(&mut m);
    let total = conn(&mut m, KEY).side.rows.len();
    assert!(total > first.len(), "taller than its room: {total}");
    assert!(
        first.last().unwrap().contains("↓") && first.last().unwrap().contains("more"),
        "{first:#?}"
    );
    go(&mut m, &["tab"]);
    let items = (0..total)
        .filter(|&i| {
            let s = &conn(&mut m, KEY).side;
            !s.refs[i].is_empty() && (i == 0 || s.refs[i - 1] != s.refs[i])
        })
        .count();
    assert!(items > 20, "{items} items");
    let mut tops = Vec::new();
    for step in 0..items + 2 {
        let rows = column(&mut m);
        let (top, shown) = {
            let s = &conn(&mut m, KEY).side;
            (s.top, s.shown)
        };
        assert!(top + shown <= total, "step {step}: top {top} past the end");
        // The item picked is lit, every row of it (the tasks' part is
        // four), and nothing else.
        let lit = rows.iter().filter(|r| r.starts_with('▍')).count();
        assert!(
            (1..=4).contains(&lit),
            "step {step}: the pick in sight: {rows:#?}"
        );
        let mut named: Vec<&String> = rows.iter().filter(|r| r.contains("watch-")).collect();
        let n = named.len();
        named.sort();
        named.dedup();
        assert_eq!(named.len(), n, "step {step}: a row drawn twice: {rows:#?}");
        tops.push(top);
        go(&mut m, &["down"]);
    }
    assert!(
        tops.windows(2).all(|w| w[0] <= w[1]),
        "never back up: {tops:?}"
    );
    assert_eq!(
        conn(&mut m, KEY).side.top + conn(&mut m, KEY).side.shown,
        total,
        "its foot in sight at the last item"
    );
    for _ in 0..items + 2 {
        go(&mut m, &["up"]);
        let rows = column(&mut m);
        assert!(rows.iter().any(|r| r.starts_with('▍')), "{rows:#?}");
    }
    assert_eq!(conn(&mut m, KEY).side.top, 0, "back at its top");
    // The wheel over it.
    go(&mut m, &["esc"]);
    let (x, y) = {
        let s = &conn(&mut m, KEY).side;
        (s.x + 4, s.y + 2)
    };
    let before = m.session.as_ref().unwrap().start;
    let wheel = |m: &mut Model, kind| {
        m.mouse_event(&crate::frame::Mouse {
            kind,
            x: x as u16,
            y: y as u16,
            mods: KeyMods::NONE,
        });
        screen(m);
        conn(m, KEY).side.top
    };
    assert_eq!(wheel(&mut m, MouseKind::ScrollUp), 0, "not past its top");
    assert_eq!(wheel(&mut m, MouseKind::ScrollDown), 3);
    for _ in 0..40 {
        wheel(&mut m, MouseKind::ScrollDown);
    }
    let top = conn(&mut m, KEY).side.top;
    assert_eq!(
        top + conn(&mut m, KEY).side.shown,
        total,
        "not past its foot"
    );
    assert_eq!(
        m.session.as_ref().unwrap().start,
        before,
        "the conversation under it stays put"
    );
}

/// A hosted child open as a session of its own: its breadcrumb says
/// which of its parent's subagents it is and the keys, alt+↓ and alt+↑
/// open the next and the one before in its place, round the ends, and
/// esc from its empty box is its parent, not the list.
#[test]
fn a_child_steps_through_its_siblings_and_back_to_its_parent() {
    let fake = Fake::new();
    let mut m = open(agent("steer", State::Working), &fake, 140, 40);
    m.agents[0].rush = true;
    for (i, id) in ["c1", "c2", "c3"].iter().enumerate() {
        let mut a = agent("steer", State::Idle);
        a.key = format!("kid-{id}");
        a.display_name = format!("lane {id}");
        a.job.id = id.to_string();
        a.rush = true;
        m.agents.push(a);
        m.roots.push(String::new());
        conn(&mut m, KEY).runs.spawns.insert(
            format!("toolu_{i}"),
            crate::spawns::SpawnRun {
                step: format!("toolu_{i}"),
                kind: "steer".into(),
                name: format!("lane {id}"),
                hosted: id.to_string(),
                born: NOW - 60_000 + i as Ms,
                ..Default::default()
            },
        );
    }
    m.rebuild();
    // Opened as the list opens it; its host says who started it.
    let at = |m: &mut Model| {
        sess(m).info.spawned_by = "k1".into();
        screen(m);
        m.session.as_ref().unwrap().key.clone()
    };
    let i = m.agents.iter().position(|a| a.key == "kid-c2").unwrap();
    m.open(i);
    m.full = true;
    assert_eq!(at(&mut m), "kid-c2");
    let f = screen(&mut m).join("\n");
    assert!(
        f.contains("subagent 2 of 3 · alt+↑↓ the others · esc back"),
        "{f}"
    );
    press(&mut m, "alt+down");
    assert_eq!(at(&mut m), "kid-c3");
    press(&mut m, "alt+down");
    assert_eq!(at(&mut m), "kid-c1", "round the end");
    press(&mut m, "alt+up");
    assert_eq!(at(&mut m), "kid-c3");
    press(&mut m, "esc");
    assert_eq!(
        m.session.as_ref().map(|v| v.key.as_str()),
        Some(KEY),
        "esc is its parent, not the list"
    );
}
