//! A conversation's piece index: what a search scores in it, kept apart
//! from the transcript: each thing you said, each thing the agent said,
//! and each step's label, command, path and output, with its turn and when
//! it was, and for each word the pieces it's in. A search with words alone
//! scores only the pieces with every word, as Session::search scores
//! them, without reading the transcript. It stands for the transcript as
//! it was made; one that grew since is read until it's indexed again.
//! Port of internal/convo/pieceindex.go:1-414, the same format on disk.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use rush_core::time::Ms;

use super::find::{Found, RawWord, Stat, raw_searchable};
use super::fuzzy::typos;
use super::wordindex::{each_word, head, index_path, indexable, write_atomic};
use super::{Hit, Query, WHO_AGENT, first_time, snippet, step_label, text_at};
use crate::model::{Item, Session};
use crate::render::{hidden, one_line};

const PIECE_YOU: u8 = 0;
const PIECE_AGENT: u8 = 1;
const PIECE_STEP: u8 = 2;

const PIECE_VERSION: &str = "rush-pieces 1";

/// One thing a search scores.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Piece {
    kind: u8,
    turn: u32,
    at: Ms,
    r#ref: String,
    /// A step's tool.
    who: String,
    /// A step's label.
    label: String,
    /// What was said; a step's label, command, path and output.
    text: String,
    /// Where a step's output starts in text.
    out_at: usize,
}

/// A conversation's pieces, and the pieces each word is in.
#[derive(Clone, Debug, Default)]
pub(crate) struct PieceIndex {
    stat: Stat,
    first: Ms,
    last: Ms,
    pieces: Vec<Piece>,
    /// Sorted.
    vocab: Vec<String>,
    /// The pieces each word of vocab is in, in order.
    posts: Vec<Vec<u32>>,
}

/// The pieces of s a search scores for words alone, in its order
/// (pieceindex.go:57 sessionPieces).
fn session_pieces(s: &Session) -> Vec<Piece> {
    let mut out = Vec::new();
    for t in &s.turns {
        let r = format!("t{}", t.n);
        if !t.prompt.is_empty() {
            out.push(Piece {
                kind: PIECE_YOU,
                turn: t.n,
                at: t.start,
                r#ref: r.clone(),
                text: t.prompt.clone(),
                ..Piece::default()
            });
        }
        for it in &t.items {
            match it {
                Item::Text { text, answer } if !text.is_empty() => out.push(Piece {
                    kind: PIECE_AGENT,
                    turn: t.n,
                    at: text_at(*answer, t),
                    r#ref: r.clone(),
                    text: text.clone(),
                    ..Piece::default()
                }),
                Item::Step(id) => {
                    let st = s.step(*id);
                    for sid in std::iter::once(*id).chain(st.children.iter().copied()) {
                        let st = s.step(sid);
                        if hidden(st) {
                            continue;
                        }
                        let x = &st.call().input;
                        let file = if matches!(
                            st.kind,
                            rush_core::tool::Kind::Search | rush_core::tool::Kind::Glob
                        ) {
                            ""
                        } else {
                            &x.path
                        };
                        let label = step_label(st);
                        let head = format!("{label}\n{}\n{file}\n", x.command);
                        out.push(Piece {
                            kind: PIECE_STEP,
                            turn: t.n,
                            at: first_time(st.start, t.start),
                            r#ref: format!("{r}:s:{}", st.id),
                            who: st.tool.to_string(),
                            label: label.to_string(),
                            out_at: head.len(),
                            text: head + &st.output,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    out
}

impl Query {
    /// pc as a search finds it, if it does.
    fn hit(&self, pc: &Piece) -> Option<Hit> {
        let sc = self.score(&pc.text)?;
        let mut h = Hit {
            r#ref: pc.r#ref.clone(),
            turn: pc.turn,
            at: pc.at,
            ..Hit::default()
        };
        match pc.kind {
            PIECE_YOU => {
                (h.who, h.snippet, h.score) =
                    ("you".into(), snippet(&pc.text, &self.words), sc + 20)
            }
            PIECE_AGENT => {
                (h.who, h.snippet, h.score) =
                    (WHO_AGENT.into(), snippet(&pc.text, &self.words), sc + 10)
            }
            _ => {
                (h.who, h.score, h.snippet) = (pc.who.clone(), sc, one_line(&pc.label));
                if !self.matches(&pc.label) {
                    h.snippet = format!(
                        "{} · {}",
                        h.snippet,
                        snippet(pc.text.get(pc.out_at..).unwrap_or(""), &self.words)
                    );
                }
            }
        }
        Some(h)
    }
}

/// Indexes s, read whole when its file stood as st.
pub(crate) fn build(s: &Session, st: Stat) -> PieceIndex {
    let mut ix = PieceIndex {
        stat: st,
        first: s.first,
        last: s.last,
        pieces: session_pieces(s),
        ..PieceIndex::default()
    };
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut vocab: Vec<String> = Vec::new();
    let mut posts: Vec<Vec<u32>> = Vec::new();
    for (i, pc) in ix.pieces.iter().enumerate() {
        each_word(&pc.text, |w| {
            let id = *ids.entry(w.clone()).or_insert_with(|| {
                vocab.push(w);
                posts.push(Vec::new());
                vocab.len() - 1
            });
            let p = &mut posts[id];
            if p.last() != Some(&(i as u32)) {
                p.push(i as u32);
            }
        });
    }
    // The words sorted, their pieces with them.
    let mut order: Vec<usize> = (0..vocab.len()).collect();
    order.sort_by(|&a, &b| vocab[a].cmp(&vocab[b]));
    ix.vocab = order
        .iter()
        .map(|&o| std::mem::take(&mut vocab[o]))
        .collect();
    ix.posts = order
        .iter()
        .map(|&o| std::mem::take(&mut posts[o]))
        .collect();
    ix
}

impl PieceIndex {
    /// The vocabulary as a word index has it.
    pub(crate) fn word_list(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for w in &self.vocab {
            out.extend_from_slice(w.as_bytes());
            out.push(b'\n');
        }
        out
    }

    /// Scores the pieces that can match p's words, keeping the closest
    /// limit (pieceindex.go:163 search).
    pub(crate) fn search(&self, p: &Query, limit: usize) -> Found {
        let cand = self.candidates(&p.words);
        let all = self
            .pieces
            .iter()
            .enumerate()
            .filter(|(i, _)| cand.as_ref().is_none_or(|c| c[*i]))
            .filter_map(|(_, pc)| p.hit(pc))
            .collect();
        found(all, limit, self.first, self.last)
    }

    /// The pieces with every word, as typed or a typo away; None when every
    /// piece is (a word the index can't answer for).
    fn candidates(&self, words: &[String]) -> Option<Vec<bool>> {
        let mut cand: Option<Vec<bool>> = None;
        for w in words {
            let w = w.to_lowercase();
            if !indexable(&w) || !raw_searchable(&w) && typos(&w) > 0 {
                continue; // the pieces' text says
            }
            let mut inn = vec![false; self.pieces.len()];
            let rw = RawWord::new(&w);
            for (i, v) in self.vocab.iter().enumerate() {
                if !v.contains(&w) && !rw.typo_in(v.as_bytes()) {
                    continue;
                }
                for &pc in &self.posts[i] {
                    inn[pc as usize] = true;
                }
            }
            cand = Some(match cand {
                None => inn,
                Some(c) => c.iter().zip(inn).map(|(a, b)| *a && b).collect(),
            });
        }
        cand
    }
}

/// A search's hits, the closest first, then the latest
/// (pieceindex.go:223 found).
pub(crate) fn found(mut all: Vec<Hit>, limit: usize, first: Ms, last: Ms) -> Found {
    all.sort_by(|a, b| b.score.cmp(&a.score).then(b.at.cmp(&a.at)));
    let total = all.len();
    all.truncate(limit);
    Found {
        hits: all,
        total,
        first,
        last,
    }
}

// --- on disk ---

fn ms_to_ns(ms: Ms) -> u64 {
    if ms <= 0 { 0 } else { (ms as u64) * 1_000_000 }
}

fn ns_to_ms(ns: i64) -> Ms {
    if ns <= 0 { 0 } else { ns / 1_000_000 }
}

fn put_uvarint(w: &mut Vec<u8>, mut n: u64) {
    while n >= 0x80 {
        w.push(n as u8 | 0x80);
        n >>= 7;
    }
    w.push(n as u8);
}

pub(crate) fn save(dir: &Path, path: &str, ix: &PieceIndex) -> io::Result<()> {
    let to = index_path(dir, path, ".pieces");
    let mut b = Vec::new();
    let str_ = |b: &mut Vec<u8>, s: &str| {
        put_uvarint(b, s.len() as u64);
        b.extend_from_slice(s.as_bytes());
    };
    put_uvarint(&mut b, ix.pieces.len() as u64);
    for pc in &ix.pieces {
        b.push(pc.kind);
        put_uvarint(&mut b, pc.turn as u64);
        put_uvarint(&mut b, ms_to_ns(pc.at));
        str_(&mut b, &pc.r#ref);
        str_(&mut b, &pc.who);
        str_(&mut b, &pc.label);
        str_(&mut b, &pc.text);
        put_uvarint(&mut b, pc.out_at as u64);
    }
    put_uvarint(&mut b, ix.vocab.len() as u64);
    for (v, posts) in ix.vocab.iter().zip(&ix.posts) {
        str_(&mut b, v);
        put_uvarint(&mut b, posts.len() as u64);
        let mut prev = 0u32;
        for &pc in posts {
            put_uvarint(&mut b, (pc - prev) as u64);
            prev = pc;
        }
    }
    write_atomic(dir, &to, |fh| {
        write!(
            fh,
            "{PIECE_VERSION}\n{path}\n{} {} {} {}\n",
            ix.stat.size,
            ix.stat.modt,
            ms_to_ns(ix.first),
            ms_to_ns(ix.last)
        )?;
        let mut z = DeflateEncoder::new(fh, Compression::fast());
        z.write_all(&b)?;
        z.finish()?;
        Ok(())
    })
}

/// Reads uvarints and strings from a decompressed stream.
struct Dec<R: Read> {
    r: R,
    bad: bool,
}

impl<R: Read> Dec<R> {
    fn byte(&mut self) -> Option<u8> {
        let mut b = [0u8];
        match self.r.read_exact(&mut b) {
            Ok(()) => Some(b[0]),
            Err(_) => {
                self.bad = true;
                None
            }
        }
    }

    fn num(&mut self) -> u64 {
        let (mut x, mut s) = (0u64, 0);
        for _ in 0..10 {
            let Some(b) = self.byte() else { return 0 };
            if b < 0x80 {
                return x | (b as u64) << s;
            }
            x |= ((b & 0x7f) as u64) << s;
            s += 7;
        }
        self.bad = true;
        0
    }

    fn str(&mut self) -> String {
        let n = self.num();
        if self.bad || n > 1 << 30 {
            self.bad = true;
            return String::new();
        }
        let mut b = vec![0u8; n as usize];
        if self.r.read_exact(&mut b).is_err() {
            self.bad = true;
        }
        String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }
}

/// path's piece index from disk, if there's one for the file as it stands
/// (pieceindex.go:313 loadPieces).
pub(crate) fn load(dir: Option<&Path>, path: &str, st: Stat) -> Option<PieceIndex> {
    let f = File::open(index_path(dir?, path, ".pieces")).ok()?;
    let mut br = BufReader::new(f);
    if head(&mut br) != PIECE_VERSION || head(&mut br) != path {
        return None;
    }
    let h = head(&mut br);
    let nums: Vec<i64> = h
        .split_whitespace()
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    let [size, modt, first, last] = nums[..] else {
        return None;
    };
    if size as u64 != st.size || modt != st.modt {
        return None;
    }
    let mut ix = PieceIndex {
        stat: st,
        first: ns_to_ms(first),
        last: ns_to_ms(last),
        ..PieceIndex::default()
    };
    let mut d = Dec {
        r: BufReader::new(DeflateDecoder::new(br)),
        bad: false,
    };
    let n = d.num();
    if d.bad || n > 1 << 26 {
        return None;
    }
    ix.pieces.reserve(n as usize);
    for _ in 0..n {
        let kind = d.byte()?;
        let turn = d.num() as u32;
        let at = ns_to_ms(d.num() as i64);
        let (r, who, label, text) = (d.str(), d.str(), d.str(), d.str());
        let out_at = (d.num() as usize).min(text.len());
        if d.bad {
            return None;
        }
        ix.pieces.push(Piece {
            kind,
            turn,
            at,
            r#ref: r,
            who,
            label,
            text,
            out_at,
        });
    }
    let nv = d.num();
    if d.bad || nv > 1 << 26 {
        return None;
    }
    for _ in 0..nv {
        let v = d.str();
        let np = d.num();
        if d.bad || np > n {
            return None;
        }
        let mut ps = Vec::with_capacity(np as usize);
        let mut prev = 0u32;
        for _ in 0..np {
            prev += d.num() as u32;
            if prev as u64 >= n {
                return None;
            }
            ps.push(prev);
        }
        ix.vocab.push(v);
        ix.posts.push(ps);
    }
    if d.bad {
        return None;
    }
    Some(ix)
}

/// Whether path's pieces on disk are of the file as it stands, reading
/// only their head (pieceindex.go:387 piecesStand).
pub(crate) fn stand(dir: Option<&Path>, path: &str, st: Stat) -> bool {
    let Some(dir) = dir else { return false };
    let Ok(f) = File::open(index_path(dir, path, ".pieces")) else {
        return false;
    };
    let mut br = BufReader::new(f.take(64 << 10));
    if head(&mut br) != PIECE_VERSION || head(&mut br) != path {
        return false;
    }
    let h = head(&mut br);
    let mut it = h.split_whitespace().map(|x| x.parse::<i64>().ok());
    matches!((it.next(), it.next()), (Some(Some(size)), Some(Some(modt))) if size as u64 == st.size && modt == st.modt)
}
