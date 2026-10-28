//! The terminal rush runs in. Set up as Go rush's bubbletea program is
//! (cmd/rush/main.go:244 `tea.NewProgram`, internal/ui/view.go:35-38 the
//! view's modes; bubbletea v2 cursed_renderer.go:150-167 what they write):
//! alternate screen, all-motion mouse in SGR encoding, bracketed paste,
//! focus reports and the kitty keyboard's key disambiguation. Given back
//! on drop and on panic.

use std::io::{self, BufWriter, Stdout, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

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
    crossterm::terminal::disable_raw_mode()?;
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
        HOOK.call_once(|| {
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
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

    /// Forgets what's on screen, so the next frame is written whole.
    pub fn clear(&mut self) -> io::Result<()> {
        self.t.clear()
    }

    /// Gives the terminal back for a while (to run another program in
    /// it). The input reader keeps reading meanwhile.
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
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = self.t.backend_mut().flush();
        let _ = leave();
    }
}
