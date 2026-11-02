//! The parts of the gate (Go `internal/gate`) the bundled plugins use:
//! its rules and settings (rules.go:1-169), where it keeps things
//! (gate.go:27-37, node.go:1-52), its node preload (node.go:54-97), and
//! how it splits a shell command into simple commands (hook.go:90-279).
//!
//! The gate itself (tickets, admission, the watchdog and the guard) is a
//! module of its own, not ported here: the bundled gate plugin starts the
//! guard, and the ts plugin takes a ticket, through [`Runtime`], which
//! the gate's port provides with [`set_runtime`]. Without one, the guard
//! doesn't run and a ts server starts ungated, as with the gate off.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rush_plugin::Ctx;

/// Set in a gated program's environment: a gate inside it lets everything
/// straight through, so nothing waits on itself.
pub const HELD_ENV: &str = "RUSH_GATE_HELD";

/// The share of memory, in percent, below which the guard stops agents'
/// programs.
pub const DEFAULT_FLOOR: i32 = 5;

const GIB: u64 = 1 << 30;

/// How one program is gated: the memory a run of it is expected to need
/// before it may start, and how long it may run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    /// bytes
    pub memory: u64,
    pub limit: Duration,
}

impl Rule {
    /// The most memory a run's processes may hold together before the
    /// watchdog stops it: three times what it was expected to need.
    pub fn cap(&self) -> u64 {
        3 * self.memory
    }
}

/// The memory a run of each program is expected to need; a program named
/// without one expects OTHER_ESTIMATE.
pub fn estimate(name: &str) -> Option<u64> {
    Some(match name {
        "tsc" | "tsgo" => 4 * GIB,
        "tsrs" => 2 * GIB,
        "vitest" | "jest" => 3 * GIB,
        "playwright" => 2 * GIB,
        "next" | "webpack" | "vite" => 3 * GIB,
        "go" | "cargo" | "rustc" => 2 * GIB,
        "golangci-lint" | "oxlint" | "eslint" | "biome" => GIB,
        _ => return None,
    })
}

/// What a program named without an estimate expects.
pub const OTHER_ESTIMATE: u64 = 2 * GIB;

/// The settings' own values, for any left unset.
pub fn default_setting(key: &str) -> &'static str {
    match key {
        "names" => {
            "tsc, tsgo, tsrs, go, cargo, rustc, webpack, vite, next, vitest, jest, playwright, \
             golangci-lint, oxlint, eslint, biome"
        }
        "limit" => "30m",
        "reserve" => "15%",
        "floor" => "5%",
        _ => "",
    }
}

/// The rule for each program from the gate's settings: names says which,
/// limit how long any may run, and limits such as "tsc=6G/45m, vitest=4G"
/// change a program's memory or time, or add it. Parts it can't read are
/// left as they were; a setting missing takes its default.
pub fn parse_rules(v: &BTreeMap<String, String>) -> HashMap<String, Rule> {
    let get = |k: &str| match v.get(k) {
        Some(s) => s.trim().to_string(),
        None => default_setting(k).to_string(),
    };
    let mut limit = Duration::from_secs(30 * 60);
    if let Ok(d) = rush_events::parse_duration(&get("limit"))
        && d > 0
    {
        limit = Duration::from_nanos(d as u64);
    }
    let rule = |name: &str| Rule {
        memory: estimate(name).unwrap_or(OTHER_ESTIMATE),
        limit,
    };
    let mut out = HashMap::new();
    for n in fields(&get("names")) {
        out.insert(n.to_string(), rule(n));
    }
    for o in fields(v.get("limits").map_or("", String::as_str)) {
        let (name, parts) = o.split_once('=').unwrap_or((o, ""));
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut r = out.get(name).copied().unwrap_or_else(|| rule(name));
        for p in parts.split('/') {
            let p = p.trim();
            match rush_events::parse_duration(p) {
                Ok(d) if d > 0 => r.limit = Duration::from_nanos(d as u64),
                _ => {
                    if let Some(b) = parse_size(p) {
                        r.memory = b;
                    }
                }
            }
        }
        out.insert(name.to_string(), r);
    }
    out
}

/// The share of the machine's memory the reserve setting keeps free of
/// gated runs: "15%", or 0.15.
pub fn parse_reserve(v: &BTreeMap<String, String>) -> f64 {
    let s = v
        .get("reserve")
        .map_or(default_setting("reserve"), String::as_str)
        .trim();
    let parsed = go_parse_float(s.strip_suffix('%').unwrap_or(s));
    let mut f = parsed.unwrap_or(0.0);
    if s.ends_with('%') || f > 1.0 {
        f /= 100.0;
    }
    if parsed.is_none() || f < 0.0 || f >= 1.0 {
        return 0.15;
    }
    f
}

/// The floor setting in percent: the memory left at which the guard stops
/// agents' programs; 0 when it's off.
pub fn parse_floor(v: &BTreeMap<String, String>) -> i32 {
    let s = v
        .get("floor")
        .map_or(default_setting("floor"), String::as_str)
        .trim();
    if s == "off" {
        return 0;
    }
    match s.strip_suffix('%').unwrap_or(s).parse::<i32>() {
        Ok(n) if n > 0 && n < 50 => n,
        _ => DEFAULT_FLOOR,
    }
}

/// A size such as 6G, 512M or 1.5GiB in bytes, counted in 1024s. A bare
/// number isn't a size.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_uppercase();
    let s = s.strip_suffix('B').unwrap_or(&s);
    let s = s.strip_suffix('I').unwrap_or(s);
    let last = s.chars().last()?;
    let mult: f64 = match last {
        'K' => (1u64 << 10) as f64,
        'M' => (1u64 << 20) as f64,
        'G' => (1u64 << 30) as f64,
        'T' => (1u64 << 40) as f64,
        _ => return None,
    };
    let f = go_parse_float(&s[..s.len() - 1])?;
    if f.is_nan() || f <= 0.0 {
        return None;
    }
    Some((f * mult) as u64)
}

/// n bytes for people: 3.1 GiB, 640 MiB.
pub fn size(n: u64) -> String {
    if n >= GIB {
        return format!("{:.1} GiB", n as f64 / GIB as f64);
    }
    format!("{} MiB", n >> 20)
}

/// A number as Go's strconv.ParseFloat reads one: Rust's reading, less
/// what Go refuses (a leading +inf spelt otherwise is the same).
fn go_parse_float(s: &str) -> Option<f64> {
    if s.is_empty() || s.contains('_') {
        return None;
    }
    s.parse::<f64>().ok()
}

fn fields(s: &str) -> impl Iterator<Item = &str> {
    s.split([',', ' ', '\t', '\n']).filter(|f| !f.is_empty())
}

/// Where the gate keeps its tickets and shims.
pub fn root() -> PathBuf {
    rush_state::dir().join("gate")
}

/// The folder of shims sessions find first on PATH.
pub fn bin_dir() -> PathBuf {
    root().join("bin")
}

/// Holds the preload and what it reads: which rush, which names.
pub fn node_dir() -> PathBuf {
    root().join("node")
}

/// The file NODE_OPTIONS requires.
pub fn preload_path() -> PathBuf {
    node_dir().join("preload.cjs")
}

/// Lists rush, then the names the gate queues, one a line; gone when the
/// gate is off, and the preload does nothing.
pub fn names_path() -> PathBuf {
    node_dir().join("names")
}

/// The gated program a node script is, by the rules: the script's own
/// name (typescript/bin/tsc is tsc, vitest/vitest.mjs is vitest), else its
/// package's, else its scope's (@playwright/test is playwright); "" for
/// none.
pub fn node_name(script: &str, rules: &HashMap<String, Rule>) -> String {
    let base = Path::new(script)
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_default();
    // filepath.Ext's suffix, trimmed.
    let base = match base.rfind('.') {
        Some(i) => base[..i].to_string(),
        None => base,
    };
    let mut cands = vec![base];
    if let Some(i) = script.rfind("/node_modules/") {
        let parts: Vec<&str> = script[i + "/node_modules/".len()..].split('/').collect();
        if parts.len() > 1 && parts[0].starts_with('@') {
            cands.push(parts[1].to_string());
            cands.push(parts[0].trim_start_matches('@').to_string());
        } else if !parts.is_empty() {
            cands.push(parts[0].to_string());
        }
    }
    cands
        .into_iter()
        .find(|c| rules.contains_key(c))
        .unwrap_or_default()
}

/// The script node loads first (node.go:54-97). It costs one read of
/// names_path in a node process that runs nothing gated, and runs rush
/// only for one that might.
pub const PRELOAD: &str = r#"'use strict';
(() => {
  const env = process.env;
  if (env.RUSH_GATE_HELD || !process.argv[1]) return;
  if (process.argv.slice(2).some((a) => /^(--watch|-w|--lsp|--stdio|--help|-h|--version)$/.test(a))) return;
  try { if (!require('worker_threads').isMainThread) return; } catch { return; }
  const fs = require('fs'), path = require('path');
  let lines;
  try { lines = fs.readFileSync(__dirname + '/names', 'utf8').split('\n').filter(Boolean); } catch { return; }
  const exe = lines.shift();
  let script = process.argv[1];
  try { script = fs.realpathSync(script); } catch {}
  const base = path.basename(script).replace(/\.[^.]*$/, '');
  const m = /\/node_modules\/((@[^/]+)\/)?([^/]+)\//.exec(script);
  const cands = [base];
  if (m) cands.push(m[3], ...(m[2] ? [m[2].slice(1)] : []));
  if (!exe || !cands.some((c) => lines.includes(c))) return;
  const os = require('os'), cp = require('child_process');
  const ready = path.join(os.tmpdir(), 'rush-gate-' + process.pid + '-' + Date.now());
  let child;
  try {
    child = cp.spawn(exe, ['gate', 'hold', '--ready', ready, '--pid', String(process.pid), '--dir', process.cwd(),
      '--', process.execPath, script, ...process.argv.slice(2)], { stdio: ['ignore', 'ignore', 'inherit'], detached: true });
    child.unref();
  } catch { return; }
  const nap = new Int32Array(new SharedArrayBuffer(4)), start = Date.now();
  for (;;) {
    let said;
    try { said = fs.readFileSync(ready, 'utf8'); } catch {}
    if (said !== undefined) {
      try { fs.unlinkSync(ready); } catch {}
      const got = said.split('\n').filter(Boolean);
      if (got[0] === 'held') for (const kv of got.slice(1)) { const i = kv.indexOf('='); if (i > 0) env[kv.slice(0, i)] = kv.slice(i + 1); }
      return;
    }
    // A holder that died unseen (its zombie still answers kill) stops beating.
    let beat = start;
    try { beat = fs.statSync(ready + '.beat').mtimeMs; } catch {}
    if (Date.now() - Math.max(beat, start) > 5000) return;
    Atomics.wait(nap, 0, 0, 100);
  }
})();
"#;

/// s as one word to sh.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A here-document's end: its line, and whether its lines may start with
/// tabs (<<-).
struct HereDocEnd {
    word: Vec<u8>,
    tabs: bool,
}

/// The words of each simple command in s, roughly as sh splits them; None
/// when a quote isn't closed. hook.go:279 Commands.
pub fn commands(s: &str) -> Option<Vec<Vec<String>>> {
    let b = s.as_bytes();
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut w: Vec<u8> = Vec::new();
    let mut inw = false;
    let mut docs: Vec<HereDocEnd> = Vec::new();
    fn word(cur: &mut Vec<String>, w: &mut Vec<u8>, inw: &mut bool) {
        if *inw {
            cur.push(String::from_utf8_lossy(w).into_owned());
            w.clear();
            *inw = false;
        }
    }
    fn end(out: &mut Vec<Vec<String>>, cur: &mut Vec<String>, w: &mut Vec<u8>, inw: &mut bool) {
        word(cur, w, inw);
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    }
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\'' => {
                let j = b[i + 1..].iter().position(|&x| x == b'\'')?;
                w.extend_from_slice(&b[i + 1..i + 1 + j]);
                inw = true;
                i += 1 + j;
            }
            b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j >= b.len() {
                    return None;
                }
                w.extend_from_slice(&b[i + 1..j]);
                inw = true;
                i = j;
            }
            b'\\' if i + 1 < b.len() => {
                if b[i + 1] != b'\n' {
                    w.push(b[i + 1]);
                    inw = true;
                }
                i += 1;
            }
            b'$' if i + 1 < b.len() && b[i + 1] == b'{' => {
                let j = b[i..].iter().position(|&x| x == b'}')?;
                w.extend_from_slice(&b[i..i + j + 1]);
                inw = true;
                i += j;
            }
            b'$' if i + 1 < b.len() && b[i + 1] == b'(' => {
                end(&mut out, &mut cur, &mut w, &mut inw);
                i += 1;
            }
            b'#' if !inw => {
                let j = b[i..]
                    .iter()
                    .position(|&x| x == b'\n')
                    .unwrap_or(b.len() - i);
                // i += j - 1, then the loop's i++: at the newline.
                i = i + j - 1;
            }
            b' ' | b'\t' => word(&mut cur, &mut w, &mut inw),
            b'<' if b[i..].starts_with(b"<<") && !b[i..].starts_with(b"<<<") => {
                word(&mut cur, &mut w, &mut inw);
                let (d, n) = here_doc_word(&b[i + 2..]);
                docs.push(d);
                i = i + 1 + n;
            }
            b'\n' if !docs.is_empty() => {
                end(&mut out, &mut cur, &mut w, &mut inw);
                i = skip_bodies(b, i + 1, &docs) - 1;
                docs.clear();
            }
            b';' | b'&' | b'|' | b'(' | b')' | b'{' | b'}' | b'`' | b'\n' => {
                end(&mut out, &mut cur, &mut w, &mut inw)
            }
            _ => {
                w.push(c);
                inw = true;
            }
        }
        i += 1;
    }
    end(&mut out, &mut cur, &mut w, &mut inw);
    Some(out)
}

/// The end of the here-document whose << s follows, and how many bytes of
/// s say it.
fn here_doc_word(s: &[u8]) -> (HereDocEnd, usize) {
    let mut i = 0;
    let tabs = s.first() == Some(&b'-');
    if tabs {
        i += 1;
    }
    while i < s.len() && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    let mut word = Vec::new();
    while i < s.len() && !b" \t\n;&|()<>".contains(&s[i]) {
        if s[i] != b'\'' && s[i] != b'"' && s[i] != b'\\' {
            word.push(s[i]);
        }
        i += 1;
    }
    (HereDocEnd { word, tabs }, i)
}

/// Where s goes on after the bodies of docs, which start at from, one
/// after another.
fn skip_bodies(s: &[u8], from: usize, docs: &[HereDocEnd]) -> usize {
    let mut i = from;
    for d in docs {
        while i < s.len() {
            let (mut line, next) = match s[i..].iter().position(|&x| x == b'\n') {
                Some(j) => (&s[i..i + j], i + j + 1),
                None => (&s[i..], s.len()),
            };
            if d.tabs {
                while let Some(rest) = line.strip_prefix(b"\t") {
                    line = rest;
                }
            }
            i = next;
            if line == &d.word[..] {
                break;
            }
        }
    }
    i
}

// The seam to the gate itself.

/// A run asking to start (gate.go Ask).
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub name: String,
    pub rule: Rule,
    /// its agent: RUSH_SESSION, else the agent's pid
    pub session: String,
    /// the share of memory kept free of gated runs
    pub reserve: f64,
}

/// A ticket that got in: its holder keeps it until release (gate.go Run).
pub trait Ticket: Send + Sync {
    /// Records what a run its holder watches itself holds.
    fn note(&self, footprint: u64);
    /// Frees the run's place.
    fn release(&self);
}

/// Tells the agent of a session why its program was stopped.
pub type Tell = Arc<dyn Fn(&str, &str) + Send + Sync>;
/// Records what the guard did and why.
pub type GuardLog = Arc<dyn Fn(&str, rush_events::Fields) + Send + Sync>;

/// What the guard watches by (guard.go Guard).
#[derive(Clone, Default)]
pub struct Guard {
    /// percent; 0 for DEFAULT_FLOOR
    pub floor: i32,
    /// The gate's programs: stopped wherever they run.
    pub names: HashSet<String>,
    pub tell: Option<Tell>,
    pub log: Option<GuardLog>,
}

/// The gate's moving parts, which its own port provides.
pub trait Runtime: Send + Sync {
    /// Guards until stop is done (Guard.Run).
    fn guard(&self, g: Guard, stop: Ctx);
    /// Waits until ask's run may start, and is its ticket (Acquire).
    fn acquire(&self, ctx: &Ctx, ask: Ask) -> Result<Box<dyn Ticket>, String>;
}

static RUNTIME: RwLock<Option<Arc<dyn Runtime>>> = RwLock::new(None);

/// Gives the bundled plugins the gate to run.
pub fn set_runtime(r: Option<Arc<dyn Runtime>>) {
    *RUNTIME.write().unwrap_or_else(|e| e.into_inner()) = r;
}

/// The gate, if this build has one.
pub fn runtime() -> Option<Arc<dyn Runtime>> {
    RUNTIME.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // gate_test.go TestParseRules
    #[test]
    fn parse_rules_and_sizes() {
        let got = parse_rules(&m(&[
            ("names", "tsc, go"),
            ("limit", "20m"),
            ("scope", "worktree"),
            ("parallel", "3"),
            ("overrides", "tsc=system/1/10s"),
            ("limits", "tsc=6G/45m, cargo=512M, go=1h, bad"),
        ]));
        let min = |n: u64| Duration::from_secs(60 * n);
        let want: HashMap<String, Rule> = [
            (
                "tsc",
                Rule {
                    memory: 6 * GIB,
                    limit: min(45),
                },
            ),
            (
                "go",
                Rule {
                    memory: 2 * GIB,
                    limit: min(60),
                },
            ),
            (
                "cargo",
                Rule {
                    memory: 512 << 20,
                    limit: min(20),
                },
            ),
            (
                "bad",
                Rule {
                    memory: OTHER_ESTIMATE,
                    limit: min(20),
                },
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        assert_eq!(got, want);
        let d = parse_rules(&BTreeMap::new());
        assert_eq!(
            d["vitest"],
            Rule {
                memory: 3 * GIB,
                limit: min(30)
            }
        );
        assert_eq!((d["oxlint"].memory, d["tsgo"].memory), (GIB, 4 * GIB));
        for n in ["golangci-lint", "oxlint", "playwright"] {
            assert!(d.contains_key(n), "{n} isn't gated by default");
        }
        for (input, want) in [
            ("15%", 0.15),
            ("0.2", 0.2),
            ("25", 0.25),
            ("nonsense", 0.15),
        ] {
            assert_eq!(parse_reserve(&m(&[("reserve", input)])), want, "{input}");
        }
        for (input, want) in [
            ("6G", Some(6 * GIB)),
            ("1.5GiB", Some(3 * GIB / 2)),
            ("512mb", Some(512 << 20)),
            ("4", None),
            ("10s", None),
        ] {
            assert_eq!(parse_size(input), want, "{input}");
        }
        assert_eq!(size(3 * GIB / 2), "1.5 GiB");
        assert_eq!(size(640 << 20), "640 MiB");
    }

    // guard_test.go TestParseFloor
    #[test]
    fn parse_floors() {
        for (input, want) in [
            ("5%", 5),
            ("10%", 10),
            ("off", 0),
            ("nonsense", DEFAULT_FLOOR),
            ("80%", DEFAULT_FLOOR),
        ] {
            assert_eq!(parse_floor(&m(&[("floor", input)])), want, "{input}");
        }
        assert_eq!(parse_floor(&BTreeMap::new()), DEFAULT_FLOOR);
    }

    // heavy_test.go TestNodeName
    #[test]
    fn node_names() {
        let rules = parse_rules(&BTreeMap::new());
        for (script, want) in [
            (
                "/r/node_modules/.pnpm/typescript@7/node_modules/typescript/bin/tsc",
                "tsc",
            ),
            (
                "/r/node_modules/.pnpm/vitest@5/node_modules/vitest/vitest.mjs",
                "vitest",
            ),
            ("/r/node_modules/vitest/dist/workers/forks.js", "vitest"),
            ("/r/node_modules/@playwright/test/cli.js", "playwright"),
            ("/r/node_modules/oxlint/bin/oxlint", "oxlint"),
            ("/r/node_modules/pnpm/bin/pnpm.cjs", ""),
            ("/r/apps/api/src/main.ts", ""),
        ] {
            assert_eq!(node_name(script, &rules), want, "{script}");
        }
    }

    fn words(s: &str) -> Vec<Vec<String>> {
        commands(s).unwrap_or_default()
    }

    #[test]
    fn commands_split_as_sh() {
        assert_eq!(
            words("cd app && LC_ALL=C grep -rn 'a b' \"c d\" | head # x"),
            [
                vec!["cd", "app"],
                vec!["LC_ALL=C", "grep", "-rn", "a b", "c d"],
                vec!["head"]
            ]
        );
        assert_eq!(
            words("echo $(git grep x)"),
            [vec!["echo"], vec!["git", "grep", "x"]]
        );
        assert_eq!(commands("echo 'open"), None);
        assert_eq!(commands("echo \"open"), None);
        // A here-document's body isn't commands.
        assert_eq!(
            words("cat <<'EOF'\ngrep -r x .\nEOF\nls"),
            [vec!["cat"], vec!["ls"]]
        );
        assert_eq!(
            words("cat <<-EOF\n\tgrep -r x\n\tEOF\nls"),
            [vec!["cat"], vec!["ls"]]
        );
        assert_eq!(words("a \\\nb ${X} c"), [vec!["a", "b", "${X}", "c"]]);
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
