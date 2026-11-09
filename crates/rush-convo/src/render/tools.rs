//! Rows of their own for the tools agents run most outside a language's
//! checkers: xcodebuild, xcrun simctl, xctrace, ffmpeg. What a step's row
//! says it does ("xcodebuild test SoundingTV iOS on iPhone 17 Pro",
//! "simctl screenshot shot.png"), the one line a running one shows of how
//! far it's got ("▰▰▰▱▱ 45/120 · now rush-convo", "tests 12 passed ·
//! now test03"), and the image a screenshot made. New in the Rust port:
//! from a command's words and what it printed only.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use super::shell::{Phrase, Places, fields_of, segments, unquote};

fn re(s: &str) -> Regex {
    Regex::new(s).expect("a valid pattern")
}

/// A simulator or device by how a row says it: a UUID's first block.
fn device(w: &str) -> String {
    let w = unquote(w);
    let uuid = w.len() == 36 && w.bytes().filter(|&b| b == b'-').count() == 4;
    if uuid {
        w[..8].to_string()
    } else {
        w.to_string()
    }
}

fn ph(verb: &str, objs: Vec<String>) -> Option<Phrase> {
    Some(Phrase {
        verb: verb.into(),
        objs: objs.into_iter().filter(|o| !o.is_empty()).collect(),
        kind: "",
    })
}

/// The row of a tool this module knows, from its program and words.
pub(crate) fn phrase(p: &Places, prog: &str, args: &[&str]) -> Option<Phrase> {
    match prog {
        "xcodebuild" => xcodebuild(args),
        "xcrun" => {
            let mut i = 0;
            while i < args.len() && args[i].starts_with('-') {
                // --sdk iphoneos, --toolchain x: their value goes too.
                i += if matches!(args[i], "--sdk" | "-sdk" | "--toolchain" | "-toolchain") {
                    2
                } else {
                    1
                };
            }
            let rest = args.get(i..).unwrap_or(&[]);
            let (tool, rest) = rest.split_first()?;
            phrase(p, tool, rest)
        }
        "simctl" => simctl(p, args),
        "xctrace" => xctrace(p, args),
        "ffmpeg" => ffmpeg(p, args),
        _ => None,
    }
}

/// xcodebuild's action ("build" when it names none), or what it lists.
pub(crate) fn xcode_action(args: &[&str]) -> &'static str {
    for a in args {
        match *a {
            "-list" => return "list",
            "-showdestinations" => return "destinations",
            "-showBuildSettings" => return "settings",
            "-showsdks" | "-version" => return "version",
            _ => {}
        }
    }
    for a in args {
        match *a {
            "test" => return "test",
            "test-without-building" => return "test-without-building",
            "build-for-testing" => return "build-for-testing",
            "archive" => return "archive",
            "analyze" => return "analyze",
            "clean" => return "clean",
            "build" => return "build",
            _ => {}
        }
    }
    "build"
}

/// Where -destination runs it: a device's name, else its platform
/// ("iOS Simulator"), else its id's first block.
fn destination(d: &str) -> String {
    let d = unquote(d);
    let mut name = "";
    let mut platform = "";
    let mut id = "";
    for kv in d.split(',') {
        let (k, v) = kv.split_once('=').unwrap_or(("", kv));
        match k.trim() {
            "name" => name = v,
            "platform" | "generic/platform" => platform = v,
            "id" => id = v,
            _ => {}
        }
    }
    if !name.is_empty() {
        name.into()
    } else if !platform.is_empty() {
        platform.into()
    } else {
        device(id)
    }
}

fn xcodebuild(args: &[&str]) -> Option<Phrase> {
    let act = xcode_action(args);
    let (mut scheme, mut dest, mut only) = (String::new(), String::new(), Vec::new());
    let mut proj = String::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        let v = args.get(i + 1).map(|v| unquote(v)).unwrap_or("");
        match a {
            "-scheme" => scheme = v.into(),
            "-destination" if dest.is_empty() => dest = destination(v),
            "-project" | "-workspace" => {
                proj = v.rsplit('/').next().unwrap_or(v).to_string();
            }
            _ => {
                if let Some(t) = a.strip_prefix("-only-testing:") {
                    // Target/Class/test: what's in it, not the target.
                    let t = unquote(t);
                    only.push(t.split_once('/').map_or(t, |x| x.1).to_string());
                }
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    let verb = match act {
        "list" => return ph("xcodebuild -list", vec![proj]),
        "destinations" => return ph("xcodebuild destinations", vec![scheme]),
        "settings" => return ph("xcodebuild settings", vec![scheme]),
        "version" => return ph("xcodebuild -version", vec![]),
        a => format!("xcodebuild {a}"),
    };
    let mut what = if only.is_empty() {
        scheme
    } else {
        only.join(", ")
    };
    if what.is_empty() {
        what = proj;
    }
    if !dest.is_empty() {
        what = if what.is_empty() {
            format!("on {dest}")
        } else {
            format!("{what} on {dest}")
        };
    }
    ph(&verb, vec![what])
}

fn simctl(p: &Places, args: &[&str]) -> Option<Phrase> {
    let ws: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with('-'))
        .collect();
    let (sub, rest) = ws.split_first()?;
    let verb = format!("simctl {sub}");
    let at = |i: usize| rest.get(i).map(|w| unquote(w)).unwrap_or("");
    match *sub {
        // io <device> screenshot <file>, io <device> recordVideo <file>
        "io" => {
            let what = at(1);
            let file = p.rel(at(2));
            match what {
                "screenshot" => ph("simctl screenshot", vec![file]),
                "recordVideo" => ph("simctl record", vec![file]),
                _ => ph(&format!("simctl io {what}"), vec![device(at(0))]),
            }
        }
        "install" => {
            let app = at(1).trim_end_matches('/');
            ph(
                &verb,
                vec![app.rsplit('/').next().unwrap_or(app).to_string()],
            )
        }
        "launch" | "terminate" | "uninstall" | "get_app_container" => {
            ph(&verb, vec![at(1).to_string()])
        }
        "openurl" => ph(&verb, vec![at(1).to_string()]),
        "boot" | "shutdown" | "erase" | "delete" => ph(&verb, vec![device(at(0))]),
        "create" => ph(&verb, vec![at(0).to_string()]),
        "list" => ph(&verb, vec![at(0).to_string()]),
        "privacy" => ph(
            &format!("simctl privacy {}", at(1)),
            vec![format!("{} for {}", at(2), at(3)).trim().to_string()],
        ),
        "spawn" => ph(&verb, vec![rest.get(1..).unwrap_or(&[]).join(" ")]),
        _ => ph(&verb, rest.iter().take(1).map(|w| device(w)).collect()),
    }
}

fn xctrace(p: &Places, args: &[&str]) -> Option<Phrase> {
    let (sub, rest) = args.split_first()?;
    let val = |k: &[&str]| -> String {
        rest.windows(2)
            .find(|w| k.contains(&w[0]))
            .map(|w| unquote(w[1]).to_string())
            .unwrap_or_default()
    };
    match *sub {
        "record" => {
            let tpl = val(&["--template", "-t"]);
            let of = {
                let l = val(&["--launch"]);
                if let Some(i) = rest.iter().position(|w| *w == "--") {
                    // --launch -- <program> <its args>
                    let c = rest.get(i + 1).map(|w| unquote(w)).unwrap_or("");
                    c.rsplit('/').next().unwrap_or(c).to_string()
                } else if !l.is_empty() {
                    l.rsplit('/').next().unwrap_or(&l).to_string()
                } else {
                    let a = val(&["--attach"]);
                    if a.is_empty() { a } else { format!("pid {a}") }
                }
            };
            let what = match (tpl.is_empty(), of.is_empty()) {
                (false, false) => format!("{tpl} of {of}"),
                (false, true) => tpl,
                (true, _) => of,
            };
            ph("xctrace record", vec![what])
        }
        "export" => ph("xctrace export", vec![p.rel(&val(&["--input"]))]),
        "list" => ph(
            "xctrace list",
            vec![rest.first().copied().unwrap_or("").into()],
        ),
        s => ph(&format!("xctrace {s}"), vec![]),
    }
}

/// ffmpeg's flags that take no value.
fn ff_bare(a: &str) -> bool {
    matches!(
        a,
        "-y" | "-n"
            | "-hide_banner"
            | "-nostdin"
            | "-an"
            | "-vn"
            | "-sn"
            | "-dn"
            | "-stats"
            | "-nostats"
            | "-shortest"
            | "-re"
    )
}

fn ffmpeg(p: &Places, args: &[&str]) -> Option<Phrase> {
    let (mut ins, mut out) = (Vec::new(), String::new());
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "-i" {
            if let Some(v) = args.get(i + 1) {
                ins.push(p.rel(unquote(v)));
            }
            i += 2;
        } else if a.starts_with('-') && !ff_bare(a) {
            i += 2;
        } else if a.starts_with('-') {
            i += 1;
        } else {
            out = p.rel(unquote(a));
            i += 1;
        }
    }
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    let what = match (ins.first(), out.is_empty()) {
        (Some(i), false) => format!("{} → {}", base(i), base(&out)),
        (Some(i), true) => base(i),
        (None, false) => base(&out),
        (None, true) => String::new(),
    };
    ph("ffmpeg", vec![what])
}

// ---- how far a running one has got ----

/// How far a running command has got: what it says, and how much of
/// what it does is done (n of m) when it knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Progress {
    pub text: String,
    pub done: Option<(usize, usize)>,
}

/// The programs of cmd's parts, xcrun's tool for xcrun.
fn programs(cmd: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for s in segments(cmd) {
        let ws = fields_of(&s.text);
        let mut ws: Vec<&str> = ws.iter().map(String::as_str).collect();
        while ws
            .first()
            .is_some_and(|w| w.contains('=') || matches!(*w, "env" | "time" | "xcrun"))
        {
            ws.remove(0);
        }
        if let Some(first) = ws.first() {
            let base = first.rsplit('/').next().unwrap_or(first).to_string();
            let sub = ws[1..]
                .iter()
                .find(|w| !w.starts_with(['-', '+']))
                .copied()
                .unwrap_or("")
                .to_string();
            out.push((base, sub));
        }
    }
    out
}

static XC_COMPILE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^(?:SwiftCompile|CompileSwift|CompileC|CompileXIB|CompileStoryboard|CompileAssetCatalog\w*)\s",
    )
});
static XC_CASE_DONE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^Test [Cc]ase '(?P<n>[^']+)' (?P<r>passed|failed|skipped)"));
static XC_CASE_START: LazyLock<Regex> =
    LazyLock::new(|| re(r"^Test [Cc]ase '(?P<n>[^']+)' started"));
static ST_DONE: LazyLock<Regex> =
    LazyLock::new(|| re(r#"^\s*[✔✘􁁛􀢄]\s+Test "?(?P<n>[^"(]+?)"?(?:\(\))? (?P<r>passed|failed)"#));
static ST_START: LazyLock<Regex> =
    LazyLock::new(|| re(r#"^\s*[◇􀟈]\s+Test "?(?P<n>[^"(]+?)"?(?:\(\))? started"#));
static RUST_TEST: LazyLock<Regex> =
    LazyLock::new(|| re(r"^test (?P<n>\S+) \.\.\. (?P<r>ok|FAILED|ignored)"));
static CARGO_BAR: LazyLock<Regex> =
    LazyLock::new(|| re(r"Building \[[^\]]*\]\s+(?P<n>\d+)/(?P<m>\d+)(?::\s*(?P<c>[^,\s(]+))?"));
static CARGO_UNIT: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:Compiling|Checking|Documenting)\s+(?P<c>\S+)"));
static FF_TIME: LazyLock<Regex> = LazyLock::new(|| re(r"time=\s*(?P<t>\d+:\d\d:\d\d(?:\.\d+)?)"));
static FF_SPEED: LazyLock<Regex> = LazyLock::new(|| re(r"speed=\s*(?P<s>[\d.]+x)"));
static FF_DUR: LazyLock<Regex> =
    LazyLock::new(|| re(r"Duration:\s*(?P<t>\d+:\d\d:\d\d(?:\.\d+)?)"));

fn secs(t: &str) -> f64 {
    let mut s = 0.0;
    for p in t.split(':') {
        s = s * 60.0 + p.parse::<f64>().unwrap_or(0.0);
    }
    s
}

fn clock(t: &str) -> &str {
    t.split('.').next().unwrap_or(t)
}

/// What a test's own name is: a test method's, not its class's.
fn test_name(n: &str) -> String {
    let n = n.trim_start_matches("-[").trim_end_matches(']');
    let n = n.rsplit([' ', '.']).next().unwrap_or(n);
    n.trim_end_matches("()").to_string()
}

fn tests_text(pass: usize, fail: usize, skip: usize, now: &str) -> String {
    let mut s = format!("tests {pass} passed");
    if fail > 0 {
        s.push_str(&format!(", {fail} failed"));
    }
    if skip > 0 {
        s.push_str(&format!(", {skip} skipped"));
    }
    if !now.is_empty() {
        s.push_str(&format!(" · now {now}"));
    }
    s
}

/// How far a running cmd has got from the lines it has printed; None
/// when it isn't one this knows or nothing says yet.
pub(crate) fn progress(cmd: &str, lines: &[&str]) -> Option<Progress> {
    let progs = programs(cmd);
    let has = |p: &str| progs.iter().any(|(b, _)| b == p);
    if has("ffmpeg") {
        let dur = lines.iter().find_map(|l| FF_DUR.captures(l));
        let at = lines.iter().rev().find_map(|l| {
            FF_TIME.captures(l).map(|m| {
                (
                    m["t"].to_string(),
                    FF_SPEED.captures(l).map(|s| s["s"].to_string()),
                )
            })
        });
        let (t, speed) = at?;
        let mut text = clock(&t).to_string();
        let mut done = None;
        if let Some(d) = dur {
            text = format!("{text} of {}", clock(&d["t"]));
            let (a, b) = (secs(&t), secs(&d["t"]));
            if b > 0.0 {
                done = Some(((a * 10.0) as usize, (b * 10.0) as usize));
            }
        }
        if let Some(s) = speed {
            text = format!("{text} · {s}");
        }
        return Some(Progress { text, done });
    }
    if has("xcodebuild") {
        let (mut files, mut pass, mut fail, mut skip) = (0, 0, 0, 0);
        let mut now = String::new();
        let mut errors: HashSet<&str> = HashSet::new();
        for l in lines {
            let t = l.trim_start();
            if XC_COMPILE.is_match(t) {
                files += 1;
            } else if let Some(m) = XC_CASE_DONE.captures(t).or_else(|| ST_DONE.captures(t)) {
                match &m["r"] {
                    "passed" => pass += 1,
                    "failed" => fail += 1,
                    _ => skip += 1,
                }
                now.clear();
            } else if let Some(m) = XC_CASE_START.captures(t).or_else(|| ST_START.captures(t)) {
                now = test_name(&m["n"]);
            } else if l.contains(": error: ") {
                errors.insert(l);
            }
        }
        let mut text = if pass + fail + skip > 0 || !now.is_empty() {
            tests_text(pass, fail, skip, &now)
        } else if files > 0 {
            format!(
                "compiled {files} {}",
                if files == 1 { "file" } else { "files" }
            )
        } else {
            return None;
        };
        if !errors.is_empty() {
            let n = errors.len();
            text.push_str(&format!(
                " · {n} {}",
                if n == 1 { "error" } else { "errors" }
            ));
        }
        return Some(Progress { text, done: None });
    }
    if progs.iter().any(|(b, _)| b == "cargo") {
        let (mut pass, mut fail, mut skip) = (0, 0, 0);
        let (mut units, mut last) = (0, String::new());
        let mut bar: Option<(usize, usize, String)> = None;
        for l in lines {
            if let Some(m) = RUST_TEST.captures(l) {
                match &m["r"] {
                    "ok" => pass += 1,
                    "FAILED" => fail += 1,
                    _ => skip += 1,
                }
            } else if let Some(m) = CARGO_UNIT.captures(l) {
                units += 1;
                last = m["c"].to_string();
            }
            if let Some(m) = CARGO_BAR.captures(l) {
                let c = m.name("c").map_or("", |c| c.as_str()).to_string();
                bar = Some((m["n"].parse().unwrap_or(0), m["m"].parse().unwrap_or(0), c));
            }
        }
        if pass + fail + skip > 0 {
            return Some(Progress {
                text: tests_text(pass, fail, skip, ""),
                done: None,
            });
        }
        // Its bar is drawn only on a terminal; piped, each crate's line.
        if let Some((n, m, c)) = bar.filter(|b| b.1 > 0) {
            let now = if c.is_empty() { last } else { c };
            let mut text = format!("{n}/{m}");
            if !now.is_empty() {
                text.push_str(&format!(" · now {now}"));
            }
            return Some(Progress {
                text,
                done: Some((n, m)),
            });
        }
        if units > 0 {
            let noun = if units == 1 { "crate" } else { "crates" };
            return Some(Progress {
                text: format!("compiled {units} {noun} · now {last}"),
                done: None,
            });
        }
    }
    None
}

// ---- how it came out ----

/// How xcodebuild test's tests came out, passed and failed: each test's
/// own line, XCTest's and Swift Testing's, else XCTest's last count.
pub(crate) fn xcode_tests(out: &str) -> Option<(usize, usize)> {
    static EXECUTED: LazyLock<Regex> =
        LazyLock::new(|| re(r"Executed (?P<n>\d+) tests?, with (?P<f>\d+) failures?"));
    let (mut p, mut f) = (0, 0);
    for l in out.lines() {
        if let Some(m) = XC_CASE_DONE
            .captures(l.trim_start())
            .or_else(|| ST_DONE.captures(l))
        {
            match &m["r"] {
                "passed" => p += 1,
                "failed" => f += 1,
                _ => {}
            }
        }
    }
    if p + f > 0 {
        return Some((p, f));
    }
    let m = EXECUTED.captures_iter(out).last()?;
    let (n, f): (usize, usize) = (m["n"].parse().ok()?, m["f"].parse().ok()?);
    Some((n.saturating_sub(f), f))
}

/// The errors and warnings xcodebuild printed, each said once; a build it
/// says failed has one at least (a linker's, a scheme that isn't there).
/// None when it said nothing of how it went.
pub(crate) fn xcode_counted(out: &str) -> Option<(usize, usize)> {
    let mut es: HashSet<&str> = HashSet::new();
    let mut ws: HashSet<&str> = HashSet::new();
    let mut said = false;
    for l in out.lines() {
        let l = l.trim_end();
        if l.starts_with("** ") && l.ends_with(" **") {
            said = true;
        }
        // Not the compiler's excerpt of the line, indented under it.
        if l.starts_with([' ', '\t', '|']) {
            continue;
        }
        if l.starts_with("error: ") || l.contains(": error: ") {
            es.insert(l);
        } else if l.starts_with("warning: ") || l.contains(": warning: ") {
            ws.insert(l);
        }
    }
    let failed = out.contains(" FAILED **");
    if !said && es.is_empty() && ws.is_empty() {
        return None;
    }
    let e = if failed { es.len().max(1) } else { es.len() };
    Some((e, ws.len()))
}

// ---- what it made ----

/// The image a command saves, when it saves one a row can show: xcrun
/// simctl io <device> screenshot <file>, screencapture <file>.
pub(crate) fn shot_path(cmd: &str) -> Option<String> {
    for s in segments(cmd) {
        let ws = fields_of(&s.text);
        let ws: Vec<&str> = ws.iter().map(String::as_str).collect();
        let at = ws.iter().position(|w| *w == "screenshot");
        let path = if let Some(i) = at.filter(|_| ws.contains(&"simctl")) {
            ws[i + 1..].iter().find(|w| !w.starts_with('-')).copied()
        } else if ws.first().is_some_and(|w| w.ends_with("screencapture")) {
            ws[1..].iter().rev().find(|w| !w.starts_with('-')).copied()
        } else {
            None
        };
        if let Some(p) = path.map(unquote)
            && !p.contains('$')
            && super::draw::thumbable(p)
        {
            return Some(p.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(cmd: &str) -> String {
        let p = Places::new("/w");
        let ws = fields_of(cmd);
        let ws: Vec<&str> = ws.iter().map(String::as_str).collect();
        let prog = ws[0].rsplit('/').next().unwrap();
        match phrase(&p, prog, &ws[1..]) {
            Some(ph) => {
                let mut s = ph.verb;
                for o in ph.objs {
                    s.push(' ');
                    s.push_str(&o);
                }
                s
            }
            None => "-".into(),
        }
    }

    #[test]
    fn rows_say_what_each_tool_does() {
        for (cmd, want) in [
            (
                r#"xcodebuild -project SoundingTV.xcodeproj -scheme "SoundingTV iOS" -destination 'generic/platform=iOS Simulator' build"#,
                "xcodebuild build SoundingTV iOS on iOS Simulator",
            ),
            (
                r#"xcodebuild test -project A.xcodeproj -scheme App -destination 'platform=iOS Simulator,name=iPhone 17 Pro' -only-testing:AppUITests/WalkTests/test01"#,
                "xcodebuild test WalkTests/test01 on iPhone 17 Pro",
            ),
            (
                "xcodebuild -list -project SoundingTV.xcodeproj",
                "xcodebuild -list SoundingTV.xcodeproj",
            ),
            (
                "xcodebuild -scheme App -destination 'id=641A0D41-E535-45E3-AA12-B68C6C7976C5'",
                "xcodebuild build App on 641A0D41",
            ),
            (
                "xcrun simctl io booted screenshot /w/shots/home.png",
                "simctl screenshot shots/home.png",
            ),
            (
                "xcrun simctl boot 641A0D41-E535-45E3-AA12-B68C6C7976C5",
                "simctl boot 641A0D41",
            ),
            (
                "xcrun simctl install booted /w/dd/Build/Products/Debug-iphonesimulator/App.app",
                "simctl install App.app",
            ),
            (
                "xcrun simctl launch booted red.forbes.app",
                "simctl launch red.forbes.app",
            ),
            (
                "xcrun simctl openurl booted https://example.com/a",
                "simctl openurl https://example.com/a",
            ),
            ("xcrun simctl list devices available", "simctl list devices"),
            (
                "xcrun xctrace record --template 'Time Profiler' --output /w/t.trace --launch -- /w/target/release/rush",
                "xctrace record Time Profiler of rush",
            ),
            (
                "xcrun xctrace record --template Allocations --attach 4012",
                "xctrace record Allocations of pid 4012",
            ),
            (
                "xctrace export --input /w/t.trace --xpath '/trace-toc'",
                "xctrace export t.trace",
            ),
            (
                "ffmpeg -y -loglevel error -i /w/in/rec.mov -vf fps=2,scale=480:-1 /w/out/f_%03d.jpg",
                "ffmpeg rec.mov → f_%03d.jpg",
            ),
            ("ls -la", "-"),
        ] {
            assert_eq!(said(cmd), want, "{cmd}");
        }
    }

    fn prog(cmd: &str, out: &str) -> Option<Progress> {
        let ls: Vec<&str> = out.lines().collect();
        progress(cmd, &ls)
    }

    #[test]
    fn a_running_tool_says_how_far_it_has_got() {
        let xb = "xcodebuild -scheme App build";
        let xt = "xcodebuild -scheme App test";
        for (cmd, out, want, done) in [
            (
                xb,
                "SwiftCompile normal arm64 /a/A.swift (in target 'A')\nSwiftCompile normal arm64 /a/B.swift (in target 'A')\nLd /a/A",
                "compiled 2 files",
                None,
            ),
            (
                xb,
                "SwiftCompile normal arm64 /a/A.swift\n/a/A.swift:3:5: error: nope\n/a/A.swift:3:5: error: nope",
                "compiled 1 file · 1 error",
                None,
            ),
            (
                xt,
                "Test Case '-[AppTests.ATests testOne]' started.\nTest Case '-[AppTests.ATests testOne]' passed (0.01 seconds).\nTest Case '-[AppTests.ATests testTwo]' started.",
                "tests 1 passed · now testTwo",
                None,
            ),
            (
                xt,
                "◇ Test \"adds\" started.\n✘ Test \"adds\" failed after 0.1 seconds.\n✔ Test subtracts() passed after 0.1 seconds.",
                "tests 1 passed, 1 failed",
                None,
            ),
            (
                "cargo build --release",
                "   Compiling serde v1.0.0\n   Compiling rush-core v0.1.0 (/w/crates/rush-core)",
                "compiled 2 crates · now rush-core",
                None,
            ),
            (
                "cargo build",
                "   Compiling serde v1.0.0\n    Building [=====>      ] 45/120: rush-convo, rush-hl",
                "45/120 · now rush-convo",
                Some((45, 120)),
            ),
            (
                "cargo test -p rush-convo",
                "test a ... ok\ntest b ... FAILED\ntest c ... ok",
                "tests 2 passed, 1 failed",
                None,
            ),
            (
                "ffmpeg -i in.mov out.mp4",
                "  Duration: 00:02:00.00, start: 0\nframe= 1234 fps=60 q=28.0 size= 1024kB time=00:00:30.00 bitrate=1 speed=2.1x",
                "00:00:30 of 00:02:00 · 2.1x",
                Some((300, 1200)),
            ),
        ] {
            let got = prog(cmd, out).expect(cmd);
            assert_eq!(got.text, want, "{cmd}");
            assert_eq!(got.done, done, "{cmd}");
        }
        assert_eq!(prog("ls", "a\nb"), None);
        assert_eq!(prog("cargo fmt", ""), None);
    }

    #[test]
    fn xcodebuild_says_how_it_came_out() {
        let fail = "/a/P.swift:132:18: error: cannot find 'q' in scope\n    |  `- error: cannot find 'q' in scope\n** BUILD FAILED **\n/a/P.swift:132:18: error: cannot find 'q' in scope\n/a/V.swift:3:1: warning: unused";
        assert_eq!(xcode_counted(fail), Some((1, 1)));
        assert_eq!(xcode_counted("** BUILD SUCCEEDED **"), Some((0, 0)));
        assert_eq!(
            xcode_counted("xcodebuild: error: Failed to build project\n** TEST FAILED **"),
            Some((1, 0))
        );
        assert_eq!(xcode_counted("** ARCHIVE FAILED **"), Some((1, 0)));
        assert_eq!(xcode_counted("Schemes:\n    App"), None);
        let tests = "Test Case '-[A.T testOne]' passed (0.1 seconds).\nTest Case '-[A.T testTwo]' failed (2.0 seconds).\n\u{2714} Test adds() passed after 0.1 seconds.\n** TEST FAILED **";
        assert_eq!(xcode_tests(tests), Some((2, 1)));
        assert_eq!(
            xcode_tests(
                "Executed 12 tests, with 0 failures (0 unexpected) in 1.0 seconds\nExecuted 40 tests, with 2 failures (0 unexpected) in 3.0 seconds"
            ),
            Some((38, 2))
        );
        assert_eq!(xcode_tests("** BUILD SUCCEEDED **"), None);
    }

    #[test]
    fn a_screenshot_is_the_image_it_saved() {
        for (cmd, want) in [
            (
                "xcrun simctl io booted screenshot /w/a.png",
                Some("/w/a.png"),
            ),
            (
                "xcrun simctl io booted screenshot --type=png shots/b.png && echo ok",
                Some("shots/b.png"),
            ),
            ("screencapture -x /w/c.png", Some("/w/c.png")),
            ("xcrun simctl io booted screenshot \"$OUT\"", None),
            ("xcrun simctl io booted recordVideo a.mov", None),
        ] {
            assert_eq!(shot_path(cmd).as_deref(), want, "{cmd}");
        }
    }
}
