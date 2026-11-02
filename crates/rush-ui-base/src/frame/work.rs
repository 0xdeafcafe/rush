//! Work handed off the UI thread and its result handed back: nothing that
//! waits (the disk, the network, another process) runs on the UI thread.
//! After Go's sheet.go:24 (sheetMsg, sheetDo) and the `later` and
//! `applyMsg` it stands beside (model.go): a closure the UI thread runs
//! with the model, sent on a lane of its own (DESIGN.md "Threads").
//! What only waits for a time ([`Model::after`]) waits on the UI thread's
//! own deadline instead, with no thread asleep for it (Rust-only).

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use rush_draw::{Sender, Step};
use rush_sys::threads::Pool;

use super::Out;
use crate::model::Model;
use crate::worker::Note;

/// later's threads: one idle from the last piece takes the next, so each
/// reading of the list (a second apart while an agent works) doesn't
/// start a thread for each piece it hands off; idle for KEEP, one ends.
/// Never fewer than the pieces running: none waits for another.
static LATER: LazyLock<Pool> = LazyLock::new(|| Pool::new("rush-later", 2 << 20, KEEP));

/// How long a later thread waits idle for another piece.
const KEEP: Duration = Duration::from_secs(10);

/// What the UI thread runs with the model when a Note::Apply lands.
pub type Apply = Box<dyn FnOnce(&mut Model) -> Out + Send>;

/// What waits for its time on the UI thread ([`Model::after`]), in the
/// order asked.
#[derive(Default)]
struct Timers(Vec<(Instant, Timed)>);

/// What a timer runs: on the UI thread, so it needn't be Send.
type Timed = Box<dyn FnOnce(&mut Model) -> Out>;

impl Model {
    /// Runs work on a thread of its own (one of LATER's), then apply with
    /// its result on the UI thread (later). With no loop (tests, `rush --render`) both run
    /// at once, as Go's Frame drains what a key started.
    pub fn later<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut Model, T) -> Out + Send + 'static,
    ) {
        let Some(tx) = self.sender() else {
            let v = work();
            let _ = apply(self, v);
            return;
        };
        let tx = tx.lane(1);
        let spawned = LATER.run(move || {
            let v = work();
            tx.send(Note::Apply(Box::new(move |m: &mut Model| apply(m, v))));
        });
        if spawned.is_err() {
            self.fail("couldn't start a thread for that");
        }
    }

    /// Runs apply on the UI thread once d has passed: the loop's own
    /// deadline wakes for it (App::wake_at), as for a spinner, so nothing
    /// sleeps on a thread of its own meanwhile (`later` with a sleep for
    /// work had one asleep for each). With no loop nothing wakes it:
    /// [`Model::run_timers`] runs what's due.
    pub fn after(&mut self, d: Duration, apply: impl FnOnce(&mut Model) -> Out + 'static) {
        let at = self.instant.unwrap_or_else(Instant::now) + d;
        self.kept::<Timers>().0.push((at, Box::new(apply)));
    }

    /// When the soonest of [`Model::after`]'s is due.
    pub fn timer_due(&self) -> Option<Instant> {
        self.kept.peek::<Timers>()?.0.iter().map(|t| t.0).min()
    }

    /// Runs those of [`Model::after`]'s due by now, in the order asked:
    /// whether any calls for a frame, Quit when one ends rush.
    pub fn run_timers(&mut self, now: Instant) -> Step {
        let due: Vec<_> = {
            let t = &mut self.kept::<Timers>().0;
            if !t.iter().any(|t| t.0 <= now) {
                return Step::Idle;
            }
            let (due, wait): (Vec<_>, Vec<_>) =
                std::mem::take(t).into_iter().partition(|t| t.0 <= now);
            *t = wait;
            due
        };
        let mut step = Step::Idle;
        for (_, f) in due {
            match f(self).step() {
                Step::Quit => return Step::Quit,
                Step::Draw => step = Step::Draw,
                Step::Idle => {}
            }
        }
        step
    }

    /// A sender into the loop, for a worker of a screen's own: give it a
    /// lane of its own (`lane(cap)`) and send `Note::Apply`. None with no
    /// loop. Never send from the UI thread itself.
    pub fn sender(&self) -> Option<Sender<Note>> {
        self.tx.clone()
    }
}
