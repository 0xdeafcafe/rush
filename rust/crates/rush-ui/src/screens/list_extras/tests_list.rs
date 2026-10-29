//! The list's own rules, after Go's internal/ui columns_test.go,
//! stacked_test.go, listmove_test.go and yourturn_test.go: they run
//! against the list as the frame draws it, with this group's hooks in.

use rush_core::agent::{Halt, State};

use super::tests::{MIN, NOW, agent, model_of};
use crate::list::{FINISHED, IDLE, JUST_LEFT, ListLine, NEEDS, STUCK, section_key};
use crate::style::text_of;

fn sections(m: &crate::model::Model) -> Vec<String> {
    m.lines
        .iter()
        .filter_map(|l| match l {
            ListLine::Section { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect()
}

/// A column hidden in Settings leaves the list, and its room goes to
/// names (columns_test.go TestHiddenColumns).
#[test]
fn hidden_columns() {
    let mut a = agent(
        "a1",
        "an agent with a fairly long name to show",
        State::Working,
        1,
    );
    a.mem = 300 << 20;
    let mut m = model_of(vec![a], 200, 50);
    let before = m.name_column(200);
    let h = text_of(&m.column_header(200));
    assert!(h.contains("RAM") && h.contains("TIME"), "{h:?}");
    m.cfg.hide_columns = vec!["ram".into(), "time".into()];
    let h = text_of(&m.column_header(200));
    assert!(
        !h.contains("RAM") && !h.contains("TIME") && h.contains("COST"),
        "{h:?}"
    );
    assert!(m.name_column(200) >= before);
}

/// Rows stack once the list has StackAt percent of the screen or less,
/// and only when too narrow once that's off (stacked_test.go).
#[test]
fn stacked_at_share() {
    let mut m = model_of(vec![agent("a1", "one", State::Idle, 1)], 300, 40);
    assert!(m.stacked(126, 20), "the default 42% stacks");
    assert!(!m.stacked(130, 20), "over 42% with room doesn't");
    m.cfg.stack_at = -1;
    assert!(!m.stacked(126, 20), "turned off, a roomy list doesn't");
    assert!(m.stacked(40, 20), "too narrow always stacks");
}

/// ↑↓ stop on every section heading; there the agent picked before stays
/// the one shown (listmove_test.go).
#[test]
fn list_move_stops_on_headings() {
    let a = agent("a", "a", State::Working, 1);
    let b = agent("b", "b", State::Idle, 2);
    let mut m = model_of(vec![a, b], 120, 40);
    let heads = sections(&m);
    assert_eq!(heads, ["Working", IDLE]);
    m.sel = "default/a".into();
    m.move_sel(1);
    assert_eq!(m.sel, section_key(IDLE));
    m.move_sel(1);
    assert_eq!(m.sel, "default/b");
    m.move_sel(-3);
    assert_eq!(m.sel, section_key("Working"));
}

/// A halt needs you; a turn that ended without asking is Finished; a
/// stopped agent goes to Just left, then Today (yourturn_test.go; Go's
/// Seen, which rush's own state keeps, isn't read yet).
#[test]
fn finished_turns_wait_for_you() {
    let at = |id: &str, st: State| {
        let mut a = agent(id, id, st, 7);
        a.interactive = false;
        a.job.updated_at = NOW - 5 * MIN;
        a
    };
    let quiet = at("quiet", State::Done);
    let mut broke = at("broke", State::Done);
    broke.spend.halt = Some(Halt {
        kind: "server_error".into(),
        text: "API Error: Response stalled mid-stream.".into(),
        at: 0, // Go's test gives it no time: too old to carry on by itself
    });
    let mut rested = at("rested", State::Done);
    rested.pid = 0;
    rested.past = true;
    rested.job.updated_at = NOW - 3 * MIN;
    let mut old = at("old", State::Done);
    old.pid = 0;
    old.past = true;
    old.job.updated_at = NOW - 40 * MIN;
    let mut m = model_of(vec![quiet, broke, rested, old], 120, 40);
    let g = |m: &crate::model::Model, k: &str| m.group_of[&format!("default/{k}")].clone();
    assert_eq!(g(&m, "quiet"), FINISHED);
    assert_eq!(g(&m, "broke"), NEEDS);
    assert_eq!(g(&m, "rested"), IDLE);
    assert_eq!(g(&m, "old"), JUST_LEFT);
    assert_eq!(sections(&m), [NEEDS, FINISHED, IDLE, JUST_LEFT]);

    let mut stuck = at("stuck", State::Working);
    stuck.job.updated_at = NOW - rush_fleet::STUCK_AFTER - MIN;
    stuck.job.mod_time = stuck.job.updated_at;
    m.agents.push(stuck);
    m.roots.push("/src/checkout".into());
    m.rebuild();
    assert_eq!(g(&m, "stuck"), STUCK);
    assert_eq!(&sections(&m)[..3], [NEEDS, STUCK, FINISHED]);

    m.cfg.active_minutes = -1;
    m.rebuild();
    assert_eq!(g(&m, "rested"), JUST_LEFT);
    m.cfg.just_left_count = 1;
    m.rebuild();
    assert_eq!(
        (g(&m, "rested"), g(&m, "old")),
        (JUST_LEFT.into(), "Today".into())
    );
    m.cfg.just_left_minutes = -1;
    m.rebuild();
    assert_eq!(g(&m, "rested"), "Today");

    let i = m
        .agents
        .iter()
        .position(|a| a.key == "default/broke")
        .unwrap();
    assert_eq!(m.row_summary(i).0, "stopped · Response stalled mid-stream.");
}
