//! The thread that reads the terminal, and its stopping while another
//! program has the terminal. Go rush's bubbletea reads through a
//! cancelreader it cancels before tea.ExecProcess hands the terminal over
//! and starts again after (bubbletea v2.0.9 exec.go:102 exec, tea.go:1332
//! releaseTerminal, :1353 RestoreTerminal); this is the same: paused, the
//! thread reads nothing, so every key goes to the program.
//!
//! It waits in one poll() on the terminal, a pipe SIGWINCH writes to and a
//! pipe that wakes it to pause: idle, it costs nothing. crossterm parses
//! what the terminal sent, asked only once there's something to read, and
//! drained before the thread waits again, so nothing it holds is left
//! behind when it pauses.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use crate::input::{Event, translate};

/// How long a pause waits for the thread to stop reading. It stops at
/// once unless crossterm is in a read of its own (a rare stall, below).
const PAUSE_WAIT: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Reading,
    /// Asked to stop reading.
    Pause,
    /// Not reading until resumed.
    Paused,
}

/// The reader thread's controls.
pub(crate) struct Reader {
    st: Mutex<State>,
    cv: Condvar,
    /// A byte here wakes the thread's wait.
    wake: UnixStream,
}

impl Reader {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stops the thread reading the terminal, and waits until it has.
    pub(crate) fn pause(&self) {
        let mut s = self.lock();
        if *s != State::Reading {
            return;
        }
        *s = State::Pause;
        let _ = (&self.wake).write(&[1]);
        let _ = self
            .cv
            .wait_timeout_while(s, PAUSE_WAIT, |s| *s == State::Pause);
    }

    /// Lets it read again.
    pub(crate) fn resume(&self) {
        *self.lock() = State::Reading;
        self.cv.notify_all();
    }

    /// Waits while paused; the thread's side of pause.
    fn park(&self) {
        let mut s = self.lock();
        loop {
            match *s {
                State::Reading => return,
                // Asked again, maybe before it read on from the last.
                State::Pause => {
                    *s = State::Paused;
                    self.cv.notify_all();
                }
                State::Paused => {}
            }
            s = self.cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// The terminal to wait on: stdin when it's one, else /dev/tty, as
/// crossterm reads (crossterm's tty_fd).
enum Tty {
    Stdin,
    Dev(File),
}

impl Tty {
    fn open() -> io::Result<Tty> {
        // SAFETY: isatty only asks.
        if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
            return Ok(Tty::Stdin);
        }
        Ok(Tty::Dev(File::open("/dev/tty")?))
    }

    fn fd(&self) -> RawFd {
        match self {
            Tty::Stdin => libc::STDIN_FILENO,
            Tty::Dev(f) => f.as_raw_fd(),
        }
    }
}

/// Reads what's waiting in a non-blocking pipe and drops it.
fn drain(s: &UnixStream) {
    let mut b = [0u8; 64];
    while matches!((&*s).read(&mut b), Ok(n) if n > 0) {}
}

/// Starts the thread; push takes each event and says false once nobody
/// wants them, which ends the thread.
pub(crate) fn start(push: impl Fn(Event) -> bool + Send + 'static) -> io::Result<Arc<Reader>> {
    let tty = Tty::open()?;
    let (wake_r, wake_w) = UnixStream::pair()?;
    wake_r.set_nonblocking(true)?;
    wake_w.set_nonblocking(true)?;
    let (winch_r, winch_w) = UnixStream::pair()?;
    winch_r.set_nonblocking(true)?;
    signal_hook::low_level::pipe::register(signal_hook::consts::SIGWINCH, winch_w)?;
    // crossterm's own reader made now, its SIGWINCH watched from here on.
    let _ = crossterm::event::poll(Duration::ZERO);
    let r = Arc::new(Reader {
        st: Mutex::new(State::Reading),
        cv: Condvar::new(),
        wake: wake_w,
    });
    let me = r.clone();
    thread::Builder::new()
        .name("rush-input".into())
        .spawn(move || run(&me, &tty, &wake_r, &winch_r, push))?;
    Ok(r)
}

fn run(r: &Reader, tty: &Tty, wake: &UnixStream, winch: &UnixStream, push: impl Fn(Event) -> bool) {
    // Times the terminal had something crossterm didn't take.
    let mut stuck = 0;
    let mut late =
        LateOsc::armed(crate::term::LATE_OSC.swap(false, std::sync::atomic::Ordering::SeqCst));
    let mut push = |e: crossterm::event::Event| -> bool {
        if late.drops(&e) {
            return true;
        }
        match translate(e) {
            Some(e) => push(e),
            None => true,
        }
    };
    loop {
        r.park();
        let mut fds = [
            libc::pollfd {
                fd: tty.fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: winch.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is three valid pollfds for the call's length.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if fds[1].revents != 0 {
            drain(wake);
            continue; // to park, or read on if resumed already
        }
        let resized = fds[2].revents != 0;
        if resized {
            drain(winch);
        }
        let t = fds[0].revents;
        if t & (libc::POLLHUP | libc::POLLNVAL | libc::POLLERR) != 0 && t & libc::POLLIN == 0 {
            return; // the terminal has gone
        }
        let readable = t & libc::POLLIN != 0;
        if !readable && !resized {
            continue;
        }
        // Everything crossterm can give without waiting.
        let mut got = false;
        loop {
            match crossterm::event::poll(Duration::ZERO) {
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => return,
            }
            got = true;
            match crossterm::event::read() {
                Ok(e) => {
                    if !push(e) {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
        // crossterm waits for the terminal's edges: bytes it left behind
        // a read (more than its buffer came at once) aren't news to it, so
        // the poll above would wake for them forever. Its own blocking
        // read takes them with the next thing typed.
        if readable && !got {
            stuck += 1;
            if stuck > 1 {
                stuck = 0;
                match crossterm::event::read() {
                    Ok(e) => {
                        if !push(e) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        } else {
            stuck = 0;
        }
    }
}

/// A colour reply the terminal sent after rush stopped waiting for it
/// (term.rs ask_colours): crossterm reads `ESC ] 11 ; rgb:… ST` as alt+],
/// its letters, then alt+\ (or ctrl+g for a BEL end). The first such run
/// within a while of starting is dropped, not typed.
pub(crate) struct LateOsc {
    until: Option<std::time::Instant>,
    inside: bool,
}

/// How long after starting a late reply is looked for.
const LATE_FOR: Duration = Duration::from_secs(10);

impl LateOsc {
    pub(crate) fn armed(on: bool) -> LateOsc {
        LateOsc {
            until: on.then(|| std::time::Instant::now() + LATE_FOR),
            inside: false,
        }
    }

    /// Whether e is part of the late reply, to drop.
    pub(crate) fn drops(&mut self, e: &crossterm::event::Event) -> bool {
        use crossterm::event::{Event as E, KeyCode, KeyEventKind, KeyModifiers};
        let Some(until) = self.until else {
            return false;
        };
        let E::Key(k) = e else {
            return self.inside;
        };
        if k.kind == KeyEventKind::Release {
            return self.inside;
        }
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if !self.inside {
            if std::time::Instant::now() > until {
                self.until = None;
                return false;
            }
            if alt && k.code == KeyCode::Char(']') {
                self.inside = true;
                return true;
            }
            return false;
        }
        // Its end: ST (alt+\) or BEL (ctrl+g); then nothing more is dropped.
        if alt && k.code == KeyCode::Char('\\') || ctrl && k.code == KeyCode::Char('g') {
            self.until = None;
            self.inside = false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A colour reply that came late is dropped, ST or BEL ended, once;
    /// keys around it, and a second one, go through (LateOsc).
    #[test]
    fn a_late_colour_reply_is_dropped_once() {
        use crossterm::event::{Event as E, KeyCode, KeyEvent, KeyModifiers as M};
        let k = |c: char, m: M| E::Key(KeyEvent::new(KeyCode::Char(c), m));
        let reply = |end: E| {
            let mut v = vec![k(']', M::ALT)];
            v.extend("11;rgb:1111/1010/0e0e".chars().map(|c| k(c, M::NONE)));
            v.push(end);
            v
        };
        let mut l = LateOsc::armed(true);
        assert!(!l.drops(&k('r', M::NONE)), "a key before it goes through");
        for e in reply(k('\\', M::ALT)) {
            assert!(l.drops(&e), "{e:?}");
        }
        assert!(!l.drops(&k('g', M::NONE)), "a key after it goes through");
        assert!(!l.drops(&k(']', M::ALT)), "only one is dropped");
        let mut l = LateOsc::armed(true);
        for e in reply(k('g', M::CONTROL)) {
            assert!(l.drops(&e), "{e:?}");
        }
        assert!(!l.drops(&k('x', M::NONE)));
        let mut l = LateOsc::armed(false);
        assert!(
            !l.drops(&k(']', M::ALT)),
            "nothing dropped when it answered"
        );
    }

    /// pause returns once the thread has stopped, and it stays stopped
    /// until resumed: its wait is woken through the pipe.
    #[test]
    fn pause_waits_for_the_thread_to_stop() {
        let (wake_r, wake_w) = UnixStream::pair().unwrap();
        wake_r.set_nonblocking(true).unwrap();
        let r = Arc::new(Reader {
            st: Mutex::new(State::Reading),
            cv: Condvar::new(),
            wake: wake_w,
        });
        let turns = Arc::new(AtomicU32::new(0));
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (me, t, d) = (r.clone(), turns.clone(), done.clone());
        let h = thread::spawn(move || {
            loop {
                me.park();
                if d.load(Ordering::SeqCst) {
                    return;
                }
                t.fetch_add(1, Ordering::SeqCst);
                // Stands for the terminal's poll(): blocks until the pipe
                // has a byte.
                let mut fds = [libc::pollfd {
                    fd: wake_r.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                }];
                // SAFETY: one valid pollfd.
                unsafe { libc::poll(fds.as_mut_ptr(), 1, -1) };
                drain(&wake_r);
            }
        });
        while turns.load(Ordering::SeqCst) == 0 {
            thread::yield_now();
        }
        r.pause();
        assert_eq!(*r.lock(), State::Paused);
        assert_eq!(turns.load(Ordering::SeqCst), 1, "stopped, not reading");
        // Paused again before it read on: still stopped, at once.
        r.resume();
        r.pause();
        assert_eq!(*r.lock(), State::Paused);
        // Reading again once resumed.
        r.resume();
        while turns.load(Ordering::SeqCst) == 1 {
            thread::yield_now();
        }
        r.pause();
        assert_eq!(*r.lock(), State::Paused);
        done.store(true, Ordering::SeqCst);
        r.resume();
        h.join().unwrap();
    }
}
