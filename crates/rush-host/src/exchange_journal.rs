//! A session's exchanges.jsonl: the messages agents sent each other
//! through it, kept so a client connecting later is replayed them, bounded
//! in bytes and records.
//!
//! Ported from internal/host/exchange_journal.go.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

use rush_core::event::Exchange;

use crate::paths::dir;

pub const JOURNAL_BYTES: u64 = 8 << 20;
const JOURNAL_RECORDS: usize = 2048;

fn path(id: &str) -> std::path::PathBuf {
    dir(id).join("exchanges.jsonl")
}

/// Reads only this session's bounded exchange journal. Call from a host or
/// a background reader, never a UI thread.
pub fn read_exchanges(id: &str) -> Vec<Exchange> {
    let Ok(mut f) = std::fs::File::open(path(id)) else {
        return Vec::new();
    };
    let Ok(st) = f.metadata() else {
        return Vec::new();
    };
    let offset = st.len().saturating_sub(JOURNAL_BYTES);
    if offset > 0 && f.seek(SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut r = BufReader::new(f.take(JOURNAL_BYTES));
    let mut line = Vec::new();
    if offset > 0 {
        let _ = r.read_until(b'\n', &mut line); // a partial oldest entry
    }
    let mut out = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let l = line.strip_suffix(b"\n").unwrap_or(&line);
        if let Ok(mut e) = serde_json::from_slice::<Exchange>(l)
            && !e.id.is_empty()
        {
            e.archived = true;
            out.push(e);
        }
    }
    if out.len() > JOURNAL_RECORDS {
        out.drain(..out.len() - JOURNAL_RECORDS);
    }
    out
}

/// Appends e to the journal; it runs under the owning host's lock. Entries
/// are appended once; now and then the journal is compacted, atomically,
/// to bound it without cutting an entry.
pub fn append_exchange(id: &str, e: &Exchange) -> std::io::Result<()> {
    if id.is_empty() || e.id.is_empty() {
        return Ok(());
    }
    let p = path(id);
    let b = crate::json::line(e);
    // A message larger than the journal's budget keeps its own transcript:
    // it doesn't make every later attachment load unbounded.
    if b.len() as u64 > JOURNAL_BYTES {
        return Ok(());
    }
    if let Ok(st) = std::fs::metadata(&p)
        && st.len() + b.len() as u64 + 2 > JOURNAL_BYTES
    {
        let records = read_exchanges(id);
        let mut kept: Vec<Vec<u8>> = Vec::new();
        let mut size = b.len() + 1;
        for mut r in records.into_iter().rev() {
            r.archived = false;
            let line = crate::json::line(&r);
            if (size + line.len() + 1) as u64 > JOURNAL_BYTES {
                break;
            }
            size += line.len() + 1;
            kept.push(line);
        }
        let mut out = Vec::with_capacity(size);
        for l in kept.iter().rev() {
            out.extend_from_slice(l);
            out.push(b'\n');
        }
        out.extend_from_slice(&b);
        out.push(b'\n');
        return crate::json::write_atomic(&p, &out);
    }
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&p)?;
    let mut line = Vec::with_capacity(b.len() + 2);
    line.push(b'\n');
    line.extend_from_slice(&b);
    line.push(b'\n');
    f.write_all(&line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    // exchange_journal_test.go:12
    #[test]
    fn journal_compacts_and_tolerates_a_truncated_tail() {
        let _h = Home::new();
        let id = "journal";
        std::fs::create_dir_all(dir(id)).unwrap();
        let text = "x".repeat(1 << 20);
        for i in 0..12u8 {
            let e = Exchange {
                id: ((b'a' + i) as char).to_string(),
                text: text.clone(),
                at: rush_core::time::now(),
                direction: "sent".into(),
                ..Exchange::default()
            };
            append_exchange(id, &e).unwrap();
        }
        let p = path(id);
        assert!(
            std::fs::metadata(&p).unwrap().len() <= JOURNAL_BYTES,
            "unbounded journal"
        );
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(br#"{"ID":"#).unwrap();
        drop(f);
        let got = read_exchanges(id);
        assert!(!got.is_empty());
        let last = got.last().unwrap();
        assert!(
            last.id == "l" && last.text == text && got[0].archived,
            "lost complete records after compaction/truncated write"
        );
    }

    #[test]
    fn reads_what_go_writes() {
        let _h = Home::new();
        std::fs::create_dir_all(dir("j")).unwrap();
        // Its time as Go writes it on this machine: in its zone.
        let go = include_str!("../tests/fixtures/exchange.json");
        let go = rush_core::wire::in_local_zone(go, "At").into_bytes();
        let mut journal = b"\n".to_vec();
        journal.extend_from_slice(&go);
        journal.push(b'\n');
        std::fs::write(path("j"), &journal).unwrap();
        let got = read_exchanges("j");
        assert_eq!(got.len(), 1);
        let mut e = got[0].clone();
        e.archived = false;
        assert_eq!(crate::json::line(&e), go);
    }
}
