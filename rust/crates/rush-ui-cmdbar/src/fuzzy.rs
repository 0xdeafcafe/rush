//! The command bar's matcher: each word of a query must appear in the
//! text, its letters in order, a whole run and the start of a word
//! scoring more and a gap less. Port of photon/fuzzy/fuzzy.go:1-89
//! (Match, fuzzyWord, bestSub, wordStart).

/// How well q matches s, and the chars of s it matched; None when it
/// doesn't.
pub fn fuzzy_match(q: &str, s: &str) -> Option<(i32, Vec<usize>)> {
    let orig: Vec<char> = s.chars().collect();
    let mut hay: Vec<char> = s.to_lowercase().chars().collect();
    if hay.len() != orig.len() {
        hay = orig.clone(); // lowering changed the length: match as is
    }
    let mut score = 0;
    let mut at = Vec::new();
    for w in q.to_lowercase().split_whitespace() {
        let w: Vec<char> = w.chars().collect();
        let (ws, wat) = fuzzy_word(&w, &hay, &orig)?;
        score += ws;
        at.extend(wat);
    }
    Some((score, at))
}

fn fuzzy_word(w: &[char], hay: &[char], orig: &[char]) -> Option<(i32, Vec<usize>)> {
    if let Some(i) = best_sub(w, hay, orig) {
        let at = (i..i + w.len()).collect();
        let mut score = 100 + 10 * w.len() as i32 - i.min(30) as i32;
        if word_start(hay, orig, i) {
            score += 40;
        }
        return Some((score, at));
    }
    let mut at = Vec::new();
    let (mut score, mut k, mut last) = (0i32, 0usize, -2isize);
    let mut i = 0;
    while i < hay.len() && k < w.len() {
        if hay[i] == w[k] {
            if i as isize == last + 1 {
                score += 6;
            } else if word_start(hay, orig, i) {
                score += 8;
            } else {
                score -= (i as isize - last).min(6) as i32;
            }
            at.push(i);
            last = i as isize;
            k += 1;
        }
        i += 1;
    }
    (k == w.len()).then_some((score, at))
}

fn best_sub(w: &[char], hay: &[char], orig: &[char]) -> Option<usize> {
    let mut first = None;
    let mut i = 0;
    while i + w.len() <= hay.len() {
        if hay[i..i + w.len()] == *w {
            if word_start(hay, orig, i) {
                return Some(i);
            }
            first.get_or_insert(i);
        }
        i += 1;
    }
    first
}

fn word_start(hay: &[char], orig: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let p = hay[i - 1];
    if !p.is_alphanumeric() {
        return true;
    }
    orig[i - 1].is_lowercase() && orig[i].is_uppercase()
}
