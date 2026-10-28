//! A small syntax highlighter: one pass over a line's bytes, no regular
//! expressions, state carried from line to line for strings and block
//! comments. Port of photon/hl
//! (~/go/pkg/mod/github.com/0xdeafcafe/photon@v0.0.0-20261008232558-b01e070b3366/hl/hl.go).
//! It reports classes, not colours, so the caller decides how each is drawn.
//!
//! API (fixed; other crates build against it):
//! - `Lang::for_name(name)`: a language from a fence tag, file name or
//!   interpreter; `None` when there's none to highlight.
//! - `lang.line(&mut state, line, |start, end, class| …)`: the runs of one
//!   line, as byte offsets, in order and without gaps (plain ones too), so
//!   they cover the whole line; adjacent runs of one class are merged.
//! - `State::fence()`: inside a markdown fence, the fenced language.

pub mod code;

use std::sync::LazyLock;

/// What a span of code is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// Anything not below.
    Plain,
    /// A keyword.
    Keyword,
    /// A string, or inline code in Markdown.
    String,
    /// A number or a constant such as true or nil.
    Number,
    /// A comment.
    Comment,
    /// A call, a key, a variable (Go's hl.Func).
    Function,
    /// A type.
    Type,
}

/// A language the highlighter knows (hl.go:27).
pub struct Lang {
    pub name: &'static str,
    id: usize,           // its word table in WORDS
    line: &'static str,  // a line comment's start: //, #, --
    line2: &'static str, // a second one, as # in PHP
    open: &'static str,  // a block comment's ends
    shut: &'static str,
    quotes: &'static [u8], // characters that quote a string on one line
    multi: u8,             // a quote that may run over lines: ` in Go and JS
    triple: bool,          // """ and ''' strings, over lines
    caps: bool,            // a capitalised word is a type
    vars: bool,            // $name is a variable
    keys: bool,            // a key before : (JSON, YAML, TOML's =)
    md: bool,              // Markdown, drawn by its own rules
    sh: bool,              // shell: a ' after a word still opens a string
    dash: bool,            // CSS: a - goes on a word
}

impl std::fmt::Debug for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lang").field("name", &self.name).finish()
    }
}

impl PartialEq for Lang {
    fn eq(&self, o: &Self) -> bool {
        std::ptr::eq(self, o)
    }
}
impl Eq for Lang {}

const BASE: Lang = Lang {
    name: "",
    id: 0,
    line: "",
    line2: "",
    open: "",
    shut: "",
    quotes: b"",
    multi: 0,
    triple: false,
    caps: false,
    vars: false,
    keys: false,
    md: false,
    sh: false,
    dash: false,
};

/// Each language's keywords, types and constants (hl.go:58), by its id.
const WORD_LISTS: [[&str; 3]; 15] = [
    // 0 go
    [
        "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var",
        "bool byte complex64 complex128 error float32 float64 int int8 int16 int32 int64 rune string uint uint8 uint16 uint32 uint64 uintptr any comparable",
        "true false nil iota",
    ],
    // 1 js
    [
        "async await break case catch class const continue debugger default delete do else export extends finally for from function if import in instanceof let new of return static super switch this throw try typeof var void while with yield as type interface enum implements declare readonly keyof satisfies namespace abstract private protected public",
        "string number boolean any unknown never object void bigint symbol Promise Array Record Map Set",
        "true false null undefined NaN Infinity",
    ],
    // 2 python
    [
        "and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield match case",
        "int str float bool list dict set tuple bytes object type self cls",
        "True False None",
    ],
    // 3 rust
    [
        "as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move mut pub ref return static struct super trait type unsafe use where while",
        "i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize f32 f64 bool char str String Vec Option Result Box Self self",
        "true false None Some Ok Err",
    ],
    // 4 shell
    [
        "if then else elif fi for in do done while until case esac function return local export readonly unset set shift exit break continue source eval exec trap",
        "echo printf cd test read declare",
        "true false",
    ],
    // 5 json
    ["", "", "true false null"],
    // 6 yaml
    ["", "", "true false null yes no on off ~"],
    // 7 toml
    ["", "", "true false"],
    // 8 css
    [
        "important media import supports keyframes font-face",
        "",
        "inherit initial unset none auto",
    ],
    // 9 sql
    [
        "select from where and or not in is null as join left right inner outer full on group by order having limit offset insert into values update set delete create table index view drop alter add column primary key foreign references unique default distinct union all case when then else end with returning exists between like asc desc begin commit rollback if SELECT FROM WHERE AND OR NOT IN IS NULL AS JOIN LEFT RIGHT INNER OUTER ON GROUP BY ORDER HAVING LIMIT OFFSET INSERT INTO VALUES UPDATE SET DELETE CREATE TABLE INDEX VIEW DROP ALTER ADD COLUMN PRIMARY KEY FOREIGN REFERENCES UNIQUE DEFAULT DISTINCT UNION ALL CASE WHEN THEN ELSE END WITH RETURNING EXISTS BETWEEN LIKE ASC DESC BEGIN COMMIT ROLLBACK IF",
        "int integer bigint text varchar char boolean timestamp timestamptz date jsonb json uuid serial numeric real INT INTEGER BIGINT TEXT VARCHAR BOOLEAN TIMESTAMP DATE JSONB UUID SERIAL NUMERIC",
        "true false TRUE FALSE",
    ],
    // 10 ruby
    [
        "alias and begin break case class def defined do else elsif end ensure for if in module next not or redo rescue retry return self super then undef unless until when while yield require require_relative attr_accessor attr_reader",
        "",
        "true false nil",
    ],
    // 11 the C family: C, C++, C#, Java, Kotlin, Swift, Dart
    [
        "auto break case catch class const continue default delete do else enum extern final finally for fun func goto if implements import in include interface let namespace new override package private protected public return sizeof static struct switch template this throw throws try typedef typename union using val var virtual void volatile when while guard defer extension protocol init self super import object data sealed suspend",
        "int long short char float double bool boolean byte string String unsigned signed size_t Int Long Double Float Bool Boolean",
        "true false null nullptr nil NULL",
    ],
    // 12 lua
    [
        "and break do else elseif end for function goto if in local not or repeat return then until while",
        "",
        "true false nil",
    ],
    // 13 php
    [
        "abstract and array as break case catch class clone const continue declare default do echo else elseif empty endif extends final finally fn for foreach function global if implements include interface isset list match namespace new or print private protected public readonly require return static switch throw trait try unset use var while yield",
        "int string bool float array object mixed void",
        "true false null TRUE FALSE NULL",
    ],
    // 14 markdown: none
    ["", "", ""],
];

// A word's class in a table: keyword, type, constant.
const KW: u8 = b'k';
const TY: u8 = b't';
const CO: u8 = b'c';

/// A language's words in a small open-addressed hash table, so a word's
/// class is a hash and a compare or two, without allocating.
struct Words {
    slots: Box<[(&'static str, u8)]>, // "" is an empty slot
    mask: usize,
    longest: usize,
}

impl Words {
    fn new(lists: &[&'static str; 3]) -> Words {
        let n: usize = lists
            .iter()
            .map(|l| l.split_ascii_whitespace().count())
            .sum();
        let size = (n * 2).next_power_of_two().max(1);
        let mut w = Words {
            slots: vec![("", 0); size].into_boxed_slice(),
            mask: size - 1,
            longest: 0,
        };
        // Later lists win a word in two, as Go's map does.
        for (list, class) in lists.iter().zip([KW, TY, CO]) {
            for word in list.split_ascii_whitespace() {
                w.longest = w.longest.max(word.len());
                let mut k = hash(word.as_bytes()) & w.mask;
                while !w.slots[k].0.is_empty() && w.slots[k].0 != word {
                    k = (k + 1) & w.mask;
                }
                w.slots[k] = (word, class);
            }
        }
        w
    }

    /// The class of word w: KW, TY, CO, or None.
    fn get(&self, w: &[u8]) -> Option<u8> {
        if w.len() > self.longest {
            return None;
        }
        let mut k = hash(w) & self.mask;
        loop {
            let (s, c) = self.slots[k];
            if s.is_empty() {
                return None;
            }
            if s.as_bytes() == w {
                return Some(c);
            }
            k = (k + 1) & self.mask;
        }
    }
}

fn hash(w: &[u8]) -> usize {
    let mut h: u64 = 0;
    for &b in w {
        h = (h.rotate_left(5) ^ b as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    (h >> 32) as usize
}

/// The word tables by language id, built on first use.
static WORDS: LazyLock<Vec<Words>> = LazyLock::new(|| WORD_LISTS.iter().map(Words::new).collect());

pub static GO: Lang = Lang {
    name: "go",
    id: 0,
    line: "//",
    open: "/*",
    shut: "*/",
    quotes: b"\"'",
    multi: b'`',
    caps: true,
    ..BASE
};
pub static JS: Lang = Lang {
    name: "javascript",
    id: 1,
    line: "//",
    open: "/*",
    shut: "*/",
    quotes: b"\"'",
    multi: b'`',
    caps: true,
    ..BASE
};
pub static PYTHON: Lang = Lang {
    name: "python",
    id: 2,
    line: "#",
    quotes: b"\"'",
    triple: true,
    ..BASE
};
pub static RUST: Lang = Lang {
    name: "rust",
    id: 3,
    line: "//",
    open: "/*",
    shut: "*/",
    quotes: b"\"",
    caps: true,
    ..BASE
};
pub static SH: Lang = Lang {
    name: "sh",
    id: 4,
    line: "#",
    quotes: b"\"'",
    vars: true,
    sh: true,
    ..BASE
};
pub static JSON: Lang = Lang {
    name: "json",
    id: 5,
    quotes: b"\"",
    keys: true,
    line: "//",
    ..BASE
};
pub static YAML: Lang = Lang {
    name: "yaml",
    id: 6,
    line: "#",
    quotes: b"\"'",
    keys: true,
    ..BASE
};
pub static TOML: Lang = Lang {
    name: "toml",
    id: 7,
    line: "#",
    quotes: b"\"'",
    keys: true,
    triple: true,
    ..BASE
};
pub static CSS: Lang = Lang {
    name: "css",
    id: 8,
    open: "/*",
    shut: "*/",
    quotes: b"\"'",
    line: "//",
    dash: true,
    ..BASE
};
pub static SQL: Lang = Lang {
    name: "sql",
    id: 9,
    line: "--",
    open: "/*",
    shut: "*/",
    quotes: b"'\"",
    ..BASE
};
pub static RUBY: Lang = Lang {
    name: "ruby",
    id: 10,
    line: "#",
    quotes: b"\"'",
    caps: true,
    ..BASE
};
/// The C family: C, C++, C#, Java, Kotlin, Swift, Dart.
pub static C: Lang = Lang {
    name: "c",
    id: 11,
    line: "//",
    open: "/*",
    shut: "*/",
    quotes: b"\"'",
    caps: true,
    ..BASE
};
pub static LUA: Lang = Lang {
    name: "lua",
    id: 12,
    line: "--",
    quotes: b"\"'",
    ..BASE
};
pub static PHP: Lang = Lang {
    name: "php",
    id: 13,
    line: "//",
    line2: "#",
    open: "/*",
    shut: "*/",
    quotes: b"\"'",
    vars: true,
    caps: true,
    ..BASE
};
pub static MARKDOWN: Lang = Lang {
    name: "markdown",
    id: 14,
    md: true,
    ..BASE
};

/// A language by a fence's tag or a file's extension (hl.go:112).
static BY_NAME: &[(&str, &Lang)] = &[
    ("go", &GO),
    ("golang", &GO),
    ("js", &JS),
    ("javascript", &JS),
    ("jsx", &JS),
    ("mjs", &JS),
    ("cjs", &JS),
    ("ts", &JS),
    ("typescript", &JS),
    ("tsx", &JS),
    ("mts", &JS),
    ("cts", &JS),
    ("py", &PYTHON),
    ("python", &PYTHON),
    ("python3", &PYTHON),
    ("pyi", &PYTHON),
    ("rs", &RUST),
    ("rust", &RUST),
    ("sh", &SH),
    ("bash", &SH),
    ("zsh", &SH),
    ("shell", &SH),
    ("console", &SH),
    ("fish", &SH),
    ("json", &JSON),
    ("jsonc", &JSON),
    ("json5", &JSON),
    ("jsonl", &JSON),
    ("yaml", &YAML),
    ("yml", &YAML),
    ("toml", &TOML),
    ("ini", &TOML),
    ("env", &TOML),
    ("css", &CSS),
    ("scss", &CSS),
    ("less", &CSS),
    ("sql", &SQL),
    ("psql", &SQL),
    ("rb", &RUBY),
    ("ruby", &RUBY),
    ("c", &C),
    ("h", &C),
    ("cpp", &C),
    ("cc", &C),
    ("hpp", &C),
    ("c++", &C),
    ("cs", &C),
    ("csharp", &C),
    ("java", &C),
    ("kt", &C),
    ("kts", &C),
    ("kotlin", &C),
    ("swift", &C),
    ("dart", &C),
    ("scala", &C),
    ("zig", &C),
    ("lua", &LUA),
    ("php", &PHP),
    ("md", &MARKDOWN),
    ("markdown", &MARKDOWN),
    ("mdx", &MARKDOWN),
];

fn by_name(name: &str) -> Option<&'static Lang> {
    BY_NAME.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

/// Every name `Lang::for_name` knows outright, with its language.
pub fn names() -> impl Iterator<Item = (&'static str, &'static Lang)> {
    BY_NAME.iter().copied()
}

/// What carries from one line to the next: an open string, block comment
/// or markdown fence (hl.go:150). The default is the start of a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    block: bool,
    str: &'static str,            // what closes the string still open
    fence: Option<&'static Lang>, // a Markdown fence's language, while in it
    in_block: bool,               // what the fenced code left open
    in_str: &'static str,
}

impl State {
    /// Inside a markdown fence, its language: `None` outside one, or in
    /// one with no language.
    pub fn fence(&self) -> Option<&'static Lang> {
        self.fence
    }
}

/// The closers a string can have, so the state holds no allocation.
fn closer(q: &[u8]) -> &'static str {
    match q {
        b"\"\"\"" => "\"\"\"",
        b"'''" => "'''",
        b"```" => "```",
        b"~~~" => "~~~",
        b"`" => "`",
        b"\"" => "\"",
        b"'" => "'",
        _ => "\0", // unreachable: every multi-line quote is above
    }
}

impl Lang {
    /// The language of a fence tag, a file name, an extension or an
    /// interpreter; `None` when there's none to highlight (hl.go:133).
    pub fn for_name(name: &str) -> Option<&'static Lang> {
        let name = name.trim();
        // Lower-cased on the stack when short and ASCII, as names are.
        let mut buf = [0u8; 64];
        let owned;
        let name: &str = if name.len() <= buf.len() && name.is_ascii() {
            let b = &mut buf[..name.len()];
            b.copy_from_slice(name.as_bytes());
            b.make_ascii_lowercase();
            std::str::from_utf8(b).unwrap_or("")
        } else {
            owned = name.to_lowercase();
            &owned
        };
        if let Some(l) = by_name(name) {
            return Some(l);
        }
        match base(name) {
            "makefile" | "dockerfile" | ".bashrc" | ".zshrc" | ".profile" => return Some(&SH),
            "go.mod" | "go.sum" => return Some(&GO),
            _ => {}
        }
        match ext(name) {
            "" => None,
            e => by_name(&e[1..]),
        }
    }

    /// Lexes one line of source, going on from `st` and leaving `st` for the
    /// next line. Calls `out(start, end, class)` for each run of `s`, as byte
    /// offsets on char boundaries, in order and without gaps, so the runs
    /// cover all of `s`. Adjacent runs of one class are merged (hl.go:174).
    pub fn line(&self, st: &mut State, s: &str, out: impl FnMut(usize, usize, Class)) {
        line(Some(self), st, s, out)
    }
}

/// `Lang::line` for a language that may be missing: `None` is all plain.
pub fn line(l: Option<&Lang>, st: &mut State, s: &str, mut out: impl FnMut(usize, usize, Class)) {
    // Merge adjacent runs of a class, holding the last back until the next
    // differs.
    let (mut pi, mut pj, mut pc) = (0usize, 0usize, Class::Plain);
    let mut emit = |i: usize, j: usize, c: Class| {
        if i >= j {
            return;
        }
        if pj > pi && c == pc && i == pj {
            pj = j;
            return;
        }
        if pj > pi {
            out(pi, pj, pc);
        }
        (pi, pj, pc) = (i, j, c);
    };
    match l {
        None => emit(0, s.len(), Class::Plain),
        Some(l) => lex(l, st, s.as_bytes(), &mut emit),
    }
    if pj > pi {
        out(pi, pj, pc);
    }
}

/// Go's filepath.Base, for a name already trimmed.
fn base(p: &str) -> &str {
    if p.is_empty() {
        return ".";
    }
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        return "/";
    }
    match p.rfind('/') {
        Some(k) => &p[k + 1..],
        None => p,
    }
}

/// Go's filepath.Ext: from the last dot of the last element, or "".
fn ext(p: &str) -> &str {
    for (i, c) in p.bytes().enumerate().rev() {
        match c {
            b'/' => return "",
            b'.' => return &p[i..],
            _ => {}
        }
    }
    ""
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() {
        return Some(0);
    }
    if n.len() > h.len() {
        return None;
    }
    if n.len() == 1 {
        return h.iter().position(|&c| c == n[0]);
    }
    h.windows(n.len()).position(|w| w == n)
}

fn find_byte(h: &[u8], c: u8) -> Option<usize> {
    h.iter().position(|&b| b == c)
}

fn is_word(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

/// The first byte after s[j..]'s spaces, or 0.
fn next(s: &[u8], mut j: usize) -> u8 {
    while j < s.len() && s[j] == b' ' {
        j += 1;
    }
    s.get(j).copied().unwrap_or(0)
}

/// Whether what ends at j is a key: a : or = follows.
fn key_after(s: &[u8], j: usize) -> bool {
    matches!(next(s, j), b':' | b'=')
}

/// Whether s[..i] is only indentation or a list's dash.
fn line_start_or_indent(s: &[u8], i: usize) -> bool {
    s[..i].iter().all(|&c| matches!(c, b' ' | b'\t' | b'-'))
}

/// Lexes a line of code in l through out, going on from st (hl.go:203).
fn lex<F: FnMut(usize, usize, Class)>(l: &Lang, st: &mut State, s: &[u8], out: &mut F) {
    if l.md {
        markdown(st, s, out);
        return;
    }
    let n = s.len();
    let mut i = 0;
    // What the last line left open.
    if st.block {
        let shut = l.shut.as_bytes();
        match find(s, shut) {
            Some(k) => {
                out(0, k + shut.len(), Class::Comment);
                i = k + shut.len();
                st.block = false;
            }
            None => {
                out(0, n, Class::Comment);
                return;
            }
        }
    }
    if !st.str.is_empty() {
        let q = st.str.as_bytes();
        match find(s, q) {
            Some(k) => {
                out(0, k + q.len(), Class::String);
                i = k + q.len();
                st.str = "";
            }
            None => {
                out(0, n, Class::String);
                return;
            }
        }
    }
    let words = &WORDS[l.id];
    let (lc, lc2) = (l.line.as_bytes(), l.line2.as_bytes());
    let (open, shut) = (l.open.as_bytes(), l.shut.as_bytes());
    while i < n {
        let c = s[i];
        let rest = &s[i..];
        if !lc.is_empty()
            && rest.starts_with(lc)
            && (lc != b"#" || i == 0 || s[i - 1] == b' ' || s[i - 1] == b'\t')
            || !lc2.is_empty() && rest.starts_with(lc2)
        {
            out(i, n, Class::Comment);
            i = n;
        } else if !open.is_empty() && rest.starts_with(open) {
            match find(&s[i + open.len()..], shut) {
                Some(k) => {
                    let end = i + open.len() + k + shut.len();
                    out(i, end, Class::Comment);
                    i = end;
                }
                None => {
                    out(i, n, Class::Comment);
                    st.block = true;
                    i = n;
                }
            }
        } else if l.triple && (rest.starts_with(b"\"\"\"") || rest.starts_with(b"'''")) {
            let q = &s[i..i + 3];
            match find(&s[i + 3..], q) {
                Some(k) => {
                    out(i, i + 3 + k + 3, Class::String);
                    i += 3 + k + 3;
                }
                None => {
                    out(i, n, Class::String);
                    st.str = closer(q);
                    i = n;
                }
            }
        } else if l.multi != 0 && c == l.multi {
            match find_byte(&s[i + 1..], c) {
                Some(k) => {
                    out(i, i + k + 2, Class::String);
                    i += k + 2;
                }
                None => {
                    out(i, n, Class::String);
                    st.str = closer(&s[i..i + 1]);
                    i = n;
                }
            }
        } else if l.quotes.contains(&c) && !(c == b'\'' && i > 0 && is_word(s[i - 1]) && !l.sh) {
            let mut j = i + 1;
            while j < n && s[j] != c {
                if s[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            j = (j + 1).min(n);
            let col = if l.keys && key_after(s, j) {
                Class::Function
            } else {
                Class::String
            };
            out(i, j, col);
            i = j;
        } else if l.vars && c == b'$' && i + 1 < n && (is_word(s[i + 1]) || s[i + 1] == b'{') {
            let mut j = i + 1;
            if s[j] == b'{' {
                if let Some(k) = find_byte(&s[j..], b'}') {
                    j += k + 1;
                }
            } else {
                while j < n && is_word(s[j]) {
                    j += 1;
                }
            }
            out(i, j, Class::Function);
            i = j;
        } else if c.is_ascii_digit() && (i == 0 || !is_word(s[i - 1])) {
            let mut j = i + 1;
            while j < n && (is_word(s[j]) || s[j] == b'.' && j + 1 < n && s[j + 1].is_ascii_digit())
            {
                j += 1;
            }
            out(i, j, Class::Number);
            i = j;
        } else if is_word(c) || c >= 0x80 {
            let mut j = i + 1;
            while j < n && (is_word(s[j]) || s[j] >= 0x80 || l.dash && s[j] == b'-') {
                j += 1;
            }
            // Words are whole chars: they end at an ASCII byte or the end.
            let w = &s[i..j];
            let found = words.get(w);
            let col = match found {
                Some(KW) => Class::Keyword,
                Some(TY) => Class::Type,
                Some(_) => Class::Number,
                None => {
                    if l.keys && key_after(s, j) && line_start_or_indent(s, i) || next(s, j) == b'('
                    {
                        Class::Function
                    } else if l.caps && c.is_ascii_uppercase() && w.len() > 1 {
                        Class::Type
                    } else {
                        Class::Plain
                    }
                }
            };
            out(i, j, col);
            i = j;
        } else {
            let mut j = i + 1;
            while j < n
                && !is_word(s[j])
                && s[j] < 0x80
                && !matches!(s[j], b'"' | b'\'' | b'`' | b'/' | b'#' | b'$' | b'-')
            {
                j += 1;
            }
            out(i, j, Class::Plain);
            i = j;
        }
    }
}

/// Lexes a line of Markdown: a heading, a quote, a rule and a fenced block
/// whole; a list's mark and a table's pipes quiet; inline code, bold and
/// links in the line. st carries a fence from line to line (hl.go:334).
fn markdown<F: FnMut(usize, usize, Class)>(st: &mut State, s: &[u8], out: &mut F) {
    let n = s.len();
    let ind = s.iter().position(|&c| c != b' ' && c != b'\t').unwrap_or(n);
    let t = &s[ind..];
    if t.starts_with(b"```") || t.starts_with(b"~~~") {
        let fence = closer(&t[..3]);
        if st.str.is_empty() {
            st.str = fence;
            st.fence = None;
            st.in_block = false;
            st.in_str = "";
            let tag = trim_bytes(t, b"`~");
            if let Some(w) = tag
                .split(|c| c.is_ascii_whitespace())
                .find(|w| !w.is_empty())
            {
                st.fence = std::str::from_utf8(w).ok().and_then(Lang::for_name);
            }
        } else if st.str == fence {
            st.str = "";
            st.fence = None;
        }
        out(0, n, Class::Comment);
        return;
    }
    if !st.str.is_empty() {
        if let Some(f) = st.fence {
            // A fenced block in its own language, from where its last line
            // left it.
            let mut inner = State {
                block: st.in_block,
                str: st.in_str,
                ..State::default()
            };
            lex(f, &mut inner, s, out);
            st.in_block = inner.block;
            st.in_str = inner.str;
        } else {
            out(0, n, Class::String);
        }
        return;
    }
    let h = t.iter().take_while(|&&c| c == b'#').count();
    if t.is_empty() {
        out(0, n, Class::Plain);
        return;
    }
    if h > 0 && h <= 6 && (h == t.len() || t[h] == b' ') {
        out(0, n, Class::Keyword);
        return;
    }
    if t[0] == b'>' {
        out(0, n, Class::Comment);
        return;
    }
    let t0 = t[0];
    if t.len() >= 3 && b"-*_=".contains(&t0) && t.iter().all(|&c| c == t0 || c == b' ')
        || t0 == b'|' && t.iter().all(|c| b"|-: ".contains(c))
    {
        out(0, n, Class::Comment); // a rule, a heading's underline, a table's
        return;
    }
    let mut i = 0;
    let m = list_mark(t);
    if m > 0 {
        out(0, ind + m, Class::Number);
        i = ind + m;
    }
    let table = t0 == b'|';
    while i < n {
        let c = s[i];
        if c == b'`' {
            let mut k = 1;
            while i + k < n && s[i + k] == b'`' {
                k += 1;
            }
            let mut j = i + k;
            if let Some(e) = find(&s[j..], &s[i..j]) {
                j += e + k;
            }
            out(i, j, Class::String);
            i = j;
        } else if c == b'*' && i + 1 < n && s[i + 1] == b'*' && find(&s[i + 2..], b"**").is_some() {
            let j = i + 2 + find(&s[i + 2..], b"**").unwrap_or(0) + 2;
            out(i, j, Class::Type);
            i = j;
        } else if c == b'[' && md_link(&s[i..]) > 0 {
            let k = find_byte(&s[i..], b']').unwrap_or(0);
            let j = i + md_link(&s[i..]);
            out(i, i + 1, Class::Comment);
            out(i + 1, i + k, Class::Function);
            out(i + k, j, Class::Comment);
            i = j;
        } else if c == b'|' && table {
            out(i, i + 1, Class::Comment);
            i += 1;
        } else {
            let mut j = i + 1;
            while j < n && !b"`*[|".contains(&s[j]) {
                j += 1;
            }
            out(i, j, Class::Plain);
            i = j;
        }
    }
}

/// Go's strings.Trim with a set of ASCII bytes.
fn trim_bytes<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let a = s.iter().position(|c| !set.contains(c)).unwrap_or(s.len());
    let b = s
        .iter()
        .rposition(|c| !set.contains(c))
        .map_or(a, |k| k + 1);
    &s[a..b.max(a)]
}

/// How long a list item's mark is with its space: "- ", "* ", "1. ", "2) ";
/// 0 when t isn't one.
fn list_mark(t: &[u8]) -> usize {
    if t.len() >= 2 && b"-*+".contains(&t[0]) && t[1] == b' ' {
        return 2;
    }
    let n = t.iter().take(9).take_while(|c| c.is_ascii_digit()).count();
    if n > 0 && n + 1 < t.len() && (t[n] == b'.' || t[n] == b')') && t[n + 1] == b' ' {
        return n + 2;
    }
    0
}

/// How long the [text](url) that s starts with is, 0 when it doesn't start
/// one.
fn md_link(s: &[u8]) -> usize {
    let k = match find_byte(s, b']') {
        Some(k) if k >= 1 => k,
        _ => return 0,
    };
    if !s[k..].starts_with(b"](") {
        return 0;
    }
    match find_byte(&s[k..], b')') {
        Some(e) => k + e + 1,
        None => 0,
    }
}
