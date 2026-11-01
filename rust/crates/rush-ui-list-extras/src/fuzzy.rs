//! Fuzzy matching of what's typed against a name: each word of the query
//! in order, a run of it scoring most, a word's start more. After Go's
//! photon/fuzzy (Match, fuzzyWord, bestSub, wordStart).

/// How well q matches s, and the chars it matched at; None when a word
/// of q isn't in s.
pub fn matches(q: &str, s: &str) -> Option<(i64, Vec<usize>)> {
    let orig: Vec<char> = s.chars().collect();
    let lower: Vec<char> = s.to_lowercase().chars().collect();
    // Lowering changed the length (İ): match as is.
    let hay = if lower.len() == orig.len() {
        lower
    } else {
        orig.clone()
    };
    let mut score = 0;
    let mut at = Vec::new();
    for w in q.to_lowercase().split_whitespace() {
        let w: Vec<char> = w.chars().collect();
        let (ws, wat) = word(&w, &hay, &orig)?;
        score += ws;
        at.extend(wat);
    }
    Some((score, at))
}

fn word(w: &[char], hay: &[char], orig: &[char]) -> Option<(i64, Vec<usize>)> {
    if let Some(i) = best_sub(w, hay, orig) {
        let mut score = 100 + 10 * w.len() as i64 - i.min(30) as i64;
        if word_start(hay, orig, i) {
            score += 40;
        }
        return Some((score, (i..i + w.len()).collect()));
    }
    let (mut score, mut k, mut last) = (0i64, 0usize, -2i64);
    let mut at = Vec::new();
    for (i, &c) in hay.iter().enumerate() {
        if k >= w.len() {
            break;
        }
        if c != w[k] {
            continue;
        }
        let ii = i as i64;
        if ii == last + 1 {
            score += 6;
        } else if word_start(hay, orig, i) {
            score += 8;
        } else {
            score -= (ii - last).min(6);
        }
        at.push(i);
        last = ii;
        k += 1;
    }
    (k == w.len()).then_some((score, at))
}

fn best_sub(w: &[char], hay: &[char], orig: &[char]) -> Option<usize> {
    let mut first = None;
    if w.len() > hay.len() {
        return None;
    }
    for i in 0..=hay.len() - w.len() {
        if hay[i..i + w.len()] != *w {
            continue;
        }
        if word_start(hay, orig, i) {
            return Some(i);
        }
        first.get_or_insert(i);
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

#[cfg(test)]
mod tests {
    use super::*;

    // photon/fuzzy's TestMatch.
    #[test]
    fn match_scores() {
        let (a, _) = matches("dev", "dev-alerts").unwrap();
        let (b, _) = matches("dev", "random-devices").unwrap();
        let (c, at) = matches("pa", "prod-alerts").unwrap();
        assert!(a > b && c > 0, "scores {a} {b} {c}");
        assert_eq!(at, vec![0, 5]);
        assert!(matches("zz", "dev").is_none());
        matches("stan", "İstanbul"); // lowering İ changes the length
    }
}
