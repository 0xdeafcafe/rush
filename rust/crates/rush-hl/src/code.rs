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
