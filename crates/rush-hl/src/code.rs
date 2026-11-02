//! Pure helpers rush draws code with: what changed in a diff's line, and the
//! prefix a read or a search puts before a line of code. Ported from
//! internal/convo/highlight.go:150 (changed), :198 (codePrefix) and :245
//! (lineNo); their colours stay in convo.

/// The span of `a` that differs from `b`, by their common start and end, on
/// UTF-8 boundaries; `None` when nearly all of it changed, as then there's
/// nothing worth pointing out.
pub fn changed(a: &str, b: &str) -> Option<(usize, usize)> {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let mut p = 0;
    while p < x.len() && p < y.len() && x[p] == y[p] {
        p += 1;
    }
    while p > 0 && p < x.len() && !a.is_char_boundary(p) {
        p -= 1;
    }
    let mut q = 0;
    while q < x.len() - p
        && q < y.len().saturating_sub(p)
        && x[x.len() - 1 - q] == y[y.len() - 1 - q]
    {
        q += 1;
    }
    while q > 0 && !a.is_char_boundary(x.len() - q) {
        q -= 1;
    }
    let (from, to) = (p, x.len() - q);
    if to <= from || p + q == 0 || (p + q) * 4 < x.len() {
        return None;
    }
    Some((from, to))
}

/// The words of `a` and of `b` that differ, by a diff of their words:
/// byte spans of each, on UTF-8 boundaries, in order, the changed words
/// with only spaces between them as one span. `None` when the two have
/// too little in common for the words to be worth pointing out (then the
/// whole line is the change), or when they're the same.
#[allow(clippy::type_complexity)]
pub fn changed_words(a: &str, b: &str) -> Option<(Vec<(usize, usize)>, Vec<(usize, usize)>)> {
    if a == b {
        return None;
    }
    let (x, y) = (words(a), words(b));
    // Past this many word pairs, the cheap test of common start and end.
    if x.len() * y.len() > 250_000 {
        let (from, to) = changed(a, b)?;
        let (bf, bt) = changed(b, a)?;
        return Some((vec![(from, to)], vec![(bf, bt)]));
    }
    // The longest run of words the two share, table from the end.
    let (n, m) = (x.len(), y.len());
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * (m + 1) + j] = if tok(a, x[i]) == tok(b, y[j]) {
                lcs[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
            };
        }
    }
    let (mut keep_a, mut keep_b) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if tok(a, x[i]) == tok(b, y[j]) {
            keep_a[i] = true;
            keep_b[j] = true;
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let same: usize = x
        .iter()
        .zip(&keep_a)
        .filter(|(_, k)| **k)
        .map(|(t, _)| t.1 - t.0)
        .sum();
    if same * 4 < a.len().max(b.len()) {
        return None;
    }
    let spans = |s: &str, ts: &[(usize, usize)], keep: &[bool]| {
        let mut out: Vec<(usize, usize)> = Vec::new();
        let mut k = 0;
        while k < ts.len() {
            if keep[k] {
                k += 1;
                continue;
            }
            let from = ts[k].0;
            let mut to = ts[k].1;
            k += 1;
            // Spaces kept between two changed words join them.
            loop {
                let mut q = k;
                while q < ts.len() && keep[q] && tok(s, ts[q]).trim().is_empty() {
                    q += 1;
                }
                if q > k && q < ts.len() && !keep[q] {
                    to = ts[q].1;
                    k = q + 1;
                } else if k < ts.len() && !keep[k] {
                    to = ts[k].1;
                    k += 1;
                } else {
                    break;
                }
            }
            out.push((from, to));
        }
        out
    };
    let (sa, sb) = (spans(a, &x, &keep_a), spans(b, &y, &keep_b));
    if sa.is_empty() && sb.is_empty() {
        return None;
    }
    Some((sa, sb))
}

/// The text of word t of s.
fn tok(s: &str, t: (usize, usize)) -> &str {
    &s[t.0..t.1]
}

/// A line's words as byte spans: a run of letters, digits and _, a run of
/// spaces, or any other character alone.
fn words(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let mut j = i + c.len_utf8();
        if word(c) || c.is_whitespace() {
            let space = c.is_whitespace();
            while let Some(&(k, d)) = it.peek() {
                if (space && d.is_whitespace()) || (!space && word(d)) {
                    j = k + d.len_utf8();
                    it.next();
                } else {
                    break;
                }
            }
        }
        out.push((i, j));
    }
    out
}

/// How long the prefix is that a read or a search puts before a line of
/// code: "  12→" or "12\t" from a read, "a.go:12:" or "a.go-12-" from grep,
/// and the path in it, if any ("" for none).
pub fn code_prefix(l: &str) -> (usize, &str) {
    let s = l.as_bytes();
    let mut i = 0;
    while i < s.len() && s[i] == b' ' {
        i += 1;
    }
    // path: or path- before the number, as grep writes it; a path can have
    // a - of its own.
    let start = i;
    let word = &s[i..];
    let word = match word.iter().position(|&c| c == b' ' || c == b'\t') {
        Some(k) => &word[..k],
        None => word,
    };
    for k in 1..word.len() {
        if word[k] != b':' && word[k] != b'-' {
            continue;
        }
        let j = i + k + 1;
        let mut m = j;
        while m < s.len() && s[m].is_ascii_digit() {
            m += 1;
        }
        if m > j && m < s.len() && (s[m] == b':' || s[m] == b'-') && l[i..i + k].contains('.') {
            return (m + 1, &l[i..i + k]);
        }
    }
    let mut j = start;
    while j < s.len() && s[j].is_ascii_digit() {
        j += 1;
    }
    if j == start {
        return (0, "");
    }
    if l[j..].starts_with('→') {
        return (j + '→'.len_utf8(), "");
    }
    // grep -n on one file: 12: for a match, 12- for the lines around.
    if j < s.len() && matches!(s[j], b'\t' | b':' | b'-') {
        return (j + 1, "");
    }
    (0, "")
}

/// The line number a code prefix gives, 0 for none.
pub fn line_no(prefix: &str) -> usize {
    let p = prefix.trim_end_matches(|c: char| !c.is_ascii_digit());
    let k = p.len() - p.bytes().rev().take_while(u8::is_ascii_digit).count();
    p[k..].parse().unwrap_or(0)
}
