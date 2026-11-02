//! The plugin's logic, with no I/O and no clock: what it knows of each
//! message box, and what it keeps. Callers pass the time in.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How long a box must stay unchanged before what's in it is written down
/// as its autosave.
pub const DEBOUNCE: Duration = Duration::from_millis(1500);

/// How many drafts a box keeps when the setting says nothing it
/// understands.
pub const DEFAULT_KEEP: usize = 50;

/// A message you typed and didn't send.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    #[serde(with = "rush_plugin_sdk::time::rfc3339")]
    pub at: SystemTime,
}

/// Everything the plugin keeps, by box: a Session's box by its rush
/// session id, the Prompt's as "".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// What each box held when it last settled, until it's sent, or kept
    /// as a draft.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub autosave: BTreeMap<String, Draft>,
    /// Each box's drafts, oldest first.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub drafts: BTreeMap<String, Vec<Draft>>,
}

/// A box's text not yet settled, and when it last changed.
struct Pending {
    text: String,
    at: SystemTime,
}

/// The plugin's state: what it knows of each box, and what it keeps.
pub struct Book {
    pub store: Store,
    keep: usize,
    pending: HashMap<String, Pending>,
}

/// The "keep" setting, read.
pub fn parse_keep(v: &str) -> usize {
    match v.parse::<i64>() {
        Ok(n) if n > 0 => n as usize,
        _ => DEFAULT_KEEP,
    }
}

impl Book {
    /// Starts from what was kept, keeping at most keep drafts a box.
    pub fn new(store: Store, keep: usize) -> Book {
        let mut b = Book {
            store,
            keep: DEFAULT_KEEP,
            pending: HashMap::new(),
        };
        b.set_keep(keep);
        b
    }

    /// Changes how many drafts a box keeps, dropping the oldest beyond it.
    /// It says whether anything was dropped.
    pub fn set_keep(&mut self, n: usize) -> bool {
        self.keep = if n == 0 { DEFAULT_KEEP } else { n };
        let mut dropped = false;
        for ds in self.store.drafts.values_mut() {
            if ds.len() > self.keep {
                ds.drain(..ds.len() - self.keep);
                dropped = true;
            }
        }
        dropped
    }

    /// Notes what box key holds now. Nothing is written until it settles.
    pub fn changed(&mut self, key: &str, text: &str, now: SystemTime) {
        self.pending.insert(
            key.to_string(),
            Pending {
                text: text.to_string(),
                at: now,
            },
        );
    }

    /// Moves each box that hasn't changed for DEBOUNCE into its autosave,
    /// and says whether the store changed. An emptied box drops its
    /// autosave.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| since(now, p.at) >= DEBOUNCE)
            .map(|(k, _)| k.clone())
            .collect();
        let mut changed = false;
        for k in due {
            let Some(p) = self.pending.remove(&k) else {
                continue;
            };
            if p.text.is_empty() {
                changed |= self.store.autosave.remove(&k).is_some();
                continue;
            }
            if self
                .store
                .autosave
                .get(&k)
                .is_some_and(|a| a.text == p.text)
            {
                continue;
            }
            self.store.autosave.insert(
                k,
                Draft {
                    text: p.text,
                    at: p.at,
                },
            );
            changed = true;
        }
        changed
    }

    /// When settle next has something to do, if it will.
    pub fn next_settle(&self) -> Option<SystemTime> {
        self.pending.values().map(|p| p.at + DEBOUNCE).min()
    }

    /// Saves what box key holds, or text if it's given (a cleared box says
    /// what it held), as a draft, and forgets the box's autosave. It says
    /// whether the store changed.
    pub fn keep(&mut self, key: &str, text: &str, now: SystemTime) -> bool {
        let mut text = text.to_string();
        if text.is_empty() {
            text = match self.pending.get(key) {
                Some(p) => p.text.clone(),
                None => self
                    .store
                    .autosave
                    .get(key)
                    .map(|a| a.text.clone())
                    .unwrap_or_default(),
            };
        }
        self.pending.remove(key);
        let had = self.store.autosave.remove(key).is_some();
        if text.is_empty() {
            return had;
        }
        let ds = self.store.drafts.entry(key.to_string()).or_default();
        if let Some(last) = ds.last_mut()
            && last.text == text
        {
            last.at = now; // the same draft again: only newer
            return true;
        }
        ds.push(Draft { text, at: now });
        if ds.len() > self.keep {
            ds.drain(..ds.len() - self.keep);
        }
        true
    }

    /// Forgets box key's unsettled text and autosave: it went. It says
    /// whether the store changed.
    pub fn sent(&mut self, key: &str) -> bool {
        self.pending.remove(key);
        self.store.autosave.remove(key).is_some()
    }

    /// Takes box key's newest draft out, to put it back in the box; or its
    /// autosave, when it has no draft (rush quit with it typed). Taking it
    /// out means restoring again goes one further back, and a restored
    /// draft cleared again comes back as the newest.
    pub fn restore(&mut self, key: &str) -> Option<String> {
        if let Some(ds) = self.store.drafts.get_mut(key)
            && let Some(d) = ds.pop()
        {
            if ds.is_empty() {
                self.store.drafts.remove(key);
            }
            return Some(d.text);
        }
        self.store.autosave.remove(key).map(|a| a.text)
    }

    /// How many drafts box key keeps.
    pub fn count(&self, key: &str) -> usize {
        self.store.drafts.get(key).map_or(0, Vec::len)
    }
}

/// How long after then now is; nothing if it's before.
fn since(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or_default()
}

/// Reads the store from path; a missing or broken file is an empty one.
pub fn load(path: &Path) -> Store {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Writes the store to path atomically: a temporary file beside it, synced,
/// then renamed over it. Only the owner can read it.
pub fn save(path: &Path, s: &Store) -> std::io::Result<()> {
    let data = serde_json::to_vec(s).map_err(std::io::Error::other)?;
    write_atomic(path, &data)
}

/// Writes data to path so a reader sees the old file or the new one, never
/// half of it.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let tmp = dir.join(format!(".drafts-{}-{nanos}", std::process::id()));
    let res = write_then_rename(&tmp, path, data);
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

fn write_then_rename(tmp: &Path, path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> SystemTime {
        rush_plugin_sdk::time::parse("2026-09-28T10:00:00Z").unwrap()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn settle_waits_for_the_box_to_stop_changing() {
        let t0 = t0();
        let mut b = Book::new(Store::default(), 50);
        b.changed("s1", "hel", t0);
        b.changed("s1", "hello", t0 + secs(1));
        assert!(
            !b.settle(t0 + secs(2)),
            "settled 1s after the last change; want to wait for DEBOUNCE"
        );
        assert_eq!(b.next_settle(), Some(t0 + secs(1) + DEBOUNCE));
        assert!(
            b.settle(t0 + secs(1) + DEBOUNCE),
            "didn't settle after DEBOUNCE"
        );
        assert_eq!(b.store.autosave["s1"].text, "hello");
        assert_eq!(
            b.next_settle(),
            None,
            "nothing should be pending once settled"
        );
        // The same text settling again changes nothing to write.
        b.changed("s1", "hello", t0 + secs(5));
        assert!(!b.settle(t0 + secs(10)), "rewrote an unchanged autosave");
    }

    #[test]
    fn emptied_box_drops_its_autosave() {
        let t0 = t0();
        let mut b = Book::new(Store::default(), 50);
        b.changed("", "draft in the prompt", t0);
        b.settle(t0 + DEBOUNCE);
        b.changed("", "", t0 + secs(3));
        assert!(
            b.settle(t0 + secs(3) + DEBOUNCE),
            "emptying the box should drop the autosave"
        );
        assert!(!b.store.autosave.contains_key(""));
    }

    #[test]
    fn leaving_keeps_unsettled_text() {
        let t0 = t0();
        let mut b = Book::new(Store::default(), 50);
        b.changed("s1", "half a thought", t0);
        // Left before it settled: the pending text is kept anyway.
        assert!(b.keep("s1", "", t0 + Duration::from_millis(100)));
        assert_eq!(b.count("s1"), 1);
        assert_eq!(b.store.drafts["s1"][0].text, "half a thought");
        assert!(b.next_settle().is_none() && b.store.autosave.is_empty());
        // Leaving an empty box keeps nothing.
        assert!(
            !b.keep("s2", "", t0),
            "kept a draft for a box never typed in"
        );
    }

    #[test]
    fn cleared_text_wins_over_pending() {
        let t0 = t0();
        let mut b = Book::new(Store::default(), 50);
        b.changed("s1", "old", t0);
        b.keep("s1", "what was cleared", t0 + secs(1));
        assert_eq!(b.store.drafts["s1"][0].text, "what was cleared");
    }

    #[test]
    fn sent_drops_the_pending_autosave() {
        let t0 = t0();
        let mut b = Book::new(Store::default(), 50);
        b.changed("s1", "going now", t0);
        b.settle(t0 + DEBOUNCE);
        b.changed("s1", "going now!", t0 + secs(2));
        assert!(b.sent("s1"), "sending should drop the autosave");
        assert!(!b.settle(t0 + secs(3600)) && b.store.autosave.is_empty());
        assert!(
            !b.keep("s1", "", t0 + secs(3600)),
            "leaving after sending kept a draft"
        );
    }

    #[test]
    fn drafts_are_capped_per_box() {
        let t0 = t0();
        let mut b = Book::new(Store::default(), 10);
        for i in 0..15u8 {
            b.keep(
                "s1",
                &char::from(b'a' + i).to_string(),
                t0 + secs(u64::from(i)),
            );
        }
        b.keep("s2", "other box", t0);
        assert_eq!(b.count("s1"), 10);
        assert_eq!(b.store.drafts["s1"][0].text, "f");
        assert_eq!(b.store.drafts["s1"][9].text, "o");
        assert_eq!(b.count("s2"), 1, "one box's cap reached into another");
        // Lowering the setting trims what's kept; raising it trims nothing.
        assert!(!b.set_keep(50), "raising keep dropped drafts");
        let mut b2 = Book::new(b.store.clone(), 10);
        for i in 0..5u8 {
            b2.keep("s1", &char::from(b'A' + i).to_string(), t0);
        }
        assert!(b2.set_keep(3));
        assert_eq!(b2.count("s1"), 3);
        assert_eq!(b2.store.drafts["s1"][2].text, "E");
    }

    #[test]
    fn same_draft_twice_is_kept_once() {
        let t0 = t0();
        let mut b = Book::new(Store::default(), 50);
        b.keep("s1", "same", t0);
        b.keep("s1", "same", t0 + secs(60));
        assert_eq!(b.count("s1"), 1);
        assert_eq!(b.store.drafts["s1"][0].at, t0 + secs(60));
    }

    #[test]
    fn restore_walks_back_then_falls_back_to_autosave() {
        let t0 = t0();
        let mut b = Book::new(Store::default(), 50);
        b.keep("s1", "first", t0);
        b.keep("s1", "second", t0 + secs(1));
        b.changed("s1", "typing when rush quit", t0 + secs(2));
        b.settle(t0 + secs(60));
        for want in ["second", "first", "typing when rush quit"] {
            assert_eq!(b.restore("s1").as_deref(), Some(want));
        }
        assert_eq!(b.restore("s1"), None, "restored from an empty box");
        assert_eq!(b.restore(""), None, "the Prompt has no drafts of s1's");
    }

    #[test]
    fn reads_keep() {
        for (v, want) in [
            ("10", 10),
            ("200", 200),
            ("", DEFAULT_KEEP),
            ("x", DEFAULT_KEEP),
            ("-3", DEFAULT_KEEP),
        ] {
            assert_eq!(parse_keep(v), want, "{v:?}");
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("autodrafts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("drafts.json");
        assert!(
            load(&path).drafts.is_empty(),
            "a missing file should load empty"
        );
        let mut b = Book::new(Store::default(), 50);
        b.keep("", "prompt draft", t0());
        save(&path, &b.store).unwrap();
        let mut got = Book::new(load(&path), 50);
        assert_eq!(got.restore("").as_deref(), Some("prompt draft"));
        assert_eq!(load(&path).drafts[""][0].at, t0());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let left = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(left, 1, "left files behind; want only drafts.json");
        std::fs::write(&path, "{broken").unwrap();
        assert!(
            load(&path).drafts.is_empty(),
            "a broken file should load empty"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
