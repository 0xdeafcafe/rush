//! The store: config.json and state.json, loaded and saved as Go rush
//! does. A file that can't be read comes back as it was last read whole;
//! every save goes through a temp file of its own and a rename, so two
//! rushes saving at once never leave half of one behind.
//!
//! Ported from internal/state/state.go:603-860 (Overlay, Store, Load, Copy,
//! Hide, SaveOverlay, SaveConfig, KeepBefore, loadJSON, writeJSON,
//! writeBytes, OnSave, ConfigOnDisk, Reload), :957-959 (WriteJSON) and
//! envconfig.go:146-157 (FromEnv).

use crate::config::Config;
use crate::envconfig::{self, EnvSet};
use crate::json::{self, lenient};
use crate::time::Time;
use crate::writebehind;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// What rush keeps of each agent beside its transcript: state.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlay {
    #[serde(
        rename = "done",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub done: BTreeMap<String, Time>,
    #[serde(
        rename = "names",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub names: BTreeMap<String, String>,
    #[serde(
        rename = "groups",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub groups: BTreeMap<String, String>,
    #[serde(
        rename = "moved",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub moved: BTreeMap<String, String>,
    #[serde(
        rename = "seen",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub seen: BTreeMap<String, Time>,
    /// Agents put away with ctrl+x: no reading lists them. Their
    /// transcripts are untouched, and the key stays here to find them by.
    #[serde(
        rename = "hidden",
        deserialize_with = "lenient",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub hidden: BTreeMap<String, Time>,
}

/// rush's config and overlay, as loaded.
#[derive(Debug, Default)]
pub struct Store {
    pub config: Config,
    pub overlay: Overlay,
    /// The settings the environment set: see envconfig.
    env: Vec<EnvSet>,
    /// Saves one at a time.
    mu: Mutex<()>,
}

fn config_path() -> PathBuf {
    crate::dir().join("config.json")
}

impl Store {
    /// Loads config.json and state.json from rush's folder, bringing an
    /// older config up to date and taking the settings the environment
    /// sets.
    pub fn load() -> Store {
        Store::load_with_env(&envconfig::lookup_env)
    }

    /// Load, with the environment read through getenv.
    pub fn load_with_env(getenv: &dyn Fn(&str) -> Option<String>) -> Store {
        let dir = crate::dir();
        let mut config: Config = load_json(&dir.join("config.json")).unwrap_or_default();
        let overlay: Overlay = load_json(&dir.join("state.json")).unwrap_or_default();
        config.migrate();
        let env = envconfig::apply_env(&mut config, getenv);
        Store {
            config,
            overlay,
            env,
            mu: Mutex::new(()),
        }
    }

    /// The config and overlay as they are now, sharing nothing with s: for
    /// reading, and only reading, off the UI's thread while the UI goes on
    /// changing s.
    pub fn copy(&self) -> Store {
        Store {
            config: self.config.clone(),
            overlay: self.overlay.clone(),
            env: Vec::new(),
            mu: Mutex::new(()),
        }
    }

    /// Puts the agent with key away for good; call save_overlay after.
    pub fn hide(&mut self, key: &str) {
        self.overlay.hidden.insert(key.into(), Time::now());
    }

    /// Writes state.json. With write-behind on, the writer encodes it too,
    /// from a copy.
    pub fn save_overlay(&self) -> io::Result<()> {
        let _g = self.mu.lock().unwrap_or_else(|e| e.into_inner());
        let path = crate::dir().join("state.json");
        if !writebehind::on() {
            return write_json(&path, &self.overlay);
        }
        let o = self.overlay.clone();
        let key = path.to_string_lossy().into_owned();
        writebehind::queue(key, Box::new(move || write_json(&path, &o)));
        Ok(())
    }

    /// Writes config.json: each setting the environment set as the file
    /// had it.
    pub fn save_config(&self) -> io::Result<()> {
        let _g = self.mu.lock().unwrap_or_else(|e| e.into_inner());
        let c = envconfig::for_saving(&self.config, &self.env);
        writebehind::save(&config_path(), &c)
    }

    /// Takes config b, as config_on_disk read it, in place of s's own.
    pub fn reload(&mut self, b: &[u8]) -> io::Result<()> {
        let mut c: Config = json::from_slice(b).map_err(io::Error::other)?;
        c.migrate();
        let env = envconfig::apply_env(&mut c, &envconfig::lookup_env);
        {
            let _g = self.mu.lock().unwrap_or_else(|e| e.into_inner());
            self.config = c;
            self.env = env;
        }
        // Taken in from outside: rush's own earlier versions no longer
        // count, so going back to one of them is someone else's change
        // too.
        note_only_config(&config_path(), b);
        Ok(())
    }

    /// The variable that set the setting at key this run, if one did.
    pub fn from_env(&self, key: &str) -> Option<String> {
        let name = envconfig::env_name(key);
        self.env
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.name.clone())
    }
}

/// Copies config.json aside as config.json.<name>, once, before a change
/// an older rush wouldn't make: the copy is never overwritten. With
/// write-behind on, the writer copies it, before any save after.
pub fn keep_before(name: &str) {
    let path = config_path();
    if writebehind::on() {
        let name = name.to_string();
        writebehind::queue(
            format!("keep:{name}"),
            Box::new(move || {
                keep_before_at(&path, &name);
                Ok(())
            }),
        );
        return;
    }
    keep_before_at(&path, name);
}

fn keep_before_at(path: &Path, name: &str) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".");
    aside.push(name);
    let aside = PathBuf::from(aside);
    if fs::metadata(&aside).is_ok() {
        return;
    }
    if let Ok(b) = fs::read(path) {
        let _ = write_file_mode(&aside, &b, 0o600);
    }
}

/// Reads one of rush's own files, if it's there.
fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let b = fs::read(path).ok()?;
    json::from_slice(&b).ok()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Reads one of rush's own files, falling back to the copy of it last read
/// whole when it can't be: starting from nothing would save nothing over
/// it, and your settings, accounts and done marks with it. The unreadable
/// one is kept aside as .broken.
pub(crate) fn load_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let b = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return read_json(&with_suffix(path, ".bak")),
    };
    if json::valid(&b) {
        note_config(path, &b);
        let v = json::from_slice(&b).ok();
        let _ = write_file_mode(&with_suffix(path, ".bak"), &b, 0o600);
        return v;
    }
    let _ = write_file_mode(&with_suffix(path, ".broken"), &b, 0o600);
    read_json(&with_suffix(path, ".bak"))
}

/// os.WriteFile: the file made with mode when new, truncated when not.
fn write_file_mode(path: &Path, b: &[u8], mode: u32) -> io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)?;
    f.write_all(b)
}

/// Writes v to path as indented JSON, all at once: a reader sees the old
/// file or the new one, never half of either.
pub fn write_json<T: Serialize + ?Sized>(path: &Path, v: &T) -> io::Result<()> {
    let b = json::to_vec_indent(v).map_err(io::Error::other)?;
    write_bytes(path, &b)
}

/// Told of each file rush saves, by path: the events log logs it. It must
/// not wait.
static ON_SAVE: Mutex<Option<fn(&Path)>> = Mutex::new(None);

/// Sets what's told of each file rush saves.
pub fn set_on_save(f: Option<fn(&Path)>) {
    *ON_SAVE.lock().unwrap_or_else(|e| e.into_inner()) = f;
}

static TEMP_N: AtomicU64 = AtomicU64::new(0);

/// Writes b to path through a temp file of its own beside it, then a
/// rename.
pub fn write_bytes(path: &Path, b: &[u8]) -> io::Result<()> {
    let on_save = *ON_SAVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = on_save {
        f(path);
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // A temp file of its own, so two rushes saving at once can't write
    // into each other's and leave half of one behind.
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (tmp, mut f) = create_temp(dir, &format!("{base}."), ".tmp")?;
    let mut res = f.write_all(b);
    drop(f);
    if res.is_ok() {
        res = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    if let Err(e) = res {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // Noted before it lands: a watcher woken by the rename reads it as
    // this rush's own save, not someone else's change.
    note_config(path, b);
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// os.CreateTemp: a new file no one else has, prefix and suffix around a number of its own.
pub(crate) fn create_temp(
    dir: &Path,
    prefix: &str,
    suffix: &str,
) -> io::Result<(PathBuf, fs::File)> {
    loop {
        let n = TEMP_N.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let p = dir.join(format!("{prefix}{}{n}{nanos}{suffix}", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
        {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// config.json as this rush last read or wrote it, so a change made by
/// anything else shows. By path, as the writer's thread may not know
/// which folder a save was for (one per test's thread). The last few
/// versions, not only the newest: a save is noted before its rename
/// lands, so a watcher woken by one save can read it after the next is
/// already noted, and the one it reads is still this rush's own.
static SEEN_CONFIG: Mutex<BTreeMap<PathBuf, std::collections::VecDeque<Vec<u8>>>> =
    Mutex::new(BTreeMap::new());

/// How many of this rush's own versions of config.json are kept.
const SEEN_KEPT: usize = 8;

fn note_config(path: &Path, b: &[u8]) {
    if path.file_name().is_none_or(|n| n != "config.json") {
        return;
    }
    let mut seen = SEEN_CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    let kept = seen.entry(path.to_path_buf()).or_default();
    kept.retain(|k| k.as_slice() != b);
    kept.push_back(b.to_vec());
    while kept.len() > SEEN_KEPT {
        kept.pop_front();
    }
}

/// Forgets every version noted of path but b.
fn note_only_config(path: &Path, b: &[u8]) {
    if let Some(kept) = SEEN_CONFIG
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(path)
    {
        kept.clear();
    }
    note_config(path, b);
}

/// config.json when something other than this rush has changed it since
/// it last read or wrote it, for reload. It reads the file, so it's for
/// off the UI's thread.
pub fn config_on_disk() -> Option<Vec<u8>> {
    let path = config_path();
    let b = fs::read(&path).ok()?;
    if !json::valid(&b) {
        return None;
    }
    let seen = SEEN_CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    let own = seen
        .get(&path)
        .is_some_and(|kept| kept.iter().any(|k| *k == b));
    (!own).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Folder, Login};

    fn home() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        crate::set_thread_dir(Some(d.path().to_path_buf()));
        d
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    // A config.json that can't be read comes back as it was last read
    // whole, rather than empty, so the next save doesn't wipe your settings.
    #[test]
    fn load_falls_back_to_last_good() {
        let d = home();
        let mut s = Store::load_with_env(&no_env);
        s.config.sort_by = "cost".into();
        s.config.folders.push(Folder {
            name: "a".into(),
            config_dir: "/x".into(),
        });
        s.save_config().unwrap();
        Store::load_with_env(&no_env); // read whole: kept as the last good copy
        let path = d.path().join("config.json");
        fs::write(&path, r#"{"sortBy":"cost"}  "groupBy":"#).unwrap();
        let s = Store::load_with_env(&no_env);
        assert!(
            s.config.sort_by == "cost" && s.config.folders.len() == 1,
            "{:?}",
            s.config
        );
        assert!(
            with_suffix(&path, ".broken").exists(),
            "the unreadable config wasn't kept aside"
        );
        let tmps: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(tmps.is_empty(), "temp files left behind");
    }

    // Two saves close together: the watcher woken by the first reads it
    // after the second is noted, and it's still this rush's own.
    #[test]
    fn config_on_disk_knows_its_own_recent_saves() {
        let d = home();
        let path = d.path().join("config.json");
        let mut s = Store::load_with_env(&no_env);
        s.config.dock_lines = 4;
        s.save_config().unwrap();
        crate::flush().unwrap();
        let first = fs::read(&path).unwrap();
        s.config.dock_lines = 5;
        s.save_config().unwrap();
        crate::flush().unwrap();
        fs::write(&path, &first).unwrap(); // as the first rename lands late
        assert!(config_on_disk().is_none(), "its own earlier save");
        fs::write(&path, br#"{"dockLines": 9}"#).unwrap();
        assert!(config_on_disk().is_some(), "someone else's");
    }

    // Saved A, an editor wrote B and rush took it in, then A by hand
    // again: that's someone else's change, not rush's own A.
    #[test]
    fn going_back_after_an_outside_edit_is_an_outside_edit() {
        let d = home();
        let path = d.path().join("config.json");
        let mut s = Store::load_with_env(&no_env);
        s.config.dock_lines = 4;
        s.save_config().unwrap();
        crate::flush().unwrap();
        let a = fs::read(&path).unwrap();
        let b = br#"{"dockLines": 9}"#;
        fs::write(&path, b).unwrap();
        let got = config_on_disk().expect("the editor's");
        s.reload(&got).unwrap();
        assert!(config_on_disk().is_none(), "taken in");
        fs::write(&path, &a).unwrap();
        assert!(config_on_disk().is_some(), "back to A by hand");
    }

    #[test]
    fn config_on_disk_sees_only_others_changes() {
        let d = home();
        let mut s = Store::load_with_env(&no_env);
        s.config.dock_lines = 4;
        write_json(&d.path().join("config.json"), &s.config).unwrap();
        assert!(
            config_on_disk().is_none(),
            "rush's own save showed as a change"
        );
        fs::write(d.path().join("config.json"), r#"{"dockLines": 9}"#).unwrap();
        let b = config_on_disk().expect("an outside edit didn't show");
        s.reload(&b).unwrap();
        assert_eq!(s.config.dock_lines, 9);
        assert!(config_on_disk().is_none(), "a reloaded config showed again");
    }

    // A copy has the config as it is now and shares none of it.
    #[test]
    fn copy_follows_changes() {
        let mut s = Store::default();
        s.config.group_by = "status".into();
        s.config.logins = vec![Login {
            id: "a".into(),
            name: "a".into(),
            ..Login::default()
        }];
        let one = s.copy();
        s.config.group_by = "agent".into();
        s.config.logins[0].name = "renamed".into();
        let three = s.copy();
        assert!(three.config.group_by == "agent" && three.config.logins[0].name == "renamed");
        assert!(one.config.group_by == "status" && one.config.logins[0].name == "a");
    }

    // A setting from the environment wins for the run, and isn't saved;
    // one you change meanwhile is.
    #[test]
    fn env_sets_any_setting() {
        let d = home();
        fs::write(
            d.path().join("config.json"),
            r#"{"theme": "dark", "sideWidth": 0.3, "groupBy": "status"}"#,
        )
        .unwrap();
        let env = |name: &str| -> Option<String> {
            Some(
                match name {
                    "RUSH_COPY_ON_SELECT" => "0",
                    "RUSH_THEME" => "light",
                    "RUSH_HIBERNATE_AFTER_MINUTES" => "7",
                    "RUSH_SIDE_WIDTH" => "0.4",
                    "RUSH_GROUP_BY" => "agent",
                    "RUSH_COLOR_BLIND" => "maybe", // not a bool: left alone
                    _ => return None,
                }
                .into(),
            )
        };
        let mut s = Store::load_with_env(&env);
        let c = &s.config;
        assert!(
            !c.copies_on_select()
                && c.theme == "light"
                && c.hibernate.after_minutes == 7
                && c.side_width == 0.4
                && c.group_by == "agent"
                && !c.color_blind,
            "{c:?}"
        );
        assert_eq!(s.from_env("theme").as_deref(), Some("RUSH_THEME"));
        s.config.group_by = "group".into(); // changed in rush: that's saved
        s.save_config().unwrap();
        let saved = fs::read_to_string(d.path().join("config.json")).unwrap();
        for want in [
            r#""theme": "dark""#,
            r#""sideWidth": 0.3"#,
            r#""groupBy": "group""#,
        ] {
            assert!(saved.contains(want), "saved lacks {want}:\n{saved}");
        }
        for not in ["copyOnSelect", r#""afterMinutes": 7"#, "light"] {
            assert!(!saved.contains(not), "saved has {not}:\n{saved}");
        }
    }
}
