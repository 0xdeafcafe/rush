//! The event loop, idle-free (DESIGN.md "Threads"): the UI thread blocks
//! on one channel that the terminal reader thread and the app's workers
//! both send on; it takes everything queued, then draws once. The only
//! timer is a one-shot deadline the app asks for (a spinner), never a
//! fixed tick. Go rush leans on bubbletea's 120 fps renderer instead
//! (cmd/rush/main.go:228, `tea.WithFPS(120)`).

use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crate::canvas::Canvas;
use crate::input::{Event, translate};
use crate::term::Term;

/// What the UI thread is woken by: the terminal, or the app's own workers.
#[derive(Debug)]
pub enum Msg<T> {
    Input(Event),
    App(T),
}

/// Sends the app's own messages into the loop; cheap to clone, one per
/// worker. A send fails once the loop is gone.
pub struct Sender<T>(mpsc::Sender<Msg<T>>);

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender(self.0.clone())
    }
}

impl<T> Sender<T> {
    /// Sends `t`; false once the loop has ended.
    pub fn send(&self, t: T) -> bool {
        self.0.send(Msg::App(t)).is_ok()
    }
    /// Sends a terminal event, as if typed (for tests and replays).
    pub fn send_input(&self, e: Event) -> bool {
        self.0.send(Msg::Input(e)).is_ok()
    }
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
}

// Past this, a flood of queued messages yields to a frame, so the screen
// keeps up while a worker floods.
const DRAIN_BUDGET: Duration = Duration::from_millis(16);

/// The loop's channel. Make it first, hand [`Loop::sender`]s to workers,
/// then [`Loop::run`].
pub struct Loop<T> {
    tx: mpsc::Sender<Msg<T>>,
    rx: Receiver<Msg<T>>,
}

impl<T: Send + 'static> Default for Loop<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> Loop<T> {
    pub fn new() -> Loop<T> {
        let (tx, rx) = mpsc::channel();
        Loop { tx, rx }
    }

    pub fn sender(&self) -> Sender<T> {
        Sender(self.tx.clone())
    }

    /// Starts the thread that reads the terminal. It blocks on the
    /// terminal, so it costs nothing while nothing is typed, and ends once
    /// the loop is gone and the next event arrives.
    pub fn read_input(&self) -> io::Result<()> {
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("rush-input".into())
            .spawn(move || {
                while let Ok(e) = crossterm::event::read() {
                    if let Some(e) = translate(e)
                        && tx.send(Msg::Input(e)).is_err()
                    {
                        return;
                    }
                }
            })?;
        Ok(())
    }

    /// Runs `app` on the terminal until it quits.
    pub fn run(&self, term: &mut Term, app: &mut impl App<T>) -> io::Result<()> {
        self.run_with(app, |app| {
            if let Some(t) = app.title() {
                term.set_title(t)?;
            }
            term.draw(|c| app.draw(c))
        })
    }

    /// Runs `app`, calling `present` for each frame (to draw it somewhere
    /// other than the terminal, as tests do). Draws first, then: block for
    /// a message (or the app's deadline), take all that's queued, draw once.
    pub fn run_with<A: App<T>>(
        &self,
        app: &mut A,
        mut present: impl FnMut(&mut A) -> io::Result<()>,
    ) -> io::Result<()> {
        present(app)?;
        loop {
            let first = match app.wake_at() {
                None => match self.rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return Ok(()), // can't happen: we hold a sender
                },
                Some(at) => {
                    let wait = at.saturating_duration_since(Instant::now());
                    match self.rx.recv_timeout(wait) {
                        Ok(m) => Some(m),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return Ok(()),
                    }
                }
            };
            let mut draw = false;
            let step = match first {
                Some(m) => app.update(m),
                None => app.wake(),
            };
            match step {
                Step::Quit => return Ok(()),
                Step::Draw => draw = true,
                Step::Idle => {}
            }
            let start = Instant::now();
            loop {
                match self.rx.try_recv() {
                    Ok(m) => match app.update(m) {
                        Step::Quit => return Ok(()),
                        Step::Draw => draw = true,
                        Step::Idle => {}
                    },
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
                if start.elapsed() >= DRAIN_BUDGET {
                    break;
                }
            }
            if draw {
                present(app)?;
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
        assert_eq!(app.seen, ["1", "q", "2"]);
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
}
