//! The thread that reads the terminal, and its stopping while another
//! program has the terminal. Go rush's bubbletea reads through a
//! cancelreader it cancels before tea.ExecProcess hands the terminal over
//! and starts again after (bubbletea v2.0.9 exec.go:102 exec, tea.go:1332
//! releaseTerminal, :1353 RestoreTerminal); this is the same: paused, the
//! thread reads nothing, so every key goes to the program.
//!
//! It waits in one poll() on the terminal, a pipe SIGWINCH writes to, a
//! pipe that wakes it to pause, and a pipe for each signal the app hears
//! ([`crate::Loop::on_signal`]: `rush reload`'s SIGUSR1, with no thread of
//! its own): idle, it costs nothing. crossterm parses
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

/// A signal the app hears: the pipe it writes to, and what to do, which
/// says false once nobody wants it.
pub(crate) type Signal = (UnixStream, Box<dyn Fn() -> bool + Send>);

/// Reads what's waiting in a non-blocking pipe and drops it.
fn drain(s: &UnixStream) {
    let mut b = [0u8; 64];
    while matches!((&*s).read(&mut b), Ok(n) if n > 0) {}
}

/// Starts the thread; push takes each event and says false once nobody
/// wants them, which ends the thread.
pub(crate) fn start(
    push: impl Fn(Event) -> bool + Send + 'static,
    signals: Vec<Signal>,
) -> io::Result<Arc<Reader>> {
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
        .spawn(move || {
            crate::qos::interactive();
            run(&me, &tty, &wake_r, &winch_r, &signals, push)
        })?;
    Ok(r)
}

fn run(
    r: &Reader,
    tty: &Tty,
    wake: &UnixStream,
    winch: &UnixStream,
    signals: &[Signal],
    push: impl Fn(Event) -> bool,
) {
    // Times the terminal had something crossterm didn't take.
    let mut stuck = 0;
    let mut late =
        LateOsc::armed(crate::term::LATE_OSC.swap(0, std::sync::atomic::Ordering::SeqCst));
    let mut push = |e: crossterm::event::Event| -> bool {
        // Colours asked again since (term.rs ask_colours_again).
        let more = crate::term::LATE_OSC.swap(0, std::sync::atomic::Ordering::SeqCst);
        if more > 0 {
            late.arm(more);
        }
        // Back in focus: the terminal's colours may have changed while
        // rush was away (its theme, or the system's light or dark).
        if matches!(e, crossterm::event::Event::FocusGained) {
            crate::term::ask_colours_again();
        }
        if late.drops(&e) {
            if let Some(body) = late.reply.take() {
                crate::term::late_colour(&body);
            }
            return true;
        }
        match translate(e) {
            Some(e) => push(e),
            None => true,
        }
    };
    loop {
        r.park();
        let pfd = |fd: RawFd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let mut fds: Vec<libc::pollfd> = [tty.fd(), wake.as_raw_fd(), winch.as_raw_fd()]
            .into_iter()
            .chain(signals.iter().map(|(p, _)| p.as_raw_fd()))
            .map(pfd)
            .collect();
        // SAFETY: fds is valid pollfds for the call's length.
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
        for ((p, heard), f) in signals.iter().zip(&fds[3..]) {
            if f.revents != 0 {
                drain(p);
                heard();
            }
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

/// The colour replies the terminal sent after rush stopped waiting for
/// them (term.rs ask_colours): crossterm reads `ESC ] 11 ; rgb:… ST` as
/// alt+], its letters, then alt+\ (or ctrl+g for a BEL end). As many as
/// were owed, within a while of starting, are taken in as colours, not
/// typed.
pub(crate) struct LateOsc {
    until: Option<std::time::Instant>,
    /// Replies still owed.
    owed: u8,
    /// The reply being read, its body so far, once alt+] has come.
    inside: Option<String>,
    /// A reply read whole, between `ESC ]` and its end, to take.
    pub(crate) reply: Option<String>,
}

/// How long after starting a late reply is looked for.
const LATE_FOR: Duration = Duration::from_secs(10);

impl LateOsc {
    pub(crate) fn armed(owed: u8) -> LateOsc {
        LateOsc {
            until: (owed > 0).then(|| std::time::Instant::now() + LATE_FOR),
            owed,
            inside: None,
            reply: None,
        }
    }

    /// Owes more replies, asked again: looked for a while from now.
    pub(crate) fn arm(&mut self, more: u8) {
        self.owed = self.owed.saturating_add(more);
        self.until = Some(std::time::Instant::now() + LATE_FOR);
    }

    /// Whether e is part of a late reply, to drop; a reply read whole is
    /// left in reply.
    pub(crate) fn drops(&mut self, e: &crossterm::event::Event) -> bool {
        use crossterm::event::{Event as E, KeyCode, KeyEventKind, KeyModifiers};
        let Some(until) = self.until else {
            return false;
        };
        let E::Key(k) = e else {
            return self.inside.is_some();
        };
        if k.kind == KeyEventKind::Release {
            return self.inside.is_some();
        }
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let Some(body) = self.inside.as_mut() else {
            if std::time::Instant::now() > until {
                self.until = None;
                return false;
            }
            if alt && k.code == KeyCode::Char(']') {
                self.inside = Some(String::new());
                return true;
            }
            // ESC _: the answer about pictures (kitty.rs QUERY), taken in
            // and dropped (late_colour takes no `_` body as a colour).
            if alt && k.code == KeyCode::Char('_') {
                self.inside = Some("_".into());
                return true;
            }
            return false;
        };
        // Its end: ST (alt+\) or BEL (ctrl+g).
        if alt && k.code == KeyCode::Char('\\') || ctrl && k.code == KeyCode::Char('g') {
            self.reply = self.inside.take();
            self.owed -= 1;
            if self.owed == 0 {
                self.until = None;
            }
        } else if let KeyCode::Char(c) = k.code
            && body.len() < 64
        {
            body.push(c);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Colour replies that came late, ST or BEL ended, are taken in as
    /// many as were owed; keys around them, and one more, go through
    /// (LateOsc).
    #[test]
    fn late_colour_replies_are_taken_in() {
        use crossterm::event::{Event as E, KeyCode, KeyEvent, KeyModifiers as M};
        let k = |c: char, m: M| E::Key(KeyEvent::new(KeyCode::Char(c), m));
        let reply = |body: &str, end: E| {
            let mut v = vec![k(']', M::ALT)];
            v.extend(body.chars().map(|c| k(c, M::NONE)));
            v.push(end);
            v
        };
        let take = |l: &mut LateOsc, evs: Vec<E>| {
            for e in evs {
                assert!(l.drops(&e), "{e:?}");
            }
            l.reply.take()
        };
        let mut l = LateOsc::armed(1);
        assert!(!l.drops(&k('r', M::NONE)), "a key before it goes through");
        let got = take(&mut l, reply("11;rgb:1111/1010/0e0e", k('\\', M::ALT)));
        assert_eq!(got.as_deref(), Some("11;rgb:1111/1010/0e0e"));
        assert!(!l.drops(&k('g', M::NONE)), "a key after it goes through");
        assert!(!l.drops(&k(']', M::ALT)), "only as many as owed");
        let mut l = LateOsc::armed(2);
        let a = take(&mut l, reply("10;rgb:e2/dd/d3", k('g', M::CONTROL)));
        let b = take(&mut l, reply("11;rgb:11/10/0e", k('\\', M::ALT)));
        assert_eq!(
            (a.as_deref(), b.as_deref()),
            (Some("10;rgb:e2/dd/d3"), Some("11;rgb:11/10/0e"))
        );
        assert!(!l.drops(&k('x', M::NONE)));
        // A late answer about pictures (ESC _ G … ST) is taken in too,
        // never typed; it's no colour.
        let mut l = LateOsc::armed(1);
        let mut evs = vec![k('_', M::ALT)];
        evs.extend("Gi=31;OK".chars().map(|c| k(c, M::NONE)));
        evs.push(k('\\', M::ALT));
        assert_eq!(take(&mut l, evs).as_deref(), Some("_Gi=31;OK"));
        assert!(!l.drops(&k('x', M::NONE)), "a key after it goes through");
        let mut l = LateOsc::armed(0);
        assert!(!l.drops(&k(']', M::ALT)), "nothing taken when it answered");
        // Asked again (focus, the system going light or dark): its
        // answers are taken in too, then keys go through again.
        l.arm(2);
        assert!(!l.drops(&k('q', M::NONE)), "a key before them goes through");
        let a = take(&mut l, reply("10;rgb:11/10/0e", k('\\', M::ALT)));
        let b = take(&mut l, reply("11;rgb:ff/ff/ff", k('\\', M::ALT)));
        assert_eq!(
            (a.as_deref(), b.as_deref()),
            (Some("10;rgb:11/10/0e"), Some("11;rgb:ff/ff/ff"))
        );
        assert!(!l.drops(&k(']', M::ALT)), "only as many as asked");
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
