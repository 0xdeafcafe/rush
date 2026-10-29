//! SSH: rush itself, its whole TUI, from any SSH client on your tailnet
//! (Termius, Blink, a laptop's ssh). Each paired key is a device; a key
//! that isn't one yet is asked for a pairing code (Settings › Remote ›
//! Pair a device), which pairs it. Each device has one rush, kept for
//! SSH_KEEP after its connection drops: connect again and it's where you
//! left it, as tmux would keep it, with no tmux.
//!
//! After Go's internal/remote/sshd.go: serveSSH :71, sshKey :99,
//! sshClientName :124, sshConn :133, sshSession :154, exitWith :206, tuis
//! :212, tui :218, attach :231, chReader :282, decMode :297, pump :302,
//! show :327, resize :348, detach :351, stop :362, end :364, endAll :373.
//! Go's x/crypto/ssh is sshwire.rs here (the transport); its user
//! authentication and channels, as much as serve uses, are below.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::ctx::Ctx;
use crate::devices::Device;
use crate::expose::{Log, say};
use crate::server::Server;
use crate::sshkey::{fingerprint_of, host_key};
use crate::sshwire::{
    MSG_SERVICE_ACCEPT, MSG_SERVICE_REQUEST, Msg, Out, Rd, Transport, takes, verify,
};

/// How long a device's rush is kept after its connection drops.
pub const SSH_KEEP: Duration = Duration::from_secs(30 * 60);

const MSG_USERAUTH_REQUEST: u8 = 50;
const MSG_USERAUTH_FAILURE: u8 = 51;
const MSG_USERAUTH_SUCCESS: u8 = 52;
const MSG_USERAUTH_PK_OK: u8 = 60;
const MSG_USERAUTH_INFO_REQUEST: u8 = 60;
const MSG_USERAUTH_INFO_RESPONSE: u8 = 61;
const MSG_GLOBAL_REQUEST: u8 = 80;
const MSG_REQUEST_FAILURE: u8 = 82;
const MSG_CHANNEL_OPEN: u8 = 90;
const MSG_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
const MSG_CHANNEL_OPEN_FAILURE: u8 = 92;
const MSG_CHANNEL_WINDOW_ADJUST: u8 = 93;
const MSG_CHANNEL_DATA: u8 = 94;
const MSG_CHANNEL_EXTENDED_DATA: u8 = 95;
const MSG_CHANNEL_CLOSE: u8 = 97;
const MSG_CHANNEL_REQUEST: u8 = 98;
const MSG_CHANNEL_SUCCESS: u8 = 99;
const MSG_CHANNEL_FAILURE: u8 = 100;

/// What serve lets a client have in flight to it on a channel.
const WINDOW: u32 = 2 << 20;
const MAX_PACKET: u32 = 1 << 15;
/// Failed sign-ins before the connection ends (x/crypto's MaxAuthTries).
const MAX_AUTH_TRIES: u32 = 6;

/// The rush each device runs; tests set their own (sshExe).
static SSH_EXE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_ssh_exe(p: Option<PathBuf>) {
    *SSH_EXE.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

fn ssh_exe() -> io::Result<PathBuf> {
    match SSH_EXE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        Some(p) => Ok(p),
        None => std::env::current_exe(),
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A key as authorized_keys writes it: "ssh-ed25519 AAAA…".
fn authorized_line(blob: &[u8]) -> String {
    let t = Rd(blob).str().unwrap_or_default();
    format!("{t} {}", STANDARD.encode(blob))
}

/// A client's name from its version, "SSH-2.0-OpenSSH_9.8" being OpenSSH
/// (sshClientName).
pub fn ssh_client_name(v: &str) -> String {
    let v = v.strip_prefix("SSH-2.0-").unwrap_or(v);
    let v = match v.find(['_', '-', ' ', '/']) {
        Some(i) if i > 0 => &v[..i],
        _ => v,
    };
    if v.is_empty() { "SSH".into() } else { v.into() }
}

impl Server {
    /// Serves SSH on the config's address until ctx ends, then ends every
    /// device's rush (serveSSH).
    pub fn serve_ssh(self: &Arc<Self>, ctx: &Ctx, log: &Log) -> Result<(), String> {
        let ln = TcpListener::bind(self.config.ssh_listen_or()).map_err(|e| {
            format!(
                "listen tcp {}: bind: {}",
                self.config.ssh_listen_or(),
                rush_plugin::gopath::errno_text(&e)
            )
        })?;
        let addr = ln.local_addr().map_err(|e| e.to_string())?;
        say(log, &format!("rush serve: ssh on {addr}\n"));
        let r = self.serve_ssh_on(ctx, ln);
        self.tuis.end_all();
        r
    }

    /// serveSSH's loop, on ln.
    pub(crate) fn serve_ssh_on(self: &Arc<Self>, ctx: &Ctx, ln: TcpListener) -> Result<(), String> {
        let (seed, public) = host_key()?;
        let addr = ln.local_addr().map_err(|e| e.to_string())?;
        {
            // The end wakes the accept by connecting to it.
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name("ssh-stop".into())
                .spawn(move || {
                    ctx.wait();
                    let _ = TcpStream::connect(addr);
                })
                .map_err(|e| e.to_string())?;
        }
        loop {
            let c = match ln.accept() {
                Ok((c, _)) => c,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    return if ctx.done() {
                        Ok(())
                    } else {
                        Err(e.to_string())
                    };
                }
            };
            if ctx.done() {
                return Ok(());
            }
            let (s, public) = (self.clone(), public.clone());
            let _ = std::thread::Builder::new()
                .name("ssh-conn".into())
                .spawn(move || s.ssh_conn(c, seed, public));
        }
    }

    /// One connection: its handshake, its sign-in, then its sessions
    /// (sshConn).
    fn ssh_conn(self: &Arc<Self>, c: TcpStream, seed: [u8; 32], public: Vec<u8>) {
        let Ok(raw) = c.try_clone() else {
            return;
        };
        let Ok(mut t) = Transport::accept(c, seed, public) else {
            let _ = raw.shutdown(std::net::Shutdown::Both);
            return;
        };
        let out = t.out.clone();
        let Some(id) = self.ssh_auth(&mut t) else {
            out.close();
            return;
        };
        // Revoking the device ends the connection and its rush.
        let ctx = Ctx::new();
        let ended = Arc::new(AtomicBool::new(false));
        let guard = self.live.add(&id, ctx.clone());
        {
            let (ctx, ended, out, s, id) = (
                ctx.clone(),
                ended.clone(),
                out.clone(),
                self.clone(),
                id.clone(),
            );
            let _ = std::thread::Builder::new()
                .name("ssh-revoke".into())
                .spawn(move || {
                    ctx.wait();
                    if !ended.load(Ordering::SeqCst) {
                        out.close();
                        s.tuis.end(&id);
                    }
                });
        }
        let mut chans: HashMap<u32, Sess> = HashMap::new();
        let mut next_id = 0u32;
        while let Ok(p) = t.read() {
            let mut r = Rd(&p[1..]);
            match p[0] {
                MSG_GLOBAL_REQUEST => {
                    let _ = r.str();
                    if r.bool() == Some(true) {
                        let _ = out.send(Msg::new(MSG_REQUEST_FAILURE));
                    }
                }
                MSG_CHANNEL_OPEN => {
                    let (Some(kind), Some(peer), Some(win), Some(max)) =
                        (r.str(), r.u32(), r.u32(), r.u32())
                    else {
                        break;
                    };
                    if kind != "session" {
                        let _ = out.send(
                            Msg::new(MSG_CHANNEL_OPEN_FAILURE)
                                .u32(peer)
                                .u32(3)
                                .str("only a session")
                                .str(""),
                        );
                        continue;
                    }
                    let local = next_id;
                    next_id += 1;
                    let ch = Arc::new(Chan {
                        out: out.clone(),
                        peer,
                        max: max.clamp(1, MAX_PACKET),
                        st: Mutex::new(ChanState {
                            window: win,
                            closed: false,
                            sent_close: false,
                        }),
                        cv: Condvar::new(),
                    });
                    chans.insert(
                        local,
                        Sess {
                            ch,
                            tui: None,
                            term: "xterm-256color".into(),
                            size: (80, 24),
                            used: 0,
                            done: false,
                        },
                    );
                    let _ = out.send(
                        Msg::new(MSG_CHANNEL_OPEN_CONFIRMATION)
                            .u32(peer)
                            .u32(local)
                            .u32(WINDOW)
                            .u32(MAX_PACKET),
                    );
                }
                MSG_CHANNEL_WINDOW_ADJUST => {
                    if let (Some(local), Some(n)) = (r.u32(), r.u32())
                        && let Some(s) = chans.get(&local)
                    {
                        s.ch.grow(n);
                    }
                }
                MSG_CHANNEL_DATA => {
                    let (Some(local), Some(data)) = (r.u32(), r.bytes()) else {
                        break;
                    };
                    if let Some(s) = chans.get_mut(&local) {
                        s.used += data.len() as u32;
                        if s.used >= WINDOW / 2 {
                            let _ = out.send(
                                Msg::new(MSG_CHANNEL_WINDOW_ADJUST)
                                    .u32(s.ch.peer)
                                    .u32(s.used),
                            );
                            s.used = 0;
                        }
                        if let Some(tui) = &s.tui {
                            tui.keys(&s.ch, data);
                        }
                    }
                }
                MSG_CHANNEL_REQUEST => {
                    let Some(local) = r.u32() else {
                        break;
                    };
                    if let Some(s) = chans.get_mut(&local) {
                        self.ssh_request(&id, s, r);
                    }
                }
                MSG_CHANNEL_CLOSE => {
                    if let Some(local) = r.u32()
                        && let Some(s) = chans.remove(&local)
                    {
                        s.end();
                    }
                }
                // EOF, extended data from the client, and replies to what
                // serve asked: nothing to do.
                _ => {}
            }
        }
        for (_, s) in chans.drain() {
            s.end();
        }
        out.close();
        ended.store(true, Ordering::SeqCst);
        ctx.cancel();
        drop(guard);
        self.devs.touch(&id); // seen until now
    }

    /// Signs the client in: a paired key gets in; any other key, proven
    /// to be the client's, is asked for a pairing code, which pairs it
    /// (sshKey). The device's id, or None for a client that didn't.
    fn ssh_auth(&self, t: &mut Transport) -> Option<String> {
        let out = t.out.clone();
        // The service request comes first.
        loop {
            let p = t.read().ok()?;
            if p[0] == MSG_SERVICE_REQUEST {
                let svc = Rd(&p[1..]).str()?;
                if svc != "ssh-userauth" {
                    out.disconnect(7, "unknown service");
                    return None;
                }
                out.send(Msg::new(MSG_SERVICE_ACCEPT).str(&svc)).ok()?;
                break;
            }
        }
        // The key proven but not paired: what the pairing code pairs.
        let mut proven: Option<(String, String)> = None;
        let mut fails = 0u32;
        let failure = |methods: &str, partial: bool| {
            out.send(Msg::new(MSG_USERAUTH_FAILURE).str(methods).bool(partial))
        };
        let mut pending: Option<Vec<u8>> = None;
        loop {
            if fails >= MAX_AUTH_TRIES {
                out.disconnect(2, "too many authentication failures");
                return None;
            }
            let p = match pending.take() {
                Some(p) => p,
                None => t.read().ok()?,
            };
            if p[0] != MSG_USERAUTH_REQUEST {
                continue;
            }
            let mut r = Rd(&p[1..]);
            let (user, svc, method) = (r.bytes()?, r.str()?, r.str()?);
            match method.as_str() {
                "publickey" if proven.is_none() => {
                    let has_sig = r.bool()?;
                    let algo = r.str()?;
                    let blob = r.bytes()?;
                    if !takes(&algo, blob) {
                        fails += 1;
                        failure("publickey", false).ok()?;
                        continue;
                    }
                    if !has_sig {
                        out.send(Msg::new(MSG_USERAUTH_PK_OK).str(&algo).bytes(blob))
                            .ok()?;
                        continue;
                    }
                    let sig = r.bytes()?;
                    let signed = Msg::default()
                        .bytes(&t.session_id)
                        .byte(MSG_USERAUTH_REQUEST)
                        .bytes(user)
                        .str(&svc)
                        .str("publickey")
                        .bool(true)
                        .str(&algo)
                        .bytes(blob)
                        .0;
                    if !verify(&algo, blob, &signed, sig) {
                        fails += 1;
                        failure("publickey", false).ok()?;
                        continue;
                    }
                    let line = authorized_line(blob);
                    if let Some(dev) = self.devs.ssh(&line) {
                        out.send(Msg::new(MSG_USERAUTH_SUCCESS)).ok()?;
                        return Some(dev.id);
                    }
                    proven = Some((line, fingerprint_of(blob)));
                    failure("keyboard-interactive", true).ok()?;
                }
                "keyboard-interactive" if proven.is_some() => {
                    let ask = Msg::new(MSG_USERAUTH_INFO_REQUEST)
                        .str("")
                        .str(&format!(
                            "This key isn't paired with rush on {} yet.\nIn rush there: Settings › Remote › Pair a device shows a code.",
                            self.config.name
                        ))
                        .str("")
                        .u32(1)
                        .str("Pairing code: ")
                        .bool(true);
                    out.send(ask).ok()?;
                    let p = t.read().ok()?;
                    if p[0] != MSG_USERAUTH_INFO_RESPONSE {
                        // Asked something else instead: that, next.
                        pending = Some(p);
                        continue;
                    }
                    let mut r = Rd(&p[1..]);
                    let n = r.u32()?;
                    let ans = if n == 1 { r.str() } else { None };
                    if !ans.is_some_and(|a| self.pairs.take(&a)) {
                        std::thread::sleep(self.bad_guess);
                        fails += 1;
                        failure("keyboard-interactive", false).ok()?;
                        continue;
                    }
                    let (line, fp) = proven.take()?;
                    let name = format!(
                        "{} · {}",
                        ssh_client_name(&t.client_version),
                        fp.get(7..15).unwrap_or_default()
                    );
                    let dev = self
                        .devs
                        .add(Device {
                            kind: "ssh".into(),
                            name,
                            key: line,
                            fingerprint: fp,
                            ..Device::default()
                        })
                        .ok()?;
                    out.send(Msg::new(MSG_USERAUTH_SUCCESS)).ok()?;
                    return Some(dev.id);
                }
                "none" => failure(
                    if proven.is_some() {
                        "keyboard-interactive"
                    } else {
                        "publickey"
                    },
                    false,
                )
                .ok()?,
                _ => {
                    fails += 1;
                    failure(
                        if proven.is_some() {
                            "keyboard-interactive"
                        } else {
                            "publickey"
                        },
                        false,
                    )
                    .ok()?;
                }
            }
        }
    }

    /// One request on a session: its terminal shows the device's rush
    /// (sshSession).
    fn ssh_request(&self, id: &str, s: &mut Sess, mut r: Rd<'_>) {
        if s.done {
            return;
        }
        let (Some(kind), Some(want)) = (r.str(), r.bool()) else {
            return;
        };
        let mut ok = false;
        match kind.as_str() {
            "pty-req" => {
                if let (Some(term), Some(cols), Some(rows)) = (r.str(), r.u32(), r.u32()) {
                    if !term.is_empty() {
                        s.term = term;
                    }
                    s.size = (cols as u16, rows as u16);
                    ok = true;
                }
            }
            "window-change" => {
                if let (Some(cols), Some(rows)) = (r.u32(), r.u32()) {
                    s.size = (cols as u16, rows as u16);
                    if let Some(t) = &s.tui {
                        t.resize(s.size);
                    }
                }
            }
            "env" => ok = true, // taken, not used
            "shell" => {
                if s.tui.is_none() {
                    match self.tuis.attach(id, &s.ch, &s.term, s.size) {
                        Ok(t) => {
                            s.tui = Some(t);
                            ok = true;
                        }
                        Err(e) => {
                            s.ch.stderr(format!("rush didn't start: {e}\r\n").as_bytes());
                            s.ch.exit(1);
                            s.done = true;
                            return;
                        }
                    }
                }
            }
            "exec" => {
                s.ch.stderr(b"rush over SSH is rush itself: ssh without a command.\r\n");
                s.ch.exit(1);
                s.done = true;
                return;
            }
            _ => {}
        }
        if want {
            let t = if ok {
                MSG_CHANNEL_SUCCESS
            } else {
                MSG_CHANNEL_FAILURE
            };
            let _ = s.ch.out.send(Msg::new(t).u32(s.ch.peer));
        }
    }
}

/// A session channel as the connection keeps it.
struct Sess {
    ch: Arc<Chan>,
    tui: Option<Arc<Tui>>,
    term: String,
    size: (u16, u16),
    /// What the client sent since serve last gave it room.
    used: u32,
    /// Ended by serve (a command, or rush not starting).
    done: bool,
}

impl Sess {
    /// The client closed it, or the connection went: rush is kept for
    /// the next.
    fn end(self) {
        self.ch.closed();
        if let Some(t) = &self.tui {
            t.detach(&self.ch);
        }
        self.ch.close();
    }
}

/// A session channel, for writing to.
pub(crate) struct Chan {
    out: Arc<Out>,
    peer: u32,
    max: u32,
    st: Mutex<ChanState>,
    cv: Condvar,
}

struct ChanState {
    window: u32,
    closed: bool,
    sent_close: bool,
}

impl Chan {
    fn grow(&self, n: u32) {
        let mut st = lock(&self.st);
        st.window = st.window.saturating_add(n);
        drop(st);
        self.cv.notify_all();
    }

    /// No more is written to it.
    fn closed(&self) {
        lock(&self.st).closed = true;
        self.cv.notify_all();
    }

    fn send(&self, ext: bool, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let n = {
                let mut st = lock(&self.st);
                while st.window == 0 && !st.closed {
                    st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                }
                if st.closed {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                let n = (data.len() as u32).min(st.window).min(self.max);
                st.window -= n;
                n as usize
            };
            let m = if ext {
                Msg::new(MSG_CHANNEL_EXTENDED_DATA).u32(self.peer).u32(1)
            } else {
                Msg::new(MSG_CHANNEL_DATA).u32(self.peer)
            };
            self.out.send(m.bytes(&data[..n]))?;
            data = &data[n..];
        }
        Ok(())
    }

    pub(crate) fn write(&self, data: &[u8]) -> io::Result<()> {
        self.send(false, data)
    }

    fn stderr(&self, data: &[u8]) {
        let _ = self.send(true, data);
    }

    /// Says how it ended, and closes it (exitWith).
    fn exit(&self, code: u32) {
        let _ = self.out.send(
            Msg::new(MSG_CHANNEL_REQUEST)
                .u32(self.peer)
                .str("exit-status")
                .bool(false)
                .u32(code),
        );
        self.close();
    }

    fn close(&self) {
        let mut st = lock(&self.st);
        st.closed = true;
        let first = !st.sent_close;
        st.sent_close = true;
        drop(st);
        self.cv.notify_all();
        if first {
            let _ = self.out.send(Msg::new(MSG_CHANNEL_CLOSE).u32(self.peer));
        }
    }
}

/// Each device's rush, by device (tuis).
#[derive(Default)]
pub struct Tuis {
    by: Arc<Mutex<HashMap<String, Arc<Tui>>>>,
}

/// One device's rush on a pty, shown on its newest connection (tui).
pub(crate) struct Tui {
    pid: i32,
    master: File,
    keys: Mutex<Sender<(Arc<Chan>, Vec<u8>)>>,
    st: Mutex<TuiState>,
}

#[derive(Default)]
struct TuiState {
    /// The connection it shows on, or none between them.
    on: Option<Arc<Chan>>,
    /// The terminal modes rush set, in the order it first set them.
    modes: Vec<(String, u8)>,
    tail: Vec<u8>,
    /// Ends the wait before rush is stopped, once a connection comes.
    keep: Option<Ctx>,
    ended: bool,
}

impl Tuis {
    /// Device id's rush, shown on ch: the one kept, or a new one.
    fn attach(
        &self,
        id: &str,
        ch: &Arc<Chan>,
        term: &str,
        size: (u16, u16),
    ) -> io::Result<Arc<Tui>> {
        let mut by = lock(&self.by);
        if let Some(t) = by.get(id) {
            t.show(ch, size);
            return Ok(t.clone());
        }
        let (child, master) = spawn(term, size)?;
        let (tx, rx) = channel::<(Arc<Chan>, Vec<u8>)>();
        let t = Arc::new(Tui {
            pid: child.id() as i32,
            master: master.try_clone()?,
            keys: Mutex::new(tx),
            st: Mutex::new(TuiState {
                on: Some(ch.clone()),
                ..TuiState::default()
            }),
        });
        by.insert(id.to_string(), t.clone());
        // The keys of the connection it shows on (chReader).
        {
            // Weak: the thread ends with the tui, when its sender goes.
            let (t, mut w) = (Arc::downgrade(&t), master.try_clone()?);
            std::thread::Builder::new()
                .name("ssh-keys".into())
                .spawn(move || {
                    for (ch, b) in rx {
                        let Some(t) = t.upgrade() else {
                            return;
                        };
                        let on = lock(&t.st).on.as_ref().is_some_and(|o| Arc::ptr_eq(o, &ch));
                        if on && w.write_all(&b).is_err() {
                            return;
                        }
                    }
                })?;
        }
        let (t2, id, all) = (t.clone(), id.to_string(), self.by.clone());
        std::thread::Builder::new()
            .name("ssh-pump".into())
            .spawn(move || {
                let mut child = child;
                t2.pump(master);
                let _ = child.wait();
                {
                    let mut by = lock(&all);
                    if by.get(&id).is_some_and(|x| Arc::ptr_eq(x, &t2)) {
                        by.remove(&id);
                    }
                }
                let on = {
                    let mut st = lock(&t2.st);
                    st.ended = true;
                    if let Some(k) = st.keep.take() {
                        k.cancel();
                    }
                    st.on.take()
                };
                if let Some(on) = on {
                    on.exit(0);
                }
            })?;
        Ok(t)
    }

    /// Stops device id's rush.
    pub(crate) fn end(&self, id: &str) {
        let t = lock(&self.by).get(id).cloned();
        if let Some(t) = t {
            t.stop();
        }
    }

    /// Stops every device's rush.
    pub(crate) fn end_all(&self) {
        for t in lock(&self.by).values() {
            t.stop();
        }
    }

    /// How many devices have a rush.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        lock(&self.by).len()
    }
}

impl Tui {
    /// Keys from ch, for rush while ch is the connection it shows on.
    fn keys(&self, ch: &Arc<Chan>, b: &[u8]) {
        let _ = lock(&self.keys).send((ch.clone(), b.to_vec()));
    }

    /// Copies rush's screen to the connection showing it, noting the
    /// modes it sets on the way; between connections it's read and
    /// dropped, and the next gets a whole new frame (pump).
    fn pump(&self, mut master: File) {
        let mut buf = vec![0u8; 32 << 10];
        loop {
            let n = match master.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            let on = {
                let mut st = lock(&self.st);
                let mut scan = std::mem::take(&mut st.tail);
                scan.extend_from_slice(&buf[..n]);
                for (mode, hl) in dec_modes(&scan) {
                    match st.modes.iter_mut().find(|(m, _)| *m == mode) {
                        Some(m) => m.1 = hl,
                        None => st.modes.push((mode, hl)),
                    }
                }
                st.tail = scan[scan.len().saturating_sub(16)..].to_vec();
                st.on.clone()
            };
            if let Some(on) = on {
                let _ = on.write(&buf[..n]);
            }
        }
    }

    /// Moves rush onto ch, from the connection it was on if any (show).
    fn show(&self, ch: &Arc<Chan>, size: (u16, u16)) {
        let old = {
            let mut st = lock(&self.st);
            if let Some(k) = st.keep.take() {
                k.cancel();
            }
            let old = st.on.replace(ch.clone()).filter(|o| !Arc::ptr_eq(o, ch));
            let set: String = st
                .modes
                .iter()
                .map(|(m, hl)| format!("\x1b[?{m}{}", *hl as char))
                .collect();
            let _ = ch.write(set.as_bytes());
            old
        };
        if let Some(old) = old {
            let _ = old.write(b"\x1b[?1049l\r\nrush moved to another connection.\r\n");
            old.exit(0);
        }
        self.resize(size);
        // The same size wouldn't redraw on its own; a resize has rush
        // draw it all.
        // SAFETY: a signal to rush's own process.
        unsafe { libc::kill(self.pid, libc::SIGWINCH) };
    }

    fn resize(&self, (cols, rows): (u16, u16)) {
        set_size(self.master.as_raw_fd(), cols, rows);
    }

    /// ch's connection is gone: rush is kept for SSH_KEEP for the next
    /// (detach).
    fn detach(self: &Arc<Self>, ch: &Arc<Chan>) {
        let mut st = lock(&self.st);
        if st.ended || !st.on.as_ref().is_some_and(|o| Arc::ptr_eq(o, ch)) {
            return;
        }
        st.on = None;
        let keep = Ctx::new();
        st.keep = Some(keep.clone());
        let t = self.clone();
        let _ = std::thread::Builder::new()
            .name("ssh-keep".into())
            .spawn(move || {
                if keep.sleep(SSH_KEEP) {
                    t.stop();
                }
            });
    }

    fn stop(&self) {
        // SAFETY: a signal to rush's own process.
        unsafe { libc::kill(self.pid, libc::SIGHUP) };
    }
}

/// The terminal modes rush sets in b (the alternate screen, the mouse,
/// pasting): \x1b[?1049h and the like, each mode with its h or l
/// (decMode).
fn dec_modes(b: &[u8]) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < b.len() {
        if &b[i..i + 3] != b"\x1b[?" {
            i += 1;
            continue;
        }
        let start = i + 3;
        let mut j = start;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b';') {
            j += 1;
        }
        if j > start && j < b.len() && (b[j] == b'h' || b[j] == b'l') {
            let list = String::from_utf8_lossy(&b[start..j]).into_owned();
            for m in list.split(';') {
                out.push((m.to_string(), b[j]));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn set_size(fd: i32, cols: u16, rows: u16) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCSWINSZ on a pty's master reads a winsize we own.
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
}

/// The device's rush on a new pty of size, with term.
fn spawn(term: &str, (cols, rows): (u16, u16)) -> io::Result<(Child, File)> {
    let exe = ssh_exe()?;
    let (mut master, mut slave) = (0, 0);
    let mut ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty fills two fds we then own.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut ws,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both fds are fresh from openpty and owned here.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // SAFETY: setting close-on-exec on an fd we own.
    unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    let mut cmd = Command::new(exe);
    if let Some(h) = std::env::var_os("HOME") {
        cmd.current_dir(h);
    }
    cmd.env("TERM", term)
        .env("COLORTERM", "truecolor")
        .env("RUSH_VIA", "ssh");
    if std::env::var_os("LANG").is_none_or(|l| l.is_empty()) {
        cmd.env("LANG", "en_US.UTF-8");
    }
    cmd.stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    Ok((child, File::from(master)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_names() {
        for (v, want) in [
            ("SSH-2.0-OpenSSH_9.8", "OpenSSH"),
            ("SSH-2.0-Termius_7.1", "Termius"),
            ("SSH-2.0-Go", "Go"),
            ("SSH-2.0-", "SSH"),
            ("SSH-2.0-libssh-0.10", "libssh"),
        ] {
            assert_eq!(ssh_client_name(v), want, "{v}");
        }
    }

    #[test]
    fn modes_rush_sets() {
        let got = dec_modes(b"x\x1b[?1049h\x1b[?1000;1006hy\x1b[?25l\x1b[?z");
        let want: Vec<(String, u8)> =
            [("1049", b'h'), ("1000", b'h'), ("1006", b'h'), ("25", b'l')]
                .iter()
                .map(|(m, c)| (m.to_string(), *c))
                .collect();
        assert_eq!(got, want);
    }
}
