//! What the command bar and the Agents list's filter both do with a found
//! hit's words: the `in:` part of a query taken out, whether the rest is
//! worth searching for, and where the words sit in a line, drawn. After
//! Go's internal/ui/cmdbar.go (splitIn, litWords, whoGlyph, litTitle).

use rush_convo::search::{self as cs};
use rush_core::text::{Line, Span, width};

use crate::style::*;

/// Takes in:<name> out of a query (splitIn).
pub fn split_in(q: &str) -> (String, String) {
    let mut keep = Vec::new();
    let mut inn = String::new();
    for f in q.split_whitespace() {
        if let Some(v) = f.to_lowercase().strip_prefix("in:") {
            inn = v.to_string();
            continue;
        }
        keep.push(f);
    }
    (keep.join(" "), inn)
}

pub fn strip_in(q: &str) -> String {
    split_in(q).0
}

/// A query with enough to it to be worth reading every transcript for.
pub fn searchable(q: &str) -> bool {
    cs::words(&strip_in(q))
        .iter()
        .map(String::len)
        .sum::<usize>()
        >= 3
}

/// The chars of s where the query's words are found (litWords).
pub fn lit_words(s: &str, words: &[String]) -> Vec<usize> {
    let mut at = Vec::new();
    for w in words {
        let Some((i, j)) = cs::match_span(s, w) else {
            continue;
        };
        let from = s[..i].chars().count();
        at.extend(from..from + s[i..j].chars().count());
    }
    at
}

/// Who said a hit: you, the agent, or a tool (whoGlyph).
pub fn who_glyph(who: &str) -> Span {
    match who {
        "you" => sp("›", TEXT),
        cs::WHO_AGENT => sp("✻", ORANGE),
        _ => sp("·", SUB),
    }
}

/// title painted, its matched chars bright, cut to w cells (litTitle).
pub fn lit_title(title: &str, lit: &[usize], w: usize) -> Line {
    let t = if width(title) > w {
        rush_draw::truncate(title, w, "…").into_owned()
    } else {
        title.to_string()
    };
    if lit.is_empty() {
        return line([sp(t, TEXT)]);
    }
    let mut l = Line::new();
    for (i, c) in t.chars().enumerate() {
        let s = c.to_string();
        let on = lit.contains(&i);
        let span = if on { spb(s, ORANGE) } else { sp(s, TEXT) };
        // Neighbours of one look run together.
        match l.spans.last_mut() {
            Some(last) if last.style == span.style => last.text.push(c),
            _ => l.spans.push(span),
        }
    }
    l
}
