//! Shell commands as rush reads them: their words, their lines as a person
//! would write them, what one amounts to (a read, a search, a write) and a
//! chain's commands in words. Port of internal/convo/shell.go (shellShape,
//! chainSpans, joinLines, dropHeredocs, grepArgs, quietTokens, scriptGist,
//! compound, scriptLines), bash.go (segments, chain, phrases, phraseOf,
//! gitPhrase, subPhrase, cargoTakesValue, cargoFlag, cargoAliases,
//! findObjs, chainLabel, echoMarks, markHeading),
//! cmdrows.go (shortPaths, tokenRows, cutAt), render.go:2662-2900 (cdRe,
//! program, shellLines, shellTokens, subshellLines, closesEarly),
//! highlight.go:187 (heredocLang) and git.go:10 (gitOut).

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use rush_hl::Lang;

use super::code::lang_for;
use super::style::one_line;
use super::style::{C_BLUE, C_DIM, C_GREEN, C_ORANGE, C_SUB, C_WHITE, faint, paint, sub};
use super::txt::{Txt, width};

/// Where a session's paths are read from: its folders, as given and as
/// they really are, and your home.
#[derive(Clone, Debug, Default)]
pub struct Places {
    pub cwd: String,
    pub bases: Vec<String>,
    pub home: String,
}

impl Places {
    pub fn new(cwd: &str) -> Places {
        let mut bases = Vec::new();
        if !cwd.is_empty() {
            bases.push(cwd.to_string());
            if let Ok(r) = std::fs::canonicalize(cwd)
                && let Some(r) = r.to_str()
                && r != cwd
            {
                bases.push(r.to_string());
            }
        }
        Places::with_bases(cwd, bases)
    }

    /// Paths read from cwd, said from the first of bases they're in
    /// (Session.bases, model.go:970, worked out by the caller).
    pub fn with_bases(cwd: &str, bases: Vec<String>) -> Places {
        let home = std::env::var("HOME")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        Places {
            cwd: cwd.to_string(),
            bases,
            home,
        }
    }

    /// p relative to the session's folder when it's inside it, else as it
    /// reads best outside (render.go:2522).
    pub fn rel(&self, p: &str) -> String {
        if p.starts_with('/') {
            let c = clean(p);
            for b in &self.bases {
                if c == *b {
                    return ".".into();
                }
                if let Some(r) = c.strip_prefix(b.as_str())
                    && let Some(r) = r.strip_prefix('/')
                {
                    return r.to_string();
                }
            }
        }
        self.short_abs(p)
    }

    /// A path outside the session: home as ~, and the middle of a long one
    /// left out (render.go:2533).
    pub fn short_abs(&self, p: &str) -> String {
        let mut p = p.to_string();
        if !self.home.is_empty()
            && let Some(rest) = p.strip_prefix(&format!("{}/", self.home))
        {
            p = format!("~/{rest}");
        }
        let parts: Vec<&str> = p.split('/').collect();
        if p.len() <= 48 || parts.len() <= 4 {
            return p;
        }
        let n = parts.len();
        [parts[0], parts[1], "…", parts[n - 2], parts[n - 1]].join("/")
    }

    /// A relative path from the session's folder; "" when there's none.
    pub fn abs(&self, p: &str) -> String {
        if p.is_empty() || p.starts_with('/') || p.starts_with("~/") {
            return p.to_string();
        }
        if self.cwd.is_empty() {
            return String::new();
        }
        clean(&format!("{}/{}", self.cwd, p))
    }
}

/// A path with its . and .. and doubled slashes taken out (filepath.Clean).
pub(crate) fn clean(p: &str) -> String {
    let abs = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !abs {
                    out.push("..");
                }
            }
            x => out.push(x),
        }
    }
    let j = out.join("/");
    match (abs, j.is_empty()) {
        (true, _) => format!("/{j}"),
        (false, true) => ".".into(),
        _ => j,
    }
}

fn re(s: &str) -> Regex {
    Regex::new(s).expect("a valid pattern")
}

/// cd somewhere && at a command's start: its length and the folder.
pub(crate) fn cd_lead(s: &str) -> Option<(usize, &str)> {
    static CD: LazyLock<Regex> = LazyLock::new(|| re(r#"^cd\s+("[^"]+"|'[^']+'|\S+)\s*&&\s*"#));
    let c = CD.captures(s)?;
    Some((c.get(0)?.end(), c.get(1)?.as_str()))
}

/// s without a leading cd somewhere &&.
pub(crate) fn drop_cd(s: &str) -> &str {
    match cd_lead(s) {
        Some((n, _)) => &s[n..],
        None => s,
    }
}

pub(crate) fn unquote(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

pub(crate) fn non_flags<'a>(xs: &[&'a str]) -> Vec<&'a str> {
    xs.iter().copied().filter(|x| !x.starts_with('-')).collect()
}

/// A command's words, keeping quoted strings and spaces whole
/// (render.go:2846).
pub(crate) fn shell_tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut start: Option<usize> = None;
    let mut i = 0;
    macro_rules! flush {
        () => {
            if let Some(st) = start.take() {
                out.push(&s[st..i]);
            }
        };
    }
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' | b'\'' => {
                flush!();
                match s[i + 1..].find(c as char) {
                    None => {
                        out.push(&s[i..]);
                        return out;
                    }
                    Some(j) => {
                        out.push(&s[i..i + j + 2]);
                        i += j + 2;
                        continue;
                    }
                }
            }
            b' ' | b'\t' => {
                flush!();
                out.push(&s[i..i + 1]);
            }
            b';' | b'|' | b'&' => {
                flush!();
                if i + 1 < b.len() && b[i + 1] == c {
                    out.push(&s[i..i + 2]);
                    i += 1;
                } else {
                    out.push(&s[i..i + 1]);
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(i);
                }
            }
        }
        i += 1;
    }
    flush!();
    out
}

/// The shell's operators.
pub(crate) fn sh_op(t: &str) -> bool {
    matches!(t, "&&" | "||" | "|" | ";" | ">" | ">>" | "<" | "2>&1" | "&")
}

/// One line of a command as shell_lines lays it out: depth 1 is a pipe's
/// later stage; verbatim is a heredoc's body.
#[derive(Clone, Debug, Default)]
pub(crate) struct ShLine {
    pub text: String,
    pub depth: usize,
    pub verbatim: bool,
    pub from: usize,
    pub to: usize,
}

fn skip_spaces(s: &[u8], mut i: usize) -> usize {
    while i + 1 < s.len() && s[i + 1] == b' ' {
        i += 1;
    }
    i
}

/// A command laid out as a person writes it: each command on its own
/// line, && and || leading the line they join, a pipe's stages under it
/// (render.go:2717).
#[allow(unused_assignments)]
pub(crate) fn shell_lines(cmd: &str) -> Vec<ShLine> {
    let b = cmd.as_bytes();
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut depth = 0usize;
    let (mut from, mut to): (isize, usize) = (-1, 0);
    macro_rules! emit {
        ($next:expr) => {{
            let s = String::from_utf8_lossy(&cur).trim().to_string();
            if !s.is_empty() {
                let f = from.max(0) as usize;
                out.push(ShLine {
                    text: s,
                    depth,
                    verbatim: false,
                    from: f,
                    to: to.max(f),
                });
            }
            cur.clear();
            depth = $next;
            from = -1;
        }};
    }
    macro_rules! put {
        ($i:expr) => {{
            let i = $i;
            cur.push(b[i]);
            if !matches!(b[i], b' ' | b'\t' | b'\n') {
                if from < 0 {
                    from = i as isize;
                }
                to = i + 1;
            }
        }};
    }
    let (mut quote, mut parens) = (0u8, 0usize);
    let mut heredoc = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            put!(i);
            if c == b'\\' && quote == b'"' && i + 1 < b.len() {
                i += 1;
                put!(i);
            } else if c == quote {
                quote = 0;
            }
        } else if c == b'\\' && i + 1 < b.len() {
            put!(i);
            i += 1;
            put!(i);
        } else if c == b'\'' || c == b'"' {
            quote = c;
            put!(i);
        } else if c == b'(' {
            parens += 1;
            put!(i);
        } else if c == b')' {
            parens = parens.saturating_sub(1);
            put!(i);
        } else if parens > 0 {
            put!(i);
        } else if c == b'<' && cmd[i..].starts_with("<<") && !cmd[i..].starts_with("<<<") {
            let rest = cmd[i + 2..].strip_prefix('-').unwrap_or(&cmd[i + 2..]);
            let rest = rest.trim_start_matches(' ');
            let end = rest
                .find([' ', '\n', ';', '|', '&', '<', '>'])
                .unwrap_or(rest.len());
            heredoc = rest[..end].trim_matches(['\'', '"']).to_string();
            put!(i);
            put!(i + 1);
            i += 1;
        } else if c == b'\n' {
            emit!(0);
            if !heredoc.is_empty() {
                let body = &cmd[i + 1..];
                for (n, l) in body.split('\n').enumerate() {
                    out.push(ShLine {
                        text: l.to_string(),
                        depth: 0,
                        verbatim: true,
                        from: i + 1,
                        to: i + 1 + l.len(),
                    });
                    i += l.len() + 1;
                    if l.trim() == heredoc || n > 10000 {
                        break;
                    }
                }
                i = i.min(b.len());
                heredoc.clear();
            }
        } else if c == b';' {
            emit!(0);
        } else if (c == b'&' && cmd[i..].starts_with("&&"))
            || (c == b'|' && cmd[i..].starts_with("||"))
        {
            emit!(0);
            cur.extend_from_slice(&b[i..i + 2]);
            cur.push(b' ');
            i = skip_spaces(b, i + 1);
        } else if c == b'|' && !(i > 0 && b[i - 1] == b'>') {
            emit!(1);
            cur.extend_from_slice(b"| ");
            i = skip_spaces(b, i);
        } else {
            put!(i);
        }
        i += 1;
    }
    emit!(0);
    out
}

/// A command that's a subshell of several, "(a; b)", a command a line
/// inside its brackets (render.go:2995).
pub(crate) fn subshell_lines(l: ShLine) -> Vec<ShLine> {
    let mut op = "";
    let mut body = l.text.as_str();
    let head = body.get(..3).unwrap_or("");
    if head == "&& " || head == "|| " {
        op = if head == "&& " { "&& " } else { "|| " };
        body = &body[3..];
    }
    if l.verbatim || l.depth > 0 || !body.starts_with('(') || !body.ends_with(')') || body.len() < 2
    {
        return vec![l];
    }
    let inner = shell_lines(&body[1..body.len() - 1]);
    if inner.len() < 2 || closes_early(body) {
        return vec![l];
    }
    let n = inner.len();
    let mut out = Vec::with_capacity(n);
    for (i, x) in inner.into_iter().enumerate() {
        let mut t = x.text;
        if i == 0 {
            t = format!("{op}( {t}");
        }
        if i == n - 1 {
            t.push(')');
        }
        out.push(ShLine {
            text: t,
            depth: if i == 0 { 0 } else { 1 + x.depth },
            verbatim: x.verbatim,
            from: l.from,
            to: l.to,
        });
    }
    out
}

fn closes_early(s: &str) -> bool {
    let b = s.as_bytes();
    let (mut depth, mut quote) = (0i32, 0u8);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' && quote == b'"' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else if c == b'\'' || c == b'"' {
            quote = c;
        } else if c == b'(' {
            depth += 1;
        } else if c == b')' {
            depth -= 1;
            if depth == 0 && i < b.len() - 1 {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// A command's first word, or first two for git, go, npm and the like
/// (render.go:2683).
pub(crate) fn program(cmd: &str) -> String {
    let cmd = drop_cd(cmd.trim()).trim();
    let first = cmd.split('\n').next().unwrap_or("");
    let mut f: Vec<&str> = first.split_whitespace().collect();
    while f.len() > 1 && f[0].contains('=') && !f[0].starts_with('-') {
        f.remove(0);
    }
    if f.is_empty() {
        return String::new();
    }
    let mut p = f[0].to_string();
    if matches!(
        f[0],
        "git"
            | "go"
            | "npm"
            | "pnpm"
            | "yarn"
            | "cargo"
            | "rustup"
            | "docker"
            | "kubectl"
            | "gh"
            | "make"
            | "uv"
            | "bun"
    ) {
        if f.len() > 1 && !f[1].starts_with('-') {
            p.push(' ');
            p.push_str(f[1]);
        }
        // cargo +nightly build: the toolchain, then what it does.
        if f.len() > 2 && f[1].starts_with('+') && !f[2].starts_with('-') {
            p.push(' ');
            p.push_str(f[2]);
        }
    }
    if cmd.contains(['|', ';', '&']) {
        p.push_str(" …");
    }
    p
}

/// What a shell command amounts to when it's really a read, a search, a
/// write, a script or a commit (shell.go:12).
#[derive(Clone, Debug, Default)]
pub(crate) struct Shape {
    pub kind: &'static str,
    pub glyph: &'static str,
    pub what: String,
    pub res: String,
    pub ctx: bool,
    pub n: usize,
    pub body: String,
}

/// Lines between commands as the ; they amount to, continued lines
/// joined; a heredoc or a quoted string keeps its lines (shell.go:262).
pub(crate) fn join_lines(cmd: &str) -> String {
    if !cmd.contains('\n') || cmd.contains("<<") {
        return cmd.to_string();
    }
    let b = cmd.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len() + 8);
    let mut q = 0u8;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if q != 0 {
            if c == q {
                q = 0;
            }
        } else if c == b'"' || c == b'\'' {
            q = c;
        } else if c == b'\\' && i + 1 < b.len() {
            i += 1;
            if b[i] == b'\n' {
                out.push(b' ');
            } else {
                out.push(c);
                out.push(b[i]);
            }
            i += 1;
            continue;
        } else if c == b'\n' {
            out.extend_from_slice(b" ; ");
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn int_of(s: &str) -> Option<usize> {
    let d: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if d.is_empty() { None } else { d.parse().ok() }
}

/// What cmd does: chains have no shape (shell.go:28).
pub(crate) fn shell_shape(p: &Places, cmd: &str) -> Shape {
    let cmd = join_lines(cmd.trim());
    let mut cmd = cmd.as_str();
    let sh = Shape::default();
    if let Some((n, _)) = cd_lead(cmd) {
        cmd = &cmd[n..];
    }
    let lines: Vec<&str> = cmd.split('\n').collect();
    let body = if lines.len() > 1 { lines.len() - 2 } else { 0 };
    let toks: Vec<&str> = shell_tokens(lines[0])
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .collect();
    let mut args: Vec<&str> = Vec::new();
    let (mut chained, mut piped) = (false, false);
    for t in &toks {
        match *t {
            "&&" | "||" | ";" | "&" => chained = true,
            "|" => piped = true,
            _ => {}
        }
        if chained || piped {
            break;
        }
        args.push(t);
    }
    if args.is_empty() {
        return sh;
    }
    let (mut heredoc, mut end) = (false, String::new());
    let mut into = String::new();
    let mut plain: Vec<&str> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = args[i];
        if a.starts_with("<<") {
            heredoc = true;
            let mut e = a.trim_start_matches(['<', '-']);
            if e.is_empty() && i + 1 < args.len() {
                i += 1;
                e = args[i];
            }
            end = unquote(e).to_string();
        } else if a == ">" || a == ">>" {
            if i + 1 < args.len() {
                into = unquote(args[i + 1]).to_string();
                i += 1;
            }
        } else if a.starts_with('>') {
            into = unquote(a.trim_start_matches('>')).to_string();
        } else if a == "2>&1" || a.starts_with("2>") {
        } else {
            plain.push(a);
        }
        i += 1;
    }
    let prog = base_name(args[0]);
    if chained || heredoc && lines[lines.len() - 1].trim() != end {
        return sh;
    }
    let files = |xs: &[&str]| -> String {
        xs.iter()
            .map(|x| p.rel(unquote(x)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut sh = sh;
    match prog {
        "cat" | "tee" => {
            let nf = non_flags(&plain);
            let mut target = into.clone();
            if prog == "tee" && !nf.is_empty() {
                target = unquote(nf[0]).to_string();
            }
            if heredoc && !target.is_empty() {
                sh.kind = "write";
                sh.glyph = "✎";
                sh.what = p.rel(&target);
                sh.res = super::style::plural(body, "line");
                sh.body = lines[1..lines.len() - 1].join("\n");
            } else if prog == "cat" && into.is_empty() && !nf.is_empty() {
                sh.kind = "read";
                sh.glyph = "◧";
                sh.what = files(&nf);
            }
        }
        "nl" | "bat" | "batcat" => {
            let f = non_flags(&plain);
            if !f.is_empty() && into.is_empty() {
                sh.kind = "read";
                sh.glyph = "◧";
                sh.what = p.rel(unquote(f[f.len() - 1]));
            }
        }
        "sed" => {
            if plain.len() >= 3 && plain[0] == "-n" && into.is_empty() {
                sh.kind = "read";
                sh.glyph = "◧";
                sh.what = files(&plain[2..]);
                static SED: LazyLock<Regex> = LazyLock::new(|| re(r"^(\d+)(?:,(\d+))?p$"));
                if let Some(m) = SED.captures(unquote(plain[1])) {
                    let a = &m[1];
                    match m.get(2) {
                        None => {
                            sh.res = format!("line {a}");
                            sh.n = 1;
                        }
                        Some(b) => {
                            sh.res = format!("lines {a}–{}", b.as_str());
                            let (x, y) = (
                                a.parse::<usize>().unwrap_or(0),
                                b.as_str().parse::<usize>().unwrap_or(0),
                            );
                            sh.n = (y + 1).saturating_sub(x);
                        }
                    }
                }
            }
        }
        "head" | "tail" => {
            let f = non_flags(&plain);
            if !f.is_empty() && into.is_empty() && !f[f.len() - 1].starts_with('-') {
                let last = f[f.len() - 1];
                if int_of(last).is_none() {
                    sh.kind = "read";
                    sh.glyph = "◧";
                    sh.what = p.rel(unquote(last));
                    for (i, a) in plain.iter().enumerate() {
                        if *a == "-n" && i + 1 < plain.len() {
                            sh.n = int_of(plain[i + 1]).unwrap_or(0);
                        } else if a.len() > 1
                            && a.as_bytes()[0] == b'-'
                            && a.as_bytes()[1].is_ascii_digit()
                        {
                            sh.n = int_of(&a[1..]).unwrap_or(0);
                        }
                    }
                }
            }
        }
        "grep" | "egrep" | "rg" => {
            let (pat, paths) = grep_args_owned(&plain);
            if !pat.is_empty() && into.is_empty() {
                sh.kind = "search";
                sh.glyph = "⌕";
                sh.what = pat;
                for a in &plain {
                    let b = a.as_bytes();
                    if b.len() >= 2 && b[0] == b'-' && matches!(b[1], b'A' | b'B' | b'C') {
                        sh.ctx = true;
                    }
                }
                if !paths.is_empty() {
                    sh.what = format!("{} in {}", sh.what, files(&paths));
                }
            }
        }
        "python" | "python3" | "node" | "bun" | "deno" | "ruby" | "perl" | "bash" | "sh"
        | "zsh" => {
            let script = heredoc || plain.iter().any(|a| matches!(*a, "-" | "-c" | "-e"));
            if script {
                sh.kind = "script";
                sh.glyph = "$";
                let hi = (lines.len().saturating_sub(1)).max(1);
                let body_lines: Vec<&str> = lines[1.min(hi)..hi].to_vec();
                sh.what = format!("{prog} {}", script_gist(&body_lines, heredoc));
                if heredoc {
                    sh.res = super::style::plural(body, "line");
                }
            }
        }
        "git" if !plain.is_empty() && plain[0] == "commit" => {
            sh.kind = "commit";
            sh.glyph = "$";
            sh.what = "git commit".into();
            for (i, a) in plain.iter().enumerate() {
                if (*a == "-m" || *a == "-am") && i + 1 < plain.len() {
                    sh.res = format!("“{}”", one_line(unquote(plain[i + 1])));
                    break;
                }
            }
        }
        _ => {}
    }
    sh
}

/// The last element of a path.
pub(crate) fn base_name(p: &str) -> &str {
    let p = p.trim_end_matches('/');
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// grep_args with the paths it names.
pub(crate) fn grep_args_owned<'a>(args: &[&'a str]) -> (String, Vec<&'a str>) {
    let mut pat = String::new();
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "-e" && i + 1 < args.len() {
            pat = unquote(args[i + 1]).to_string();
            i += 1;
        } else if matches!(
            a,
            "-e" | "-A" | "-B" | "-C" | "-m" | "-g" | "-t" | "-f" | "--glob" | "--type"
        ) {
            i += 1;
        } else if a.starts_with('-') {
        } else if pat.is_empty() {
            pat = unquote(a).to_string();
        } else {
            paths.push(a);
        }
        i += 1;
    }
    (pat.replace("\\|", "|"), paths)
}

/// A heredoc script's gist: the files it edits or reads, else its first
/// line of code (shell.go:494).
pub(crate) fn script_gist(body: &[&str], heredoc: bool) -> String {
    if !heredoc {
        return "script".into();
    }
    static PATH: LazyLock<Regex> = LazyLock::new(|| re(r#"["']([\w./~-]+\.[A-Za-z]{1,5})["']"#));
    let src = body.join("\n");
    let mut paths: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for m in PATH.captures_iter(&src) {
        let p = m[1].to_string();
        if !seen.contains(&p) && (!p.starts_with('.') || p.matches('.').count() > 1) {
            seen.insert(p.clone());
            paths.push(p);
        }
    }
    if !paths.is_empty() {
        let verb = if src.contains(".write(")
            || src.contains("write_text(")
            || src.contains("writeFileSync(")
        {
            "edits "
        } else {
            "reads "
        };
        if paths.len() > 2 {
            let more = format!("+{}", paths.len() - 2);
            paths.truncate(2);
            paths.push(more);
        }
        return format!("· {verb}{}", paths.join(", "));
    }
    for l in body {
        let l = l.trim();
        if l.is_empty()
            || ["#", "//", "import ", "from ", "use ", "require"]
                .iter()
                .any(|p| l.starts_with(p))
        {
            continue;
        }
        return format!("· {}", one_line(l));
    }
    "script".into()
}

/// Whether a command's lines hold a loop, an if, a case, a group or a
/// function: a script (shell.go:533).
pub(crate) fn compound(lines: &[ShLine]) -> bool {
    lines.iter().any(|l| {
        if l.verbatim || l.depth > 0 {
            return false;
        }
        let t = l
            .text
            .strip_prefix("&& ")
            .or(l.text.strip_prefix("|| "))
            .unwrap_or(&l.text);
        let w = t.split(' ').next().unwrap_or("");
        matches!(
            w,
            "while" | "until" | "for" | "if" | "case" | "select" | "function" | "{"
        ) || w.ends_with("()")
    })
}

/// A compound command's lines laid out as a script is written
/// (shell.go:550).
pub(crate) fn script_lines(lines: &[ShLine]) -> Vec<ShLine> {
    struct S {
        out: Vec<ShLine>,
        depth: i32,
    }
    fn add(s: &mut S, t: &str, dp: i32) {
        s.out.push(ShLine {
            text: t.to_string(),
            depth: dp.max(0) as usize,
            ..ShLine::default()
        });
    }
    fn join(s: &mut S, t: &str) {
        if let Some(l) = s.out.last_mut()
            && !l.verbatim
        {
            l.text.push_str(t);
            return;
        }
        let d = s.depth;
        add(s, t.trim_start_matches([';', ' ']), d);
    }
    fn put(s: &mut S, t: &str) {
        let t = t.trim();
        if t.is_empty() {
            return;
        }
        let (w, rest) = t.split_once(' ').unwrap_or((t, ""));
        match w {
            "do" | "then" => {
                join(s, &format!("; {w}"));
                s.depth += 1;
                put(s, rest);
            }
            "else" => {
                let d = s.depth - 1;
                add(s, w, d);
                put(s, rest);
            }
            "elif" => {
                s.depth -= 1;
                let d = s.depth;
                add(s, t, d);
            }
            "done" | "fi" | "esac" | "}" => {
                s.depth -= 1;
                let d = s.depth;
                add(s, t, d);
            }
            "{" => {
                let d = s.depth;
                add(s, w, d);
                s.depth += 1;
                put(s, rest);
            }
            "&&" | "||" => {
                if let Some(g) = rest.trim().strip_prefix('{')
                    && (g.is_empty() || g.starts_with(' '))
                {
                    join(s, &format!(" {w} {{"));
                    s.depth += 1;
                    put(s, g);
                    return;
                }
                let d = s.depth;
                add(s, t, d);
            }
            _ => {
                if (w == "function" || w.ends_with("()"))
                    && let Some((head, body)) = t.split_once('{')
                {
                    let d = s.depth;
                    add(s, &format!("{} {{", head.trim()), d);
                    s.depth += 1;
                    put(s, body);
                    return;
                }
                let d = s.depth;
                add(s, t, d);
                if w == "case" {
                    s.depth += 1;
                }
            }
        }
    }
    let mut s = S {
        out: Vec::new(),
        depth: 0,
    };
    for l in lines {
        if l.verbatim {
            s.out.push(l.clone());
        } else if l.depth > 0 && !s.out.is_empty() {
            join(&mut s, &format!(" {}", l.text));
        } else {
            put(&mut s, &l.text);
        }
    }
    s.out
}

/// Words with operators and redirects left out, quotes kept whole
/// (bash.go:171).
pub(crate) fn fields_of(s: &str) -> Vec<String> {
    static REDIR: LazyLock<Regex> = LazyLock::new(|| re(r"\s\d?>&\d\b|\s&>>?\s*\S+"));
    let s = if s.contains('>') {
        REDIR.replace_all(s, "")
    } else {
        std::borrow::Cow::Borrowed(s)
    };
    let mut out: Vec<String> = Vec::new();
    let mut skip = false;
    // A quote right after key= is the key's value: one word, as the shell
    // reads `-c user.name="A B"`. Go's fieldsOf splits it there.
    let mut glued = false;
    for t in shell_tokens(&s) {
        let joins = std::mem::take(&mut glued);
        if t.trim().is_empty() {
        } else if joins && t.starts_with(['"', '\'']) {
            out.last_mut().unwrap().push_str(t);
        } else if skip {
            skip = false;
        } else if matches!(t, ">" | ">>" | "<" | "2>") {
            skip = true;
        } else if t.starts_with('>')
            || t.starts_with("2>")
            || t.starts_with("&>")
            || t.starts_with("<<")
        {
        } else {
            glued = t.ends_with('=') && !t.starts_with(['"', '\'']);
            out.push(t.to_string());
        }
    }
    out
}

/// One simple command of a chain, its pipe stages and heredoc with it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Segment {
    pub text: String,
    pub filters: Vec<String>,
    pub body: Vec<String>,
}

impl Segment {
    fn full(&self) -> String {
        let mut c = self.text.clone();
        for f in &self.filters {
            c.push_str(" | ");
            c.push_str(f);
        }
        if !self.body.is_empty() {
            c.push('\n');
            c.push_str(&self.body.join("\n"));
        }
        c
    }
}

pub(crate) fn segments(cmd: &str) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for l in shell_lines(cmd) {
        if l.verbatim && !out.is_empty() {
            out.last_mut().unwrap().body.push(l.text);
        } else if l.depth > 0 && !out.is_empty() {
            let f = l
                .text
                .strip_prefix('|')
                .unwrap_or(&l.text)
                .trim()
                .to_string();
            out.last_mut().unwrap().filters.push(f);
        } else {
            let t = l.text.strip_prefix("&& ").unwrap_or(&l.text);
            let t = t.strip_prefix("|| ").unwrap_or(t);
            out.push(Segment {
                text: t.trim().to_string(),
                ..Segment::default()
            });
        }
    }
    out
}

/// One command of a chain in words.
#[derive(Clone, Debug, Default)]
pub(crate) struct Phrase {
    pub verb: String,
    pub objs: Vec<String>,
    pub kind: &'static str,
}

fn first(mut xs: Vec<String>, n: usize) -> Vec<String> {
    if xs.len() > n {
        xs.truncate(n);
        xs.push("…".into());
    }
    xs
}

pub(crate) fn is_assign(w: &str) -> bool {
    let b = w.as_bytes();
    if b.is_empty() || !(b[0].is_ascii_alphabetic() || b[0] == b'_') {
        return false;
    }
    for (i, &c) in b.iter().enumerate().skip(1) {
        if c == b'=' {
            return i > 0;
        }
        if !(c.is_ascii_alphanumeric() || c == b'_') {
            return false;
        }
    }
    false
}

/// Args as a row shows them: unquoted, relative to the session's folder.
fn paths(p: &Places, args: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for a in args {
        let a = unquote(a);
        if a.contains('%')
            || a.trim_matches(['|', '&', '(', ')', ';']).is_empty()
            || a == "."
            || a == "./"
        {
            continue;
        }
        out.push(p.rel(a));
    }
    out
}

/// A chain of commands as what each one does (bash.go:90).
pub(crate) fn phrases(p: &Places, cmd: &str) -> Vec<Phrase> {
    let segs = segments(&cmd.trim().replace("\\\n", " "));
    let (mut loop_var, mut loop_over) = (String::new(), String::new());
    let mut out: Vec<Phrase> = Vec::new();
    for mut s in segs {
        let mut words = fields_of(&s.text);
        if words.is_empty() {
            continue;
        }
        if words[0] == "for" && words.len() >= 4 && words[2] == "in" {
            loop_var = words[1].clone();
            let rest: Vec<&str> = words[3..].iter().map(String::as_str).collect();
            loop_over = first(paths(p, &rest), 3).join(", ");
            continue;
        }
        if words[0] == "done" {
            loop_var.clear();
        }
        if matches!(words[0].as_str(), "do" | "then" | "else") {
            words.remove(0);
            let t = s.text.as_str();
            let t = t.strip_prefix("do").unwrap_or(t);
            let t = t.strip_prefix("then").unwrap_or(t);
            let t = t.strip_prefix("else").unwrap_or(t);
            s.text = t.trim().to_string();
        }
        while !words.is_empty() && is_assign(&words[0]) {
            s.text = s
                .text
                .strip_prefix(words[0].as_str())
                .unwrap_or(&s.text)
                .trim()
                .to_string();
            words.remove(0);
        }
        if words.is_empty() {
            continue;
        }
        let w0 = &words[0];
        if w0.starts_with(['"', '\'', '(', ')', '-', '|'])
            || w0.starts_with('$') && !w0.contains('/')
        {
            continue;
        }
        if words[0] == "cd" && words.len() > 1 {
            continue;
        }
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let Some(mut ph) = phrase_of(p, &s, &refs) else {
            continue;
        };
        if !loop_var.is_empty() {
            for o in &mut ph.objs {
                if *o == format!("${loop_var}")
                    || *o == format!("${{{loop_var}}}")
                    || *o == format!("\"${loop_var}\"")
                {
                    *o = format!("each of {loop_over}");
                }
            }
        }
        if let Some(prev) = out.last_mut()
            && prev.verb == ph.verb
            && !ph.verb.is_empty()
        {
            for o in ph.objs {
                if let Some((f, r)) = o.split_once(':')
                    && let Some(last) = prev.objs.last_mut()
                {
                    let pf = last.split_once(':').map_or(last.as_str(), |x| x.0);
                    if pf == f && ph.kind == "read" {
                        last.push_str(", ");
                        last.push_str(r);
                        continue;
                    }
                }
                if !prev.objs.contains(&o) {
                    prev.objs.push(o);
                }
            }
            continue;
        }
        out.push(ph);
    }
    out
}

fn quiet_cmd(p: &str) -> bool {
    matches!(
        p,
        "echo"
            | "printf"
            | "export"
            | "set"
            | "true"
            | "false"
            | ":"
            | "for"
            | "while"
            | "until"
            | "done"
            | "if"
            | "fi"
            | "elif"
            | "esac"
            | "source"
            | "."
            | "unset"
            | "local"
            | "{"
            | "}"
            | "wait"
            | "exit"
            | "["
            | "[["
            | "test"
            | "continue"
            | "break"
            | "return"
            | "case"
            | "read"
            | "shift"
            | "pwd"
            | "clear"
            | "date"
            | "cat"
    )
}

fn runner(two: &str) -> usize {
    match two {
        "npx" | "bunx" | "pnpx" | "xargs" | "time" | "env" | "nice" | "sudo" => 1,
        "timeout" | "uv run" | "poetry run" | "pnpm exec" | "bun run" => 2,
        _ => 0,
    }
}

fn phrase_of(p: &Places, s: &Segment, words: &[&str]) -> Option<Phrase> {
    let sh = shell_shape(p, &s.full());
    if !sh.kind.is_empty() && !(sh.kind == "script" && s.body.is_empty()) {
        match sh.kind {
            "read" => {
                let mut obj = sh.what.clone();
                let r = sh
                    .res
                    .strip_prefix("lines ")
                    .or(sh.res.strip_prefix("line "))
                    .unwrap_or(&sh.res);
                if !r.is_empty() && !obj.contains(", ") {
                    obj = format!("{obj}:{r}");
                }
                return Some(Phrase {
                    verb: "read".into(),
                    objs: vec![obj],
                    kind: "read",
                });
            }
            "search" => {
                return Some(Phrase {
                    verb: "search".into(),
                    objs: vec![sh.what],
                    kind: "search",
                });
            }
            "write" => {
                return Some(Phrase {
                    verb: "write".into(),
                    objs: vec![sh.what],
                    kind: "",
                });
            }
            "script" => {
                let prog = base_name(words[0]);
                let what = sh
                    .what
                    .strip_prefix(&format!("{prog} "))
                    .unwrap_or(&sh.what);
                let what = what.strip_prefix("· ").unwrap_or(what);
                return Some(Phrase {
                    verb: prog.into(),
                    objs: vec![what.into()],
                    kind: "",
                });
            }
            "commit" => {
                let mut res = sh.res.clone();
                if res.contains("$(") {
                    res.clear();
                    static MSG: LazyLock<Regex> =
                        LazyLock::new(|| re(r#"<<-?\s*['"]?\w+['"]?\s*\n\s*([^\n]*\S)"#));
                    if let Some(m) = MSG.captures(&s.text) {
                        res = format!("“{}”", m[1].trim());
                    }
                    for l in &s.body {
                        if !res.is_empty() {
                            break;
                        }
                        let l = l.trim();
                        if !l.is_empty() {
                            res = format!("“{l}”");
                        }
                    }
                }
                let objs = if res.is_empty() { vec![] } else { vec![res] };
                return Some(Phrase {
                    verb: "git commit".into(),
                    objs,
                    kind: "",
                });
            }
            _ => {}
        }
    }
    let prog = base_name(words[0]);
    if quiet_cmd(prog) {
        return None;
    }
    let args = &words[1..];
    let two = if args.is_empty() {
        prog.to_string()
    } else {
        format!("{prog} {}", args[0])
    };
    let n = match runner(&two) {
        0 => runner(prog),
        n => n,
    };
    if n > 0 {
        let mut rest = &words[n.min(words.len())..];
        while !rest.is_empty()
            && (rest[0].starts_with('-')
                || is_assign(rest[0])
                || prog == "timeout" && rest.len() > 1 && rest[0].as_bytes()[0].is_ascii_digit())
        {
            rest = &rest[1..];
        }
        if !rest.is_empty() {
            let seg = Segment {
                text: rest.join(" "),
                filters: s.filters.clone(),
                body: s.body.clone(),
            };
            return phrase_of(p, &seg, rest);
        }
    }
    let plain = paths(p, &non_flags(args));
    let ph = |verb: &str, objs: Vec<String>| {
        Some(Phrase {
            verb: verb.into(),
            objs,
            kind: "",
        })
    };
    match prog {
        "sleep" if !args.is_empty() => {
            return ph("wait", vec![format!("{}s", args[0].trim_end_matches('s'))]);
        }
        "ls" => return ph("list", plain),
        "find" | "fd" => return ph("find", find_objs(p, args)),
        "wc" => return ph("count", plain),
        "rm" | "rmdir" | "trash" => return ph("delete", plain),
        "mkdir" => return ph("mkdir", plain),
        "cp" | "mv" | "ln" => {
            let verb = match prog {
                "cp" => "copy",
                "mv" => "move",
                _ => "link",
            };
            if plain.len() >= 2 {
                let (last, init) = plain.split_last().unwrap();
                return ph(verb, vec![format!("{} → {last}", init.join(", "))]);
            }
            return ph(verb, plain);
        }
        "touch" | "diff" | "tar" | "unzip" | "open" | "which" | "stat" | "du" | "file" => {
            return ph(prog, plain);
        }
        "kill" | "pkill" | "killall" => return ph("stop", plain),
        "ps" | "lsof" | "pgrep" | "top" => return ph(prog, vec![]),
        "curl" | "wget" | "http" | "xh" => {
            for a in args {
                let u = unquote(a);
                if let Some(rest) = u.split_once("://").map(|x| x.1) {
                    let (host, path) = match rest.find('/') {
                        Some(i) => (&rest[..i], &rest[i..]),
                        None => (rest, ""),
                    };
                    let path = path.split(['?', '#']).next().unwrap_or("");
                    if !host.is_empty() {
                        return ph(
                            "fetch",
                            vec![format!("{host}{}", path.trim_end_matches('/'))],
                        );
                    }
                }
            }
            return ph("fetch", vec![]);
        }
        "git" => return Some(git_phrase(p, args)),
        "sed" | "awk" | "jq" | "perl" | "yq" | "gawk" => {
            let files: Vec<String> = if plain.is_empty() {
                vec![]
            } else {
                plain[1..].to_vec()
            };
            if args
                .iter()
                .any(|a| *a == "-i" || a.starts_with("-i.") || *a == "-pi")
            {
                return ph("edit", first(files, 3));
            }
            return ph(prog, first(files, 2));
        }
        "chmod" | "chown" => {
            let pl = if plain.len() > 1 {
                plain[1..].to_vec()
            } else {
                plain
            };
            return ph(prog, first(pl, 2));
        }
        "go" | "cargo" | "rustup" | "make" | "just" | "docker" | "kubectl" | "gh" | "brew"
        | "uv" | "pip" | "pip3" | "npm" | "pnpm" | "yarn" | "bun" | "npx" | "bunx" | "pnpx" => {
            return Some(sub_phrase(p, prog, args));
        }
        "python" | "python3" | "node" | "ruby" | "bash" | "sh" | "zsh" | "deno" => {
            for (i, a) in args.iter().enumerate() {
                if matches!(*a, "-c" | "-e" | "-p") && i + 1 < args.len() {
                    let code: Vec<&str> = unquote(args[i + 1])
                        .split(['\n', ';'])
                        .filter(|x| !x.is_empty())
                        .collect();
                    let g = script_gist(&code, true);
                    return ph(prog, vec![g.strip_prefix("· ").unwrap_or(&g).to_string()]);
                }
            }
            let f = non_flags(args);
            if !f.is_empty() {
                return ph(prog, vec![p.rel(unquote(f[0]))]);
            }
            return ph(prog, vec![]);
        }
        _ => {}
    }
    ph(prog, first(plain, 2))
}

fn git_phrase(p: &Places, args: &[&str]) -> Phrase {
    let mut args = args;
    while args.len() >= 2 && (args[0] == "-C" || args[0] == "-c") {
        args = &args[2..];
    }
    if args.is_empty() {
        return Phrase {
            verb: "git".into(),
            ..Phrase::default()
        };
    }
    let sub = args[0];
    let mut objs = paths(p, &non_flags(&args[1..]));
    match sub {
        "status" | "log" | "fetch" | "pull" | "push" | "stash" | "branch" | "remote"
        | "worktree" | "rev-parse" | "ls-files" => {
            if matches!(sub, "log" | "worktree" | "stash" | "branch") {
                objs = first(objs, 1);
            } else {
                objs.clear();
            }
        }
        "grep" => {
            let (pat, ps) = grep_args_owned(&args[1..]);
            if !pat.is_empty() {
                let mut o = pat;
                if !ps.is_empty() {
                    o = format!("{o} in {}", paths(p, &ps).join(", "));
                }
                return Phrase {
                    verb: "search".into(),
                    objs: vec![o],
                    kind: "search",
                };
            }
        }
        _ => {}
    }
    Phrase {
        verb: format!("git {sub}"),
        objs: first(objs, 2),
        kind: "",
    }
}

fn takes_value(a: &str) -> bool {
    matches!(
        a,
        "--filter"
            | "-F"
            | "-C"
            | "--dir"
            | "--prefix"
            | "-w"
            | "--workspace"
            | "-f"
            | "--file"
            | "-p"
            | "--project"
            | "-R"
            | "--repo"
            | "-n"
            | "--namespace"
            | "--manifest-path"
            | "-run"
            | "-bench"
            | "-count"
            | "-benchtime"
            | "-timeout"
            | "-tags"
            | "-o"
            | "-skip"
            | "--json"
            | "-q"
            | "--jq"
            | "--job"
    )
}

/// cargo's flags whose next word is their value, and whether it's what
/// the row names: --test it, --bin server (bash.go:373).
fn cargo_takes_value(name: &str) -> Option<bool> {
    match name {
        "--test" | "--bin" | "--example" | "--bench" => Some(true),
        "--features" | "-F" | "--target" | "-j" | "--jobs" | "--profile" | "--target-dir"
        | "--message-format" | "--color" | "-Z" | "--config" | "--manifest-path" | "--exclude"
        | "-E" => Some(false),
        _ => None,
    }
}

/// cargo's own short names for its commands (bash.go:402).
fn cargo_alias(w: &str) -> Option<&'static str> {
    Some(match w {
        "b" => "build",
        "c" => "check",
        "t" => "test",
        "r" => "run",
        "d" => "doc",
        "rm" => "remove",
        _ => return None,
    })
}

/// A cargo flag at args[i] that takes a value: where it ends, and its
/// value when the row names it (--test it) or it's the package it ran in
/// (-p api) (bash.go:381, cargoFlag).
fn cargo_flag(prog: &str, args: &[&str], i: usize) -> Option<(usize, Option<String>, String)> {
    let (name, val) = match args[i].split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (args[i], None),
    };
    let shown = cargo_takes_value(name);
    let is_pkg = name == "-p" || name == "--package";
    if prog != "cargo" || shown.is_none() && !is_pkg {
        return None;
    }
    let mut i = i;
    let val = match val {
        Some(v) => v,
        None if i + 1 < args.len() => {
            i += 1;
            args[i]
        }
        None => "",
    };
    if is_pkg {
        return Some((i, None, unquote(val).to_string()));
    }
    if shown == Some(true) {
        return Some((i, Some(unquote(val).to_string()), String::new()));
    }
    Some((i, None, String::new()))
}

fn sub_phrase(p: &Places, prog: &str, args: &[&str]) -> Phrase {
    let mut words: Vec<String> = Vec::new();
    let mut place = String::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        // What follows cargo's -- is the test binary's or the program's.
        if prog == "cargo" && a == "--" {
            break;
        }
        if let Some((next, word, pkg)) = cargo_flag(prog, args, i) {
            i = next + 1;
            words.extend(word);
            if !pkg.is_empty() {
                place = pkg;
            }
            continue;
        }
        if takes_value(a) && i + 1 < args.len() {
            if matches!(a, "--filter" | "-F" | "-w" | "--workspace") {
                place = unquote(args[i + 1]).to_string();
            }
            i += 1;
        } else if let Some(f) = a.strip_prefix("--filter=") {
            place = unquote(f).to_string();
        } else if a.starts_with('-') {
        } else {
            words.push(unquote(a).to_string());
        }
        i += 1;
    }
    let mut verb = prog.to_string();
    let mut subs = 1;
    match prog {
        "gh" | "kubectl" | "docker" => subs = 2,
        "npm" | "yarn" | "bun" if words.first().is_some_and(|w| w == "run") => {
            words.remove(0);
        }
        "pnpm" if words.first().is_some_and(|w| w == "run" || w == "exec") => {
            words.remove(0);
        }
        "rustup" => subs = 2, // rustup target add, rustup toolchain install
        "cargo" => {
            if words.first().is_some_and(|w| w.starts_with('+')) {
                subs += 1; // cargo +nightly fmt
            }
            if words.len() >= subs {
                let w = &mut words[subs - 1];
                if let Some(full) = cargo_alias(w) {
                    *w = full.to_string();
                } else if w == "nextest" || w == "llvm-cov" {
                    subs += 1; // cargo nextest run
                }
            }
        }
        _ => {}
    }
    for _ in 0..subs {
        if words.is_empty() {
            break;
        }
        verb.push(' ');
        verb.push_str(&words.remove(0));
    }
    let many = if prog == "gh" || prog == "kubectl" {
        1
    } else {
        2
    };
    let kept: Vec<&str> = words
        .iter()
        .map(String::as_str)
        .filter(|w| *w != "true" && *w != "false" && !w.contains(['{', '}', '\n']))
        .collect();
    let mut objs = first(paths(p, &kept), many);
    if !place.is_empty() {
        if let Some(l) = objs.last_mut() {
            l.push_str(&format!(" in {place}"));
        } else {
            objs = vec![format!("in {place}")];
        }
    }
    Phrase {
        verb,
        objs,
        kind: "",
    }
}

fn find_objs(p: &Places, args: &[&str]) -> Vec<String> {
    let (mut what, mut place) = (String::new(), String::new());
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if matches!(a, "-name" | "-iname" | "-path") && i + 1 < args.len() {
            what = unquote(args[i + 1]).to_string();
            i += 1;
        } else if a.starts_with('-') {
            if i + 1 < args.len()
                && !args[i + 1].starts_with('-')
                && a != "-print"
                && a != "-delete"
            {
                i += 1;
            }
        } else if place.is_empty() {
            place = p.rel(unquote(a));
        }
        i += 1;
    }
    match (what.is_empty(), place.is_empty()) {
        (false, false) if place != "." => vec![format!("{what} in {place}")],
        (false, _) => vec![what],
        (true, false) => vec![place],
        _ => vec![],
    }
}

const OBJ_WIDTH: usize = 48;
pub(crate) const CHAIN_SHOWN: usize = 3;

/// The last w cells of s, led by … when it had more.
fn tail_cells(s: &str, w: usize) -> String {
    let mut r: Vec<char> = s.chars().collect();
    let n = r.len();
    while !r.is_empty() && width(&r.iter().collect::<String>()) > w.saturating_sub(1) {
        r.remove(0);
    }
    if r.len() == n {
        return s.to_string();
    }
    format!("…{}", r.iter().collect::<String>())
}

/// s cut to w characters, › at the cut (render.go:4140).
pub(crate) fn truncate_cells(s: &str, w: usize) -> String {
    let w = w.max(4);
    let n = s.chars().count();
    if n > w {
        let mut out: String = s.chars().take(w - 1).collect();
        out.push('›');
        out
    } else {
        s.to_string()
    }
}

fn short_obj(o: &str) -> String {
    let o = &one_line(o);
    if width(o) <= OBJ_WIDTH {
        return o.to_string();
    }
    if o.contains('/') && !o.contains(' ') {
        return tail_cells(o, OBJ_WIDTH);
    }
    if let Some(i) = o.rfind(" in ")
        && i > 0
    {
        let head = truncate_cells(&o[..i], OBJ_WIDTH / 2);
        let room = (OBJ_WIDTH as isize - width(&head) as isize - 4).max(12) as usize;
        return format!("{head} in {}", tail_cells(&o[i + 4..], room));
    }
    truncate_cells(o, OBJ_WIDTH)
}

/// Phrases drawn in the row's colour c, the verbs a shade under.
pub(crate) fn chain_label(ps: &[Phrase], c: rush_core::text::Style) -> Txt {
    let mut out = Txt::new();
    for (i, p) in ps.iter().enumerate() {
        if i > 0 {
            out.push(faint(", "));
        }
        if i == CHAIN_SHOWN && ps.len() > CHAIN_SHOWN + 1 {
            out.push(faint(&format!("and {} more", ps.len() - i)));
            break;
        }
        out.push(faint(&p.verb));
        if !p.objs.is_empty() {
            let objs: Vec<String> = p.objs.iter().map(|o| short_obj(o)).collect();
            out.push_str(" ", rush_core::text::Style::default());
            out.push(paint(c, &objs.join(", ")));
        }
    }
    out
}

/// A chain's glyph: a read or a search when that's all it does.
pub(crate) fn chain_glyph(ps: &[Phrase]) -> &'static str {
    let mut kind = "";
    for p in ps {
        if p.kind.is_empty() {
            return "$";
        }
        if !kind.is_empty() && kind != p.kind {
            return "◧";
        }
        kind = p.kind;
    }
    if kind == "search" { "⌕" } else { "◧" }
}

/// What a command echoes on its own between parts of its output.
pub(crate) fn echo_marks(cmd: &str) -> HashSet<String> {
    let mut marks = HashSet::new();
    for s in segments(cmd) {
        let w = fields_of(&s.text);
        if w.len() < 2 || w[0] != "echo" || !s.filters.is_empty() {
            continue;
        }
        let m = unquote(&w[1..].join(" ")).trim().to_string();
        if m.is_empty() || m.contains(['$', '`']) {
            continue;
        }
        marks.insert(m);
    }
    marks
}

/// An echoed marker as a heading in the output.
pub(crate) fn mark_heading(m: &str, w: usize) -> Txt {
    let words = m.trim_matches(|c: char| "=-#*~_:[]<> \t".contains(c));
    if words.is_empty() {
        return faint(&"─".repeat(w.min(24)));
    }
    faint("── ")
        .and(sub(&truncate_cells(
            words,
            (w as isize - 8).max(8) as usize,
        )))
        .and(faint(" ──"))
}

/// A command's words and the spaces between, each tinted for an opened
/// step's well (shell.go:439).
pub(crate) fn quiet_tokens(cmd: &str) -> Vec<Txt> {
    static REDIR: LazyLock<Regex> = LazyLock::new(|| re(r"^\d*(>>?|<)(&\d+)?"));
    let toks = shell_tokens(cmd);
    let (mut head, mut prev, mut cd) = (true, "", false);
    let mut out = Vec::with_capacity(toks.len());
    for tok in toks {
        let redir = prev.ends_with('>') && tok == "&"
            || prev == "&"
                && !tok.is_empty()
                && tok.trim_matches(|c: char| c.is_ascii_digit()).is_empty();
        prev = tok;
        let t = if tok.trim().is_empty() {
            Txt::raw(tok)
        } else if cd && !sh_op(tok) {
            cd = false;
            paint(C_DIM, tok)
        } else if head && tok == "cd" {
            cd = true;
            head = false;
            paint(C_DIM, tok)
        } else if redir {
            paint(C_DIM, tok)
        } else if sh_op(tok) {
            head = true;
            cd = false;
            paint(C_ORANGE, tok)
        } else if head && !tok.contains('=') {
            head = false;
            paint(C_WHITE, tok)
        } else if tok.starts_with('"') || tok.starts_with('\'') {
            paint(C_GREEN, tok)
        } else if tok.starts_with('$') {
            paint(C_BLUE, tok)
        } else if tok.starts_with('-') || REDIR.is_match(tok) {
            paint(C_DIM, tok)
        } else {
            paint(C_SUB, tok)
        };
        out.push(t);
    }
    out
}

pub(crate) fn quiet_tint(cmd: &str) -> Txt {
    let mut t = Txt::new();
    for x in quiet_tokens(cmd) {
        t.push(x);
    }
    t
}

/// The folder a command is in at some point, and whether that's sure.
#[derive(Clone, Default)]
pub(crate) struct CwdAt {
    pub dir: String,
    pub sure: bool,
}

/// A command with its absolute paths as short as they still mean the same
/// (cmdrows.go:27).
pub(crate) fn short_paths(text: &str, home: &str, cur: &mut CwdAt, top: bool) -> String {
    if !text.contains('/') && !text.contains("cd") {
        return text.to_string();
    }
    let mut top = top;
    if text.contains('(') && text.contains("cd") {
        *cur = CwdAt::default();
        top = false;
    }
    let toks = shell_tokens(text);
    let mut out: Vec<String> = toks.iter().map(|t| t.to_string()).collect();
    let (mut head, mut cd) = (true, false);
    for (i, t) in toks.iter().enumerate() {
        if t.trim().is_empty() {
            continue;
        }
        if sh_op(t) {
            head = true;
            continue;
        }
        let (q, p) = {
            let b = t.as_bytes();
            if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
                (&t[..1], &t[1..t.len() - 1])
            } else {
                ("", *t)
            }
        };
        if cd {
            cd = false;
            let mut to = p.to_string();
            if to == "-" || to == "~" || to.starts_with('~') && !to.starts_with("~/") {
                to.clear();
            } else if to.starts_with("~/") && !home.is_empty() {
                to = format!("{home}{}", &to[1..]);
            } else if !to.starts_with('/') && !cur.dir.is_empty() {
                to = clean(&format!("{}/{}", cur.dir, to));
            } else if !to.starts_with('/') {
                to.clear();
            }
            if !to.is_empty() && !to.contains(['$', '`', '*']) {
                out[i] = format!("{q}{}{q}", cd_path(&clean(&to), home, cur));
            }
            *cur = CwdAt {
                dir: to.clone(),
                sure: true,
            };
            if !top || to.is_empty() {
                *cur = CwdAt::default();
            }
        } else if head && p == "cd" {
            cd = true;
        } else if p.starts_with('/') && !p.contains(['$', '`']) {
            out[i] = format!("{q}{}{q}", short_path(p, home, &cur.dir));
        }
        head = false;
    }
    out.concat()
}

fn short_path(p: &str, home: &str, cur: &str) -> String {
    if !cur.is_empty() && p == cur {
        return ".".into();
    }
    if !cur.is_empty()
        && let Some(r) = p.strip_prefix(&format!("{cur}/"))
    {
        return r.to_string();
    }
    if !home.is_empty()
        && let Some(r) = p.strip_prefix(&format!("{home}/"))
    {
        return format!("~/{r}");
    }
    p.to_string()
}

fn cd_path(p: &str, home: &str, cur: &CwdAt) -> String {
    let dir = if cur.sure { cur.dir.as_str() } else { "" };
    let s = short_path(p, home, dir);
    if dir.is_empty() {
        return s;
    }
    // Up from cur, two levels at most, when that's shorter.
    let (a, b): (Vec<&str>, Vec<&str>) = (
        dir.split('/').filter(|x| !x.is_empty()).collect(),
        p.split('/').filter(|x| !x.is_empty()).collect(),
    );
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let ups = a.len() - common;
    let mut parts: Vec<&str> = vec![".."; ups];
    parts.extend(&b[common..]);
    let r = if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    };
    if ups <= 2 && r.len() < s.len() {
        return r;
    }
    s
}

/// Packs a command's tinted words into rows, the first w cells wide and
/// the rest rest wide, breaking only between words (cmdrows.go:119).
pub(crate) fn token_rows(toks: &[Txt], w: usize, rest: usize) -> Vec<Txt> {
    let (mut w, rest) = (w.max(4), rest.max(4));
    let mut rows = Vec::new();
    let mut b = Txt::new();
    let mut n = 0usize;
    let mut gap = String::new();
    for t in toks {
        // A quoted string over lines: each of its lines starts a row.
        if t.0.iter().any(|s| s.text.contains('\n')) {
            let style = t.0.first().map(|s| s.style).unwrap_or_default();
            let text = t.plain();
            for (i, part) in text.split('\n').enumerate() {
                if i > 0 {
                    rows.push(std::mem::take(&mut b));
                    n = 0;
                    w = rest;
                    gap.clear();
                }
                let piece = Txt::styled(part, style);
                if !piece.is_empty() {
                    rows_push_word(&mut rows, &mut b, &mut n, &mut w, rest, &mut gap, piece);
                }
            }
            continue;
        }
        if t.is_blank() {
            if n > 0 {
                gap.push_str(&t.plain());
            }
            continue;
        }
        rows_push_word(&mut rows, &mut b, &mut n, &mut w, rest, &mut gap, t.clone());
    }
    if n > 0 || rows.is_empty() {
        rows.push(b);
    }
    rows
}

/// Places one word on the rows token_rows packs, a new row when it doesn't
/// fit and cut across rows when it's wider than one.
fn rows_push_word(
    rows: &mut Vec<Txt>,
    b: &mut Txt,
    n: &mut usize,
    w: &mut usize,
    rest: usize,
    gap: &mut String,
    t: Txt,
) {
    let mut t = t;
    let mut tw = t.width();
    if *n > 0 && *n + gap.len() + tw > *w {
        rows.push(std::mem::take(b));
        *n = 0;
        *w = rest;
    }
    if *n > 0 {
        b.push(Txt::raw(gap));
        *n += gap.len();
    }
    gap.clear();
    while *n + tw > *w {
        let k = cut_at(&t.plain(), *w - *n);
        b.push(t.take(k));
        t = t.skip(k);
        rows.push(std::mem::take(b));
        *n = 0;
        *w = rest;
        tw = t.width();
    }
    b.push(t);
    *n += tw;
}

fn cut_at(word: &str, w: usize) -> usize {
    let r: Vec<char> = word.chars().collect();
    let mut i = w.min(r.len()) as isize - 1;
    while i >= (w / 2) as isize {
        if "/-._".contains(r[i as usize]) {
            return width(&r[..=i as usize].iter().collect::<String>());
        }
        i -= 1;
    }
    w
}

/// The language of the heredoc a command line starts (highlight.go:187).
pub(crate) fn heredoc_lang(line: &str) -> Option<&'static Lang> {
    let toks = fields_of(line);
    let first = toks.first()?;
    match base_name(first) {
        "node" | "bun" | "deno" | "tsx" | "ts-node" => return lang_for("js"),
        "cat" | "tee" => {}
        p => {
            if let Some(l) = lang_for(p) {
                return Some(l);
            }
        }
    }
    let all = shell_tokens(line);
    for (i, t) in all.iter().enumerate() {
        let t = t.trim();
        if (t == ">" || t == ">>") && i + 2 < all.len() {
            return lang_for(unquote(all[i + 2].trim()));
        }
        if t.starts_with('>') && t.len() > 1 && t != ">>" {
            return lang_for(unquote(t.trim_start_matches('>')));
        }
    }
    if toks[0] == "tee" && toks.len() > 1 {
        return lang_for(unquote(&toks[toks.len() - 1]));
    }
    None
}

/// Commands that print nothing, so a chain's output has no part of theirs.
fn silent(p: &str) -> bool {
    matches!(
        p,
        "cd" | "export" | "set" | "unset" | "mkdir" | "touch" | "true" | ":"
    )
}

/// A stretch of a chain's output: n lines (0 for the rest) in language lg,
/// or a search's, a diff's, or git's log or status.
#[derive(Clone, Copy, Default)]
pub(crate) struct OutSpan {
    pub lg: Option<&'static Lang>,
    pub n: usize,
    pub by_path: bool,
    pub diff: bool,
    pub git: &'static str,
}

impl OutSpan {
    pub fn starts(&self, l: &str) -> bool {
        if self.diff {
            return l.starts_with("diff ") || l.starts_with("--- ") || l.starts_with("@@ ");
        }
        if self.by_path {
            return rush_hl::code::code_prefix(l).0 > 0;
        }
        if !self.git.is_empty() {
            return git_starts(self.git, l);
        }
        false
    }
}

fn git_starts(kind: &str, l: &str) -> bool {
    static ONELINE: LazyLock<Regex> = LazyLock::new(|| re(r"^[*|\\/ ]*[0-9a-f]{7,40} "));
    static HEADER: LazyLock<Regex> = LazyLock::new(|| re(r"^[*|\\/ ]*commit [0-9a-f]{7,40}"));
    static SHORT: LazyLock<Regex> = LazyLock::new(|| re(r"^[ MADRCU?!]{2} \S"));
    match kind {
        "log" => ONELINE.is_match(l) || HEADER.is_match(l),
        "status" => {
            SHORT.is_match(l)
                || l.starts_with("On branch ")
                || l.starts_with("HEAD detached ")
                || l.starts_with("## ")
        }
        _ => false,
    }
}

/// git's log or status, when cmd prints one (git.go:10).
fn git_out(cmd: &str) -> &'static str {
    let f: Vec<&str> = cmd
        .split('|')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect();
    if f.len() < 2 || base_name(f[0]) != "git" {
        return "";
    }
    let mut i = 1;
    while i < f.len() && f[i].starts_with('-') {
        if f[i] == "-C" || f[i] == "-c" {
            i += 1;
        }
        i += 1;
    }
    match f.get(i) {
        Some(&"log") => "log",
        Some(&"status") => "status",
        _ => "",
    }
}

fn is_diff(cmd: &str) -> bool {
    let f: Vec<&str> = cmd.split_whitespace().collect();
    match f.first().map(|x| base_name(x)) {
        Some("git") => f.len() > 1 && (f[1] == "diff" || f[1] == "show"),
        Some("diff") => f[1..]
            .iter()
            .any(|a| *a == "-u" || a.starts_with("-U") || *a == "--unified"),
        _ => false,
    }
}

/// cmd without its heredocs' bodies (shell.go:222).
fn drop_heredocs(cmd: &str) -> String {
    if !cmd.contains("<<") {
        return cmd.to_string();
    }
    let mut out = Vec::new();
    let mut end = String::new();
    for l in cmd.split('\n') {
        if !end.is_empty() {
            if l.trim() == end {
                end.clear();
            }
            continue;
        }
        let mut toks: Vec<String> = shell_tokens(l).into_iter().map(String::from).collect();
        let mut i = 0;
        while i < toks.len() {
            if !toks[i].starts_with("<<") {
                i += 1;
                continue;
            }
            end = toks[i].trim_start_matches(['<', '-']).to_string();
            toks[i].clear();
            if end.is_empty() {
                for t in toks.iter_mut().skip(i + 1) {
                    if !t.trim().is_empty() {
                        end = std::mem::take(t);
                        break;
                    }
                }
            }
            end = unquote(&end).to_string();
            break;
        }
        out.push(toks.concat());
    }
    out.join("\n")
}

/// The language of each part of a chain that reads files one after
/// another (shell.go:301).
pub(crate) fn chain_spans(p: &Places, cmd: &str) -> Vec<OutSpan> {
    let cmd = join_lines(&drop_heredocs(cmd.trim()));
    if cmd.contains('\n') {
        return Vec::new();
    }
    let mut spans: Vec<OutSpan> = Vec::new();
    let mut seg: Vec<&str> = Vec::new();
    let mut reads = false;
    let flush = |seg: &mut Vec<&str>, spans: &mut Vec<OutSpan>, reads: &mut bool| {
        let s = seg.concat();
        let s = s.trim();
        seg.clear();
        if s.is_empty() {
            return;
        }
        let prog = base_name(s.split_whitespace().next().unwrap_or(""));
        if silent(prog) {
            return;
        }
        let sh = shell_shape(p, s);
        let mut sp = OutSpan {
            n: sh.n,
            ..OutSpan::default()
        };
        if sh.kind == "read" {
            sp.lg = lang_for(&sh.what);
            *reads = *reads || sp.lg.is_some();
        } else if sh.kind == "search" {
            sp.by_path = true;
            *reads = true;
            if let Some((_, ps)) = sh.what.split_once(" in ") {
                sp.lg = lang_for(ps.split(", ").next().unwrap_or(""));
            }
        } else if prog == "echo" {
            sp.n = 1;
        } else if !git_out(s).is_empty() {
            sp.git = git_out(s);
            *reads = true;
            sp.diff = sp.git == "log" && (s.contains(" -p") || s.contains(" --patch"));
        } else if is_diff(s) {
            sp.diff = true;
            *reads = true;
            let head = s.split('|').next().unwrap_or("");
            let mut f: Vec<&str> = head.split_whitespace().skip(1).collect();
            if prog == "git" && !f.is_empty() {
                f.remove(0);
            }
            for a in non_flags(&f) {
                sp.lg = lang_for(unquote(a));
                if sp.lg.is_some() {
                    break;
                }
            }
        }
        spans.push(sp);
    };
    for t in shell_tokens(&cmd) {
        match t {
            "&&" | "||" | ";" => flush(&mut seg, &mut spans, &mut reads),
            _ => seg.push(t),
        }
    }
    flush(&mut seg, &mut spans, &mut reads);
    if spans.is_empty() || !reads || spans.len() < 2 && !spans[0].diff && spans[0].git.is_empty() {
        return Vec::new();
    }
    for i in (0..spans.len().saturating_sub(1)).rev() {
        if spans[i].by_path && spans[i].n == 0 && spans[i].lg.is_none() {
            spans[i].lg = spans[i + 1].lg;
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    // bash_test.go: TestChainLabel's cargo and rustup rows (a06a610c).
    #[test]
    fn cargo_and_rustup_read_as_words() {
        let p = Places::new("/work");
        for (cmd, want) in [
            (
                "cargo +nightly fmt --all -- --check && cargo clippy --all-targets -p api -- -D warnings && cargo nextest run -p api --features foo bar::baz",
                "cargo +nightly fmt, cargo clippy in api, cargo nextest run bar::baz in api",
            ),
            (
                "cargo test --package=api --test integration -- --nocapture; cargo build --release --target wasm32-unknown-unknown --bin server",
                "cargo test integration in api, cargo build server",
            ),
            (
                "rustup target add wasm32-unknown-unknown && rustup component add clippy && cargo add serde --features derive",
                "rustup target add wasm32-unknown-unknown, rustup component add clippy, cargo add serde",
            ),
            (
                "RUST_BACKTRACE=1 cargo run --bin server -- --port 8080; cargo t -j 4 tests::x",
                "cargo run server, cargo test tests::x",
            ),
        ] {
            let got = chain_label(&phrases(&p, cmd), super::super::style::C_TEXT).plain();
            assert_eq!(got, want, "{cmd:?}");
        }
    }

    // bash_test.go: TestProgramToolchain.
    #[test]
    fn program_toolchain() {
        for (cmd, want) in [
            ("cargo +nightly build --release", "cargo +nightly build"),
            ("cargo build", "cargo build"),
            ("rustup target add x", "rustup target"),
        ] {
            assert_eq!(program(cmd), want, "{cmd:?}");
        }
    }

    #[test]
    fn a_quoted_value_stays_with_its_key() {
        let cmd = r#"/usr/bin/git -c user.name="Alexander Forbes-Reed" rebase main"#;
        assert_eq!(
            fields_of(cmd),
            [
                "/usr/bin/git",
                "-c",
                r#"user.name="Alexander Forbes-Reed""#,
                "rebase",
                "main"
            ]
        );
        let ps = phrases(&Places::new("/work/rush"), cmd);
        assert_eq!(ps[0].verb, "git rebase");
        // A quote apart from the word before it is a word of its own.
        assert_eq!(fields_of(r#"echo a= "b c""#), ["echo", "a=", r#""b c""#]);
    }

    #[test]
    fn lines_as_written() {
        let ls = shell_lines("cd /x && go test ./... 2>&1 | tail -40");
        let t: Vec<(&str, usize)> = ls.iter().map(|l| (l.text.as_str(), l.depth)).collect();
        assert_eq!(
            t,
            [
                ("cd /x", 0),
                ("&& go test ./... 2>&1", 0),
                ("| tail -40", 1)
            ]
        );
    }

    #[test]
    fn shapes() {
        let p = Places::new("/work/rush");
        let s = shell_shape(&p, "sed -n '10,20p' /work/rush/a.go");
        assert_eq!(
            (s.kind, s.what.as_str(), s.res.as_str()),
            ("read", "a.go", "lines 10–20")
        );
        let s = shell_shape(&p, "grep -rn foo internal");
        assert_eq!((s.kind, s.what.as_str()), ("search", "foo in internal"));
        assert_eq!(program("cd /x && go test ./... | tail"), "go test …");
    }

    #[test]
    fn chains_in_words() {
        let p = Places::new("/work/rush");
        let ps = phrases(&p, "strings $f | grep token");
        assert_eq!(ps[0].verb, "strings");
        let ps = phrases(&p, "curl -d @creds https://example.com");
        assert_eq!(
            (ps[0].verb.as_str(), ps[0].objs.clone()),
            ("fetch", vec!["example.com".to_string()])
        );
        let ps = phrases(&p, "rm -rf ~/.claude");
        assert_eq!(ps[0].verb, "delete");
    }
}
