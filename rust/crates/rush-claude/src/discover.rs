//! Claude Code's sessions as found on disk, read again only when their
//! files change: background jobs, live session files and past transcripts.
//! Port of internal/adapters/claude/discover.go.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rush_core::time::Ms;

use crate::account::Account;
use crate::jobs::{ClaudeJob, list_job_ids, load_job, ms_of, mtime};
use crate::past::{Convo, read_convo};
use crate::sessions::{Session, read_session, session_alive};

/// How often a projects folder is listed again for conversations that
/// ended or appeared since.
pub const PAST_EVERY: Ms = 15_000;

/// What was read, so a listing every second reads only what changed.
#[derive(Default)]
pub struct Discovery {
    jobs: HashMap<PathBuf, JobRead>,
    sessions: HashMap<PathBuf, SessionRead>,
    past: HashMap<PathBuf, PastListing>,
    buf: Vec<u8>,
}

struct JobRead {
    j: ClaudeJob,
    modified: Ms,
    checked: Ms,
}

struct SessionRead {
    modified: Ms,
    size: Option<u64>,
    s: Option<Session>,
}

#[derive(Default)]
struct PastListing {
    at: Ms,
    files: Vec<PastFile>,
}

/// One transcript in a projects folder, read again only when it changes.
#[derive(Clone, Debug)]
pub struct PastFile {
    pub path: PathBuf,
    pub modified: Ms,
    pub size: u64,
    /// None when it's no conversation of its own (see read_convo).
    pub convo: Option<Convo>,
}

impl Discovery {
    pub fn new() -> Discovery {
        Discovery::default()
    }

    /// The account's background jobs, sorted by id. A finished job's file
    /// is looked at every 10s, a live one's every time.
    pub fn jobs(&mut self, acct: &Account, now: Ms) -> Vec<ClaudeJob> {
        let root = acct.jobs_dir();
        let ids = list_job_ids(acct);
        let mut out = Vec::with_capacity(ids.len());
        let mut listed = Vec::with_capacity(ids.len());
        for id in &ids {
            let dir = root.join(id);
            if let Some(j) = self.read_job(acct, id, &dir, now) {
                out.push(j);
            }
            listed.push(dir);
        }
        self.jobs
            .retain(|dir, _| !dir.starts_with(&root) || listed.contains(dir));
        out
    }

    fn read_job(&mut self, acct: &Account, id: &str, dir: &Path, now: Ms) -> Option<ClaudeJob> {
        if let Some(r) = self.jobs.get(dir)
            && !r.j.live()
            && r.j.job.in_flight == 0
            && now - r.checked < 10_000
        {
            return Some(r.j.clone());
        }
        let Some(modified) = mtime(&dir.join("state.json")) else {
            self.jobs.remove(dir);
            return None;
        };
        if let Some(r) = self.jobs.get_mut(dir)
            && r.modified == modified
        {
            r.checked = now;
            return Some(r.j.clone());
        }
        match load_job(acct, id) {
            Some(j) => {
                self.jobs.insert(
                    dir.to_path_buf(),
                    JobRead {
                        j: j.clone(),
                        modified,
                        checked: now,
                    },
                );
                Some(j)
            }
            // Mid-write: keep the last good read.
            None => self.jobs.get(dir).map(|r| r.j.clone()),
        }
    }

    /// The account's session files whose process is alive, parsing only
    /// the files that changed.
    pub fn sessions(&mut self, acct: &Account) -> Vec<Session> {
        let dir = acct.sessions_dir();
        let Ok(ents) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut listed = Vec::new();
        for e in ents.flatten() {
            if !e.file_name().to_string_lossy().ends_with(".json") {
                continue;
            }
            let path = e.path();
            let (modified, size) = match e.metadata() {
                Ok(m) => (m.modified().map(ms_of).unwrap_or(0), Some(m.len())),
                Err(_) => (0, None),
            };
            let r = self.sessions.entry(path.clone()).or_insert(SessionRead {
                modified: -1,
                size: None,
                s: None,
            });
            if r.size != size || r.modified != modified || r.modified == -1 {
                r.modified = modified;
                r.size = size;
                r.s = read_session(&path);
            }
            if let Some(s) = &r.s
                && session_alive(s)
            {
                out.push(s.clone());
            }
            listed.push(path);
        }
        self.sessions
            .retain(|p, _| !p.starts_with(&dir) || listed.contains(p));
        out
    }

    /// Lists the projects folder again at the next `past`, whenever it was
    /// last listed: something in it changed.
    pub fn expire_past(&mut self, acct: &Account) {
        if let Some(ls) = self.past.get_mut(&acct.projects_dir()) {
            ls.at = Ms::MIN / 2;
        }
    }

    /// Every transcript in the account's projects folder, listed again at
    /// most every PAST_EVERY; those that are conversations have `convo`.
    pub fn past(&mut self, acct: &Account, now: Ms) -> impl Iterator<Item = &PastFile> {
        let root = acct.projects_dir();
        let fresh = self
            .past
            .get(&root)
            .is_some_and(|ls| now - ls.at < PAST_EVERY);
        if !fresh {
            let prev = self.past.remove(&root).unwrap_or_default();
            let mut old: HashMap<PathBuf, PastFile> = prev
                .files
                .into_iter()
                .map(|f| (f.path.clone(), f))
                .collect();
            let mut next = PastListing {
                at: now,
                files: Vec::with_capacity(old.len()),
            };
            let mut projects: Vec<_> = std::fs::read_dir(&root)
                .map(|r| r.flatten().collect())
                .unwrap_or_default();
            projects.sort_by_key(|e| e.file_name());
            for pd in projects {
                if !pd.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let mut ents: Vec<_> = std::fs::read_dir(pd.path())
                    .map(|r| r.flatten().collect())
                    .unwrap_or_default();
                ents.sort_by_key(|e| e.file_name());
                for e in ents {
                    if !e.file_name().to_string_lossy().ends_with(".jsonl") {
                        continue;
                    }
                    let Ok(m) = e.metadata() else {
                        continue;
                    };
                    if m.is_dir() {
                        continue;
                    }
                    let path = e.path();
                    let modified = m.modified().map(ms_of).unwrap_or(0);
                    let f = match old.remove(&path) {
                        Some(f) if f.modified == modified && f.size == m.len() => f,
                        _ => PastFile {
                            convo: read_convo(&path, &mut self.buf),
                            path,
                            modified,
                            size: m.len(),
                        },
                    };
                    next.files.push(f);
                }
            }
            if self.buf.capacity() > 256 << 10 {
                self.buf = Vec::new();
            }
            self.past.insert(root.clone(), next);
        }
        self.past
            .get(&root)
            .into_iter()
            .flat_map(|ls| ls.files.iter())
            .filter(|f| f.convo.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::tests::TempDir;

    // A profile's running sessions and past conversations are found on disk.
    #[test]
    fn discover_live_and_past() {
        let d = TempDir::new("discover");
        let acct = Account::at("claude", &d.0);
        let sid = "0123456789abcdef-past";
        let proj = d.0.join("projects").join("-r");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(d.0.join("sessions")).unwrap();
        let lines = [
            r#"{"type":"user","cwd":"/r","timestamp":"2026-09-29T10:00:00Z","uuid":"u1","message":{"role":"user","content":"fix the build"}}"#,
            r#"{"type":"assistant","cwd":"/r","timestamp":"2026-09-29T10:00:05Z","uuid":"a1","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Fixed."}]}}"#,
            r#"{"type":"user","cwd":"/r","timestamp":"2026-09-29T11:00:00Z","uuid":"u2","message":{"role":"user","content":"later"}}"#,
        ];
        std::fs::write(proj.join(format!("{sid}.jsonl")), lines.join("\n") + "\n").unwrap();
        let live = format!(
            r#"{{"pid":{},"sessionId":"fedcba9876543210-live","cwd":"/r","kind":"interactive","status":"busy"}}"#,
            std::process::id()
        );
        std::fs::write(d.0.join("sessions").join("1.json"), live).unwrap();

        let mut disc = Discovery::new();
        let now = rush_core::time::now();
        let past: Vec<_> = disc.past(&acct, now).cloned().collect();
        assert_eq!(past.len(), 1);
        let c = past[0].convo.as_ref().unwrap();
        assert_eq!((c.session_id.as_str(), c.cwd.as_str()), (sid, "/r"));
        let got = disc.sessions(&acct);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].session_id, "fedcba9876543210-live");
        assert_eq!(got[0].status, "busy");
        // Listed again within PAST_EVERY: the same listing, files unread.
        std::fs::remove_file(proj.join(format!("{sid}.jsonl"))).unwrap();
        assert_eq!(disc.past(&acct, now + 1).count(), 1);
        assert_eq!(disc.past(&acct, now + PAST_EVERY).count(), 0);
        // Told the folder changed, it lists it again at once.
        std::fs::write(proj.join(format!("{sid}.jsonl")), lines.join("\n") + "\n").unwrap();
        assert_eq!(disc.past(&acct, now + PAST_EVERY + 1).count(), 0);
        disc.expire_past(&acct);
        assert_eq!(disc.past(&acct, now + PAST_EVERY + 2).count(), 1);
    }

    #[test]
    fn jobs_kept_until_their_file_changes() {
        let d = TempDir::new("jobs");
        let acct = Account::at("default", &d.0);
        let jd = acct.jobs_dir().join("abcd1234");
        std::fs::create_dir_all(&jd).unwrap();
        std::fs::create_dir_all(acct.jobs_dir().join("toolongname")).unwrap();
        std::fs::write(jd.join("state.json"), r#"{"state":"working","name":"one"}"#).unwrap();
        let mut disc = Discovery::new();
        let now = rush_core::time::now();
        let js = disc.jobs(&acct, now);
        assert_eq!(js.len(), 1);
        assert_eq!(js[0].job.name, "one");
        // Mid-write: the last good read stands.
        std::fs::write(jd.join("state.json"), r#"{"state":"#).unwrap();
        let js = disc.jobs(&acct, now + 1);
        assert_eq!(js[0].job.name, "one");
        std::fs::remove_dir_all(&jd).unwrap();
        assert!(disc.jobs(&acct, now + 2).is_empty());
        assert!(disc.jobs.is_empty());
    }
}
