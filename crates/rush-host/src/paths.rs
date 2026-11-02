//! Where a session's host keeps its files: one folder per session under
//! rush's folder's sessions, its socket and its ids.
//!
//! Ported from internal/host/host.go:358-394 (Root, dir, SockPath,
//! NewSessionID), client.go:154-172 (ShortID, shortOf), events.go:18
//! (EventsPath) and host.go:1220 (movedPath).

use std::path::PathBuf;

/// Holds one folder per session.
pub fn root() -> PathBuf {
    rush_state::dir().join("sessions")
}

/// Session id's folder.
pub fn dir(id: &str) -> PathBuf {
    root().join(id)
}

/// Session id's host's socket.
pub fn sock_path(id: &str) -> PathBuf {
    dir(id).join("host.sock")
}

/// Where a session's events are kept.
pub fn events_path(id: &str) -> PathBuf {
    dir(id).join("events.jsonl")
}

/// The mark MarkMoved leaves: sessions started on their login before it
/// move to the login in use when they next start.
pub fn moved_path() -> PathBuf {
    rush_state::dir().join("moved")
}

/// n random bytes.
pub(crate) fn rand_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // SAFETY: b is N writable bytes, and getentropy takes at most 256.
    let r = unsafe { libc::getentropy(b.as_mut_ptr().cast(), N) };
    if r != 0 {
        // No entropy to be had: the clock and pid, mixed, are unique enough
        // for an id.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            ^ ((std::process::id() as u128) << 64);
        for (i, x) in b.iter_mut().enumerate() {
            *x = (t >> ((i % 16) * 8)) as u8;
        }
    }
    b
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A fresh conversation id and the short id derived from it, the way
/// Claude Code pairs them.
pub fn new_session_id() -> (String, String) {
    let mut b = rand_bytes::<16>();
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h = hex(&b);
    let session = format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    );
    (session, h[0..8].to_string())
}

/// The id a new session started on session_id gets: its folder, socket
/// and the name rush lists it by.
pub fn short_id(session_id: &str) -> String {
    session_id.chars().filter(|&c| c != '-').take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_pair_as_claude_codes_do() {
        let (s, short) = new_session_id();
        assert_eq!(s.len(), 36);
        assert_eq!(short.len(), 8);
        assert!(s.replace('-', "").starts_with(&short));
        assert_eq!(&s[14..15], "4");
        assert_eq!(short_id(&s), short);
        assert_eq!(short_id("ab-cd"), "abcd");
    }
}
