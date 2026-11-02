//! The whole view started as the TUI starts it (rush_ui::start), with no
//! terminal and no agents, in a throwaway home: how many threads it keeps
//! once the first reading of the list is in, and what an idle minute on
//! that quiet list wakes it for and draws, stepped through on the model's
//! own deadlines (Model::wake_on) rather than waited. Alone in its own
//! file, so no other test's threads are counted (DESIGN.md "Threads").

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::time::{Duration, Instant};

use rush_draw::{App, Headless, Loop, Step, TrySend};
use rush_ui::{Note, frame};

/// At most this many threads once the list is read: the UI's own, its
/// workers and watchers, and those the plugins' client and the network's
/// watch run. 2026-11-02: 18 (21 before the timers and the yap watch).
const THREADS: usize = 18;
/// At most this many wakes of the loop in an idle minute on a quiet list,
/// and no frame: the chores Go's tick runs at its seconds.
const WAKES: usize = 6;

#[test]
fn an_idle_view_keeps_few_threads_and_sleeps() {
    let before = rush_sys::threads::count();
    let lp: Loop<Note> = Loop::new();
    let mut m = rush_ui::new_model_with_workers("test", lp.sender());
    m.w = 120;
    m.h = 40;
    // The broker isn't started: only one already running is talked to,
    // and a test has none.
    let hooks = rush_plugind::hooks::Client::with_dial(Box::new(rush_plugind::hooks::dial_running));
    rush_ui::start(&lp, &mut m, None, Some(hooks));
    // Drawn until the first reading of the list is in, then ended.
    let quit = lp.sender();
    let mut hd = Headless::new(120, 40);
    let mut asked = false;
    lp.run_with(&mut m, |m| {
        hd.draw(|c| App::<Note>::draw(m, c));
        if m.loaded && !asked {
            asked = true;
            if let Err(TrySend::Closed(_)) =
                quit.try_send(Note::Apply(Box::new(|_| frame::Out::Quit)))
            {
                panic!("the loop ended");
            }
        }
        Ok(())
    })
    .unwrap();
    let threads = rush_sys::threads::count().saturating_sub(before);

    // An idle minute: the loop woken only at the deadlines the model
    // asks for, each run as the loop runs it.
    let from = Instant::now();
    let until = from + Duration::from_secs(60);
    let (mut wakes, mut frames) = (0, 0);
    while let Some(at) = App::<Note>::wake_at(&m) {
        if at > until || wakes > 1000 {
            break;
        }
        wakes += 1;
        if m.wake_on(at.max(from)) == Step::Draw {
            frames += 1;
            hd.draw(|c| App::<Note>::draw(&mut m, c));
        }
    }
    let said = format!("threads {threads}, an idle minute: {wakes} wakes, {frames} frames");
    assert!(threads <= THREADS, "{said}");
    assert!(wakes <= WAKES, "{said}");
    assert_eq!(frames, 0, "{said}");
}
