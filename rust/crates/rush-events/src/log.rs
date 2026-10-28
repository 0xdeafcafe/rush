//! Writing rush's log. Ported from Go `internal/events/events.go`.
//!
//! Log never waits: lines go to a writer thread of their own, so the UI can
//! log from its thread. A short-lived process (a hook, a command) calls
//! flush before it exits. The writer blocks on its queue: idle, it costs
//! nothing.

use std::collections::hash_map::RandomState;
use std::fs::OpenOptions;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::json::marshal;

/// A line's fields, besides those every line has.
pub type Fields = Map<String, Value>;

/// How big a log grows before it moves to its .1, the one before going
/// (events.go:33).
const MAX_SIZE: u64 = 16 << 20;

/// How many lines can wait for the writer before new ones go.
const QUEUE: usize = 4096;

/// rush's folder: RUSH_HOME, else ~/.config/rush, or ~/.config/agtop where
/// only that has a config.json (Go `state.Dir`, state.go:27). The answer
/// for a home is kept, as Go keeps it: which folder is in use changes only
/// between runs, and it's asked for on every line logged.
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RUSH_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    static DIRS: Mutex<Vec<(PathBuf, PathBuf)>> = Mutex::new(Vec::new());
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut dirs = DIRS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, d)) = dirs.iter().find(|(h, _)| *h == home) {
        return d.clone();
    }
    let rush = home.join(".config").join("rush");
    let agtop = home.join(".config").join("agtop");
    let d = if !rush.join("config.json").exists() && agtop.join("config.json").exists() {
        agtop
    } else {
        rush
    };
    dirs.push((home, d.clone()));
    d
}

/// rush's own log, of every process and session (events.go:37).
pub fn path() -> PathBuf {
    dir().join("events.jsonl")
}

/// A new ev, for a caller that names a line before logging it (as "ev" in
/// its fields) to give it as another's cause. Eight hex digits, no
/// syscall; in a full log two lines share one about once (events.go:89).
pub fn id() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = RandomState::new().build_hasher();
    h.write_u64(N.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:08x}", h.finish() as u32)
}

/// The environment entry that makes ev the cause of a process's lines
/// (events.go:93).
pub fn env(ev: &str) -> String {
    format!("RUSH_CAUSE={ev}")
}

struct Pending {
    n: Mutex<usize>,
    zero: Condvar,
}

impl Pending {
    fn done(&self) {
        let mut n = self.n.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        if *n == 0 {
            self.zero.notify_all();
        }
    }
}

struct Job {
    paths: Vec<PathBuf>,
    b: Vec<u8>,
}

/// Where a Logger writes rush's own log.
enum Main {
    None,
    At(PathBuf),
    /// [`path`], looked up for each line as Go's Path() is: RUSH_HOME can
    /// change after the first line.
    Home,
}

struct Who {
    proc: String,  // which of rush's processes this is: ui, host, hook, …
    cause: String, // the line that led to this process, if one did
}

/// A log writer: rush's log at main (where there is one) and each
/// session's own. The process has one, behind [`log`] and [`session`];
/// tests make their own.
pub struct Logger {
    main: Main,
    who: Mutex<Who>,
    queue: OnceLock<SyncSender<Job>>,
    pending: Arc<Pending>,
}

impl Logger {
    /// A writer to rush's log at main, as process "rush" with no cause.
    pub fn new(main: Option<PathBuf>) -> Logger {
        Logger::with(main.map_or(Main::None, Main::At))
    }

    fn with(main: Main) -> Logger {
        Logger {
            main,
            who: Mutex::new(Who {
                proc: "rush".into(),
                cause: String::new(),
            }),
            queue: OnceLock::new(),
            pending: Arc::new(Pending {
                n: Mutex::new(0),
                zero: Condvar::new(),
            }),
        }
    }

    fn who(&self) -> std::sync::MutexGuard<'_, Who> {
        self.who.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Names this process in its lines: ui, host, hook, cli.
    pub fn set_process(&self, name: &str) {
        self.who().proc = name.to_string();
    }

    /// Makes ev the cause of this process's lines from now on, where a line
    /// doesn't name its own; "" for none.
    pub fn set_cause(&self, ev: &str) {
        self.who().cause = ev.to_string();
    }

    /// What this process's lines give as their cause.
    pub fn cause(&self) -> String {
        self.who().cause.clone()
    }

    /// Records an event of kind, with fields, in rush's log.
    pub fn log(&self, kind: &str, fields: Fields) {
        self.write(kind, "", None, fields)
    }

    /// Records an event of a session's, in its own log at path and in
    /// rush's, where it says which session.
    pub fn session(&self, id: &str, path: &Path, kind: &str, fields: Fields) {
        self.write(kind, id, Some(path), fields)
    }

    fn write(&self, kind: &str, session: &str, path: Option<&Path>, mut m: Fields) {
        if m.get("ev").and_then(Value::as_str).unwrap_or("").is_empty() {
            m.insert("ev".into(), Value::String(id()));
        }
        {
            let who = self.who();
            m.insert("proc".into(), Value::String(who.proc.clone()));
            match m.get("cause") {
                None if !who.cause.is_empty() => {
                    m.insert("cause".into(), Value::String(who.cause.clone()));
                }
                Some(Value::String(c)) if c.is_empty() => {
                    m.remove("cause"); // one that names none has none
                }
                _ => {}
            }
        }
        m.insert("t".into(), Value::String(rfc3339_nano(SystemTime::now())));
        m.insert("kind".into(), Value::String(kind.into()));
        m.insert("pid".into(), Value::from(std::process::id()));
        if !session.is_empty() {
            m.insert("session".into(), Value::String(session.into()));
        }
        let mut b = marshal(&Value::Object(m), false).into_bytes();
        b.push(b'\n');
        let mut paths = Vec::with_capacity(2);
        match &self.main {
            Main::None => {}
            Main::At(p) => paths.push(p.clone()),
            Main::Home => paths.push(self::path()),
        }
        if let Some(p) = path.filter(|p| !p.as_os_str().is_empty()) {
            paths.push(p.to_path_buf());
        }
        if paths.is_empty() {
            return;
        }
        let queue = self.queue.get_or_init(|| self.start());
        *self.pending.n.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        match queue.try_send(Job { paths, b }) {
            Ok(()) => {}
            // The writer is that far behind (or gone): this one goes.
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => self.pending.done(),
        }
    }

    fn start(&self) -> SyncSender<Job> {
        let (tx, rx): (SyncSender<Job>, Receiver<Job>) = sync_channel(QUEUE);
        let pending = self.pending.clone();
        let _ = std::thread::Builder::new()
            .name("rush-events".into())
            .spawn(move || {
                for j in rx {
                    for p in &j.paths {
                        append_line(p, &j.b);
                    }
                    pending.done();
                }
            });
        tx
    }

    /// Waits, up to a second, for what's been logged to be written.
    pub fn flush(&self) {
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut n = self.pending.n.lock().unwrap_or_else(|e| e.into_inner());
        while *n > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            n = match self.pending.zero.wait_timeout(n, left) {
                Ok((n, _)) => n,
                Err(e) => e.into_inner().0,
            };
        }
    }
}

/// Adds b to the log at p. A line appended whole never interleaves with
/// another process's (events.go:174).
fn append_line(p: &Path, b: &[u8]) {
    if std::fs::metadata(p).is_ok_and(|st| st.len() > MAX_SIZE) {
        let _ = std::fs::rename(p, rotated(p));
    }
    let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(p)
    else {
        return; // its folder is gone, or never was: nothing to keep it in
    };
    let _ = f.write_all(b);
}

/// The log before p: its .1.jsonl.
pub(crate) fn rotated(p: &Path) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    let base = s.strip_suffix(".jsonl").unwrap_or(&s);
    PathBuf::from(format!("{base}.1.jsonl"))
}

/// t as Go's time.RFC3339Nano writes a UTC time: trailing zeros of the
/// fraction dropped, no fraction where it's whole seconds.
pub(crate) fn rfc3339_nano(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let mut s = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    );
    let nanos = d.subsec_nanos();
    if nanos > 0 {
        let f = format!("{nanos:09}");
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    s.push('Z');
    s
}

fn global() -> &'static Logger {
    static G: OnceLock<Logger> = OnceLock::new();
    G.get_or_init(|| {
        let l = Logger::with(Main::Home);
        l.set_cause(&std::env::var("RUSH_CAUSE").unwrap_or_default());
        l
    })
}

/// Names this process in its lines: ui, host, hook, cli.
pub fn set_process(name: &str) {
    global().set_process(name)
}

/// Makes ev the cause of this process's lines from now on, where a line
/// doesn't name its own; "" for none.
pub fn set_cause(ev: &str) {
    global().set_cause(ev)
}

/// What this process's lines give as their cause: RUSH_CAUSE, unless
/// set_cause changed it.
pub fn cause() -> String {
    global().cause()
}

/// Records an event of kind, with fields, in rush's log.
pub fn log(kind: &str, fields: Fields) {
    global().log(kind, fields)
}

/// Records an event of a session's, in its own log at path and in rush's,
/// where it says which session.
pub fn session(id: &str, path: &Path, kind: &str, fields: Fields) {
    global().session(id, path, kind, fields)
}

/// Waits, up to a second, for what's been logged to be written.
pub fn flush() {
    global().flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TempDir;
    use serde_json::json;

    fn fields(v: Value) -> Fields {
        match v {
            Value::Object(m) => m,
            _ => Fields::new(),
        }
    }

    // events_test.go:10
    #[test]
    fn log_writes_rush_and_session_logs() {
        let home = TempDir::new();
        let main = home.0.join("events.jsonl");
        let l = Logger::new(Some(main.clone()));
        l.set_process("test");
        let sess = home.0.join("s1.jsonl");
        l.log("restarted", fields(json!({"why": "stale"})));
        l.session("s1", &sess, "check-in", Fields::new());
        l.flush();
        let all = std::fs::read_to_string(&main).unwrap();
        let own = std::fs::read_to_string(&sess).unwrap();
        assert!(
            all.matches('\n').count() == 2
                && all.contains(r#""session":"s1""#)
                && all.contains(r#""proc":"test""#),
            "rush's log:\n{all}"
        );
        assert!(
            own.contains(r#""kind":"check-in""#) && !own.contains("restarted"),
            "the session's log:\n{own}"
        );
    }

    #[test]
    fn a_full_log_moves_to_its_one_before() {
        let home = TempDir::new();
        let main = home.0.join("events.jsonl");
        std::fs::write(&main, vec![b'x'; MAX_SIZE as usize + 1]).unwrap();
        let l = Logger::new(Some(main.clone()));
        l.log("k", Fields::new());
        l.flush();
        assert_eq!(
            std::fs::metadata(home.0.join("events.1.jsonl"))
                .unwrap()
                .len(),
            MAX_SIZE + 1
        );
        assert!(
            std::fs::read_to_string(&main)
                .unwrap()
                .contains(r#""kind":"k""#)
        );
    }

    #[test]
    fn times_as_go_writes_them() {
        for (secs, nanos, want) in [
            (0, 0, "1970-01-01T00:00:00Z"),
            (1_793_190_896, 789_000_000, "2026-10-28T12:34:56.789Z"),
            (1_793_190_896, 123_456_789, "2026-10-28T12:34:56.123456789Z"),
            (951_782_400, 10, "2000-02-29T00:00:00.00000001Z"),
        ] {
            let t = UNIX_EPOCH + Duration::new(secs, nanos);
            assert_eq!(rfc3339_nano(t), want);
        }
    }

    #[test]
    fn ids_are_eight_hex_digits() {
        let a = id();
        assert_eq!(a.len(), 8);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(a, id());
        assert_eq!(env("ab"), "RUSH_CAUSE=ab");
    }
}
