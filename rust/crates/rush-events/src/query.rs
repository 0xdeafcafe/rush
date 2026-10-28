//! Reading the log back, for rush events and rush why: newest first, from
//! the end of the file, stopping as soon as there's enough. A full log is
//! 16MB; a question about the last few minutes reads the last few KB of it.
//! Ported from Go `internal/events/query.go`.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

use rush_core::time::Ms;
use serde_json::{Map, Value};

use crate::Error;
use crate::json::marshal;
use crate::log::rotated;
use crate::since::parse_rfc3339;

/// A line of the log: what it's filtered and linked by, read without
/// decoding the rest, and the line itself (query.go:22). The strings are as
/// written, escapes and all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    /// When, in Unix ms; 0 where it doesn't say.
    pub at: Ms,
    pub kind: String,
    pub proc: String,
    pub session: String,
    pub ev: String,
    pub cause: String,
    pub from: String,
    pub xid: String,
    pub spawned_by: String,
    /// Its kind says something failed, or it has an err.
    pub failed: bool,
    pub raw: Vec<u8>,
}

/// What lines a query wants. Its default wants every one (query.go:32).
#[derive(Clone, Debug, Default)]
pub struct Filter {
    /// Any of these.
    pub sessions: Vec<String>,
    /// Any of these; one ending in * is any kind it starts.
    pub kinds: Vec<String>,
    /// Which of rush's processes logged it.
    pub proc: String,
    /// None before (Unix ms; 0 for no bound).
    pub since: Ms,
    /// The line has this, in any case.
    pub grep: String,
    /// At most this many; 0 is no limit.
    pub limit: usize,
}

impl Filter {
    fn matches(&self, l: &Line, grep: Option<&str>) -> bool {
        if l.at != 0 && l.at < self.since {
            return false;
        }
        if !self.sessions.is_empty() && !self.sessions.contains(&l.session) {
            return false;
        }
        if !self.proc.is_empty() && l.proc != self.proc {
            return false;
        }
        if !self.kinds.is_empty()
            && !self.kinds.iter().any(|k| match k.strip_suffix('*') {
                Some(p) => l.kind.starts_with(p),
                None => *k == l.kind,
            })
        {
            return false;
        }
        match grep {
            None => true,
            Some(g) => String::from_utf8_lossy(&l.raw).to_lowercase().contains(g),
        }
    }
}

/// The lines of the log at log that f wants, newest first, and whether more
/// matched past f.limit (query.go:63).
pub fn query(log: &Path, f: &Filter) -> Result<(Vec<Line>, bool), Error> {
    let grep = (!f.grep.is_empty()).then(|| f.grep.to_lowercase());
    let mut lines = Vec::new();
    let mut more = false;
    each_back(log, f.since, |l| {
        if !f.matches(&l, grep.as_deref()) {
            return true;
        }
        if f.limit > 0 && lines.len() == f.limit {
            more = true;
            return false;
        }
        lines.push(l);
        true
    })?;
    Ok((lines, more))
}

/// What happened to session since: its own lines, those of the sessions
/// linked to it (linked, and any the log shows sending to it, started by
/// it or starting it), rush's own failures, and the lines that led to any
/// of those. Oldest first (query.go:86).
pub fn query_why(
    log: &Path,
    session: &str,
    linked: &[String],
    since: Ms,
) -> Result<Vec<Line>, Error> {
    let mut all = Vec::new();
    each_back(log, since, |l| {
        if l.at >= since {
            all.push(l);
        }
        true
    })?;
    all.reverse();
    let mut near: HashSet<&str> = HashSet::new();
    near.insert(session);
    near.extend(linked.iter().map(String::as_str));
    for l in &all {
        if l.session == session {
            near.insert(&l.from);
            near.insert(&l.spawned_by);
        } else if l.from == session || l.spawned_by == session {
            near.insert(&l.session);
        }
    }
    near.remove("");
    let mut keep = vec![false; all.len()];
    let at = evs(&all);
    for (i, l) in all.iter().enumerate() {
        if near.contains(l.session.as_str()) || l.session.is_empty() && l.failed {
            keep[i] = true;
            // And what led to it, back to the start of the window.
            let mut c = l.cause.as_str();
            while !c.is_empty() {
                let Some(&j) = at.get(c) else { break };
                if keep[j] {
                    break;
                }
                keep[j] = true;
                c = &all[j].cause;
            }
        }
    }
    drop(near);
    Ok(all
        .into_iter()
        .zip(keep)
        .filter_map(|(l, k)| k.then_some(l))
        .collect())
}

/// The chain of causes line ev is in: the line nothing led to that it comes
/// from, and everything that came of that. Oldest first (query.go:135).
pub fn chain(log: &Path, ev: &str) -> Result<Vec<Line>, Error> {
    let mut seen: Vec<Line> = Vec::new();
    let mut want = ev.to_string();
    let mut root: Option<usize> = None;
    each_back(log, 0, |l| {
        let hit = l.ev == want;
        let cause = l.cause.clone();
        seen.push(l);
        if !hit {
            return true;
        }
        root = Some(seen.len() - 1);
        want = cause;
        !want.is_empty() // the first cause: everything after it is seen
    })?;
    let Some(root) = root else {
        return Err(Error::NoLine(ev.to_string()));
    };
    // The furthest cause found is the root, where one before it is gone with
    // an older log.
    seen.truncate(root + 1);
    seen.reverse();
    // What came of the root, through as many steps as it took. A line can
    // be written a little before its cause (each process writes its own),
    // so it's looked at again until nothing more joins.
    let parent = parents(&seen);
    let mut inn = vec![false; seen.len()];
    inn[0] = true;
    let mut grew = true;
    while grew {
        grew = false;
        for i in 0..seen.len() {
            if !inn[i]
                && let Some(p) = parent[i]
                && inn[p]
            {
                inn[i] = true;
                grew = true;
            }
        }
    }
    Ok(seen
        .into_iter()
        .zip(inn)
        .filter_map(|(l, k)| k.then_some(l))
        .collect())
}

/// Where each ev's line is in lines.
fn evs(lines: &[Line]) -> HashMap<&str, usize> {
    let mut at = HashMap::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        if !l.ev.is_empty() {
            at.insert(l.ev.as_str(), i);
        }
    }
    at
}

/// For each of lines, which line led to it: its cause, else, for a message
/// delivered from the queue, its queuing (query.go:189).
fn parents(lines: &[Line]) -> Vec<Option<usize>> {
    let at = evs(lines);
    let mut queued: HashMap<&str, usize> = HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        if l.kind == "queued" && !l.xid.is_empty() {
            queued.insert(&l.xid, i);
        }
    }
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if !l.cause.is_empty()
                && let Some(&j) = at.get(l.cause.as_str())
                && j != i
            {
                return Some(j);
            }
            if l.cause.is_empty() && l.kind == "delivered" {
                return queued.get(l.xid.as_str()).copied();
            }
            None
        })
        .collect()
}

/// A line in a tree of causes, and how deep it is.
#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    pub line: &'a Line,
    pub depth: usize,
}

/// Puts lines (oldest first) under their causes: each line after the one
/// that led to it, a step deeper. A message delivered from the queue goes
/// under its queuing. The lines nothing among them led to are the roots,
/// newest first, as a query's are; what came of each is in the order it
/// happened (query.go:217).
pub fn tree<'a>(lines: &'a [Line]) -> Vec<Node<'a>> {
    let parent = parents(lines);
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            kids[*p].push(i);
        }
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut shown = vec![false; lines.len()];
    let mut walk = |root: usize, out: &mut Vec<Node<'a>>| {
        let mut stack = vec![(root, 0usize)];
        while let Some((i, depth)) = stack.pop() {
            if shown[i] {
                continue; // two lines that share an ev, each the other's cause
            }
            shown[i] = true;
            out.push(Node {
                line: &lines[i],
                depth,
            });
            for &k in kids[i].iter().rev() {
                stack.push((k, depth + 1));
            }
        }
    };
    for i in (0..lines.len()).rev() {
        if parent[i].is_none() {
            walk(i, &mut out);
        }
    }
    // A ring of causes has no root: each at the top.
    for i in 0..lines.len() {
        walk(i, &mut out);
    }
    out
}

/// Lines as nodes at the top, for format.
pub fn flat(lines: &[Line]) -> Vec<Node<'_>> {
    lines.iter().map(|line| Node { line, depth: 0 }).collect()
}

/// How much text a query prints at most: enough to read, and little enough
/// for an agent to take whole.
pub const BUDGET: usize = 16 << 10;

/// Lines as text, a line each: how long ago, what, which session, which
/// process, its ev and cause, then the rest of it, each value cut short.
/// Past budget it stops, saying how many more there were and to narrow,
/// and the same if more past them weren't read (query.go:258).
pub fn format(nodes: &[Node<'_>], now: Ms, budget: usize, more: bool, narrow: &str) -> String {
    let mut b = String::new();
    for (i, n) in nodes.iter().enumerate() {
        let mut row = format_line(n.line, now);
        let d = n.depth.min(8);
        if d > 0 {
            row = "  ".repeat(d) + &row;
        }
        if b.len() + row.len() + 1 > budget {
            b.push_str(&format!(
                "… {} more; narrow with {narrow}\n",
                nodes.len() - i
            ));
            return b;
        }
        b.push_str(&row);
        b.push('\n');
    }
    if more {
        b.push_str(&format!("… more before these; narrow with {narrow}\n"));
    }
    b
}

/// How much of a value a row shows, and of the row.
const VALUE_MAX: usize = 120;
const ROW_MAX: usize = 320;

/// A line's fields said before the rest, as they say most.
const SHOWN_FIRST: [&str; 5] = ["why", "err", "from", "xid", "turn"];

/// The fields every row has a column for.
const OWN: [&str; 7] = ["t", "kind", "proc", "pid", "session", "ev", "cause"];

fn format_line(l: &Line, now: Ms) -> String {
    let mut b = String::new();
    // A line with no time is from Go's zero time: as long ago as can be.
    b.push_str(&age(if l.at == 0 {
        i64::MAX / 1_000_000
    } else {
        now - l.at
    }));
    b.push_str("  ");
    b.push_str(&l.kind);
    if !l.session.is_empty() {
        b.push_str("  s=");
        b.push_str(&l.session);
    }
    let m: Map<String, Value> = serde_json::from_slice(&l.raw).unwrap_or_default();
    b.push_str("  ");
    b.push_str(&l.proc);
    if let Some(pid) = m.get("pid").and_then(Value::as_f64) {
        b.push_str(&format!(":{}", pid as i64));
    }
    if !l.ev.is_empty() {
        b.push_str("  #");
        b.push_str(&l.ev);
    }
    if !l.cause.is_empty() {
        b.push_str(" ←");
        b.push_str(&l.cause);
    }
    let mut keys: Vec<&str> = m
        .keys()
        .map(String::as_str)
        .filter(|k| !OWN.contains(k) && !SHOWN_FIRST.contains(k))
        .collect();
    keys.sort_unstable();
    for k in SHOWN_FIRST.iter().copied().chain(keys) {
        let Some(v) = m.get(k) else { continue };
        if v.is_null() || v.as_str() == Some("") {
            continue;
        }
        b.push_str("  ");
        b.push_str(k);
        b.push('=');
        b.push_str(&cut(&value(v), VALUE_MAX));
    }
    cut(&b, ROW_MAX)
}

/// v as a row shows it: a string as it is where it has no spaces, else as
/// JSON.
fn value(v: &Value) -> String {
    if let Value::String(s) = v
        && !s.is_empty()
        && !s.contains([' ', '\t', '\n', '"'])
    {
        return s.clone();
    }
    marshal(v, true)
}

fn cut(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        None => s.to_string(),
        Some(_) => {
            let end = s.char_indices().nth(n - 1).map_or(s.len(), |(i, _)| i);
            format!("{}…", &s[..end])
        }
    }
}

/// d (ms) as a row says it: 40s, 12m, 3h, 2d.
pub fn age(d: i64) -> String {
    if d < 60_000 {
        format!("{}s", (d / 1000).max(0))
    } else if d < 3_600_000 {
        format!("{}m", d / 60_000)
    } else if d < 48 * 3_600_000 {
        format!("{}h", d / 3_600_000)
    } else {
        format!("{}d", d / 86_400_000)
    }
}

/// How far out of order the log's lines can be: each process writes its
/// own, a little after it logs them.
const SLACK: Ms = 60_000;

/// Hands f the log at log a line at a time, newest first, through its last
/// log too, until f says stop or the lines are from before since.
fn each_back(log: &Path, since: Ms, mut f: impl FnMut(Line) -> bool) -> Result<(), Error> {
    for p in [log.to_path_buf(), rotated(log)] {
        let done = file_back(&p, |raw| {
            let Some(l) = parse_head(raw) else {
                return true;
            };
            if since != 0 && l.at != 0 && l.at < since - SLACK {
                return false;
            }
            f(l)
        });
        match done {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Io(go_err(&e, &p))),
        }
    }
    Ok(())
}

/// An os error as Go says it: "open PATH: permission denied".
fn go_err(e: &io::Error, p: &Path) -> String {
    let what = match e.raw_os_error() {
        Some(code) => {
            let s = io::Error::from_raw_os_error(code).to_string();
            let s = s.split(" (os error").next().unwrap_or("").to_string();
            let mut c = s.chars();
            c.next().map_or(String::new(), |f| {
                f.to_lowercase().collect::<String>() + c.as_str()
            })
        }
        None => e.to_string(),
    };
    format!("open {}: {what}", p.display())
}

/// How much of a log is read at a time, from its end.
const BACK_CHUNK: u64 = 64 << 10;

/// Hands f the lines of the file at p, last first, until it says stop:
/// Ok(true) is whether it did.
fn file_back(p: &Path, mut f: impl FnMut(&[u8]) -> bool) -> io::Result<bool> {
    let file = File::open(p)?;
    let size = file.metadata()?.len();
    let mut head: Vec<u8> = Vec::new(); // the start of a line whose rest was in the chunk after
    let mut off = size;
    while off > 0 {
        let n = BACK_CHUNK.min(off);
        off -= n;
        let mut b = vec![0u8; n as usize + head.len()];
        file.read_exact_at(&mut b[..n as usize], off)?;
        b[n as usize..].copy_from_slice(&head);
        let mut end = b.len();
        for i in (0..b.len()).rev() {
            if b[i] != b'\n' {
                continue;
            }
            if i + 1 < end && !f(&b[i + 1..end]) {
                return Ok(true);
            }
            end = i;
        }
        b.truncate(end);
        head = b;
    }
    if !head.is_empty() && !f(&head) {
        return Ok(true);
    }
    Ok(false)
}

/// Reads the fields of a line that Line has, from the top of its object
/// only, skipping the rest without decoding it (query.go:477).
pub(crate) fn parse_head(raw: &[u8]) -> Option<Line> {
    let mut l = Line::default();
    let mut i = skip_space(raw, 0);
    if i >= raw.len() || raw[i] != b'{' {
        return None;
    }
    i += 1;
    loop {
        i = skip_space(raw, i);
        if i >= raw.len() {
            return None;
        }
        if raw[i] == b'}' {
            break;
        }
        if raw[i] == b',' {
            i += 1;
            continue;
        }
        let (k, j) = str_at(raw, i)?;
        i = skip_space(raw, j);
        if i >= raw.len() || raw[i] != b':' {
            return None;
        }
        i = skip_space(raw, i + 1);
        if i >= raw.len() {
            return None;
        }
        if raw[i] != b'"' {
            i = skip_value(raw, i)?;
            if k == b"err" {
                l.failed = true; // an error, not as a string
            }
            continue;
        }
        let (v, j) = str_at(raw, i)?;
        i = j;
        let s = || String::from_utf8_lossy(v).into_owned();
        match k {
            b"t" => l.at = parse_rfc3339(&s()).unwrap_or(0),
            b"kind" => l.kind = s(),
            b"proc" => l.proc = s(),
            b"session" => l.session = s(),
            b"ev" => l.ev = s(),
            b"cause" => l.cause = s(),
            b"from" => l.from = s(),
            b"xid" => l.xid = s(),
            b"spawnedBy" => l.spawned_by = s(),
            b"err" => l.failed = l.failed || !v.is_empty(),
            _ => {}
        }
    }
    l.failed = l.failed || failed_kind(&l.kind);
    l.raw = raw.to_vec();
    Some(l)
}

/// Whether a line of kind says something went wrong.
fn failed_kind(kind: &str) -> bool {
    [
        "fail", "kill", "stuck", "forced", "refused", "panic", "crash",
    ]
    .iter()
    .any(|w| kind.contains(w))
}

fn skip_space(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    i
}

/// The string at b[i] as written, escapes and all, and where it ends.
fn str_at(b: &[u8], i: usize) -> Option<(&[u8], usize)> {
    if i >= b.len() || b[i] != b'"' {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 1,
            b'"' => return Some((&b[i + 1..j], j + 1)),
            _ => {}
        }
        j += 1;
    }
    None
}

/// Where the value at b[i] that isn't a string ends.
fn skip_value(b: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i = str_at(b, i)?.1;
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            b',' if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{Fields, Logger};
    use crate::testdir::TempDir;
    use serde_json::json;

    /// A test's rush log, with lines as they'd be kept, oldest first.
    fn write_log(lines: &[String]) -> (TempDir, std::path::PathBuf) {
        let home = TempDir::new();
        let p = home.0.join("events.jsonl");
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        (home, p)
    }

    fn at(ago_ms: i64) -> String {
        crate::log::rfc3339_nano(
            std::time::UNIX_EPOCH
                + std::time::Duration::from_millis((rush_core::time::now() - ago_ms) as u64),
        )
    }

    const SEC: i64 = 1000;
    const MIN: i64 = 60 * SEC;
    const HOUR: i64 = 60 * MIN;

    fn fields(v: Value) -> Fields {
        match v {
            Value::Object(m) => m,
            _ => Fields::new(),
        }
    }

    // query_test.go:22
    #[test]
    fn lines_carry_ev_and_the_process_cause() {
        let home = TempDir::new();
        let p = home.0.join("events.jsonl");
        let lg = Logger::new(Some(p.clone()));
        lg.set_cause("abc12345");
        lg.log("first", Fields::new());
        lg.log(
            "second",
            fields(json!({"ev": "00000001", "cause": "feedface"})),
        );
        lg.log("third", fields(json!({"cause": ""})));
        lg.flush();
        let (lines, _) = query(&p, &Filter::default()).unwrap();
        assert_eq!(lines.len(), 3, "{lines:?}");
        let (third, second, first) = (&lines[0], &lines[1], &lines[2]);
        assert!(
            first.ev.len() == 8 && first.cause == "abc12345",
            "the process's cause goes on its lines: {first:?}"
        );
        assert!(
            second.ev == "00000001" && second.cause == "feedface",
            "a line's own ev and cause are kept: {second:?}"
        );
        assert!(
            third.cause.is_empty() && !String::from_utf8_lossy(&third.raw).contains("cause"),
            "a line that names no cause has none: {third:?}"
        );
    }

    // query_test.go:46
    #[test]
    fn query_filters() {
        let (_home, p) = write_log(&[
            format!(
                r#"{{"ev":"a1","kind":"host-started","proc":"host","session":"s1","t":"{}"}}"#,
                at(3 * HOUR)
            ),
            format!(
                r#"{{"ev":"a2","kind":"delivered","proc":"host","session":"s1","t":"{}","text":"fix the Auth bug"}}"#,
                at(20 * MIN)
            ),
            format!(
                r#"{{"ev":"a3","kind":"queued","proc":"host","session":"s2","t":"{}"}}"#,
                at(10 * MIN)
            ),
            "not json".into(),
            format!(
                r#"{{"ev":"a4","kind":"plugin-start","info":{{"kind":"x","session":"s1"}},"proc":"plugind","t":"{}"}}"#,
                at(MIN)
            ),
            format!(
                r#"{{"ev":"a5","kind":"plugin-stop","proc":"plugind","t":"{}"}}"#,
                at(30 * SEC)
            ),
        ]);
        let evs = |f: Filter| {
            let (lines, more) = query(&p, &f).unwrap();
            let mut out: Vec<&str> = Vec::new();
            let s: Vec<String> = lines.iter().map(|l| l.ev.clone()).collect();
            out.extend(s.iter().map(String::as_str));
            if more {
                out.push("+");
            }
            out.join(" ")
        };
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for (f, want) in [
            (Filter::default(), "a5 a4 a3 a2 a1"),
            (
                Filter {
                    limit: 2,
                    ..Default::default()
                },
                "a5 a4 +",
            ),
            // Not a4, whose session is inside another field.
            (
                Filter {
                    sessions: s(&["s1"]),
                    ..Default::default()
                },
                "a2 a1",
            ),
            (
                Filter {
                    kinds: s(&["delivered", "queued"]),
                    ..Default::default()
                },
                "a3 a2",
            ),
            (
                Filter {
                    kinds: s(&["plugin*"]),
                    ..Default::default()
                },
                "a5 a4",
            ),
            (
                Filter {
                    proc: "plugind".into(),
                    ..Default::default()
                },
                "a5 a4",
            ),
            (
                Filter {
                    since: rush_core::time::now() - 15 * MIN,
                    ..Default::default()
                },
                "a5 a4 a3",
            ),
            (
                Filter {
                    grep: "auth".into(),
                    ..Default::default()
                },
                "a2",
            ),
        ] {
            let desc = format!("{f:?}");
            assert_eq!(evs(f), want, "{desc}");
        }
    }

    // query_test.go:89
    #[test]
    fn query_reads_back_across_chunks_and_the_last_log() {
        let lines: Vec<String> = (0..3000)
            .map(|i| {
                format!(
                    r#"{{"ev":"n{i}","kind":"k","pad":"{}","t":"{}"}}"#,
                    "x".repeat(40),
                    at((3000 - i) * SEC)
                )
            })
            .collect();
        let (_home, p) = write_log(&lines);
        std::fs::write(
            rotated(&p),
            format!(
                "{{\"ev\":\"old\",\"kind\":\"k\",\"t\":\"{}\"}}\n",
                at(5 * HOUR)
            ),
        )
        .unwrap();
        let (all, _) = query(&p, &Filter::default()).unwrap();
        assert_eq!(all.len(), 3001);
        assert_eq!(all[3000].ev, "old");
        for (i, l) in all[..3000].iter().enumerate() {
            assert_eq!(l.ev, format!("n{}", 2999 - i), "line {i}");
        }
    }

    // query_test.go:109
    #[test]
    fn format_keeps_to_its_budget() {
        let now = rush_core::time::now();
        let lines: Vec<Line> = (0..50)
            .map(|i| {
                let raw = format!(
                    r#"{{"ev":"e{i}","kind":"state","pid":7,"proc":"host","session":"s1","why":"{}","t":"x"}}"#,
                    "long ".repeat(100)
                );
                let mut l = parse_head(raw.as_bytes()).unwrap();
                l.at = now - i * 90 * SEC;
                l
            })
            .collect();
        let out = format(&flat(&lines), now, 2000, false, "-k");
        assert!(
            out.len() <= 2000 + 80,
            "{} chars past a budget of 2000",
            out.len()
        );
        let rows: Vec<&str> = out.trim().split('\n').collect();
        let last = rows[rows.len() - 1];
        assert!(
            last.starts_with("… ") && last.ends_with(" more; narrow with -k"),
            "it ends saying what's left: {last:?}"
        );
        assert!(
            rows[0].starts_with("0s  state  s=s1  host:7  #e0")
                && rows[0].contains("why=\"long long")
                && rows[0].contains('…'),
            "row: {:?}",
            rows[0]
        );
        assert!(rows[1].starts_with("1m  "), "ages: {:?}", rows[1]);
    }

    #[test]
    fn rows_say_fields_in_order() {
        let raw = r#"{"zeta":[1,2],"alpha":{"b":2,"a":"x y"},"kind":"k","proc":"p","pid":3,"t":"x","err":"","why":"because","turn":2,"n":null}"#;
        let l = parse_head(raw.as_bytes()).unwrap();
        assert_eq!(
            format_line(&l, 0),
            r#"106751d  k  p:3  why=because  turn=2  alpha={"a":"x y","b":2}  zeta=[1,2]"#
        );
    }

    // query_test.go:136
    #[test]
    fn ages() {
        for (d, want) in [
            (40 * SEC, "40s"),
            (12 * MIN, "12m"),
            (3 * HOUR, "3h"),
            (50 * HOUR, "2d"),
            (-5 * SEC, "0s"),
        ] {
            assert_eq!(age(d), want);
        }
    }

    #[test]
    fn heads_say_what_failed() {
        for (raw, failed) in [
            (r#"{"kind":"plugind-exit"}"#, false),
            (r#"{"kind":"x","err":"broker died"}"#, true),
            (r#"{"kind":"x","err":""}"#, false),
            (r#"{"kind":"x","err":{"code":1}}"#, true),
            (r#"{"kind":"host-killed"}"#, true),
            (r#"{"kind":"turn-stuck"}"#, true),
        ] {
            assert_eq!(parse_head(raw.as_bytes()).unwrap().failed, failed, "{raw}");
        }
        for bad in ["not json", "{\"kind\":", "{\"a\":[1,2"] {
            assert!(parse_head(bad.as_bytes()).is_none(), "{bad}");
        }
    }

    /// A story across two sessions: A's agent sends to B, which was asleep;
    /// the send wakes B's host, whose agent starts and ends a turn. A
    /// message queued in B is delivered later with no cause, by its
    /// exchange (query_test.go:145).
    fn story() -> (TempDir, std::path::PathBuf) {
        write_log(&[
            format!(
                r#"{{"ev":"aS","kind":"agent-started","proc":"host","session":"A","t":"{}"}}"#,
                at(50 * MIN)
            ),
            format!(
                r#"{{"ev":"zz","kind":"state","proc":"host","session":"C","t":"{}"}}"#,
                at(45 * MIN)
            ),
            format!(
                r#"{{"ev":"hS","cause":"wk","kind":"host-started","proc":"host","session":"B","t":"{}"}}"#,
                at(40 * MIN)
            ),
            format!(
                r#"{{"ev":"wk","cause":"aS","kind":"woken","proc":"session","session":"B","t":"{}"}}"#,
                at(40 * MIN - SEC)
            ),
            format!(
                r#"{{"ev":"dl","cause":"wk","from":"A","kind":"delivered","proc":"host","session":"B","turn":1,"xid":"x1","t":"{}"}}"#,
                at(39 * MIN)
            ),
            format!(
                r#"{{"ev":"q1","cause":"aS","from":"A","kind":"queued","proc":"host","session":"B","xid":"x2","t":"{}"}}"#,
                at(38 * MIN)
            ),
            format!(
                r#"{{"ev":"te","cause":"dl","kind":"turn-end","proc":"host","session":"B","turn":1,"t":"{}"}}"#,
                at(30 * MIN)
            ),
            format!(
                r#"{{"ev":"d2","from":"A","kind":"delivered","proc":"host","session":"B","turn":2,"xid":"x2","t":"{}"}}"#,
                at(29 * MIN)
            ),
            format!(
                r#"{{"ev":"gf","kind":"plugind-exit","err":"broker died","proc":"plugind","t":"{}"}}"#,
                at(20 * MIN)
            ),
            format!(
                r#"{{"ev":"ok","kind":"saved","proc":"ui","t":"{}"}}"#,
                at(10 * MIN)
            ),
        ])
    }

    fn shape(nodes: &[Node<'_>]) -> String {
        nodes
            .iter()
            .map(|n| ".".repeat(n.depth) + &n.line.ev)
            .collect::<Vec<_>>()
            .join(" ")
    }

    // query_test.go:170
    #[test]
    fn why_indents_under_causes() {
        let (_home, p) = story();
        let lines = query_why(&p, "B", &[], rush_core::time::now() - HOUR).unwrap();
        // A's agent is in it as B's sender and the cause of the wake; C
        // isn't linked; the broker's failure is rush-wide; the ui's save
        // isn't. The host started is written before the wake that caused
        // it, and still goes under it.
        assert_eq!(shape(&tree(&lines)), "gf aS .wk ..hS ..dl ...te .q1 ..d2");
        let out = format(
            &tree(&lines),
            rush_core::time::now(),
            BUDGET,
            false,
            "--last",
        );
        assert!(
            out.contains("\n  ") && out.contains("#wk ←aS"),
            "formatted:\n{out}"
        );
    }

    // query_test.go:189
    #[test]
    fn chain_follows_one_line() {
        let (_home, p) = story();
        let lines = chain(&p, "te").unwrap();
        assert_eq!(shape(&tree(&lines)), "aS .wk ..hS ..dl ...te .q1 ..d2");
        let err = chain(&p, "nope").unwrap_err();
        assert_eq!(err.to_string(), "no line in rush's log is nope");
    }

    #[test]
    fn a_ring_of_causes_still_shows_each_line() {
        let lines: Vec<Line> = [
            r#"{"ev":"a","cause":"b","kind":"k"}"#,
            r#"{"ev":"b","cause":"a","kind":"k"}"#,
        ]
        .iter()
        .map(|r| parse_head(r.as_bytes()).unwrap())
        .collect();
        assert_eq!(shape(&tree(&lines)), "a .b");
    }

    #[test]
    fn a_missing_log_is_no_lines() {
        let home = TempDir::new();
        let (lines, more) = query(&home.0.join("events.jsonl"), &Filter::default()).unwrap();
        assert!(lines.is_empty() && !more);
    }
}
