//! A running watch as it last stood, kept by the background task it runs
//! as, so rush can show what it's waiting on. The record's file is the
//! watch module's; this reads it as the card needs.
//! Ported from internal/watch/live.go:14-120 (Live, LiveFor, saveLive)
//! and pr.go:155-163 (Until).

use std::path::PathBuf;

use rush_state::Time;
use serde::{Deserialize, Serialize};

/// When a watch ends.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Until(pub String);

impl Until {
    /// every check has finished, or the pull request merged or closed
    pub const DONE: &str = "done";
    /// a check failed, else as done
    pub const FAIL: &str = "fail";
    /// the first check to finish, or anything else that changes
    pub const FIRST: &str = "first";
    /// as done, saying each check as it finishes
    pub const EACH: &str = "each";
}

/// A running watch as it last stood.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Live {
    pub task: String,
    pub pid: i32,
    pub repo: String,
    pub number: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub until: Until,
    /// nanoseconds, as Go writes a time.Duration
    pub timeout: i64,
    pub start: Time,
    /// When GitHub last answered, zero before it has.
    #[serde(skip_serializing_if = "zero")]
    pub asked: Time,
    /// When the watch asks again.
    #[serde(skip_serializing_if = "zero")]
    pub next: Time,
    #[serde(skip_serializing_if = "is_false")]
    pub limited: bool,
    pub passed: i64,
    pub failed: i64,
    pub pending: i64,
    pub skipped: i64,
    /// The checks failed, and still running, by name.
    #[serde(rename = "failedChecks", skip_serializing_if = "Vec::is_empty")]
    pub failed_checks: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<String>,
}

fn zero(t: &Time) -> bool {
    t.is_zero()
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Live {
    /// Whether a failure already seen ends the watch: until fail and first
    /// say one at once, done and each only once all have finished.
    pub fn reports(&self) -> bool {
        self.until.0 == Until::FAIL || self.until.0 == Until::FIRST
    }
}

/// Where the records are.
pub fn live_dir() -> PathBuf {
    rush_state::dir().join("watch").join("live")
}

/// How long past its timeout a watch's record is believed: past it, the
/// watch was killed before it could take it away.
const LIVE_GRACE_NS: i128 = 5 * 60 * 1_000_000_000;

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
    if l.timeout > 0 && now.sub_nanos(l.start) > l.timeout as i128 + LIVE_GRACE_NS {
        return None;
    }
    // SAFETY: kill with signal 0 only asks whether the process is there.
    if l.pid > 0
        && unsafe { libc::kill(l.pid, 0) } != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    {
        return None;
    }
    Some(l)
}

/// Keeps l for its task, replacing what was there whole (the watch's
/// saveLive).
pub fn save_live(l: &Live) {
    if l.task.is_empty() || std::fs::create_dir_all(live_dir()).is_err() {
        return;
    }
    let Ok(b) = rush_state::json::to_vec(l) else {
        return;
    };
    let file = live_dir().join(format!("{}.json", l.task));
    let tmp = live_dir().join(format!("{}.json.tmp", l.task));
    if std::fs::write(&tmp, b).is_ok() {
        let _ = std::fs::rename(&tmp, &file);
    }
}

fn valid_task(task: &str) -> bool {
    task.chars()
        .all(|r| r.is_ascii_alphanumeric() || r == '_' || r == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Home;

    // live_test.go TestALiveRecordIsntBelievedPastItsTimeoutOrItsProcess
    #[test]
    fn a_live_record_isnt_believed_past_its_timeout_or_its_process() {
        let _h = Home::new();
        let start = Time::from_unix(1000, 0);
        let min = |n: i64| n * 60 * 1_000_000_000;
        save_live(&Live {
            task: "a".into(),
            pid: std::process::id() as i32,
            start,
            timeout: min(10),
            ..Default::default()
        });
        assert!(
            live_for("a", start.add_nanos(min(14))).is_some(),
            "not believed within its grace"
        );
        assert!(
            live_for("a", start.add_nanos(min(16))).is_none(),
            "believed past its timeout and grace"
        );
        let mut c = std::process::Command::new("true").spawn().unwrap();
        let pid = c.id() as i32;
        c.wait().unwrap();
        save_live(&Live {
            task: "b".into(),
            pid,
            start,
            ..Default::default()
        });
        assert!(
            live_for("b", start).is_none(),
            "believed with its process gone"
        );
        assert!(live_for("../a", start).is_none());
        let b = std::fs::read_to_string(live_dir().join("a.json")).unwrap();
        assert!(
            b.starts_with(r#"{"task":"a","pid":"#)
                && b.contains(
                    r#""until":"","timeout":600000000000,"start":"1970-01-01T00:16:40Z","passed":0"#
                ),
            "{b}"
        );
    }
}
