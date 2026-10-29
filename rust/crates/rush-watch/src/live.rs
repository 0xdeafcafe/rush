//! A running watch as it last stood, kept by the background task it runs
//! as, so rush can show what it's waiting on and leave it out of long-task
//! check-ins: it ends itself, at its timeout at the latest.
//! Ported from internal/watch/live.go and stdout_darwin.go /
//! stdout_other.go.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use rush_state::Time;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::gh::{mkdir_all, time_or_zero, write_private};
use crate::pr::{Options, PR, Until};

/// A running watch, as its task keeps it. Its fields are Go's, in Go's
/// order, so either rush reads what the other wrote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Live {
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub pid: i64,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub number: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, with = "until_words")]
    pub until: Until,
    /// Zero: never. In nanoseconds on disk, as Go writes a time.Duration.
    #[serde(default, with = "nanos")]
    pub timeout: Duration,
    #[serde(default, deserialize_with = "time_or_zero")]
    pub start: Time,
    /// When GitHub last answered, zero before it has.
    #[serde(
        default,
        deserialize_with = "time_or_zero",
        skip_serializing_if = "is_zero_time"
    )]
    pub asked: Time,
    /// When the watch asks again.
    #[serde(
        default,
        deserialize_with = "time_or_zero",
        skip_serializing_if = "is_zero_time"
    )]
    pub next: Time,
    #[serde(default, skip_serializing_if = "is_false")]
    pub limited: bool,
    #[serde(default)]
    pub passed: usize,
    #[serde(default)]
    pub failed: usize,
    #[serde(default)]
    pub pending: usize,
    #[serde(default)]
    pub skipped: usize,
    /// The checks that failed.
    #[serde(
        default,
        rename = "failedChecks",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::gh::nd"
    )]
    pub failed_checks: Vec<String>,
    /// The checks still running.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::gh::nd"
    )]
    pub running: Vec<String>,
}

impl Live {
    /// Whether a failure already seen ends the watch: until fail and first
    /// say one at once, done and each only once all have finished.
    pub fn reports(&self) -> bool {
        self.until == Until::Fail || self.until == Until::First
    }
}

fn is_zero_time(t: &Time) -> bool {
    t.is_zero()
}

fn is_false(b: &bool) -> bool {
    !*b
}

mod until_words {
    use super::*;

    pub fn serialize<S: Serializer>(u: &Until, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(u.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Until, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        Ok(s.and_then(|s| Until::parse(&s)).unwrap_or_default())
    }
}

mod nanos {
    use super::*;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let n: Option<i64> = Option::deserialize(d)?;
        Ok(Duration::from_nanos(n.unwrap_or(0).max(0) as u64))
    }
}

fn live_dir() -> PathBuf {
    crate::gh::root().join("live")
}

/// A background task's output file, as Claude Code names it:
/// <tasks folder>/<task id>.output.
static TASK_OUTPUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/tasks/([A-Za-z0-9_-]+)\.output$").unwrap());

/// The background task this process runs as, from where its stdout goes;
/// "" when it isn't one.
pub fn task_id() -> String {
    task_of(&stdout_path())
}

/// The task a process whose stdout goes to path runs as.
pub fn task_of(path: &str) -> String {
    TASK_OUTPUT
        .captures(path)
        .map(|m| m[1].to_string())
        .unwrap_or_default()
}

/// The file stdout writes to, as fcntl's F_GETPATH says.
#[cfg(target_os = "macos")]
fn stdout_path() -> String {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN bytes into buf.
    if unsafe { libc::fcntl(1, libc::F_GETPATH, buf.as_mut_ptr()) } == -1 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// The file stdout writes to, as /proc says.
#[cfg(not(target_os = "macos"))]
fn stdout_path() -> String {
    std::fs::read_link("/proc/self/fd/1")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Keeps l for its task, replacing what was there whole.
pub fn save_live(l: &Live) {
    if l.task.is_empty() || mkdir_all(&live_dir()).is_err() {
        return;
    }
    let Ok(b) = rush_state::json::to_vec(l) else {
        return;
    };
    let file = live_dir().join(format!("{}.json", l.task));
    let tmp = live_dir().join(format!("{}.json.tmp", l.task));
    if write_private(&tmp, &b).is_ok() {
        let _ = std::fs::rename(&tmp, &file);
    }
}

pub(crate) fn drop_live(task: &str) {
    if !task.is_empty() {
        let _ = std::fs::remove_file(live_dir().join(format!("{task}.json")));
    }
}

/// How long past its timeout a watch's record is believed: past it, the
/// watch was killed before it could take it away.
const LIVE_GRACE: Duration = Duration::from_secs(5 * 60);

/// The running watch task runs as, if it is one.
pub fn live_for(task: &str, now: Time) -> Option<Live> {
    if task.is_empty() || !valid_task(task) {
        return None;
    }
    let b = std::fs::read(live_dir().join(format!("{task}.json"))).ok()?;
    let l: Live = rush_state::json::from_slice(&b).ok()?;
    if l.task != task {
        return None;
    }
    if !l.timeout.is_zero() && now.sub_nanos(l.start) > (l.timeout + LIVE_GRACE).as_nanos() as i128
    {
        return None;
    }
    if l.pid > 0 {
        // SAFETY: signal 0 only asks whether the process is there.
        let r = unsafe { libc::kill(l.pid as libc::pid_t, 0) };
        if r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return None;
        }
    }
    Some(l)
}

fn valid_task(task: &str) -> bool {
    task.chars()
        .all(|r| r.is_ascii_alphanumeric() || r == '_' || r == '-')
}

/// p as the watch with o stands now, about to ask again at next.
pub(crate) fn live(
    o: &Options,
    task: &str,
    start: Time,
    p: Option<&PR>,
    asked: Time,
    next: Time,
    limited: bool,
) -> Live {
    let mut l = Live {
        task: task.to_string(),
        pid: std::process::id() as i64,
        repo: o.repo.clone(),
        number: o.number,
        until: o.until,
        timeout: o.timeout,
        start,
        asked,
        next,
        limited,
        ..Default::default()
    };
    let Some(p) = p else {
        return l;
    };
    l.title = p.title.clone();
    (l.passed, l.failed, l.pending, l.skipped) = p.counts();
    for c in &p.checks {
        match c.state.as_str() {
            "failed" => l.failed_checks.push(c.name.clone()),
            "pending" => l.running.push(c.name.clone()),
            _ => {}
        }
    }
    l
}
