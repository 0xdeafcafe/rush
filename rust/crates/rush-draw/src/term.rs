//! The terminal rush runs in. Set up as Go rush's bubbletea program is
//! (cmd/rush/main.go:244 `tea.NewProgram`, internal/ui/view.go:35-38 the
//! view's modes; bubbletea v2 cursed_renderer.go:150-167 what they write):
//! alternate screen, all-motion mouse in SGR encoding, bracketed paste,
//! focus reports and the kitty keyboard's key disambiguation. Given back
//! on drop and on a panic of the thread that took it.
//!
//! Release builds unwind (DESIGN.md "Panics"): a panic on another thread
//! ends only that thread, so it mustn't give the terminal back under the
//! screen still drawing. Its message is kept and printed once the terminal
//! is given back.

use std::io::{self, BufWriter, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::thread::ThreadId;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;

use crate::canvas::Canvas;

// Alternate screen, cursor hidden, mouse any-event + SGR (1003, 1006, as
// tea.MouseModeAllMotion), bracketed paste, focus reports, then kitty
// keyboard flags 1 (bubbletea's "basic key disambiguation", always on).
const ENTER: &str = "\x1b[?1049h\x1b[?25l\x1b[?1003h\x1b[?1006h\x1b[?2004h\x1b[?1004h\x1b[>1u";
// The same undone, in reverse; the cursor shown again.
const LEAVE: &str =
    "\x1b[<1u\x1b[?1004l\x1b[?2004l\x1b[?1006l\x1b[?1003l\x1b[0m\x1b[?25h\x1b[?1049l";

// Whether the terminal is set up, so drop and the panic hook give it back
// once between them.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static HOOK: Once = Once::new();
// The thread that took the terminal: only its panic gives it back.
static OWNER: OnceLock<ThreadId> = OnceLock::new();
// Other threads' panics while the terminal was taken, printed after.
static KEPT: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn enter() -> io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    let mut out = io::stdout().lock();
    out.write_all(ENTER.as_bytes())?;
    out.flush()
}

fn leave() -> io::Result<()> {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    let mut out = io::stdout().lock();
    let w = out.write_all(LEAVE.as_bytes()).and_then(|_| out.flush());
    drop(out);
    let r = crossterm::terminal::disable_raw_mode();
    for m in KEPT
        .lock()
        .map(|mut k| std::mem::take(&mut *k))
        .unwrap_or_default()
    {
        eprintln!("{m}");
    }
    r?;
    w
}

/// The terminal, taken over for rush's screens.
pub struct Term {
    t: Terminal<CrosstermBackend<BufWriter<Stdout>>>,
    title: String,
}

impl Term {
    /// Takes over the terminal. A panic later gives it back before the
    /// panic's message is printed.
    pub fn new() -> io::Result<Term> {
        let _ = OWNER.set(std::thread::current().id());
        HOOK.call_once(|| {
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let me = std::thread::current();
                if ACTIVE.load(Ordering::SeqCst) && OWNER.get() != Some(&me.id()) {
                    let m = format!("thread '{}' {info}", me.name().unwrap_or("<unnamed>"));
                    if let Ok(mut k) = KEPT.lock() {
                        k.push(m);
                    }
                    return;
                }
                let _ = leave();
                prev(info);
            }));
        });
        enter()?;
        // One write per frame: ratatui flushes once it has drawn.
        let out = BufWriter::with_capacity(1 << 16, io::stdout());
        let t = match Terminal::new(CrosstermBackend::new(out)) {
            Ok(t) => t,
            Err(e) => {
                let _ = leave();
                return Err(e);
            }
        };
        Ok(Term {
            t,
            title: String::new(),
        })
    }

    /// The screen's size in cells (width, height).
    pub fn size(&self) -> io::Result<(u16, u16)> {
        let s = crossterm::terminal::size()?;
        Ok(s)
    }

    /// Draws a frame: `f` paints the whole of it, and only the cells that
    /// differ from the last frame are written.
    pub fn draw(&mut self, f: impl FnOnce(&mut Canvas)) -> io::Result<()> {
        self.t.draw(|frame| {
            let mut c = Canvas::new(frame.buffer_mut());
            f(&mut c);
            if let Some((x, y)) = c.cursor() {
                frame.set_cursor_position(Position::new(x, y));
            }
        })?;
        Ok(())
    }

    /// Sets the window title (Go rush's View.WindowTitle); written only
    /// when it changes.
    pub fn set_title(&mut self, title: &str) -> io::Result<()> {
        if self.title == title {
            return Ok(());
        }
        // Control chars would end the title early or run as escapes.
        self.title.clear();
        self.title.extend(title.chars().filter(|c| !c.is_control()));
        let out = self.t.backend_mut();
        write!(out, "\x1b]2;{}\x07", self.title)?;
        out.flush()
    }

    /// Puts text on the clipboard through the terminal (OSC 52), as Go
    /// rush's copy does (tea.SetClipboard).
    pub fn copy(&mut self, text: &str) -> io::Result<()> {
        let out = self.t.backend_mut();
        write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
        out.flush()
    }

    /// Forgets what's on screen, so the next frame is written whole.
    pub fn clear(&mut self) -> io::Result<()> {
        self.t.clear()
    }

    /// Gives the terminal back for a while (to run another program in
    /// it): the alternate screen, raw mode, the mouse, bracketed paste,
    /// focus reports and the kitty keyboard's mode left. Stop the input
    /// reader first ([`crate::Loop::run`] does), or it reads what's typed
    /// meanwhile.
    pub fn suspend(&mut self) -> io::Result<()> {
        self.t.backend_mut().flush()?;
        leave()
    }

    /// Takes the terminal over again after [`Term::suspend`]; the next
    /// frame is written whole.
    pub fn resume(&mut self) -> io::Result<()> {
        enter()?;
        if !self.title.is_empty() {
            let out = self.t.backend_mut();
            write!(out, "\x1b]2;{}\x07", self.title)?;
        }
        self.t.clear()
    }

    /// Runs f with the terminal given back, then takes it over again
    /// (bubbletea's exec: releaseTerminal, the program, RestoreTerminal).
    /// While f runs ^C and ^\ are the program's: rush itself ignores them,
    /// as bubbletea's ignoreSignals and system(3) do, while what f starts
    /// gets them as usual.
    pub fn hand_over<R>(&mut self, f: impl FnOnce() -> R) -> io::Result<R> {
        self.suspend()?;
        let held = HeldSignals::hold();
        let r = f();
        drop(held);
        self.resume()?;
        Ok(r)
    }
}

/// SIGINT and SIGQUIT caught by a handler that does nothing while
/// another program has the terminal, then set back as they were. A
/// handler rather than SIG_IGN: exec sets a caught signal back to its
/// default, so the program started meanwhile still gets them.
struct HeldSignals {
    old: [(libc::c_int, libc::sigaction); 2],
}

extern "C" fn nothing(_: libc::c_int) {}

/// An all-zero sigaction: SIG_DFL, no flags, an empty mask.
fn zeroed_action() -> libc::sigaction {
    // SAFETY: sigaction is plain data; all zeroes is a valid value.
    unsafe { std::mem::zeroed() }
}

impl HeldSignals {
    fn hold() -> HeldSignals {
        let mut old = [
            (libc::SIGINT, zeroed_action()),
            (libc::SIGQUIT, zeroed_action()),
        ];
        for (sig, was) in old.iter_mut() {
            let mut sa = zeroed_action();
            sa.sa_sigaction = nothing as extern "C" fn(libc::c_int) as usize;
            // SAFETY: sa is a valid sigaction; was takes the old one.
            unsafe {
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(*sig, &sa, was);
            }
        }
        HeldSignals { old }
    }
}

impl Drop for HeldSignals {
    fn drop(&mut self) {
        for (sig, was) in &self.old {
            // SAFETY: was is what sigaction gave back.
            unsafe {
                libc::sigaction(*sig, was, std::ptr::null_mut());
            }
        }
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = self.t.backend_mut().flush();
        let _ = leave();
    }
}

/// Standard base64 with padding.
fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_pads() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
        ] {
            assert_eq!(super::base64(i.as_bytes()), o);
        }
    }
}
