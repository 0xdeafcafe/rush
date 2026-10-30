//! The reminders agents set: a message rush sends a session at a time the
//! agent chose, so it can come back to something later without staying
//! awake to watch the clock. Each reminder is a file under dir(), and a
//! small detached rush process (rush remind run) waits for it and sends
//! it with the session's host, waking the session if it sleeps or queuing
//! behind a turn at work. Nothing polls: the process sleeps until the
//! reminder is due.
//!
//! Ported from internal/remind/remind.go:1-332 (with 4b3f2591's selfexe).

mod local;

use std::cell::Cell;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use rush_state::Time;
use serde::{Deserialize, Serialize};

/// One message a session asked rush to send it at due.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reminder {
    pub id: String,
    pub session: String,
    pub text: String,
    pub due: Time,
    pub created: Time,
}

/// How far ahead a reminder may be set.
pub const LONGEST: Duration = Duration::from_secs(90 * 24 * 3600);

/// Where reminders and their waiters' locks and logs are kept.
pub fn dir() -> PathBuf {
    rush_state::dir().join("reminders")
}

fn path(id: &str, ext: &str) -> PathBuf {
    dir().join(format!("{id}{ext}"))
}

fn nanos(d: Duration) -> i64 {
    d.as_nanos().min(i64::MAX as u128) as i64
}

fn since(now: Time, t: Time) -> i128 {
    now.sub_nanos(t)
}

fn go_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// Saves a reminder for session, due at due, and starts its waiter.
pub fn add(session: &str, text: &str, due: Time, now: Time) -> Result<Reminder, String> {
    let text = text.trim();
    if session.is_empty() {
        return Err("no session to remind".into());
    }
    if text.is_empty() {
        return Err("a reminder needs its text".into());
    }
    if due.after(now.add_nanos(nanos(LONGEST))) {
        return Err(format!(
            "that is more than {} days away",
            LONGEST.as_secs() / 86_400
        ));
    }
    let r = Reminder {
        id: random_hex(4),
        session: session.into(),
        text: text.into(),
        due,
        created: now,
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir())
        .map_err(|e| e.to_string())?;
    let raw = rush_state::json::to_vec(&r).map_err(|e| e.to_string())?;
    let tmp = path(&r.id, ".tmp");
    write_private(&tmp, &raw).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path(&r.id, ".json")).map_err(|e| e.to_string())?;
    start(&r.id)?;
    Ok(r)
}

fn write_private(p: &std::path::Path, b: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(p)?;
    f.write_all(b)
}

/// n random bytes in hex: a reminder's id, or its message's.
pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    fill_random(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn fill_random(b: &mut [u8]) {
    use std::io::Read;
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(b))
        .is_err()
    {
        let n = Time::now().unix_nanos() as u64 ^ (std::process::id() as u64) << 32;
        for (i, x) in b.iter_mut().enumerate() {
            *x = (n >> (i * 8 % 64)) as u8;
        }
    }
}

/// The reminder saved as id.
pub fn read(id: &str) -> Result<Reminder, String> {
    let b = std::fs::read(path(id, ".json")).map_err(|e| e.to_string())?;
    rush_state::json::from_slice(&b).map_err(|e| e.to_string())
}

/// session's reminders, soonest first; every session's, "".
pub fn list(session: &str) -> Vec<Reminder> {
    let Ok(rd) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
        .collect();
    names.sort(); // as Go's Glob
    let mut out: Vec<Reminder> = names
        .iter()
        .filter_map(|id| read(id).ok())
        .filter(|r| session.is_empty() || r.session == session)
        .collect();
    out.sort_by_key(|r| r.due);
    out
}

/// Drops session's reminder id; its waiter sees it gone and ends.
pub fn cancel(session: &str, id: &str) -> Result<(), String> {
    match read(id) {
        Ok(r) if r.session == session => remove(id),
        _ => Err(format!("no reminder {id}")),
    }
}

/// Drops every reminder session has.
pub fn cancel_all(session: &str) -> Result<(), String> {
    let errs: Vec<String> = list(session)
        .iter()
        .filter_map(|r| remove(&r.id).err())
        .collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

fn remove(id: &str) -> Result<(), String> {
    match std::fs::remove_file(path(id, ".json")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

fn open_lock(id: &str) -> std::io::Result<std::fs::File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path(id, ".lock"))
}

fn try_lock(f: &std::fs::File) -> bool {
    // SAFETY: flock on a descriptor f owns.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// Whether a waiter holds id's lock.
fn held(id: &str) -> bool {
    let Ok(f) = open_lock(id) else {
        return false;
    };
    if !try_lock(&f) {
        return true;
    }
    // SAFETY: as try_lock.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
    false
}

thread_local! {
    /// Set in a test of this thread's: start starts no waiter (Go's
    /// starting, swapped in tests).
    static NO_WAITERS: Cell<bool> = const { Cell::new(false) };
}

/// Makes start start no waiter on this thread, for tests: they run the
/// Waiter themselves.
pub fn no_waiters(on: bool) {
    NO_WAITERS.with(|n| n.set(on));
}

/// Has a detached waiter of its own wait for reminder id, unless one
/// already does.
pub fn start(id: &str) -> Result<(), String> {
    if NO_WAITERS.with(Cell::get) {
        return Ok(());
    }
    start_waiter(id)
}

fn start_waiter(id: &str) -> Result<(), String> {
    if held(id) {
        return Ok(());
    }
    let exe = rush_sys::selfexe::path().map_err(|e| e.to_string())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path(id, ".log"))
        .map_err(|e| e.to_string())?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.args(["remind", "run", id])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log2);
    // SAFETY: setsid is async-signal-safe; nothing else runs in the child
    // before exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    // Released, as Go's Process.Release: never waited for.
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Starts a waiter for each reminder that has none, as after a restart of
/// the machine, when every waiter ended with it. A reminder that fell due
/// meanwhile is sent at once.
pub fn resume() {
    for r in list("") {
        let _ = start(&r.id);
    }
}

/// Reminder id's lock, held until it's dropped.
pub struct Held {
    _f: std::fs::File,
}

/// Takes reminder id's lock until what it returns is dropped; None when
/// another waiter has it.
pub fn hold(id: &str) -> Option<Held> {
    let f = open_lock(id).ok()?;
    try_lock(&f).then_some(Held { _f: f })
}

/// Waits for one reminder and sends it. Its clock and its way of sending
/// are its own, for tests.
pub struct Waiter<'a> {
    pub now: &'a mut dyn FnMut() -> Time,
    pub sleep: &'a mut dyn FnMut(Duration),
    /// Sends the reminder's message to its session.
    pub deliver: &'a mut dyn FnMut(&Reminder, &str) -> Result<(), String>,
    /// Whether the session no longer exists, so nothing is sent.
    pub gone: &'a mut dyn FnMut(&str) -> bool,
}

/// The longest a waiter sleeps between looks: short enough that a
/// cancelled reminder's waiter ends soon, and that a machine waking from
/// sleep past the due time sends it within a minute.
pub const SLICE: Duration = Duration::from_secs(60);

/// How many times a waiter tries to send before it gives up.
pub const TRIES: u32 = 5;

impl Waiter<'_> {
    /// Waits until reminder id is due, then sends it and drops it. It ends
    /// early when the reminder is cancelled or its session is gone.
    pub fn run(&mut self, id: &str) -> Result<(), String> {
        loop {
            let Ok(r) = read(id) else {
                tidy(id, true);
                return Ok(()); // cancelled
            };
            if (self.gone)(&r.session) {
                tidy(id, true);
                return remove(id);
            }
            let left = r.due.sub_nanos((self.now)());
            if left > 0 {
                let left = Duration::from_nanos(left.min(u64::MAX as i128) as u64);
                (self.sleep)(left.min(SLICE));
                continue;
            }
            let mut err = None;
            for i in 0..TRIES {
                let now = (self.now)();
                match (self.deliver)(&r, &message(&r, now)) {
                    Ok(()) => {
                        err = None;
                        break;
                    }
                    Err(e) => {
                        eprintln!(
                            "{} rush remind: sending {id}: {e}",
                            local::rfc3339((self.now)())
                        );
                        err = Some(e);
                    }
                }
                if i < TRIES - 1 {
                    (self.sleep)(Duration::from_secs(30 * (i as u64 + 1)));
                }
            }
            tidy(id, err.is_none()); // a failure's log is kept
            let removed = remove(id);
            return match (err, removed) {
                (None, r) => r,
                (Some(e), Ok(())) => Err(e),
                (Some(e), Err(r)) => Err(format!("{e}\n{r}")),
            };
        }
    }
}

/// Removes the lock and, when log, the log a waiter for id leaves.
fn tidy(id: &str, log: bool) {
    let _ = std::fs::remove_file(path(id, ".lock"));
    if log {
        let _ = std::fs::remove_file(path(id, ".log"));
    }
}

/// What the session is sent when r falls due at now.
pub fn message(r: &Reminder, now: Time) -> String {
    let d = since(now, r.due);
    let late = if d > 2 * 60 * 1_000_000_000 {
        format!(
            ", {} late (the machine was asleep or off)",
            span(Duration::from_nanos(d as u64))
        )
    } else {
        String::new()
    };
    format!(
        "<rush-reminder id={} set={} due={}>\nA reminder you set{late}:\n\n{}\n</rush-reminder>",
        go_quote(&r.id),
        go_quote(&local::date_time(local::here(r.created))),
        go_quote(&local::date_time(local::here(r.due))),
        r.text
    )
}

/// r as one line, for lists.
pub fn line(r: &Reminder, now: Time) -> String {
    format!(
        "{}  due {} ({})  {}",
        r.id,
        local::short(r.due),
        until(r.due, now),
        first_line(&r.text)
    )
}

/// t as a reminder's due time is said when it's set: "Mon 2 Jan 15:04 MST",
/// in the local zone.
pub fn due_words(t: Time) -> String {
    local::short_zoned(t)
}

fn first_line(s: &str) -> String {
    let s = s.split('\n').next().unwrap_or("");
    let r: Vec<char> = s.chars().collect();
    if r.len() > 80 {
        return r[..79].iter().collect::<String>() + "…";
    }
    s.to_string()
}

/// d rounded to the minute, halves away from zero, as Go's
/// Round(time.Minute); in nanoseconds.
fn round_minute(ns: i128) -> i128 {
    const M: i128 = 60 * 1_000_000_000;
    let r = ns.rem_euclid(M);
    let down = ns - r;
    if r * 2 >= M { down + M } else { down }
}

/// How long until t, in words: "in 25m", "now".
pub fn until(t: Time, now: Time) -> String {
    let d = t.sub_nanos(now);
    if round_minute(d) <= 0 {
        return "now".into();
    }
    format!("in {}", span(Duration::from_nanos(d as u64)))
}

/// d in words, to the minute: "25m", "1h30m", "3d".
fn span(d: Duration) -> String {
    let mins = (round_minute(d.as_nanos() as i128) / (60 * 1_000_000_000)) as i64;
    let (h, m) = (mins / 60, mins % 60);
    match () {
        _ if h >= 48 => format!("{}d", h / 24),
        _ if h > 0 && m > 0 => format!("{h}h{m}m"),
        _ if h > 0 => format!("{h}h"),
        _ => format!("{m}m"),
    }
}

/// Reads when a reminder is due, relative to now and in the local zone: a
/// wait ("25m", "in 2h", "1h30m"), a time of day ("15:30", "3pm",
/// "3:30pm", "tomorrow 9:00"; today's, or tomorrow's once it has passed),
/// or a date and time ("2026-11-02 09:00", RFC 3339).
pub fn parse(s: &str, now: Time) -> Result<Time, String> {
    let raw = s.trim();
    let lower = raw.to_lowercase();
    let wait = lower.strip_prefix("in ").unwrap_or(&lower).trim();
    if let Ok(ns) = rush_events::parse_duration(wait) {
        if ns <= 0 {
            return Err(format!("{} is not in the future", go_quote(&lower)));
        }
        return Ok(now.add_nanos(ns));
    }
    let at = Time::parse(raw).or_else(|| {
        let (y, mo, d, h, mi, sec) = date_and_clock(raw)?;
        Some(local::date(y, mo, d, h, mi, sec))
    });
    if let Some(t) = at {
        if !t.after(now) {
            return Err(format!("{} has passed", local::date_time(t)));
        }
        return Ok(t);
    }
    let (mut day, mut s) = (0, lower.as_str());
    if let Some(rest) = s.strip_prefix("tomorrow") {
        let rest = rest.trim();
        day = 1;
        s = rest.strip_prefix("at ").unwrap_or(rest).trim();
    }
    if let Some((h, m)) = clock(s) {
        let c = local::civil(now);
        let mut t = local::date(c.year, c.month, c.day + day, h, m, 0);
        if !t.after(now) {
            t = local::date(c.year, c.month, c.day + day + 1, h, m, 0);
        }
        return Ok(t);
    }
    Err(format!(
        r#"can't read {} as a time: give a wait ("25m", "2h"), a time of day ("15:30", "3pm", "tomorrow 9:00") or a date and time ("2026-11-02 09:00")"#,
        go_quote(raw)
    ))
}

/// One or two digits, as Go's layouts read an hour: the number and what
/// follows.
fn hour_digits(s: &str) -> Option<(i64, &str)> {
    let b = s.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() {
        return None;
    }
    let n = if b.len() > 1 && b[1].is_ascii_digit() {
        2
    } else {
        1
    };
    Some((s[..n].parse().ok()?, &s[n..]))
}

/// Exactly two digits, as Go's layouts read minutes and seconds.
fn two_digits(s: &str) -> Option<(i64, &str)> {
    let b = s.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_digit() || !b[1].is_ascii_digit() {
        return None;
    }
    Some((s[..2].parse().ok()?, &s[2..]))
}

/// A time of day by Go's layouts "15:04", "3pm", "3:04pm", "3 pm" and
/// "3:04 pm".
fn clock(s: &str) -> Option<(i64, i64)> {
    let (h, rest) = hour_digits(s)?;
    let (m, rest) = match rest.strip_prefix(':') {
        Some(r) => {
            let (m, r) = two_digits(r)?;
            if m >= 60 {
                return None;
            }
            if r.is_empty() {
                return (h < 24).then_some((h, m));
            }
            (m, r)
        }
        None => (0, rest),
    };
    let ampm = rest.trim_start_matches(' ');
    let pm = match ampm {
        "pm" => true,
        "am" => false,
        _ => return None,
    };
    if h > 12 {
        return None;
    }
    let h = match (pm, h) {
        (true, h) if h < 12 => h + 12,
        (false, 12) => 0,
        (_, h) => h,
    };
    Some((h, m))
}

/// A date and time by Go's layouts "2006-01-02 15:04", "2006-01-02T15:04"
/// and the same with seconds.
fn date_and_clock(s: &str) -> Option<(i64, i64, i64, i64, i64, i64)> {
    let b = s.as_bytes();
    if b.len() < 11 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b' ' || b[10] == b'T') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let p = s.get(r)?;
        p.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| p.parse().ok())?
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, rest) = hour_digits(&s[11..])?;
    let (mi, rest) = two_digits(rest.strip_prefix(':')?)?;
    let (sec, rest) = match rest.strip_prefix(':') {
        Some(r) => two_digits(r)?,
        None => (0, rest),
    };
    let days = match mo {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let ok = rest.is_empty()
        && (1..=12).contains(&mo)
        && (1..=days).contains(&d)
        && h < 24
        && mi < 60
        && sec < 60;
    ok.then_some((y, mo, d, h, mi, sec))
}

#[cfg(test)]
mod tests;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
