//! Tickets: a run waits only for memory, fairly by agent. It's files and
//! flock under rush's folder's gate, no daemon: a ticket frees itself when
//! its holder dies, however it dies. Same files as Go rush, so either's
//! runs wait on the other's.
//! Ported from internal/gate/gate.go:1-363.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::rules::{Rule, size};
use crate::share::{State, Ticket, admit, order, room};
use crate::sys::{Machine, Real};

/// Set in a gated program's environment: a gate inside it lets everything
/// straight through, so nothing waits on itself.
pub const HELD_ENV: &str = "RUSH_GATE_HELD";

/// How often a waiter looks again: memory comes back with no event to
/// wait on.
const POLL: Duration = Duration::from_millis(500);

/// Stops a wait: Go's cancelled context. Once cancelled it stays so.
#[derive(Clone, Default)]
pub struct Cancel(Arc<(Mutex<bool>, Condvar)>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        let (m, cv) = &*self.0;
        *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits up to d for the cancel, without waking before: whether it was
    /// cancelled.
    pub fn wait(&self, d: Duration) -> bool {
        let (m, cv) = &*self.0;
        let g = m.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = cv
            .wait_timeout_while(g, d, |c| !*c)
            .unwrap_or_else(|e| e.into_inner());
        *g
    }

    /// Waits until cancelled.
    pub fn wait_forever(&self) {
        let (m, cv) = &*self.0;
        let g = m.lock().unwrap_or_else(|e| e.into_inner());
        let _g = cv.wait_while(g, |c| !*c).unwrap_or_else(|e| e.into_inner());
    }
}

/// Why a run couldn't get in.
#[derive(Debug)]
pub enum AcquireError {
    /// Its waiting was stopped.
    Cancelled,
    Io(io::Error),
}

impl fmt::Display for AcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcquireError::Cancelled => f.write_str("context canceled"),
            AcquireError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AcquireError {}

impl From<io::Error> for AcquireError {
    fn from(e: io::Error) -> Self {
        AcquireError::Io(e)
    }
}

/// A run asking to start.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub name: String,
    pub rule: Rule,
    /// Its agent: RUSH_SESSION, else the agent's pid.
    pub session: String,
    /// The share of memory kept free of gated runs.
    pub reserve: f64,
}

/// Where a waiter stands, told each time it changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wait {
    pub ahead: usize,
    /// -1 before anything is known.
    pub running: i64,
    /// The memory it waits for, and room what there is, when it's next and
    /// memory holds it; both 0 while others go first.
    pub need: u64,
    pub room: u64,
}

/// The gate: where its tickets are, and the machine it shares.
#[derive(Clone)]
pub struct Gate {
    root: PathBuf,
    machine: Arc<dyn Machine>,
}

impl Default for Gate {
    fn default() -> Self {
        Gate::new()
    }
}

impl fmt::Debug for Gate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gate").field("root", &self.root).finish()
    }
}

impl Gate {
    /// The gate under rush's folder, on this machine.
    pub fn new() -> Gate {
        Gate::at(root(), Arc::new(Real))
    }

    /// The gate kept in root, sharing machine: for tests.
    pub fn at(root: impl Into<PathBuf>, machine: Arc<dyn Machine>) -> Gate {
        Gate {
            root: root.into(),
            machine,
        }
    }

    /// Where the gate keeps its tickets and shims.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn machine(&self) -> &dyn Machine {
        &*self.machine
    }

    /// The one folder of tickets: memory is the machine's, so the whole
    /// system shares it.
    fn runs(&self) -> PathBuf {
        self.root.join("system")
    }

    /// Waits until a's run may start, and is its ticket. waiting, if set,
    /// hears each change while it waits.
    pub fn acquire(
        &self,
        cancel: &Cancel,
        a: Ask,
        mut waiting: Option<&mut dyn FnMut(Wait)>,
    ) -> Result<Arc<Run>, AcquireError> {
        let dir = self.runs();
        mkdir_all(&dir)?;
        let (f, path) = take_ticket(&dir, &a)?;
        let mut last = Wait {
            running: -1,
            ..Wait::default()
        };
        loop {
            match self.try_start(&dir, &path, &a) {
                Err(e) => {
                    let _ = std::fs::remove_file(&path);
                    return Err(e.into());
                }
                Ok(Started::In(running)) => {
                    return Ok(Arc::new(Run {
                        gate: self.clone(),
                        f: Mutex::new(Some(f)),
                        path: running,
                        ask: a,
                        w: Mutex::new(WatchState::default()),
                        released: AtomicBool::new(false),
                    }));
                }
                Ok(Started::Waits(w)) => {
                    if !same(&w, &last)
                        && let Some(tell) = waiting.as_mut()
                    {
                        last = w;
                        tell(w);
                    }
                }
            }
            if cancel.wait(POLL) {
                let _ = std::fs::remove_file(&path);
                return Err(AcquireError::Cancelled);
            }
        }
    }

    /// Starts the run when it's next by the fair order and memory lets it,
    /// else says where it stands. Starting is done under the folder's
    /// lock, so two waiters never both judge themselves next.
    fn try_start(&self, dir: &Path, path: &Path, a: &Ask) -> io::Result<Started> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join("admit"))?;
        // ponytail: a blocking lock, held only while a waiter decides.
        flock(&lock, libc::LOCK_EX)?;
        let ts = self.tickets();
        let mine = file_name(path);
        let mut w = Wait {
            running: ts.iter().filter(|t| t.running).count() as i64,
            ahead: order(&ts).iter().take_while(|t| t.file != mine).count(),
            ..Wait::default()
        };
        if w.ahead > 0 {
            return Ok(Started::Waits(w));
        }
        let me = Ticket {
            file: mine.clone(),
            memory: a.rule.memory,
            ..Ticket::default()
        };
        if w.running > 0 {
            // Memory it can't read lets the run start, as without a gate.
            let (avail, total) = self.machine.memory();
            if total > 0 {
                let reserve = (total as f64 * a.reserve) as u64;
                if !admit(&me, &ts, avail, reserve) {
                    w.need = me.memory;
                    w.room = room(&ts, avail, reserve);
                    return Ok(Started::Waits(w));
                }
            }
        }
        let running = dir.join(format!(
            "run-{}",
            mine.strip_prefix("ticket-").unwrap_or(&mine)
        ));
        std::fs::rename(path, &running)?;
        Ok(Started::In(running))
    }

    /// Every live ticket, waiting and running, oldest first; dead ones are
    /// removed.
    pub fn tickets(&self) -> Vec<Ticket> {
        let dir = self.runs();
        let Ok(ents) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for e in ents.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let running = n.starts_with("run-");
            if !running && !n.starts_with("ticket-") || n.contains('.') {
                continue;
            }
            let p = dir.join(&n);
            if !live(&p) {
                let _ = std::fs::remove_file(&p);
                let _ = std::fs::remove_file(with_ext(&p, ".state"));
                continue;
            }
            let mut t = read_ticket(&p);
            t.file = strip(&n).to_string();
            t.running = running;
            if running {
                t.state = read_state(&with_ext(&p, ".state"));
            }
            out.push(t);
        }
        out.sort_by(|a, b| a.file.cmp(&b.file));
        for t in &mut out {
            t.file = format!("ticket-{}", t.file);
        }
        out
    }

    /// Whether the gate holder pid waits for its run to start, or holds a
    /// run it paused: either way the run is waiting, not hung.
    pub fn waiting(&self, pid: i32) -> bool {
        self.tickets()
            .iter()
            .any(|t| t.pid == pid && (!t.running || t.paused()))
    }

    /// Whether pid's run is gated already: a running ticket's holder is
    /// pid's child. So it is when node, its preload's holder holding its
    /// ticket, replaces itself in place (process.execve) with a shim,
    /// losing HELD_ENV: waiting then, it would wait on its own ticket for
    /// ever.
    pub fn held_for(&self, pid: i32) -> bool {
        let mut tab = None;
        for t in self.tickets() {
            if !t.running {
                continue;
            }
            let tab = tab.get_or_insert_with(rush_sys::proc::snapshot);
            if tab.procs.get(&t.pid).is_some_and(|p| p.ppid == pid) {
                return true;
            }
        }
        false
    }

    /// The runs going, oldest first, and the waiters in the order they go,
    /// with reserve the share of memory kept free.
    pub fn status(&self, reserve: f64) -> (Vec<Ticket>, Vec<Waiter>) {
        let ts = self.tickets();
        let running: Vec<Ticket> = ts.iter().filter(|t| t.running).cloned().collect();
        let mut waiting = Vec::new();
        for (i, t) in order(&ts).into_iter().enumerate() {
            let mut why = format!("behind {i}");
            if i == 0 {
                why = "starting".into();
                let (avail, total) = self.machine.memory();
                if !running.is_empty() && total > 0 {
                    let res = (total as f64 * reserve) as u64;
                    if !admit(&t, &ts, avail, res) {
                        why = format!(
                            "for memory: needs {}, there's {} past what's kept free",
                            size(t.memory),
                            size(room(&ts, avail, res))
                        );
                    }
                }
            }
            waiting.push(Waiter {
                ticket: t,
                place: i + 1,
                why,
            });
        }
        (running, waiting)
    }

    /// The memory pressure the gate acts on: the kernel's level, raised to
    /// critical once the memory available is at floor percent or less, and
    /// to warning at twice that, since macOS can say normal while it pages
    /// itself to a halt.
    pub fn pressure(&self, floor: i32) -> i32 {
        crate::guard::pressure(self.machine(), floor)
    }
}

enum Started {
    In(PathBuf),
    Waits(Wait),
}

/// A waiting ticket, with its place in the order they go (1 is next) and
/// why it waits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Waiter {
    pub ticket: Ticket,
    pub place: usize,
    pub why: String,
}

/// Where the gate keeps its tickets and shims: rush's folder's gate.
pub fn root() -> PathBuf {
    rush_state::dir().join("gate")
}

/// The folder of shims sessions find first on PATH.
pub fn bin_dir(root: &Path) -> PathBuf {
    root.join("bin")
}

/// Whether a waiter would tell w as it told last: the room moves each
/// look, so only whether memory is the reason counts.
fn same(w: &Wait, last: &Wait) -> bool {
    w.ahead == last.ahead && w.running == last.running && (w.need > 0) == (last.need > 0)
}

/// What a holder's watchdog keeps, shared with wake.
#[derive(Debug, Default)]
pub(crate) struct WatchState {
    pub root: i32,
    pub state: State,
    /// Paused in all, so far.
    pub paused: Duration,
}

/// A ticket that got in: its holder keeps it until it's released, or
/// dropped.
#[derive(Debug)]
pub struct Run {
    pub(crate) gate: Gate,
    f: Mutex<Option<File>>,
    pub(crate) path: PathBuf,
    pub(crate) ask: Ask,
    pub(crate) w: Mutex<WatchState>,
    released: AtomicBool,
}

impl Run {
    /// What the run asked for.
    pub fn ask(&self) -> &Ask {
        &self.ask
    }

    /// Records what a run its holder watches itself holds, so the room
    /// counts its reservation as what it's yet to take, not all it might.
    pub fn note(&self, footprint: u64) {
        let st = {
            let mut w = self.w.lock().unwrap_or_else(|e| e.into_inner());
            w.state.footprint = footprint;
            w.state.clone()
        };
        self.write_state(&st);
    }

    /// Frees the run's place.
    pub fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = std::fs::remove_file(with_ext(&self.path, ".state"));
        let _ = std::fs::remove_file(&self.path);
        self.f.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// Says s of the run, whole, for anyone reading at once.
    pub(crate) fn write_state(&self, s: &State) {
        let at = s.paused_at.map_or(0, nanos);
        let b = format!(
            "{}\n{}\n{}\n{}\n",
            s.nice,
            s.footprint,
            at,
            s.why.replace('\n', " ")
        );
        let tmp = with_ext(&self.path, ".state.tmp");
        if write_file(&tmp, b.as_bytes(), 0o600).is_ok() {
            let _ = std::fs::rename(&tmp, with_ext(&self.path, ".state"));
        }
    }

    /// The run's ticket as Tickets names it.
    pub(crate) fn ticket_name(&self) -> String {
        format!("ticket-{}", strip(&file_name(&self.path)))
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.release();
    }
}

static SEQ: AtomicI64 = AtomicI64::new(0);

/// Joins the waiters: a file named by when, held with flock so it's seen
/// as gone the moment its holder is. It's locked under a hidden name
/// first, so no one sees it unheld.
fn take_ticket(dir: &Path, a: &Ask) -> io::Result<(File, PathBuf)> {
    let now = nanos(SystemTime::now());
    let base = format!(
        "{:020}-{}-{}",
        now,
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst) + 1
    );
    let tmp = dir.join(format!(".t-{base}"));
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    if let Err(e) = flock(&f, libc::LOCK_EX | libc::LOCK_NB) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let session = a.session.replace('\n', " ");
    let _ = write!(
        f,
        "{}\n{}\n{}\n{}\n{}\n",
        std::process::id(),
        a.name,
        nanos(SystemTime::now()),
        session,
        a.rule.memory
    );
    let path = dir.join(format!("ticket-{base}"));
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok((f, path))
}

/// Whether someone holds path's lock.
fn live(path: &Path) -> bool {
    let Ok(f) = File::open(path) else {
        return false;
    };
    flock(&f, libc::LOCK_EX | libc::LOCK_NB).is_err()
}

fn read_ticket(path: &Path) -> Ticket {
    let b = read_lossy(path);
    let l: Vec<&str> = b.split('\n').collect();
    let mut t = Ticket::default();
    if l.len() >= 5 {
        t.pid = l[0].parse().unwrap_or(0);
        t.name = l[1].to_string();
        t.session = l[3].to_string();
        t.since = l[2].parse::<i64>().ok().map(from_nanos);
        t.memory = l[4].parse().unwrap_or(0);
    }
    t
}

/// What a holder last wrote of its run: nice, footprint, when it was
/// paused (0 for not) and why.
fn read_state(path: &Path) -> State {
    let b = read_lossy(path);
    let l: Vec<&str> = b.splitn(4, '\n').collect();
    let mut s = State::default();
    if l.len() >= 4 {
        s.nice = l[0].parse().unwrap_or(0);
        s.footprint = l[1].parse().unwrap_or(0);
        if let Ok(ns) = l[2].parse::<i64>()
            && ns > 0
        {
            s.paused_at = Some(from_nanos(ns));
        }
        s.why = l[3].trim().to_string();
    }
    s
}

fn read_lossy(path: &Path) -> String {
    let mut b = Vec::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.read_to_end(&mut b);
    }
    String::from_utf8_lossy(&b).into_owned()
}

/// A name without its run- or ticket- prefix.
fn strip(n: &str) -> &str {
    let n = n.strip_prefix("run-").unwrap_or(n);
    n.strip_prefix("ticket-").unwrap_or(n)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// p with ext added to its name.
pub(crate) fn with_ext(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(ext);
    PathBuf::from(s)
}

pub(crate) fn nanos(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    }
}

pub(crate) fn from_nanos(ns: i64) -> SystemTime {
    if ns >= 0 {
        UNIX_EPOCH + Duration::from_nanos(ns as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(ns.unsigned_abs())
    }
}

pub(crate) fn flock(f: &File, how: i32) -> io::Result<()> {
    loop {
        // SAFETY: flock takes the file's own descriptor.
        if unsafe { libc::flock(f.as_raw_fd(), how) } == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

pub(crate) fn mkdir_all(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

use std::os::unix::fs::DirBuilderExt;

/// Writes b to path, made with perm if it's new: os.WriteFile.
pub(crate) fn write_file(path: &Path, b: &[u8], perm: u32) -> io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(perm)
        .open(path)?;
    f.write_all(b)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::rules::GIB;
    use std::sync::atomic::AtomicI32;
    use std::sync::mpsc;

    /// A machine with avail of 64 GiB, whose levels tests set.
    pub struct Fake {
        pub avail: std::sync::atomic::AtomicU64,
        pub pressure: AtomicI32,
        pub level: AtomicI32,
    }

    impl Fake {
        pub fn new(avail: u64) -> Arc<Fake> {
            Arc::new(Fake {
                avail: avail.into(),
                pressure: 0.into(),
                level: (-1).into(),
            })
        }
    }

    impl Machine for Fake {
        fn memory(&self) -> (u64, u64) {
            (self.avail.load(Ordering::SeqCst), 64 * GIB)
        }
        fn pressure_level(&self) -> i32 {
            self.pressure.load(Ordering::SeqCst)
        }
        fn memory_level(&self) -> i32 {
            self.level.load(Ordering::SeqCst)
        }
    }

    pub fn ask(name: &str, memory: u64, session: &str) -> Ask {
        Ask {
            name: name.into(),
            rule: Rule {
                memory,
                limit: Duration::from_secs(3600),
            },
            session: session.into(),
            reserve: 0.0,
        }
    }

    // gate_test.go TestMemoryHoldsTheSecond: a run waits while there's no
    // memory for it beside another, and gets in once that one's gone; one
    // always may when none run.
    #[test]
    fn memory_holds_the_second() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::new(4 * GIB);
        let g = Gate::at(dir.path(), fake);
        let a = g
            .acquire(&Cancel::new(), ask("tsc", 8 * GIB, "a"), None)
            .unwrap();
        let (waits_tx, waits) = mpsc::channel();
        let (got_tx, got) = mpsc::channel();
        let g2 = g.clone();
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        let h = std::thread::spawn(move || {
            let mut tell = |w: Wait| {
                let _ = waits_tx.send(w);
            };
            let b = g2.acquire(&c2, ask("go", 2 * GIB, "b"), Some(&mut tell));
            let _ = got_tx.send(b.is_ok());
            b
        });
        let w = waits.recv().unwrap();
        assert_eq!((w.running, w.need), (1, 2 * GIB), "waiting as {w:?}");
        assert!(got.try_recv().is_err(), "got in with no memory for it");
        let (run, wait) = g.status(0.0);
        assert_eq!((run.len(), wait.len()), (1, 1));
        assert!(wait[0].why.starts_with("for memory"), "{}", wait[0].why);
        a.release();
        assert!(got.recv().unwrap(), "the second didn't get in");
        h.join().unwrap().unwrap().release();
    }

    // A wait stopped gives its ticket up.
    #[test]
    fn cancelled_waits_leave() {
        let dir = tempfile::tempdir().unwrap();
        let g = Gate::at(dir.path(), Fake::new(0));
        let _a = g
            .acquire(&Cancel::new(), ask("tsc", GIB, "a"), None)
            .unwrap();
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        let g2 = g.clone();
        let (told_tx, told) = mpsc::channel();
        let h = std::thread::spawn(move || {
            let mut tell = |_: Wait| {
                let _ = told_tx.send(());
            };
            g2.acquire(&c2, ask("go", GIB, "b"), Some(&mut tell))
                .map(|_| ())
        });
        told.recv().unwrap();
        cancel.cancel();
        assert!(matches!(h.join().unwrap(), Err(AcquireError::Cancelled)));
        assert_eq!(g.tickets().len(), 1);
    }

    // gate_test.go TestDeadHolderFreesItsPlace: a holder gone frees its
    // place. Its lock goes with its file; a process's death does the same.
    #[test]
    fn dead_holder_frees_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let g = Gate::at(dir.path(), Fake::new(0));
        let a = g
            .acquire(&Cancel::new(), ask("held", GIB, ""), None)
            .unwrap();
        // Its file lingers, but no one holds it: as after a kill -9.
        let path = a.path.clone();
        a.f.lock().unwrap().take();
        std::mem::forget(a);
        assert!(path.exists());
        let run = g.acquire(&Cancel::new(), ask("me", GIB, ""), None).unwrap();
        assert!(!path.exists(), "the dead ticket was left");
        run.release();
    }

    // Go's tickets read as Rust's, and the other way: the same file.
    #[test]
    fn ticket_files_as_go_writes_them() {
        let dir = tempfile::tempdir().unwrap();
        let g = Gate::at(dir.path(), Fake::new(0));
        let run = g
            .acquire(&Cancel::new(), ask("tsc", 4 * GIB, "sess\nion"), None)
            .unwrap();
        let name = file_name(&run.path);
        assert!(name.starts_with("run-") && name.len() > 24, "{name}");
        let b = std::fs::read_to_string(&run.path).unwrap();
        let l: Vec<&str> = b.split('\n').collect();
        assert_eq!(l[0], std::process::id().to_string());
        assert_eq!(
            (l[1], l[3], l[4], l[5]),
            ("tsc", "sess ion", "4294967296", "")
        );
        run.write_state(&State {
            nice: 5,
            footprint: 7,
            paused_at: Some(from_nanos(1_700_000_000_000_000_000)),
            why: "short\nof memory".into(),
        });
        assert_eq!(
            std::fs::read_to_string(with_ext(&run.path, ".state")).unwrap(),
            "5\n7\n1700000000000000000\nshort of memory\n"
        );
        let ts = g.tickets();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].file, run.ticket_name());
        assert!(ts[0].running && ts[0].paused());
        assert_eq!(ts[0].state.why, "short of memory");
        assert!(g.waiting(std::process::id() as i32));
    }

    // gate_test.go TestHeldForTheHoldersParent: a process whose child
    // holds a running ticket is gated already.
    #[test]
    fn held_for_the_holders_parent() {
        let dir = tempfile::tempdir().unwrap();
        let g = Gate::at(dir.path(), Fake::new(64 * GIB));
        let ppid = unsafe { libc::getppid() };
        assert!(!g.held_for(ppid), "held before any ticket");
        let run = g
            .acquire(&Cancel::new(), ask("vitest", GIB, "a"), None)
            .unwrap();
        assert!(g.held_for(ppid), "the holder's parent isn't held");
        assert!(!g.held_for(std::process::id() as i32));
        run.release();
    }
}
