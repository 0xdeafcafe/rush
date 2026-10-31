//! The list's colours, grounds and number formats. After Go's
//! internal/ui/style.go (applyColors :51, fit :106, right :204, oneLine
//! :212, age :240, dur :253, mem :270, money :281, moneyShort :295,
//! tokens :316, rule :401, tidyFit :418) and rail.go:24 (railColor).

use rush_core::text::{Color, Line, Mods, Span, Style, width};
use rush_core::theme::DARK;
use rush_core::time::Ms;

pub const TEXT: Color = DARK.text;
pub const SUB: Color = DARK.sub;
pub const DIM: Color = DARK.dim;
pub const FAINT: Color = DARK.faint;
pub const ORANGE: Color = DARK.orange;
pub const YELLOW: Color = DARK.yellow;
pub const BLUE: Color = DARK.blue;
pub const GREEN: Color = DARK.green;
pub const RED: Color = DARK.red;
pub const BRIGHT: Color = Color::Rgb(240, 236, 228);
pub const QUEUE: Color = Color::Rgb(178, 160, 214);

pub const SEL_BG: Color = DARK.sel_bg;
pub const HOVER_BG: Color = DARK.hover_bg;
pub const PANEL_BG: Color = DARK.panel_bg;
/// Every other agent in the rail look sits on it.
pub const BAND_BG: Color = Color::Rgb(25, 23, 21);
/// A row that asks you something, and an error; the second of each when
/// it's the row picked.
pub const ASK_BG: Color = Color::Rgb(44, 38, 24);
pub const ASK_SEL_BG: Color = Color::Rgb(62, 52, 30);
pub const ERR_BG: Color = Color::Rgb(46, 28, 26);
pub const ERR_SEL_BG: Color = Color::Rgb(66, 36, 32);
/// The Session's chrome, and its tab showing.
pub const CHROME_BG: Color = Color::Rgb(30, 28, 26);
pub const TAB_ON_BG: Color = Color::Rgb(17, 16, 14);
/// The place tabs: the one you're in filled orange, the rest quiet.
pub const TAB_OFF_BG: Color = Color::Rgb(40, 37, 34);
pub const TAB_ON_FG: Color = Color::Rgb(24, 22, 20);

pub const SPINNER: [&str; 10] = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];

pub fn st(c: Color) -> Style {
    Style::fg(c)
}
pub fn bold(c: Color) -> Style {
    Style::fg(c).bold()
}

pub fn sp(text: impl Into<String>, c: Color) -> Span {
    Span::new(text, Style::fg(c))
}
pub fn spb(text: impl Into<String>, c: Color) -> Span {
    Span::new(text, Style::fg(c).bold())
}
pub fn plain(text: impl Into<String>) -> Span {
    Span::plain(text)
}

/// A line of spans, empty ones dropped.
pub fn line(spans: impl IntoIterator<Item = Span>) -> Line {
    Line::from_spans(spans.into_iter().filter(|s| !s.text.is_empty()).collect())
}

/// Appends `o`'s spans to `l`.
pub fn join(l: &mut Line, o: Line) {
    l.spans.extend(o.spans);
}

/// `l` padded or cut to exactly `w` cells: cut with … unless all it loses
/// is blanks.
pub fn fit(l: Line, w: usize) -> Line {
    if w == 0 {
        return Line::new();
    }
    let lw = l.width();
    if lw > w {
        let cut = rush_draw::truncate_line(&l, w, "");
        let kept = cut.plain().len();
        let all = l.plain();
        if all[kept.min(all.len())..].trim_end_matches(' ').is_empty() {
            return cut;
        }
        return rush_draw::truncate_line(&l, w, "…");
    }
    pad(l, w)
}

/// `l` with blanks up to `w` cells.
pub fn pad(mut l: Line, w: usize) -> Line {
    let lw = l.width();
    if lw < w {
        l.spans.push(Span::plain(" ".repeat(w - lw)));
    }
    l
}

/// `s` padded or cut to `w` cells, as fit does a line.
pub fn fit_str(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let sw = width(s);
    if sw > w {
        let cut = rush_draw::truncate(s, w, "");
        if s[cut.len()..].trim_end_matches(' ').is_empty() {
            return cut.into_owned();
        }
        return rush_draw::truncate(s, w, "…").into_owned();
    }
    let mut out = String::with_capacity(s.len() + w - sw);
    out.push_str(s);
    out.extend(std::iter::repeat_n(' ', w - sw));
    out
}

/// `s` right-aligned in exactly `w` cells.
pub fn right(s: &str, w: usize) -> String {
    let sw = width(s);
    if sw >= w {
        return rush_draw::truncate(s, w, "…").into_owned();
    }
    let mut out = " ".repeat(w - sw);
    out.push_str(s);
    out
}

pub fn blanks(n: usize) -> String {
    " ".repeat(n)
}

/// `l` fit to `w` on ground `bg`: every span without a ground of its own
/// takes it, and the rest of the row is filled.
pub fn on_ground(l: Line, w: usize, bg: Color) -> Line {
    let mut l = fit(l, w);
    for s in &mut l.spans {
        if s.style.bg == Color::Default {
            s.style.bg = bg;
        }
    }
    l.bg = bg;
    l
}

/// Whitespace of any kind collapsed to single spaces, trimmed.
pub fn one_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    one_line_into(&mut out, s);
    out
}

/// Whether s is printable ASCII that is one line already: words with one
/// space between them, none before or after. Its [`one_line`] is itself,
/// a cell a byte.
pub fn plain_one_line(s: &str) -> bool {
    let b = s.as_bytes();
    b.first() != Some(&b' ')
        && b.last() != Some(&b' ')
        && b.iter().all(|c| (0x20..0x7f).contains(c))
        && !b.windows(2).any(|w| w == b"  ")
}

/// [`one_line`] of s, written over out (to reuse it).
pub fn one_line_into(out: &mut String, s: &str) {
    out.clear();
    // Most are one line already: single spaces between words, no others.
    let b = s.as_bytes();
    if s.is_ascii()
        && b.first() != Some(&b' ')
        && b.last() != Some(&b' ')
        && !b.windows(2).any(|w| w == b"  ")
        && !b
            .iter()
            .any(|c| matches!(c, b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
    {
        out.push_str(s);
        return;
    }
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
}

/// Markdown's marks taken out of a row's summary (Go's mdPlain).
pub fn unmark(s: &str) -> String {
    if !s.contains(['*', '`', '_']) {
        return s.to_string();
    }
    s.replace("**", "").replace('`', "").replace("__", "")
}

const SEC: Ms = 1000;
const MIN: Ms = 60 * SEC;
const HOUR: Ms = 60 * MIN;

/// As the native view: 3s, 12m, 4h, 2d.
pub fn age(d: Ms) -> String {
    if d < MIN {
        format!("{}s", d / SEC)
    } else if d < HOUR {
        format!("{}m", d / MIN)
    } else if d < 48 * HOUR {
        format!("{}h", d / HOUR)
    } else {
        format!("{}d", d / HOUR / 24)
    }
}

pub fn dur(d: Ms) -> String {
    if d <= 0 {
        "–".into()
    } else if d < MIN {
        format!("{}s", d / SEC)
    } else if d < HOUR {
        format!("{}m", d / MIN)
    } else if d >= 48 * HOUR {
        let h = d / HOUR;
        format!("{}d{:02}h", h / 24, h % 24)
    } else {
        format!("{}h{:02}m", d / HOUR, d / MIN % 60)
    }
}

pub fn mem(b: u64) -> String {
    if b == 0 {
        "–".into()
    } else if b >= 1 << 30 {
        format!("{:.1}G", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{}M", b >> 20)
    }
}

pub fn money(c: f64) -> String {
    if c <= 0.0 {
        "–".into()
    } else if c < 10.0 {
        format!("${c:.2}")
    } else {
        format!("${}", thousands((c + 0.5) as i64))
    }
}

/// money in at most six characters: past $9,999 it counts thousands.
pub fn money_short(c: f64) -> String {
    if c < 10_000.0 {
        money(c)
    } else if c < 99_950.0 {
        format!("${:.1}k", c / 1e3)
    } else if c < 999_500.0 {
        format!("${:.0}k", c / 1e3)
    } else {
        format!("${:.1}M", c / 1e6)
    }
}

pub fn thousands(n: i64) -> String {
    let s = n.to_string();
    let neg = s.starts_with('-');
    let digits = if neg { &s[1..] } else { &s[..] };
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    if neg {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn tokens(n: i64) -> String {
    if n >= 1_000_000 && n % 1_000_000 == 0 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

/// A section title with a hairline running to the edge.
pub fn rule(title: &str, meta: &str, w: usize) -> Line {
    let mut l = line([spb(title, SUB)]);
    if !meta.is_empty() {
        l.spans.push(plain("  "));
        l.spans.push(sp(meta, DIM));
    }
    let n = w.saturating_sub(l.width() + 2);
    l.spans.push(plain(" "));
    l.spans.push(sp("─".repeat(n), FAINT));
    l
}

/// "key label" pairs, the key brighter, joined by a faint dot.
pub fn keys(pairs: &[(&str, &str)]) -> Line {
    let mut l = Line::new();
    for (i, (k, what)) in pairs.iter().enumerate() {
        if i > 0 {
            l.spans.push(sp("  ·  ", FAINT));
        }
        l.spans.push(spb(*k, TEXT));
        l.spans.push(sp(format!(" {what}"), DIM));
    }
    l
}

/// keys, dropping the pairs before the last until they fit in `w`.
pub fn keys_fit(w: usize, pairs: &[(&str, &str)]) -> Line {
    let mut pairs = pairs.to_vec();
    while pairs.len() > 1 {
        let l = keys(&pairs);
        if l.width() <= w {
            return l;
        }
        pairs.remove(pairs.len() - 2);
    }
    keys(&pairs)
}

/// Your home as ~.
pub fn tildify(p: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if !home.is_empty()
            && let Some(rest) = p.strip_prefix(home.as_ref())
        {
            return format!("~{rest}");
        }
    }
    p.to_string()
}

/// fit for a sentence that may not fit: home as ~, a long path keeps its
/// start and its file, and what still runs over ends on a word.
pub fn tidy_fit(s: &str, w: usize) -> String {
    if w == 0 || width(s) <= w {
        return fit_str(s, w);
    }
    let mut s = s.to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if !home.is_empty() {
            s = s.replace(&format!("{home}/"), "~/");
        }
    }
    let mut words: Vec<String> = s.split(' ').map(str::to_string).collect();
    let mut keep = 2;
    while keep >= 1 && width(&words.join(" ")) > w {
        for x in &mut words {
            let parts: Vec<&str> = x.split('/').collect();
            if parts.len() > keep + 2 {
                *x = format!(
                    "{}/{}/…/{}",
                    parts[0],
                    parts[1],
                    parts[parts.len() - keep..].join("/")
                );
            }
        }
        keep -= 1;
    }
    let s = words.join(" ");
    if width(&s) <= w {
        return fit_str(&s, w);
    }
    let mut cut = rush_draw::truncate(&s, w.saturating_sub(1), "").into_owned();
    if let Some(i) = cut.rfind(' ')
        && !s[cut.len()..].starts_with(' ')
        && i > 0
        && width(&cut[..i]) >= w * 2 / 3
    {
        cut.truncate(i);
    }
    let t = cut.trim_end_matches([' ', ',', '.', ';', ':', '·', '-']);
    fit_str(&format!("{t}…"), w)
}

/// A colour `t` of the way from a to b.
pub fn mix(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let l = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
            Color::Rgb(l(ar, br), l(ag, bg), l(ab, bb))
        }
        _ if t < 0.5 => a,
        _ => b,
    }
}

/// The text with no style: a quick check for tests.
pub fn text_of(l: &Line) -> String {
    l.plain()
}

pub const NONE: Mods = Mods::NONE;

// ---- The rest of Go's style.go (cpuColor :325, memColor :336, bar :343,
// wrap :360, highlight :387) and view.go:1801 (hoverLine). ----

/// A CPU figure coloured by how busy it is.
pub fn cpu_color(p: f64, s: impl Into<String>) -> Span {
    if p >= 50.0 {
        sp(s, YELLOW)
    } else if p >= 1.0 {
        sp(s, GREEN)
    } else {
        plain(s)
    }
}

/// A memory figure, yellow past a gigabyte.
pub fn mem_color(b: u64, s: impl Into<String>) -> Span {
    if b >= 1 << 30 {
        sp(s, YELLOW)
    } else {
        plain(s)
    }
}

/// A 10-cell usage meter.
pub fn bar(pct: f64) -> Line {
    let n = ((pct / 10.0 + 0.5) as isize).clamp(0, 10) as usize;
    let c = if pct >= 80.0 {
        RED
    } else if pct >= 50.0 {
        YELLOW
    } else {
        GREEN
    };
    line([sp("▰".repeat(n), c), sp("▱".repeat(10 - n), FAINT)])
}

/// l fit to w on the selection's ground (highlight).
pub fn highlight(l: Line, w: usize) -> Line {
    on_ground(l, w, SEL_BG)
}

/// l fit to w on the pointer's ground (hoverLine).
pub fn hover_line(l: Line, w: usize) -> Line {
    on_ground(l, w, HOVER_BG)
}

/// s with its reverse video on: a cursor's block.
pub fn reversed(mut st: Style) -> Style {
    st.mods = st.mods.with(Mods::REVERSE);
    st
}

/// The rows chars wraps to in w cells, as [from, to) offsets: at spaces
/// (dropped at a break) and after - and /, a word longer than a row
/// broken (ansi.Wrap(s, w, " -/")).
fn wrap_ranges(cs: &[char], w: usize) -> Vec<(usize, usize)> {
    let cw = |c: char| width(c.encode_utf8(&mut [0; 4]));
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let (mut row_w, mut row_end) = (0, 0);
    let mut i = 0;
    while i < cs.len() {
        let gap_from = i;
        while i < cs.len() && cs[i] == ' ' {
            i += 1;
        }
        if i >= cs.len() {
            break; // spaces at the end go
        }
        let gap = i - gap_from;
        let wf = i;
        let mut ww = 0;
        while i < cs.len() && cs[i] != ' ' {
            ww += cw(cs[i]);
            i += 1;
            if matches!(cs[i - 1], '-' | '/') {
                break;
            }
        }
        match start {
            // A paragraph's leading spaces stay; a row's after a break go.
            None if out.is_empty() => {
                start = Some(gap_from);
                row_w = gap;
            }
            None => {
                start = Some(wf);
                row_w = 0;
            }
            Some(s) if row_w + gap + ww > w => {
                out.push((s, row_end));
                start = Some(wf);
                row_w = 0;
            }
            Some(_) => row_w += gap,
        }
        // Placed: a word wider than the row is broken where it ends.
        for (p, &c) in cs.iter().enumerate().take(i).skip(wf) {
            let c = cw(c);
            if row_w + c > w && row_w > 0 {
                out.push((start.unwrap_or(wf), p));
                start = Some(p);
                row_w = 0;
            }
            row_w += c;
        }
        row_end = i;
    }
    match start {
        Some(s) => out.push((s, row_end)),
        None => out.push((0, 0)),
    }
    out
}

/// s wrapped to w cells (at least 8), each paragraph on its own; an
/// empty paragraph an empty row (wrap).
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(8);
    let mut out = Vec::new();
    for para in s.split('\n') {
        let para = para.trim_end_matches(' ');
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let cs: Vec<char> = para.chars().collect();
        for (f, t) in wrap_ranges(&cs, w) {
            out.push(cs[f..t].iter().collect());
        }
    }
    out
}

/// A styled line wrapped as wrap does its words, each span keeping its
/// style.
pub fn wrap_line(l: &Line, w: usize) -> Vec<Line> {
    let w = w.max(8);
    let mut cs = Vec::new();
    let mut st = Vec::new();
    for s in &l.spans {
        for c in s.text.chars() {
            cs.push(c);
            st.push(s.style);
        }
    }
    let mut out = Vec::new();
    for (f, t) in wrap_ranges(&cs, w) {
        let mut row = Line::new();
        let mut run = String::new();
        let mut at: Option<Style> = None;
        for j in f..t {
            if at != Some(st[j]) {
                if let Some(a) = at {
                    row.spans.push(Span::new(std::mem::take(&mut run), a));
                }
                at = Some(st[j]);
            }
            run.push(cs[j]);
        }
        if let Some(a) = at {
            row.spans.push(Span::new(run, a));
        }
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_as_split_on_whitespace() {
        for s in [
            "",
            " ",
            "fix the test",
            " fix",
            "fix ",
            "fix  the",
            "fix\tthe\nflaky\rtest",
            "a\x0bb\x0cc",
            "\x1fa b",
            "café  au lait",
            "a\u{a0}b\u{3000}c",
            "x",
        ] {
            let want = s.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(one_line(s), want, "{s:?}");
            if plain_one_line(s) {
                assert_eq!((s, s.len()), (want.as_str(), width(&want)), "{s:?}");
            }
        }
    }

    #[test]
    fn formats() {
        let cases: &[(Ms, &str, &str)] = &[
            (0, "0s", "–"),
            (59_999, "59s", "59s"),
            (60_000, "1m", "1m"),
            (3_599_000, "59m", "59m"),
            (3_600_000, "1h", "1h00m"),
            (7_380_000, "2h", "2h03m"),
            (47 * HOUR, "47h", "47h00m"),
            (49 * HOUR, "2d", "2d01h"),
        ];
        for &(d, a, u) in cases {
            assert_eq!(age(d), a, "age {d}");
            assert_eq!(dur(d), u, "dur {d}");
        }
        let money_cases: &[(f64, &str, &str)] = &[
            (0.0, "–", "–"),
            (0.244, "$0.24", "$0.24"),
            (9.99, "$9.99", "$9.99"),
            (10.4, "$10", "$10"),
            (1234.5, "$1,235", "$1,235"),
            (12_345.0, "$12,345", "$12.3k"),
            (123_456.0, "$123,456", "$123k"),
            (2_500_000.0, "$2,500,000", "$2.5M"),
        ];
        for &(c, m, s) in money_cases {
            assert_eq!(money(c), m);
            assert_eq!(money_short(c), s);
        }
        assert_eq!(mem(0), "–");
        assert_eq!(mem(296 << 20), "296M");
        assert_eq!(mem(3 << 30), "3.0G");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(101_500), "101k");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_500_000), "1.5M");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn fits() {
        assert_eq!(fit_str("abc", 5), "abc  ");
        assert_eq!(fit_str("abcdef", 4), "abc…");
        assert_eq!(fit_str("ab    ", 4), "ab  ");
        assert_eq!(right("7m", 5), "   7m");
        assert_eq!(right("abcdef", 4), "abc…");
        let l = line([sp("hello ", TEXT), sp("world", DIM)]);
        assert_eq!(fit(l.clone(), 8).plain(), "hello w…");
        assert_eq!(fit(l, 13).plain(), "hello world  ");
        assert_eq!(one_line(" a\n b\t c "), "a b c");
        assert_eq!(unmark("**bold** `x`"), "bold x");
    }

    #[test]
    fn wraps_like_ansi_wrap() {
        let cases: &[(&str, usize, &[&str])] = &[
            ("hello world", 20, &["hello world"]),
            ("hello world again", 11, &["hello world", "again"]),
            ("one two three four", 9, &["one two", "three", "four"]),
            // Go's ansi.Wrap(s, w, " -/") gives these.
            (
                "a well-known path/to/file",
                12,
                &["a well-known", "path/to/file"],
            ),
            (
                "a well-known path/to/file",
                9,
                &["a well-", "known", "path/to/", "file"],
            ),
            ("  indented text here", 10, &["  indented", "text here"]),
            ("ab abcdefghijkl", 8, &["ab", "abcdefgh", "ijkl"]),
            ("abcdefghijklmnop", 8, &["abcdefgh", "ijklmnop"]),
            ("", 10, &[""]),
            ("a\n\nb", 10, &["a", "", "b"]),
        ];
        for (s, w, want) in cases {
            assert_eq!(wrap(s, *w), *want, "wrap({s:?}, {w})");
        }
        let l = line([sp("red words ", RED), sp("blue words", BLUE)]);
        let rows = wrap_line(&l, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].plain(), "red words");
        assert_eq!(rows[1].spans[0].style.fg, BLUE);
    }

    #[test]
    fn tidy() {
        assert_eq!(tidy_fit("short", 10), "short     ");
        let s = tidy_fit("editing /a/b/c/d/e/file.rs now", 24);
        assert_eq!(width(&s), 24);
        assert!(s.contains("…/"), "{s}");
    }
}
