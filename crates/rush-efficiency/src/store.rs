//! Every transcript's figures, read incrementally and kept between runs.
//! Port of internal/efficiency/store.go. Reading happens outside the lock,
//! on threads of the refresh's own, so a view can be worked out while a
//! long first read runs.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use rush_core::adapter::Profile;
use serde::{Deserialize, Serialize};

use crate::scan::{Bucket, File, Use, scan};
use crate::source::{Source, source};

/// Bumped when File changes shape: the transcripts are then read again
/// from the start.
const CACHE_VERSION: i64 = 2;

/// How many transcripts are read at once.
const READERS: usize = 6;

/// What's kept of transcripts Claude Code has deleted (after 30 days, by
/// default), so the graphs reach back further than they do.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retired {
    #[serde(rename = "h")]
    pub hours: HashMap<i64, Bucket>,
    #[serde(rename = "u")]
    pub uses: HashMap<String, Use>,
    #[serde(rename = "s")]
    pub sessions: i64,
}

#[derive(Default)]
pub(crate) struct Inner {
    pub(crate) files: HashMap<String, File>,
    /// By account folder.
    pub(crate) retired: HashMap<String, Retired>,
    saved: Option<Instant>,
    dirty: bool,
}

/// Every transcript's figures.
#[derive(Default)]
pub struct Store {
    inner: RwLock<Inner>,
    /// One refresh at a time.
    scan: Mutex<()>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct CacheFile {
    version: i64,
    files: Option<HashMap<String, File>>,
}

fn cache_path() -> PathBuf {
    rush_state::cache_path("efficiency.json")
}

fn retired_path() -> PathBuf {
    crate::dir().join("retired.json")
}

impl Store {
    /// The store as its cache left it; the first refresh fills it.
    pub fn open() -> Store {
        let mut inner = Inner::default();
        if let Some(c) = std::fs::read(cache_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<CacheFile>(&b).ok())
            && c.version == CACHE_VERSION
            && let Some(files) = c.files
        {
            inner.files = files;
        }
        if let Some(r) = std::fs::read(retired_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
        {
            inner.retired = r;
        }
        Store {
            inner: RwLock::new(inner),
            scan: Mutex::new(()),
        }
    }

    /// A store holding files, and nothing retired: for tests.
    pub fn of(files: HashMap<String, File>) -> Store {
        Store {
            inner: RwLock::new(Inner {
                files,
                ..Inner::default()
            }),
            scan: Mutex::new(()),
        }
    }

    pub(crate) fn read(&self) -> RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    /// How many transcripts it knows.
    pub fn len(&self) -> usize {
        self.read().files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reads what's new in every profile's transcripts, through the
    /// agent's Source; whether anything changed.
    pub fn refresh(&self, profiles: &[Profile]) -> bool {
        self.refresh_from(source(), profiles)
    }

    /// refresh, reading through src.
    pub fn refresh_from(&self, src: &dyn Source, profiles: &[Profile]) -> bool {
        let _one = self.scan.lock().unwrap_or_else(|e| e.into_inner());
        let mut jobs: Vec<(String, File)> = Vec::new();
        let mut listed = HashSet::new();
        let mut gone = Vec::new();
        {
            let inner = self.read();
            for a in profiles {
                for p in src.transcripts(a) {
                    listed.insert(p.clone());
                    let Ok(st) = std::fs::metadata(&p) else {
                        continue;
                    };
                    let size = st.len() as i64;
                    let old = inner.files.get(&p);
                    if old.is_some_and(|o| o.size == size && o.offset == size) {
                        continue;
                    }
                    let f = match old {
                        Some(o) => o.clone(),
                        None => File::new(
                            &a.dir,
                            p.contains(&format!("{0}subagents{0}", std::path::MAIN_SEPARATOR)),
                        ),
                    };
                    jobs.push((p, f));
                }
            }
            for (p, f) in &inner.files {
                if listed.contains(p) {
                    continue;
                }
                let mine = profiles.iter().any(|a| f.account == a.dir);
                if mine
                    && std::fs::metadata(p)
                        .err()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                {
                    gone.push(p.clone());
                }
            }
        }

        let changed = !jobs.is_empty() || !gone.is_empty();
        if !jobs.is_empty() {
            let queue = Mutex::new(jobs);
            std::thread::scope(|s| {
                for _ in 0..READERS {
                    s.spawn(|| {
                        let mut buf = Vec::with_capacity(64 << 10);
                        loop {
                            let job = queue.lock().unwrap_or_else(|e| e.into_inner()).pop();
                            let Some((path, mut f)) = job else { break };
                            buf = scan(src, &path, &mut f, std::mem::take(&mut buf))
                                .unwrap_or_default();
                            if buf.capacity() > 8 << 20 {
                                buf = Vec::with_capacity(64 << 10);
                            }
                            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
                            inner.files.insert(path, f);
                            inner.dirty = true;
                        }
                    });
                }
            });
        }
        if !gone.is_empty() {
            {
                let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
                for p in &gone {
                    if let Some(f) = inner.files.remove(p) {
                        inner.retire(&f);
                    }
                }
                inner.dirty = true;
            }
            self.save_retired();
        }
        let stale = self
            .read()
            .saved
            .is_none_or(|t| t.elapsed() > Duration::from_secs(120));
        if changed && stale {
            self.save();
        }
        changed
    }

    /// Folds a deleted transcript into its account's rollup.
    pub fn retire(&self, f: &File) {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retire(f);
    }

    fn save_retired(&self) {
        let b = serde_json::to_vec(&self.read().retired);
        if let Ok(b) = b {
            let _ = crate::write_file(&retired_path(), &b);
        }
    }

    /// Writes the cache when it has changed.
    pub fn save(&self) {
        let b = {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            if !inner.dirty {
                return;
            }
            inner.dirty = false;
            inner.saved = Some(Instant::now());
            #[derive(Serialize)]
            struct Out<'a> {
                version: i64,
                files: &'a HashMap<String, File>,
            }
            serde_json::to_vec(&Out {
                version: CACHE_VERSION,
                files: &inner.files,
            })
        };
        if let Ok(b) = b {
            let _ = crate::write_file(&cache_path(), &b);
        }
    }
}

impl Inner {
    fn retire(&mut self, f: &File) {
        let r = self.retired.entry(f.account.clone()).or_default();
        f.each_hour(|h, b| r.hours.entry(h).or_default().add(b));
        for (k, u) in &f.uses {
            let ru = r.uses.entry(k.clone()).or_default();
            ru.n += u.n;
            if ru.first.is_zero() || u.first < ru.first {
                ru.first = u.first;
            }
            if u.last > ru.last {
                ru.last = u.last;
            }
        }
        if !f.sub {
            r.sessions += 1;
        }
    }
}
