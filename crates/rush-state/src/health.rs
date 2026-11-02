//! What every rush process knows together about whether each provider's
//! login works: one record per harness and account, kept in one file under
//! rush's folder, of when a turn last got an answer and how its requests
//! last failed.
//!
//! Only failures that say something about the provider count (see
//! failure::Kind): a login that stopped working makes it failing at once;
//! the provider failing for a moment does after STREAK failures in a row,
//! or two or more spanning STREAK_WINDOW, with no answer between. Being
//! busy or rate limited never does, and isn't kept. The error kept is
//! failure::summary's, so nothing a token or key could be in reaches the
//! file.
//!
//! Every time in a record only moves forward, and a record's state counts
//! only failures newer than its last answer, so writes from several
//! processes in any order agree.
//!
//! Ported from internal/health/health.go.

use crate::failure;
use crate::json::{self, lenient};
use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// How many transient failures in a row, with no answer between, make a
/// provider failing.
pub const STREAK: usize = 3;

/// How far apart two transient failures with no answer between make it
/// failing too.
pub const STREAK_WINDOW: Duration = Duration::from_secs(5 * 60);

/// One harness and account's record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// A turn last got an answer.
    #[serde(
        rename = "ok",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero"
    )]
    pub ok: Time,
    /// A request last failed, of any kind.
    #[serde(
        rename = "failed",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero"
    )]
    pub failed: Time,
    /// That failure, as failure::summary says it.
    #[serde(
        rename = "error",
        deserialize_with = "lenient",
        skip_serializing_if = "String::is_empty"
    )]
    pub error: String,
    /// The login last stopped working.
    #[serde(
        rename = "auth",
        deserialize_with = "lenient",
        skip_serializing_if = "is_zero"
    )]
    pub auth: Time,
    /// The newest STREAK failures for a moment, oldest first.
    #[serde(
        rename = "transient",
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub transient: Vec<Time>,
}

fn is_zero(t: &Time) -> bool {
    t.is_zero()
}

fn later(a: Time, b: Time) -> Time {
    if b.after(a) { b } else { a }
}

/// How a provider stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing has answered or failed since its login.
    Unknown,
    Ok,
    Failing,
}

impl Entry {
    /// How e stands: failing on a login that stopped working since the last
    /// answer, or a streak of failures for a moment; else OK once it has
    /// answered.
    pub fn state(&self) -> State {
        if self.auth.after(self.ok) {
            return State::Failing;
        }
        let streak: Vec<Time> = self
            .transient
            .iter()
            .copied()
            .filter(|t| t.after(self.ok))
            .collect();
        if streak.len() >= STREAK
            || streak.len() > 1
                && streak[streak.len() - 1].sub_nanos(streak[0]) >= STREAK_WINDOW.as_nanos() as i128
        {
            return State::Failing;
        }
        if self.ok.is_zero() {
            return State::Unknown;
        }
        State::Ok
    }

    /// Whether an answer at at would change e: a failure newer than its
    /// last answer to clear, or no answer yet.
    fn owes(&self, at: Time) -> bool {
        self.ok.is_zero() || self.failed.after(self.ok) && at.after(self.ok)
    }

    /// e and o together: each time its newest, the newest STREAK transient
    /// failures of both, and the error of the newest failure.
    fn merge(&self, o: &Entry) -> Entry {
        let mut m = Entry {
            ok: later(self.ok, o.ok),
            failed: later(self.failed, o.failed),
            auth: later(self.auth, o.auth),
            error: self.error.clone(),
            transient: Vec::new(),
        };
        if !o.failed.before(self.failed) && !o.error.is_empty() {
            m.error = o.error.clone();
        }
        let mut t: Vec<Time> = self.transient.iter().chain(&o.transient).copied().collect();
        t.sort();
        t.dedup();
        if t.len() > STREAK {
            t.drain(..t.len() - STREAK);
        }
        m.transient = t;
        m
    }
}

/// The record of harness's account: a login's id, or the profile it runs
/// under.
pub fn key(harness: &str, account: &str) -> String {
    format!("{harness}/{account}")
}

/// Key's harness and account.
pub fn split(key: &str) -> (&str, &str) {
    key.split_once('/').unwrap_or((key, ""))
}

/// The file every rush process shares.
pub fn path() -> PathBuf {
    crate::dir().join("health.json")
}

/// Every record, by key.
pub type All = BTreeMap<String, Entry>;

/// The file as last read: parsed again only once it changed, as a turn's
/// end looks at it every time.
struct Read {
    path: PathBuf,
    modified: Option<SystemTime>,
    size: u64,
    ino: u64,
    all: Arc<All>,
}

static READ: Mutex<Option<Read>> = Mutex::new(None);

/// Every record, by key; empty when there's no file. Shared: copy it to
/// change it.
pub fn load() -> Arc<All> {
    let p = path();
    let Ok(fi) = fs::metadata(&p) else {
        return Arc::default();
    };
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = read.as_ref()
        && r.path == p
        && r.modified == fi.modified().ok()
        && r.size == fi.len()
        && r.ino == fi.ino()
    {
        return r.all.clone();
    }
    let all = Arc::new(load_file(&p));
    *read = Some(Read {
        path: p,
        modified: fi.modified().ok(),
        size: fi.len(),
        ino: fi.ino(),
        all: all.clone(),
    });
    all
}

fn load_file(p: &std::path::Path) -> All {
    // A torn or hand-edited file starts over.
    let mut all: All = fs::read(p)
        .ok()
        .and_then(|b| json::from_slice(&b).ok())
        .unwrap_or_default();
    for e in all.values_mut() {
        e.error = failure::summary(&e.error); // a hand-edited file keeps nothing else either
    }
    all
}

/// Records a turn of key's getting an answer at at. It writes only when
/// that changes the record: a failure to clear, or its first answer.
pub fn answered(key: &str, at: Time) {
    if let Some(e) = load().get(key)
        && !e.owes(at)
    {
        return;
    }
    update(|all| {
        let e = all.get(key).cloned().unwrap_or_default();
        if e.owes(at) {
            all.insert(
                key.into(),
                e.merge(&Entry {
                    ok: at,
                    ..Entry::default()
                }),
            );
        }
    });
}

/// Records a request of key's failing at at on err. A failure that says
/// nothing about the provider (a usage limit, the network, the prompt)
/// isn't kept, nor one of it busy or rate limiting: that never makes it
/// failing, so it writes nothing. It is the state after.
pub fn failed(key: &str, err: &str, at: Time) -> State {
    let mut f = Entry {
        failed: at,
        error: failure::summary(err),
        ..Entry::default()
    };
    match failure::kind_of(err) {
        failure::Kind::Auth => f.auth = at,
        failure::Kind::Transient => f.transient = vec![at],
        failure::Kind::Capacity => {
            return load().get(key).map(Entry::state).unwrap_or(State::Unknown);
        }
        _ => return State::Unknown,
    }
    let mut st = State::Unknown;
    update(|all| {
        let e = all.get(key).cloned().unwrap_or_default().merge(&f);
        st = e.state();
        all.insert(key.into(), e);
    });
    st
}

/// Forgets harness's records, so each reads Unknown until it next answers
/// or fails: its login was added, renewed or removed, and what the old one
/// did says nothing about the new. account narrows it to one; "" is all of
/// harness's.
pub fn reset(harness: &str, account: &str) {
    if !load().keys().any(|k| matches(k, harness, account)) {
        return; // nothing to forget: no write
    }
    update(|all| all.retain(|k, _| !matches(k, harness, account)));
}

fn matches(key: &str, harness: &str, account: &str) -> bool {
    let (h, a) = split(key);
    h == harness && (account.is_empty() || a == account)
}

/// Every record failing, by key.
pub fn all_failing() -> All {
    load()
        .iter()
        .filter(|(_, e)| e.state() == State::Failing)
        .map(|(k, e)| (k.clone(), e.clone()))
        .collect()
}

/// Changes the file under a lock every rush process shares: read, change,
/// then write a temporary file and rename it over.
fn update(f: impl FnOnce(&mut All)) {
    let p = path();
    let Some(dir) = p.parent() else { return };
    if fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .is_err()
    {
        return;
    }
    let mut lock = p.as_os_str().to_owned();
    lock.push(".lock");
    let lf = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(PathBuf::from(lock));
    if let Ok(lf) = &lf {
        // SAFETY: flock on a descriptor lf owns; let go when it's closed.
        unsafe { libc::flock(lf.as_raw_fd(), libc::LOCK_EX) };
    }
    let mut all = load_file(&p);
    f(&mut all);
    let Ok(b) = json::to_vec(&all) else { return };
    let mut tmp = p.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let wrote = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| std::io::Write::write_all(&mut f, &b));
    if wrote.is_ok() {
        let _ = fs::rename(&tmp, &p);
    }
    drop(lf);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        crate::set_thread_dir(Some(d.path().to_path_buf()));
        d
    }

    fn secs(t: Time, s: i64) -> Time {
        t.add_nanos(s * 1_000_000_000)
    }

    // A login that stops working is failing at once; an answer since clears
    // it. A busy provider never fails. Failures for a moment do after
    // STREAK in a row, or two spanning STREAK_WINDOW.
    #[test]
    fn state() {
        let _d = home();
        let t0 = Time::now();
        let k = "claude/a";
        answered(k, t0);
        assert_eq!(
            failed(k, "API Error: 429 rate_limit_error", secs(t0, 1)),
            State::Ok,
            "rate limited"
        );
        assert_eq!(
            failed(k, "API Error: 401 invalid_grant sk-secret", secs(t0, 2)),
            State::Failing,
            "a refused login fails at once"
        );
        assert_eq!(
            load()[k].error,
            "401 invalid_grant",
            "the error kept is the summary"
        );
        answered(k, secs(t0, 3));
        for i in 0..STREAK - 1 {
            assert_eq!(
                failed(k, "API Error: Connection error.", secs(t0, 4 + i as i64)),
                State::Ok,
                "failure {} for a moment",
                i + 1
            );
        }
        assert_eq!(
            failed(k, "API Error: Connection error.", secs(t0, 10)),
            State::Failing,
            "a streak"
        );
        answered(k, secs(t0, 11));
        failed(k, "API Error: 500", secs(t0, 12));
        assert_eq!(
            failed(k, "API Error: 500", secs(t0, 12) + STREAK_WINDOW),
            State::Failing,
            "two spanning the window"
        );
        assert_eq!(
            failed(k, "Prompt is too long", secs(t0, 3600)),
            State::Unknown
        );
        assert_eq!(
            load()[k].error,
            "500",
            "a failure saying nothing of the provider isn't kept"
        );
    }

    // Writes merge by their newest times, so a late write of an old answer
    // doesn't clear a newer failure; an answer with nothing to clear
    // writes nothing; a login resets its harness's records.
    #[test]
    fn merge_and_reset() {
        let _d = home();
        let t0 = Time::now();
        answered("codex/x", t0);
        failed("codex/x", "401 Unauthorized", secs(t0, 2));
        answered("codex/x", secs(t0, 1)); // an older answer, written late
        assert_eq!(
            load()["codex/x"].state(),
            State::Failing,
            "an older answer cleared a newer failure"
        );
        answered("claude/a", t0);
        // Each write renames a new file over: the same inode is no write.
        let ino = fs::metadata(path()).unwrap().ino();
        answered("claude/a", secs(t0, 60));
        assert_eq!(
            fs::metadata(path()).unwrap().ino(),
            ino,
            "an answer that changes nothing wrote"
        );
        reset("codex", "");
        assert!(
            !load().contains_key("codex/x"),
            "logging in should forget the harness's records"
        );
        assert!(
            load().contains_key("claude/a"),
            "another harness's record went too"
        );
    }
}
