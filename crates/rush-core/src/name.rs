//! A short string that comes from a small set of values and is kept many
//! times over: a tool's name, a model's, a folder a session works in, an
//! image's media type. Rust-only (Go's strings share their bytes already).
//!
//! A Name known ahead (KNOWN) is a `&'static str` and allocates nothing;
//! any other is shared through one process-wide table, so each distinct
//! value is allocated once however many hold it. Only values from a small
//! set belong in a Name: the table keeps every one it was handed.
//!
//! It is as small as a `Box<str>`, 16 bytes (a String is 24): the str's
//! pointer and length, the length's top bit saying the pointer is an
//! `Arc<str>`'s (Arc::into_raw) rather than a static's.

use std::borrow::Borrow;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A string from a small set of values: cheap to copy, compared and
/// hashed as the str it is.
pub struct Name {
    ptr: NonNull<u8>,
    /// The str's length; SHARED set when ptr is an Arc<str>'s.
    len: usize,
}

/// The length's bit saying the str is an Arc's.
const SHARED: usize = 1 << (usize::BITS - 1);

// SAFETY: a Name is an immutable str, either 'static or held by an
// Arc<str>, both of which are Send and Sync.
unsafe impl Send for Name {}
// SAFETY: as above.
unsafe impl Sync for Name {}

/// The values known ahead: the tools Claude Code calls, the levels a
/// notice comes at, the media types of the images it sends, the roles of
/// a message.
const KNOWN: &[&str] = &[
    "",
    "Agent",
    "AskUserQuestion",
    "Bash",
    "BashOutput",
    "Edit",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "Glob",
    "Grep",
    "KillShell",
    "ListAgents",
    "LS",
    "Monitor",
    "MultiEdit",
    "NotebookEdit",
    "PushNotification",
    "Read",
    "SendMessage",
    "Skill",
    "SlashCommand",
    "SubagentHandback",
    "Task",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TaskUpdate",
    "TodoWrite",
    "ToolSearch",
    "WebFetch",
    "WebSearch",
    "Write",
    "info",
    "warning",
    "error",
    "ok",
    "user",
    "assistant",
    "tool",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
];

/// Every Name not known ahead, by its value.
static TABLE: LazyLock<Mutex<HashSet<Arc<str>>>> = LazyLock::new(Default::default);

impl Name {
    /// The empty name.
    pub const EMPTY: Name = Name::known("");

    /// A name known ahead, allocating nothing.
    pub const fn known(s: &'static str) -> Name {
        Name {
            // SAFETY: a str's pointer is never null, even an empty one's.
            ptr: unsafe { NonNull::new_unchecked(s.as_ptr() as *mut u8) },
            len: s.len(),
        }
    }

    /// The Name holding a, sharing it.
    fn shared(a: Arc<str>) -> Name {
        let len = a.len();
        let p = Arc::into_raw(a) as *const u8 as *mut u8;
        Name {
            // SAFETY: Arc::into_raw never gives null.
            ptr: unsafe { NonNull::new_unchecked(p) },
            len: len | SHARED,
        }
    }

    /// The Arc<str> this Name holds, as Arc::into_raw gave it.
    fn raw(&self) -> *const str {
        std::ptr::slice_from_raw_parts(self.ptr.as_ptr() as *const u8, self.len & !SHARED)
            as *const str
    }

    /// The Name for s: its static copy when it's known ahead, else the one
    /// copy every holder of the same value shares.
    pub fn new(s: &str) -> Name {
        if let Some(k) = KNOWN.iter().find(|k| **k == s) {
            return Name::known(k);
        }
        let mut t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = t.get(s) {
            return Name::shared(a.clone());
        }
        let a: Arc<str> = Arc::from(s);
        t.insert(a.clone());
        Name::shared(a)
    }

    pub fn as_str(&self) -> &str {
        // SAFETY: ptr and len are a str's, kept alive by 'static or by the
        // Arc this Name holds a count of.
        unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                self.ptr.as_ptr(),
                self.len & !SHARED,
            ))
        }
    }

    /// Whether it allocates nothing of its own (known ahead).
    pub fn is_static(&self) -> bool {
        self.len & SHARED == 0
    }
}

impl Clone for Name {
    fn clone(&self) -> Name {
        if !self.is_static() {
            // SAFETY: raw is an Arc<str>'s from into_raw, alive while self is.
            unsafe { Arc::increment_strong_count(self.raw()) };
        }
        Name {
            ptr: self.ptr,
            len: self.len,
        }
    }
}

impl Drop for Name {
    fn drop(&mut self) {
        if !self.is_static() {
            // SAFETY: raw is an Arc<str>'s from into_raw; this Name holds
            // one of its counts, given back here once.
            unsafe { Arc::decrement_strong_count(self.raw()) };
        }
    }
}

impl Default for Name {
    fn default() -> Name {
        Name::EMPTY
    }
}

impl Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for Name {
    fn eq(&self, o: &Name) -> bool {
        self.as_str() == o.as_str()
    }
}

impl Eq for Name {}

impl PartialEq<str> for Name {
    fn eq(&self, o: &str) -> bool {
        self.as_str() == o
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, o: &&str) -> bool {
        self.as_str() == *o
    }
}

impl PartialEq<String> for Name {
    fn eq(&self, o: &String) -> bool {
        self.as_str() == o
    }
}

impl PartialEq<Name> for String {
    fn eq(&self, o: &Name) -> bool {
        self == o.as_str()
    }
}

impl PartialEq<Name> for str {
    fn eq(&self, o: &Name) -> bool {
        self == o.as_str()
    }
}

impl PartialEq<Name> for &str {
    fn eq(&self, o: &Name) -> bool {
        *self == o.as_str()
    }
}

impl PartialOrd for Name {
    fn partial_cmp(&self, o: &Name) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Name {
    fn cmp(&self, o: &Name) -> std::cmp::Ordering {
        self.as_str().cmp(o.as_str())
    }
}

impl Hash for Name {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_str().hash(h)
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Name {
        Name::new(s)
    }
}

impl From<&String> for Name {
    fn from(s: &String) -> Name {
        Name::new(s)
    }
}

impl From<String> for Name {
    fn from(s: String) -> Name {
        Name::new(&s)
    }
}

impl From<Name> for String {
    fn from(n: Name) -> String {
        n.as_str().to_owned()
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Name, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Name;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Name, E> {
                Ok(Name::new(s))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Name, E> {
                Ok(Name::EMPTY)
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        // (value, known ahead)
        let cases = [
            ("Bash", true),
            ("", true),
            ("image/png", true),
            ("mcp__linear__get_issue", false),
            ("claude-opus-4-7", false),
        ];
        for (s, known) in cases {
            let a = Name::new(s);
            let b = Name::from(s.to_string());
            assert_eq!(a, b);
            assert_eq!(a.as_str(), s);
            assert_eq!(a.is_static(), known, "{s}");
            assert_eq!(a.as_str().as_ptr(), b.as_str().as_ptr(), "{s} is shared");
            let c = a.clone();
            drop(a);
            assert_eq!(c.as_str(), s);
            let j = serde_json::to_string(&c).unwrap();
            assert_eq!(j, serde_json::to_string(s).unwrap());
            assert_eq!(serde_json::from_str::<Name>(&j).unwrap(), c);
        }
        assert_eq!(serde_json::from_str::<Name>("null").unwrap(), Name::EMPTY);
        assert_eq!(size_of::<Name>(), 16);
        assert_eq!(size_of::<Option<Name>>(), 16);
    }
}
