//! frame/work.rs's Model::after: what only waits for its time waits on
//! the loop's own deadline, with no thread asleep for it. No sleeps: the
//! times are handed to run_timers.

use std::time::{Duration, Instant};

use rush_draw::{App, Step};

use crate::Note;
use crate::frame::Out;

#[derive(Default)]
struct Ran(Vec<u32>);

#[test]
fn after_waits_on_the_loops_deadline() {
    let mut m = crate::kit::model(80, 24);
    assert!(m.timer_due().is_none());
    let asked = Instant::now();
    m.after(Duration::from_secs(60), |m| {
        m.kept::<Ran>().0.push(2);
        Out::Draw
    });
    m.after(Duration::ZERO, |m| {
        m.kept::<Ran>().0.push(1);
        // One that asks again, as updates' recheck does: kept, not lost.
        m.after(Duration::from_secs(120), |m| {
            m.kept::<Ran>().0.push(3);
            Out::Idle
        });
        Out::Idle
    });
    // The loop is asked to wake at the soonest.
    let due = m.timer_due().unwrap();
    assert!(due <= Instant::now());
    assert!(App::<Note>::wake_at(&m).is_some_and(|t| t <= due));
    // Due now: the first alone, and it draws nothing.
    assert_eq!(m.run_timers(Instant::now()), Step::Idle);
    assert_eq!(m.kept::<Ran>().0, [1]);
    assert!(m.timer_due().unwrap() >= asked + Duration::from_secs(60));
    // A minute on: the one that draws.
    assert_eq!(m.run_timers(asked + Duration::from_secs(61)), Step::Draw);
    assert_eq!(m.kept::<Ran>().0, [1, 2]);
    assert_eq!(m.run_timers(asked + Duration::from_secs(61)), Step::Idle);
    assert_eq!(m.run_timers(asked + Duration::from_secs(200)), Step::Idle);
    assert_eq!(m.kept::<Ran>().0, [1, 2, 3]);
    assert!(m.timer_due().is_none());
}

/// What the model is handed (a message, a frame) runs with the UI
/// thread marked, so a blocking helper called there panics in tests;
/// later's work, run at once with no loop, runs with it lifted.
#[test]
fn the_ui_thread_is_marked_while_the_model_has_it() {
    use rush_draw::Msg;
    let mut m = crate::kit::model(80, 24);
    assert!(!rush_core::offui::on_ui());
    m.update(Msg::App(Note::Apply(Box::new(|m: &mut crate::Model| {
        assert!(rush_core::offui::on_ui(), "a message is the UI thread's");
        m.later(rush_core::offui::on_ui, |m, marked| {
            assert!(!marked, "later's work is off the UI thread");
            assert!(rush_core::offui::on_ui(), "its apply is on it");
            m.kept::<Ran>().0.push(1);
            Out::Idle
        });
        Out::Idle
    }))));
    assert_eq!(m.kept::<Ran>().0, [1]);
    assert!(!rush_core::offui::on_ui(), "the mark goes with the turn");
}
