//! What every rush process knows together about whether the API holds up:
//! each session's host and the list keep one record per API, of when a
//! request last failed on the connection, how long checks have passed
//! unbroken since, and when a real request last got an answer.
//!
//! Sessions an API error stopped wait on it as a group: one check every few
//! seconds serves them all, and one of them failing again sets them all
//! back. A session whose prompt cache is still warm tries again as soon as
//! the API can be reached, since a try costs it little. One whose cache has
//! expired re-reads the whole conversation at full price on its next try,
//! so it waits for proof the connection is back for good: a real request
//! answered since the failure, or checks passing unbroken for Hold, and
//! then only one such session goes first (the canary) until one answers.
//!
//! Ported from Go internal/netproof/netproof.go (whole file). The record is
//! the same file Go writes: `<rush>/netproof/<sha256(target)[:8] hex>.json`,
//! changed under `flock` on `<file>.lock`.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::event;

/// How often the API is checked while something waits on it; a check
/// younger than this is shared, not made again.
pub const EVERY: Duration = Duration::from_secs(5);

/// How long checks must pass unbroken, with no request failing, before a
/// session whose cache has expired may try again.
pub const HOLD: Duration = Duration::from_secs(60);

/// How long the one cold session that goes first holds the way: if it
/// hasn't been answered by then, another may go.
const CANARY_FOR: Duration = Duration::from_secs(10 * 60);

/// The record for one API.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    /// A request last failed on the connection.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::when::opt"
    )]
    pub failed: Option<SystemTime>,
    /// Checks have all passed since; None while down.
    #[serde(
        rename = "upSince",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::when::opt"
    )]
    pub up_since: Option<SystemTime>,
    /// The last check.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::when::opt"
    )]
    pub checked: Option<SystemTime>,
    /// A real request last got an answer.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::when::opt"
    )]
    pub answered: Option<SystemTime>,
    /// The cold session trying first.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub canary: String,
    #[serde(
        rename = "canaryAt",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::when::opt"
    )]
    pub canary_at: Option<SystemTime>,
}

/// Whether a is after b, a time never set being the earliest of all (Go's
/// zero time).
fn after(a: Option<SystemTime>, b: Option<SystemTime>) -> bool {
    a > b
}

/// now - t, or None when t is unset (an age longer than any) and zero when
/// t is ahead of now.
fn since(now: SystemTime, t: Option<SystemTime>) -> Option<Duration> {
    t.map(|t| now.duration_since(t).unwrap_or_default())
}

impl Proof {
    /// Whether the last check passed.
    pub fn up(&self) -> bool {
        self.up_since.is_some()
    }

    /// Whether a real request got an answer since the last failure: the
    /// strongest proof.
    pub fn answering(&self) -> bool {
        self.answered.is_some() && self.answered >= self.failed
    }

    /// Whether checks have passed unbroken for hold since the last failure.
    pub fn holding(&self, now: SystemTime, hold: Duration) -> bool {
        self.up()
            && after(self.up_since, self.failed)
            && self
                .up_since
                .is_some_and(|u| now.duration_since(u).is_ok_and(|d| d >= hold))
    }
}

/// The API requests go to: Anthropic's, or the one ANTHROPIC_BASE_URL
/// names, in env (KEY=value, the last wins) or else in this process's
/// environment.
pub fn target(env: &[&str]) -> String {
    target_from(
        &std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default(),
        env,
    )
}

/// [`target`] with this process's ANTHROPIC_BASE_URL given.
pub fn target_from(base: &str, env: &[&str]) -> String {
    let mut base = base;
    for kv in env {
        if let Some(v) = kv.strip_prefix("ANTHROPIC_BASE_URL=") {
            base = v;
        }
    }
    match crate::url::parse(base) {
        Some(u) if !u.hostname().is_empty() => format!("{}://{}", u.scheme, u.host),
        _ => "https://api.anthropic.com".into(),
    }
}

/// Whether target answers HTTP at all: any response, even a refusal, says
/// the connection works end to end, not just that a port opened. It asks
/// curl (on every Mac and nearly every Linux) rather than carry a TLS
/// stack; where there is no curl, a TCP connection to its port stands in.
pub fn http_answers(target: &str) -> bool {
    use std::process::{Command, Stdio};
    let status = Command::new("curl")
        // -q first: no ~/.curlrc, which Go's client doesn't read either.
        .args(["-q", "-s", "-o", "/dev/null", "-I", "-m", "5", "--", target])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) => s.success(),
        Err(_) => {
            let Some(u) = crate::url::parse(target) else {
                return false;
            };
            let port = match (u.port(), u.scheme.as_str()) {
                ("", "http") => "80",
                ("", _) => "443",
                (p, _) => p,
            };
            let addr = crate::url::join_host_port(u.hostname(), port);
            crate::netwatch::dial_addr(&addr, Duration::from_secs(5)).is_ok()
        }
    }
}

type Probe = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// The records, kept under one folder, and how they're checked.
pub struct Store {
    dir: Option<PathBuf>,
    /// How often the API is checked; see [`EVERY`].
    pub every: Duration,
    /// How long checks must hold; see [`HOLD`].
    pub hold: Duration,
    probe: Probe,
    /// target → when this process last recorded an answer.
    answered: Mutex<HashMap<String, SystemTime>>,
}

static DEFAULT: LazyLock<Store> = LazyLock::new(|| Store {
    dir: None,
    every: EVERY,
    hold: HOLD,
    probe: Box::new(http_answers),
    answered: Mutex::new(HashMap::new()),
});

/// The records under rush's folder, as every rush process shares them.
pub fn store() -> &'static Store {
    &DEFAULT
}

impl Store {
    /// Records kept in `dir/netproof`, checked with probe.
    pub fn new(
        dir: impl Into<PathBuf>,
        probe: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Store {
        Store {
            dir: Some(dir.into()),
            every: EVERY,
            hold: HOLD,
            probe: Box::new(probe),
            answered: Mutex::new(HashMap::new()),
        }
    }

    fn path(&self, target: &str) -> PathBuf {
        let sum = Sha256::digest(target.as_bytes());
        let name: String = sum[..8].iter().map(|b| format!("{b:02x}")).collect();
        let dir = self.dir.clone().unwrap_or_else(crate::state_dir);
        dir.join("netproof").join(name + ".json")
    }

    /// Changes target's record under a lock every rush process shares, and
    /// returns it as changed.
    fn update(&self, target: &str, f: impl FnOnce(&mut Proof)) -> Proof {
        let p = self.path(target);
        let dir = p.parent().unwrap_or(Path::new("."));
        if fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .is_err()
        {
            let mut pr = Proof::default();
            f(&mut pr);
            return pr;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(with_ext(&p, ".lock"));
        if let Ok(lf) = &lock {
            // Released when lf closes, at the end of this call.
            unsafe { libc::flock(lf.as_raw_fd(), libc::LOCK_EX) };
        }
        let mut pr = self.load(target);
        f(&mut pr);
        if let Ok(b) = serde_json::to_vec(&pr) {
            let tmp = with_ext(&p, ".tmp");
            if write_file(&tmp, &b).is_ok() {
                let _ = fs::rename(&tmp, &p);
            }
        }
        drop(lock);
        pr
    }

    /// target's record as last written.
    pub fn load(&self, target: &str) -> Proof {
        fs::read(self.path(target))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// target's record with a check no older than every: made now, without
    /// any lock held, if the last is older.
    pub fn check(&self, target: &str) -> Proof {
        let now = SystemTime::now();
        let p = self.load(target);
        if since(now, p.checked).is_some_and(|d| d < self.every) {
            return p;
        }
        let up = (self.probe)(target);
        self.update(target, |p| {
            p.checked = Some(SystemTime::now());
            if !up {
                if p.up() {
                    event("api-check-failed", &[("target", target.into())]);
                }
                p.up_since = None;
            } else if p.up_since.is_none() {
                event(
                    "api-check-passed",
                    &[
                        ("target", target.into()),
                        ("why", "checks pass again; proving it holds".into()),
                    ],
                );
                p.up_since = Some(now);
            }
        })
    }

    /// Records a request failing on the connection at at: every waiting
    /// session starts proving it again.
    pub fn fail(&self, target: &str, at: SystemTime) {
        self.update(target, |p| {
            if p.up() || p.answering() {
                event(
                    "api-failing",
                    &[
                        ("target", target.into()),
                        (
                            "why",
                            "a request failed on the connection; waiting sessions prove it again"
                                .into(),
                        ),
                    ],
                );
            }
            if after(Some(at), p.failed) {
                p.failed = Some(at);
            }
            p.up_since = None;
            p.canary.clear();
        });
    }

    /// Records a real request getting an answer at at. A process writes it
    /// at most every few seconds.
    pub fn answer(&self, target: &str, at: SystemTime) {
        {
            let mut m = self.answered.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(last) = m.get(target)
                && at.duration_since(*last).map_or(true, |d| d < self.every)
            {
                return;
            }
            m.insert(target.to_string(), at);
        }
        self.update(target, |p| {
            let was = p.answering();
            if after(Some(at), p.answered) {
                p.answered = Some(at);
            }
            if !was && p.answering() {
                event(
                    "api-answering",
                    &[
                        ("target", target.into()),
                        (
                            "why",
                            "a real request got an answer since the failure".into(),
                        ),
                    ],
                );
            }
            p.canary.clear();
        });
    }

    /// Asks for id to be the cold session that goes first. It is true when
    /// no other holds the way, or the one that did has had long enough.
    pub fn claim(&self, target: &str, id: &str) -> bool {
        let now = SystemTime::now();
        let mut ok = false;
        self.update(target, |p| {
            let stale = since(now, p.canary_at).is_none_or(|d| d > CANARY_FOR);
            if p.canary.is_empty() || p.canary == id || stale {
                if p.canary != id {
                    event(
                        "api-canary",
                        &[
                            ("target", target.into()),
                            ("session", id.into()),
                            ("was", Value::from(p.canary.as_str())),
                            ("why", "checks held; it tries first".into()),
                        ],
                    );
                }
                p.canary = id.to_string();
                p.canary_at = Some(now);
                ok = true;
            }
        });
        ok
    }

    /// Whether a session that waits on target may try again now: warm (its
    /// cache hasn't expired) once the API can be reached; cold only on
    /// proof the connection holds, and then first only if it's the canary.
    /// id names it for the claim. It makes a check if the last is stale, so
    /// it must not be called from a UI's update.
    pub fn may_go(&self, target: &str, id: &str, warm: bool) -> bool {
        let p = self.check(target);
        if !p.up() {
            return false;
        }
        if warm || p.answering() {
            return true;
        }
        if p.holding(SystemTime::now(), self.hold) {
            return self.claim(target, id);
        }
        false
    }
}

fn with_ext(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(ext);
    PathBuf::from(s)
}

fn write_file(p: &Path, b: &[u8]) -> std::io::Result<()> {
    let mut f: File = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(p)?;
    f.write_all(b)
}

/// target's record as last written, under rush's folder.
pub fn load(target: &str) -> Proof {
    store().load(target)
}

/// See [`Store::check`].
pub fn check(target: &str) -> Proof {
    store().check(target)
}

/// See [`Store::fail`].
pub fn fail(target: &str, at: SystemTime) {
    store().fail(target, at)
}

/// See [`Store::answer`].
pub fn answer(target: &str, at: SystemTime) {
    store().answer(target, at)
}

/// See [`Store::claim`].
pub fn claim(target: &str, id: &str) -> bool {
    store().claim(target, id)
}

/// See [`Store::may_go`].
pub fn may_go(target: &str, id: &str, warm: bool) -> bool {
    store().may_go(target, id, warm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    fn fake(up: Arc<AtomicBool>) -> (tempfile::TempDir, Store, Arc<AtomicU32>) {
        let dir = tempfile::tempdir().unwrap();
        let checks = Arc::new(AtomicU32::new(0));
        let c = checks.clone();
        let mut s = Store::new(dir.path(), move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            up.load(Ordering::SeqCst)
        });
        s.every = Duration::ZERO;
        s.hold = Duration::from_secs(3600);
        (dir, s, checks)
    }

    // A warm session goes once the API can be reached. A cold one waits for
    // proof: checks passing unbroken for Hold, and then only one goes first,
    // or a real answer since the failure, and then every one goes.
    #[test]
    fn cold_sessions_wait_for_proof() {
        let up = Arc::new(AtomicBool::new(false));
        let (_dir, mut s, _) = fake(up.clone());
        const API: &str = "https://api.example";
        // A moment back, so the check that follows is surely after it
        // without sleeping.
        s.fail(API, SystemTime::now() - Duration::from_millis(1));
        assert!(
            !s.may_go(API, "a", true) && !s.may_go(API, "a", false),
            "nothing goes while it's down"
        );
        up.store(true, Ordering::SeqCst);
        assert!(s.may_go(API, "a", true), "a warm session goes once it's up");
        assert!(
            !s.may_go(API, "a", false),
            "a cold session waits for it to hold"
        );
        s.hold = Duration::ZERO;
        assert!(
            s.may_go(API, "a", false) && !s.may_go(API, "b", false),
            "once it holds, one cold session goes first, alone"
        );
        assert!(
            s.may_go(API, "a", false),
            "the one going first keeps the way"
        );
        s.answer(API, SystemTime::now());
        assert!(
            s.may_go(API, "b", false),
            "once one is answered, every cold session goes"
        );

        // Failing again sets every one back.
        s.hold = Duration::from_secs(3600);
        s.fail(API, SystemTime::now() + Duration::from_millis(1));
        assert!(
            !s.may_go(API, "b", false),
            "a failure should undo the proof"
        );
        let p = s.load(API);
        assert!(
            p.canary.is_empty() && !p.answering(),
            "after a failure: {p:?}"
        );
    }

    // A check younger than Every is shared by everyone asking.
    #[test]
    fn checks_are_shared() {
        let (_dir, mut s, checks) = fake(Arc::new(AtomicBool::new(true)));
        s.every = Duration::from_secs(3600);
        for _ in 0..5 {
            s.check("https://api.example");
        }
        assert_eq!(checks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn target_from_env() {
        assert_eq!(target_from("", &[]), "https://api.anthropic.com");
        assert_eq!(
            target_from(
                "",
                &["FOO=1", "ANTHROPIC_BASE_URL=http://localhost:11434/v1"]
            ),
            "http://localhost:11434"
        );
        assert_eq!(
            target_from("https://proxy.example:8443/x", &[]),
            "https://proxy.example:8443"
        );
        assert_eq!(
            target_from("https://proxy.example", &["ANTHROPIC_BASE_URL=localhost:1"]),
            "https://api.anthropic.com"
        );
    }

    // The file is where Go keeps it, and Go's own record reads back.
    #[test]
    fn same_file_as_go() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path(), |_| true);
        let p = s.path("https://api.anthropic.com");
        assert_eq!(p, dir.path().join("netproof").join("85f13dfe69cdbb33.json"));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(
            &p,
            r#"{"failed":"2026-10-09T03:49:27.510218+02:00","upSince":"2026-10-09T03:49:32.510599+02:00","checked":"2026-10-09T13:21:43.624026+02:00","answered":"2026-10-28T22:10:08.767571+01:00","canaryAt":"2026-10-09T03:48:10.16894+02:00"}"#,
        )
        .unwrap();
        let pr = s.load("https://api.anthropic.com");
        assert!(
            pr.up() && pr.answering() && pr.canary.is_empty() && pr.canary_at.is_some(),
            "{pr:?}"
        );
        // Written back, it reads the same, with Go's names and no empty
        // canary.
        s.update("https://api.anthropic.com", |_| {});
        let b = fs::read_to_string(&p).unwrap();
        assert_eq!(
            b,
            r#"{"failed":"2026-10-09T01:49:27.510218Z","upSince":"2026-10-09T01:49:32.510599Z","checked":"2026-10-09T11:21:43.624026Z","answered":"2026-10-28T21:10:08.767571Z","canaryAt":"2026-10-09T01:48:10.16894Z"}"#
        );
        assert_eq!(s.load("https://api.anthropic.com"), pr);
        assert!(with_ext(&p, ".lock").exists());
    }
}
