//! What rush knows about the network: whether the API can be reached,
//! which network the machine is on and when that changed, how fast bytes
//! are moving, and which of rush's own jobs that need the network are
//! waiting for it to come back.
//!
//! Everything is read on its own thread; the UI only ever takes the last
//! reading ([`now`]), so a slow or absent network never holds up a frame.
//!
//! Ported from Go internal/netwatch/netwatch.go (whole file). One change:
//! Go sampled the interfaces every 5s to notice a new network and to rate
//! bytes; here the kernel says when addresses or links change (see sys),
//! and the interfaces are sampled on a timer only while something shows the
//! rates ([`watch_rates`]). Idle, the watcher wakes only for the API check:
//! every 2 minutes while the network is steady.

mod sys;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rush_core::time::{Ms, now as wall};

use crate::event;

/// How often the API is checked: rarely while the network has been steady,
/// more often while it's been shaky, so it can be called steady again on
/// evidence, and while it's down from every 5s backing off to every 30s,
/// so it's noticed soon after it's back. A new network, or a job failing on
/// what looks like the network, checks at once.
#[derive(Clone, Debug)]
pub struct Timing {
    pub steady_every: Duration,
    pub shaky_every: Duration,
    pub down_first: Duration,
    pub down_most: Duration,
    pub dial_for: Duration,
    /// How often the interfaces are read while something shows the rates.
    pub sample: Duration,
    /// How far back steadiness looks.
    pub window: Duration,
    /// How long a connection may take before it counts as slow.
    pub slow: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            steady_every: Duration::from_secs(120),
            shaky_every: Duration::from_secs(30),
            down_first: Duration::from_secs(5),
            down_most: Duration::from_secs(30),
            dial_for: Duration::from_secs(4),
            sample: Duration::from_secs(5),
            window: Duration::from_secs(10 * 60),
            slow: Duration::from_secs(1),
        }
    }
}

/// What a job gets for asking while the API can't be reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offline;

impl fmt::Display for Offline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("offline · waits for the network")
    }
}

impl Error for Offline {}

/// The last reading.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// A check has finished.
    pub known: bool,
    /// The API answered it.
    pub up: bool,
    /// A check is being made.
    pub checking: bool,
    /// Why the last check failed.
    pub why: String,
    /// Up, or down, since.
    pub since: Ms,
    /// When the last check finished.
    pub checked: Ms,
    /// How long the last check's connection took.
    pub latency: Duration,
    pub checks: u32,
    /// What's checked: host:port.
    pub target: String,

    /// The network the machine is on, as "en0 192.168.1.20".
    pub network: String,
    /// When that last changed; 0 if it hasn't since rush opened.
    pub changed: Ms,
    pub changes: u32,
    /// How long after the network last changed a check said whether the
    /// API answers on it; zero until one has.
    pub noticed: Duration,

    /// Bytes a second in and out, across the machine's own interfaces;
    /// rated is false where they can't be read.
    pub rx_rate: f64,
    pub tx_rate: f64,
    pub rated: bool,

    /// Whether, over the last window, every check answered, none was slow,
    /// and the machine stayed on one network; shaky says what broke it.
    pub steady: bool,
    pub shaky: Vec<String>,
    pub window: Duration,

    /// By name.
    pub jobs: Vec<Job>,
}

/// One of rush's jobs that needs the network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    /// It asked while the network was down, and waits for it.
    pub paused: bool,
    /// When it last ran.
    pub last: Ms,
    /// How it last failed; empty if it didn't.
    pub err: String,
    pub runs: u32,
    /// Times it was held back while the network was down.
    pub skipped: u32,
}

/// One check, kept to judge steadiness.
struct Probe {
    at: Instant,
    up: bool,
    latency: Duration,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CheckResult {
    up: bool,
    latency: Duration,
    why: String,
}

enum Msg {
    Kick,
    Checked(CheckResult),
    /// The machine's addresses or links changed, or the rates are wanted.
    Look,
}

#[derive(Default)]
struct Inner {
    s: State,
    jobs: BTreeMap<String, Job>,
    started: bool,
    subs: Vec<Sender<()>>,
    /// Each interface's bytes in and out, as last read.
    count: HashMap<u32, [u64; 2]>,
    at: Option<Instant>,
    net_key: String,
    /// When the network last changed.
    changed_at: Option<Instant>,
    /// The network changed and no check has finished since.
    pending: bool,
    /// Checks within the window, oldest first.
    hist: VecDeque<Probe>,
    /// Checks failed in a row.
    fails: u32,
}

impl Inner {
    /// name's record.
    fn job(&mut self, name: &str) -> &mut Job {
        self.jobs.entry(name.to_string()).or_insert_with(|| Job {
            name: name.to_string(),
            ..Job::default()
        })
    }

    /// Tells every Changes receiver.
    fn wake(&mut self) {
        for c in self.subs.drain(..) {
            let _ = c.send(());
        }
    }

    /// Judges the network over the window.
    fn steady(&self, now: Instant, t: &Timing) -> (bool, Vec<String>, Duration) {
        let (mut failed, mut slowed) = (0, 0);
        for p in &self.hist {
            if now.saturating_duration_since(p.at) > t.window {
                continue;
            }
            if !p.up {
                failed += 1;
            } else if p.latency > t.slow {
                slowed += 1;
            }
        }
        let mut why = Vec::new();
        if failed > 0 {
            why.push(format!(
                "{failed} check{} failed",
                if failed == 1 { "" } else { "s" }
            ));
        }
        if slowed > 0 {
            why.push(format!("{slowed} slow to connect"));
        }
        if self
            .changed_at
            .is_some_and(|c| now.saturating_duration_since(c) <= t.window)
        {
            why.push("joined another network".into());
        }
        (why.is_empty(), why, t.window)
    }
}

/// The watcher: one per process, behind the free functions below; tests
/// make their own.
pub struct Watcher {
    inner: Mutex<Inner>,
    timing: Timing,
    tx: Mutex<Option<Sender<Msg>>>,
    kicked: AtomicBool,
    rates: AtomicUsize,
}

impl Default for Watcher {
    fn default() -> Self {
        Watcher::new(Timing::default())
    }
}

impl Watcher {
    pub fn new(timing: Timing) -> Watcher {
        Watcher {
            inner: Mutex::new(Inner::default()),
            timing,
            tx: Mutex::new(None),
            kicked: AtomicBool::new(false),
            rates: AtomicUsize::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Begins watching; it is safe to call more than once. A process that
    /// never calls it (a session's host) lets every job run.
    pub fn start(&'static self) {
        {
            let mut w = self.lock();
            if w.started {
                return;
            }
            w.started = true;
            w.s.target = target();
        }
        let (tx, rx) = mpsc::channel();
        let heard = {
            let tx = tx.clone();
            sys::on_change(move || tx.send(Msg::Look).is_ok())
        };
        *self.tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
        let _ = std::thread::Builder::new()
            .name("netwatch".into())
            .spawn(move || self.run_loop(rx, tx, heard));
    }

    /// The last reading.
    pub fn now(&self) -> State {
        self.now_at(Instant::now())
    }

    fn now_at(&self, now: Instant) -> State {
        let w = self.lock();
        let mut s = w.s.clone();
        s.jobs = w.jobs.values().cloned().collect();
        (s.steady, s.shaky, s.window) = w.steady(now, &self.timing);
        s
    }

    /// A receiver told the next time the API goes from down to up, or the
    /// machine joins another network.
    pub fn changes(&self) -> Receiver<()> {
        let (tx, rx) = mpsc::channel();
        self.lock().subs.push(tx);
        rx
    }

    /// Whether a check has said the API can't be reached.
    pub fn down(&self) -> bool {
        let w = self.lock();
        w.started && w.s.known && !w.s.up
    }

    /// A job asking to use the network: false (and the job marked paused)
    /// while it's down.
    pub fn run(&self, name: &str) -> bool {
        let mut w = self.lock();
        let down = w.started && w.s.known && !w.s.up;
        let j = w.job(name);
        if down {
            if !j.paused {
                event(
                    "net-job-held",
                    &[
                        ("job", name.into()),
                        ("why", "the API can't be reached".into()),
                    ],
                );
            }
            j.paused = true;
            j.skipped += 1;
            return false;
        }
        j.paused = false;
        true
    }

    /// Records how a job that ran went. An error that looks like the
    /// network has the API checked at once, rather than at the next check.
    pub fn done(&self, name: &str, err: Option<&(dyn Error + 'static)>) {
        {
            let mut w = self.lock();
            let j = w.job(name);
            j.last = wall();
            j.runs += 1;
            j.err = err.map(|e| e.to_string()).unwrap_or_default();
        }
        if err.is_some_and(is_net_err) {
            self.check();
        }
    }

    /// Runs f as the job name, unless the network is down; then it's
    /// [`Offline`].
    pub fn do_job<T>(
        &self,
        name: &str,
        f: impl FnOnce() -> Result<T, Box<dyn Error + Send + Sync>>,
    ) -> Result<T, Box<dyn Error + Send + Sync>> {
        if !self.run(name) {
            return Err(Box::new(Offline));
        }
        let r = f();
        self.done(
            name,
            r.as_ref().err().map(|e| &**e as &(dyn Error + 'static)),
        );
        r
    }

    /// Has the API checked now.
    pub fn check(&self) {
        if let Some(tx) = &*self.tx.lock().unwrap_or_else(|e| e.into_inner())
            && !self.kicked.swap(true, Ordering::SeqCst)
        {
            let _ = tx.send(Msg::Kick);
        }
    }

    /// Has the interfaces read every few seconds, for the rates, until the
    /// guard is dropped.
    pub fn watch_rates(&self) -> RatesGuard<'_> {
        if self.rates.fetch_add(1, Ordering::SeqCst) == 0
            && let Some(tx) = &*self.tx.lock().unwrap_or_else(|e| e.into_inner())
        {
            let _ = tx.send(Msg::Look);
        }
        RatesGuard(self)
    }

    /// Reads the interfaces when something happens, and checks the API
    /// when it's due, when asked, and when the network changes. It sleeps
    /// in between: until the next check is due, or the next sample while
    /// rates are wanted (or always, where network changes can't be heard).
    fn run_loop(&'static self, rx: Receiver<Msg>, tx: Sender<Msg>, heard: bool) {
        let mut checking = false;
        let mut next = Instant::now(); // check now
        loop {
            let now = Instant::now();
            if self.sample_net(now) && !checking {
                next = now;
            }
            if !checking && now >= next {
                checking = true;
                let addr = {
                    let mut w = self.lock();
                    w.s.checking = true;
                    w.s.target.clone()
                };
                let (tx, dial_for) = (tx.clone(), self.timing.dial_for);
                let _ = std::thread::Builder::new()
                    .name("netwatch-dial".into())
                    .spawn(move || {
                        let _ = tx.send(Msg::Checked(dial(&addr, dial_for)));
                    });
            }
            let mut until = (!checking).then_some(next);
            if !heard || self.rates.load(Ordering::SeqCst) > 0 {
                let s = now + self.timing.sample;
                until = Some(until.map_or(s, |u| u.min(s)));
            }
            let msg = match until {
                Some(u) => rx.recv_timeout(u.saturating_duration_since(now)),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(Msg::Kick) => {
                    self.kicked.store(false, Ordering::SeqCst);
                    if !checking {
                        next = Instant::now();
                    }
                }
                Ok(Msg::Checked(r)) => {
                    checking = false;
                    let at = Instant::now();
                    next = at + self.took(r, at);
                }
                Ok(Msg::Look) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Takes in a check made at now, and says when the next is due.
    fn took(&self, r: CheckResult, now: Instant) -> Duration {
        let t = &self.timing;
        let mut w = self.lock();
        let (was, known) = (w.s.up, w.s.known);
        let s = &mut w.s;
        s.known = true;
        s.up = r.up;
        s.checking = false;
        s.checked = wall();
        s.checks += 1;
        s.latency = r.latency;
        s.why = r.why.clone();
        if !known || was != r.up {
            s.since = s.checked;
            if r.up {
                event(
                    "net-up",
                    &[
                        ("target", s.target.clone().into()),
                        ("latency", go_duration(r.latency).into()),
                    ],
                );
            } else {
                event(
                    "net-down",
                    &[
                        ("target", s.target.clone().into()),
                        ("why", r.why.clone().into()),
                    ],
                );
            }
        }
        if w.pending {
            w.pending = false;
            w.s.noticed = w
                .changed_at
                .map_or(Duration::ZERO, |c| now.saturating_duration_since(c));
        }
        w.hist.push_back(Probe {
            at: now,
            up: r.up,
            latency: r.latency,
        });
        while w
            .hist
            .front()
            .is_some_and(|p| now.saturating_duration_since(p.at) > t.window)
        {
            w.hist.pop_front();
        }
        if !r.up {
            w.fails += 1;
            let shift = (w.fails - 1).min(16);
            return (t.down_first * (1 << shift)).min(t.down_most);
        }
        w.fails = 0;
        for j in w.jobs.values_mut() {
            j.paused = false;
        }
        if known && !was {
            w.wake();
        }
        if w.steady(now, t).0 {
            t.steady_every
        } else {
            t.shaky_every
        }
    }

    /// Reads the interfaces' counters, for the rates, and which network the
    /// machine is on, to notice when that changes; true when it has.
    fn sample_net(&self, now: Instant) -> bool {
        let count = sys::counters();
        let (key, label) = sys::network();
        self.took_sample(now, count, key, label)
    }

    fn took_sample(
        &self,
        now: Instant,
        count: Option<sys::Counters>,
        key: String,
        label: String,
    ) -> bool {
        let mut w = self.lock();
        if let (Some(count), Some(at)) = (&count, w.at) {
            let (mut rx, mut tx) = (0u64, 0u64);
            for (i, c) in count {
                if let Some(p) = w.count.get(i) {
                    rx += grew(p[0], c[0]);
                    tx += grew(p[1], c[1]);
                }
            }
            let d = now.saturating_duration_since(at).as_secs_f64();
            if d > 0.0 {
                w.s.rx_rate = rx as f64 / d;
                w.s.tx_rate = tx as f64 / d;
            }
            w.s.rated = true;
        }
        if let Some(count) = count {
            w.count = count;
            w.at = Some(now);
        }
        if key == w.net_key {
            return false;
        }
        let first = w.net_key.is_empty() && w.s.network.is_empty();
        w.net_key = key;
        w.s.network = label.clone();
        if first {
            return false;
        }
        event("net-changed", &[("network", label.into())]);
        w.s.changed = wall();
        w.changed_at = Some(now);
        w.s.changes += 1;
        w.pending = true;
        w.wake();
        true
    }
}

/// Keeps the interfaces read for the rates while it lives.
pub struct RatesGuard<'a>(&'a Watcher);

impl Drop for RatesGuard<'_> {
    fn drop(&mut self) {
        self.0.rates.fetch_sub(1, Ordering::SeqCst);
    }
}

static W: LazyLock<Watcher> = LazyLock::new(Watcher::default);

/// This process's watcher.
pub fn watcher() -> &'static Watcher {
    &W
}

/// See [`Watcher::start`].
pub fn start() {
    W.start()
}

/// See [`Watcher::now`].
pub fn now() -> State {
    W.now()
}

/// See [`Watcher::changes`].
pub fn changes() -> Receiver<()> {
    W.changes()
}

/// See [`Watcher::down`].
pub fn down() -> bool {
    W.down()
}

/// See [`Watcher::run`].
pub fn run(name: &str) -> bool {
    W.run(name)
}

/// See [`Watcher::done`].
pub fn done(name: &str, err: Option<&(dyn Error + 'static)>) {
    W.done(name, err)
}

/// See [`Watcher::do_job`].
pub fn do_job<T>(
    name: &str,
    f: impl FnOnce() -> Result<T, Box<dyn Error + Send + Sync>>,
) -> Result<T, Box<dyn Error + Send + Sync>> {
    W.do_job(name, f)
}

/// See [`Watcher::check`].
pub fn check() {
    W.check()
}

/// See [`Watcher::watch_rates`].
pub fn watch_rates() -> RatesGuard<'static> {
    W.watch_rates()
}

/// Whether err looks like the network rather than the other end: no
/// route, no DNS, refused, reset, timed out.
pub fn is_net_err(err: &(dyn Error + 'static)) -> bool {
    use std::io::ErrorKind::*;
    let mut e: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(cur) = e {
        if cur.is::<Offline>() {
            return false;
        }
        if let Some(io) = cur.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                ConnectionRefused
                    | ConnectionReset
                    | ConnectionAborted
                    | TimedOut
                    | HostUnreachable
                    | NetworkUnreachable
                    | NetworkDown
                    | NotConnected
                    | AddrNotAvailable
            )
        {
            return true;
        }
        let t = cur.to_string().to_lowercase();
        if [
            "no such host",
            "network is unreachable",
            "connection refused",
            "connection reset",
            "i/o timeout",
            "no route to host",
            "network is down",
            "tls handshake timeout",
            // Rust's word for no DNS answer.
            "failed to lookup address",
        ]
        .iter()
        .any(|s| t.contains(s))
        {
            return true;
        }
        e = cur.source();
    }
    false
}

/// What's dialled to see whether the API answers: Anthropic's, or the one
/// ANTHROPIC_BASE_URL names.
pub fn target() -> String {
    target_from(&std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default())
}

/// [`target`] for a given ANTHROPIC_BASE_URL.
pub fn target_from(base: &str) -> String {
    match crate::url::parse(base) {
        Some(u) if !u.hostname().is_empty() => {
            let port = match (u.port(), u.scheme.as_str()) {
                ("", "http") => "80",
                ("", _) => "443",
                (p, _) => p,
            };
            crate::url::join_host_port(u.hostname(), port)
        }
        _ => "api.anthropic.com:443".into(),
    }
}

/// Opens a TCP connection to addr (host:port), giving up after dial_for in
/// all; how long it took.
pub(crate) fn dial_addr(addr: &str, dial_for: Duration) -> std::io::Result<Duration> {
    use std::net::{TcpStream, ToSocketAddrs};
    let start = Instant::now();
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no such host");
    for a in addr.to_socket_addrs()? {
        let left = dial_for.saturating_sub(start.elapsed());
        if left.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "i/o timeout",
            ));
        }
        match TcpStream::connect_timeout(&a, left) {
            Ok(_) => return Ok(start.elapsed()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn dial(addr: &str, dial_for: Duration) -> CheckResult {
    match dial_addr(addr, dial_for) {
        Ok(latency) => CheckResult {
            up: true,
            latency,
            why: String::new(),
        },
        Err(e) => CheckResult {
            why: short_err(&e),
            ..CheckResult::default()
        },
    }
}

/// The part of a dial's error that says why, in Go's words.
fn short_err(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    let t = e.to_string();
    let words = match e.kind() {
        ConnectionRefused => "connection refused",
        ConnectionReset => "connection reset by peer",
        TimedOut => "i/o timeout",
        HostUnreachable => "no route to host",
        NetworkUnreachable => "network is unreachable",
        NetworkDown => "network is down",
        _ if t.contains("failed to lookup address") => "no such host",
        _ => {
            let t = match t.find(" (os error") {
                Some(i) => &t[..i],
                None => &t[..],
            };
            let t = match t.rfind(": ") {
                Some(i) => &t[i + 2..],
                None => t,
            };
            return t.to_lowercase();
        }
    };
    words.into()
}

/// How far a counter moved from was to is, allowing for one cut to 32 bits
/// wrapping; a counter that went back otherwise was reset.
fn grew(was: u64, is: u64) -> u64 {
    if is >= was {
        is - was
    } else if was < 1 << 32 {
        is + (1 << 32) - was
    } else {
        0
    }
}

/// A duration as Go's Duration.String writes it, as the events log has
/// it: "52.123ms", "1.5s", "0s".
fn go_duration(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns == 0 {
        return "0s".into();
    }
    let trim = |v: u128, unit: u128, digits: usize| -> String {
        let whole = v / unit;
        let frac = v % unit;
        if frac == 0 {
            return whole.to_string();
        }
        let f = format!("{frac:0digits$}");
        format!("{whole}.{}", f.trim_end_matches('0'))
    };
    if ns < 1_000 {
        return format!("{ns}ns");
    }
    if ns < 1_000_000 {
        return format!("{}µs", trim(ns, 1_000, 3));
    }
    if ns < 1_000_000_000 {
        return format!("{}ms", trim(ns, 1_000_000, 6));
    }
    let secs = ns / 1_000_000_000;
    let (h, m) = (secs / 3600, secs / 60 % 60);
    let s = trim(ns % 60_000_000_000, 1_000_000_000, 9);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, m) => format!("{m}m{s}s"),
        (h, m) => format!("{h}h{m}m{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn up(latency: Duration) -> CheckResult {
        CheckResult {
            up: true,
            latency,
            why: String::new(),
        }
    }

    fn failed() -> CheckResult {
        CheckResult {
            why: "timeout".into(),
            ..CheckResult::default()
        }
    }

    #[test]
    fn counters_and_network() {
        let (key, label) = sys::network();
        assert!(!key.is_empty() && !label.is_empty(), "no network key");
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(sys::counters().is_some());
        }
    }

    #[test]
    fn run_pauses_while_down() {
        let w = Watcher::default();
        {
            let mut i = w.lock();
            i.started = true;
            i.s.known = true;
            i.s.up = false;
        }
        let r = w.do_job("usage", || -> Result<(), _> { panic!("ran while down") });
        assert!(r.unwrap_err().is::<Offline>(), "not held back");
        let j = &w.now().jobs[0];
        assert!(j.paused && j.skipped == 1, "job {j:?}");
        let ch = w.changes();
        w.took(up(ms(1)), Instant::now());
        assert!(ch.try_recv().is_ok(), "coming back up didn't say so");
        assert!(!w.now().jobs[0].paused, "still paused once up");
        assert!(w.run("usage"));
    }

    #[test]
    fn unstarted_lets_every_job_run() {
        let w = Watcher::default();
        w.took(failed(), Instant::now());
        assert!(!w.down() && w.run("usage"));
        let r: Result<u8, _> = w.do_job("usage", || Ok(7));
        assert_eq!(r.unwrap(), 7);
        let j = &w.now().jobs[0];
        assert_eq!((j.runs, j.err.as_str()), (1, ""));
        let _ = w.do_job("usage", || -> Result<(), _> {
            Err("401 unauthorized".into())
        });
        assert_eq!(w.now().jobs[0].err, "401 unauthorized");
    }

    #[test]
    fn is_net_err_tells_the_network() {
        let refused = std::net::TcpStream::connect_timeout(
            &"127.0.0.1:1".parse().unwrap(),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(is_net_err(&refused));
        let other: Box<dyn Error> = "401 unauthorized".into();
        assert!(!is_net_err(other.as_ref()));
        assert!(!is_net_err(&Offline));
        let wrapped: Box<dyn Error> = "dial tcp: lookup x: no such host".into();
        assert!(is_net_err(wrapped.as_ref()));
        assert_eq!(short_err(&refused), "connection refused");
    }

    #[test]
    fn grew_wraps() {
        assert_eq!(grew(10, 25), 15);
        assert_eq!(grew((1 << 32) - 10, 5), 15);
        assert_eq!(grew(1 << 40, 5), 0);
    }

    #[test]
    fn steadiness() {
        let w = Watcher::default();
        let t = w.timing.clone();
        let t0 = Instant::now();
        let (ok, why, _) = w.lock().steady(t0, &t);
        assert!(ok, "no checks yet, yet shaky: {why:?}");
        assert_eq!(w.took(up(ms(50)), t0), t.steady_every, "steady check next");
        assert_eq!(w.took(failed(), t0), t.down_first, "first failure next");
        w.took(failed(), t0);
        w.took(failed(), t0);
        assert_eq!(w.took(failed(), t0), t.down_most, "backoff not capped");
        assert_eq!(
            w.took(up(ms(50)), t0),
            t.shaky_every,
            "back up after failures"
        );
        let (ok, why, _) = w.lock().steady(t0, &t);
        assert!(
            !ok && why == ["4 checks failed"],
            "failures within the window: {why:?}"
        );
        let (ok, _, _) = w.lock().steady(t0 + t.window + Duration::from_secs(1), &t);
        assert!(ok, "failures past the window still count");
        w.took(up(ms(1500)), t0);
        assert_eq!(
            w.lock().steady(t0, &t).1,
            ["4 checks failed", "1 slow to connect"]
        );
    }

    #[test]
    fn network_changes_wake_and_are_noticed() {
        let w = Watcher::default();
        let t0 = Instant::now();
        assert!(
            !w.took_sample(t0, None, "en0 1.2.3.4".into(), "en0 1.2.3.4".into()),
            "the first network isn't a change"
        );
        let ch = w.changes();
        assert!(w.took_sample(t0, None, "en0 5.6.7.8".into(), "en0 5.6.7.8".into()));
        assert!(ch.try_recv().is_ok(), "a new network didn't say so");
        let s = w.now_at(t0);
        assert_eq!((s.network.as_str(), s.changes), ("en0 5.6.7.8", 1));
        assert_eq!(s.shaky, ["joined another network"]);
        w.took(up(ms(10)), t0 + ms(300));
        assert_eq!(w.now().noticed, ms(300));
    }

    #[test]
    fn rates() {
        let w = Watcher::default();
        let t0 = Instant::now();
        let key = || ("k".to_string(), "l".to_string());
        w.took_sample(t0, Some(HashMap::from([(4, [100, 50])])), key().0, key().1);
        assert!(!w.now().rated, "one reading rates nothing");
        w.took_sample(
            t0 + ms(500),
            Some(HashMap::from([(4, [1100, 550]), (5, [9, 9])])),
            key().0,
            key().1,
        );
        let s = w.now();
        assert!(
            s.rated && s.rx_rate == 2000.0 && s.tx_rate == 1000.0,
            "{s:?}"
        );
        // The machine's own counters read as well.
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let w = Watcher::default();
            w.sample_net(t0);
            w.sample_net(t0 + ms(100));
            assert!(w.now().rated, "no reading");
        }
    }

    #[test]
    fn targets() {
        for (base, want) in [
            ("", "api.anthropic.com:443"),
            ("http://localhost:11434/v1", "localhost:11434"),
            ("http://proxy.local", "proxy.local:80"),
            ("https://proxy.local/x", "proxy.local:443"),
            ("https://[::1]:8443", "[::1]:8443"),
        ] {
            assert_eq!(target_from(base), want, "{base}");
        }
    }

    #[test]
    fn go_durations() {
        for (d, want) in [
            (Duration::ZERO, "0s"),
            (Duration::from_nanos(800), "800ns"),
            (Duration::from_micros(1500), "1.5ms"),
            (ms(52), "52ms"),
            (Duration::from_nanos(52_123_456), "52.123456ms"),
            (ms(1500), "1.5s"),
            (Duration::from_secs(90), "1m30s"),
            (Duration::from_secs(3661), "1h1m1s"),
        ] {
            assert_eq!(go_duration(d), want);
        }
    }
}
