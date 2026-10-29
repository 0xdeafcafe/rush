//! What the editor colours, and what it finds wrong: markdown read from
//! the top for the frontmatter and code blocks it's in, JSON through the
//! highlighter, and each kind's problems. After Go's
//! internal/ui/docstyle.go (styles :32, runeStyles :71, mdState :99,
//! line :108, fenceOf :232, mdInline :244, problems :332, jsonProblems
//! :352, trailingComma :389, lineCol :398, formatJSON :406, docLinks
//! :416, mdProblems :467) and convo/highlighter.go.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use rush_core::text::{Color, Mods, Style};
use rush_hl::{Class, Lang, State as HlState};

use super::{DocEditor, Kind};
use crate::style::*;

/// A list item's start: its indent, its bullet or number, and a task box.
pub static MD_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)([-*+]|\d+[.)])\s(\[[ xX]\]\s)?").unwrap());
static MD_HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,6})(\s|$)").unwrap());
static MD_RULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([-*_])(\s*[-*_]){2,}\s*$").unwrap());
static MD_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]]*\]\(([^)\s]+)[^)]*\)").unwrap());

fn c(col: Color) -> Style {
    Style::fg(col)
}

fn with(s: Style, m: Mods) -> Style {
    Style {
        mods: s.mods.with(m),
        ..s
    }
}

// Syntax colours, muted to sit under rush's text (convo's style.go).
const HL_KW: Color = Color::Rgb(204, 153, 205);
const HL_STR: Color = Color::Rgb(163, 190, 140);
const HL_NUM: Color = Color::Rgb(222, 165, 132);
const HL_FN: Color = Color::Rgb(137, 180, 222);
const HL_TYPE: Color = Color::Rgb(120, 190, 175);
const HL_COMMENT: Color = Color::Rgb(122, 116, 108);

/// A highlighter for one file or fence: its language and what carries
/// from line to line (convo.Highlighter).
pub struct Highlighter {
    l: &'static Lang,
    st: HlState,
}

impl Highlighter {
    /// None when there's no language to highlight (NewHighlighter).
    pub fn new(name: &str) -> Option<Highlighter> {
        Some(Highlighter {
            l: Lang::for_name(name)?,
            st: HlState::default(),
        })
    }

    /// The next line's chars' styles, over base.
    pub fn line(&mut self, s: &str, base: Style) -> Vec<Style> {
        let mut out = Vec::with_capacity(s.len());
        self.l.line(&mut self.st, s, |i, j, cl| {
            let st = match cl {
                Class::Plain => base,
                Class::Keyword => c(HL_KW),
                Class::String => c(HL_STR),
                Class::Number => c(HL_NUM),
                Class::Function => c(HL_FN),
                Class::Type => c(HL_TYPE),
                Class::Comment => c(HL_COMMENT),
            };
            out.extend(std::iter::repeat_n(st, s[i..j].chars().count()));
        });
        let n = s.chars().count();
        out.resize(n, base);
        out
    }
}

impl DocEditor {
    /// Each char's style for lines first to last (styles).
    pub fn styles(&self, first: usize, last: usize) -> HashMap<usize, Vec<Style>> {
        let mut out = HashMap::new();
        let mut md = MdState::default();
        let mut hl = if self.kind == Kind::Json {
            Highlighter::new("json")
        } else {
            None
        };
        let (mut n, mut start) = (0, 0);
        let mut i = 0;
        while i <= self.buf.len() && n <= last {
            if i < self.buf.len() && self.buf[i] != '\n' {
                i += 1;
                continue;
            }
            let line = &self.buf[start..i];
            let show = n >= first;
            if self.kind == Kind::Markdown {
                let st = md.line(line, self.siblings.as_ref(), show);
                if show {
                    out.insert(n, st);
                }
            } else if show {
                match hl.as_mut() {
                    Some(h) => {
                        let s: String = line.iter().collect();
                        out.insert(n, h.line(&s, c(TEXT)));
                    }
                    None => {
                        out.insert(n, vec![c(TEXT); line.len()]);
                    }
                }
            }
            start = i + 1;
            n += 1;
            i += 1;
        }
        out
    }
}

/// What a markdown file's earlier lines leave open (mdState).
#[derive(Default)]
pub struct MdState {
    n: usize,
    /// 1 inside the frontmatter, 2 past it or there's none.
    front: u8,
    fence: String,
    code: Option<Highlighter>,
}

fn paint_range(st: &mut [Style], from: usize, to: usize, s: Style) {
    for x in st.iter_mut().take(to).skip(from) {
        *x = s;
    }
}

fn fill_all(st: &mut [Style], s: Style) {
    for x in st.iter_mut() {
        *x = s;
    }
}

/// How many chars s's first `bytes` bytes hold.
fn chars_in(s: &str, bytes: usize) -> usize {
    s[..bytes].chars().count()
}

/// The fence a line opens or closes a code block with (fenceOf).
pub fn fence_of(trim: &str) -> &'static str {
    for f in ["```", "~~~"] {
        if trim.starts_with(f) {
            return f;
        }
    }
    ""
}

impl MdState {
    /// Colours one line of markdown, or, when show is false, only follows
    /// what it opens and closes (line).
    pub fn line(&mut self, rs: &[char], sib: Option<&HashSet<String>>, show: bool) -> Vec<Style> {
        let st = self.line_inner(rs, sib, show);
        self.n += 1;
        st
    }

    fn line_inner(&mut self, rs: &[char], sib: Option<&HashSet<String>>, show: bool) -> Vec<Style> {
        let text: String = rs.iter().collect();
        let trim = text.trim();
        let mut st = if show {
            vec![c(TEXT); rs.len()]
        } else {
            Vec::new()
        };
        if self.n == 0 && trim == "---" {
            self.front = 1;
            fill_all(&mut st, c(DIM));
            return st;
        }
        if self.front == 1 {
            if trim == "---" {
                self.front = 2;
                fill_all(&mut st, c(DIM));
                return st;
            }
            if show {
                match text.find(':') {
                    Some(k) if k > 0 && !trim.starts_with('#') && !trim.starts_with('-') => {
                        let k = chars_in(&text, k);
                        let n = st.len();
                        paint_range(&mut st, 0, k, c(BLUE));
                        paint_range(&mut st, k, k + 1, c(DIM));
                        paint_range(&mut st, k + 1, n, c(SUB));
                    }
                    _ if trim.starts_with('#') => fill_all(&mut st, c(DIM)),
                    _ => fill_all(&mut st, c(SUB)),
                }
            }
            return st;
        }
        self.front = 2;
        let f = fence_of(trim);
        if !f.is_empty()
            && (self.fence.is_empty()
                || trim.starts_with(&self.fence)
                    && trim
                        .trim_start_matches(self.fence.chars().next().unwrap_or('`'))
                        .is_empty())
        {
            if self.fence.is_empty() {
                self.fence = f.to_string();
                let tag = trim
                    .trim_start_matches(f.chars().next().unwrap_or('`'))
                    .trim();
                self.code = Highlighter::new(tag);
            } else {
                self.fence.clear();
                self.code = None;
            }
            if show {
                fill_all(&mut st, c(DIM));
                if let Some(k) = text.find(|r: char| r != ' ' && r != '`' && r != '~') {
                    let n = st.len();
                    paint_range(&mut st, chars_in(&text, k), n, c(SUB));
                }
            }
            return st;
        }
        if !self.fence.is_empty() {
            match (show, self.code.as_mut()) {
                (false, Some(h)) => {
                    h.line(&text, c(SUB)); // keeps its strings and comments going
                }
                (true, Some(h)) => st = h.line(&text, c(SUB)),
                (true, None) => fill_all(&mut st, c(SUB)),
                (false, None) => {}
            }
            return st;
        }
        if !show {
            return st;
        }
        // Block starts, then what's inline in the rest.
        let mut from = 0;
        let mut base = c(TEXT);
        let n = st.len();
        if let Some(m) = MD_HEADING.captures(&text) {
            let hashes = m[1].len();
            base = with(c(TEXT), Mods::BOLD);
            if hashes <= 2 {
                base = with(c(ORANGE), Mods::BOLD);
            }
            paint_range(&mut st, 0, hashes, c(DIM));
            from = hashes;
            paint_range(&mut st, from, n, base);
        } else if MD_RULE.is_match(&text) {
            fill_all(&mut st, c(DIM));
            return st;
        } else if trim.starts_with('>') {
            let k = chars_in(&text, text.find('>').unwrap_or(0)) + 1;
            paint_range(&mut st, 0, k, c(ORANGE));
            base = with(c(SUB), Mods::ITALIC);
            from = k;
            paint_range(&mut st, from, n, base);
        } else if trim.starts_with('|') {
            for (i, r) in rs.iter().enumerate() {
                if *r == '|' {
                    st[i] = c(DIM);
                }
            }
        } else if let Some(m) = MD_ITEM.captures(&text) {
            let b = m.get(2).expect("bullet");
            let (b0, b1) = (chars_in(&text, b.start()), chars_in(&text, b.end()));
            paint_range(&mut st, b0, b1, c(ORANGE));
            from = chars_in(&text, m.get(0).expect("item").end());
            if let Some(bx) = m.get(3) {
                let c0 = chars_in(&text, bx.start());
                if bx.as_str().contains(['x', 'X']) {
                    paint_range(&mut st, c0, c0 + 3, c(GREEN));
                    base = with(c(DIM), Mods::STRIKE);
                    paint_range(&mut st, from, n, base);
                } else {
                    paint_range(&mut st, c0, c0 + 3, c(SUB));
                }
            }
        }
        md_inline(rs, &mut st, from, base, sib);
        st
    }
}

fn word_rune(r: char) -> bool {
    r.is_alphabetic() || r.is_numeric()
}

/// Colours code, emphasis, links and [[links]] in rs from from
/// (mdInline).
pub fn md_inline(
    rs: &[char],
    st: &mut [Style],
    from: usize,
    base: Style,
    sib: Option<&HashSet<String>>,
) {
    let find = |i: usize, pat: &[char]| -> Option<usize> {
        let mut j = i;
        while j + pat.len() <= rs.len() {
            if &rs[j..j + pat.len()] == pat {
                return Some(j);
            }
            j += 1;
        }
        None
    };
    let starts = |i: usize, p: &str| -> bool {
        let p: Vec<char> = p.chars().collect();
        rs.len() >= i + p.len() && rs[i..i + p.len()] == p[..]
    };
    let mut i = from;
    while i < rs.len() {
        let r = rs[i];
        if r == '`' {
            if let Some(j) = find(i + 1, &['`']) {
                paint_range(st, i, i + 1, c(DIM));
                paint_range(st, i + 1, j, c(YELLOW));
                paint_range(st, j, j + 1, c(DIM));
                i = j + 1;
                continue;
            }
        } else if r == '<' && starts(i, "<!--") {
            let j = find(i + 4, &['-', '-', '>']).unwrap_or(rs.len().saturating_sub(3));
            paint_range(st, i, j + 3, c(DIM));
            i = j + 3;
            continue;
        } else if r == '[' && i + 1 < rs.len() && rs[i + 1] == '[' {
            if let Some(j) = find(i + 2, &[']', ']']) {
                let mut col = BLUE;
                if let Some(sib) = sib
                    && !sib.contains(&rs[i + 2..j].iter().collect::<String>())
                {
                    col = SUB; // not written yet
                }
                paint_range(st, i, i + 2, c(DIM));
                paint_range(st, i + 2, j, c(col));
                paint_range(st, j, j + 2, c(DIM));
                i = j + 2;
                continue;
            }
        } else if r == '[' {
            if let Some(j) = find(i + 1, &[']'])
                && j + 1 < rs.len()
                && rs[j + 1] == '('
                && let Some(k) = find(j + 2, &[')'])
            {
                paint_range(st, i, i + 1, c(DIM));
                paint_range(st, i + 1, j, c(BLUE));
                paint_range(st, j, k + 1, c(DIM));
                i = k + 1;
                continue;
            }
        } else if (r == '*' || r == '_') && i + 1 < rs.len() && rs[i + 1] == r {
            if let Some(j) = find(i + 2, &[r, r])
                && j > i + 2
            {
                paint_range(st, i, i + 2, c(DIM));
                paint_range(st, i + 2, j, with(base, Mods::BOLD));
                paint_range(st, j, j + 2, c(DIM));
                i = j + 2;
                continue;
            }
        } else if (r == '*' || r == '_')
            && i + 1 < rs.len()
            && rs[i + 1] != ' '
            && (i == 0 || !word_rune(rs[i - 1]))
        {
            let mut j = i + 1;
            while j < rs.len()
                && !(rs[j] == r
                    && rs[j - 1] != ' '
                    && (j + 1 >= rs.len() || r == '*' || !word_rune(rs[j + 1])))
            {
                j += 1;
            }
            if j < rs.len() {
                paint_range(st, i, i + 1, c(DIM));
                paint_range(st, i + 1, j, with(base, Mods::ITALIC));
                paint_range(st, j, j + 1, c(DIM));
                i = j + 1;
                continue;
            }
        } else if r == 'h' && (starts(i, "https://") || starts(i, "http://")) {
            let mut j = i;
            while j < rs.len() && rs[j] != ' ' && rs[j] != ')' && rs[j] != '>' {
                j += 1;
            }
            paint_range(st, i, j, c(BLUE));
            i = j;
            continue;
        }
        st[i] = base;
        i += 1;
    }
}

// --- problems ---

/// A problem found in the file, at a line and column (chars) (docDiag).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diag {
    pub line: usize,
    pub col: usize,
    pub msg: String,
    /// A warning rather than an error.
    pub warn: bool,
}

impl DocEditor {
    /// What's wrong with the file as it stands (problems).
    pub fn problems(&mut self) -> &[Diag] {
        let links = self.links.version();
        if self.diag_ver == Some(self.ver)
            && (self.kind != Kind::Markdown || self.link_ver == links)
        {
            return &self.diags;
        }
        self.diag_ver = Some(self.ver);
        let text: String = self.buf.iter().collect();
        self.diags = match self.kind {
            Kind::Json => json_problems(&text),
            Kind::Markdown => {
                let links = self.links.clone();
                let d = md_problems(&self.path, &text, &|p| links.exists(p));
                self.link_ver = self.links.version();
                d
            }
            Kind::Text => Vec::new(),
        };
        &self.diags
    }
}

/// A byte offset as a line and a char column, from zero (lineCol).
pub fn line_col(text: &str, off: usize) -> (usize, usize) {
    let before = &text[..off];
    let l = before.matches('\n').count();
    let from = before.rfind('\n').map_or(0, |i| i + 1);
    (l, before[from..].chars().count())
}

/// Whether the closer at off follows a comma (trailingComma).
fn trailing_comma(text: &str, off: usize) -> bool {
    let b = text.as_bytes();
    if off >= b.len() || (b[off] != b'}' && b[off] != b']') {
        return false;
    }
    text[..off]
        .trim_end_matches([' ', '\t', '\r', '\n'])
        .ends_with(',')
}

/// Checks JSON strictly: its syntax, and names given twice in one object
/// (jsonProblems).
pub fn json_problems(text: &str) -> Vec<Diag> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut p = Json::new(text);
    let err = match p.value() {
        Ok(()) => {
            let rest = &text[p.at..];
            if !rest.trim().is_empty() {
                let off =
                    p.at + rest.len() - rest.trim_start_matches([' ', '\t', '\r', '\n']).len();
                let (line, col) = line_col(text, off);
                return vec![Diag {
                    line,
                    col,
                    msg: "more after the JSON ends: a file holds one value".into(),
                    warn: false,
                }];
            }
            return Vec::new();
        }
        Err(e) => e,
    };
    let (mut off, mut msg) = (err.off, err.msg);
    if err.eof {
        msg = "it ends before every { [ or \" is closed".into();
    } else if let Some(name) = err.twice {
        msg = format!(
            "{} is in this object twice; only one counts",
            go_quote(&name)
        );
    } else if trailing_comma(text, off) {
        msg = "a comma before the close: JSON allows none there".into();
        off = text[..off].rfind(',').unwrap_or(off);
    }
    if !err.pointer.is_empty() && !msg.contains("twice") {
        msg += &format!(" · in {}", err.pointer);
    }
    let (line, col) = line_col(text, off.min(text.len()));
    vec![Diag {
        line,
        col,
        msg,
        warn: false,
    }]
}

fn go_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// Lays JSON out again with the file's indent, keeping the order of its
/// names (formatJSON).
pub fn format_json(text: &str, indent: &str) -> Result<String, String> {
    let t = text.trim();
    let mut p = Json::new(t);
    p.value().map_err(|e| e.msg)?;
    if !t[p.at..].trim().is_empty() {
        return Err("more after the JSON ends".into());
    }
    let mut out = String::with_capacity(t.len() * 2);
    let mut depth = 0usize;
    let b = t.as_bytes();
    let mut i = 0;
    let nl = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(indent);
        }
    };
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'"' => {
                let j = string_end(b, i);
                out.push_str(&t[i..j]);
                i = j;
            }
            o @ (b'{' | b'[') => {
                let close = if o == b'{' { b'}' } else { b']' };
                let mut k = i + 1;
                while k < b.len() && matches!(b[k], b' ' | b'\t' | b'\r' | b'\n') {
                    k += 1;
                }
                out.push(o as char);
                if k < b.len() && b[k] == close {
                    out.push(close as char);
                    i = k + 1;
                } else {
                    depth += 1;
                    nl(&mut out, depth);
                    i += 1;
                }
            }
            c @ (b'}' | b']') => {
                depth = depth.saturating_sub(1);
                nl(&mut out, depth);
                out.push(c as char);
                i += 1;
            }
            b',' => {
                out.push(',');
                nl(&mut out, depth);
                i += 1;
            }
            b':' => {
                out.push_str(": ");
                i += 1;
            }
            _ => {
                let start = i;
                while i < b.len()
                    && !matches!(
                        b[i],
                        b' ' | b'\t' | b'\r' | b'\n' | b',' | b'}' | b']' | b':'
                    )
                {
                    i += 1;
                }
                out.push_str(&t[start..i]);
            }
        }
    }
    out.push('\n');
    Ok(out)
}

fn string_end(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// Why JSON doesn't read, and where.
struct JsonErr {
    off: usize,
    msg: String,
    eof: bool,
    twice: Option<String>,
    pointer: String,
}

/// A strict reader of one JSON value that says where it went wrong, in
/// encoding/json/jsontext's words, and the JSON pointer it was at.
struct Json<'a> {
    s: &'a str,
    b: &'a [u8],
    at: usize,
    path: Vec<String>,
}

impl<'a> Json<'a> {
    fn new(s: &'a str) -> Json<'a> {
        Json {
            s,
            b: s.as_bytes(),
            at: 0,
            path: Vec::new(),
        }
    }

    fn pointer(&self) -> String {
        self.path
            .iter()
            .map(|p| format!("/{}", p.replace('~', "~0").replace('/', "~1")))
            .collect()
    }

    fn err(&self, off: usize, msg: String) -> JsonErr {
        JsonErr {
            off,
            msg,
            eof: false,
            twice: None,
            pointer: self.pointer(),
        }
    }

    fn eof(&self) -> JsonErr {
        JsonErr {
            off: self.b.len(),
            msg: "unexpected EOF".into(),
            eof: true,
            twice: None,
            pointer: self.pointer(),
        }
    }

    fn ws(&mut self) {
        while self.at < self.b.len() && matches!(self.b[self.at], b' ' | b'\t' | b'\r' | b'\n') {
            self.at += 1;
        }
    }

    fn bad(&self, what: &str) -> JsonErr {
        let ch = self.s[self.at..].chars().next().unwrap_or(' ');
        self.err(
            self.at,
            format!("invalid character {} {what}", quote_char(ch)),
        )
    }

    fn value(&mut self) -> Result<(), JsonErr> {
        self.ws();
        let Some(&c) = self.b.get(self.at) else {
            return Err(self.eof());
        };
        match c {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(|_| ()),
            b'-' | b'0'..=b'9' => self.number(),
            b't' => self.literal("true"),
            b'f' => self.literal("false"),
            b'n' => self.literal("null"),
            _ => Err(self.bad("at start of value")),
        }
    }

    fn literal(&mut self, w: &str) -> Result<(), JsonErr> {
        for (k, want) in w.bytes().enumerate() {
            match self.b.get(self.at + k) {
                None => return Err(self.eof()),
                Some(&g) if g != want => {
                    self.at += k;
                    return Err(self.bad(&format!(
                        "within literal {w} (expecting {})",
                        quote_char(want as char)
                    )));
                }
                _ => {}
            }
        }
        self.at += w.len();
        Ok(())
    }

    fn number(&mut self) -> Result<(), JsonErr> {
        let start = self.at;
        if self.b[self.at] == b'-' {
            self.at += 1;
        }
        let digits = |j: &mut Json| {
            let from = j.at;
            while j.at < j.b.len() && j.b[j.at].is_ascii_digit() {
                j.at += 1;
            }
            j.at - from
        };
        match self.b.get(self.at) {
            None => return Err(self.eof()),
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return Err(self.bad("in number (expecting digit)")),
        }
        if self.b.get(self.at) == Some(&b'.') {
            self.at += 1;
            if self.at >= self.b.len() {
                return Err(self.eof());
            }
            if digits(self) == 0 {
                return Err(self.bad("in number (expecting digit)"));
            }
        }
        if matches!(self.b.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.b.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.at >= self.b.len() {
                return Err(self.eof());
            }
            if digits(self) == 0 {
                return Err(self.bad("in number (expecting digit)"));
            }
        }
        let _ = start;
        Ok(())
    }

    fn string(&mut self) -> Result<String, JsonErr> {
        let start = self.at;
        self.at += 1;
        loop {
            let Some(&c) = self.b.get(self.at) else {
                return Err(self.eof());
            };
            match c {
                b'"' => {
                    self.at += 1;
                    let raw = &self.s[start..self.at];
                    return Ok(serde_json::from_str::<String>(raw)
                        .unwrap_or_else(|_| raw[1..raw.len() - 1].to_string()));
                }
                b'\\' => match self.b.get(self.at + 1) {
                    None => return Err(self.eof()),
                    Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => self.at += 2,
                    Some(b'u') => {
                        for k in 0..4 {
                            match self.b.get(self.at + 2 + k) {
                                None => return Err(self.eof()),
                                Some(h) if h.is_ascii_hexdigit() => {}
                                Some(_) => {
                                    self.at += 2 + k;
                                    return Err(self.bad("in string escape code"));
                                }
                            }
                        }
                        self.at += 6;
                    }
                    Some(_) => {
                        self.at += 1;
                        return Err(self.bad("in string escape code"));
                    }
                },
                0..=0x1f => return Err(self.bad("in string literal")),
                _ => self.at += 1,
            }
        }
    }

    fn object(&mut self) -> Result<(), JsonErr> {
        self.at += 1;
        let mut names = HashSet::new();
        self.ws();
        if self.b.get(self.at) == Some(&b'}') {
            self.at += 1;
            return Ok(());
        }
        loop {
            self.ws();
            match self.b.get(self.at) {
                None => return Err(self.eof()),
                Some(b'"') => {}
                Some(_) => return Err(self.bad("at start of string (expecting '\"')")),
            }
            let name_at = self.at;
            let name = self.string()?;
            if !names.insert(name.clone()) {
                let mut e = self.err(
                    name_at,
                    format!("duplicate object member name {}", go_quote(&name)),
                );
                e.twice = Some(name);
                return Err(e);
            }
            self.ws();
            match self.b.get(self.at) {
                None => return Err(self.eof()),
                Some(b':') => self.at += 1,
                Some(_) => return Err(self.bad("after object name (expecting ':')")),
            }
            self.path.push(name);
            self.value()?;
            self.path.pop();
            self.ws();
            match self.b.get(self.at) {
                None => return Err(self.eof()),
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(());
                }
                Some(_) => return Err(self.bad("after object value (expecting ',' or '}')")),
            }
        }
    }

    fn array(&mut self) -> Result<(), JsonErr> {
        self.at += 1;
        self.ws();
        if self.b.get(self.at) == Some(&b']') {
            self.at += 1;
            return Ok(());
        }
        let mut n = 0;
        loop {
            self.path.push(n.to_string());
            self.value()?;
            self.path.pop();
            n += 1;
            self.ws();
            match self.b.get(self.at) {
                None => return Err(self.eof()),
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(());
                }
                Some(_) => return Err(self.bad("after array element (expecting ',' or ']')")),
            }
        }
    }
}

fn quote_char(c: char) -> String {
    match c {
        '\'' => "'\\''".into(),
        '"' => "'\"'".into(),
        c if (c as u32) < 0x20 => format!("'\\x{:02x}'", c as u32),
        c => format!("'{c}'"),
    }
}

/// Whether the files a markdown file links to exist, looked at in the
/// background: a link isn't a problem until it's known to be one
/// (docLinks).
#[derive(Clone, Default)]
pub struct Links(Arc<Mutex<LinksIn>>);

#[derive(Default)]
struct LinksIn {
    known: HashMap<String, bool>,
    asked: HashMap<String, Instant>,
    /// Bumps when an answer changes what's known.
    ver: u64,
    /// Look at once, not in the background (tests).
    now: bool,
}

/// How long what's known of a link stands before it's looked at again
/// (linkEvery).
pub const LINK_EVERY: Duration = Duration::from_secs(3);

impl Links {
    /// Looks at links on the calling thread: for tests.
    pub fn at_once(&self) {
        self.lock().now = true;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LinksIn> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether the file at p exists, and whether that's known yet; one not
    /// looked at lately is looked at in the background (exists).
    pub fn exists(&self, p: &str) -> (bool, bool) {
        let (ask, now) = {
            let mut l = self.lock();
            let ask = l.asked.get(p).is_none_or(|t| t.elapsed() >= LINK_EVERY);
            if ask {
                l.asked.insert(p.to_string(), Instant::now());
            }
            (ask, l.now)
        };
        if ask {
            let me = self.clone();
            let path = p.to_string();
            let look = move || {
                let ok = Path::new(&path).exists();
                let mut l = me.lock();
                if l.known.get(&path) != Some(&ok) {
                    l.known.insert(path, ok);
                    l.ver += 1;
                }
            };
            if now {
                look();
            } else {
                let _ = std::thread::Builder::new()
                    .name("rush-doc-links".into())
                    .spawn(look);
            }
        }
        let l = self.lock();
        match l.known.get(p) {
            Some(&ok) => (ok, true),
            None => (false, false),
        }
    }

    pub fn version(&self) -> u64 {
        self.lock().ver
    }
}

/// Checks markdown: frontmatter and code blocks that never close, what a
/// memory note, skill or agent needs in its frontmatter, and links to
/// files that don't exist, as exists knows them (mdProblems).
pub fn md_problems(path: &str, text: &str, exists: &dyn Fn(&str) -> (bool, bool)) -> Vec<Diag> {
    let mut out = Vec::new();
    let diag = |line, col, msg: String, warn| Diag {
        line,
        col,
        msg,
        warn,
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let mut front: HashMap<String, usize> = HashMap::new();
    let mut front_end: isize = -1;
    if lines.first().is_some_and(|l| l.trim() == "---") {
        for (i, l) in lines.iter().enumerate().skip(1) {
            let t = l.trim();
            if t == "---" {
                front_end = i as isize;
                break;
            }
            if let Some((k, _)) = t.split_once(':')
                && !t.starts_with('#')
            {
                front.entry(k.trim().to_string()).or_insert(i);
            }
        }
        if front_end < 0 {
            out.push(diag(
                0,
                0,
                "the frontmatter never closes: it needs a --- line after it".into(),
                false,
            ));
            front.clear();
        }
    }
    let (mut fence, mut fence_at) = ("", 0);
    let dir = Path::new(path).parent().unwrap_or(Path::new("."));
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if i as isize <= front_end {
            continue;
        }
        let f = fence_of(t);
        if !f.is_empty()
            && (fence.is_empty()
                || t.trim_start_matches(f.chars().next().unwrap_or('`'))
                    .is_empty())
        {
            if fence.is_empty() {
                fence = f;
                fence_at = i;
            } else {
                fence = "";
            }
            continue;
        }
        if !fence.is_empty() {
            continue;
        }
        for m in MD_LINK.captures_iter(l) {
            let g = m.get(1).expect("target");
            let target = g.as_str();
            if target.contains("://") || target.starts_with('#') || target.starts_with("mailto:") {
                continue;
            }
            let target = target.split('#').next().unwrap_or("");
            let p = if Path::new(target).is_absolute() {
                target.to_string()
            } else {
                dir.join(target).to_string_lossy().into_owned()
            };
            let (ok, known) = exists(&p);
            if known && !ok {
                out.push(diag(
                    i,
                    l[..g.start()].chars().count(),
                    format!("{target} doesn't exist"),
                    true,
                ));
            }
        }
    }
    if !fence.is_empty() {
        out.push(diag(
            fence_at,
            0,
            "this code block never closes".into(),
            true,
        ));
    }
    let need = |out: &mut Vec<Diag>, keys: &[&str], why: &str| {
        if front_end < 0 {
            out.push(diag(0, 0, format!("no frontmatter: {why}"), true));
            return;
        }
        let missing: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|k| !front.contains_key(*k))
            .collect();
        if !missing.is_empty() {
            out.push(diag(
                front_end as usize,
                0,
                format!("the frontmatter has no {}: {why}", missing.join(" or ")),
                true,
            ));
        }
    };
    let base = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir_base = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if base == "MEMORY.md" {
        if lines.len() > 200 {
            out.push(diag(
                200,
                0,
                "Claude loads only the first 200 lines of MEMORY.md".into(),
                true,
            ));
        }
    } else if dir_base == "memory" {
        need(
            &mut out,
            &["name", "description", "type"],
            "a memory note needs a name, a one-line description and a type",
        );
        if let Some(&i) = front.get("type") {
            let v = lines[i].split_once(':').map_or("", |(_, v)| v);
            if !matches!(
                v.trim().trim_matches(['"', '\'']),
                "user" | "feedback" | "project" | "reference"
            ) {
                out.push(diag(
                    i,
                    0,
                    "type is one of user, feedback, project or reference".into(),
                    true,
                ));
            }
        }
    } else if base == "SKILL.md" {
        need(
            &mut out,
            &["name", "description"],
            "Claude picks a skill by its description",
        );
    } else if dir_base == "agents" {
        need(
            &mut out,
            &["name", "description"],
            "Claude picks an agent by its description",
        );
    }
    out
}
