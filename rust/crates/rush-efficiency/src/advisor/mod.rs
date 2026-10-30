//! The advisor watches how you use your agents and says what would cut
//! tokens or time. It works in two passes: a cheap model (Haiku) reads a
//! digest rush works out itself and proposes candidates, and now and then
//! an expensive one (Opus) checks the ones worth money against the
//! transcripts before they reach you. It's opt-in: nothing runs until you
//! turn it on. Port of internal/advisor/advisor.go; digest.go, brief.go
//! and run.go are its modules of the same names.

pub mod brief;
pub mod digest;
pub mod run;
mod sha1;

use std::path::PathBuf;
use std::time::Duration;

use rush_core::adapter::Profile;
use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::source::transcripts;
use crate::when::{DAY, HOUR, since};

pub use brief::briefs;
pub use digest::{Input, digest};
pub use run::{Ask, Cli, Result as PassResult, pass, pass_with, stop};

/// The least time between passes, in seconds.
pub const GAP: i64 = 3 * HOUR;
/// How many new requests make a pass worth running.
pub const MIN_REQUESTS: i64 = 150;
/// The dollars a week a candidate must be worth for Opus to check it: a
/// review costs cents to a dollar or so.
pub const REVIEW_AT: f64 = 2.0;
/// How many reviews may fail on one candidate before it's left alone, so
/// one that times out can't take every day's reviews.
pub const TRIES: i64 = 2;
/// The Opus reviews in any 24 hours.
pub const REVIEWS_PER_DAY: i64 = 3;
/// How many findings are kept, rejected and put away ones included:
/// they're what stops a finding being proposed again.
pub const KEEP: usize = 24;
/// When a pass's lock is taken to be left by a process that died: longer
/// than a pass can run.
const LOCK_STALE: Duration = Duration::from_secs(45 * 60);

/// Haiku's, not checked.
pub const CANDIDATE: &str = "candidate";
/// Opus checked it and it holds.
pub const CONFIRMED: &str = "confirmed";
/// Opus checked it and it doesn't; kept so it isn't proposed again.
pub const REJECTED: &str = "rejected";

fn zero_f(n: &f64) -> bool {
    *n == 0.0
}
fn zero_i(n: &i64) -> bool {
    *n == 0
}
fn zero_t(t: &Time) -> bool {
    t.is_zero()
}

/// Something the advisor noticed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Finding {
    pub id: String,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// Dollars a week at stake, roughly.
    #[serde(skip_serializing_if = "zero_f")]
    pub weekly: f64,
    /// A saver's ID.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fix: String,
    /// A file to change.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub open: String,
    pub status: String,
    /// The reviewer's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Reviews that failed.
    #[serde(skip_serializing_if = "zero_i")]
    pub tries: i64,
    pub at: Time,
}

/// What the advisor keeps between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    #[serde(rename = "lastRun", skip_serializing_if = "zero_t")]
    pub last_run: Time,
    #[serde(skip_serializing_if = "zero_i")]
    pub runs: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<Time>,
    /// What the advisor itself has cost.
    #[serde(skip_serializing_if = "zero_f")]
    pub spent: f64,
    /// Why the last pass failed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub err: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    /// Findings you put away: never shown or proposed again.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dismissed: Vec<String>,
}

/// Where the advisor keeps its record, and where its passes run.
pub fn dir() -> PathBuf {
    rush_state::dir().join("advisor")
}

fn record_path() -> PathBuf {
    dir().join("record.json")
}

/// The record; a missing one is empty. One that can't be read is kept
/// aside, and the record starts as if a pass had just run, so a broken
/// file can't let one run at once or reset the day's reviews.
pub fn load() -> Record {
    let Ok(b) = std::fs::read(record_path()) else {
        return Record::default();
    };
    match serde_json::from_slice(&b) {
        Ok(r) => r,
        Err(_) => {
            let mut bad = record_path().into_os_string();
            bad.push(".bad");
            let _ = std::fs::rename(record_path(), bad);
            Record {
                last_run: Time::now(),
                err: "its record couldn't be read, and is kept as record.json.bad".into(),
                ..Record::default()
            }
        }
    }
}

/// Whether the advisor is on in the config on disk: another rush may have
/// turned it off. RUSH_ADVISOR decides it for this run.
pub fn enabled() -> bool {
    if let Some(on) = rush_state::envconfig::env_bool("advisor") {
        return on;
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct C {
        advisor: bool,
    }
    std::fs::read(rush_state::dir().join("config.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<C>(&b).ok())
        .is_some_and(|c| c.advisor)
}

/// Marks a pass as started, before it spends anything: if rush quits
/// halfway, the next one still waits out the gap.
pub fn begin(now: Time) -> std::io::Result<()> {
    let mut r = load();
    r.last_run = now;
    r.save()
}

/// Counts a review against the day's cap before it runs, for the same
/// reason.
pub fn reserve(now: Time) -> std::io::Result<()> {
    let mut r = load();
    r.reviews.push(now);
    r.save()
}

impl Record {
    /// Writes the record.
    pub fn save(&self) -> std::io::Result<()> {
        let b = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        // A temp file of its own, so two rushes saving at once can't write
        // into each other's.
        crate::write_file(&record_path(), &b)
    }

    /// Whether a pass should run now, given how many requests your agents
    /// have made since the last one.
    pub fn due(&self, now: Time, new_reqs: i64) -> bool {
        if self.last_run.is_zero() {
            return new_reqs > 0;
        }
        since(now, self.last_run) >= GAP && new_reqs >= MIN_REQUESTS
    }

    /// How many Opus reviews the day's cap still allows.
    pub fn reviews_left(&self, now: Time) -> i64 {
        let n = self
            .reviews
            .iter()
            .filter(|&&t| now.sub_nanos(t) < DAY as i128 * 1_000_000_000)
            .count() as i64;
        (REVIEWS_PER_DAY - n).max(0)
    }

    /// The findings to show: confirmed first, then candidates, most at
    /// stake first within each.
    pub fn shown(&self) -> Vec<Finding> {
        let mut out: Vec<Finding> = self
            .findings
            .iter()
            .filter(|f| f.status != REJECTED && !self.dismissed.contains(&f.id))
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            let (ca, cb) = (a.status == CONFIRMED, b.status == CONFIRMED);
            cb.cmp(&ca).then(b.weekly.total_cmp(&a.weekly))
        });
        out
    }

    /// The candidates still waiting on a review, not put away, and not
    /// failed too often.
    pub fn pending(&self) -> Vec<Finding> {
        self.findings
            .iter()
            .filter(|f| f.status == CANDIDATE && f.tries < TRIES && !self.dismissed.contains(&f.id))
            .cloned()
            .collect()
    }

    /// The findings Opus has decided on, or you put away: a pass doesn't
    /// propose or check them again.
    pub fn settled(&self) -> Vec<String> {
        let mut out = self.dismissed.clone();
        out.extend(
            self.findings
                .iter()
                .filter(|f| f.status != CANDIDATE)
                .map(|f| f.id.clone()),
        );
        out
    }

    /// The titles a pass mustn't propose again: everything kept, rejected
    /// and dismissed included.
    pub fn known(&self) -> Vec<String> {
        self.findings.iter().map(|f| f.title.clone()).collect()
    }

    /// Puts a finding away for good.
    pub fn dismiss(&mut self, id: &str) {
        if !self.dismissed.iter().any(|d| d == id) {
            self.dismissed.push(id.to_string());
        }
    }

    /// Takes a pass's result in, and reports the findings that are newly
    /// confirmed. Its reviews were counted as they started (reserve).
    pub fn merge(&mut self, res: PassResult) -> Vec<Finding> {
        self.last_run = res.at;
        self.runs += 1;
        self.spent += res.spent;
        self.err = res.err.clone().unwrap_or_default();
        let mut fresh = Vec::new();
        for f in res.findings {
            match self.findings.iter().position(|o| o.id == f.id) {
                None => self.findings.push(f.clone()),
                Some(i) if self.findings[i].status == CANDIDATE => self.findings[i] = f.clone(),
                Some(_) => continue, // Opus has settled it
            }
            if f.status == CONFIRMED && !self.dismissed.contains(&f.id) {
                fresh.push(f);
            }
        }
        // Over KEEP, the oldest go: rejected and put away ones first, then
        // guesses, then what Opus confirmed.
        if self.findings.len() > KEEP {
            let dismissed = self.dismissed.clone();
            let rank = |f: &Finding| {
                if f.status == REJECTED || dismissed.contains(&f.id) {
                    0
                } else if f.status == CANDIDATE {
                    1
                } else {
                    2
                }
            };
            self.findings
                .sort_by(|a, b| rank(b).cmp(&rank(a)).then(b.at.cmp(&a.at)));
            self.findings.truncate(KEEP);
        }
        let cut = crate::when::add_secs(Time::now(), -DAY);
        self.reviews.retain(|t| *t >= cut);
        fresh
    }
}

/// Claims the next pass for this process, so two rushes open at once
/// don't both run one (and both spend the day's reviews). None when
/// another holds it; the Lock lets it go when dropped.
pub fn lock() -> Option<Lock> {
    std::fs::create_dir_all(dir()).ok()?;
    let p = dir().join("pass.lock");
    let open = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
    };
    let f = match open() {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let stale = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|d| d > LOCK_STALE));
            if !stale {
                return None;
            }
            let _ = std::fs::remove_file(&p);
            open().ok()?
        }
        r => r.ok()?,
    };
    use std::io::Write as _;
    let mut f = f;
    let _ = writeln!(f, "{}", std::process::id());
    Some(Lock(p))
}

/// The pass's claim: let go when dropped.
pub struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Whether any of p's transcripts changed since t: a look at file times,
/// far cheaper than reading the figures, so a quiet day costs nothing.
pub fn active(p: &Profile, t: Time) -> bool {
    transcripts(p).iter().any(|path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|m| {
                let d = m.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                Time::from_unix(d.as_secs() as i64, d.subsec_nanos()) > t
            })
    })
}

/// A finding named by its title, so the same one proposed twice is one.
pub fn id_of(title: &str) -> String {
    let s = title
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let h = sha1::digest(s.as_bytes());
    h[..6].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::when::add_secs;

    fn home() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().to_path_buf()));
        d
    }

    #[test]
    fn due() {
        let now = Time::now();
        let mut r = Record::default();
        assert!(!r.due(now, 0), "a first pass with nothing to look at");
        assert!(r.due(now, 1), "no first pass once there's work");
        r.last_run = add_secs(now, -HOUR);
        assert!(!r.due(now, 10_000), "a pass inside the gap");
        r.last_run = add_secs(now, -GAP);
        assert!(
            !r.due(now, MIN_REQUESTS - 1),
            "a pass with too little new work"
        );
        assert!(r.due(now, MIN_REQUESTS));
    }

    #[test]
    fn reviews_left() {
        let now = Time::now();
        let r = Record {
            reviews: vec![add_secs(now, -25 * HOUR), add_secs(now, -HOUR), now],
            ..Record::default()
        };
        assert_eq!(r.reviews_left(now), REVIEWS_PER_DAY - 2);
    }

    #[test]
    fn ids_are_go_s() {
        // sha1("trim claude.md")[:6], as Go's idOf writes it.
        assert_eq!(id_of("Trim  CLAUDE.md"), id_of("trim claude.md"));
        assert_eq!(id_of("abc"), "a9993e364706");
    }

    #[test]
    fn merges() {
        let mut r = Record::default();
        let at = Time::now();
        let f = |t: &str, s: &str, w: f64| Finding {
            id: id_of(t),
            title: t.into(),
            status: s.into(),
            weekly: w,
            at,
            ..Finding::default()
        };
        let (a, b, c) = (
            f("Trim CLAUDE.md", CANDIDATE, 0.0),
            f("Use sonnet for subagents", CONFIRMED, 4.0),
            f("Read less", REJECTED, 0.0),
        );
        let fresh = r.merge(PassResult {
            at,
            findings: vec![a, b.clone(), c],
            spent: 0.2,
            ..PassResult::default()
        });
        assert!(fresh.len() == 1 && fresh[0].id == b.id);
        let shown = r.shown();
        assert!(shown.len() == 2 && shown[0].id == b.id);
        let mut again = b.clone();
        again.title = "use Sonnet for subagents".into();
        again.status = CANDIDATE.into();
        again.id = id_of(&again.title);
        let fresh = r.merge(PassResult {
            at,
            findings: vec![again],
            ..PassResult::default()
        });
        assert!(fresh.is_empty(), "a confirmed finding came back as news");
        assert_eq!(r.shown()[0].status, CONFIRMED);
        r.dismiss(&b.id);
        assert!(r.shown().iter().all(|f| f.id != b.id));
        assert_eq!(r.known().len(), 3, "rejected and dismissed ones stay known");
        assert!(r.spent == 0.2 && r.runs == 2);
    }

    #[test]
    fn settled_and_eviction() {
        let mut r = Record::default();
        let old = add_secs(Time::now(), -HOUR);
        let mut fs: Vec<Finding> = (0..KEEP)
            .map(|i| Finding {
                id: format!("c{i}"),
                title: format!("confirmed {i}"),
                status: CONFIRMED.into(),
                at: old,
                ..Finding::default()
            })
            .collect();
        fs.push(Finding {
            id: "r".into(),
            title: "rejected".into(),
            status: REJECTED.into(),
            at: Time::now(),
            ..Finding::default()
        });
        r.merge(PassResult {
            at: Time::now(),
            findings: fs,
            ..PassResult::default()
        });
        assert!(r.findings.len() == KEEP && r.findings.iter().all(|f| f.id != "r"));
        r.findings.push(Finding {
            id: "x".into(),
            status: REJECTED.into(),
            ..Finding::default()
        });
        r.dismiss("d");
        let s = r.settled();
        assert!(s.contains(&"x".into()) && s.contains(&"d".into()) && s.contains(&"c0".into()));
        r.merge(PassResult {
            at: Time::now(),
            findings: vec![Finding {
                id: "x".into(),
                status: CANDIDATE.into(),
                ..Finding::default()
            }],
            ..PassResult::default()
        });
        let x = r.findings.iter().find(|f| f.id == "x");
        assert!(
            x.is_none_or(|f| f.status == REJECTED),
            "a candidate reopened what Opus rejected"
        );
    }

    #[test]
    fn locks() {
        let _h = home();
        let l = lock().expect("a lock");
        assert!(lock().is_none(), "two rushes both hold the pass");
        drop(l);
        let l2 = lock().expect("the lock wasn't let go");
        drop(l2);
        let p = dir().join("pass.lock");
        std::fs::write(&p, "").unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(std::time::SystemTime::now() - LOCK_STALE - Duration::from_secs(60))
            .unwrap();
        assert!(lock().is_some(), "a stale lock blocks for good");
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn reserve_and_begin() {
        let _h = home();
        let now = Time::now();
        begin(now).unwrap();
        reserve(now).unwrap();
        reserve(now).unwrap();
        let mut r = load();
        assert!(r.last_run == now && r.reviews_left(now) == REVIEWS_PER_DAY - 2);
        r.merge(PassResult {
            at: now,
            reviewed: vec![now, now],
            ..PassResult::default()
        });
        assert_eq!(r.reviews_left(now), REVIEWS_PER_DAY - 2, "counted twice");
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn corrupt_record() {
        let _h = home();
        std::fs::create_dir_all(dir()).unwrap();
        std::fs::write(record_path(), "{broken").unwrap();
        let r = load();
        assert!(
            !r.due(Time::now(), 1_000_000),
            "a broken record lets a pass run at once"
        );
        assert!(dir().join("record.json.bad").exists());
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn active_by_file_times() {
        let tmp = tempfile::tempdir().unwrap();
        let acct = Profile {
            kind: crate::AGENT.into(),
            name: String::new(),
            dir: tmp.path().to_string_lossy().into_owned(),
        };
        let t = add_secs(Time::now(), -60);
        assert!(!active(&acct, t));
        let d = PathBuf::from(crate::source::transcripts_dir(&acct)).join("proj");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("s.jsonl");
        std::fs::write(&p, "{}").unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        assert!(!active(&acct, t), "an old transcript counts as new work");
        f.set_modified(std::time::SystemTime::now()).unwrap();
        assert!(active(&acct, t));
    }
}
