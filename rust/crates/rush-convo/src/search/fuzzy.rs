//! A word of a query matches text loosely: anywhere in it, as with find,
//! or, failing that, as a word of the text a letter or two off (a typo).
//! Each match scores how close it is, so the best of many come first.
//! Port of internal/convo/fuzzy.go:1-138 and search.go:118 (findFold).

/// A whole word beats the start of one, which beats the middle of one,
/// which beats a typo (fuzzy.go:14).
pub(crate) const SCORE_WHOLE: i32 = 100;
pub(crate) const SCORE_START: i32 = 80;
pub(crate) const SCORE_INSIDE: i32 = 60;
/// Less 10 for each letter off past the first.
pub(crate) const SCORE_TYPO: i32 = 40;
/// The words, as typed, side by side in the text.
pub(crate) const SCORE_PHRASE: i32 = 50;

/// How much of a text is read for typos: past it a word is only found as
/// it was typed.
const TYPO_SCAN: usize = 64 << 10;

/// How many letters a word may be off by: none when short, so "fix"
/// doesn't find "fox", two when long.
pub(crate) fn typos(w: &str) -> usize {
    match w.chars().count() {
        n if n < 4 => 0,
        n if n < 8 => 1,
        _ => 2,
    }
}

/// c in lower case, one char for one (Go's unicode.ToLower).
pub(crate) fn lower(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(a), None) => a,
        (Some(a), Some(_)) if a.is_ascii() => a, // İ is i
        _ => c,
    }
}

/// Whether c is part of a word: a letter, a digit or _.
pub(crate) fn word_rune(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric() || c == '_'
}

/// s in lower case, char for char.
pub(crate) fn lower_str(s: &str) -> String {
    if s.bytes().all(|b| !b.is_ascii_uppercase() && b.is_ascii()) {
        return s.to_string();
    }
    s.chars().map(lower).collect()
}

/// Finds w in s ignoring case, char by char, and returns the byte span of
/// the match in s itself: lowering can change a string's length, so an
/// index into a lowered copy doesn't fit the original (search.go:118).
pub(crate) fn find_fold(s: &str, w: &str) -> Option<(usize, usize)> {
    let mut wc = w.chars().map(lower);
    let Some(first) = wc.next() else {
        return Some((0, 0));
    };
    let rest: Vec<char> = wc.collect();
    'outer: for (i, c) in s.char_indices() {
        if lower(c) != first {
            continue;
        }
        let mut end = i + c.len_utf8();
        let mut sc = s[end..].chars();
        for &want in &rest {
            match sc.next() {
                Some(c) if lower(c) == want => end += c.len_utf8(),
                _ => continue 'outer,
            }
        }
        return Some((i, end));
    }
    None
}

/// How well w matches text, and the byte span in text of the match; None
/// when it doesn't (fuzzy.go:41 wordMatch).
pub(crate) fn word_match(text: &str, w: &str) -> Option<(i32, usize, usize)> {
    if let Some((i, j)) = find_fold(text, w) {
        let mut score = SCORE_INSIDE;
        let before = text[..i].chars().next_back();
        let after = text[j..].chars().next();
        if before.is_none_or(|c| !word_rune(c)) {
            score = SCORE_START;
            if after.is_none_or(|c| !word_rune(c)) {
                score = SCORE_WHOLE;
            }
        }
        return Some((score, i, j));
    }
    let k = typos(w);
    if k == 0 {
        return None;
    }
    let mut cut = text.len().min(TYPO_SCAN);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let hay = &text[..cut];
    let lw: Vec<char> = w.chars().map(lower).collect();
    let (mut best, mut bi, mut bj) = (k + 1, usize::MAX, 0);
    let mut word = Vec::new();
    let mut it = hay.char_indices().peekable();
    while let Some(&(i, c)) = it.peek() {
        if !word_rune(c) {
            it.next();
            continue;
        }
        let mut j = i;
        word.clear();
        while let Some(&(at, c)) = it.peek() {
            if !word_rune(c) {
                break;
            }
            j = at + c.len_utf8();
            word.push(lower(c));
            it.next();
        }
        let d = distance(&lw, &word, k);
        if d < best {
            (best, bi, bj) = (d, i, j);
            if d == 1 {
                break;
            }
        }
    }
    if bi == usize::MAX {
        return None;
    }
    Some((SCORE_TYPO - 10 * (best as i32 - 1), bi, bj))
}

/// How many letters a and b are apart (added, dropped, changed or two
/// swapped), or more than k when that's all that's known.
pub(crate) fn distance<T: PartialEq + Copy>(a: &[T], b: &[T], k: usize) -> usize {
    if a.len().abs_diff(b.len()) > k {
        return k + 1;
    }
    let n = b.len() + 1;
    // Three rows of the table, two back for a swap; on the stack when the
    // word is short, as nearly all are.
    let mut stack = [[0usize; 33]; 3];
    let mut heap: Vec<usize>;
    let (mut prev2, mut prev, mut cur): (&mut [usize], &mut [usize], &mut [usize]) = if n <= 33 {
        let [a, b, c] = &mut stack;
        (&mut a[..n], &mut b[..n], &mut c[..n])
    } else {
        heap = vec![0usize; 3 * n];
        let (a, rest) = heap.split_at_mut(n);
        let (b, c) = rest.split_at_mut(n);
        (a, b, c)
    };
    for (j, p) in prev.iter_mut().enumerate() {
        *p = j;
    }
    for i in 1..=a.len() {
        cur[0] = i;
        let mut low = cur[0];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                cur[j] = cur[j].min(prev2[j - 2] + 1);
            }
            low = low.min(cur[j]);
        }
        if low > k {
            return k + 1;
        }
        (prev2, prev, cur) = (prev, cur, prev2);
    }
    prev[b.len()]
}

/// Where w matches s, as a search finds it (a typo too), as a byte span
/// of s; None when it doesn't (fuzzy.go:131 MatchSpan).
pub fn match_span(s: &str, w: &str) -> Option<(usize, usize)> {
    if w.trim().is_empty() {
        return None;
    }
    word_match(s, w).map(|(_, i, j)| (i, j))
}

/// w found in s ignoring case, as a byte span of s (find.go:313 FindFold).
pub fn find_fold_span(s: &str, w: &str) -> Option<(usize, usize)> {
    if w.trim().is_empty() {
        return None;
    }
    find_fold(s, w)
}
