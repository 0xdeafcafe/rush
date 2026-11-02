//! The whole view started as the TUI starts it (rush_ui::start), with no
//! terminal and no agents, in a throwaway home: how many threads it keeps
//! once the first reading of the list is in, and what an idle minute on
//! that quiet list wakes it for and draws, stepped through on the model's
//! own deadlines (Model::wake_on) rather than waited. Alone in its own
//! file, so no other test's threads are counted (DESIGN.md "Threads").

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::time::{Duration, Instant};

use rush_draw::{App, Headless, Key, KeyMods, Loop, Step, TrySend};
use rush_ui::{Note, frame};

/// At most this many threads once the list is read, later's idle ones
/// left out: the UI's own, its workers and watchers, and those the
/// plugins' client and the network's watch run. 2026-11-02 on CI: 15 with
/// later's idle ones (16 before the timers, netwatch's hook and the
/// pool; rush-updates, rush-net and a rush-later asleep among them).
const THREADS: usize = 15;
/// At most this many wakes of the loop in an idle minute on a quiet
/// list: the chores Go's tick runs at its seconds (5) and the look for
/// updates' minute, here never having looked (in a rush that has, its
/// fourth hour). 2026-11-02: 6; before, 5 seen here and the rush-updates
/// thread's minute besides.
const WAKES: usize = 6;
/// Frames among them: a chore draws the screen again only when it shows
/// more than the list (here the tips beside it), as Go's tick does.
const FRAMES: usize = 5;

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
    // later's idle threads end 10 s on: not counted as kept.
    let idle = frame::work::later_idle();
    let threads = rush_sys::threads::count()
        .saturating_sub(before)
        .saturating_sub(idle);

    // The first start's layout sheet put away as you would (esc: side
    // by side), so the list alone shows.
    m.key(Key::Esc, KeyMods::NONE);
    hd.draw(|c| App::<Note>::draw(&mut m, c));

    // Idle minutes: the loop woken only at the deadlines the model asks
    // for, each run as the loop runs it, both clocks the model reads
    // moved to the deadline's time. The first minute has the start's
    // one-off looks; the second is counted.
    let from = Instant::now();
    let wall = rush_core::time::now();
    let counted = from + Duration::from_secs(60);
    let until = from + Duration::from_secs(120);
    let (mut wakes, mut frames, mut steps) = (0, 0, 0);
    let mut why: Vec<&str> = Vec::new();
    while let Some(at) = App::<Note>::wake_at(&m) {
        if at > until || steps > 2000 {
            break;
        }
        steps += 1;
        let at = at.max(from);
        let counts = at > counted;
        if counts {
            wakes += 1;
            if let Some((name, _)) = m
                .deadlines()
                .into_iter()
                .find(|(_, t)| t.is_some_and(|t| t.max(from) <= at))
            {
                why.push(name);
            }
        }
        m.instant = Some(at);
        m.clock = Some(wall + at.duration_since(from).as_millis() as i64);
        if m.wake_on(at) == Step::Draw {
            frames += counts as usize;
            hd.draw(|c| App::<Note>::draw(&mut m, c));
        }
    }
    let said = format!(
        "threads {threads} (and {idle} of later's, idle), an idle minute: {wakes} wakes ({why:?}), {frames} frames\n{}",
        hd.plain()
    );
    assert!(threads <= THREADS, "{said}");
    assert!(wakes <= WAKES, "{said}");
    assert!(frames <= FRAMES, "{said}");
}
