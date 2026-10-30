//! A conversation's word index: every distinct word, in lower case, of
//! what a search looks at in it (what you said, what the agent said, its
//! steps' commands, paths and output; not the folder it ran in, ids or
//! pictures). A search asks it first: a conversation without the words,
//! as typed or a typo away, is passed over without reading the file. It's
//! made whenever a session is read whole, kept on disk so it outlasts
//! rush, and grows with a transcript: the lines added since are read raw,
//! their words added. Port of internal/convo/wordindex.go:1-317, the same
//! format on disk, so Go's rush and this one share them.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use sha2::{Digest, Sha256};

use super::find::{Library, RawWord, Stat, raw_searchable};
use super::fuzzy::{lower, typos, word_rune};
use super::pieces;
use crate::model::Session;

/// The format on disk; another is read as none.
const INDEX_VERSION: &str = "rush-words 1";

/// A conversation's words, each followed by a newline, and the file as it
/// was indexed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WordIndex {
    pub stat: Stat,
    pub words: Vec<u8>,
}

/// Where the index of the transcript at path is kept, its kind's
/// extension given (".words", ".pieces").
pub(crate) fn index_path(dir: &Path, path: &str, ext: &str) -> PathBuf {
    let sum = Sha256::digest(path.as_bytes());
    let mut name = String::with_capacity(24 + ext.len());
    for b in &sum[..12] {
        name.push_str(&format!("{b:02x}"));
    }
    name.push_str(ext);
    dir.join(name)
}

impl WordIndex {
    /// Whether the conversation can have every word: one of its words has
    /// it inside, or is a typo away. A word it can't answer for (one a
    /// word of text can't hold whole, as "a.b") counts as there
    /// (wordindex.go:96 mayHold).
    pub(crate) fn may_hold(&self, words: &[String]) -> bool {
        for w in words {
            let w = w.to_lowercase();
            if !indexable(&w) || memchr::memmem::find(&self.words, w.as_bytes()).is_some() {
                continue;
            }
            if raw_searchable(&w) {
                if !RawWord::new(&w).typo_in(&self.words) {
                    return false;
                }
            } else if typos(&w) == 0 {
                return false;
            }
            // A long word past ASCII may be a typo away, letter by letter
            // rather than byte by byte: the session says.
        }
        true
    }

    /// ix with the words of what the file gained since added: read raw, as
    /// JSON, its escapes breaking words rather than joining them, so no
    /// word said is lost (wordindex.go:170 readOn).
    fn read_on(&self, path: &str, st: Stat) -> io::Result<WordIndex> {
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(self.stat.size))?;
        let mut b = Vec::new();
        f.take(st.size - self.stat.size).read_to_end(&mut b)?;
        let mut seen: BTreeSet<String> = self
            .words
            .split(|&c| c == b'\n')
            .filter(|w| !w.is_empty())
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .collect();
        raw_words(&b, |w| {
            seen.insert(w);
        });
        Ok(WordIndex {
            stat: Stat {
                size: self.stat.size + b.len() as u64,
                modt: st.modt,
            },
            words: join_words(seen),
        })
    }
}

/// A word the index can say yes or no to: all of it is a word's.
pub(crate) fn indexable(w: &str) -> bool {
    !w.is_empty() && w.chars().all(word_rune)
}

pub(crate) fn join_words(seen: BTreeSet<String>) -> Vec<u8> {
    let mut out = Vec::new();
    for w in seen {
        out.extend_from_slice(w.as_bytes());
        out.push(b'\n');
    }
    out
}

/// Calls f with each word of text, in lower case.
pub(crate) fn each_word(text: &str, mut f: impl FnMut(String)) {
    let mut start = None;
    for (i, c) in text.char_indices() {
        match (word_rune(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                f(lower_word(&text[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        f(lower_word(&text[s..]));
    }
}

/// w in lower case.
fn lower_word(w: &str) -> String {
    if w.bytes().all(|c| !c.is_ascii_uppercase() && c.is_ascii()) {
        return w.to_string();
    }
    w.chars().map(lower).collect()
}

/// Calls f with each word of raw JSON b, in lower case: a backslash and
/// what it escapes end a word.
fn raw_words(b: &[u8], mut f: impl FnMut(String)) {
    let s = String::from_utf8_lossy(b);
    let b = s.as_bytes();
    let mut cur = String::new();
    let mut flush = |cur: &mut String| {
        if !cur.is_empty() {
            f(lower_word(cur));
            cur.clear();
        }
    };
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            flush(&mut cur);
            i += 2;
            if i - 1 < b.len() && b[i - 1] == b'u' {
                i += 4;
            }
            continue;
        }
        let Some(c) = s.get(i..).and_then(|r| r.chars().next()) else {
            i += 1;
            continue;
        };
        if word_rune(c) {
            cur.push(c);
        } else {
            flush(&mut cur);
        }
        i += c.len_utf8();
    }
    flush(&mut cur);
}

impl Library {
    /// path's index if it's as the file stands, read on to it when the
    /// file has only grown; None when there's none to trust
    /// (wordindex.go:65 indexFor).
    pub(crate) fn index_for(&self, path: &str, st: Stat) -> Option<Arc<WordIndex>> {
        let have = self.indexes.lock().unwrap().get(path).cloned();
        let ix = match have {
            Some(ix) => ix,
            None => {
                let ix = Arc::new(load_index(self.dir().as_deref(), path)?);
                self.keep_index(path, ix.clone(), false);
                ix
            }
        };
        if st == ix.stat {
            return Some(ix);
        }
        if st.size > ix.stat.size {
            let grown = Arc::new(ix.read_on(path, st).ok()?);
            // Held, not written: the one on disk reads on from where it
            // got to just as well, and a live one grows by the second.
            self.keep_index(path, grown.clone(), false);
            return Some(grown);
        }
        None // rewritten: read whole again
    }

    /// Indexes the conversation at path from s, as read whole when the
    /// file stood as st.
    pub(crate) fn index_session(&self, path: &str, st: Stat, s: &Session) {
        let pi = pieces::build(s, st);
        let dir = self.dir();
        if let Some(d) = &dir {
            let _ = pieces::save(d, path, &pi);
        }
        let ix = WordIndex {
            stat: st,
            words: pi.word_list(),
        };
        self.keep_index(path, Arc::new(ix), true);
    }

    fn keep_index(&self, path: &str, ix: Arc<WordIndex>, save: bool) {
        self.indexes
            .lock()
            .unwrap()
            .insert(path.to_string(), ix.clone());
        if save && let Some(d) = self.dir() {
            let _ = save_index(&d, path, &ix);
        }
    }

    /// Whether the conversation at path has an index as the file stands,
    /// or one it can be read on to (wordindex.go:255 Indexed).
    pub fn indexed(&self, path: &str) -> bool {
        Stat::of(path).is_ok_and(|(st, _)| self.index_for(path, st).is_some())
    }

    /// Lets go of path's index in memory (the one on disk stays).
    pub fn forget_index(&self, path: &str) {
        self.indexes.lock().unwrap().remove(path);
    }
}

/// Writes to a temp file in dir, then renames it to `to`.
pub(crate) fn write_atomic(
    dir: &Path,
    to: &Path,
    f: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}-{}",
        to.file_name().and_then(|n| n.to_str()).unwrap_or("index"),
        std::process::id()
    ));
    let res = File::create(&tmp).and_then(|mut fh| {
        f(&mut fh)?;
        fh.sync_all()
    });
    match res.and_then(|_| fs::rename(&tmp, to)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn save_index(dir: &Path, path: &str, ix: &WordIndex) -> io::Result<()> {
    let to = index_path(dir, path, ".words");
    write_atomic(dir, &to, |fh| {
        write!(
            fh,
            "{INDEX_VERSION}\n{path}\n{} {}\n",
            ix.stat.size, ix.stat.modt
        )?;
        let mut z = DeflateEncoder::new(fh, Compression::fast());
        z.write_all(&ix.words)?;
        z.finish()?;
        Ok(())
    })
}

/// Reads a head line, without its newline.
pub(crate) fn head(r: &mut impl BufRead) -> String {
    let mut l = String::new();
    let _ = r.read_line(&mut l);
    if l.ends_with('\n') {
        l.pop();
    }
    l
}

fn load_index(dir: Option<&Path>, path: &str) -> Option<WordIndex> {
    let f = File::open(index_path(dir?, path, ".words")).ok()?;
    let mut r = BufReader::new(f);
    if head(&mut r) != INDEX_VERSION || head(&mut r) != path {
        return None;
    }
    let h = head(&mut r);
    let mut it = h.split_whitespace();
    let size = it.next()?.parse().ok()?;
    let modt = it.next()?.parse().ok()?;
    let mut words = Vec::new();
    DeflateDecoder::new(r).read_to_end(&mut words).ok()?;
    Some(WordIndex {
        stat: Stat { size, modt },
        words,
    })
}
