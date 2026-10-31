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

/// Settings › Second line transparency fades a stacked row's second line
/// toward the background: "50%" half way (view.go:2111 subLineFade,
/// fadeText).
#[test]
fn the_second_line_fades() {
    use crate::frame::ground::{Rgb, mix, painted};
    let mut m = model_of(vec![agent("a1", "one", State::Idle, 1)], 300, 40);
    let i = m.agents.iter().position(|a| a.key.ends_with("a1")).unwrap();
    let fgs = |m: &crate::model::Model| -> Vec<_> {
        m.agent_sub(i, 60)
            .spans
            .iter()
            .skip(2)
            .map(|s| s.style.fg)
            .collect()
    };
    let before = fgs(&m);
    assert!(!before.is_empty());
    m.cfg.sub_line = "50%".into();
    let after = fgs(&m);
    for (b, a) in before.iter().zip(&after) {
        let want = Rgb::of(*b).map(|c| mix(c, painted().bg, 0.5).color());
        assert_eq!(Some(*a), want);
    }
    assert_eq!(
        m.agent_sub(i, 60).spans[1].style.fg,
        crate::style::FAINT,
        "the hook stays as it is"
    );
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

/// Earlier goes by the time its rows show, newest first: one last at work
/// days ago sits below one at work yesterday, and a subagent's later
/// activity doesn't move it (listorder_test.go
/// TestEarlierGoesByTheTimeShown).
#[test]
fn earlier_goes_by_the_time_shown() {
    let mut old = agent("old", "old", State::Done, 0);
    old.job.updated_at = NOW - 72 * 60 * MIN;
    let mut newer = agent("newer", "newer", State::Done, 0);
    newer.job.updated_at = NOW - 26 * 60 * MIN;
    newer.job.mod_time = newer.job.updated_at;
    let m = model_of(vec![old, newer], 120, 40);
    let order: Vec<&str> = m
        .order
        .iter()
        .map(|&i| m.agents[i].job.id.as_str())
        .collect();
    assert_eq!(order, ["newer", "old"]);
    assert!(sections(&m).contains(&"Earlier".to_string()));
}

/// A section's heading counts its rows and adds up what they spent, what
/// each row's transcripts priced (model.go:2382 rebuild, :2644
/// sectionMeta); none spent, the count alone.
#[test]
fn section_totals() {
    let mut a = agent("a", "a", State::Working, 1);
    a.spend.cost = 469.32;
    let mut b = agent("b", "b", State::Working, 2);
    b.spend.cost = 846.78;
    let c = agent("c", "c", State::Idle, 3);
    let m = model_of(vec![a, b, c], 120, 40);
    let meta = |t: &str| {
        m.lines
            .iter()
            .find_map(|l| match l {
                ListLine::Section { title, meta, .. } if title == t => Some(meta.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {t} section"))
    };
    assert_eq!(meta("Working"), "2 · $1,316");
    assert!(meta(IDLE).starts_with('1'), "{}", meta(IDLE));
    assert!(!meta(IDLE).contains('$'), "{}", meta(IDLE));
}

/// The picked row's card says what it spent and its tokens, and its
/// context against the window it compacts within when that's smaller
/// than the model's (view.go:1887-1899).
#[test]
fn card_spend_and_context() {
    let mut a = agent("a1", "busy one", State::Working, 1);
    a.spend.cost = 846.78;
    a.spend.usage.output = 801_000;
    a.spend.usage.cache_read = 238_600_000;
    a.compaction = rush_core::adapter::Compaction {
        window: 100_000,
        headroom: 10_000,
    };
    let mut m = model_of(vec![a], 120, 40);
    m.previews.insert(
        "default/a1".into(),
        rush_core::agent::Preview {
            context: 50_000,
            text: "first line".into(),
            ..Default::default()
        },
    );
    m.sel = "default/a1".into();
    let text: String = m.frame().iter().map(|l| l.plain() + "\n").collect();
    assert!(text.contains("spent $847"), "{text}");
    assert!(text.contains("out 801k  cache 238.6M"), "{text}");
    assert!(text.contains("50% of 100k auto-compact"), "{text}");
}

/// The name column makes room for each name's badges: its harness's glyph
/// and the pull requests it linked, two at most and how many more
/// (view.go:2060 widestName, :2621 badges).
#[test]
fn names_make_room_for_their_badges() {
    let mut a = agent("a1", "fix the tax rounding", State::Stopped, 0);
    a.prs = [12, 13, 14]
        .into_iter()
        .map(|n| rush_core::adapter::Pr {
            number: n,
            state: "OPEN".into(),
            ..Default::default()
        })
        .collect();
    let mut m = model_of(vec![a], 120, 40);
    let b = text_of(&m.badges(&m.agents[0], false));
    assert_eq!(b, "✻ #12 #13 +1");
    // The name, a space and its badges.
    assert_eq!(m.name_column(120), 20 + 1 + b.chars().count());
    let f: String = m.frame().iter().map(|l| l.plain() + "\n").collect();
    // Picked, the glyph names its harness; the pull requests stay after.
    assert!(
        f.contains("fix the tax rounding ✻ claude #12 #13 +1"),
        "{f}"
    );
}

/// The glyph by a name is the harness the open session's host says it
/// runs in, over the one the list knows; what plugins put on its row
/// follows its badges (view.go:2336 pluginStatus, :2621 badges, caps.go:29
/// sessionAgent).
#[test]
fn badges_name_the_open_sessions_harness_and_plugins_status() {
    use crate::screens::accounts::look::row_badge;
    let a = agent("a1", "fix the tax rounding", State::Idle, 0);
    let mut m = model_of(vec![a], 120, 40);
    let claude = row_badge(&m.reg, "claude", false).text;
    assert_eq!(text_of(&m.badges(&m.agents[0], false)), claude);
    // Its session open, its host saying it runs in another harness.
    let src = crate::session::Source::Transcript {
        path: "/nonexistent/t.jsonl".into(),
        decode: rush_convo::decode(rush_claude::messages),
    };
    let key = m.agents[0].key.clone();
    let mut v = crate::session::SessionView::new(key.clone(), src, 1);
    v.sess.info.kind = "codex".into();
    m.session = Some(v);
    let codex = row_badge(&m.reg, "codex", false).text;
    assert_ne!(codex, claude, "the two harnesses' glyphs differ");
    assert_eq!(text_of(&m.badges(&m.agents[0], false)), codex);
    // Another agent's row keeps its own.
    m.session.as_mut().unwrap().key = "other".into();
    assert_eq!(text_of(&m.badges(&m.agents[0], false)), claude);
    // A plugin's status sits after the badges.
    let st = rush_plugin::UiState {
        statuses: std::collections::BTreeMap::from([(
            key,
            vec![rush_plugin::UiStatus {
                plugin: "drafts".into(),
                status: rush_plugin::Status {
                    text: "2 drafts".into(),
                    ..Default::default()
                },
            }],
        )]),
        ..Default::default()
    };
    m.plugins().client = Some(rush_plugind::hooks::Client::fixed(st));
    m.session = None;
    let f: String = m.frame().iter().map(|l| l.plain() + "\n").collect();
    // Picked, its glyph names its harness in full.
    let full = row_badge(&m.reg, "claude", true).text;
    assert!(
        f.contains(&format!("fix the tax rounding {full} 2 drafts")),
        "{f}"
    );
}
