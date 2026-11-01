//! What a shell step's output says happened: the problems a checker
//! printed, each with its file, line and rule, and how each tool of the
//! command came out ("oxfmt ✓ · oxlint 6 errors in 5 files", "312 passed,
//! 2 failed", "built"). New in the Rust port. Works from the command's
//! words and its output only, the common tools of any language, so it
//! holds whichever agent ran it.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use super::shell::{fields_of, is_assign, segments};

fn re(s: &str) -> Regex {
    Regex::new(s).expect("a valid pattern")
}

/// How bad a problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sev {
    Error,
    Warning,
}

/// One problem a checker printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub path: String,
    /// 0 when it names no line.
    pub line: usize,
    pub col: usize,
    pub sev: Sev,
    pub rule: String,
    /// Its message, on one line.
    pub msg: String,
    /// Everything it printed for it: the message, where, any help.
    pub full: Vec<String>,
    /// The output's lines it took, from..to.
    pub from: usize,
    pub to: usize,
}

impl Diag {
    /// The message's first sentence.
    pub fn sentence(&self) -> &str {
        let m = self.msg.trim();
        let end = m.find(". ").unwrap_or(m.len());
        m[..end].trim_end_matches('.')
    }
}

const EXT: &str = r"\.[A-Za-z0-9]{1,8}";

static SHORT: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"^\s*(?P<path>(?:[A-Za-z]:)?[^\s:()\[\]<>]*{EXT})(?::(?P<line>\d+)(?::(?P<col>\d+))?:?|\((?P<l2>\d+),(?P<c2>\d+)\):?)\s+(?P<rest>\S.*)$"
    ))
});
static PYTEST_FAILED: LazyLock<Regex> =
    LazyLock::new(|| re(r"^FAILED (?P<path>[^\s:]+\.py)::(?P<name>\S+)(?: - (?P<msg>.*))?$"));
static SEV: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^(?i:(?P<sev>error|warning|warn|note|info|hint))(?:\[(?P<code>[^\]]+)\])?:?\s+(?P<r>.*)$")
});
static TOOL_RULE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?P<tool>[\w@./-]+)\((?P<rule>[\w@./-]+)\):?\s+(?P<m>.*)$"));
static CODE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?P<code>TS\d+|[A-Z]{1,4}\d{2,5}):?\s+(?:\[\*\]\s+)?(?P<m>.*)$"));
static UNIX_TAIL: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?P<m>.*?)\s+\[(?P<sev>Error|Warning)/(?P<rule>[^\]]+)\]$"));
static BRACKET_TAIL: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?P<m>.*?)\s+\[(?P<rule>[a-z][\w.-]*)\]$"));
static PAREN_TAIL: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?P<m>.*?)\s+\((?P<rule>[a-z][\w-]*)\)$"));
static RUST_HEAD: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?P<sev>error|warning)(?:\[(?P<code>[^\]]+)\])?: (?P<m>.+)$"));
static RUST_AT: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*--> (?P<path>[^:\s]+):(?P<line>\d+):(?P<col>\d+)"));
static OX_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^\s*(?P<g>[x×!⚠])\s+(?:(?P<tool>[\w@/-]+)\((?P<rule>[\w@/.-]+)\):?\s*)?(?P<m>\S.*)$")
});
static OX_AT: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:,-+|╭─+)\[(?P<path>[^\]]+?):(?P<line>\d+):(?P<col>\d+)\]"));
static CLIPPY_RULE: LazyLock<Regex> = LazyLock::new(|| re(r"clippy::[a-z_]+"));
static STYLISH_PATH: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^(?P<path>[^\s:]+{EXT})$")));
static STYLISH_ROW: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^\s+(?P<line>\d+):(?P<col>\d+)\s+(?P<sev>error|warning)\s+(?P<m>.+?)\s{2,}(?P<rule>[\w@/.-]+)\s*$",
    )
});

/// The problems in output's lines. known: a checker ran, so a bare
/// `path:line: message` is one too; otherwise it needs a severity or a
/// rule to be told from a search's hit.
pub fn diagnostics(lines: &[String], known: bool) -> Vec<Diag> {
    let mut out: Vec<Diag> = Vec::new();
    let n = lines.len();
    let mut i = 0;
    while i < n {
        let l = lines[i].trim_end();
        if let Some(d) = long_form(lines, i) {
            i = d.to;
            out.push(d);
            continue;
        }
        if let Some((rows, to)) = stylish(lines, i) {
            out.extend(rows);
            i = to;
            continue;
        }
        if let Some(m) = PYTEST_FAILED.captures(l) {
            let msg = m.name("msg").map_or("", |x| x.as_str()).to_string();
            out.push(Diag {
                path: m["path"].to_string(),
                line: 0,
                col: 0,
                sev: Sev::Error,
                rule: m["name"].to_string(),
                full: vec![l.to_string()],
                msg,
                from: i,
                to: i + 1,
            });
            i += 1;
            continue;
        }
        if let Some(mut d) = short_form(l, known) {
            d.from = i;
            let mut j = i + 1;
            // Lines it carries on to, indented under it.
            while j < n
                && lines[j].starts_with([' ', '\t'])
                && !lines[j].trim().is_empty()
                && short_form(&lines[j], known).is_none()
                && long_form(lines, j).is_none()
            {
                d.full.push(lines[j].trim_end().to_string());
                j += 1;
            }
            d.to = j;
            out.push(d);
            i = j;
            continue;
        }
        // A note on the problem before it (mypy's).
        i += 1;
    }
    out
}

fn short_form(l: &str, known: bool) -> Option<Diag> {
    let m = SHORT.captures(l)?;
    let path = m["path"].to_string();
    if path.contains("://") {
        return None;
    }
    let num = |a: &str, b: &str| -> usize {
        m.name(a)
            .or_else(|| m.name(b))
            .and_then(|x| x.as_str().parse().ok())
            .unwrap_or(0)
    };
    let (line, col) = (num("line", "l2"), num("col", "c2"));
    let mut rest = m["rest"].trim().trim_start_matches("- ").to_string();
    let mut sev: Option<Sev> = None;
    let mut rule = String::new();
    if let Some(s) = SEV.captures(&rest) {
        let w = s["sev"].to_ascii_lowercase();
        if matches!(w.as_str(), "note" | "info" | "hint") {
            return None;
        }
        sev = Some(if w.starts_with("warn") {
            Sev::Warning
        } else {
            Sev::Error
        });
        if let Some(c) = s.name("code") {
            rule = c.as_str().to_string();
        }
        rest = s["r"].to_string();
    }
    if rule.is_empty() {
        if let Some(t) = TOOL_RULE.captures(&rest) {
            rule = t["rule"].to_string();
            rest = t["m"].to_string();
        } else if let Some(c) = CODE.captures(&rest) {
            rule = c["code"].to_string();
            rest = c["m"].to_string();
        }
    }
    if rule.is_empty() {
        if let Some(t) = UNIX_TAIL.captures(&rest) {
            rule = t["rule"].to_string();
            if let Some((_, r)) = rule.split_once('(') {
                rule = r.trim_end_matches(')').to_string();
            }
            sev = Some(if &t["sev"] == "Warning" {
                Sev::Warning
            } else {
                Sev::Error
            });
            rest = t["m"].to_string();
        } else if let Some(t) = BRACKET_TAIL.captures(&rest)
            && (sev.is_some() || known)
        {
            rule = t["rule"].to_string();
            rest = t["m"].to_string();
        } else if let Some(t) = PAREN_TAIL.captures(&rest)
            && known
        {
            rule = t["rule"].to_string();
            rest = t["m"].to_string();
        }
    }
    if sev.is_none() && rule.is_empty() && !known {
        return None;
    }
    // A search's hit names a line of code, not a problem in it.
    if sev.is_none() && rule.is_empty() && (line == 0 || rest.starts_with(['{', '}', '/', '#'])) {
        return None;
    }
    Some(Diag {
        path,
        line,
        col,
        sev: sev.unwrap_or(Sev::Error),
        rule,
        msg: rest,
        full: vec![l.trim_end().to_string()],
        from: 0,
        to: 0,
    })
}

/// rustc's and clippy's long form, and oxlint's: a heading, then where.
fn long_form(lines: &[String], i: usize) -> Option<Diag> {
    let l = lines[i].trim_end();
    let (sev, mut rule, msg, at): (Sev, String, String, &Regex) =
        if let Some(m) = RUST_HEAD.captures(l) {
            let sev = if &m["sev"] == "warning" {
                Sev::Warning
            } else {
                Sev::Error
            };
            (
                sev,
                m.name("code").map_or("", |x| x.as_str()).to_string(),
                m["m"].to_string(),
                &*RUST_AT,
            )
        } else {
            let m = OX_HEAD.captures(l)?;
            let sev = if matches!(&m["g"], "x" | "×") {
                Sev::Error
            } else {
                Sev::Warning
            };
            (
                sev,
                m.name("rule").map_or("", |x| x.as_str()).to_string(),
                m["m"].to_string(),
                &*OX_AT,
            )
        };
    let mut found = None;
    for (k, x) in lines.iter().enumerate().skip(i + 1).take(3) {
        if let Some(m) = at.captures(x) {
            found = Some((
                k,
                m["path"].to_string(),
                m["line"].parse().unwrap_or(0),
                m["col"].parse().unwrap_or(0),
            ));
            break;
        }
    }
    let (k, path, line, col) = found?;
    let mut to = k + 1;
    while to < lines.len() && !lines[to].trim().is_empty() {
        if RUST_HEAD.is_match(lines[to].trim_end()) || OX_HEAD.is_match(&lines[to]) {
            break;
        }
        to += 1;
    }
    let full: Vec<String> = lines[i..to]
        .iter()
        .map(|x| x.trim_end().to_string())
        .collect();
    if rule.is_empty()
        && let Some(c) = full.iter().find_map(|x| CLIPPY_RULE.find(x))
    {
        rule = c.as_str().to_string();
    }
    Some(Diag {
        path,
        line,
        col,
        sev,
        rule,
        msg,
        full,
        from: i,
        to,
    })
}

/// eslint's stylish form: a file alone on a line, its problems under it.
fn stylish(lines: &[String], i: usize) -> Option<(Vec<Diag>, usize)> {
    let path = STYLISH_PATH.captures(lines[i].trim_end())?["path"].to_string();
    let mut out = Vec::new();
    let mut j = i + 1;
    while j < lines.len() {
        let Some(m) = STYLISH_ROW.captures(&lines[j]) else {
            break;
        };
        out.push(Diag {
            path: path.clone(),
            line: m["line"].parse().unwrap_or(0),
            col: m["col"].parse().unwrap_or(0),
            sev: if &m["sev"] == "warning" {
                Sev::Warning
            } else {
                Sev::Error
            },
            rule: m["rule"].to_string(),
            msg: m["m"].to_string(),
            full: vec![lines[j].trim().to_string()],
            from: if out.is_empty() { i } else { j },
            to: j + 1,
        });
        j += 1;
    }
    if out.is_empty() {
        return None;
    }
    Some((out, j))
}

/// Each path cut to its shortest tail no other path shares.
pub fn short_tails(paths: &[&str]) -> Vec<String> {
    let uniq: Vec<&str> = {
        let mut seen = HashSet::new();
        paths.iter().copied().filter(|p| seen.insert(*p)).collect()
    };
    paths
        .iter()
        .map(|p| {
            let parts: Vec<&str> = p.split('/').filter(|x| !x.is_empty()).collect();
            for k in 1..=parts.len() {
                let tail = parts[parts.len() - k..].join("/");
                let clash = uniq
                    .iter()
                    .any(|q| q != p && (*q == tail || q.ends_with(&format!("/{tail}"))));
                if !clash {
                    return tail;
                }
            }
            p.to_string()
        })
        .collect()
}

// ---- how each tool came out ----

/// What a tool of a command is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Fmt,
    Lint,
    Tests,
    Build,
}

/// How one tool came out: its name and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Came {
    pub tool: String,
    pub what: String,
    pub bad: bool,
    pub warn: bool,
}

/// The checking tools a command runs, in order, by name and family.
pub fn tools(cmd: &str) -> Vec<(String, Family)> {
    let mut out: Vec<(String, Family)> = Vec::new();
    for s in segments(cmd) {
        let ws = fields_of(&s.text);
        let mut ws: Vec<&str> = ws
            .iter()
            .map(|w| w.trim_matches(['(', ')', '"', '\'']))
            .filter(|w| !w.is_empty())
            .collect();
        while ws
            .first()
            .is_some_and(|w| is_assign(w) || *w == "env" || *w == "time")
        {
            ws.remove(0);
        }
        // Runners: pnpm exec x, npx x, uv run x, python -m x.
        loop {
            let a = ws.first().copied().unwrap_or("");
            let b = ws.get(1).copied().unwrap_or("");
            let skip = match (a, b) {
                ("pnpm" | "npm" | "yarn" | "bun", "exec" | "x" | "dlx") => 2,
                ("uv" | "poetry" | "pipenv" | "hatch" | "rye", "run") => 2,
                ("python" | "python3", "-m") => 2,
                ("npx" | "bunx" | "pnpx", _) => 1,
                _ => 0,
            };
            if skip == 0 {
                break;
            }
            ws.drain(..skip.min(ws.len()));
            while ws.first().is_some_and(|w| w.starts_with('-')) {
                ws.remove(0);
            }
        }
        let Some(first) = ws.first() else { continue };
        let base = first.rsplit('/').next().unwrap_or(first);
        let next = ws[1..]
            .iter()
            .find(|w| !w.starts_with(['-', '+']))
            .copied()
            .unwrap_or("");
        let found: Option<(String, Family)> = match base {
            "cargo" => match next {
                "test" | "t" | "nextest" => Some(("cargo test".into(), Family::Tests)),
                "clippy" => Some(("clippy".into(), Family::Lint)),
                "fmt" => Some(("cargo fmt".into(), Family::Fmt)),
                "build" | "b" => Some(("cargo build".into(), Family::Build)),
                "check" | "c" => Some(("cargo check".into(), Family::Build)),
                _ => None,
            },
            "go" => match next {
                "test" => Some(("go test".into(), Family::Tests)),
                "vet" => Some(("go vet".into(), Family::Lint)),
                "build" => Some(("go build".into(), Family::Build)),
                _ => None,
            },
            "ruff" if next == "format" => Some(("ruff format".into(), Family::Fmt)),
            "biome" if next == "format" => Some(("biome".into(), Family::Fmt)),
            "oxfmt" | "prettier" | "gofmt" | "rustfmt" | "black" | "dprint" => {
                Some((base.into(), Family::Fmt))
            }
            "oxlint" | "eslint" | "ruff" | "flake8" | "pylint" | "golangci-lint" | "shellcheck"
            | "biome" | "stylelint" | "tsc" | "vue-tsc" | "mypy" | "pyright" => {
                Some((base.into(), Family::Lint))
            }
            "pytest" | "jest" | "vitest" | "mocha" => Some((base.into(), Family::Tests)),
            _ => None,
        };
        if let Some(f) = found
            && !out.contains(&f)
        {
            out.push(f);
        }
    }
    out
}

fn sum(r: &Regex, s: &str, k: &str) -> Option<usize> {
    let mut any = false;
    let mut n = 0;
    for c in r.captures_iter(s) {
        any = true;
        n += c
            .name(k)
            .and_then(|x| x.as_str().parse::<usize>().ok())
            .unwrap_or(0);
    }
    any.then_some(n)
}

fn last_num(r: &Regex, s: &str, k: &str) -> Option<usize> {
    r.captures_iter(s)
        .last()
        .and_then(|c| c.name(k).and_then(|x| x.as_str().parse().ok()))
}

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// How the tests a step ran came out: passed, failed.
fn tests_of(tool: &str, out: &str) -> Option<(usize, usize)> {
    static CARGO: LazyLock<Regex> =
        LazyLock::new(|| re(r"test result: \w+\. (?P<p>\d+) passed; (?P<f>\d+) failed"));
    static NEXTEST: LazyLock<Regex> = LazyLock::new(|| {
        re(r"Summary \[[^\]]*\]\s+\d+ tests? run: (?P<p>\d+) passed(?:[^,\n]*, (?P<f>\d+) failed)?")
    });
    static PYTEST: LazyLock<Regex> =
        LazyLock::new(|| re(r"(?m)^=+ (?P<s>.*\d+ (?:passed|failed).*) in [\d.]+s.*=+$"));
    static PY_F: LazyLock<Regex> = LazyLock::new(|| re(r"(?P<n>\d+) failed"));
    static PY_P: LazyLock<Regex> = LazyLock::new(|| re(r"(?P<n>\d+) passed"));
    static VITEST: LazyLock<Regex> = LazyLock::new(|| {
        re(r"Tests?\s+(?:(?P<f>\d+) failed\s*\|\s*)?(?:\d+ skipped\s*\|\s*)?(?P<p>\d+) passed")
    });
    static JEST: LazyLock<Regex> = LazyLock::new(|| {
        re(r"Tests:\s+(?:(?P<f>\d+) failed, )?(?:\d+ skipped, )?(?:\d+ todo, )?(?P<p>\d+) passed")
    });
    static GO_FAIL: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*--- FAIL"));
    static GO_PASS: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*--- PASS"));
    static GO_OK: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^ok\s+\S+"));
    static GO_PKG_FAIL: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^FAIL\s+\S+"));
    match tool {
        "cargo test" => {
            if let Some(m) = NEXTEST.captures(out) {
                let f = m.name("f").map_or(0, |x| x.as_str().parse().unwrap_or(0));
                return Some((m["p"].parse().unwrap_or(0), f));
            }
            Some((sum(&CARGO, out, "p")?, sum(&CARGO, out, "f").unwrap_or(0)))
        }
        "pytest" => {
            let m = PYTEST.captures_iter(out).last()?;
            let s = &m["s"];
            Some((
                last_num(&PY_P, s, "n").unwrap_or(0),
                last_num(&PY_F, s, "n").unwrap_or(0),
            ))
        }
        "jest" | "vitest" | "mocha" => {
            let m = JEST
                .captures_iter(out)
                .last()
                .or_else(|| VITEST.captures_iter(out).last())?;
            let f = m.name("f").map_or(0, |x| x.as_str().parse().unwrap_or(0));
            Some((m["p"].parse().unwrap_or(0), f))
        }
        "go test" => {
            let f = GO_FAIL.find_iter(out).count();
            let p = GO_PASS.find_iter(out).count();
            if f + p > 0 {
                return Some((p, f));
            }
            let (ok, bad) = (
                GO_OK.find_iter(out).count(),
                GO_PKG_FAIL.find_iter(out).count(),
            );
            (ok + bad > 0).then_some((0, bad))
        }
        _ => None,
    }
}

/// Errors and warnings a tool's own last words count.
fn counted(tool: &str, out: &str) -> Option<(usize, usize)> {
    static OX: LazyLock<Regex> =
        LazyLock::new(|| re(r"Found (?P<w>\d+) warnings? and (?P<e>\d+) errors?"));
    static ESLINT: LazyLock<Regex> =
        LazyLock::new(|| re(r"✖ \d+ problems? \((?P<e>\d+) errors?, (?P<w>\d+) warnings?\)"));
    static FOUND: LazyLock<Regex> = LazyLock::new(|| re(r"Found (?P<e>\d+) errors?"));
    static RUST_E: LazyLock<Regex> = LazyLock::new(|| re(r"due to (?P<e>\d+) previous errors?"));
    static RUST_W: LazyLock<Regex> = LazyLock::new(|| re(r"generated (?P<w>\d+) warnings?"));
    let clean = out.contains("All checks passed!")
        || out.contains("Success: no issues found")
        || out.contains("0 errors, 0 warnings");
    match tool {
        "oxlint" | "biome" => OX
            .captures_iter(out)
            .last()
            .map(|m| (m["e"].parse().unwrap_or(0), m["w"].parse().unwrap_or(0))),
        "eslint" => ESLINT
            .captures_iter(out)
            .last()
            .map(|m| (m["e"].parse().unwrap_or(0), m["w"].parse().unwrap_or(0))),
        "ruff" | "mypy" | "tsc" | "vue-tsc" | "pyright" => {
            if clean {
                return Some((0, 0));
            }
            last_num(&FOUND, out, "e").map(|e| (e, 0))
        }
        "clippy" | "cargo build" | "cargo check" => {
            let e = sum(&RUST_E, out, "e");
            let w = sum(&RUST_W, out, "w");
            (e.is_some() || w.is_some()).then(|| (e.unwrap_or(0), w.unwrap_or(0)))
        }
        _ => None,
    }
}

/// Whether a formatter said all was well, or how many files it would
/// change.
fn formatted(tool: &str, out: &str) -> Option<usize> {
    static OXFMT: LazyLock<Regex> =
        LazyLock::new(|| re(r"(?:Format|Code style) issues found in (?P<n>\d+)"));
    static BLACK: LazyLock<Regex> = LazyLock::new(|| re(r"(?P<n>\d+) files? would be reformatted"));
    static DIFF_IN: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^Diff in (?P<p>\S+)"));
    match tool {
        "oxfmt" | "prettier" | "biome" | "dprint" => {
            if out.contains("All matched files use")
                || out.contains("No fixes applied")
                || out.contains("No fixes needed")
            {
                return Some(0);
            }
            if out.contains("Code style issues found in the above file") {
                return Some(1);
            }
            last_num(&OXFMT, out, "n")
        }
        "black" | "ruff format" => {
            if let Some(n) = last_num(&BLACK, out, "n") {
                return Some(n);
            }
            out.contains("would be left unchanged").then_some(0)
        }
        "cargo fmt" | "rustfmt" => {
            let n: HashSet<&str> = DIFF_IN
                .captures_iter(out)
                .map(|m| m.name("p").unwrap().as_str())
                .collect();
            (!n.is_empty()).then_some(n.len())
        }
        _ => None,
    }
}

/// Whether problem d is tool's, of the checkers lints the command ran.
fn owns(tool: &str, fam: Family, lints: &[&String], d: &Diag) -> bool {
    if lints.len() <= 1 {
        return fam == Family::Lint || lints.is_empty();
    }
    let ts_code = d.rule.starts_with("TS");
    let rusty = d.rule.starts_with("clippy::")
        || d.rule.len() > 1
            && d.rule.starts_with('E')
            && d.rule[1..].chars().all(|c| c.is_ascii_digit());
    match tool {
        "tsc" | "vue-tsc" => ts_code,
        "clippy" => rusty,
        _ => {
            let first = lints
                .iter()
                .find(|t| !matches!(t.as_str(), "tsc" | "vue-tsc" | "clippy"));
            !ts_code && !rusty && first.is_some_and(|t| t.as_str() == tool)
        }
    }
}

/// How each checking tool of cmd came out, read from what it printed;
/// empty when none can be told. ok: the command as a whole succeeded.
pub fn outcome(cmd: &str, out: &str, diags: &[Diag], ok: bool) -> Vec<Came> {
    let ts = tools(cmd);
    if ts.is_empty() {
        return Vec::new();
    }
    let lints: Vec<&String> = ts
        .iter()
        .filter(|(_, f)| *f == Family::Lint)
        .map(|(t, _)| t)
        .collect();
    let mut res = Vec::new();
    for (k, (tool, fam)) in ts.iter().enumerate() {
        let last = k + 1 == ts.len();
        let came = |what: String, bad: bool, warn: bool| Came {
            tool: tool.clone(),
            what,
            bad,
            warn,
        };
        match fam {
            Family::Fmt => match formatted(tool, out) {
                Some(0) => res.push(came("✓".into(), false, false)),
                Some(n) => res.push(came(
                    format!("{} unformatted", plural(n, "file")),
                    true,
                    false,
                )),
                None if ok => res.push(came("✓".into(), false, false)),
                None => {}
            },
            Family::Tests => match tests_of(tool, out) {
                Some((p, f)) if f > 0 && p > 0 => {
                    res.push(came(format!("{p} passed, {f} failed"), true, false));
                }
                Some((_, f)) if f > 0 => res.push(came(format!("{f} failed"), true, false)),
                Some((p, _)) if p > 0 => res.push(came(format!("{p} passed"), false, false)),
                Some(_) if ok => res.push(came("✓".into(), false, false)),
                _ if ok && last => res.push(came("✓".into(), false, false)),
                _ => {}
            },
            Family::Lint | Family::Build => {
                // The problems are this tool's when it's the only checker
                // that reports any, or the kind it prints says so.
                let mine: Vec<&Diag> = diags
                    .iter()
                    .filter(|d| owns(tool, *fam, &lints, d))
                    .collect();
                let said = counted(tool, out);
                let (mut e, mut w) = (
                    mine.iter().filter(|d| d.sev == Sev::Error).count(),
                    mine.iter().filter(|d| d.sev == Sev::Warning).count(),
                );
                if let Some((ce, cw)) = said {
                    e = e.max(ce);
                    w = w.max(cw);
                }
                let files: HashSet<&str> = mine.iter().map(|d| d.path.as_str()).collect();
                let in_files = if files.len() > 1 && e + w > 1 {
                    format!(" in {}", plural(files.len(), "file"))
                } else {
                    String::new()
                };
                if e > 0 {
                    let mut s = plural(e, "error");
                    if w > 0 {
                        s = format!("{s}, {}", plural(w, "warning"));
                    }
                    res.push(came(format!("{s}{in_files}"), true, false));
                } else if w > 0 {
                    let mut s = format!("{}{in_files}", plural(w, "warning"));
                    if *fam == Family::Build && ok {
                        s = format!("built, {s}");
                    }
                    res.push(came(s, false, true));
                } else if ok || said.is_some() {
                    let what = if *fam == Family::Build {
                        "built"
                    } else {
                        "✓"
                    };
                    res.push(came(what.into(), false, false));
                }
            }
        }
    }
    // One tool that only passed says nothing its command doesn't: the
    // head stays the command ("✓ cargo test", not "✓ tests  ✓").
    if res.len() == 1 && res[0].what == "✓" {
        res.clear();
    }
    res
}

/// A tool's own lines that only count what the outcome already says.
pub fn tally_line(l: &str) -> bool {
    static TALLY: LazyLock<Regex> = LazyLock::new(|| {
        re(
            r"^\s*(Found \d+ warnings? and \d+ errors?\.?|Finished in .* with \d+ rules.*|✖ \d+ problems? \(.*\)|Found \d+ errors?( in \d+ files?)?\.?)\s*$",
        )
    });
    TALLY.is_match(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    fn rows(s: &str, known: bool) -> Vec<(String, usize, Sev, String, String)> {
        diagnostics(&lines(s), known)
            .into_iter()
            .map(|d| {
                let m = d.sentence().to_string();
                (d.path, d.line, d.sev, d.rule, m)
            })
            .collect()
    }

    type Row<'a> = (&'a str, usize, Sev, &'a str, &'a str);

    // Real tools' output, a row each problem: path, line, how bad, rule,
    // the message's first sentence.
    #[test]
    fn real_outputs_parse_to_rows() {
        use Sev::*;
        let cases: &[(&str, &str, bool, &[Row])] = &[
            (
                "oxlint unix",
                "src/a.ts:12:7: Variable 'x' is declared but never used. Remove it. [Error/eslint(no-unused-vars)]",
                false,
                &[(
                    "src/a.ts",
                    12,
                    Error,
                    "no-unused-vars",
                    "Variable 'x' is declared but never used",
                )],
            ),
            (
                "oxlint graphical",
                "  x local(condition-shape): test makes 3 calls; max 2. Split it.\n     ,-[src/shared-lock.ts:98:3]\n  97 |   it('locks', () => {\n  98 |     expect(a).toBe(1)\n     :     ^^^^^^\n     `----\n  help: one condition a test\n\n  ! eslint(no-console): Unexpected console statement.\n     ,-[src/main.ts:12:5]\n  12 |     console.log(x)\n     `----\n",
                false,
                &[
                    (
                        "src/shared-lock.ts",
                        98,
                        Error,
                        "condition-shape",
                        "test makes 3 calls; max 2",
                    ),
                    (
                        "src/main.ts",
                        12,
                        Warning,
                        "no-console",
                        "Unexpected console statement",
                    ),
                ],
            ),
            (
                "eslint stylish",
                "/w/src/a.js\n  3:10  error    'foo' is defined but never used  no-unused-vars\n  7:1   warning  Unexpected console statement     no-console\n\n✖ 2 problems (1 error, 1 warning)",
                false,
                &[
                    (
                        "/w/src/a.js",
                        3,
                        Error,
                        "no-unused-vars",
                        "'foo' is defined but never used",
                    ),
                    (
                        "/w/src/a.js",
                        7,
                        Warning,
                        "no-console",
                        "Unexpected console statement",
                    ),
                ],
            ),
            (
                "rustc short",
                "src/main.rs:4:18: error[E0308]: mismatched types\nsrc/lib.rs:9:5: warning: unused variable: `x`",
                false,
                &[
                    ("src/main.rs", 4, Error, "E0308", "mismatched types"),
                    ("src/lib.rs", 9, Warning, "", "unused variable: `x`"),
                ],
            ),
            (
                "rustc long",
                "error[E0308]: mismatched types\n --> src/main.rs:4:18\n  |\n4 |     let x: i32 = \"a\";\n  |            ---   ^^^ expected `i32`, found `&str`\n\nwarning: this `if` has identical blocks\n  --> src/lib.rs:20:5\n   |\n   = help: for further information visit https://rust-lang.github.io/rust-clippy\n   = note: `#[warn(clippy::if_same_then_else)]` on by default\n\nerror: could not compile `x` (bin \"x\") due to 1 previous error",
                false,
                &[
                    ("src/main.rs", 4, Error, "E0308", "mismatched types"),
                    (
                        "src/lib.rs",
                        20,
                        Warning,
                        "clippy::if_same_then_else",
                        "this `if` has identical blocks",
                    ),
                ],
            ),
            (
                "tsc",
                "src/app.ts(14,3): error TS2322: Type 'string' is not assignable to type 'number'.\nsrc/b.ts:3:1 - error TS2304: Cannot find name 'y'.",
                false,
                &[
                    (
                        "src/app.ts",
                        14,
                        Error,
                        "TS2322",
                        "Type 'string' is not assignable to type 'number'",
                    ),
                    ("src/b.ts", 3, Error, "TS2304", "Cannot find name 'y'"),
                ],
            ),
            (
                "go vet",
                "# example.com/x\n./main.go:9:2: fmt.Printf format %d has arg s of wrong type string",
                true,
                &[(
                    "./main.go",
                    9,
                    Error,
                    "",
                    "fmt.Printf format %d has arg s of wrong type string",
                )],
            ),
            (
                "pytest",
                "tests/test_a.py:12: AssertionError\nFAILED tests/test_a.py::test_adds - assert 3 == 4\n=== 1 failed, 9 passed in 0.12s ===",
                true,
                &[
                    ("tests/test_a.py", 12, Error, "", "AssertionError"),
                    ("tests/test_a.py", 0, Error, "test_adds", "assert 3 == 4"),
                ],
            ),
            (
                "ruff",
                "app.py:1:8: F401 [*] `os` imported but unused\nFound 1 error.",
                false,
                &[("app.py", 1, Error, "F401", "`os` imported but unused")],
            ),
            (
                "mypy",
                "a.py:3: error: Incompatible types in assignment  [assignment]\na.py:3: note: see docs\nFound 1 error in 1 file (checked 2 source files)",
                false,
                &[(
                    "a.py",
                    3,
                    Error,
                    "assignment",
                    "Incompatible types in assignment",
                )],
            ),
            (
                "a search's hits are not problems",
                "src/a.rs:12:    let x = 1;\nsrc/b.rs:3:fn main() {",
                false,
                &[],
            ),
        ];
        for (name, out, known, want) in cases {
            let got = rows(out, *known);
            let want: Vec<(String, usize, Sev, String, String)> = want
                .iter()
                .map(|(p, l, s, r, m)| (p.to_string(), *l, *s, r.to_string(), m.to_string()))
                .collect();
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn tails_are_the_shortest_unique() {
        assert_eq!(
            short_tails(&["src/a/x.ts", "src/b/x.ts", "src/c/y.ts", "src/c/y.ts"]),
            ["a/x.ts", "b/x.ts", "y.ts", "y.ts"]
        );
    }

    #[test]
    fn each_tool_says_how_it_came_out() {
        let say = |cmd: &str, out: &str, ok: bool| -> String {
            let ls = lines(out);
            let d = diagnostics(&ls, !tools(cmd).is_empty());
            outcome(cmd, out, &d, ok)
                .into_iter()
                .map(|c| format!("{} {}", c.tool, c.what))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        assert_eq!(
            say(
                "cd x && pnpm exec oxfmt --check $P | tail -2 && pnpm exec oxlint --quiet $P | tail -60",
                "All matched files use the correct format.\n  x local(a): b\n   ,-[a.ts:1:1]\n\n  x local(a): b\n   ,-[b.ts:1:1]\n\n  x local(a): b\n   ,-[b.ts:2:1]\nFound 0 warnings and 3 errors.",
                false
            ),
            "oxfmt ✓ · oxlint 3 errors in 2 files"
        );
        assert_eq!(
            say(
                "cargo test -p x",
                "test result: FAILED. 312 passed; 2 failed; 0 ignored",
                false
            ),
            "cargo test 312 passed, 2 failed"
        );
        assert_eq!(
            say("npx vitest run", " Tests  2 failed | 40 passed (42)", false),
            "vitest 40 passed, 2 failed"
        );
        assert_eq!(
            say("cargo build", "   Finished `dev` profile", true),
            "cargo build built"
        );
        assert_eq!(
            say(
                "cargo build",
                "error[E0308]: x\n --> a.rs:1:1\n\nerror[E0308]: y\n --> a.rs:2:1\n\nerror: could not compile `a` due to 2 previous errors",
                false
            ),
            "cargo build 2 errors"
        );
        assert_eq!(say("ls -la", "a\nb", true), "");
    }
}
