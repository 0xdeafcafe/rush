//! Asking GitHub through the host's gh, signed in as the user. It's files
//! and flock under state.Dir()/watch, no daemon: every agent watching the
//! same pull request shares one answer, and asking again sends the last
//! ETag, so an answer that hasn't changed (304) costs none of the user's
//! rate limit.
//! Ported from internal/watch/gh.go.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use base64::Engine as _;
use rush_gh::ghpr::RunError;
use rush_state::Time;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// What running gh gave: its output, what it said on stderr, and how it
/// failed, if it did. Output is kept even when it failed: `gh api -i`
/// exits 1 on a 304, its headers on stdout.
#[derive(Debug, Clone, Default)]
pub struct GhOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub err: Option<RunError>,
}

/// Runs gh with args, in dir when one is given. The real one is
/// [`SystemGh`]; tests set a fake with [`set_thread_gh`].
pub trait Gh: Send + Sync {
    fn run(&self, dir: Option<&Path>, args: &[String]) -> GhOutput;
}

impl<F> Gh for F
where
    F: Fn(Option<&Path>, &[String]) -> GhOutput + Send + Sync,
{
    fn run(&self, dir: Option<&Path>, args: &[String]) -> GhOutput {
        self(dir, args)
    }
}

/// The gh command on the PATH. Like Go's exec.Command, its stdin is
/// /dev/null and it runs for as long as it takes; ^C reaches it too, being
/// in rush's process group.
pub struct SystemGh;

impl Gh for SystemGh {
    fn run(&self, dir: Option<&Path>, args: &[String]) -> GhOutput {
        let mut cmd = Command::new("gh");
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
            cmd.current_dir(d);
        }
        match cmd.output() {
            Ok(o) => GhOutput {
                err: (!o.status.success()).then(|| RunError::Exit {
                    status: exit_words(o.status),
                    stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
                }),
                stdout: o.stdout,
                stderr: o.stderr,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => GhOutput {
                err: Some(RunError::NotFound),
                ..Default::default()
            },
            Err(e) => GhOutput {
                err: Some(RunError::Other(e.to_string())),
                ..Default::default()
            },
        }
    }
}

/// An exit as Go's ExitError says it: "exit status 1", "signal: killed".
fn exit_words(s: std::process::ExitStatus) -> String {
    if let Some(code) = s.code() {
        return format!("exit status {code}");
    }
    match s.signal() {
        Some(libc::SIGKILL) => "signal: killed".into(),
        Some(libc::SIGINT) => "signal: interrupt".into(),
        Some(libc::SIGTERM) => "signal: terminated".into(),
        Some(n) => format!("signal: {n}"),
        None => "exit status -1".into(),
    }
}

thread_local! {
    static THREAD_GH: RefCell<Option<Arc<dyn Gh>>> = const { RefCell::new(None) };
}

/// Sets this thread's gh, over the real one; None goes back to it. For
/// tests, which run side by side in one process.
pub fn set_thread_gh(gh: Option<Arc<dyn Gh>>) {
    THREAD_GH.with(|g| *g.borrow_mut() = gh);
}

/// The gh this thread runs.
pub fn gh() -> Arc<dyn Gh> {
    THREAD_GH
        .with(|g| g.borrow().clone())
        .unwrap_or_else(|| Arc::new(SystemGh))
}

/// Runs gh with args, as strings.
pub fn run<S: AsRef<str>>(dir: Option<&Path>, args: &[S]) -> GhOutput {
    let args: Vec<String> = args.iter().map(|a| a.as_ref().to_string()).collect();
    gh().run(dir, &args)
}

/// Runner runs gh api with args and gives its output, headers first (-i):
/// Go's watch.Runner.
fn runner(args: &[&str]) -> GhOutput {
    let mut all = vec!["api", "-i"];
    all.extend_from_slice(args);
    run(None, &all)
}

/// Where watches keep what they share.
pub fn root() -> PathBuf {
    rush_state::dir().join("watch")
}

/// How asking GitHub went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// GitHub saying the rate limit is spent.
    Limited,
    /// Anything else, in a few words.
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Limited => f.write_str("GitHub's rate limit is spent"),
            Error::Other(s) => f.write_str(s),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(e.to_string())
    }
}

/// The rate limit GitHub last reported to this process, -1 before it has
/// said.
static LAST_REMAINING: AtomicI64 = AtomicI64::new(-1);

/// The rate limit GitHub last reported, -1 when unknown.
pub fn remaining() -> i64 {
    LAST_REMAINING.load(Ordering::SeqCst)
}

/// One gh api answer as kept for every watcher. Its fields are Go's, in
/// Go's order: the body base64, as Go writes a []byte.
#[derive(Serialize, Deserialize, Default)]
struct Cached {
    #[serde(default, deserialize_with = "crate::gh::nd")]
    etag: String,
    #[serde(default, with = "bytes64")]
    body: Option<Vec<u8>>,
    #[serde(default, deserialize_with = "time_or_zero")]
    at: Time,
    #[serde(default, deserialize_with = "crate::gh::nd")]
    remaining: i64,
}

/// A JSON null read as the type's zero, as Go reads it.
pub fn nd<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A time, null read as the zero time, as Go reads it.
pub(crate) fn time_or_zero<'de, D: Deserializer<'de>>(d: D) -> Result<Time, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.and_then(|s| Time::parse(&s)).unwrap_or_default())
}

mod bytes64 {
    use super::*;

    pub fn serialize<S: Serializer>(b: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match b {
            Some(b) => s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        match s {
            None => Ok(None),
            Some(s) => base64::engine::general_purpose::STANDARD
                .decode(s)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

/// Holds an exclusive flock on a file for as long as it lives.
struct Locked(std::fs::File);

impl Locked {
    fn take(path: &Path) -> Result<Locked, Error> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        loop {
            // SAFETY: f's descriptor is open for as long as f lives.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(Locked(f));
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: the descriptor is still open.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// path's body from GitHub. An answer any watcher had within fresh is used
/// as it is; an older one is asked again with its ETag, and kept when
/// GitHub says it hasn't changed. With it, the rate limit left, -1 when
/// unknown.
pub fn get(path: &str, fresh: Duration) -> Result<(Vec<u8>, i64), Error> {
    let dir = root().join("gh");
    mkdir_all(&dir)?;
    let sum = Sha256::digest(path.as_bytes());
    let file = dir.join(hex(&sum[..8]));
    // One asks while the rest wait for its answer.
    let _lock = Locked::take(&with_ext(&file, ".lock"))?;

    let mut c: Cached = std::fs::read(with_ext(&file, ".json"))
        .ok()
        .and_then(|b| rush_state::json::from_slice(&b).ok())
        .unwrap_or_default();
    if let Some(body) = &c.body
        && Time::now().sub_nanos(c.at) < fresh.as_nanos() as i128
    {
        return Ok((body.clone(), least(c.remaining)));
    }
    // GitHub said the limit is spent: nobody asks until it resets.
    if limited_until().is_some() {
        LAST_REMAINING.store(0, Ordering::SeqCst);
        return Err(Error::Limited);
    }
    let tag = format!("If-None-Match: {}", c.etag);
    let args: Vec<&str> = if !c.etag.is_empty() && c.body.is_some() {
        vec!["-H", &tag, path]
    } else {
        vec![path]
    };
    let out = runner(&args);
    let (status, h, b) = parse_response(&out.stdout);
    match status {
        304 if c.body.is_some() => {
            c.at = Time::now();
            if let Some(n) = h.get("x-ratelimit-remaining") {
                c.remaining = n.parse().unwrap_or(0);
            }
        }
        200 => {
            c = Cached {
                etag: h.get("etag").cloned().unwrap_or_default(),
                body: Some(b.to_vec()),
                at: Time::now(),
                remaining: -1,
            };
            if let Some(n) = h.get("x-ratelimit-remaining") {
                c.remaining = n.parse().unwrap_or(0);
            }
        }
        403 | 429 => {
            // The hourly limit spent, or a secondary limit (too many at
            // once), which says when to come back in Retry-After.
            if h.get("x-ratelimit-remaining").map(String::as_str) == Some("0")
                || status == 429
                || h.contains_key("retry-after")
                || contains(&b.to_ascii_lowercase(), b"rate limit")
            {
                LAST_REMAINING.store(0, Ordering::SeqCst);
                set_limited(reset_at(&h, Time::now()), &h);
                return Err(Error::Limited);
            }
            return Err(gh_error(status, b, &out.stderr, out.err.as_ref()));
        }
        _ => return Err(gh_error(status, b, &out.stderr, out.err.as_ref())),
    }
    if c.remaining >= 0 {
        LAST_REMAINING.store(c.remaining, Ordering::SeqCst);
    }
    note_budget(&h);
    if let Ok(enc) = rush_state::json::to_vec(&c) {
        let tmp = with_ext(&file, ".tmp");
        if write_private(&tmp, &enc).is_ok() {
            let _ = std::fs::rename(&tmp, with_ext(&file, ".json"));
        }
    }
    Ok((c.body.unwrap_or_default(), c.remaining))
}

/// The user's rate limit as GitHub last said it to any watcher on this
/// machine, kept in watch/gh/ratelimit.json so every watcher sees it: what
/// was left and when it resets (from each answer's X-RateLimit-Remaining
/// and X-RateLimit-Reset), and until when nobody asks, once GitHub said
/// it's spent or to slow down.
#[derive(Serialize, Deserialize, Default)]
struct Budget {
    #[serde(default, deserialize_with = "time_or_zero")]
    until: Time,
    #[serde(default)]
    remaining: Option<i64>,
    #[serde(default, deserialize_with = "time_or_zero")]
    reset: Time,
}

fn budget_file() -> PathBuf {
    root().join("gh").join("ratelimit.json")
}

fn read_budget() -> Budget {
    std::fs::read(budget_file())
        .ok()
        .and_then(|b| rush_state::json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_budget(b: &Budget) {
    let file = budget_file();
    if let Some(d) = file.parent() {
        let _ = mkdir_all(d);
    }
    if let Ok(enc) = rush_state::json::to_vec(b) {
        // A file of this process's own, then renamed: watchers in other
        // processes write it too.
        let tmp = with_ext(&file, &format!(".{}.tmp", std::process::id()));
        if write_private(&tmp, &enc).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }
}

/// When GitHub's rate limit resets, if a watcher was told it is spent and
/// it hasn't reset yet.
pub fn limited_until() -> Option<Time> {
    let b = read_budget();
    (b.until > Time::now()).then_some(b.until)
}

/// What's left of the rate limit as GitHub last told any watcher, -1 when
/// unknown or reset since.
pub fn shared_remaining() -> i64 {
    let b = read_budget();
    match b.remaining {
        Some(r) if b.reset > Time::now() => r,
        _ => -1,
    }
}

/// The lower of what one answer said was left and what the last answer to
/// any watcher said, so every watcher slows down together.
fn least(own: i64) -> i64 {
    let shared = shared_remaining();
    if shared >= 0 && (own < 0 || shared < own) {
        shared
    } else {
        own
    }
}

/// Keeps what an answer's headers said of the rate limit for every watcher.
fn note_budget(h: &HashMap<String, String>) {
    let Some(left) = h
        .get("x-ratelimit-remaining")
        .and_then(|v| v.trim().parse::<i64>().ok())
    else {
        return;
    };
    let mut b = read_budget();
    b.remaining = Some(left);
    if let Some(r) = h
        .get("x-ratelimit-reset")
        .and_then(|v| v.trim().parse::<i64>().ok())
    {
        b.reset = Time::from_unix(r, 0);
    }
    if left == 0 {
        // That was the last: the next waits for the reset.
        b.until = reset_at(h, Time::now());
    }
    write_budget(&b);
}

/// Holds every watcher off until then.
fn set_limited(until: Time, h: &HashMap<String, String>) {
    let mut b = read_budget();
    b.until = until;
    if h.get("x-ratelimit-remaining").map(String::as_str) == Some("0") {
        b.remaining = Some(0);
        b.reset = until;
    }
    write_budget(&b);
}

/// When to ask again after GitHub turned a call back: Retry-After's
/// seconds, else the limit's reset, else a minute (GitHub's advice for a
/// secondary limit that says neither); never more than an hour away.
pub fn reset_at(h: &HashMap<String, String>, now: Time) -> Time {
    let most = now + Duration::from_secs(3600);
    if let Some(s) = h
        .get("retry-after")
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        return (now + Duration::from_secs(s.max(1))).min(most);
    }
    if let Some(r) = h
        .get("x-ratelimit-reset")
        .and_then(|v| v.trim().parse::<i64>().ok())
    {
        let t = Time::from_unix(r, 0);
        if t > now {
            return t.min(most);
        }
    }
    now + Duration::from_secs(60)
}

/// path's body from GitHub, unshared and uncached: a job's log.
pub fn raw(path: &str) -> Result<Vec<u8>, Error> {
    // A log is coloured, which gh won't print unless asked; an older gh
    // doesn't know the flag, and prints it anyway.
    let mut out = runner(&["--allow-escape-sequences", path]);
    if contains(&out.stderr, b"unknown flag") {
        out = runner(&[path]);
    }
    let (status, _, b) = parse_response(&out.stdout);
    if status != 200 {
        return Err(gh_error(status, b, &out.stderr, out.err.as_ref()));
    }
    Ok(b.to_vec())
}

/// Splits gh api -i's output into its status, its headers (lowercased; the
/// last response's, past any redirect) and its body.
pub fn parse_response(mut out: &[u8]) -> (u16, HashMap<String, String>, &[u8]) {
    let mut h = HashMap::new();
    let mut status = 0;
    while out.starts_with(b"HTTP/") {
        let (head, body) = match cut(out, b"\r\n\r\n") {
            Some(p) => p,
            None => cut(out, b"\n\n").unwrap_or((out, &[])),
        };
        h.clear();
        let text = String::from_utf8_lossy(head);
        for (i, line) in text.split('\n').enumerate() {
            let line = line.trim_end_matches('\r');
            if i == 0 {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() > 1 {
                    status = f[1].parse().unwrap_or(0);
                }
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                h.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        }
        out = body;
    }
    (status, h, out)
}

/// Why a gh api call failed, in a few words.
pub fn gh_error(status: u16, body: &[u8], stderr: &[u8], err: Option<&RunError>) -> Error {
    if matches!(err, Some(RunError::NotFound)) {
        return Error::Other("gh isn't installed".into());
    }
    #[derive(Deserialize)]
    struct M {
        #[serde(default, deserialize_with = "crate::gh::nd")]
        message: String,
    }
    if let Ok(m) = rush_state::json::from_slice::<M>(body)
        && !m.message.is_empty()
    {
        return Error::Other(format!("GitHub says: {} (HTTP {status})", m.message));
    }
    let msg = String::from_utf8_lossy(stderr);
    let msg = msg.trim();
    if msg.contains("gh auth login") || msg.contains("authentication") {
        return Error::Other("gh isn't signed in · gh auth login".into());
    }
    if !msg.is_empty() {
        let first = match msg.find('\n') {
            Some(i) if i > 0 => &msg[..i],
            _ => msg,
        };
        return Error::Other(first.to_string());
    }
    if let Some(e) = err {
        return Error::Other(e.to_string());
    }
    Error::Other(format!("GitHub answered HTTP {status}"))
}

fn cut<'a>(b: &'a [u8], sep: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let i = b.windows(sep.len()).position(|w| w == sep)?;
    Some((&b[..i], &b[i + sep.len()..]))
}

pub(crate) fn contains(b: &[u8], what: &[u8]) -> bool {
    b.windows(what.len()).any(|w| w == what)
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn with_ext(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(ext);
    PathBuf::from(s)
}

/// Writes b to path, readable only by its owner, as Go's
/// os.WriteFile(…, 0o600).
pub fn write_private(path: &Path, b: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(b)
}

/// Makes dir and those above it, readable only by their owner, as Go's
/// os.MkdirAll(…, 0o700).
pub fn mkdir_all(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_response_takes_the_last_after_a_redirect() {
        let out = b"HTTP/2.0 302 Found\r\nLocation: x\r\n\r\nHTTP/2.0 200 OK\r\nEtag: W/\"e\"\r\nX-Ratelimit-Remaining: 42\r\n\r\n{\"a\":1}";
        let (status, h, body) = parse_response(out);
        assert_eq!(status, 200);
        assert_eq!(h["etag"], "W/\"e\"");
        assert_eq!(h["x-ratelimit-remaining"], "42");
        assert_eq!(body, b"{\"a\":1}");
    }
}
