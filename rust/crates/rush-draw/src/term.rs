//! The terminal rush runs in. Set up as Go rush's bubbletea program is
//! (cmd/rush/main.go:244 `tea.NewProgram`, internal/ui/view.go:35-38 the
//! view's modes; bubbletea v2 cursed_renderer.go:150-167 what they write):
//! alternate screen, all-motion mouse in SGR encoding, bracketed paste,
//! focus reports and the kitty keyboard's key disambiguation. Given back
//! on drop and on a panic of the thread that took it.
//!
//! Frames are diffed and written here rather than by ratatui's Terminal,
//! so each cell's link is written as OSC 8 around its text, as bubbletea's
//! renderer does with the escapes Go's rows carry (ultraviolet
//! terminal_renderer.go:581 ConvertLink; ui/finish.go:400 link). Where
//! Go's colour profile is NoTTY (colorprofile env.go:72 colorProfile: not
//! a terminal, or TERM unset or dumb, unless forced) links are left out.
//!
//! Release builds unwind (DESIGN.md "Panics"): a panic on another thread
//! ends only that thread, so it mustn't give the terminal back under the
//! screen still drawing. Its message is kept and printed once the terminal
//! is given back.

use std::io::{self, BufWriter, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::thread::ThreadId;

use ratatui::backend::{Backend, ClearType, CrosstermBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect as RRect};
use rush_core::text::{self, Link};

use crate::canvas::Canvas;

// Alternate screen, cursor hidden, mouse any-event + SGR (1003, 1006, as
// tea.MouseModeAllMotion), bracketed paste, focus reports, then kitty
// keyboard flags 1 (bubbletea's "basic key disambiguation", always on;
// cursed_renderer.go:162-167). bubbletea also asks for xterm's
// modifyOtherKeys 2, whose CSI 27;mod;code~ crossterm can't read (it
// drops them, keys and all), so that's left off: a terminal without the
// kitty keyboard sends ctrl+\ ] ^ _ as the bytes 0x1c-0x1f, which
// input.rs names as bubbletea does.
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
    out: CrosstermBackend<BufWriter<Stdout>>,
    /// The frame being drawn and the one on screen, and their links.
    bufs: [Buffer; 2],
    links: [Vec<Link>; 2],
    cur: usize,
    /// Whether links are written (Go: the profile isn't NoTTY).
    linked: bool,
    title: String,
}

/// A frame's cell to write: where, the cell, its link.
type Update<'a> = (u16, u16, &'a Cell, Link);

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
        let (w, h) = match crossterm::terminal::size() {
            Ok(s) => s,
            Err(e) => {
                let _ = leave();
                return Err(e);
            }
        };
        let area = RRect::new(0, 0, w, h);
        let n = w as usize * h as usize;
        let tty = std::io::IsTerminal::is_terminal(&io::stdout());
        let mut t = Term {
            out: CrosstermBackend::new(out),
            bufs: [Buffer::empty(area), Buffer::empty(area)],
            links: [vec![Link::NONE; n], vec![Link::NONE; n]],
            cur: 0,
            linked: links_ok(tty, |k| std::env::var(k).ok()),
            title: String::new(),
        };
        if let Err(e) = t.clear() {
            let _ = leave();
            return Err(e);
        }
        Ok(t)
    }

    /// The screen's size in cells (width, height).
    pub fn size(&self) -> io::Result<(u16, u16)> {
        let s = crossterm::terminal::size()?;
        Ok(s)
    }

    /// Draws a frame: `f` paints the whole of it, and only the cells that
    /// differ from the last frame are written.
    pub fn draw(&mut self, f: impl FnOnce(&mut Canvas)) -> io::Result<()> {
        // A new size is drawn whole on a cleared screen (ratatui's
        // autoresize).
        let (w, h) = self.size()?;
        if (w, h) != (self.bufs[0].area.width, self.bufs[0].area.height) {
            let area = RRect::new(0, 0, w, h);
            for i in 0..2 {
                self.bufs[i].resize(area);
                self.links[i].resize(w as usize * h as usize, Link::NONE);
            }
            self.clear()?;
        }
        let cur = self.cur;
        self.bufs[cur].reset();
        self.links[cur].fill(Link::NONE);
        let cursor = {
            let [b0, b1] = &mut self.bufs;
            let [l0, l1] = &mut self.links;
            let (b, l) = if cur == 0 { (b0, l0) } else { (b1, l1) };
            let mut c = Canvas::new(b, l);
            f(&mut c);
            c.cursor()
        };
        let (prev, next) = (1 - cur, cur);
        let updates = diff(
            &self.bufs[prev],
            &self.links[prev],
            &self.bufs[next],
            &self.links[next],
            self.linked,
        );
        if let Some(p) = crate::perf::on() {
            let b = &self.bufs[next];
            p.drawn(updates.len(), || {
                b.content.iter().map(|c| c.symbol()).collect()
            });
        }
        write_updates(&mut self.out, &updates)?;
        match cursor {
            None => self.out.hide_cursor()?,
            Some((x, y)) => {
                self.out.show_cursor()?;
                self.out.set_cursor_position(Position::new(x, y))?;
            }
        }
        self.cur = prev;
        Backend::flush(&mut self.out)
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
        let out = &mut self.out;
        write!(out, "\x1b]2;{}\x07", self.title)?;
        Write::flush(out)
    }

    /// Puts text on the clipboard through the terminal (OSC 52), as Go
    /// rush's copy does (tea.SetClipboard).
    pub fn copy(&mut self, text: &str) -> io::Result<()> {
        let out = &mut self.out;
        write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
        Write::flush(out)
    }

    /// Forgets what's on screen, so the next frame is written whole.
    pub fn clear(&mut self) -> io::Result<()> {
        self.out.clear_region(ClearType::All)?;
        let prev = 1 - self.cur;
        self.bufs[prev].reset();
        self.links[prev].fill(Link::NONE);
        Ok(())
    }

    /// Gives the terminal back for a while (to run another program in
    /// it): the alternate screen, raw mode, the mouse, bracketed paste,
    /// focus reports and the kitty keyboard's mode left. Stop the input
    /// reader first ([`crate::Loop::run`] does), or it reads what's typed
    /// meanwhile.
    pub fn suspend(&mut self) -> io::Result<()> {
        Write::flush(&mut self.out)?;
        leave()
    }

    /// Takes the terminal over again after [`Term::suspend`]; the next
    /// frame is written whole.
    pub fn resume(&mut self) -> io::Result<()> {
        enter()?;
        if !self.title.is_empty() {
            write!(self.out, "\x1b]2;{}\x07", self.title)?;
        }
        self.clear()
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
        let _ = Write::flush(&mut self.out);
        let _ = leave();
    }
}

/// Whether links are written: not where Go's colour profile would be
/// NoTTY (colorprofile env.go:33 Detect, :72 colorProfile). That is: not
/// a terminal (unless TTY_FORCE), or TERM unset or dumb, unless
/// CLICOLOR_FORCE says otherwise and NO_COLOR doesn't.
pub(crate) fn links_ok(tty: bool, env: impl Fn(&str) -> Option<String>) -> bool {
    // Go's strconv.ParseBool, true side.
    let set = |k: &str| {
        env(k).is_some_and(|v| matches!(v.as_str(), "1" | "t" | "T" | "TRUE" | "true" | "True"))
    };
    let tty = tty || set("TTY_FORCE");
    let dumb = env("TERM").is_none_or(|t| t == "dumb");
    if tty && !dumb {
        return true;
    }
    !set("NO_COLOR") && set("CLICOLOR_FORCE")
}

/// The cells of next to write over prev: ratatui's Buffer::diff, with a
/// cell whose link alone changed written again too.
pub(crate) fn diff<'a>(
    prev: &Buffer,
    prev_links: &[Link],
    next: &'a Buffer,
    next_links: &[Link],
    linked: bool,
) -> Vec<Update<'a>> {
    let mut out = Vec::new();
    // Cells a wide char drawn or replaced before them covers.
    let mut invalidated = 0usize;
    let mut to_skip = 0usize;
    for (i, (c, p)) in next.content.iter().zip(&prev.content).enumerate() {
        let (l, pl) = if linked {
            (next_links[i], prev_links[i])
        } else {
            (Link::NONE, Link::NONE)
        };
        if !c.skip && (c != p || l != pl || invalidated > 0) && to_skip == 0 {
            let (x, y) = next.pos_of(i);
            out.push((x, y, c, l));
        }
        let (cw, pw) = (text::width(c.symbol()), text::width(p.symbol()));
        to_skip = cw.saturating_sub(1);
        invalidated = cw.max(pw).max(invalidated).saturating_sub(1);
    }
    out
}

/// Writes the cells, a run of one link at a time between its OSC 8s; a
/// link left open is closed after.
fn write_updates<W: Write>(out: &mut CrosstermBackend<W>, ups: &[Update]) -> io::Result<()> {
    let mut start = 0;
    while start < ups.len() {
        let l = ups[start].3;
        let end = start + ups[start..].iter().take_while(|u| u.3 == l).count();
        if !l.is_none() {
            let mut s = String::new();
            crate::headless::osc8(&mut s, l);
            out.write_all(s.as_bytes())?;
        }
        out.draw(ups[start..end].iter().map(|&(x, y, c, _)| (x, y, c)))?;
        if !l.is_none() {
            out.write_all(b"\x1b]8;;\x1b\\")?;
        }
        start = end;
    }
    Ok(())
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
    use super::*;

    // colorprofile env.go:72: NoTTY off a terminal, or with TERM unset or
    // dumb; CLICOLOR_FORCE brings it back unless NO_COLOR is set.
    #[test]
    fn links_where_go_has_a_profile() {
        type Case = (bool, &'static [(&'static str, &'static str)], bool);
        let cases: &[Case] = &[
            (true, &[("TERM", "xterm-256color")], true),
            (true, &[("TERM", "dumb")], false),
            (true, &[], false),
            (false, &[("TERM", "xterm-ghostty")], false),
            (false, &[("TERM", "xterm"), ("TTY_FORCE", "1")], true),
            (true, &[("TERM", "dumb"), ("CLICOLOR_FORCE", "true")], true),
            (
                false,
                &[
                    ("TERM", "xterm"),
                    ("CLICOLOR_FORCE", "1"),
                    ("NO_COLOR", "1"),
                ],
                false,
            ),
            (true, &[("TERM", "xterm"), ("NO_COLOR", "1")], true),
            (false, &[("CLICOLOR_FORCE", "yes")], false),
        ];
        for &(tty, env, want) in cases {
            let get = |k: &str| {
                env.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            };
            assert_eq!(links_ok(tty, get), want, "tty={tty} {env:?}");
        }
    }

    fn frame(w: u16, f: impl FnOnce(&mut Canvas)) -> (Buffer, Vec<Link>) {
        let mut b = Buffer::empty(RRect::new(0, 0, w, 1));
        let mut l = vec![Link::NONE; w as usize];
        f(&mut Canvas::new(&mut b, &mut l));
        (b, l)
    }

    // A cell whose link alone changed is written again, inside its OSC 8;
    // with links off it isn't.
    #[test]
    fn writes_link_changes() {
        use rush_core::text::Style;
        let (pb, pl) = frame(6, |c| {
            c.put_str(0, 0, "ab cd", Style::default(), 6);
        });
        let (nb, nl) = frame(6, |c| {
            c.put_str(0, 0, "ab ", Style::default(), 6);
            c.put_str(3, 0, "cd", Style::default().linked("rush:agent/a"), 6);
        });
        assert!(diff(&pb, &pl, &nb, &nl, false).is_empty());
        let ups = diff(&pb, &pl, &nb, &nl, true);
        assert_eq!(
            ups.iter().map(|u| (u.0, u.1)).collect::<Vec<_>>(),
            [(3, 0), (4, 0)]
        );
        struct Into(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
        impl Write for Into {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let got = std::rc::Rc::default();
        let mut out = CrosstermBackend::new(Into(std::rc::Rc::clone(&got)));
        write_updates(&mut out, &ups).unwrap();
        drop(out);
        let s = String::from_utf8(got.take()).unwrap();
        assert!(
            s.starts_with("\x1b]8;;rush:agent/a\x1b\\\x1b[1;4Hcd"),
            "{s:?}"
        );
        assert!(s.ends_with("\x1b]8;;\x1b\\"), "{s:?}");
    }

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
