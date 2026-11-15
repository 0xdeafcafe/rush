//! What's said in a conversation with guests: chat.jsonl in the lead's
//! folder, a room's log entry a line (rush_room::room::Entry), so a
//! guest's turn reads it as a room member reads its room
//! (rush_room::seen). The lead's host and each guest's write it; every
//! write is under its lock and stamped then, so the lines are in time
//! order. Past MAX bytes it's cut to its last KEEP. Rust-only.

use std::io::Write;
use std::path::PathBuf;

use rush_room::room::Entry;
use rush_state::Time;

use crate::paths::dir;

/// Past this many bytes the chat is cut down…
pub const MAX: u64 = 512 << 10;
/// …to its last this many.
pub const KEEP: usize = 256 << 10;

/// Where lead's chat is kept.
pub fn path(lead: &str) -> PathBuf {
    dir(lead).join("chat.jsonl")
}

/// Adds e to lead's chat, stamped now, and says when that was.
pub fn append(lead: &str, mut e: Entry) -> std::io::Result<Time> {
    let p = path(lead);
    let _l = super::roster::lock(&p)?;
    e.at = stamp();
    let mut b = serde_json::to_vec(&e).map_err(std::io::Error::other)?;
    b.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)?;
    f.write_all(&b)?;
    if f.metadata().map(|m| m.len()).unwrap_or(0) > MAX {
        let all = std::fs::read(&p)?;
        crate::json::write_private(&p, &keep_end(&all, KEEP))?;
    }
    Ok(e.at)
}

/// Now, after any time this process stamped before: two lines never
/// share a time, so "after" never drops one.
fn stamp() -> Time {
    static LAST: std::sync::Mutex<Option<Time>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let mut t = Time::now();
    if let Some(l) = *last
        && !t.after(l)
    {
        t = l.add_nanos(1);
    }
    *last = Some(t);
    t
}

/// The last whole lines of b, at most most bytes of them.
fn keep_end(b: &[u8], most: usize) -> Vec<u8> {
    if b.len() <= most {
        return b.to_vec();
    }
    let tail = &b[b.len() - most..];
    match tail.iter().position(|&c| c == b'\n') {
        Some(i) => tail[i + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// Lead's chat, oldest first; lines that don't read are passed over.
pub fn read(lead: &str) -> Vec<Entry> {
    let Ok(b) = std::fs::read(path(lead)) else {
        return Vec::new();
    };
    b.split(|&c| c == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_slice(l).ok())
        .collect()
}

/// The entries of es after time t (all of them when t is zero).
pub fn after(es: &[Entry], t: Time) -> &[Entry] {
    if t.is_zero() {
        return es;
    }
    let i = es.iter().position(|e| e.at.after(t)).unwrap_or(es.len());
    &es[i..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_room::room::SAY;

    fn say(from: &str, text: &str) -> Entry {
        Entry {
            from: from.into(),
            kind: SAY.into(),
            text: text.into(),
            ..Entry::default()
        }
    }

    #[test]
    fn kept_in_order_and_read_after_a_time() {
        let lead = "guests-chat-lead";
        let t1 = append(lead, say("Claude", "one")).unwrap();
        let t2 = append(lead, say("Kimi", "two")).unwrap();
        append(lead, say("Sol", "three")).unwrap();
        let es = read(lead);
        let texts = |es: &[Entry]| es.iter().map(|e| e.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&es), ["one", "two", "three"]);
        assert_eq!(texts(after(&es, Time::default())), ["one", "two", "three"]);
        assert_eq!(texts(after(&es, t1)), ["two", "three"]);
        assert_eq!(texts(after(&es, t2)), ["three"]);
    }

    #[test]
    fn cut_to_whole_lines() {
        for (b, most, want) in [
            ("a\nb\nc\n", 100, "a\nb\nc\n"),
            ("a\nb\nc\n", 4, "c\n"),
            ("aaaa\nbb\n", 4, "bb\n"),
            ("aaaaaaaa", 3, ""),
        ] {
            assert_eq!(
                String::from_utf8(keep_end(b.as_bytes(), most)).unwrap(),
                want,
                "{b:?} {most}"
            );
        }
    }
}
