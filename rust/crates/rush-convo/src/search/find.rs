//! Searching a transcript on disk as an open session is searched: one pass
//! over the raw bytes (or its word index) passes over a file that doesn't
//! hold every word; only one that does is read into a session, searched,
//! and held a while for the next search. Port of internal/convo/find.go
//! (SearchFile :20, search :49, holdsAll :144, rawWord :193, lowerASCII
//! :272).

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rush_core::time::Ms;

use super::cache::{Body, Held};
use super::fuzzy::{distance, typos};
use super::indexer::Indexer;
use super::pieces::{self, found};
use super::wordindex::WordIndex;
use super::{Hit, parse_query};
use crate::model::Session;
use crate::transcript::{Decode, Tail};

/// What a search found in one conversation: its closest hits, the closest
/// first, how many there were, and when it started and last moved
/// (find.go:26 Found).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub total: usize,
    pub first: Ms,
    pub last: Ms,
}

impl Found {
    /// The closest hit's score, 0 with none.
    pub fn best(&self) -> i32 {
        self.hits.first().map_or(0, |h| h.score)
    }
}

/// How a conversation on disk is read into a session: Claude Code's
/// transcript through a Tail with its decoder, or another agent's history
/// through its adapter (None when it can't be).
#[derive(Clone)]
pub enum Reader {
    Transcript(Decode),
    Other(Arc<dyn Fn() -> Option<Session> + Send + Sync>),
}

/// A conversation to search or index: its file and how to read it
/// (indexer.go:19 Source).
#[derive(Clone)]
pub struct Source {
    pub path: String,
    pub read: Reader,
}

/// How long a transcript goes unwritten before it's taken to have stopped
/// growing, and worth indexing again (find.go:111).
pub(crate) const SETTLED: Duration = Duration::from_secs(30);

/// What rush keeps for searching: the conversations held after a search
/// read them, their word and piece indexes (on disk under `dir`, when it
/// has one; in memory only without), and the indexer. Go keeps these in
/// package globals.
pub struct Library {
    dir: Mutex<Option<PathBuf>>,
    pub(crate) held: Arc<Mutex<Held>>,
    pub(crate) indexes: Mutex<std::collections::HashMap<String, Arc<WordIndex>>>,
    pub(crate) indexer: Mutex<Indexer>,
}

/// rush's library: indexes kept nowhere until [`Library::set_dir`] says
/// where (rush's own folder's `search`).
pub fn library() -> &'static Arc<Library> {
    static LIB: OnceLock<Arc<Library>> = OnceLock::new();
    LIB.get_or_init(|| Library::new(None))
}

/// A file as it stands: its size and when it was written, in nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stat {
    pub size: u64,
    pub modt: i64,
}

impl Stat {
    pub(crate) fn of(path: &str) -> io::Result<(Stat, bool)> {
        let m = fs::metadata(path)?;
        let modt = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i64);
        Ok((
            Stat {
                size: m.len(),
                modt,
            },
            m.is_dir(),
        ))
    }

    /// Whether it was last written longer ago than SETTLED.
    pub(crate) fn settled(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as i64);
        now - self.modt > SETTLED.as_nanos() as i64
    }
}

impl Library {
    /// A library keeping its indexes in dir (None: in memory only, as a
    /// test's are).
    pub fn new(dir: Option<PathBuf>) -> Arc<Library> {
        Arc::new(Library {
            dir: Mutex::new(dir),
            held: Arc::new(Mutex::new(Held::default())),
            indexes: Mutex::new(Default::default()),
            indexer: Mutex::new(Indexer::default()),
        })
    }

    /// Where the indexes are kept from now on.
    pub fn set_dir(&self, dir: Option<PathBuf>) {
        *self.dir.lock().unwrap() = dir;
    }

    pub(crate) fn dir(&self) -> Option<PathBuf> {
        self.dir.lock().unwrap().clone()
    }

    /// Searches the conversation at path for q, as Session::search does,
    /// keeping up to limit hits and counting them all. A query with no
    /// words finds nothing: every transcript would have to be read whole
    /// (find.go:20 SearchFile, SearchRead).
    pub fn search(&self, path: &str, q: &str, limit: usize, read: &Reader) -> io::Result<Found> {
        let p = parse_query(q);
        if p.words.is_empty() {
            return Ok(Found::default());
        }
        let (st, _) = Stat::of(path)?;
        // Words alone are scored as well by the pieces as by the session.
        let plain = !p.filtered();
        let mut h = self.held_for(path, st);
        if h.as_ref().is_some_and(|b| b.lock().unwrap().is_pieces()) && !plain {
            h = None; // is:, file: and turn: need the session
        }
        let body = match h {
            Some(b) => b,
            None => {
                // Its index says, when it has one; else a pass over the
                // raw bytes.
                let ix = self.index_for(path, st);
                let mut body = None;
                match &ix {
                    None => {
                        if !file_holds_all(path, &p.words)? {
                            return Ok(Found::default());
                        }
                    }
                    Some(ix) if !ix.may_hold(&p.words) => return Ok(Found::default()),
                    Some(ix) if plain && ix.stat == st => {
                        // As it stands: its pieces answer, the transcript
                        // unread.
                        if let Some(pi) = pieces::load(self.dir().as_deref(), path, st) {
                            body = Some(self.hold(path, st, Body::Pieces(pi)));
                        }
                    }
                    Some(_) => {}
                }
                match body {
                    Some(b) => b,
                    None => {
                        let b = read_whole(path, read)?;
                        // Indexed afresh when it had none, or has settled
                        // since it grew.
                        if let Some(s) = b.session()
                            && (ix.is_none() || st.settled())
                        {
                            self.index_session(path, st, s);
                        }
                        self.hold(path, st, b)
                    }
                }
            }
        };
        let mut b = body.lock().unwrap();
        if let Body::Pieces(pi) = &*b {
            return Ok(pi.search(&p, limit));
        }
        if let Body::Tail(t, s) = &mut *b
            && st.size > t.size()
        {
            t.read(s)?;
            self.grew(path, st);
        }
        let Some(s) = b.session() else {
            return Ok(Found::default());
        };
        Ok(found(s.search(q), limit, s.first, s.last))
    }
}

/// Reads the conversation at path into a session to hold.
pub(crate) fn read_whole(path: &str, read: &Reader) -> io::Result<Body> {
    match read {
        Reader::Other(f) => Ok(Body::Read(f())),
        Reader::Transcript(d) => {
            let mut t = Tail::new(path, d.clone());
            let mut s = Session::new();
            t.read(&mut s)?;
            Ok(Body::Tail(t, s))
        }
    }
}

/// holds_all over the file at path.
fn file_holds_all(path: &str, words: &[String]) -> io::Result<bool> {
    holds_all(File::open(path)?, words)
}

const READ_BUF: usize = 64 << 10;

/// Whether r holds every word, ignoring ASCII case, or a word a typo away
/// from it (as word_match finds them). It can say yes when the search
/// would say no (a word only in a field the search doesn't look at), never
/// the other way round: words it can't look for in raw JSON (non-ASCII,
/// quotes, backslashes) count as present (find.go:144).
pub(crate) fn holds_all(mut r: impl Read, words: &[String]) -> io::Result<bool> {
    let mut need: Vec<RawWord> = Vec::new();
    let mut long = 0;
    for w in words {
        if raw_searchable(w) {
            need.push(RawWord::new(w));
            long = long.max(w.len());
        }
    }
    if need.is_empty() {
        return Ok(true);
    }
    let mut buf = vec![0u8; READ_BUF];
    let mut keep = 0; // the end of the last chunk, so a word across two is found
    loop {
        let mut n = 0;
        let mut eof = false;
        while keep + n < buf.len() {
            match r.read(&mut buf[keep + n..]) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(k) => n += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let chunk = &mut buf[..keep + n];
        lower_ascii(&mut chunk[keep..]);
        let chunk = &buf[..keep + n];
        need.retain(|w| !(memchr::memmem::find(chunk, &w.w).is_some() || w.typo_in(chunk)));
        if need.is_empty() {
            return Ok(true);
        }
        if eof {
            return Ok(false);
        }
        let len = chunk.len();
        keep = (long - 1).min(len);
        buf.copy_within(len - keep..len, 0);
    }
}

/// A word looked for in raw JSON, as typed or a typo away (find.go:193).
pub(crate) struct RawWord {
    pub w: Vec<u8>,
    /// How many letters off it may be.
    k: usize,
    /// Each byte's class: 0 not a word's, 1 a word's that w has, 2 a
    /// word's that w hasn't, a letter off.
    class: [u8; 256],
}

/// The bytes of a word as word_rune has them; anything past ASCII counts,
/// so a word with an accent isn't cut in two.
const fn word_byte(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c >= 0x80
}

impl RawWord {
    pub(crate) fn new(w: &str) -> RawWord {
        let mut rw = RawWord {
            w: w.as_bytes().to_vec(),
            k: typos(w),
            class: [0; 256],
        };
        for c in 0..=255u8 {
            if word_byte(c) {
                rw.class[c as usize] = 2;
            }
        }
        for &c in w.as_bytes() {
            if word_byte(c) {
                rw.class[c as usize] = 1;
            }
        }
        rw
    }

    /// Whether chunk (lower case) has a word a typo away from rw. Two cheap
    /// checks rule out nearly every word before distance runs: a byte rw
    /// hasn't is a letter off, and one letter off keeps one of its first
    /// two in one of the word's first two places (find.go:228 typoIn).
    pub(crate) fn typo_in(&self, chunk: &[u8]) -> bool {
        let (k, w) = (self.k, &self.w);
        if k == 0 {
            return false;
        }
        let (lo, hi) = (w.len().saturating_sub(k), w.len() + k);
        let mut i = 0;
        while i < chunk.len() {
            if self.class[chunk[i] as usize] == 0 {
                i += 1;
                continue;
            }
            let (mut j, mut off, mut wide) = (i, 0, false);
            while j < chunk.len() {
                let c = chunk[j];
                let cl = self.class[c as usize];
                if cl == 0 {
                    break;
                }
                off += (cl >> 1) as usize;
                wide = wide || c >= 0x80;
                j += 1;
            }
            let word = &chunk[i..j];
            if wide {
                // Letters past ASCII take bytes apiece: count letters.
                let a: Vec<char> = String::from_utf8_lossy(w).chars().collect();
                let b: Vec<char> = String::from_utf8_lossy(word).chars().collect();
                if distance(&a, &b, k) <= k {
                    return true;
                }
            } else {
                let n = j - i;
                if n >= lo
                    && n <= hi
                    && off <= k
                    && (k > 1 || near(w, word))
                    && distance(w, word, k) <= k
                {
                    return true;
                }
            }
            i = j;
        }
        false
    }
}

/// Whether t could be one letter off w: changed, added, dropped or
/// swapped, one of w's first two letters is in t's first two places.
fn near(w: &[u8], t: &[u8]) -> bool {
    let (a, b) = (t[0], t[1.min(t.len() - 1)]);
    let w1 = w.get(1).copied().unwrap_or(w[0]);
    a == w[0] || a == w1 || b == w[0] || b == w1
}

/// A word that reads the same inside a JSON string.
pub(crate) fn raw_searchable(w: &str) -> bool {
    !w.is_empty()
        && w.bytes()
            .all(|c| (0x20..0x80).contains(&c) && c != b'"' && c != b'\\')
}

/// Lowers b's ASCII capitals in place, eight bytes at a time: a byte's
/// top bit says whether it's past 'A' and past 'Z', and a capital gets
/// 0x20 added. Bytes past ASCII are left as they are (find.go:272).
pub(crate) fn lower_ascii(b: &mut [u8]) {
    const LOW: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    const ONES: u64 = 0x0101_0101_0101_0101;
    let (words, rest) = b.as_chunks_mut::<8>();
    for c in words {
        let x = u64::from_le_bytes(*c);
        let y = x & LOW;
        let upper = (y.wrapping_add(ONES * (0x80 - b'A' as u64)))
            & !(y.wrapping_add(ONES * (0x80 - b'Z' as u64 - 1)))
            & !x
            & HIGH;
        if upper != 0 {
            *c = (x | upper >> 2).to_le_bytes();
        }
    }
    rest.make_ascii_lowercase();
}
