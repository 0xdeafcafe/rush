//! What each agent's temp work measured last, kept across restarts in the
//! cache's temp.json (the same file Go rush keeps), so a folder is walked
//! again only when its agent has done something since. Go:
//! internal/fleet/temp.go:178-300 (TempSize, every, TempSizes, Load, Due,
//! Set, Save) and internal/ui/tempwork.go:28 (measureTemp's due batch).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use rush_core::agent::Agent;
use rush_core::time::Ms;
use serde::{Deserialize, Serialize};

/// Walks take at most 1/TEMP_SHARE of a core.
const TEMP_SHARE: u32 = 50;

/// An agent's temp work as last measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TempSize {
    #[serde(default)]
    pub bytes: i64,
    #[serde(default, with = "rush_core::wire::time")]
    pub at: Ms,
    /// How long measuring it took: a folder of millions of files takes a
    /// core for most of a minute to walk.
    #[serde(
        default,
        with = "rush_core::wire::duration",
        skip_serializing_if = "Duration::is_zero"
    )]
    pub took: Duration,
}

impl TempSize {
    /// How long a running agent's temp work goes before it's measured
    /// again: ten minutes, longer for one slow to walk, at most two hours.
    fn every(&self) -> Ms {
        let d = (self.took * TEMP_SHARE).clamp(Duration::from_secs(600), Duration::from_secs(7200));
        d.as_millis() as Ms
    }
}

/// The measurements, by agent key.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct TempSizes {
    #[serde(default)]
    pub sizes: HashMap<String, TempSize>,
    #[serde(skip)]
    pub dirty: bool,
    /// Read from disk yet: until then it isn't written, or it would write
    /// over what's there.
    #[serde(skip)]
    pub loaded: bool,
}

/// Where they're kept.
pub(crate) fn temp_path() -> PathBuf {
    rush_state::dir::cache_path("temp.json")
}

impl TempSizes {
    /// What an agent's temp work measured last.
    pub fn bytes(&self, key: &str) -> Option<i64> {
        self.sizes.get(key).map(|s| s.bytes)
    }

    /// Reads the last measurements from path, under any set since. Off the
    /// UI thread.
    pub fn load(&mut self, path: &std::path::Path) {
        if self.loaded {
            return;
        }
        let disk: TempSizes = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for (k, v) in disk.sizes {
            self.sizes.entry(k).or_insert(v);
        }
        self.loaded = true;
    }

    /// Whether an agent's temp work should be measured: never measured,
    /// running and not measured for a while, or stopped and busy since.
    pub fn due(&self, a: &Agent, now: Ms) -> bool {
        match self.sizes.get(&a.key) {
            None => true,
            Some(e) if a.pid != 0 => now - e.at > e.every(),
            Some(e) => a.job.updated_at > e.at,
        }
    }

    /// Records measurements.
    pub fn set(&mut self, m: HashMap<String, TempSize>) {
        if m.is_empty() {
            return;
        }
        self.sizes.extend(m);
        self.dirty = true;
    }

    /// Writes the measurements to path if they changed. Off the UI thread.
    pub fn save(&mut self, path: &std::path::Path) {
        if !self.dirty || !self.loaded {
            return;
        }
        if let Ok(b) = serde_json::to_vec(self) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(key: &str, pid: i32, updated: Ms) -> Agent {
        let mut a = Agent {
            key: key.into(),
            pid,
            ..Default::default()
        };
        a.job.updated_at = updated;
        a
    }

    // Go: fleet/temp_test.go (TestTempDue).
    #[test]
    fn what_is_due() {
        let now = 10 * 3_600_000;
        let mut t = TempSizes::default();
        t.sizes.insert(
            "fresh".into(),
            TempSize {
                at: now - 60_000,
                ..Default::default()
            },
        );
        t.sizes.insert(
            "old".into(),
            TempSize {
                at: now - 3_600_000,
                ..Default::default()
            },
        );
        t.sizes.insert(
            "slow".into(),
            TempSize {
                at: now - 3_600_000,
                took: Duration::from_secs(120),
                ..Default::default()
            },
        );
        for (a, want) in [
            (agent("never", 0, 0), true),
            (agent("fresh", 1, now), false),
            (agent("old", 1, now), true),
            // 120s × 50 is past two hours: two hours, so an hour isn't due.
            (agent("slow", 1, now), false),
            (agent("old", 0, now - 7_200_000), false),
            (agent("old", 0, now - 60_000), true),
        ] {
            assert_eq!(t.due(&a, now), want, "{} pid {}", a.key, a.pid);
        }
    }

    #[test]
    fn reads_and_writes_what_go_writes() {
        let dir = std::env::temp_dir().join(format!("rush-temp-sizes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("temp.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"{"sizes":{"default/a:1":{"bytes":74412032,"at":"2026-10-01T14:17:18.002735+02:00","took":149606583},"default/a:2":{"bytes":0,"at":"2026-10-12T10:21:52Z"}}}"#,
        )
        .unwrap();
        let mut t = TempSizes::default();
        t.set(HashMap::from([(
            "default/a:2".to_string(),
            TempSize {
                bytes: 5,
                ..Default::default()
            },
        )]));
        // Unread, it isn't written.
        t.save(&path);
        t.load(&path);
        assert_eq!(t.bytes("default/a:1"), Some(74412032));
        assert_eq!(t.sizes["default/a:1"].took, Duration::from_nanos(149606583));
        assert_eq!(
            t.sizes["default/a:1"].at,
            rush_core::time::parse_rfc3339("2026-10-01T12:17:18.002Z")
        );
        // What was set before the read stays over what's on disk.
        assert_eq!(t.bytes("default/a:2"), Some(5));
        t.save(&path);
        let mut back = TempSizes::default();
        back.load(&path);
        assert_eq!(back.sizes, t.sizes);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
