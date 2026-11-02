//! One thread reads every JSON-RPC connection in the process (Rust-only:
//! Go reads each connection in a goroutine, which costs far less than a
//! thread). It waits in one kqueue on every connection's descriptor, reads
//! what's ready without blocking, cuts it into messages and hands each to
//! its connection (`Conn::deliver`, rpc.rs). No handler runs on it: a
//! slow one would hold up every other connection.
//! The framing is internal/plugin/rpc.go's (frames, lines), as rpc.rs's
//! blocking readers do it.

use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::rpc::{Conn, Item, MAX_FRAME};

/// How a connection's bytes are cut into messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Framing {
    /// A 4-byte big-endian length, then that many bytes of JSON.
    Frames,
    /// One message a line: MCP's stdio transport.
    Lines,
}

/// What one read asks for at least: the 64 KB the blocking readers buffer.
const CHUNK: usize = 64 << 10;

/// Cuts what's read into messages, keeping a part message until the rest
/// comes.
pub(crate) struct Framer {
    framing: Framing,
    buf: Vec<u8>,
    /// Where the next message starts in buf.
    start: usize,
    /// Lines: buf has no newline before this.
    scanned: usize,
}

impl Framer {
    pub(crate) fn new(framing: Framing) -> Framer {
        Framer {
            framing,
            buf: Vec::new(),
            start: 0,
            scanned: 0,
        }
    }

    /// Moves a part message to the front, and lets a big buffer go once
    /// it's empty: an idle connection keeps at most CHUNK.
    fn compact(&mut self) {
        if self.start > 0 {
            self.buf.drain(..self.start);
            self.scanned -= self.start;
            self.start = 0;
        }
        if self.buf.is_empty() && self.buf.capacity() > CHUNK {
            self.buf = Vec::new();
        }
    }

    /// How much a read should ask for: CHUNK, or the rest of a frame
    /// bigger than that, read whole as the blocking reader reads a body.
    fn want(&self) -> usize {
        let have = &self.buf[self.start..];
        if self.framing == Framing::Frames && have.len() >= 4 {
            let n = u32::from_be_bytes([have[0], have[1], have[2], have[3]]) as usize;
            if n <= MAX_FRAME {
                return (4 + n).saturating_sub(have.len()).max(CHUNK);
            }
        }
        CHUNK
    }

    /// Reads what fd has without waiting: recv(MSG_DONTWAIT) on a socket
    /// (whose descriptor its writer shares, so it stays blocking), read on
    /// a pipe set non-blocking. Ok(0) at its end.
    fn read_from(&mut self, fd: RawFd, sock: bool) -> io::Result<usize> {
        self.compact();
        let want = self.want();
        self.buf.reserve(want);
        let spare = self.buf.spare_capacity_mut();
        let n = spare.len().min(want);
        let p = spare.as_mut_ptr().cast::<libc::c_void>();
        // SAFETY: p has room for n bytes, buf's spare capacity.
        let r = unsafe {
            if sock {
                libc::recv(fd, p, n, libc::MSG_DONTWAIT)
            } else {
                libc::read(fd, p, n)
            }
        };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        let len = self.buf.len() + r as usize;
        // SAFETY: the kernel wrote r bytes after len.
        unsafe { self.buf.set_len(len) };
        Ok(r as usize)
    }

    #[cfg(test)]
    fn feed(&mut self, b: &[u8]) {
        self.compact();
        self.buf.extend_from_slice(b);
    }

    /// Where the next whole message is in what's read, None until one is
    /// whole; Err (the words rpc.rs's readers use) once the connection
    /// must end.
    fn next(&mut self) -> Result<Option<Range<usize>>, String> {
        match self.framing {
            Framing::Frames => {
                let have = &self.buf[self.start..];
                if have.len() < 4 {
                    return Ok(None);
                }
                let n = u32::from_be_bytes([have[0], have[1], have[2], have[3]]) as usize;
                if n > MAX_FRAME {
                    return Err(format!("frame of {n} bytes is over the {MAX_FRAME} limit"));
                }
                if have.len() < 4 + n {
                    return Ok(None);
                }
                let at = self.start + 4;
                self.start = at + n;
                Ok(Some(at..at + n))
            }
            Framing::Lines => loop {
                let from = self.scanned.max(self.start);
                match self.buf[from..].iter().position(|&c| c == b'\n') {
                    Some(i) => {
                        let end = from + i + 1;
                        if end - self.start > MAX_FRAME {
                            return Err(format!("line over the {MAX_FRAME} byte limit"));
                        }
                        let line = self.start..end;
                        self.start = end;
                        self.scanned = end;
                        if self.buf[line.clone()]
                            .iter()
                            .any(|c| !c.is_ascii_whitespace())
                        {
                            return Ok(Some(line));
                        }
                    }
                    None => {
                        self.scanned = self.buf.len();
                        if self.buf.len() - self.start > MAX_FRAME {
                            return Err(format!("line over the {MAX_FRAME} byte limit"));
                        }
                        return Ok(None);
                    }
                }
            },
        }
    }

    /// Why the connection ended when its other side went: io.ReadFull's
    /// "unexpected EOF" for a frame cut short, as the blocking reader says.
    fn eof(&self) -> String {
        if self.framing == Framing::Frames && self.start < self.buf.len() {
            "unexpected EOF".into()
        } else {
            "EOF".into()
        }
    }
}

/// Why a connection is ended from its side.
#[derive(Clone, Copy)]
pub(crate) enum Why {
    /// Its socket was shut down: what a read would then say.
    Eof,
    /// Its pipe was closed: sys::WakeableReader's words.
    Closed,
}

struct Entry {
    token: u64,
    fd: OwnedFd,
    sock: bool,
    conn: Conn,
    state: Mutex<State>,
}

struct State {
    framer: Framer,
    /// No more is read: the connection is told of its end once.
    ended: bool,
}

/// The process's one reader.
pub(crate) struct Reader {
    kq: OwnedFd,
    conns: Mutex<HashMap<u64, Arc<Entry>>>,
    next: AtomicU64,
    /// Its wait failed: connections made from now read on threads.
    broken: AtomicBool,
}

static READER: OnceLock<Option<Reader>> = OnceLock::new();
static STARTED: OnceLock<bool> = OnceLock::new();

impl Reader {
    /// The process's reader, started the first time it's asked for; None
    /// when it can't be (no kqueue, no thread), or its wait has failed.
    pub(crate) fn get() -> Option<&'static Reader> {
        let r = READER.get_or_init(Reader::make).as_ref()?;
        let started = *STARTED.get_or_init(|| {
            rush_sys::threads::spawn_small("plugin-rpc", || {
                if let Some(Some(r)) = READER.get() {
                    r.run();
                }
            })
            .is_ok()
        });
        (started && !r.broken.load(Ordering::SeqCst)).then_some(r)
    }

    fn make() -> Option<Reader> {
        // SAFETY: kqueue takes nothing; a non-negative result is ours.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return None;
        }
        // SAFETY: kq is a descriptor we just made, ours to own.
        let kq = unsafe {
            libc::fcntl(kq, libc::F_SETFD, libc::FD_CLOEXEC);
            OwnedFd::from_raw_fd(kq)
        };
        Some(Reader {
            kq,
            conns: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
            broken: AtomicBool::new(false),
        })
    }

    /// A name for a connection about to be added, for its closer to end it
    /// by.
    pub(crate) fn token(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Reads fd for conn from now on, cut by framing. Gives fd back when
    /// it can't be watched.
    pub(crate) fn add(
        &self,
        token: u64,
        fd: OwnedFd,
        conn: Conn,
        framing: Framing,
    ) -> Result<(), OwnedFd> {
        let raw = fd.as_raw_fd();
        let sock = is_socket(raw);
        if !sock {
            // SAFETY: plain fcntl on a descriptor we own; only we read it.
            unsafe {
                let fl = libc::fcntl(raw, libc::F_GETFL);
                libc::fcntl(raw, libc::F_SETFL, fl | libc::O_NONBLOCK);
            }
        }
        let e = Arc::new(Entry {
            token,
            fd,
            sock,
            conn,
            state: Mutex::new(State {
                framer: Framer::new(framing),
                ended: false,
            }),
        });
        lock(&self.conns).insert(token, e);
        if self.ctl(raw, libc::EV_ADD, token).is_ok() {
            return Ok(());
        }
        // Never watched, so nothing else holds it.
        let e = lock(&self.conns).remove(&token).expect("just added");
        match Arc::try_unwrap(e) {
            Ok(e) => Err(e.fd),
            Err(_) => unreachable!("an entry never watched is held only here"),
        }
    }

    fn ctl(&self, fd: RawFd, flags: u16, token: u64) -> io::Result<()> {
        // SAFETY: kevent is plain data.
        let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
        ev.ident = fd as usize;
        ev.filter = libc::EVFILT_READ;
        ev.flags = flags;
        ev.udata = std::ptr::without_provenance_mut(token as usize);
        // SAFETY: one change from a valid kevent; no events asked for.
        let r = unsafe {
            libc::kevent(
                self.kq.as_raw_fd(),
                &ev,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn run(&self) {
        // SAFETY: kevent is plain data.
        let mut events = vec![unsafe { std::mem::zeroed::<libc::kevent>() }; 64];
        loop {
            // SAFETY: events is valid for its length; no timeout.
            let n = unsafe {
                libc::kevent(
                    self.kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as i32,
                    std::ptr::null(),
                )
            };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                self.fail(&e.to_string());
                return;
            }
            for ev in &events[..n as usize] {
                let token = ev.udata.addr() as u64;
                let Some(e) = lock(&self.conns).get(&token).cloned() else {
                    continue; // ended since
                };
                if ev.flags & libc::EV_ERROR != 0 {
                    let err = io::Error::from_raw_os_error(ev.data as i32).to_string();
                    self.end_locked(&e, &mut lock(&e.state), err);
                    continue;
                }
                self.pump(&e);
            }
        }
    }

    /// Reads what e has and hands on every whole message, in order.
    fn pump(&self, e: &Entry) {
        let mut st = lock(&e.state);
        if st.ended {
            return;
        }
        match st.framer.read_from(e.fd.as_raw_fd(), e.sock) {
            Ok(0) => {
                let err = st.framer.eof();
                self.end_locked(e, &mut st, err);
            }
            Ok(_) => loop {
                match st.framer.next() {
                    Ok(Some(r)) => e.conn.deliver(crate::rpc::item_of(&st.framer.buf[r])),
                    Ok(None) => break,
                    Err(err) => {
                        self.end_locked(e, &mut st, err);
                        break;
                    }
                }
            },
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(err) => self.end_locked(e, &mut st, err.to_string()),
        }
    }

    /// Stops reading e and tells its connection why, after all it was
    /// handed before. Called with e's state held, so nothing it reads
    /// comes after.
    fn end_locked(&self, e: &Entry, st: &mut State, err: String) {
        if st.ended {
            return;
        }
        st.ended = true;
        let _ = self.ctl(e.fd.as_raw_fd(), libc::EV_DELETE, e.token);
        lock(&self.conns).remove(&e.token);
        e.conn.deliver(Item::Finish(err));
    }

    /// The wait itself failed: every connection ends with its error.
    fn fail(&self, err: &str) {
        self.broken.store(true, Ordering::SeqCst);
        let all: Vec<_> = lock(&self.conns).values().cloned().collect();
        for e in all {
            self.end_locked(&e, &mut lock(&e.state), err.to_string());
        }
    }
}

/// Ends the connection added as token, from its own side (its closer):
/// nothing more is read, and it's told why. Nothing when it has ended.
pub(crate) fn end(token: u64, why: Why) {
    let Some(Some(r)) = READER.get() else {
        return;
    };
    let Some(e) = lock(&r.conns).get(&token).cloned() else {
        return;
    };
    let mut st = lock(&e.state);
    let err = match why {
        Why::Eof => st.framer.eof(),
        Why::Closed => "file already closed".into(),
    };
    r.end_locked(&e, &mut st, err);
}

fn is_socket(fd: RawFd) -> bool {
    // SAFETY: zeroed is a valid stat for fstat to fill.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat on a descriptor we hold, into st.
    let r = unsafe { libc::fstat(fd, &mut st) };
    r == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFSOCK
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(b: &[u8]) -> Vec<u8> {
        let mut v = (b.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(b);
        v
    }

    /// Feeds each chunk in turn, taking every whole message after each:
    /// what's cut out, and the error that ended it, or how its end reads.
    fn cut(framing: Framing, chunks: &[Vec<u8>]) -> (Vec<String>, String) {
        let mut f = Framer::new(framing);
        let mut got = Vec::new();
        for c in chunks {
            f.feed(c);
            loop {
                match f.next() {
                    Ok(Some(r)) => got.push(String::from_utf8_lossy(&f.buf[r]).into_owned()),
                    Ok(None) => break,
                    Err(e) => return (got, e),
                }
            }
        }
        (got, f.eof())
    }

    #[test]
    fn cuts_as_the_blocking_readers_do() {
        let big = vec![b'x'; MAX_FRAME];
        let over = (MAX_FRAME as u32 + 1).to_be_bytes().to_vec();
        let mut whole_big = big.clone();
        whole_big.push(b'\n');
        struct Case(
            &'static str,
            Framing,
            Vec<Vec<u8>>,
            Vec<&'static str>,
            String,
        );
        let cases = vec![
            Case("nothing", Framing::Frames, vec![], vec![], "EOF".into()),
            Case(
                "two in one read",
                Framing::Frames,
                vec![[frame(b"{}"), frame(b"[1]")].concat()],
                vec!["{}", "[1]"],
                "EOF".into(),
            ),
            Case(
                "split in the length and the body",
                Framing::Frames,
                vec![vec![0, 0], vec![0, 3, b'a'], vec![b'b', b'c']],
                vec!["abc"],
                "EOF".into(),
            ),
            Case(
                "empty frame",
                Framing::Frames,
                vec![frame(b"")],
                vec![""],
                "EOF".into(),
            ),
            Case(
                "cut short",
                Framing::Frames,
                vec![vec![0, 0, 0, 9, b'a']],
                vec![],
                "unexpected EOF".into(),
            ),
            Case(
                "half a length",
                Framing::Frames,
                vec![vec![0, 0]],
                vec![],
                "unexpected EOF".into(),
            ),
            Case(
                "over the limit",
                Framing::Frames,
                vec![over],
                vec![],
                format!(
                    "frame of {} bytes is over the {MAX_FRAME} limit",
                    MAX_FRAME + 1
                ),
            ),
            Case(
                "lines, blank ones skipped",
                Framing::Lines,
                vec![b"{\"a\":1}\n\n  \r\n[2]\n".to_vec()],
                vec!["{\"a\":1}\n", "[2]\n"],
                "EOF".into(),
            ),
            Case(
                "a line across reads",
                Framing::Lines,
                vec![b"{\"a\"".to_vec(), b":1}".to_vec(), b"\n{".to_vec()],
                vec!["{\"a\":1}\n"],
                "EOF".into(),
            ),
            Case(
                "a line at the limit, its newline over it",
                Framing::Lines,
                vec![whole_big],
                vec![],
                format!("line over the {MAX_FRAME} byte limit"),
            ),
            Case(
                "a part line over the limit",
                Framing::Lines,
                vec![big, b"x".to_vec()],
                vec![],
                format!("line over the {MAX_FRAME} byte limit"),
            ),
        ];
        for Case(name, framing, chunks, want, end) in cases {
            let (got, e) = cut(framing, &chunks);
            assert_eq!(got, want, "{name}");
            assert_eq!(e, end, "{name}");
        }
    }
}
