//! keybindings.json: the keys you gave rush's actions.
//!
//! Ported from internal/keymap/file.go:1-45 and keymap.go:187-191 (File),
//! with internal/state/state.go:27-62 (Dir) and :771-811 (writeJSON) for
//! where it lives and how it's written, until the state module is ported.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

/// keybindings.json: for each action you changed, its keys. An empty list
/// unbinds it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    #[serde(default, alias = "Bindings", deserialize_with = "bindings_or_null")]
    pub bindings: BTreeMap<String, Vec<String>>,
}

/// Go writes a File with no bindings as `"bindings": null`, and an action's
/// keys could be null too; both read as none.
fn bindings_or_null<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, Vec<String>>, D::Error> {
    let m: Option<BTreeMap<String, Option<Vec<String>>>> = Option::deserialize(d)?;
    Ok(m.unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, v.unwrap_or_default()))
        .collect())
}

impl File {
    /// f with id's keys set; None goes back to the defaults, an empty list
    /// unbinds it.
    pub fn with(&self, id: &str, keys: Option<Vec<String>>) -> File {
        let mut out = self.clone();
        match keys {
            None => {
                out.bindings.remove(id);
            }
            Some(k) => {
                out.bindings.insert(id.to_string(), k);
            }
        }
        out
    }
}

/// rush's folder: $RUSH_HOME, else ~/.config/rush, or ~/.config/agtop while
/// that's the one in use (rush's has no config.json in it yet).
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RUSH_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let rush = home.join(".config").join("rush");
    let agtop = home.join(".config").join("agtop");
    if !rush.join("config.json").exists() && agtop.join("config.json").exists() {
        return agtop;
    }
    rush
}

/// keybindings.json.
pub fn path() -> PathBuf {
    state_dir().join("keybindings.json")
}

/// Reads keybindings.json; none is no bindings of yours.
pub fn load() -> io::Result<File> {
    load_from(&path())
}

/// Reads the keybindings file at p; none is no bindings of yours.
pub fn load_from(p: &Path) -> io::Result<File> {
    match std::fs::read(p) {
        Ok(b) => serde_json::from_slice(&b).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(File::default()),
        Err(e) => Err(e),
    }
}

/// Writes keybindings.json.
pub fn save(f: &File) -> io::Result<()> {
    save_to(&path(), f)
}

/// Writes f to p as Go's state.WriteJSON does: indented two spaces, through
/// a temp file of its own so two rushes saving at once can't write into
/// each other's, readable only by you.
pub fn save_to(p: &Path, f: &File) -> io::Result<()> {
    let b = serde_json::to_vec_pretty(f).map_err(io::Error::other)?;
    write_bytes(p, &b)
}

fn write_bytes(p: &Path, b: &[u8]) -> io::Result<()> {
    let dir = p.parent().unwrap_or(Path::new("."));
    create_dir(dir)?;
    let base = p.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let (tmp, mut f) = create_temp(dir, base)?;
    let res = f.write_all(b).and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, p).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(unix)]
fn create_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// A new file named base.<n>.tmp in dir, as Go's os.CreateTemp makes one.
fn create_temp(dir: &Path, base: &str) -> io::Result<(PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ u64::from(std::process::id()) << 32;
    loop {
        let n = seed.wrapping_add(N.fetch_add(1, Ordering::Relaxed)) % 1_000_000_000;
        let tmp = dir.join(format!("{base}.{n}.tmp"));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}
