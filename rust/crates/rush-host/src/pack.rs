//! The replay ring is most of a long-lived host's memory: whole messages
//! and tool results, kept for a client that connects later. They're only
//! read back on a connect, so the big ones are kept deflated, which about
//! halves them (thinking signatures don't compress). The ring still counts
//! every line at its full size, so what it keeps and replays is unchanged.
//!
//! Ported from internal/host/pack.go.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;

use crate::lines::is_stream_event;

/// The shortest line worth deflating.
const PACK_MIN: usize = 2 << 10;

/// Marks a deflated ring line; a line as an agent or the host wrote it
/// starts with '{'.
pub const PACKED: u8 = 0;

/// line as the ring keeps it: deflated when it's one of the agent's big
/// lines, else as it is. Streamed deltas are dropped from the ring soon and
/// the host's own lines are looked at again, so those stay as they are.
pub fn pack(line: &[u8]) -> Vec<u8> {
    if line.len() < PACK_MIN || !line.starts_with(br#"{"type":""#) || is_stream_event(line) {
        return line.to_vec();
    }
    let mut out = vec![PACKED];
    put_uvarint(&mut out, line.len() as u64);
    let mut z = DeflateEncoder::new(out, Compression::fast());
    if z.write_all(line).is_err() {
        return line.to_vec();
    }
    match z.finish() {
        Ok(b) if b.len() < line.len() => b,
        _ => line.to_vec(),
    }
}

/// The length of a ring line as it was written.
pub fn line_len(l: &[u8]) -> usize {
    if l.first() != Some(&PACKED) {
        return l.len();
    }
    uvarint(&l[1..]).0 as usize
}

/// Ring line l as it was written.
pub fn unpack(l: &[u8], out: &mut Vec<u8>) -> std::io::Result<()> {
    if l.first() != Some(&PACKED) {
        out.extend_from_slice(l);
        return Ok(());
    }
    let (n, k) = uvarint(&l[1..]);
    out.reserve(n as usize);
    DeflateDecoder::new(&l[1 + k..]).read_to_end(out)?;
    Ok(())
}

fn put_uvarint(b: &mut Vec<u8>, mut x: u64) {
    while x >= 0x80 {
        b.push(x as u8 | 0x80);
        x >>= 7;
    }
    b.push(x as u8);
}

/// A uvarint and how many bytes it took, as Go's binary.Uvarint.
fn uvarint(b: &[u8]) -> (u64, usize) {
    let mut x = 0u64;
    let mut s = 0;
    for (i, &c) in b.iter().enumerate().take(10) {
        if c < 0x80 {
            return (x | (c as u64) << s, i + 1);
        }
        x |= ((c & 0x7f) as u64) << s;
        s += 7;
    }
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // pack_test.go:11: big lines are kept deflated and come back out as
    // they went in; small ones, deltas and the host's own lines as they are.
    #[test]
    fn ring_packing() {
        let big = format!(
            r#"{{"type":"user","message":{{"content":"{}"}}}}"#,
            "some file contents ".repeat(2000)
        );
        let lines = [
            big.clone(),
            r#"{"type":"assistant","message":{"content":"short"}}"#.to_string(),
            format!(
                r#"{{"type":"stream_event","event":{{"text":"{}"}}}}"#,
                "x".repeat(4000)
            ),
            format!(
                r#"{{"agtop_sent":true,"message":{{"content":"{}"}}}}"#,
                "y".repeat(4000)
            ),
            big,
        ];
        for (i, l) in lines.iter().enumerate() {
            let k = pack(l.as_bytes());
            let packs = i == 0 || i == 4;
            assert_eq!(k[0] == PACKED, packs, "line {i}");
            if packs {
                assert!(k.len() < l.len() / 4, "line {i} packed to {}", k.len());
            }
            assert_eq!(line_len(&k), l.len(), "line {i}");
            let mut out = Vec::new();
            unpack(&k, &mut out).unwrap();
            assert_eq!(out, l.as_bytes(), "line {i}");
        }
    }
}
