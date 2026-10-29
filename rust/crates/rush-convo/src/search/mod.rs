//! Searching conversations: a query's words matched loosely (fuzzy.rs),
//! an open session's turns and steps (Session::search), and transcripts
//! on disk, passed over by a word index or answered by a piece index
//! without reading them, held a while for the next search, and indexed in
//! the background (find.rs, cache.rs, wordindex.rs, pieces.rs,
//! indexer.rs). Port of internal/convo/search.go:1-306 (all but
//! SearchView, the open session's search view), find.go, fuzzy.go,
//! searchcache.go, wordindex.go, pieceindex.go and indexer.go.
//!
//! No terminal code. What Go keeps in package globals (held
//! conversations, indexes, the indexer) lives in a [`Library`], so a test
//! has its own; rush has one, [`library`].

mod cache;
mod find;
mod fuzzy;
mod indexer;
mod pieces;
mod wordindex;

pub use find::{Found, Library, Reader, Source, library};
pub use fuzzy::{find_fold_span, match_span};

use rush_core::time::Ms;
use rush_core::tool::Kind;

use crate::model::{Item, Session, Step, Turn};
use crate::render::{glyph_for, hidden, one_line};
use crate::text::first_non_empty;
use fuzzy::{SCORE_PHRASE, find_fold, word_match};

/// One search result: where it is and the text around the match
/// (search.go:15 Hit).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hit {
    /// The turn or step it jumps to: "t3", "t3:s:toolu_…".
    pub r#ref: String,
    pub turn: u32,
    /// "you", the agent ([`WHO_AGENT`]), or the tool.
    pub who: String,
    pub snippet: String,
    /// When it was said, or the step ran; 0 when not known.
    pub at: Ms,
    /// How closely it matches: higher is closer.
    pub score: i32,
}

/// Who a hit in what the agent said is from, and the word that finds only
/// those.
pub const WHO_AGENT: &str = "agent";

/// is:claude, kept for searches typed before rush ran other agents.
const WHO_CLAUDE: &str = "claude";

const KNOWN_KINDS: [&str; 6] = ["failed", "edit", "cmd", "read", "you", WHO_AGENT];

/// A query read: its words, and what narrows it (search.go:24).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Query {
    pub words: Vec<String>,
    /// failed, edit, cmd, read, you, agent.
    pub kinds: Vec<&'static str>,
    pub file: String,
    /// The turn range; 0 means any.
    pub from: u32,
    pub to: u32,
    pub unknown: Vec<String>,
}

pub(crate) fn parse_query(q: &str) -> Query {
    let mut p = Query::default();
    let low = fuzzy::lower_str(q);
    for f in low.split_whitespace() {
        if let Some(k) = f.strip_prefix("is:") {
            let k = if k == WHO_CLAUDE { WHO_AGENT } else { k };
            match KNOWN_KINDS.iter().find(|x| **x == k) {
                Some(k) => {
                    if !p.kinds.contains(k) {
                        p.kinds.push(k);
                    }
                }
                None => p.unknown.push(f.to_string()),
            }
        } else if let Some(file) = f.strip_prefix("file:") {
            p.file = file.to_string();
        } else if let Some(r) = f.strip_prefix("turn:") {
            let (a, b) = match r.split_once('-') {
                Some((a, b)) => (a, Some(b)),
                None => (r, None),
            };
            p.from = a.parse().unwrap_or(0);
            p.to = p.from;
            if let Some(b) = b {
                p.to = if b.is_empty() {
                    1 << 30 // turn:5- runs to the end
                } else {
                    b.parse().unwrap_or(0)
                };
            }
            if p.to < p.from {
                std::mem::swap(&mut p.from, &mut p.to);
            }
        } else {
            p.words.push(f.to_string());
        }
    }
    p
}

impl Query {
    pub(crate) fn has(&self, k: &str) -> bool {
        self.kinds.contains(&k)
    }

    /// Whether every word is in text, loosely.
    pub(crate) fn matches(&self, text: &str) -> bool {
        self.score(text).is_some()
    }

    /// How closely text matches every word: each word's match, and more
    /// when the words are there as typed, side by side (search.go:94).
    pub(crate) fn score(&self, text: &str) -> Option<i32> {
        let mut total = 0;
        for w in &self.words {
            total += word_match(text, w)?.0;
        }
        if self.words.len() > 1 && find_fold(text, &self.words.join(" ")).is_some() {
            total += SCORE_PHRASE;
        }
        Some(total)
    }

    /// Whether it narrows by kind, file or turn.
    fn filtered(&self) -> bool {
        !self.kinds.is_empty() || !self.file.is_empty() || self.from > 0
    }

    /// A step as a hit, if it matches (search.go:197).
    fn step(&self, st: &Step, turn_ref: &str) -> Option<Hit> {
        if hidden(st) {
            return None;
        }
        let x = &st.call().input;
        let g = glyph_for(st);
        if self.has("failed") && st.status != crate::model::Status::Failed
            || self.has("edit") && g != "✎"
            || self.has("cmd") && g != "$"
            || self.has("read") && g != "◧"
            || !self.file.is_empty()
                && !fuzzy::lower_str(&format!("{} {}", x.path, x.command)).contains(&self.file)
        {
            return None;
        }
        let label = step_label(st);
        let file = step_file(st);
        let hay = format!("{label}\n{}\n{file}\n{}", x.command, st.output);
        let score = self.score(&hay)?;
        let mut snip = one_line(label);
        if !self.words.is_empty() && !self.matches(label) {
            snip = format!("{snip} · {}", snippet(&st.output, &self.words));
        }
        Some(Hit {
            r#ref: format!("{turn_ref}:s:{}", st.id),
            who: st.tool.to_string(),
            snippet: snip,
            score,
            ..Hit::default()
        })
    }
}

/// A search's or glob's path is where it looked, not a file it's about.
fn step_file(st: &Step) -> &str {
    if matches!(st.kind, Kind::Search | Kind::Glob) {
        ""
    } else {
        &st.call().input.path
    }
}

/// What a step is found by first: its description, command, file,
/// pattern, address, query, else its tool.
pub(crate) fn step_label(st: &Step) -> &str {
    let x = &st.call().input;
    first_non_empty(&[
        &x.description,
        &x.command,
        step_file(st),
        &x.pattern,
        &x.url,
        &x.query,
        &st.tool,
    ])
}

/// When the agent said it: the turn's end for its final words, else the
/// turn's start, all that's kept.
pub(crate) fn text_at(answer: bool, t: &Turn) -> Ms {
    if answer && t.end != 0 { t.end } else { t.start }
}

/// The first of ts that's known.
pub(crate) fn first_time(a: Ms, b: Ms) -> Ms {
    if a != 0 { a } else { b }
}

/// The text around the first word found, on one line (search.go:236).
pub(crate) fn snippet(text: &str, words: &[String]) -> String {
    let t = one_line(text);
    if words.is_empty() {
        return t;
    }
    let Some(i) = words.iter().find_map(|w| word_match(&t, w).map(|m| m.1)) else {
        return t;
    };
    // About 30 characters before it, starting on a whole character.
    let start = t[..i]
        .char_indices()
        .rev()
        .take(30)
        .last()
        .map_or(i, |(at, _)| at);
    if start > 0 {
        format!("…{}", &t[start..])
    } else {
        t[start..].to_string()
    }
}

impl Session {
    /// The turns and steps that match q: plain words anywhere, narrowed by
    /// is:failed, is:edit, is:cmd, is:read, is:you, is:agent, file:<part
    /// of a path> and turn:12 or turn:10-13 (search.go:139 Search).
    pub fn search(&self, q: &str) -> Vec<Hit> {
        let p = parse_query(q);
        if p.words.is_empty() && p.kinds.is_empty() && p.file.is_empty() && p.from == 0 {
            return Vec::new();
        }
        let steps =
            p.has("failed") || p.has("edit") || p.has("cmd") || p.has("read") || !p.file.is_empty();
        let mut hits = Vec::new();
        for t in &self.turns {
            if p.from > 0 && (t.n < p.from || t.n > p.to) {
                continue;
            }
            let r = format!("t{}", t.n);
            if !steps && !p.has(WHO_AGENT) && (!p.words.is_empty() || p.has("you") || p.from > 0) {
                // What you said counts for more than what was said back.
                if let Some(sc) = p.score(&t.prompt) {
                    hits.push(Hit {
                        r#ref: r.clone(),
                        turn: t.n,
                        who: "you".into(),
                        snippet: snippet(&t.prompt, &p.words),
                        at: t.start,
                        score: sc + 20,
                    });
                }
            }
            if p.has("you") {
                continue;
            }
            for it in &t.items {
                match it {
                    Item::Text { text, answer } => {
                        if steps || p.words.is_empty() && !p.has(WHO_AGENT) {
                            continue;
                        }
                        let Some(sc) = p.score(text) else { continue };
                        hits.push(Hit {
                            r#ref: r.clone(),
                            turn: t.n,
                            who: WHO_AGENT.into(),
                            snippet: snippet(text, &p.words),
                            at: text_at(*answer, t),
                            score: sc + 10,
                        });
                    }
                    Item::Step(id) => {
                        if p.has(WHO_AGENT) {
                            continue;
                        }
                        let st = self.step(*id);
                        let all = std::iter::once(*id).chain(st.children.iter().copied());
                        for sid in all {
                            let s = self.step(sid);
                            if let Some(mut h) = p.step(s, &r) {
                                h.turn = t.n;
                                h.at = first_time(s.start, t.start);
                                hits.push(h);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        hits
    }

    /// The ref of the step a subagent's step belongs to, or "" when r
    /// isn't a subagent's step: opening it shows the step (search.go:295).
    pub fn parent_ref(&self, r: &str) -> String {
        let Some((turn, id)) = r.split_once(":s:") else {
            return String::new();
        };
        let Some(sid) = self.step_by_call(id) else {
            return String::new();
        };
        match self.step(sid).parent {
            Some(p) => format!("{turn}:s:{}", self.step(p).id),
            None => String::new(),
        }
    }
}

/// How closely text matches every word of q, as a search scores it (a
/// typo too), and whether it does (find.go:300 Score).
pub fn score(text: &str, q: &str) -> Option<i32> {
    parse_query(q).score(text)
}

/// The plain words of a query, without its filters (find.go:303 Words).
pub fn words(q: &str) -> Vec<String> {
    parse_query(q).words
}

/// Whether q narrows by kind, file or turn (find.go:306 Filtered).
pub fn filtered(q: &str) -> bool {
    parse_query(q).filtered()
}

/// The filters typed that aren't any (is:foo): what a search view says
/// it can't take.
pub fn unknown_filters(q: &str) -> Vec<String> {
    parse_query(q).unknown
}

#[cfg(test)]
mod tests;
