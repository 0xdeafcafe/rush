//! The Claude Code daemon's control socket (proto 1): one JSON request
//! line, one JSON response line. After a successful attach the connection
//! becomes a raw terminal stream in both directions. Port of
//! internal/adapters/claude/daemon: daemon.go (Client) and attach.go
//! (Session, Relay, the markers).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::account::Account;
use crate::json::nd;

const PROTO: i64 = 1;

/// One account's daemon.
#[derive(Clone, Debug)]
pub struct Client {
    pub account: Account,
}

/// A refusal from the daemon, e.g. ENOJOB or EKICKED.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

/// Whether err is, or was caused by, the daemon refusing with code.
pub fn is_refusal(err: &(dyn std::error::Error + 'static), code: &str) -> bool {
    let mut e = Some(err);
    while let Some(x) = e {
        if x.downcast_ref::<Error>().is_some_and(|r| r.code == code) {
            return true;
        }
        e = x.source();
    }
    false
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Response {
    #[serde(deserialize_with = "nd")]
    ok: bool,
    #[serde(deserialize_with = "nd")]
    code: String,
    #[serde(deserialize_with = "nd")]
    error: String,
    #[serde(deserialize_with = "nd")]
    message: String,
}

/// What the daemon says of a session as it attaches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AttachInfo {
    #[serde(rename = "decModes", deserialize_with = "nd")]
    pub dec_modes: Vec<i32>,
    #[serde(deserialize_with = "nd")]
    pub via: String,
    #[serde(deserialize_with = "nd")]
    pub state: String,
    #[serde(deserialize_with = "nd")]
    pub stale: bool,
}

/// An attached session's stream: write keys to conn, read its screen from
/// r (which may hold what came with the answer).
pub struct Attached {
    pub conn: UnixStream,
    pub r: BufReader<UnixStream>,
    pub info: AttachInfo,
}

impl Client {
    pub fn new(account: Account) -> Client {
        Client { account }
    }

    /// /tmp/cc-daemon-<uid>/<sha256(configDir)[:8]>.
    pub fn sock_dir(&self) -> PathBuf {
        use sha2::Digest;
        let sum = sha2::Sha256::digest(self.account.config_dir.as_os_str().as_encoded_bytes());
        let hex: String = sum[..4].iter().map(|b| format!("{b:02x}")).collect();
        // SAFETY: getuid can't fail.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/cc-daemon-{uid}")).join(hex)
    }

    fn control_sock(&self) -> PathBuf {
        self.sock_dir().join("control.sock")
    }

    /// Whether this account's daemon is up.
    pub fn running(&self) -> bool {
        self.control_sock().exists()
    }

    fn key(&self) -> String {
        std::fs::read_to_string(self.account.config_dir.join("daemon").join("control.key"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    fn dial(
        &self,
        mut req: Map<String, Value>,
    ) -> Result<(UnixStream, BufReader<UnixStream>, String), rush_core::adapter::Error> {
        let conn = UnixStream::connect(self.control_sock())?;
        req.insert("proto".into(), PROTO.into());
        let k = self.key();
        if !k.is_empty() {
            req.insert("auth".into(), k.into());
        }
        let mut b = serde_json::to_vec(&Value::Object(req))?;
        b.push(b'\n');
        let limit = Some(Duration::from_secs(10));
        conn.set_read_timeout(limit)?;
        conn.set_write_timeout(limit)?;
        (&conn).write_all(&b)?;
        let mut r = BufReader::with_capacity(64 << 10, conn.try_clone()?);
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        let res: Response = serde_json::from_str(line.trim_end())?;
        if !res.ok {
            let message = if res.message.is_empty() {
                res.error
            } else {
                res.message
            };
            let code = if res.code.is_empty() {
                "EFAIL".to_string()
            } else {
                res.code
            };
            return Err(Box::new(Error { code, message }));
        }
        let _ = conn.set_read_timeout(None); // fails once the daemon has hung up
        let _ = conn.set_write_timeout(None);
        Ok((conn, r, line))
    }

    fn call(&self, req: Value) -> Result<(), rush_core::adapter::Error> {
        let Value::Object(m) = req else {
            unreachable!("requests are objects")
        };
        self.dial(m).map(drop)
    }

    /// Types text into a session as if the user sent it.
    pub fn reply(&self, short: &str, text: &str) -> Result<(), rush_core::adapter::Error> {
        self.call(json!({"op": "reply", "short": short, "text": text}))
    }

    /// Asks the daemon to stop a session; the conversation is kept.
    pub fn kill(&self, short: &str) -> Result<(), rush_core::adapter::Error> {
        self.call(json!({"op": "kill", "short": short}))
    }

    pub fn resize(
        &self,
        short: &str,
        attach_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), rush_core::adapter::Error> {
        self.call(json!({"op": "resize", "short": short, "attachId": attach_id, "cols": cols, "rows": rows}))
    }

    /// Opens the live terminal stream of a session.
    pub fn attach(
        &self,
        short: &str,
        attach_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Attached, rush_core::adapter::Error> {
        let Value::Object(m) = json!({
            "op": "attach", "short": short, "attachId": attach_id, "cols": cols, "rows": rows,
        }) else {
            unreachable!()
        };
        let (conn, r, line) = self.dial(m)?;
        let info = serde_json::from_str(line.trim_end()).unwrap_or_default();
        Ok(Attached { conn, r, info })
    }
}

const DETACH_MARKER: &[u8] = b"\x1b_cc-daemon-detach\x1b\\";
const HINT_MARKER: &[u8] = b"\x1b_cc-daemon-hint\x1b\\";
const KICKED: &[u8] = b"EKICKED: Session opened in another window";
const ESCAPE_KEY: u8 = 0x1d; // ctrl+]

/// How pump_out stopped.
#[derive(Debug)]
enum Stop {
    /// The session asked to detach.
    Detach,
    /// It ended, or was opened elsewhere.
    Eof,
    Err(std::io::Error),
}

/// Copies a session's terminal stream to w with the daemon's control
/// markers taken out, until it detaches or ends.
pub fn relay(r: &mut dyn Read, w: &mut dyn Write) -> std::io::Result<()> {
    match pump_out(r, w) {
        Stop::Detach | Stop::Eof => Ok(()),
        Stop::Err(e) if closed(&e) => Ok(()),
        Stop::Err(e) => Err(e),
    }
}

fn closed(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
    )
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(hay, needle)
}

fn pump_out(r: &mut dyn Read, w: &mut dyn Write) -> Stop {
    let mut buf = vec![0u8; 32 << 10];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => return Stop::Eof,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Stop::Err(e),
        };
        let mut chunk = std::mem::take(&mut carry);
        chunk.extend_from_slice(&buf[..n]);
        if let Some(i) = find(&chunk, DETACH_MARKER) {
            let _ = w.write_all(&strip_hints(&chunk[..i]));
            let _ = w.flush();
            return Stop::Detach;
        }
        if find(&chunk, KICKED).is_some() {
            return Stop::Eof;
        }
        // Hold back only a tail that could still grow into a marker.
        let k = partial_marker(&chunk);
        if k > 0 {
            carry = chunk[chunk.len() - k..].to_vec();
            chunk.truncate(chunk.len() - k);
        }
        if let Err(e) = w.write_all(&strip_hints(&chunk)).and_then(|()| w.flush()) {
            return Stop::Err(e);
        }
    }
}

/// The length of the longest suffix of b that is a proper prefix of a
/// marker the daemon sends in-band.
fn partial_marker(b: &[u8]) -> usize {
    let mut best = 0;
    for m in [DETACH_MARKER, HINT_MARKER, KICKED] {
        let mut k = (m.len() - 1).min(b.len());
        while k > best {
            if b.ends_with(&m[..k]) {
                best = k;
                break;
            }
            k -= 1;
        }
    }
    best
}

fn strip_hints(b: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if find(b, HINT_MARKER).is_none() {
        return b.into();
    }
    let mut out = Vec::with_capacity(b.len());
    let mut rest = b;
    while let Some(i) = find(rest, HINT_MARKER) {
        out.extend_from_slice(&rest[..i]);
        rest = &rest[i + HINT_MARKER.len()..];
    }
    out.extend_from_slice(rest);
    out.into()
}

/// A session's screen run in rush's terminal: the terminal is handed over,
/// run relays the live session, and the list comes back when the session
/// asks to detach (or you press ctrl+]).
pub struct Session {
    pub client: Client,
    pub short: String,
}

/// The terminal's size, columns and rows.
fn term_size(fd: RawFd) -> Option<(u16, u16)> {
    // SAFETY: winsize is plain data; TIOCGWINSZ fills it.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: as above, on a descriptor we were given.
    let r = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
    (r == 0 && ws.ws_col > 0).then_some((ws.ws_col, ws.ws_row))
}

/// The terminal at fd, raw until dropped.
struct Raw {
    fd: RawFd,
    was: libc::termios,
}

impl Raw {
    fn new(fd: RawFd) -> std::io::Result<Raw> {
        // SAFETY: termios is plain data; tcgetattr fills it.
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        if unsafe { libc::tcgetattr(fd, &mut t) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let was = t;
        // SAFETY: t is a termios tcgetattr filled.
        unsafe { libc::cfmakeraw(&mut t) };
        // SAFETY: as above.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &t) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Raw { fd, was })
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // SAFETY: putting back what tcgetattr gave.
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.was) };
    }
}

/// What the relay's threads tell run.
enum Said {
    Winch,
    Out(Stop),
}

impl Session {
    pub fn run(&self) -> Result<(), rush_core::adapter::Error> {
        let (inp, outp) = (0, 1);
        let (cols, rows) = term_size(outp).unwrap_or((120, 40));
        let attach_id = format!("rush-{}", std::process::id());
        let a = self.client.attach(&self.short, &attach_id, cols, rows)?;
        // Everything that can fail is made before the terminal is handed
        // over, so a failure never leaves it raw or on the other screen.
        // Resizes, through a pipe the signal writes to.
        let (winch_r, winch_w) = UnixStream::pair()?;
        let winch_stop = winch_r.try_clone()?;
        // Keys go to the session until it ends: the stop pipe says when,
        // so the next key, which belongs to the list, is never swallowed.
        let (stop_r, stop_w) = UnixStream::pair()?;
        let conn = a.conn.try_clone()?;
        let winch = signal_hook::low_level::pipe::register(libc::SIGWINCH, winch_w)?;
        let raw = match Raw::new(inp) {
            Ok(r) => r,
            Err(e) => {
                signal_hook::low_level::unregister(winch);
                return Err(e.into());
            }
        };
        let mut out = std::io::stdout();
        // The session repaints relative to a blank screen, so give it one:
        // the alternate screen, cleared, as Claude Code's own attach does.
        // Without it the repaint lands on top of whatever the shell left
        // behind.
        let mut hello = b"\x1b[?1049h\x1b[H\x1b[2J".to_vec();
        for m in &a.info.dec_modes {
            hello.extend(format!("\x1b[?{m}h").bytes());
        }
        let _ = out.write_all(&hello).and_then(|()| out.flush());

        let (tx, rx) = mpsc::channel();
        let t = tx.clone();
        std::thread::spawn(move || {
            let mut b = [0u8; 64];
            while matches!((&winch_r).read(&mut b), Ok(n) if n > 0) {
                if t.send(Said::Winch).is_err() {
                    break;
                }
            }
        });
        let mut r = a.r;
        let t = tx;
        std::thread::spawn(move || {
            let _ = t.send(Said::Out(pump_out(&mut r, &mut std::io::stdout())));
        });
        let keys = std::thread::spawn(move || pump_in(inp, conn, &stop_r));

        let mut result = Ok(());
        for s in rx.iter() {
            match s {
                Said::Winch => {
                    if let Some((c, r)) = term_size(outp) {
                        let _ = self.client.resize(&self.short, &attach_id, c, r);
                    }
                }
                Said::Out(stop) => {
                    if let Stop::Err(e) = stop
                        && !closed(&e)
                    {
                        result = Err(e.into());
                    }
                    break;
                }
            }
        }
        drop(stop_w);
        // The list reads stdin again only once the relay has stopped
        // reading it.
        let _ = keys.join();
        signal_hook::low_level::unregister(winch);
        // Its resize reader ends with it.
        let _ = winch_stop.shutdown(std::net::Shutdown::Both);
        let _ = a.conn.shutdown(std::net::Shutdown::Both);
        // Modes the session turned on while attached are reset too.
        let mut bye = Vec::new();
        for m in a
            .info
            .dec_modes
            .iter()
            .chain(&[1000, 1002, 1003, 1004, 1006, 2004])
        {
            bye.extend(format!("\x1b[?{m}l").bytes());
        }
        bye.extend(b"\x1b[<u\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.write_all(&bye).and_then(|()| out.flush());
        drop(raw);
        result
    }
}

/// Copies keys from fd to conn until stop closes (or ctrl+], which leaves
/// the session). Waits on both, so it never reads a key past the end.
fn pump_in(fd: RawFd, mut conn: UnixStream, stop: &UnixStream) {
    let mut buf = [0u8; 4096];
    let mut fds = [
        libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: stop.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        // SAFETY: fds is two pollfds we own.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if fds[1].revents != 0 {
            return;
        }
        if fds[0].revents & libc::POLLIN == 0 {
            if fds[0].revents != 0 {
                return;
            }
            continue;
        }
        // SAFETY: reading into our buffer.
        let k = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if k <= 0 {
            let e = std::io::Error::last_os_error().kind();
            if k < 0
                && (e == std::io::ErrorKind::Interrupted || e == std::io::ErrorKind::WouldBlock)
            {
                continue;
            }
            return;
        }
        let keys = &buf[..k as usize];
        if let Some(i) = keys.iter().position(|&b| b == ESCAPE_KEY) {
            let _ = conn.write_all(&keys[..i]);
            let _ = conn.shutdown(std::net::Shutdown::Both);
            return;
        }
        if conn.write_all(keys).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// Reads that hand out one part each.
    struct Chunks(Vec<Vec<u8>>);

    impl Read for Chunks {
        fn read(&mut self, p: &mut [u8]) -> std::io::Result<usize> {
            if self.0.is_empty() {
                return Ok(0);
            }
            let c = self.0.remove(0);
            p[..c.len()].copy_from_slice(&c);
            Ok(c.len())
        }
    }

    #[test]
    fn detach_marker_split_across_reads() {
        let m = DETACH_MARKER;
        let mut r = Chunks(vec![
            b"hello".to_vec(),
            m[..m.len() - 1].to_vec(),
            m[m.len() - 1..].to_vec(),
        ]);
        let mut out = Vec::new();
        assert!(matches!(pump_out(&mut r, &mut out), Stop::Detach));
        assert_eq!(out, b"hello", "marker leaked into output");
    }

    #[test]
    fn trailing_escape_is_not_held_back() {
        let mut out = Vec::new();
        let _ = pump_out(&mut Chunks(vec![b"frame\x1b[?25h".to_vec()]), &mut out);
        assert_eq!(out, b"frame\x1b[?25h");
    }

    #[test]
    fn output_mentioning_ekicked_does_not_detach() {
        let mut out = Vec::new();
        let s = pump_out(
            &mut Chunks(vec![b"grep EKICKED attach.go\n".to_vec()]),
            &mut out,
        );
        assert!(matches!(s, Stop::Eof) && !out.is_empty());
    }

    #[test]
    fn hints_are_taken_out_and_a_kick_ends_it() {
        let mut out = Vec::new();
        relay(
            &mut Chunks(vec![
                [b"a".as_slice(), HINT_MARKER, b"b"].concat(),
                [b"c".as_slice(), KICKED].concat(),
                b"never".to_vec(),
            ]),
            &mut out,
        )
        .unwrap();
        assert_eq!(out, b"ab");
    }

    /// A daemon on a socket of its own, answering one request per
    /// connection with each of answers, and sending back what it was
    /// asked.
    fn daemon(answers: Vec<&'static str>) -> (Client, mpsc::Receiver<Value>, tempfile::TempDir) {
        let cfg = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cfg.path().join("daemon")).unwrap();
        std::fs::write(cfg.path().join("daemon/control.key"), "k3y\n").unwrap();
        // The socket goes where the daemon's would, for a folder of its
        // own: a temp dir's, which no daemon has.
        let c = Client::new(Account::at("x", cfg.path()));
        std::fs::create_dir_all(c.sock_dir()).unwrap();
        let _ = std::fs::remove_file(c.control_sock());
        let l = UnixListener::bind(c.control_sock()).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for a in answers {
                let (s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let _ = tx.send(serde_json::from_str(&line).unwrap());
                (&s).write_all(a.as_bytes()).unwrap();
            }
        });
        (c, rx, cfg)
    }

    #[test]
    fn speaks_proto_one() {
        let (c, asked, _cfg) = daemon(vec![
            "{\"ok\":true}\n",
            "{\"ok\":false,\"code\":\"ENOJOB\",\"message\":\"no such job\"}\n",
            "{\"ok\":true,\"decModes\":[2004],\"state\":\"idle\"}\nSCREEN",
            "{\"ok\":false,\"error\":\"bad\"}\n",
        ]);
        assert!(c.running());
        c.reply("abcd1234", "hi").unwrap();
        let req = asked.recv().unwrap();
        assert_eq!(
            req,
            json!({"op":"reply","short":"abcd1234","text":"hi","proto":1,"auth":"k3y"})
        );
        let e = c.kill("abcd1234").unwrap_err();
        assert_eq!(e.to_string(), "ENOJOB: no such job");
        assert!(is_refusal(&*e, "ENOJOB"));
        assert!(!is_refusal(&*e, "EKICKED"));
        let mut a = c.attach("abcd1234", "rush-1", 80, 24).unwrap();
        assert_eq!(asked.recv().unwrap()["op"], "kill");
        assert_eq!(asked.recv().unwrap()["cols"], 80);
        assert_eq!(a.info.dec_modes, vec![2004]);
        let mut screen = Vec::new();
        relay(&mut a.r, &mut screen).unwrap();
        assert_eq!(screen, b"SCREEN", "what came with the answer is kept");
        let e = c.resize("abcd1234", "rush-1", 1, 1).unwrap_err();
        assert!(is_refusal(&*e, "EFAIL"));
        // An account with no daemon.
        let none = Client::new(Account::at("x", "/nonexistent/rush-test"));
        assert!(!none.running());
        assert!(none.reply("x", "y").is_err());
        let _ = std::fs::remove_dir_all(c.sock_dir());
    }

    #[test]
    fn partial_markers() {
        assert_eq!(partial_marker(b"abc\x1b_cc"), 4);
        assert_eq!(partial_marker(b"abc"), 0);
        assert_eq!(partial_marker(b"EKICK"), 5);
    }
}
