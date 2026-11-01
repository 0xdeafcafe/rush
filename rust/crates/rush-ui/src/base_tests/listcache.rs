//! listcache.rs's tests (rush-ui-base) that draw the list: its rows are
//! the screens' as much as the base's.

use rush_core::agent::Agent;
use rush_core::text::Line;

use crate::listcache::*;

/// The kept list's rows are drawn as they were but for a mark that
/// says what an agent is doing now: neutral and still till the live
/// reading lands.
#[test]
fn kept_rows_keep_their_look_but_not_their_state() {
    use rush_core::agent::State as JobState;
    let mut m = crate::new_model("0.1.0");
    m.w = 120;
    m.h = 30;
    let now = rush_core::time::now();
    m.clock = Some(now);
    m.now = now;
    let mut a = Agent {
        key: "claude:alex:abc".into(),
        display_name: "Working one".into(),
        pid: 42,
        ..Agent::default()
    };
    a.job.id = "abc".into();
    a.job.state = JobState::Working;
    m.agents = vec![a];
    m.roots = vec!["/r".into()];
    m.loaded = true;
    m.rebuild();
    let live: Vec<Line> = m.frame();
    let row = |f: &[Line]| {
        f.iter()
            .find(|l| l.plain().contains("Working one"))
            .cloned()
            .expect("its row")
    };
    let (lr, mark) = (row(&live), |l: &Line| {
        l.spans.iter().find(|s| !s.text.trim().is_empty()).cloned()
    });
    // Kept: the same row, its mark neutral.
    m.kept::<Showing>().0 = true;
    m.rebuild();
    let kept = m.frame();
    let kr = row(&kept);
    let km = kr
        .spans
        .iter()
        .find(|s| s.text == "◌")
        .expect("a still mark");
    assert_eq!(km.style.fg, crate::style::FAINT);
    let name = |l: &Line| {
        l.spans
            .iter()
            .find(|s| s.text.contains("Working one"))
            .cloned()
    };
    assert_eq!(name(&kr), name(&lr), "the name keeps its look");
    assert_ne!(mark(&lr), Some(km.clone()), "live, it says it's working");
}
