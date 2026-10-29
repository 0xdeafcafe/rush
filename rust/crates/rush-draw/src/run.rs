//! The event loop, idle-free (DESIGN.md "Threads"): the UI thread blocks
//! until the terminal reader thread or one of the app's workers gives it
//! something; it takes what's queued, then draws once. The only timer is
//! a one-shot deadline the app asks for (a spinner), never a fixed tick.
//! Go rush leans on bubbletea's 120 fps renderer instead
//! (cmd/rush/main.go:228, `tea.WithFPS(120)`).
//!
//! Input goes first: what's typed waits in a queue of its own, and the
//! loop takes all of it before each of the workers' messages, so keys
//! never queue behind a flood. Workers can't run ahead of the UI: each
//! sends on a lane holding at most a few messages not yet handled, and a
//! send waits while its lane is full (the UI handling one makes room).

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::canvas::Canvas;
use crate::input::Event;
use crate::reader::{self, Reader};
use crate::term::Term;

/// What the UI thread is woken by: the terminal, or the app's own workers.
#[derive(Debug)]
pub enum Msg<T> {
    Input(Event),
    App(T),
}

/// How many of a lane's messages may wait unhandled, unless it's given
/// its own number ([`Sender::lane`]).
pub const LANE: usize = 4;

/// One worker's share of the queue: how many of its messages wait.
struct Lane {
    cap: usize,
    pending: AtomicUsize,
}

struct Queues<T> {
    input: VecDeque<Event>,
    app: VecDeque<(T, Arc<Lane>)>,
    /// The loop has ended: sends fail, and waiting ones give up.
    closed: bool,
}

struct Shared<T> {
    q: Mutex<Queues<T>>,
    /// The loop waits here for something to do.
    wake: Condvar,
    /// Senders wait here for room in their lane.
    room: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, Queues<T>> {
        self.q.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Sends the app's own messages into the loop, on a lane of its own; a
/// clone shares the lane. A send waits while the lane is full, so never
/// send from the UI thread itself. It fails once the loop is gone.
pub struct Sender<T> {
    shared: Arc<Shared<T>>,
    lane: Arc<Lane>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender {
            shared: self.shared.clone(),
            lane: self.lane.clone(),
        }
    }
}

impl<T> Sender<T> {
    /// A sender on a new lane of its own, holding at most cap messages not
    /// yet handled (at least one): one per worker.
    pub fn lane(&self, cap: usize) -> Sender<T> {
        Sender {
            shared: self.shared.clone(),
            lane: Arc::new(Lane {
                cap: cap.max(1),
                pending: AtomicUsize::new(0),
            }),
        }
    }

    /// Sends `t`, waiting while the lane is full; false once the loop has
    /// ended.
    pub fn send(&self, t: T) -> bool {
        let mut q = self.shared.lock();
        while !q.closed && self.lane.pending.load(Ordering::Relaxed) >= self.lane.cap {
            q = self.shared.room.wait(q).unwrap_or_else(|e| e.into_inner());
        }
        if q.closed {
            return false;
        }
        self.lane.pending.fetch_add(1, Ordering::Relaxed);
        q.app.push_back((t, self.lane.clone()));
        self.shared.wake.notify_one();
        true
    }

    /// Sends `t` only if its lane has room now, never waiting: for a
    /// thread that must not stall, such as an agent's reader calling a
    /// `Sink` (rush_core::adapter::Sink). Gives `t` back when the lane is
    /// full or the loop has ended, for the caller to hold, coalesce with
    /// what comes next, or drop.
    pub fn try_send(&self, t: T) -> Result<(), TrySend<T>> {
        let mut q = self.shared.lock();
        if q.closed {
            return Err(TrySend::Closed(t));
        }
        if self.lane.pending.load(Ordering::Relaxed) >= self.lane.cap {
            return Err(TrySend::Full(t));
        }
        self.lane.pending.fetch_add(1, Ordering::Relaxed);
        q.app.push_back((t, self.lane.clone()));
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Sends a terminal event, as if typed (for tests and replays).
    pub fn send_input(&self, e: Event) -> bool {
        push_input(&self.shared, e)
    }
}

/// Why [`Sender::try_send`] didn't send, with what it was given back.
#[derive(Debug, PartialEq, Eq)]
pub enum TrySend<T> {
    /// The lane holds all the UI hasn't handled that it may.
    Full(T),
    /// The loop has ended.
    Closed(T),
}

fn push_input<T>(shared: &Shared<T>, e: Event) -> bool {
    let mut q = shared.lock();
    if q.closed {
        return false;
    }
    q.input.push_back(e);
    shared.wake.notify_one();
    true
}

/// What handling a message calls for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing on screen changed.
    Idle,
    /// Draw once what's queued is handled.
    Draw,
    /// End the loop.
    Quit,
}

/// An app run by [`Loop`].
pub trait App<T> {
    /// Handles one message.
    fn update(&mut self, msg: Msg<T>) -> Step;
    /// Paints the whole frame.
    fn draw(&mut self, c: &mut Canvas);
    /// When to be woken with nothing else happening (a spinner's next
    /// frame); asked after every draw. None sleeps until a message.
    fn wake_at(&self) -> Option<Instant> {
        None
    }
    /// The deadline from [`App::wake_at`] passed.
    fn wake(&mut self) -> Step {
        Step::Draw
    }
    /// The window title; written when it changes.
    fn title(&self) -> Option<&str> {
        None
    }
    /// Text to put on the clipboard (OSC 52), taken once a frame.
    fn clipboard(&mut self) -> Option<String> {
        None
    }
    /// A program to hand the terminal to, asked for by the message just
    /// handled (Go's tea.ExecProcess): taken after each one. The loop
    /// stops reading the terminal, gives it back, runs it, takes the
    /// terminal again, hands the app what it returned, and draws whole.
    fn outside(&mut self) -> Option<Outside<T>> {
        None
    }
}

/// What runs with the terminal given back ([`App::outside`]): it runs the
/// program, waiting for it, and returns the message that says how it went.
pub type Outside<T> = Box<dyn FnOnce() -> T>;

// Past this, workers' queued messages yield to a frame, so the screen
// keeps up while a worker floods. Input is never held back by it.
const DRAIN_BUDGET: Duration = Duration::from_millis(16);

/// The loop's queues. Make it first, hand [`Loop::sender`]s to workers,
/// then [`Loop::run`]. Dropping it ends every send.
pub struct Loop<T> {
    shared: Arc<Shared<T>>,
    /// The terminal's reader, once started.
    reader: Mutex<Option<Arc<Reader>>>,
}

impl<T: Send + 'static> Default for Loop<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for Loop<T> {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.room.notify_all();
    }
}

/// What the loop takes next.
enum Next<T> {
    Input(Event),
    App(T, Arc<Lane>),
}

impl<T: Send + 'static> Loop<T> {
    pub fn new() -> Loop<T> {
        Loop {
            shared: Arc::new(Shared {
                q: Mutex::new(Queues {
                    input: VecDeque::new(),
                    app: VecDeque::new(),
                    closed: false,
                }),
                wake: Condvar::new(),
                room: Condvar::new(),
            }),
            reader: Mutex::new(None),
        }
    }

    /// A sender on a new lane of [`LANE`] messages.
    pub fn sender(&self) -> Sender<T> {
        Sender {
            shared: self.shared.clone(),
            lane: Arc::new(Lane {
                cap: LANE,
                pending: AtomicUsize::new(0),
            }),
        }
    }

    /// Starts the thread that reads the terminal. It blocks on the
    /// terminal, so it costs nothing while nothing is typed, stops reading
    /// while another program has the terminal ([`App::outside`]), and ends
    /// once the loop is gone and the next event arrives.
    pub fn read_input(&self) -> io::Result<()> {
        let shared = self.shared.clone();
        let r = reader::start(move |e| push_input(&shared, e))?;
        *self.reader.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        Ok(())
    }

    /// The terminal's reader, when started.
    fn reader(&self) -> Option<Arc<Reader>> {
        self.reader
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Runs a program with the terminal handed to it: the reader stopped,
    /// the terminal given back, then both taken again; the screen's size
    /// read again after, as it may have changed meanwhile (bubbletea's
    /// RestoreTerminal: checkResize).
    fn hand_over(&self, term: &mut Term, run: Outside<T>) -> io::Result<T> {
        let reader = self.reader();
        if let Some(r) = &reader {
            r.pause();
        }
        let r = term.hand_over(run);
        if let Some(r) = &reader {
            r.resume();
        }
        if let Ok((w, h)) = term.size() {
            push_input(&self.shared, Event::Resize(w, h));
        }
        r
    }

    /// The next thing to handle, input first; app messages only when
    /// `app` allows. Waits until `until` (forever when None) when `wait`.
    fn next(&self, wait: bool, until: Option<Instant>, app: bool) -> Option<Next<T>> {
        let mut q = self.shared.lock();
        loop {
            if let Some(e) = q.input.pop_front() {
                return Some(Next::Input(e));
            }
            if app && let Some((t, lane)) = q.app.pop_front() {
                return Some(Next::App(t, lane));
            }
            if !wait {
                return None;
            }
            q = match until {
                None => self.shared.wake.wait(q).unwrap_or_else(|e| e.into_inner()),
                Some(at) => {
                    let left = at.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return None;
                    }
                    self.shared
                        .wake
                        .wait_timeout(q, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
            };
        }
    }

    /// Hands one to the app; a worker's message, once handled, makes room
    /// in its lane.
    fn handle<A: App<T>>(&self, app: &mut A, n: Next<T>) -> Step {
        match n {
            Next::Input(e) => app.update(Msg::Input(e)),
            Next::App(t, lane) => {
                let step = app.update(Msg::App(t));
                let _q = self.shared.lock();
                lane.pending.fetch_sub(1, Ordering::Relaxed);
                self.shared.room.notify_all();
                step
            }
        }
    }

    /// Runs `app` on the terminal until it quits.
    pub fn run(&self, term: &mut Term, app: &mut impl App<T>) -> io::Result<()> {
        self.drive(
            app,
            term,
            |term, app| {
                if let Some(t) = app.title() {
                    term.set_title(t)?;
                }
                if let Some(t) = app.clipboard() {
                    term.copy(&t)?;
                }
                term.draw(|c| app.draw(c))
            },
            |term, run| self.hand_over(term, run),
        )
    }

    /// Runs `app`, calling `present` for each frame (to draw it somewhere
    /// other than the terminal, as tests do). Draws first, then: block for
    /// something to do (or the app's deadline), take what's queued (all
    /// the input, the workers' messages for up to a frame's time), draw
    /// once. A program the app asks for ([`App::outside`]) runs at once,
    /// with no terminal to give back.
    pub fn run_with<A: App<T>>(
        &self,
        app: &mut A,
        mut present: impl FnMut(&mut A) -> io::Result<()>,
    ) -> io::Result<()> {
        self.drive(app, &mut (), |_, app| present(app), |_, run| Ok(run()))
    }

    /// The loop itself, drawing with present on s and running what the
    /// app asks to run outside with hand_over.
    fn drive<A: App<T>, S>(
        &self,
        app: &mut A,
        s: &mut S,
        mut present: impl FnMut(&mut S, &mut A) -> io::Result<()>,
        mut hand_over: impl FnMut(&mut S, Outside<T>) -> io::Result<T>,
    ) -> io::Result<()> {
        // A message handled, then any program it asked for: run, and what
        // it said handled in turn.
        let mut step = |app: &mut A, s: &mut S, mut st: Step| -> io::Result<Step> {
            while st != Step::Quit
                && let Some(run) = app.outside()
            {
                let t = hand_over(s, run)?;
                st = match app.update(Msg::App(t)) {
                    Step::Quit => Step::Quit,
                    // The screen was given away: drawn again whole.
                    _ => Step::Draw,
                };
            }
            Ok(st)
        };
        present(s, app)?;
        loop {
            let mut draw = false;
            let st = match self.next(true, app.wake_at(), true) {
                Some(n) => self.handle(app, n),
                None => app.wake(),
            };
            match step(app, s, st)? {
                Step::Quit => return Ok(()),
                Step::Draw => draw = true,
                Step::Idle => {}
            }
            let start = Instant::now();
            while let Some(n) = self.next(false, None, start.elapsed() < DRAIN_BUDGET) {
                let st = self.handle(app, n);
                match step(app, s, st)? {
                    Step::Quit => return Ok(()),
                    Step::Draw => draw = true,
                    Step::Idle => {}
                }
            }
            if draw {
                present(s, app)?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::Headless;
    use crate::input::Key;
    use rush_core::text::Style;
    use std::thread;

    struct Counter {
        seen: Vec<String>,
        n: u32,
        wake: Option<Instant>,
        woke: u32,
    }

    impl App<u32> for Counter {
        fn update(&mut self, msg: Msg<u32>) -> Step {
            match msg {
                Msg::App(0) => Step::Quit,
                Msg::App(n) => {
                    self.n += n;
                    self.seen.push(n.to_string());
                    Step::Draw
                }
                Msg::Input(e) => {
                    self.seen.push(e.key_string().unwrap_or_default());
                    Step::Idle
                }
            }
        }
        fn draw(&mut self, c: &mut Canvas) {
            c.put_str(0, 0, &self.n.to_string(), Style::default(), 10);
        }
        fn wake_at(&self) -> Option<Instant> {
            self.wake
        }
        fn wake(&mut self) -> Step {
            self.woke += 1;
            self.wake = None;
            Step::Draw
        }
    }

    fn counter() -> Counter {
        Counter {
            seen: vec![],
            n: 0,
            wake: None,
            woke: 0,
        }
    }

    #[test]
    fn drains_the_queue_then_draws_once() {
        let l: Loop<u32> = Loop::new();
        let tx = l.sender();
        tx.send(1);
        tx.send_input(Event::key(Key::Char('q')));
        tx.send(2);
        let mut app = counter();
        let mut screen = Headless::new(10, 1);
        let mut frames = 0;
        l.run_with(&mut app, |a| {
            frames += 1;
            screen.draw(|c| a.draw(c));
            if frames == 2 {
                tx.send(0);
            }
            Ok(())
        })
        .unwrap();
        // What was typed first, then the workers' messages.
        assert_eq!(app.seen, ["q", "1", "2"]);
        // The first frame, then one for all three.
        assert_eq!(frames, 2);
        assert_eq!(screen.plain(), "3");
        assert_eq!(app.n, 3);
    }

    #[test]
    fn wakes_at_the_deadline_without_messages() {
        let l: Loop<u32> = Loop::new();
        let tx = l.sender();
        let mut app = counter();
        app.wake = Some(Instant::now());
        let mut frames = 0;
        l.run_with(&mut app, |a| {
            frames += 1;
            if a.woke == 1 {
                tx.send(0);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(app.woke, 1);
        assert_eq!(frames, 2);
    }

    /// A worker can't run ahead: its second message waits until the UI
    /// has handled the first.
    #[test]
    fn a_full_lane_waits_for_the_ui() {
        use std::sync::atomic::AtomicBool;
        struct Ahead {
            sent: Arc<AtomicBool>,
            early: bool,
            n: u32,
        }
        impl App<u32> for Ahead {
            fn update(&mut self, msg: Msg<u32>) -> Step {
                match msg {
                    Msg::App(0) => Step::Quit,
                    Msg::App(1) => {
                        // The second send can't have finished yet.
                        self.early |= self.sent.load(Ordering::SeqCst);
                        self.n += 1;
                        Step::Idle
                    }
                    _ => {
                        self.n += 1;
                        Step::Idle
                    }
                }
            }
            fn draw(&mut self, _: &mut Canvas) {}
        }
        let l: Loop<u32> = Loop::new();
        let tx = l.sender().lane(1);
        let sent = Arc::new(AtomicBool::new(false));
        let s2 = sent.clone();
        let t2 = tx.clone();
        tx.send(1);
        let h = thread::spawn(move || {
            t2.send(2);
            s2.store(true, Ordering::SeqCst);
            t2.send(0);
        });
        let mut app = Ahead {
            sent,
            early: false,
            n: 0,
        };
        l.run_with(&mut app, |_| Ok(())).unwrap();
        h.join().unwrap();
        assert!(!app.early, "the worker ran ahead of the UI");
        assert_eq!(app.n, 2);
    }

    /// try_send never waits: a full lane hands the message back, and so
    /// does a loop that has ended.
    #[test]
    fn try_send_never_waits() {
        let l: Loop<u32> = Loop::new();
        let tx = l.sender().lane(1);
        assert_eq!(tx.try_send(1), Ok(()));
        assert_eq!(tx.try_send(2), Err(TrySend::Full(2)));
        tx.shared.lock().closed = true;
        assert_eq!(tx.try_send(3), Err(TrySend::Closed(3)));
    }

    /// A send waiting on a full lane gives up once the loop is gone.
    #[test]
    fn sends_end_with_the_loop() {
        let l: Loop<u32> = Loop::new();
        let tx = l.sender().lane(1);
        assert!(tx.send(1));
        let t2 = tx.clone();
        let h = thread::spawn(move || t2.send(2));
        drop(l);
        assert!(!h.join().unwrap());
        assert!(!tx.send(3));
    }

    /// A program the app asks for runs once the message that asked is
    /// handled, before the next; what it returned is handed back, and the
    /// screen drawn after it.
    #[test]
    fn outside_runs_between_messages_and_draws_after() {
        struct Asks {
            seen: Vec<u32>,
            want: Option<u32>,
        }
        impl App<u32> for Asks {
            fn update(&mut self, msg: Msg<u32>) -> Step {
                match msg {
                    Msg::App(0) => Step::Quit,
                    Msg::App(n) => {
                        self.seen.push(n);
                        if n == 1 {
                            self.want = Some(100);
                        }
                        Step::Idle
                    }
                    Msg::Input(_) => Step::Idle,
                }
            }
            fn draw(&mut self, _: &mut Canvas) {}
            fn outside(&mut self) -> Option<Outside<u32>> {
                let n = self.want.take()?;
                Some(Box::new(move || n))
            }
        }
        let l: Loop<u32> = Loop::new();
        let tx = l.sender();
        tx.send(1);
        tx.send(2);
        let mut app = Asks {
            seen: vec![],
            want: None,
        };
        let mut frames = 0;
        l.run_with(&mut app, |a| {
            frames += 1;
            if frames == 2 {
                assert_eq!(a.seen, [1, 100, 2]);
                tx.send(0);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(app.seen, [1, 100, 2]);
        // Every message was Idle: only the program's return drew.
        assert_eq!(frames, 2);
    }
}
