//! Code as rush colours it: rush-hl's classes to colours, a diff's changed
//! words on their own ground, and JSON laid out. Port of
//! internal/convo/highlight.go:56-150 (langFor, highlight, paintCode,
//! hlColour) and render.go:3893 (prettyJSON).

use rush_core::text::{Color, Style};
use rush_hl::{Class, Lang, State};

use super::style::{C_OUT, HL_COMMENT, HL_FN, HL_KW, HL_NUM, HL_OUT_COMMENT, HL_STR, HL_TYPE};
use super::txt::Txt;

pub(crate) type HlState = State;

/// The language of a fence tag, a file name or an interpreter.
pub(crate) fn lang_for(name: &str) -> Option<&'static Lang> {
    Lang::for_name(name)
}

/// A span of a line (byte offsets) drawn on its own ground: a diff line's
/// changed words.
#[derive(Clone, Copy)]
pub(crate) struct Emph {
    pub from: usize,
    pub to: usize,
    pub on: Color,
}

/// What rush draws a class of code in, over base.
fn colour(c: Class, base: Style) -> Style {
    let s = match c {
        Class::Keyword => HL_KW,
        Class::String => HL_STR,
        Class::Number => HL_NUM,
        Class::Function => HL_FN,
        Class::Type => HL_TYPE,
        Class::Comment if base == C_OUT => HL_OUT_COMMENT,
        Class::Comment => HL_COMMENT,
        Class::Plain => return base,
    };
    Style { bg: base.bg, ..s }
}

/// One line of code in language l over colour base, going on from st and
/// leaving st for the next line; with no language, all in base.
pub(crate) fn highlight(
    l: Option<&Lang>,
    st: &mut HlState,
    s: &str,
    base: Style,
    em: Option<Emph>,
) -> Txt {
    let mut out = Txt::new();
    if s.is_empty() {
        return out;
    }
    rush_hl::line(l, st, s, |i, j, c| {
        let style = colour(c, base);
        let mut i = i;
        while i < j {
            let mut k = j;
            let mut sty = style;
            if let Some(e) = em {
                let on = i >= e.from && i < e.to;
                if on {
                    sty.bg = e.on;
                }
                if i < e.from && e.from < k {
                    k = e.from;
                }
                if i < e.to && e.to < k {
                    k = e.to;
                }
            }
            out.push_str(&s[i..k], sty);
            i = k;
        }
    });
    out
}

/// s laid out two spaces a level when it's one JSON object or array, or
/// as it is when it's JSON lines; None for anything else.
pub(crate) fn pretty_json(s: &str) -> Option<String> {
    let t = s.trim();
    if t.len() < 2 || t.len() > 4 << 20 || !(t.starts_with('{') || t.starts_with('[')) {
        return None;
    }
    if serde_json::from_str::<serde::de::IgnoredAny>(t).is_ok() {
        return Some(indent_json(t));
    }
    for l in t.split('\n') {
        let l = l.trim();
        if !l.is_empty()
            && (!(l.starts_with('{') || l.starts_with('['))
                || serde_json::from_str::<serde::de::IgnoredAny>(l).is_err())
        {
            return None;
        }
    }
    Some(t.to_string())
}

/// Valid JSON indented as Go's json.Indent does it with two spaces: values
/// kept as written, an empty object or array left closed up.
fn indent_json(t: &str) -> String {
    let b = t.as_bytes();
    let mut out = String::with_capacity(t.len() * 2);
    let mut depth = 0usize;
    let mut i = 0;
    let nl = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    };
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                out.push_str(&t[start..(i + 1).min(b.len())]);
            }
            b'{' | b'[' => {
                // An empty one stays closed up.
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < b.len() && (b[j] == b'}' || b[j] == b']') {
                    out.push(c as char);
                    out.push(b[j] as char);
                    i = j;
                } else {
                    out.push(c as char);
                    depth += 1;
                    nl(&mut out, depth);
                }
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                nl(&mut out, depth);
                out.push(c as char);
            }
            b',' => {
                out.push(',');
                nl(&mut out, depth);
            }
            b':' => out.push_str(": "),
            c if c.is_ascii_whitespace() => {}
            _ => {
                let start = i;
                while i + 1 < b.len()
                    && !matches!(b[i + 1], b',' | b'}' | b']' | b':')
                    && !b[i + 1].is_ascii_whitespace()
                {
                    i += 1;
                }
                out.push_str(&t[start..=i]);
            }
        }
        i += 1;
    }
    out
}

/// s laid out for reading and the language to colour it in, when it's
/// JSON, XML or HTML, or YAML (pretty.go:16).
pub(crate) fn pretty_text(s: &str) -> Option<(String, Option<&'static Lang>)> {
    let t = s.trim();
    match structure(t) {
        b'j' => Some((reindent_json(t), lang_for("json"))),
        b'x' => Some((reindent_xml(t), None)),
        b'y' => Some((super::udiff::expand_tabs(t), lang_for("yaml"))),
        _ => None,
    }
}

/// What t looks like: b'j' JSON, b'x' XML or HTML, b'y' YAML, 0 none.
pub(crate) fn structure(t: &str) -> u8 {
    let b = t.as_bytes();
    if b.len() < 2 {
        return 0;
    }
    match b[0] {
        b'{' | b'[' => {
            let rest = t[1..].trim_start_matches([' ', '\t', '\r', '\n']);
            if rest.is_empty() || "\"{[]}-0123456789tfn".contains(rest.as_bytes()[0] as char) {
                return b'j';
            }
        }
        b'<' => {
            let c = b[1];
            if c == b'?' || c == b'!' || c == b'/' || c.is_ascii_alphabetic() {
                return b'x';
            }
        }
        _ => {}
    }
    if looks_yaml(t) { b'y' } else { 0 }
}

fn looks_yaml(t: &str) -> bool {
    let first = t.split('\n').next().unwrap_or("");
    if first.trim() == "---" {
        return true;
    }
    let k = match first.split_once(": ") {
        Some((k, _)) => k,
        None => match first.strip_suffix(':') {
            Some(k) => k,
            None => return false,
        },
    };
    !k.is_empty()
        && k.chars()
            .all(|r| r == '_' || r == '-' || r == '.' || r.is_ascii_alphanumeric())
        && t.matches(':').count() >= 2
}

/// Whether t reads better laid out than as it came (pretty.go:72).
pub(crate) fn better_pretty(t: &str) -> bool {
    let t = t.trim();
    let lines = t.matches('\n').count() + 1;
    match structure(t) {
        b'j' => {
            lines <= 2
                || t.len() <= 4 << 20 && serde_json::from_str::<serde::de::IgnoredAny>(t).is_ok()
        }
        b'x' => lines <= 2 || t.len() / lines > 200,
        _ => false,
    }
}

/// JSON laid out two spaces a level from its tokens alone, however it was
/// cut (pretty.go:88).
pub(crate) fn reindent_json(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() * 2);
    let mut depth = 0usize;
    fn nl(out: &mut String, depth: usize) {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    }
    let ws = |c: u8| matches!(c, b' ' | b'\t' | b'\r' | b'\n');
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let top = |out: &mut String, depth: usize| {
            if depth == 0 && !out.is_empty() {
                out.push('\n');
            }
        };
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => {}
            b'{' | b'[' => {
                top(&mut out, depth);
                out.push(c as char);
                let mut j = i + 1;
                while j < b.len() && ws(b[j]) {
                    j += 1;
                }
                if j < b.len() && (b[j] == b'}' || b[j] == b']') {
                    out.push(b[j] as char);
                    i = j + 1;
                    continue;
                }
                depth += 1;
                if j < b.len() {
                    nl(&mut out, depth);
                }
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                nl(&mut out, depth);
                out.push(c as char);
            }
            b',' => {
                out.push(',');
                nl(&mut out, depth);
            }
            b':' => out.push_str(": "),
            b'"' => {
                top(&mut out, depth);
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                let j = (j + 1).min(b.len());
                out.push_str(&s[i..j]);
                i = j;
                continue;
            }
            _ => {
                top(&mut out, depth);
                let mut j = i;
                while j < b.len() && !b" \t\r\n{}[],:\"".contains(&b[j]) {
                    j += 1;
                }
                out.push_str(&s[i..j]);
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Each tag on a line of its own, two spaces a level (pretty.go:158).
pub(crate) fn reindent_xml(s: &str) -> String {
    let mut out = String::new();
    let (mut depth, mut last) = (0usize, 0u8);
    let put = |out: &mut String, depth: usize, t: &str| {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&"  ".repeat(depth));
        out.push_str(t);
    };
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] != b'<' {
            let j = s[i..].find('<').unwrap_or(s.len() - i);
            let t = s[i..i + j].split_whitespace().collect::<Vec<_>>().join(" ");
            if !t.is_empty() {
                if last == b'o' {
                    out.push_str(&t);
                    last = b't';
                } else {
                    put(&mut out, depth, &t);
                    last = 0;
                }
            }
            i += j;
            continue;
        }
        let mut j = tag_end(s, i);
        let tag = &s[i..j];
        let name = tag_name(tag).to_ascii_lowercase();
        if tag.starts_with("</") {
            depth = depth.saturating_sub(1);
            if last != 0 {
                out.push_str(tag);
            } else {
                put(&mut out, depth, tag);
            }
            last = 0;
        } else if tag.starts_with("<!")
            || tag.starts_with("<?")
            || tag.ends_with("/>")
            || matches!(
                name.as_str(),
                "area"
                    | "base"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "link"
                    | "meta"
                    | "source"
                    | "track"
                    | "wbr"
            )
            || !tag.ends_with('>')
        {
            put(&mut out, depth, tag);
            last = 0;
        } else {
            put(&mut out, depth, tag);
            depth += 1;
            last = b'o';
            if name == "script" || name == "style" {
                let k = s[j..]
                    .to_ascii_lowercase()
                    .find(&format!("</{name}"))
                    .unwrap_or(s.len() - j);
                let body = s[j..j + k].trim();
                if !body.is_empty() {
                    for l in body.split('\n') {
                        put(&mut out, depth + 1, l.trim());
                    }
                    last = 0;
                }
                j += k;
            }
        }
        i = j;
    }
    out
}

fn tag_end(s: &str, i: usize) -> usize {
    for (a, z) in [("<!--", "-->"), ("<![CDATA[", "]]>")] {
        if s[i..].starts_with(a) {
            return match s[i + a.len()..].find(z) {
                Some(k) => i + a.len() + k + z.len(),
                None => s.len(),
            };
        }
    }
    let b = s.as_bytes();
    let mut quote = 0u8;
    for (j, &c) in b.iter().enumerate().skip(i + 1) {
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return j + 1;
        }
    }
    s.len()
}

fn tag_name(tag: &str) -> &str {
    let t = tag.trim_start_matches(['<', '/']);
    match t.find([' ', '\t', '\r', '\n', '/', '>']) {
        Some(k) => &t[..k],
        None => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_laid_out() {
        assert_eq!(
            pretty_json(r#"{"a":1,"b":[true,{}],"c":"x, y"}"#).unwrap(),
            "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    {}\n  ],\n  \"c\": \"x, y\"\n}"
        );
        assert!(pretty_json("not json").is_none());
        assert_eq!(
            pretty_json("{\"a\":1}\n{\"b\":2}").unwrap(),
            "{\"a\":1}\n{\"b\":2}"
        );
    }

    #[test]
    fn code_takes_colours() {
        let mut st = HlState::default();
        let t = highlight(lang_for("go"), &mut st, "func f() {}", C_OUT, None);
        assert_eq!(t.plain(), "func f() {}");
        assert_eq!(t.0[0].style.fg, HL_KW.fg);
    }
}
