//! A command of yours run in a terminal of its own (a pseudo-terminal),
//! for a shell box or a `!` command when Settings has them run so: sudo,
//! ssh and the rest that open /dev/tty find one, and what checks for a
//! terminal colours its output. `rush pty-run` is the small process that
//! does it (main): it opens the terminal, starts the login shell on it as
//! the leader of a session of its own (its stdin still /dev/null, so a
//! command reading stdin ends as before), and copies what it writes to its
//! own stdout (the run's output file), cursor moves and titles taken out
//! (Clean), colours kept. It runs apart from rush, as the plain runner
//! does, so rush quitting doesn't end the command.
//!
//! Nothing is typed into the terminal but what comes through the run's
//! input FIFO (send): rush writes there only what you type into the box's
//! password field. While the terminal reads a line without echoing it (a
//! password prompt: echo off, line mode on) a file beside the output says
//! so (asks_of), for the box to show the field. When the shell ends, what
//! it left is read for a moment more, its code is written to the exit
//! file (by way of a .part file), and the FIFO and the asks file go.
//!
//! TERM, INT, HUP and CONT sent to pty-run go on to the command's group;
//! a TERM it hasn't ended on in STOP_GRACE is followed by KILL. Rust-only.

use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// The terminal's size: about as wide as a box's inside.
pub const COLS: u16 = 100;
pub const ROWS: u16 = 30;

/// How long after its shell exits what it left is still read.
const DRAIN: Duration = Duration::from_millis(300);

/// How long after a TERM the command's group is made to end.
const STOP_GRACE: Duration = Duration::from_millis(2500);

/// How often the terminal's mode is looked at when nothing is written.
const TICK: Duration = Duration::from_millis(50);

/// What pty-run runs: the login shell, the script, and its files.
#[derive(Clone, Debug)]
pub struct Spec {
    pub shell: String,
    pub script: String,
    /// Where its code is written as it ends.
    pub exit: PathBuf,
    /// The FIFO what's typed for it comes through.
    pub input: PathBuf,
    /// There while it asks for a password.
    pub asks: PathBuf,
}

/// The FIFO a run whose output goes to log takes what's typed from.
pub fn input_of(log: &Path) -> PathBuf {
    log.with_extension("in")
}

/// The file that's there while the run whose output goes to log asks for
/// a password.
pub fn asks_of(log: &Path) -> PathBuf {
    log.with_extension("asks")
}

/// Whether a terminal in mode t is reading a password: a line at a time,
/// what's typed not shown. A full-screen program (no line mode) isn't.
pub fn asks(t: &libc::termios) -> bool {
    t.c_lflag & libc::ECHO == 0 && t.c_lflag & libc::ICANON != 0
}

/// Types b into the terminal of the run whose FIFO is input. Fails when
/// no pty-run reads it (the run has ended).
pub fn send(input: &Path, b: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
    if !std::fs::metadata(input)?.file_type().is_fifo() {
        return Err(std::io::Error::other("not a run's input"));
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(input)?;
    f.write_all(b)
}

static CHILD: AtomicI32 = AtomicI32::new(0);
static TERMED: AtomicBool = AtomicBool::new(false);

extern "C" fn forward(sig: libc::c_int) {
    if sig == libc::SIGTERM {
        TERMED.store(true, Ordering::SeqCst);
    }
    let p = CHILD.load(Ordering::SeqCst);
    if p > 0 {
        // SAFETY: kill is async-signal-safe.
        unsafe { libc::kill(-p, sig) };
    }
}

/// `rush pty-run <shell> <script> <exit file> <input fifo> <asks file>`:
/// runs the script as Spec says, its output to stdout; its exit code.
pub fn main(args: &[String]) -> i32 {
    let [shell, script, exit, input, asks] = args else {
        eprintln!("rush pty-run: wants <shell> <script> <exit> <input> <asks>");
        return 2;
    };
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGCONT] {
        // SAFETY: forward only does what's async-signal-safe.
        unsafe { libc::signal(sig, forward as *const () as libc::sighandler_t) };
    }
    let spec = Spec {
        shell: shell.clone(),
        script: script.clone(),
        exit: exit.into(),
        input: input.into(),
        asks: asks.into(),
    };
    let mut out = std::io::stdout().lock();
    match run(&spec, &mut out) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(out, "rush: couldn't run it in a terminal: {e}");
            let _ = write_exit(&spec.exit, 1);
            1
        }
    }
}

fn os_err<T>(r: libc::c_int, v: T) -> std::io::Result<T> {
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(v)
    }
}

/// A terminal: its master side and the side a program is given.
fn open_pty() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let (mut m, mut s): (RawFd, RawFd) = (-1, -1);
    let mut ws = libc::winsize {
        ws_row: ROWS,
        ws_col: COLS,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: out-pointers to locals; no name asked for.
    let r = unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            std::ptr::null_mut::<libc::c_char>(),
            std::ptr::null_mut::<libc::termios>(),
            &mut ws,
        )
    };
    os_err(r, ())?;
    // SAFETY: openpty gave both, ours alone.
    let (m, s) = unsafe { (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s)) };
    for fd in [m.as_raw_fd(), s.as_raw_fd()] {
        // SAFETY: an fd we hold.
        os_err(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            (),
        )?;
    }
    // A new line written stays one, as it is through a pipe.
    // SAFETY: a zeroed termios is filled by tcgetattr.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: s is a terminal we hold; t is ours.
    if unsafe { libc::tcgetattr(s.as_raw_fd(), &mut t) } == 0 {
        t.c_oflag &= !libc::ONLCR;
        // SAFETY: as above.
        unsafe { libc::tcsetattr(s.as_raw_fd(), libc::TCSANOW, &t) };
    }
    Ok((m, s))
}

/// The FIFO at p, made afresh and opened to read without waiting (and to
/// write, so it never reads as ended between writers).
fn open_fifo(p: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(p);
    let c = CString::new(p.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let mode: libc::mode_t = 0o600;
    // SAFETY: a valid C string.
    os_err(unsafe { libc::mkfifo(c.as_ptr(), mode) }, ())?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(p)
}

/// Writes code to exit, by way of a .part file.
fn write_exit(exit: &Path, code: i32) -> std::io::Result<()> {
    let part = exit.with_extension("exit.part");
    std::fs::write(&part, format!("{code}\n"))?;
    std::fs::rename(part, exit)
}

/// Which of fds can be read now, waiting up to wait.
fn readable(fds: &[RawFd], wait: Duration) -> Vec<bool> {
    // SAFETY: an fd_set is plain data, zeroed then filled by FD_SET.
    let mut set: libc::fd_set = unsafe { std::mem::zeroed() };
    let mut most = 0;
    for &fd in fds {
        // SAFETY: fd is open and below FD_SETSIZE (a fresh process's).
        unsafe { libc::FD_SET(fd, &mut set) };
        most = most.max(fd);
    }
    let mut tv = libc::timeval {
        tv_sec: wait.as_secs() as _,
        tv_usec: wait.subsec_micros() as _,
    };
    // SAFETY: set and tv are ours.
    let n = unsafe {
        libc::select(
            most + 1,
            &mut set,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut tv,
        )
    };
    if n <= 0 {
        return vec![false; fds.len()];
    }
    fds.iter()
        // SAFETY: set as select left it.
        .map(|&fd| unsafe { libc::FD_ISSET(fd, &set) })
        .collect()
}

/// Runs spec's script in a terminal of its own, what it writes (Clean)
/// to out, and what's written to its FIFO typed into it; writes its code
/// to the exit file as it ends and returns it (128 + the signal when one
/// ended it).
pub fn run(spec: &Spec, out: &mut dyn Write) -> std::io::Result<i32> {
    let (master, slave) = open_pty()?;
    let fifo = open_fifo(&spec.input)?;
    let _ = std::fs::remove_file(&spec.asks);
    let err = slave.try_clone()?;
    let mut cmd = Command::new(&spec.shell);
    cmd.arg("-lc")
        .arg(&spec.script)
        .stdin(Stdio::null())
        .stdout(Stdio::from(slave))
        .stderr(Stdio::from(err))
        .env("TERM", "xterm-256color")
        // A pager would wait on keys no one types.
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat");
    // SAFETY: setsid and ioctl are async-signal-safe, all that runs
    // between fork and exec here; stdout is the terminal by then.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(1, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    if TERMED.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&spec.input);
        write_exit(&spec.exit, 128 + libc::SIGTERM)?;
        return Ok(128 + libc::SIGTERM);
    }
    let spawned = cmd.spawn();
    drop(cmd); // its copies of the terminal: only the command's are left
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&spec.input);
            return Err(e);
        }
    };
    let pid = child.id() as i32;
    CHILD.store(pid, Ordering::SeqCst);
    let mut m = File::from(master);
    let mut fifo = fifo;
    let (mfd, ffd) = (m.as_raw_fd(), fifo.as_raw_fd());
    let mut clean = Clean::default();
    let mut buf = [0u8; 16 * 1024];
    let mut cleaned = Vec::with_capacity(buf.len());
    let mut status = None;
    let mut ended_at: Option<Instant> = None;
    let mut termed_at: Option<Instant> = None;
    let mut asking = false;
    let mut open = true;
    loop {
        if open {
            let r = readable(&[mfd, ffd], TICK);
            if r[0] {
                match m.read(&mut buf) {
                    Ok(0) => open = false,
                    Ok(n) => {
                        cleaned.clear();
                        clean.feed(&buf[..n], &mut cleaned);
                        out.write_all(&cleaned)?;
                        out.flush()?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    // EIO: no one has the terminal open any more.
                    Err(_) => open = false,
                }
            }
            if r[1] {
                // What's typed is the password: kept no longer than it's
                // passed on.
                if let Ok(n) = fifo.read(&mut buf) {
                    let _ = m.write_all(&buf[..n]);
                    buf[..n].fill(0);
                }
            }
            // SAFETY: a zeroed termios is filled by tcgetattr.
            let mut t: libc::termios = unsafe { std::mem::zeroed() };
            // SAFETY: mfd is the terminal's master, which we hold.
            let got = unsafe { libc::tcgetattr(mfd, &mut t) } == 0;
            let now = got && asks(&t) && status.is_none();
            if now != asking {
                asking = now;
                if now {
                    let _ = File::create(&spec.asks);
                } else {
                    let _ = std::fs::remove_file(&spec.asks);
                }
            }
        }
        if status.is_none() {
            if let Some(s) = child.try_wait()? {
                status = Some(s);
                ended_at = Some(Instant::now());
            } else if !open {
                // The terminal is gone but the shell isn't: wait for it.
                status = Some(child.wait()?);
                ended_at = Some(Instant::now());
            }
        }
        if TERMED.load(Ordering::SeqCst) && status.is_none() {
            let at = *termed_at.get_or_insert_with(Instant::now);
            if at.elapsed() >= STOP_GRACE {
                // SAFETY: the command's own group.
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
        }
        if let Some(at) = ended_at
            && (!open || at.elapsed() >= DRAIN)
        {
            break;
        }
    }
    CHILD.store(0, Ordering::SeqCst);
    drop(fifo);
    let _ = std::fs::remove_file(&spec.input);
    let _ = std::fs::remove_file(&spec.asks);
    let s = status.expect("the loop ends once the shell has");
    let code = s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0));
    write_exit(&spec.exit, code)?;
    Ok(code)
}

/// What a terminal program writes, made fit for a box: colours (SGR)
/// kept, other control sequences (cursor moves, clearing, titles, modes)
/// and bells taken out, a sequence cut between two writes read whole.
/// Lines and carriage returns stay: the box reads a line redrawn with \r
/// as its last state.
#[derive(Debug, Default)]
pub struct Clean {
    st: St,
    seq: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum St {
    #[default]
    Text,
    Esc,
    Csi,
    /// OSC, DCS and the like, up to BEL or ESC \.
    Str,
    StrEsc,
    /// ESC ( and its kin: one byte more.
    Charset,
}

/// The longest control sequence kept whole; a longer one is let go of.
const SEQ_MOST: usize = 64;

impl Clean {
    /// Adds what's left of b once cleaned to out.
    pub fn feed(&mut self, b: &[u8], out: &mut Vec<u8>) {
        for &c in b {
            match self.st {
                St::Text => match c {
                    0x1b => {
                        self.st = St::Esc;
                        self.seq.clear();
                        self.seq.push(c);
                    }
                    b'\n' | b'\r' | b'\t' => out.push(c),
                    0..=0x1f | 0x7f => {}
                    _ => out.push(c),
                },
                St::Esc => {
                    self.st = match c {
                        b'[' => {
                            self.seq.push(c);
                            St::Csi
                        }
                        b']' | b'P' | b'_' | b'^' | b'X' => St::Str,
                        b'(' | b')' | b'*' | b'+' => St::Charset,
                        0x1b => St::Esc,
                        _ => St::Text,
                    }
                }
                St::Csi => {
                    self.seq.push(c);
                    if (0x40..=0x7e).contains(&c) {
                        let body = &self.seq[2..self.seq.len() - 1];
                        if c == b'm'
                            && body
                                .iter()
                                .all(|b| b.is_ascii_digit() || *b == b';' || *b == b':')
                        {
                            out.extend_from_slice(&self.seq);
                        }
                        self.st = St::Text;
                    } else if self.seq.len() > SEQ_MOST {
                        self.st = St::Text;
                    }
                }
                St::Str => match c {
                    0x07 => self.st = St::Text,
                    0x1b => self.st = St::StrEsc,
                    _ => {}
                },
                St::StrEsc => {
                    self.st = if c == b'\\' { St::Text } else { St::Str };
                }
                St::Charset => self.st = St::Text,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Colours stay; cursor moves, clearing, titles, modes, charsets and
    /// bells go, however the writes cut them.
    #[test]
    fn clean_keeps_colours_alone() {
        let cases: &[(&str, &[u8], &[u8])] = &[
            ("plain", b"hello\r\nworld\n", b"hello\r\nworld\n"),
            ("colour", b"\x1b[1;31mred\x1b[0m", b"\x1b[1;31mred\x1b[0m"),
            ("reset", b"\x1b[m", b"\x1b[m"),
            ("clear line", b"\x1b[2Kdone\r", b"done\r"),
            ("cursor", b"a\x1b[3Ab\x1b[10;4Hc", b"abc"),
            ("private mode", b"\x1b[?25lx\x1b[?25h", b"x"),
            ("title by bel", b"\x1b]0;title\x07after", b"after"),
            ("title by st", b"\x1b]2;t\x1b\\after", b"after"),
            ("charset", b"\x1b(Bok", b"ok"),
            ("keypad", b"\x1b=ok\x1b>", b"ok"),
            ("bell and backspace", b"a\x07b\x08c", b"abc"),
            ("utf-8", "café ✓".as_bytes(), "café ✓".as_bytes()),
            ("tab", b"a\tb", b"a\tb"),
        ];
        for (name, input, want) in cases {
            let mut out = Vec::new();
            Clean::default().feed(input, &mut out);
            assert_eq!(
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(want),
                "{name}"
            );
            // Fed a byte at a time, the same.
            let mut c = Clean::default();
            let mut out = Vec::new();
            for b in input.iter() {
                c.feed(std::slice::from_ref(b), &mut out);
            }
            assert_eq!(
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(want),
                "{name}, a byte at a time"
            );
        }
    }

    /// Echo off in line mode is a password asked for; echo on, or a
    /// full-screen program's raw mode, isn't.
    #[test]
    fn asks_is_echo_off_in_line_mode() {
        // SAFETY: plain data.
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        let cases = [
            (libc::ECHO | libc::ICANON, false),
            (libc::ICANON, true),
            (0, false),
            (libc::ECHO, false),
        ];
        for (lflag, want) in cases {
            t.c_lflag = lflag;
            assert_eq!(asks(&t), want, "lflag {lflag:#x}");
        }
    }

    /// Waits, without sleeping, for f to hold; fails after a while.
    fn until(what: &str, f: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !f() {
            assert!(Instant::now() < end, "never: {what}");
            std::thread::yield_now();
        }
    }

    fn spec(dir: &Path, script: &str) -> (Spec, PathBuf) {
        let log = dir.join("1-t1_bang_1.log");
        (
            Spec {
                shell: "/bin/sh".into(),
                script: script.into(),
                exit: log.with_extension("exit"),
                input: input_of(&log),
                asks: asks_of(&log),
            },
            log,
        )
    }

    /// A command that reads its password from /dev/tty, as sudo does,
    /// with echo off: the asks file says so while it waits, what's sent
    /// to the FIFO is what it reads, never shown, and it ends with its
    /// code written and its files gone.
    #[test]
    fn a_password_read_from_the_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let (sp, log) = spec(
            dir.path(),
            r#"test -t 1 && echo "a terminal"
test -t 0 || echo "stdin closed"
printf 'Password:' > /dev/tty
stty -echo < /dev/tty
read -r pw < /dev/tty
stty echo < /dev/tty
echo
echo "got $pw"
exit 4"#,
        );
        let (sp2, log2) = (sp.clone(), log.clone());
        let t = std::thread::spawn(move || {
            let mut out = File::create(&log2).unwrap();
            run(&sp2, &mut out)
        });
        until("it asks", || sp.asks.exists());
        send(&sp.input, b"hunter2\n").unwrap();
        let code = t.join().unwrap().unwrap();
        assert_eq!(code, 4);
        let out = std::fs::read_to_string(&log).unwrap();
        assert!(out.contains("a terminal\n"), "{out:?}");
        assert!(out.contains("stdin closed\n"), "{out:?}");
        assert!(out.contains("Password:"), "{out:?}");
        assert!(out.contains("got hunter2\n"), "{out:?}");
        assert_eq!(out.matches("hunter2").count(), 1, "echoed: {out:?}");
        assert_eq!(std::fs::read_to_string(&sp.exit).unwrap().trim(), "4");
        assert!(!sp.asks.exists() && !sp.input.exists());
        // Its input is gone with it.
        assert!(send(&sp.input, b"x").is_err());
    }

    /// A shell ended by a signal says so as 128 + its number, as a shell
    /// would; one that reads nothing never asks.
    #[test]
    fn its_code_and_a_signal() {
        let dir = tempfile::tempdir().unwrap();
        for (script, want) in [
            ("echo hi; exit 0", 0),
            ("kill -TERM $$", 128 + libc::SIGTERM),
        ] {
            let (sp, log) = spec(dir.path(), script);
            let mut out = File::create(&log).unwrap();
            assert_eq!(run(&sp, &mut out).unwrap(), want, "{script}");
            assert_eq!(
                std::fs::read_to_string(&sp.exit).unwrap().trim(),
                want.to_string()
            );
            assert!(!sp.asks.exists());
        }
        let (sp, log) = spec(dir.path(), "printf 'one\\ntwo\\n'");
        let mut out = File::create(&log).unwrap();
        run(&sp, &mut out).unwrap();
        // New lines stay new lines, as through a pipe.
        let out = std::fs::read_to_string(&log).unwrap();
        assert!(
            out.ends_with("one\ntwo\n") && !out.contains('\r'),
            "{out:?}"
        );
    }
}
