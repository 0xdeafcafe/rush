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
///
/// Rust-only speed: when w starts with an ASCII character, only the
/// bytes that can lower to it are tried (memchr): its two cases, and the
/// lead byte of the one character outside ASCII that lowers to it, if
/// any (İ to i, the Kelvin sign to k; `LOWERS_TO_ASCII`). Same answer
/// as trying every character.
pub(crate) fn find_fold(s: &str, w: &str) -> Option<(usize, usize)> {
    let mut wc = w.chars().map(lower);
    let Some(first) = wc.next() else {
        return Some((0, 0));
    };
    let rest = wc;
    // The rest of w matched from end on; the match's end.
    let tail = |mut end: usize| {
        let mut sc = s[end..].chars();
        for want in rest.clone() {
            match sc.next() {
                Some(c) if lower(c) == want => end += c.len_utf8(),
                _ => return None,
            }
        }
        Some(end)
    };
    if first.is_ascii() {
        let lo = first as u8;
        let up = first.to_ascii_uppercase() as u8;
        let other = LOWERS_TO_ASCII
            .iter()
            .find(|(_, a)| *a == first)
            .map_or(lo, |(c, _)| {
                let mut b = [0u8; 4];
                c.encode_utf8(&mut b);
                b[0]
            });
        let b = s.as_bytes();
        let mut from = 0;
        while let Some(off) = memchr::memchr3(lo, up, other, &b[from..]) {
            let i = from + off;
            // Each byte looked for starts a character.
            let c = s[i..].chars().next()?;
            if lower(c) == first
                && let Some(end) = tail(i + c.len_utf8())
            {
                return Some((i, end));
            }
            from = i + 1;
        }
        return None;
    }
    for (i, c) in s.char_indices() {
        if lower(c) != first {
            continue;
        }
        if let Some(end) = tail(i + c.len_utf8()) {
            return Some((i, end));
        }
    }
    None
}

/// The characters outside ASCII that [`lower`] makes ASCII, and what to.
pub(crate) const LOWERS_TO_ASCII: [(char, char); 2] = [('\u{130}', 'i'), ('\u{212a}', 'k')];

/// Whether the character at byte i of s is part of a word, its length in
/// bytes and the character; ASCII without decoding.
#[inline]
fn word_at(s: &str, i: usize) -> (bool, usize, char) {
    let b = s.as_bytes()[i];
    if b < 0x80 {
        return (b.is_ascii_alphanumeric() || b == b'_', 1, b as char);
    }
    let c = s[i..].chars().next().unwrap_or(' ');
    (word_rune(c), c.len_utf8(), c)
}

/// bit of each ASCII word byte, lowered; 0 for the rest.
static ASCII_BITS: [u64; 256] = {
    let mut t = [0u64; 256];
    let mut x = 0;
    while x < 128 {
        let b = x as u8;
        if b.is_ascii_alphanumeric() || b == b'_' {
            t[x] = 1 << (b.to_ascii_lowercase() as u32 % 64);
        }
        x += 1;
    }
    t
};

/// A bit for each (lowered) character, folded into 64: a word whose bits
/// lack more of w's than k can't be within k letters of it, as each
/// letter missing takes an edit of its own to put in.
#[inline]
fn bit(c: char) -> u64 {
    1 << (c as u32 % 64)
}

/// The words of hay, each lowered, as far as a typo is looked for: the
/// closest to lw within k letters so far is best (its distance, and its
/// byte span offset by at). True once one a letter off is found: nothing
/// closer can be (fuzzy.go:41's loop). Rust-only speed: a word too long
/// or short, or missing too many of lw's letters, is passed over without
/// working out its distance, which would be over k.
fn typo_scan(
    hay: &str,
    at: usize,
    lw: &[char],
    k: usize,
    best: &mut (usize, usize, usize),
    word: &mut Vec<char>,
) -> bool {
    let want = lw.iter().fold(0, |m, c| m | bit(*c));
    let b = hay.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        // Past what's no part of a word, ASCII a byte at a time.
        let x = b[i];
        if x < 0x80 {
            if ASCII_BITS[x as usize] == 0 {
                i += 1;
                continue;
            }
        } else {
            let (is, len, _) = word_at(hay, i);
            if !is {
                i += len;
                continue;
            }
        }
        let start = i;
        let (mut chars, mut has) = (0usize, 0u64);
        while i < n {
            let x = b[i];
            let m = ASCII_BITS[x as usize];
            if m != 0 {
                has |= m;
                i += 1;
            } else if x < 0x80 {
                break;
            } else {
                let (is, len, c) = word_at(hay, i);
                if !is {
                    break;
                }
                has |= bit(lower(c));
                i += len;
            }
            chars += 1;
        }
        if chars.abs_diff(lw.len()) > k || (want & !has).count_ones() as usize > k {
            continue; // distance would say more than k
        }
        word.clear();
        word.extend(hay[start..i].chars().map(lower));
        let d = distance(lw, word, k);
        if d < best.0 {
            *best = (d, at + start, at + i);
            if d == 1 {
                return true;
            }
        }
    }
    false
}

/// How well w matches text, and the byte span in text of the match; None
/// when it doesn't (fuzzy.go:41 wordMatch).
pub(crate) fn word_match(text: &str, w: &str) -> Option<(i32, usize, usize)> {
    if let Some(m) = exact_match(text, w) {
        return Some(m);
    }
    let k = typos(w);
    if k == 0 {
        return None;
    }
    let mut cut = text.len().min(TYPO_SCAN);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let lw: Vec<char> = w.chars().map(lower).collect();
    let mut best = (k + 1, usize::MAX, 0);
    typo_scan(&text[..cut], 0, &lw, k, &mut best, &mut Vec::new());
    let (best, bi, bj) = best;
    if bi == usize::MAX {
        return None;
    }
    Some((SCORE_TYPO - 10 * (best as i32 - 1), bi, bj))
}

/// word_match's score of w in the parts joined by newlines, without
/// joining them (Rust-only: a step's label, command, file and output are
/// searched as one text in Go).
pub(crate) fn word_score_parts(parts: &[&str], w: &str) -> Option<i32> {
    // A newline is no part of a word nor of w: a match is in one part,
    // and starts or ends a word there as it would in the whole.
    for p in parts {
        if let Some((sc, _, _)) = exact_match(p, w) {
            return Some(sc);
        }
    }
    let k = typos(w);
    if k == 0 {
        return None;
    }
    let lw: Vec<char> = w.chars().map(lower).collect();
    let mut best = (k + 1, usize::MAX, 0);
    let mut word = Vec::new();
    let mut at = 0;
    for p in parts {
        if at >= TYPO_SCAN {
            break;
        }
        let mut cut = p.len().min(TYPO_SCAN - at);
        while !p.is_char_boundary(cut) {
            cut -= 1;
        }
        if typo_scan(&p[..cut], at, &lw, k, &mut best, &mut word) {
            break;
        }
        at += p.len() + 1;
    }
    if best.1 == usize::MAX {
        return None;
    }
    Some(SCORE_TYPO - 10 * (best.0 as i32 - 1))
}

/// w found in text as typed (ignoring case), scored by whether it's a
/// whole word, a word's start or inside one.
fn exact_match(text: &str, w: &str) -> Option<(i32, usize, usize)> {
    let (i, j) = find_fold(text, w)?;
    let mut score = SCORE_INSIDE;
    let before = text[..i].chars().next_back();
    let after = text[j..].chars().next();
    if before.is_none_or(|c| !word_rune(c)) {
        score = SCORE_START;
        if after.is_none_or(|c| !word_rune(c)) {
            score = SCORE_WHOLE;
        }
    }
    Some((score, i, j))
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
