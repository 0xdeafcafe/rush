//! Edits a JSON settings file in place, as an agent reads it live: keys
//! keep their order, keys rush doesn't know are kept exactly, and a save
//! applies only the changes made here to the file as it is then.
//!
//! Ported from internal/settingsfile/settingsfile.go.

use crate::json;
use serde::de::{DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// A settings file, edited in place. Keys keep their order, keys rush
/// doesn't know about are kept exactly, and a save re-reads the file and
/// applies only the changes made here, so whatever the agent wrote in the
/// meantime survives. It writes a temp file then renames it (through a
/// symlink, to the real file), because running sessions read the file live.
#[derive(Debug)]
pub struct File {
    pub path: PathBuf,
    raw: Object,
    changes: Vec<Change>,
}

#[derive(Clone, Debug)]
struct Change {
    keys: Vec<String>,
    /// None removes.
    val: Option<String>,
}

/// A JSON object that remembers its key order; its values as written.
#[derive(Clone, Debug, Default)]
struct Object {
    entries: Vec<(String, String)>,
}

impl Object {
    fn get(&self, k: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(x, _)| x == k)
            .map(|(_, v)| v.as_str())
    }

    fn set(&mut self, k: &str, v: Option<String>) {
        let at = self.entries.iter().position(|(x, _)| x == k);
        match (v, at) {
            (None, Some(i)) => {
                self.entries.remove(i);
            }
            (None, None) => {}
            (Some(v), Some(i)) => self.entries[i].1 = v,
            (Some(v), None) => self.entries.push((k.into(), v)),
        }
    }

    fn bytes(&self) -> String {
        let mut b = String::from("{");
        for (i, (k, v)) in self.entries.iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            b.push_str(&encode(k).unwrap_or_default());
            b.push(':');
            b.push_str(v);
        }
        b.push('}');
        b
    }
}

/// Reads an object's entries in order, each value as written; a repeated
/// key keeps its first place and its last value.
impl<'de> Deserialize<'de> for Object {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Object, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Object;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Object, A::Error> {
                let mut o = Object::default();
                while let Some((k, v)) = m.next_entry::<String, Box<RawValue>>()? {
                    o.set(&k, Some(v.get().to_string()));
                }
                Ok(o)
            }
        }
        d.deserialize_map(V)
    }
}

fn parse_object(b: &[u8]) -> io::Result<Object> {
    if b.iter().all(|c| c.is_ascii_whitespace()) {
        return Ok(Object::default());
    }
    let first = b.iter().find(|c| !c.is_ascii_whitespace());
    if first != Some(&b'{') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "settings.json isn't a JSON object",
        ));
    }
    // As Go's decoder reads it: the object, and nothing after it looked at.
    let read = |b: &[u8]| Object::deserialize(&mut serde_json::Deserializer::from_slice(b));
    match read(b) {
        Ok(o) => Ok(o),
        // A string cut mid-character still reads, as jsonx lets it.
        Err(e) => match std::str::from_utf8(b) {
            Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(_) => read(String::from_utf8_lossy(b).as_bytes())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        },
    }
}

/// v in JSON, <, > and & unescaped: a hook command like `a && b > out`
/// must stay readable.
fn encode<T: Serialize + ?Sized>(v: &T) -> Option<String> {
    json::to_vec(v).ok().and_then(|b| String::from_utf8(b).ok())
}

fn read_object(path: &Path) -> io::Result<Object> {
    match fs::read(path) {
        Ok(b) => parse_object(&b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Object::default()),
        Err(e) => Err(e),
    }
}

fn set_in(o: &mut Object, keys: &[String], val: Option<String>) {
    let k = &keys[0];
    if keys.len() == 1 {
        o.set(k, val);
        return;
    }
    let mut child = o
        .get(k)
        .and_then(|raw| parse_object(raw.as_bytes()).ok())
        .unwrap_or_default();
    set_in(&mut child, &keys[1..], val);
    if child.entries.is_empty() {
        o.set(k, None);
        return;
    }
    o.set(k, Some(child.bytes()));
}

impl File {
    /// Reads a settings file; a missing one is empty.
    pub fn load(path: impl Into<PathBuf>) -> io::Result<File> {
        let path = path.into();
        let raw = read_object(&path)?;
        Ok(File {
            path,
            raw,
            changes: Vec::new(),
        })
    }

    fn lookup(&self, keys: &[&str]) -> Option<String> {
        let mut cur = self.raw.clone();
        for (i, k) in keys.iter().enumerate() {
            let raw = cur.get(k)?.to_string();
            if i == keys.len() - 1 {
                return Some(raw);
            }
            cur = parse_object(raw.as_bytes()).ok()?;
        }
        None
    }

    /// The value at a dotted path, e.g. "permissions.defaultMode"; None
    /// when it's unset or isn't a T.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Option<T> {
        let keys: Vec<&str> = path.split('.').collect();
        let raw = self.lookup(&keys)?;
        json::from_slice(raw.as_bytes()).ok()
    }

    /// get for a string, "" when unset.
    pub fn string(&self, path: &str) -> String {
        self.get(path).unwrap_or_default()
    }

    /// Writes a value by dotted path. Objects on the way are created, and
    /// their other keys kept in order.
    pub fn set<T: Serialize + ?Sized>(&mut self, path: &str, v: &T) -> io::Result<()> {
        let val = encode(v).ok_or_else(|| io::Error::other("can't write that value"))?;
        self.change(path, Some(val));
        Ok(())
    }

    /// Removes the key at a dotted path, and objects left empty by it.
    pub fn delete(&mut self, path: &str) {
        self.change(path, None);
    }

    fn change(&mut self, path: &str, val: Option<String>) {
        let keys: Vec<String> = path.split('.').map(String::from).collect();
        set_in(&mut self.raw, &keys, val.clone());
        self.changes.push(Change { keys, val });
    }

    /// The settings' env block: variables every session starts with.
    pub fn env(&self) -> std::collections::BTreeMap<String, String> {
        self.get("env").unwrap_or_default()
    }

    /// Sets one variable; an empty value removes it.
    pub fn set_env(&mut self, name: &str, value: &str) -> io::Result<()> {
        if value.is_empty() {
            self.delete(&format!("env.{name}"));
            return Ok(());
        }
        self.set(&format!("env.{name}"), value)
    }

    /// Hands the changes made here so far to a File of their own, to be
    /// saved on another thread while this one is still read and changed:
    /// s keeps showing them, and a save of it later won't write them again.
    pub fn detach(&mut self) -> File {
        File {
            path: self.path.clone(),
            raw: Object::default(),
            changes: std::mem::take(&mut self.changes),
        }
    }

    /// Whether there are changes not yet saved.
    pub fn changed(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Applies the changes made here to the file as it is now, indented
    /// two spaces, as agents write theirs.
    pub fn save(&mut self) -> io::Result<()> {
        // A dotfiles symlink stays a symlink.
        let path = fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let mut cur = read_object(&path)?;
        for c in &self.changes {
            set_in(&mut cur, &c.keys, c.val.clone());
        }
        let mut out = json::indent(cur.bytes().as_bytes())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not JSON"))?;
        out.push(b'\n');
        let dir = path.parent().unwrap_or(Path::new("."));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let (tmp, mut f) = crate::store::create_temp(dir, ".settings-", ".json")?;
        let res = f.write_all(&out);
        drop(f);
        if let Err(e) = res {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        let mode = fs::metadata(&path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o644);
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode));
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        self.raw = cur;
        self.changes.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Detach hands the changes so far to a File saved elsewhere: the one
    // still being edited keeps showing them, and doesn't write them twice.
    #[test]
    fn detach() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("settings.json");
        fs::write(&path, r#"{"model":"opus","env":{"A":"1"}}"#).unwrap();
        let mut s = File::load(&path).unwrap();
        s.set("env.B", "2").unwrap();
        let mut det = s.detach();
        assert_eq!(
            s.string("env.B"),
            "2",
            "the edited one should still show the change"
        );
        // The file changes on disk before the save: that's kept.
        fs::write(&path, r#"{"model":"sonnet","env":{"A":"1"}}"#).unwrap();
        det.save().unwrap();
        let b = fs::read_to_string(&path).unwrap();
        assert!(
            b.contains(r#""B": "2""#) && b.contains("sonnet"),
            "saved:\n{b}"
        );
        assert!(
            !s.changed(),
            "the changes should have gone with the detached one"
        );
    }

    // Through a symlink, the real file is written and the link stays.
    #[test]
    fn saves_through_a_symlink() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real.json");
        fs::write(&real, "{}").unwrap();
        let link = d.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut s = File::load(&link).unwrap();
        s.set("theme", "dark").unwrap();
        s.save().unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "{\n  \"theme\": \"dark\"\n}\n"
        );
        assert!(
            File::load(d.path().join("missing.json"))
                .unwrap()
                .env()
                .is_empty()
        );
        fs::write(&real, "[1]").unwrap();
        assert!(File::load(&link).is_err(), "an array isn't settings");
    }

    // As Go's decoder reads one: what follows the object isn't looked at,
    // and a string cut mid-character still reads.
    #[test]
    fn reads_as_go_decodes() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("settings.json");
        fs::write(&path, "{\"model\":\"opus\"}\n}").unwrap();
        assert_eq!(File::load(&path).unwrap().string("model"), "opus");
        fs::write(&path, b"{\"model\":\"op\xffus\",\"n\":1}").unwrap();
        let f = File::load(&path).unwrap();
        assert_eq!(f.string("model"), "op\u{fffd}us");
        assert_eq!(f.get::<i64>("n"), Some(1));
        fs::write(&path, "{\"model\":").unwrap();
        assert!(File::load(&path).is_err(), "a torn object isn't read");
    }
}
