//! Who is in a conversation besides its lead: guests.json in the lead's
//! folder, each guest's session and how much of the chat it has been
//! told, what the lead has been told, and the asks between agents since
//! you last spoke. Changed only under its lock, by whichever process asks
//! (a host, the UI, `rush mcp-tools`). Rust-only.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::paths::dir;
use crate::wire::zero_time;

/// One guest of a conversation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Guest {
    /// Its name in the chat: "Kimi".
    pub name: String,
    /// What tags it: "kimi".
    pub handle: String,
    /// Its rush session's id.
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Its model's context window in tokens, when known.
    #[serde(skip_serializing_if = "is_zero")]
    pub context: i64,
    /// The chat it has been told, up to and including the entry of this
    /// time.
    #[serde(skip_serializing_if = "zero_time")]
    pub seen: Time,
}

/// A conversation's guests.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roster {
    /// The lead's name in the chat: "Claude".
    pub lead: String,
    pub guests: Vec<Guest>,
    /// The chat the lead has been told of, up to this time.
    #[serde(skip_serializing_if = "zero_time")]
    pub lead_seen: Time,
    /// Asks between agents since you last spoke.
    #[serde(skip_serializing_if = "is_zero")]
    pub hops: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Roster {
    /// The guest handle names (as typed, any case).
    pub fn find(&self, handle: &str) -> Option<&Guest> {
        let h = crate::agentmatch::clean_name(handle);
        self.guests.iter().find(|g| g.handle == h)
    }

    /// The guest whose session is id.
    pub fn by_id(&self, id: &str) -> Option<&Guest> {
        self.guests.iter().find(|g| g.id == id)
    }

    /// Everyone in the chat by name, the lead first.
    pub fn names(&self) -> Vec<String> {
        let mut out = vec![self.lead.clone()];
        out.extend(self.guests.iter().map(|g| g.name.clone()));
        out
    }
}

/// Where lead's guests are kept.
pub fn path(lead: &str) -> PathBuf {
    dir(lead).join("guests.json")
}

/// Lead's guests; none when it has none.
pub fn load(lead: &str) -> Roster {
    std::fs::read(path(lead))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Holds f's file (`<f>.lock` beside it) for as long as it's kept.
pub(crate) struct Locked(#[allow(dead_code)] File);

/// Takes the lock beside file p, waiting for it.
pub(crate) fn lock(p: &std::path::Path) -> std::io::Result<Locked> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut name = p.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(p.with_file_name(name))?;
    // SAFETY: flock on a descriptor f owns; the lock goes with it.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Locked(f))
}

/// Changes lead's guests with f, under their lock, and says what f said.
pub fn update<T>(lead: &str, f: impl FnOnce(&mut Roster) -> T) -> std::io::Result<T> {
    let p = path(lead);
    let _l = lock(&p)?;
    let mut r = load(lead);
    let out = f(&mut r);
    let b = serde_json::to_vec_pretty(&r).map_err(std::io::Error::other)?;
    crate::json::write_private(&p, &b)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_and_found() {
        let lead = "guests-roster-lead";
        assert_eq!(load(lead), Roster::default());
        update(lead, |r| {
            r.lead = "Claude".into();
            r.guests.push(Guest {
                name: "Kimi".into(),
                handle: "kimi".into(),
                id: "g1".into(),
                kind: "kimi".into(),
                ..Guest::default()
            });
            r.hops = 2;
        })
        .unwrap();
        let r = load(lead);
        assert_eq!(r.find("KIMI").map(|g| g.id.as_str()), Some("g1"));
        assert_eq!(r.find("sol"), None);
        assert_eq!(r.by_id("g1").map(|g| g.name.as_str()), Some("Kimi"));
        assert_eq!(r.names(), vec!["Claude".to_string(), "Kimi".to_string()]);
        assert_eq!(r.hops, 2);
    }
}
