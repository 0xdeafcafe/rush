//! The cost cache keeps transcript totals so a restart does not rescan
//! gigabytes: costs.json in rush's cache folder, compact, as Go writes it.
//!
//! Ported from internal/state/state.go:862-955 (CostCache, LoadCostCache,
//! Get, MarkDirty, Save). Go's Get hands out a shared pointer under the
//! cache's lock; here get_mut borrows the cache, which its owner guards.

use crate::json;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

/// 5: the folder each transcript last worked in, cd and writes included.
pub const COST_CACHE_VERSION: i64 = 5;

/// What the scan keeps of each transcript, by path.
#[derive(Debug, Serialize, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct CostCache<T> {
    #[serde(rename = "version", default)]
    pub version: i64,
    #[serde(
        rename = "files",
        default = "BTreeMap::new",
        deserialize_with = "null_map"
    )]
    pub files: BTreeMap<String, T>,
    #[serde(skip)]
    dirty: bool,
}

fn null_map<'de, D, T>(d: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<BTreeMap<String, T>>::deserialize(d)?.unwrap_or_default())
}

impl<T: Serialize + DeserializeOwned + Default> CostCache<T> {
    /// Reads costs.json; one of another version starts over. Transcripts
    /// their agent has since deleted (it keeps them 30 days by default)
    /// needn't be remembered.
    pub fn load() -> CostCache<T> {
        let path = crate::cache_path("costs.json");
        let mut c: CostCache<T> = fs::read(&path)
            .ok()
            .and_then(|b| json::from_slice(&b).ok())
            .unwrap_or(CostCache {
                version: 0,
                files: BTreeMap::new(),
                dirty: false,
            });
        if c.version != COST_CACHE_VERSION {
            c.files.clear();
            c.version = COST_CACHE_VERSION;
        }
        let before = c.files.len();
        c.files.retain(
            |p, _| !matches!(fs::metadata(p), Err(e) if e.kind() == io::ErrorKind::NotFound),
        );
        c.dirty = c.files.len() != before;
        c
    }

    /// What's kept of the transcript at path, made empty when there's
    /// nothing yet.
    pub fn get_mut(&mut self, path: &str) -> &mut T {
        self.files.entry(path.to_string()).or_default()
    }

    /// Says the cache changed, so the next save writes it.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Writes costs.json when it changed. Compact: it's a cache nobody
    /// reads, and indenting made it a third bigger. It's written as it's
    /// made: it runs to megabytes, saved every 30s.
    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        let path = crate::cache_path("costs.json");
        if let Some(dir) = path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        let f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        let mut w = BufWriter::with_capacity(64 << 10, f);
        let res = json::to_writer(&mut w, self)
            .map_err(io::Error::other)
            .and_then(|_| w.flush());
        drop(w);
        if let Err(e) = res {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        fs::rename(&tmp, &path)
    }
}
