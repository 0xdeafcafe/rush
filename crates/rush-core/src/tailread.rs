//! Rust-only: a JSONL file read from its end backwards, a chunk at a time,
//! for a long conversation's last screens without reading its start.
//!
//! Lines are split on `\n` alone, which never appears inside a UTF-8
//! character, so a chunk edge falling mid-character or mid-line only
//! means reading the chunk before it too. A last line with no `\n` yet is
//! one still being written: it's left out, for whoever follows the file
//! to read once it's whole. A `\r` before the `\n` (CRLF) is dropped.

use std::io::{self, Read, Seek, SeekFrom};

/// The first chunk read from the end; each further one, while a line is
/// still unfinished, twice the last, so a line of any length costs
/// reading it about twice.
const CHUNK: usize = 64 << 10;
/// The largest chunk read at once.
const MOST_CHUNK: usize = 8 << 20;

/// Whole lines of a file, handed out from its last backwards.
pub struct BackLines<R> {
    r: R,
    /// The file's offset of buf's first byte: what's before it is unread.
    lo: u64,
    /// Bytes lo.. read and not handed out yet; it always ends with the
    /// `\n` of the next line to hand out, or is empty.
    buf: Vec<u8>,
    /// Where the whole lines end: after the last `\n` before the end
    /// asked for.
    whole: u64,
    chunk: usize,
    /// The chunk read first, and again once a line is found.
    base: usize,
}

impl<R: Read + Seek> BackLines<R> {
    /// The whole lines of r's first `end` bytes (its length, usually),
    /// from the last.
    pub fn new(r: R, end: u64) -> io::Result<BackLines<R>> {
        BackLines::with_chunk(r, end, CHUNK)
    }

    /// As new, reading `chunk` bytes at a time to start with (tests use
    /// tiny ones to put the chunk edges everywhere).
    pub fn with_chunk(r: R, end: u64, chunk: usize) -> io::Result<BackLines<R>> {
        let mut b = BackLines {
            r,
            lo: end,
            buf: Vec::new(),
            whole: 0,
            chunk: chunk.max(1),
            base: chunk.max(1),
        };
        // Back to the last `\n`: what's after it is still being written.
        loop {
            if let Some(i) = memchr::memrchr(b'\n', &b.buf) {
                b.buf.truncate(i + 1);
                b.whole = b.lo + i as u64 + 1;
                break;
            }
            if b.lo == 0 {
                b.buf.clear();
                break; // no whole line at all
            }
            b.read_before()?;
        }
        b.chunk = b.base;
        Ok(b)
    }

    /// Where the whole lines end: after the last `\n`. 0 when there's none.
    pub fn whole_end(&self) -> u64 {
        self.whole
    }

    /// Where the next line to hand out starts, without its bytes: the
    /// line before the last one handed out. None at the file's start.
    pub fn next_start(&mut self) -> io::Result<Option<u64>> {
        let Some((at, from)) = self.next_at()? else {
            return Ok(None);
        };
        self.buf.truncate(from);
        Ok(Some(at))
    }

    /// The next line back, without its `\n` (or `\r\n`), and where it
    /// starts in the file. None once the first line has been handed out.
    pub fn next_line(&mut self) -> io::Result<Option<(u64, Vec<u8>)>> {
        let Some((at, from)) = self.next_at()? else {
            return Ok(None);
        };
        let mut line = self.buf[from..].to_vec();
        self.buf.truncate(from);
        line.pop(); // its \n
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Ok(Some((at, line)))
    }

    /// The next line back: where it starts in the file and in buf, left
    /// in buf for the caller to take or drop.
    fn next_at(&mut self) -> io::Result<Option<(u64, usize)>> {
        if self.buf.is_empty() {
            return Ok(None);
        }
        // The `\n` that ends the line before this one.
        let mut searched = self.buf.len() - 1;
        loop {
            if let Some(i) = memchr::memrchr(b'\n', &self.buf[..searched]) {
                let at = self.lo + i as u64 + 1;
                self.chunk = self.base;
                return Ok(Some((at, i + 1)));
            }
            if self.lo == 0 {
                return Ok(Some((0, 0))); // the file's first line
            }
            let n = self.read_before()?;
            searched += n;
        }
    }

    /// Reads the chunk before lo into buf's front; how many bytes.
    fn read_before(&mut self) -> io::Result<usize> {
        let n = (self.chunk as u64).min(self.lo) as usize;
        let from = self.lo - n as u64;
        let mut b = vec![0u8; n + self.buf.len()];
        self.r.seek(SeekFrom::Start(from))?;
        self.r.read_exact(&mut b[..n])?;
        b[n..].copy_from_slice(&self.buf);
        self.buf = b;
        self.lo = from;
        // A line longer than this chunk reads the next twice as big.
        self.chunk = (self.chunk * 2).min(MOST_CHUNK);
        Ok(n)
    }
}

/// Where a file's tail of whole lines starts and ends: the whole lines in
/// its last `most` bytes, back from the last `\n`; a line crossing that
/// mark is left out, as a window cut there would drop it, but the last
/// line is always in it, however long. A last line still being written
/// (no `\n` yet) is left out: `end` is where the whole lines end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TailSpan {
    pub start: u64,
    pub end: u64,
}

impl TailSpan {
    /// Whether it leaves some of the file's start out.
    pub fn cut(&self) -> bool {
        self.start > 0
    }

    /// Its length in bytes.
    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

/// The tail of r's first `size` bytes as TailSpan says; reads only the
/// tail (and the start of the line crossing its mark), from the end.
pub fn tail_span<R: Read + Seek>(r: R, size: u64, most: u64) -> io::Result<TailSpan> {
    tail_span_chunked(r, size, most, CHUNK)
}

/// tail_span with the chunk to read first (for tests).
pub fn tail_span_chunked<R: Read + Seek>(
    r: R,
    size: u64,
    most: u64,
    chunk: usize,
) -> io::Result<TailSpan> {
    let most = most.max(1);
    let mut b = BackLines::with_chunk(r, size, chunk)?;
    let end = b.whole_end();
    let mut start = end;
    while let Some(at) = b.next_start()? {
        if end - at > most && start < end {
            break; // the line crossing the mark: left out
        }
        start = at;
        if end - at >= most {
            break;
        }
    }
    Ok(TailSpan { start, end })
}

/// The tail of the file at path, as tail_span: its span, and the file
/// open, for reading it.
pub fn file_tail(path: &std::path::Path, most: u64) -> io::Result<(std::fs::File, TailSpan)> {
    crate::offui::check("a file's tail read");
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let span = tail_span(&mut f, size, most)?;
    Ok((f, span))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn all_back(text: &[u8], chunk: usize) -> Vec<(u64, String)> {
        let mut b = BackLines::with_chunk(Cursor::new(text), text.len() as u64, chunk).unwrap();
        let mut out = Vec::new();
        while let Some((at, l)) = b.next_line().unwrap() {
            out.push((at, String::from_utf8(l).unwrap()));
        }
        out
    }

    /// Every line, from the last, at every chunk size: lines across chunk
    /// edges, multi-byte characters cut by them, CRLF, empty lines.
    #[test]
    fn lines_back_at_any_chunk() {
        let text = "{\"a\":1}\n\nnaïve ✓ 日本語 🦀\r\n{\"long\":\"".to_string()
            + &"é".repeat(300)
            + "\"}\nlast\n";
        let mut want: Vec<(u64, String)> = Vec::new();
        let mut at = 0u64;
        for l in text.split_inclusive('\n') {
            let body = l.trim_end_matches('\n').trim_end_matches('\r');
            want.push((at, body.to_string()));
            at += l.len() as u64;
        }
        want.reverse();
        for chunk in [1, 2, 3, 5, 7, 64, 1000, 1 << 20] {
            assert_eq!(all_back(text.as_bytes(), chunk), want, "chunk {chunk}");
        }
    }

    /// A last line still being written is left out, whatever its length;
    /// a file with no whole line has none.
    #[test]
    fn a_line_being_written_waits() {
        for chunk in [1, 4, 64] {
            let b = BackLines::with_chunk(Cursor::new(b"one\ntwo\nthr"), 11, chunk).unwrap();
            assert_eq!(b.whole_end(), 8);
            assert_eq!(
                all_back(b"one\ntwo\n{\"half\":\"written", chunk),
                vec![(4, "two".into()), (0, "one".into())]
            );
            assert!(all_back(b"no newline yet", chunk).is_empty());
            assert!(all_back(b"", chunk).is_empty());
        }
    }

    /// Only the bytes before `end` are read: a file growing past it, or a
    /// cutoff, is read as if it ended there.
    #[test]
    fn reads_only_to_end() {
        let text = b"a\nb\nc\nd\n";
        let mut b = BackLines::with_chunk(Cursor::new(&text[..]), 4, 1).unwrap();
        assert_eq!(b.next_line().unwrap(), Some((2, b"b".to_vec())));
        assert_eq!(b.next_line().unwrap(), Some((0, b"a".to_vec())));
        assert_eq!(b.next_line().unwrap(), None);
    }

    /// The tail starts at a line's start: the line crossing the mark is
    /// left out; the last line is always in.
    #[test]
    fn tail_span_lands_on_a_line() {
        // Lines of 10 bytes each (9 + \n), then one being written.
        let text = "000000000\n111111111\n222222222\n333333333\npartial";
        let t = text.as_bytes();
        let n = t.len() as u64;
        for chunk in [1, 3, 16, 1 << 16] {
            let span = |most| tail_span_chunked(Cursor::new(t), n, most, chunk).unwrap();
            // Exactly two lines.
            assert_eq!(span(20), TailSpan { start: 20, end: 40 });
            // Mid-line: the line crossing it left out.
            assert_eq!(span(15), TailSpan { start: 30, end: 40 });
            // More than there is: the whole file, uncut.
            let all = span(1000);
            assert_eq!(all, TailSpan { start: 0, end: 40 });
            assert!(!all.cut());
            assert!(span(15).cut());
        }
        // A long line crossing the mark: left out.
        let long = format!("{}\nshort\n", "x".repeat(100));
        let l = long.as_bytes();
        let s = tail_span_chunked(Cursor::new(l), l.len() as u64, 10, 4).unwrap();
        assert_eq!(
            s,
            TailSpan {
                start: 101,
                end: 107
            }
        );
        // ...unless it's the last line: then it's all there is.
        let s = tail_span_chunked(Cursor::new(&l[..101]), 101, 10, 4).unwrap();
        assert_eq!(s, TailSpan { start: 0, end: 101 });
        // Nothing whole yet.
        let s = tail_span_chunked(Cursor::new(&b"abc"[..]), 3, 10, 4).unwrap();
        assert!(s.is_empty());
    }

    /// CRLF files land on the line after the \r\n, not between them.
    #[test]
    fn tail_span_crlf() {
        let t = b"one\r\ntwo\r\nthree\r\n";
        let s = tail_span_chunked(Cursor::new(&t[..]), t.len() as u64, 7, 2).unwrap();
        assert_eq!(s, TailSpan { start: 10, end: 17 });
        assert_eq!(&t[s.start as usize..s.end as usize], b"three\r\n");
    }

    /// A tail read from a real file, while it grows: the line being
    /// written is left out, then counted once whole.
    #[test]
    fn file_tail_while_written() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all("{\"n\":1}\n{\"n\":2,\"s\":\"ü".as_bytes())
            .unwrap();
        let (_, s) = file_tail(&p, 1 << 20).unwrap();
        assert_eq!(s, TailSpan { start: 0, end: 8 });
        f.write_all("\"}\n".as_bytes()).unwrap();
        let (_, s) = file_tail(&p, 1 << 20).unwrap();
        assert_eq!(s.end, std::fs::metadata(&p).unwrap().len());
    }
}
