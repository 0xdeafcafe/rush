//! Rust-only: the open conversation searched again on each key typed in
//! the command bar (This session, This chat: Go's barTurns calls Search on
//! every key, cmdbar.go:1101), for only the hits the bar shows. Same
//! hits, in the same order, as [`Session::search`] reversed, sorted by
//! score (when words are typed) and cut to the bar's limit; less work:
//!
//! - where each word is first found as typed is kept from key to key; a
//!   word typed on (rust from rus) can only be where the shorter one was,
//!   from there on, so only those texts are read again, from there;
//! - a text without a word as typed is read for a typo only when it could
//!   still reach the bar: once enough texts score more than a typo could,
//!   the rest aren't read;
//! - a hit's snippet is made only for the hits shown.

use std::collections::HashMap;

use crate::model::{Item, Session, StepId};
use crate::render::hidden;

use super::fuzzy::{
    SCORE_PHRASE, SCORE_TYPO, bit, distance, exact_match_from, lower, typo_score, typo_score_parts,
    typo_start, typo_words, typos,
};
use super::{Hit, Query, WHO_AGENT, first_time, parse_query, snippet, step_parts, text_at};

/// A text searched, in the order Session::search reads them.
#[derive(Clone, Copy, Debug)]
enum Text {
    /// A turn's prompt.
    Prompt(usize),
    /// What the agent said: turn and item.
    Answer(usize, usize),
    /// A step (or a subagent's), in its turn.
    Step(usize, StepId),
}

/// A word's first match as typed in a text: its part, the byte it
/// starts at, and its score; part NONE where it isn't.
#[derive(Clone, Copy, Debug)]
struct At {
    part: u8,
    score: u8,
    byte: u32,
}

const NONE: u8 = u8::MAX;
const MISSING: At = At {
    part: NONE,
    score: 0,
    byte: 0,
};

/// How much text to look for typos in before a vocabulary is worth making
/// (on another thread).
const VOCAB_FROM: usize = 256 << 10;

/// A typo's score in a text: not worked out yet, none, or the score.
const UNKNOWN: i8 = -1;
const NO_TYPO: i8 = -2;

/// What one conversation's searches keep for the next key. Held by the
/// caller (the bar while it's open) and given to each search; a session
/// changed since (a step added or its output grown, another session) is
/// read afresh.
#[derive(Default)]
pub struct SearchMemo {
    key: (u64, u64, u64, usize),
    texts: Vec<Text>,
    /// By word (or phrase) as typed, lowered: each text's first match.
    exact: HashMap<String, Vec<At>>,
    /// By word: each text's typo score, as worked out so far.
    typo: HashMap<String, Vec<i8>>,
    /// The words a typo is looked for among, once made.
    vocab: Option<Vocab>,
    /// The state a vocabulary is being made for (prepare).
    asked: Option<(u64, u64, u64, usize)>,
}

/// Every word a typo is looked for among (the start of each text, as
/// typo_score_parts reads it), lowered, each once; and which are in each
/// text. A word's typos are then worked out once a word, not once a
/// text it's in.
struct Vocab {
    words: Vec<Box<[char]>>,
    /// Each word's letters, as bits (fuzzy::bit).
    bits: Vec<u64>,
    /// Each text's words: texts[x]..texts[x + 1] in ids.
    texts: Vec<u32>,
    ids: Vec<u32>,
}

/// A quick hash for the words (FxHash's): SipHash over every word of a
/// conversation costs more than reading it.
#[derive(Default)]
struct Fx(u64);

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..c.len()].copy_from_slice(c);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(b))
                .wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
}

type FxBuild = std::hash::BuildHasherDefault<Fx>;

/// What a vocabulary is made from, copied out of the conversation so it
/// can be made on another thread: the start of each text that a typo is
/// looked for in, its parts joined by newlines, one after another.
pub struct VocabJob {
    key: (u64, u64, u64, usize),
    starts: String,
    ends: Vec<usize>,
}

/// A vocabulary made, for [`SearchMemo::take`].
pub struct VocabMade {
    key: (u64, u64, u64, usize),
    vocab: Vocab,
}

impl VocabJob {
    /// Makes the vocabulary (on whatever thread).
    pub fn make(self) -> VocabMade {
        let mut from = 0;
        let texts = self.ends.iter().map(|&end| {
            let t = &self.starts[from..end];
            from = end;
            t
        });
        VocabMade {
            key: self.key,
            vocab: Vocab::new(texts),
        }
    }
}

impl Vocab {
    /// Of each text's start (as far as a typo is looked for), in order.
    fn new<'a>(texts: impl Iterator<Item = &'a str>) -> Vocab {
        let mut by: HashMap<Box<str>, u32, FxBuild> = HashMap::default();
        let mut v = Vocab {
            words: Vec::new(),
            bits: Vec::new(),
            texts: Vec::new(),
            ids: Vec::new(),
        };
        let mut word = String::new();
        for t in texts {
            v.texts.push(v.ids.len() as u32);
            let from = v.ids.len();
            typo_words(&[t], |w| {
                word.clear();
                if w.is_ascii() {
                    word.push_str(w);
                    word.make_ascii_lowercase();
                } else {
                    word.extend(w.chars().map(lower));
                }
                let id = match by.get(word.as_str()) {
                    Some(id) => *id,
                    None => {
                        let id = v.words.len() as u32;
                        let chars: Box<[char]> = word.chars().collect();
                        v.bits.push(chars.iter().fold(0, |m, c| m | bit(*c)));
                        v.words.push(chars);
                        by.insert(word.as_str().into(), id);
                        id
                    }
                };
                v.ids.push(id);
            });
            let mine = &mut v.ids[from..];
            mine.sort_unstable();
            let mut n = 0;
            for i in 0..mine.len() {
                if n == 0 || mine[i] != mine[n - 1] {
                    mine[n] = mine[i];
                    n += 1;
                }
            }
            v.ids.truncate(from + n);
        }
        v.texts.push(v.ids.len() as u32);
        v
    }

    /// Each text's typo score for w (typos(w) > 0): the closest of its
    /// words within k letters, as typo_score_parts finds it.
    fn typos(&self, w: &str) -> Vec<i8> {
        let k = typos(w);
        let lw: Vec<char> = w.chars().map(lower).collect();
        let want = lw.iter().fold(0, |m, c| m | bit(*c));
        let off: Vec<u8> = self
            .words
            .iter()
            .zip(&self.bits)
            .map(|(u, has)| {
                if u.len().abs_diff(lw.len()) > k || (want & !has).count_ones() as usize > k {
                    return (k + 1) as u8;
                }
                distance(&lw, u, k).min(k + 1) as u8
            })
            .collect();
        self.texts
            .windows(2)
            .map(|r| {
                let best = self.ids[r[0] as usize..r[1] as usize]
                    .iter()
                    .map(|id| off[*id as usize] as usize)
                    .min()
                    .unwrap_or(k + 1);
                if best <= k {
                    typo_score(best) as i8
                } else {
                    NO_TYPO
                }
            })
            .collect()
    }
}

impl SearchMemo {
    /// Ready for s: what was kept for another session or an older state
    /// of it forgotten.
    fn sync(&mut self, s: &Session) {
        let key = (s.serial(), s.applied(), s.step_ver(), s.turns.len());
        if key == self.key && !self.texts.is_empty() {
            return;
        }
        self.key = key;
        self.exact.clear();
        self.typo.clear();
        self.vocab = None;
        self.texts.clear();
        for (ti, t) in s.turns.iter().enumerate() {
            self.texts.push(Text::Prompt(ti));
            for (ii, it) in t.items.iter().enumerate() {
                match it {
                    Item::Text { .. } => self.texts.push(Text::Answer(ti, ii)),
                    Item::Step(id) => {
                        let st = s.step(*id);
                        for sid in std::iter::once(*id).chain(st.children.iter().copied()) {
                            if !hidden(s.step(sid)) {
                                self.texts.push(Text::Step(ti, sid));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// What making the vocabulary of s's texts needs, copied, to make on
    /// another thread and hand back to take: None when it's made already,
    /// or s is short enough to read as each key asks.
    pub fn prepare(&mut self, s: &Session) -> Option<VocabJob> {
        self.prepare_from(s, VOCAB_FROM)
    }

    /// prepare, for any conversation with at least min bytes to read
    /// (tests make one for a short one).
    #[doc(hidden)]
    pub fn prepare_from(&mut self, s: &Session, min: usize) -> Option<VocabJob> {
        self.sync(s);
        if self.vocab.is_some() || self.asked == Some(self.key) {
            return None;
        }
        self.asked = Some(self.key);
        let mut starts = String::new();
        let mut ends = Vec::with_capacity(self.texts.len());
        for &t in &self.texts {
            let (ps, n) = parts(s, t);
            typo_start(&ps[..n], &mut starts);
            ends.push(starts.len());
        }
        if starts.len() < min {
            return None;
        }
        Some(VocabJob {
            key: self.key,
            starts,
            ends,
        })
    }

    /// Takes a vocabulary made from prepare's copy, if s hasn't changed
    /// since.
    pub fn take(&mut self, made: VocabMade) {
        if made.key == self.key && made.vocab.texts.len() == self.texts.len() + 1 {
            self.vocab = Some(made.vocab);
        }
    }

    /// Keeps only what the words typed now can use: their matches, and
    /// those of what they were typed on from.
    fn keep_for(&mut self, words: &[String], phrase: &str) {
        self.exact.retain(|k, _| {
            phrase.starts_with(k.as_str()) || words.iter().any(|w| w.starts_with(k.as_str()))
        });
        self.typo.retain(|k, _| words.iter().any(|w| w == k));
    }
}

/// The parts of a text, as a search reads them.
fn parts(s: &Session, t: Text) -> ([&str; 4], usize) {
    match t {
        Text::Prompt(ti) => ([s.turns[ti].prompt.as_str(), "", "", ""], 1),
        Text::Answer(ti, ii) => match &s.turns[ti].items[ii] {
            Item::Text { text, .. } => ([text.as_str(), "", "", ""], 1),
            _ => (["", "", "", ""], 1),
        },
        Text::Step(_, sid) => (step_parts(s.step(sid)), 4),
    }
}

/// w's first match as typed in a text, from where the shorter word it was
/// typed on from was first found (`from`), or from the start.
fn first_match(parts: &[&str], w: &str, from: Option<At>) -> At {
    let (p0, b0) = match from {
        Some(a) if a.part == NONE => return MISSING,
        Some(a) => (a.part as usize, a.byte as usize),
        None => (0, 0),
    };
    for (pi, p) in parts.iter().enumerate().skip(p0) {
        let start = if pi == p0 { b0 } else { 0 };
        if let Some((sc, i, _)) = exact_match_from(p, w, start) {
            return At {
                part: pi as u8,
                score: sc as u8,
                byte: i as u32,
            };
        }
    }
    MISSING
}

impl Session {
    /// The hits the bar shows for q, at most limit: Session::search's
    /// hits, the latest first, then (with words typed) the closest first,
    /// cut to limit (barTurns with the bar's section limit). memo keeps
    /// what the next key's search can use.
    pub fn search_top(&self, q: &str, limit: usize, memo: &mut SearchMemo) -> Vec<Hit> {
        let p = parse_query(q);
        if p.words.is_empty() && p.kinds.is_empty() && p.file.is_empty() && p.from == 0
            || limit == 0
        {
            return Vec::new();
        }
        memo.sync(self);
        let phrase = if p.words.len() > 1 {
            p.words.join(" ")
        } else {
            String::new()
        };
        memo.keep_for(&p.words, &phrase);
        let mut keys: Vec<&str> = p.words.iter().map(String::as_str).collect();
        if !phrase.is_empty() {
            keys.push(&phrase);
        }
        for k in &keys {
            self.exact_all(memo, k);
        }
        for w in &p.words {
            memo.typo
                .entry(w.clone())
                .or_insert_with(|| vec![UNKNOWN; memo.texts.len()]);
        }
        let steps =
            p.has("failed") || p.has("edit") || p.has("cmd") || p.has("read") || !p.file.is_empty();
        // Hits scored: (text, score); and to score: (text, its bonus, the
        // score so far, how many words still want a typo looked for).
        let mut done: Vec<(usize, i32)> = Vec::new();
        let mut open: Vec<(usize, i32, i32, usize)> = Vec::new();
        for (x, &t) in memo.texts.iter().enumerate() {
            let Some(bonus) = self.wanted(&p, t, steps) else {
                continue;
            };
            let mut score = bonus;
            let mut want = 0;
            let mut dead = false;
            for w in &p.words {
                let a = memo.exact[w][x];
                if a.part != NONE {
                    score += a.score as i32;
                    continue;
                }
                if typos(w) == 0 {
                    dead = true;
                    break;
                }
                match memo.typo[w][x] {
                    UNKNOWN => want += 1,
                    NO_TYPO => {
                        dead = true;
                        break;
                    }
                    sc => score += sc as i32,
                }
            }
            if dead {
                continue;
            }
            if want == 0 {
                if !phrase.is_empty() && memo.exact[&phrase][x].part != NONE {
                    score += SCORE_PHRASE;
                }
                done.push((x, score));
            } else {
                // A typo scores at most SCORE_TYPO; with one there's no
                // phrase as typed.
                open.push((x, bonus, score, want));
            }
        }
        let sorted = !p.words.is_empty();
        if !open.is_empty() {
            // The limit-th best score of those known: a text that can't
            // reach it can't be shown.
            let floor = if sorted && done.len() >= limit {
                let mut sc: Vec<i32> = done.iter().map(|d| d.1).collect();
                let (_, nth, _) = sc.select_nth_unstable_by(limit - 1, |a, b| b.cmp(a));
                *nth
            } else {
                i32::MIN
            };
            for (x, bonus, score, want) in open {
                // (A typo a letter off scores SCORE_TYPO; none off
                // can't be one, but is allowed for.)
                if score + (SCORE_TYPO + 10) * (want as i32) < floor {
                    continue;
                }
                let mut score = bonus;
                let mut dead = false;
                for w in &p.words {
                    let a = memo.exact[w][x];
                    if a.part != NONE {
                        score += a.score as i32;
                        continue;
                    }
                    if memo.typo[w][x] == UNKNOWN {
                        match &memo.vocab {
                            // Every text's at once, from the vocabulary.
                            Some(v) => {
                                let col = v.typos(w);
                                memo.typo.insert(w.clone(), col);
                            }
                            None => {
                                let (ps, n) = parts(self, memo.texts[x]);
                                let sc = typo_score_parts(&ps[..n], w);
                                memo.typo.get_mut(w).expect("made above")[x] =
                                    sc.map_or(NO_TYPO, |s| s as i8);
                            }
                        }
                    }
                    match memo.typo[w][x] {
                        NO_TYPO => {
                            dead = true;
                            break;
                        }
                        sc => score += sc as i32,
                    }
                }
                if !dead {
                    done.push((x, score));
                }
            }
        }
        // The latest first; with words, the closest first, as close the
        // latest first (a stable sort of the reversed hits).
        if sorted {
            done.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
        } else {
            done.sort_by_key(|d| std::cmp::Reverse(d.0));
        }
        done.truncate(limit);
        done.into_iter()
            .map(|(x, score)| self.hit(&p, memo.texts[x], score))
            .collect()
    }

    /// Every text's first match of k as typed, kept in memo: from the
    /// longest word kept that k was typed on from, where there's one.
    fn exact_all(&self, memo: &mut SearchMemo, k: &str) {
        if memo.exact.contains_key(k) {
            return;
        }
        let from = memo
            .exact
            .keys()
            .filter(|v| k.starts_with(v.as_str()))
            .max_by_key(|v| v.len())
            .cloned();
        let prev = from.and_then(|v| memo.exact.get(&v));
        let col: Vec<At> = memo
            .texts
            .iter()
            .enumerate()
            .map(|(x, &t)| {
                let from = prev.map(|c| c[x]);
                if from.is_some_and(|a| a.part == NONE) {
                    return MISSING;
                }
                let (ps, n) = parts(self, t);
                first_match(&ps[..n], k, from)
            })
            .collect();
        memo.exact.insert(k.to_string(), col);
    }

    /// Whether search reads text t for p, and what it adds to a hit's
    /// score there (what you said counts for more than what was said
    /// back); None when it's passed over.
    fn wanted(&self, p: &Query, t: Text, steps: bool) -> Option<i32> {
        let ti = match t {
            Text::Prompt(ti) | Text::Answer(ti, _) | Text::Step(ti, _) => ti,
        };
        let turn = &self.turns[ti];
        if p.from > 0 && (turn.n < p.from || turn.n > p.to) {
            return None;
        }
        match t {
            Text::Prompt(_) => {
                (!steps && !p.has(WHO_AGENT) && (!p.words.is_empty() || p.has("you") || p.from > 0))
                    .then_some(20)
            }
            Text::Answer(..) => {
                (!p.has("you") && !(steps || p.words.is_empty() && !p.has(WHO_AGENT))).then_some(10)
            }
            Text::Step(_, sid) => {
                (!p.has("you") && !p.has(WHO_AGENT) && p.step_kind(self.step(sid))).then_some(0)
            }
        }
    }

    /// Text t's hit, as Session::search makes it.
    fn hit(&self, p: &Query, t: Text, score: i32) -> Hit {
        match t {
            Text::Prompt(ti) => {
                let turn = &self.turns[ti];
                Hit {
                    r#ref: format!("t{}", turn.n),
                    turn: turn.n,
                    who: "you".into(),
                    snippet: snippet(&turn.prompt, &p.words),
                    at: turn.start,
                    score,
                }
            }
            Text::Answer(ti, ii) => {
                let turn = &self.turns[ti];
                let (text, answer) = match &turn.items[ii] {
                    Item::Text { text, answer } => (text.as_str(), *answer),
                    _ => ("", false),
                };
                Hit {
                    r#ref: format!("t{}", turn.n),
                    turn: turn.n,
                    who: WHO_AGENT.into(),
                    snippet: snippet(text, &p.words),
                    at: text_at(answer, turn),
                    score,
                }
            }
            Text::Step(ti, sid) => {
                let turn = &self.turns[ti];
                let st = self.step(sid);
                let mut h = p.step_hit(st, &format!("t{}", turn.n), score);
                h.turn = turn.n;
                h.at = first_time(st.start, turn.start);
                h
            }
        }
    }
}
