//! What a transcript has cost and says: Scan reads only what was appended
//! since its last offset, pricing each message once; ReadPreview reads the
//! tail backwards for the last tool call, text and prompt. Port of
//! internal/adapters/claude/claude/transcript.go.

use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use rush_core::agent::{Halt, Preview, PreviewLine, SubagentStats, TokenUsage};
use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::json::{Str, contains, day, lines, ms, nd, str_of};
use crate::pricing::cost;
use crate::tools::doing;

/// What one transcript file has cost so far. It is small so the scan
/// resumes from `offset` instead of re-reading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub offset: u64,
    pub size: u64,
    pub by_model: Vec<(String, TokenUsage)>,
    pub cost: f64,
    pub first: Ms,
    pub last: Ms,
    pub prs: Vec<String>,
    pub last_model: String,
    /// Spend by local day (yyyymmdd).
    pub days: Vec<(u32, f64)>,
    /// The folders the session worked in (the line's cwd), which says
    /// which worktrees are its; capped so a wandering one stays small.
    pub dirs: Vec<String>,
    /// Where the session works now: where it last wrote a file, or, before
    /// it has, its newest cwd or where it last cd'd. Once it writes, only
    /// another write or entering a worktree moves it.
    pub dir: String,
    /// `dir` is where it last wrote.
    pub wrote: bool,
    /// When `dir` last changed.
    pub dir_at: Ms,
    /// The newest line's cwd, so only a real move of it overrides `dir`.
    pub cwd: String,
    /// The newest assistant message; its usage can still change while more
    /// content blocks of the same message are appended.
    pub pending_id: String,
    pub pending_model: String,
    pub pending_use: TokenUsage,
    pub pending_fast: bool,
    pub pending_at: Ms,
    /// The error the session's last turn ended on; the next real reply
    /// clears it.
    pub halt: Option<Halt>,
    /// The last sentence it wrote with a count going from one number to
    /// another ("lint 11,065 → 9,052"), and when.
    pub progress: String,
    pub progress_at: Ms,
    /// How many times its context was compacted.
    pub compacts: u32,
}

impl Totals {
    /// How much of the model's window the newest message used.
    pub fn context(&self) -> i64 {
        let u = &self.pending_use;
        u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h
    }

    fn commit_pending(&mut self) {
        if self.pending_id.is_empty() {
            return;
        }
        let model = std::mem::take(&mut self.pending_model);
        let u = std::mem::take(&mut self.pending_use);
        match self.by_model.iter_mut().find(|(m, _)| *m == model) {
            Some((_, t)) => t.add(u),
            None => self.by_model.push((model.clone(), u)),
        }
        let c = cost(&model, u, self.pending_fast);
        self.cost += c;
        let d = day(self.pending_at);
        match self.days.iter_mut().find(|(k, _)| *k == d) {
            Some((_, v)) => *v += c,
            None => self.days.push((d, c)),
        }
        self.pending_id.clear();
        self.pending_fast = false;
    }

    /// Cost and tokens, the pending message included so a running agent's
    /// figure moves live.
    pub fn spend(&self) -> (f64, TokenUsage) {
        let mut u = TokenUsage::default();
        for (_, m) in &self.by_model {
            u.add(*m);
        }
        u.add(self.pending_use);
        (
            self.cost + cost(&self.pending_model, self.pending_use, self.pending_fast),
            u,
        )
    }

    /// What was spent on one local calendar day (yyyymmdd, see `day`).
    pub fn day_spend(&self, d: u32) -> f64 {
        let mut c = self
            .days
            .iter()
            .find(|(k, _)| *k == d)
            .map(|(_, v)| *v)
            .unwrap_or(0.0);
        if !self.pending_id.is_empty() && day(self.pending_at) == d {
            c += cost(&self.pending_model, self.pending_use, self.pending_fast);
        }
        c
    }

    fn set_dir(&mut self, d: &str, at: Ms) {
        if d != self.dir {
            self.dir = d.to_string();
            self.dir_at = at;
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(borrow)]
    timestamp: Str<'a>,
    #[serde(borrow)]
    cwd: Str<'a>,
    #[serde(borrow)]
    effort: Str<'a>,
    #[serde(rename = "isApiErrorMessage", deserialize_with = "nd")]
    api_error: bool,
    #[serde(borrow)]
    error: Str<'a>,
    #[serde(borrow, deserialize_with = "nd")]
    message: Msg<'a>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Msg<'a> {
    #[serde(borrow)]
    id: Str<'a>,
    #[serde(borrow)]
    model: Str<'a>,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    #[serde(borrow)]
    usage: Option<Usage<'a>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Usage<'a> {
    #[serde(deserialize_with = "nd")]
    input_tokens: i64,
    #[serde(deserialize_with = "nd")]
    output_tokens: i64,
    #[serde(deserialize_with = "nd")]
    cache_read_input_tokens: i64,
    #[serde(deserialize_with = "nd")]
    cache_creation_input_tokens: i64,
    #[serde(borrow)]
    speed: Str<'a>,
    cache_creation: Option<Breakup>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Breakup {
    #[serde(deserialize_with = "nd")]
    ephemeral_5m_input_tokens: i64,
    #[serde(deserialize_with = "nd")]
    ephemeral_1h_input_tokens: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ContentBlock<'a> {
    #[serde(rename = "type", borrow)]
    typ: Str<'a>,
    #[serde(borrow)]
    text: Str<'a>,
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow)]
    input: Option<&'a RawValue>,
}

fn blocks_of(content: Option<&RawValue>) -> Option<Vec<ContentBlock<'_>>> {
    serde_json::from_str(content?.get()).ok()
}

const ASSISTANT: &[u8] = br#""type":"assistant""#;
const USER: &[u8] = br#""type":"user""#;
const COMPACT: &[u8] = br#""compact_boundary""#;
const RELOCATED: &[u8] = br#""type":"relocated""#;
const TOOL_USE: &[u8] = br#""type":"tool_use""#;

/// Advances t over whatever was appended to path since `t.offset`. Only
/// complete lines are consumed, so a half-written line is read next time.
/// buf is a reusable read buffer.
pub fn scan(path: &Path, t: &mut Totals, buf: &mut Vec<u8>) -> io::Result<()> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    if size < t.offset {
        *t = Totals::default();
    }
    t.size = size;
    if size == t.offset {
        return Ok(());
    }
    f.seek(SeekFrom::Start(t.offset))?;
    const CHUNK: usize = 256 << 10;
    buf.clear();
    let mut start = 0; // where buf's unconsumed bytes begin
    loop {
        if start > 0 {
            buf.drain(..start);
            start = 0;
        }
        let have = buf.len();
        buf.resize(have + CHUNK, 0);
        let n = match f.read(&mut buf[have..]) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => 0,
            Err(e) => {
                buf.truncate(have);
                return Err(e);
            }
        };
        buf.truncate(have + n);
        if n == 0 {
            return Ok(()); // what's left is a line still being written
        }
        // Only the new bytes can hold the next newline.
        let mut from = have;
        while let Some(i) = memchr::memchr(b'\n', &buf[from..]) {
            let end = from + i + 1;
            t.offset += (end - start) as u64;
            consume(t, &buf[start..end]);
            start = end;
            from = end;
        }
    }
}

/// Folds one transcript line into t.
pub fn consume(t: &mut Totals, b: &[u8]) {
    if contains(b, COMPACT) && contains(b, br#""subtype":"compact_boundary""#) {
        t.compacts += 1;
        return;
    }
    // Entering or leaving a worktree moves the transcript and says where:
    // the session works there from now on, before its next reply says so.
    if contains(b, RELOCATED) {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R<'a> {
            #[serde(rename = "relocatedCwd", borrow)]
            cwd: Str<'a>,
            #[serde(borrow)]
            timestamp: Str<'a>,
        }
        if let Ok(r) = serde_json::from_slice::<R>(b)
            && !r.cwd.is_empty()
        {
            t.cwd = r.cwd.to_string();
            t.wrote = false;
            t.set_dir(&r.cwd, ms(&r.timestamp));
        }
        return;
    }
    if !contains(b, ASSISTANT) {
        return;
    }
    let Ok(l) = serde_json::from_slice::<Line>(b) else {
        return;
    };
    if &*l.typ != "assistant" {
        return;
    }
    let ts = ms(&l.timestamp);
    if !l.cwd.is_empty() {
        if *l.cwd != *t.cwd {
            t.cwd = l.cwd.to_string();
            if !t.wrote {
                t.set_dir(&l.cwd, ts);
            }
        }
        if t.dirs.len() < 64 && t.dirs.last().is_none_or(|d| *d != *l.cwd) {
            add_unique(&mut t.dirs, &l.cwd);
        }
    }
    let m = &l.message;
    if contains(b, TOOL_USE) {
        let (d, wrote) = worked_in(m.content);
        if !d.is_empty() && (wrote || !t.wrote) {
            t.wrote = wrote;
            t.set_dir(&d, ts);
            if t.dirs.len() < 64 {
                add_unique(&mut t.dirs, &d);
            }
        }
    }
    if ts != 0 {
        if t.first == 0 {
            t.first = ts;
        }
        t.last = ts;
    }
    if contains(b, b"/pull/") {
        pr_urls(b, |u| add_unique(&mut t.prs, u));
    }
    let real = !m.model.is_empty() && &*m.model != "<synthetic>";
    if l.api_error && !l.error.is_empty() {
        t.halt = Some(Halt {
            kind: l.error.to_string(),
            text: first_text(m.content),
            at: ts,
        });
    } else if real {
        t.halt = None; // it answered after all
        if contains(b, "→".as_bytes()) || contains(b, b"->") {
            let p = progress_in(m.content);
            if !p.is_empty() {
                t.progress = p;
                t.progress_at = ts;
            }
        }
    }
    let Some(u) = &m.usage else {
        return;
    };
    if !real {
        return;
    }
    if *m.id != *t.pending_id {
        t.commit_pending();
    }
    let mut tu = TokenUsage {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_read: u.cache_read_input_tokens,
        ..Default::default()
    };
    match &u.cache_creation {
        Some(cb) if cb.ephemeral_5m_input_tokens + cb.ephemeral_1h_input_tokens > 0 => {
            tu.cache_write_5m = cb.ephemeral_5m_input_tokens;
            tu.cache_write_1h = cb.ephemeral_1h_input_tokens;
        }
        _ => tu.cache_write_5m = u.cache_creation_input_tokens,
    }
    if *m.id != *t.pending_id {
        t.pending_id = m.id.to_string();
    }
    if *m.model != *t.pending_model {
        t.pending_model = m.model.to_string();
    }
    t.pending_use = tu;
    t.pending_fast = &*u.speed == "fast";
    t.pending_at = ts;
    if *m.model != *t.last_model {
        t.last_model = m.model.to_string();
    }
}

/// Calls f with each pull request URL in b:
/// `https://github\.com/[\w.-]+/[\w.-]+/pull/\d+`.
fn pr_urls(b: &[u8], mut f: impl FnMut(&str)) {
    const PRE: &[u8] = b"https://github.com/";
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'-';
    let mut at = 0;
    while let Some(i) = memchr::memmem::find(&b[at..], PRE) {
        let s = at + i;
        let mut j = s + PRE.len();
        let mut ok = true;
        for _ in 0..2 {
            let k = j;
            while j < b.len() && word(b[j]) {
                j += 1;
            }
            if j == k || j >= b.len() || b[j] != b'/' {
                ok = false;
                break;
            }
            j += 1;
        }
        if ok && b[j..].starts_with(b"pull/") {
            j += 5;
            let k = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > k {
                // Every byte matched is ASCII.
                f(std::str::from_utf8(&b[s..j]).unwrap_or_default());
                at = j;
                continue;
            }
        }
        at = s + 1;
    }
}

/// HTTP statuses: a pair of them is a status changing, not a count.
fn http_code(s: &str) -> bool {
    matches!(
        s,
        "200"
            | "201"
            | "204"
            | "301"
            | "302"
            | "304"
            | "400"
            | "401"
            | "403"
            | "404"
            | "409"
            | "422"
            | "429"
            | "500"
            | "502"
            | "503"
    )
}

/// A count at b[i..], when one starts there: `(?:\d[\d,.]*)?\dk?`, its end.
fn count_at(b: &[u8], i: usize) -> Option<usize> {
    if i >= b.len() || !b[i].is_ascii_digit() {
        return None;
    }
    let mut j = i;
    while j < b.len() && (b[j].is_ascii_digit() || b[j] == b',' || b[j] == b'.') {
        j += 1;
    }
    while !b[j - 1].is_ascii_digit() {
        j -= 1;
    }
    if j < b.len() && b[j] == b'k' {
        j += 1;
    }
    Some(j)
}

fn space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

/// A run of bytes, start to end.
type Span = (usize, usize);

/// A count going somewhere at b[i..]: "11,065 → 9,052", "22->18". The
/// match's end, and both counts' spans. Ported from Go's
/// `((?:\d[\d,.]*)?\dk?)\**\s?(?:→|->)\s?\**~?((?:\d[\d,.]*)?\dk?)`.
fn pair_at(b: &[u8], i: usize) -> Option<(usize, Span, Span)> {
    let e1 = count_at(b, i)?;
    let mut j = e1;
    while j < b.len() && b[j] == b'*' {
        j += 1;
    }
    let arrow = |j: usize| {
        if b[j..].starts_with("→".as_bytes()) {
            Some(j + 3)
        } else if b[j..].starts_with(b"->") {
            Some(j + 2)
        } else {
            None
        }
    };
    let mut k = arrow(j);
    if k.is_none() && j < b.len() && space(b[j]) {
        k = arrow(j + 1);
    }
    let mut j = k?;
    let after = |mut j: usize| {
        while j < b.len() && b[j] == b'*' {
            j += 1;
        }
        if j < b.len() && b[j] == b'~' {
            j += 1;
        }
        j
    };
    let mut s2 = after(j);
    let mut e2 = count_at(b, s2);
    if e2.is_none() && j < b.len() && space(b[j]) {
        j += 1;
        s2 = after(j);
        e2 = count_at(b, s2);
    }
    let e2 = e2?;
    Some((e2, (i, e1), (s2, e2)))
}

/// The sentence of a message's text with its last count going from one
/// number to another, trimmed to that pair and a little before it.
fn progress_in(content: Option<&RawValue>) -> String {
    let Some(blocks) = blocks_of(content) else {
        return String::new();
    };
    let mut best = String::new();
    for bl in &blocks {
        if &*bl.typ != "text" {
            continue;
        }
        for line in bl.text.split('\n') {
            let b = line.as_bytes();
            let mut i = 0;
            while i < b.len() {
                let Some((end, (f0, f1), (t0, t1))) = pair_at(b, i) else {
                    i += 1;
                    continue;
                };
                let next = end;
                if http_code(&line[f0..f1]) && http_code(&line[t0..t1]) {
                    i = next;
                    continue;
                }
                let loc0 = i;
                let mut start = line[..loc0]
                    .rfind(['.', ';', '!', '?', '|'])
                    .map_or(0, |p| p + 1);
                if loc0 - start > 72 {
                    start = loc0 - 72;
                    while !line.is_char_boundary(start) {
                        start += 1;
                    }
                    if let Some(sp) = line[start..loc0].find(' ') {
                        start += sp + 1;
                    }
                }
                let s = tidy(&line[start..end]);
                if !s.is_empty() {
                    best = s;
                }
                i = next;
            }
        }
    }
    best
}

/// A progress sentence without markdown: no bold or code marks, table bars
/// as spaces, leading bullets gone, spaces collapsed.
fn tidy(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match c {
            '*' if s[i + 1..].starts_with('*') => {
                it.next();
            }
            '`' => {}
            '|' => r.push(' '),
            _ => r.push(c),
        }
    }
    let t = r.trim_start_matches(['-', '*', '#', '>', ' ', ':']);
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The folder a message's tool calls last worked in: a Bash command's
/// leading `cd /abs`, or the folder of a file it wrote.
fn worked_in(content: Option<&RawValue>) -> (String, bool) {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct B<'a> {
        #[serde(rename = "type", borrow)]
        typ: Str<'a>,
        #[serde(borrow)]
        name: Str<'a>,
        #[serde(borrow, deserialize_with = "nd")]
        input: I<'a>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct I<'a> {
        #[serde(borrow)]
        command: Str<'a>,
        #[serde(borrow)]
        file_path: Str<'a>,
        #[serde(borrow)]
        notebook_path: Str<'a>,
    }
    let Some(c) = content else {
        return (String::new(), false);
    };
    let Ok(blocks) = serde_json::from_str::<Vec<B>>(c.get()) else {
        return (String::new(), false);
    };
    let (mut dir, mut wrote) = (String::new(), false);
    for b in &blocks {
        if &*b.typ != "tool_use" {
            continue;
        }
        let d = match &*b.name {
            "Bash" => cd_target(&b.input.command).to_string(),
            "Edit" | "Write" | "MultiEdit" => dir_of(&b.input.file_path).to_string(),
            "NotebookEdit" => dir_of(&b.input.notebook_path).to_string(),
            _ => String::new(),
        };
        let bash = &*b.name == "Bash";
        if work_folder(&d) && (!bash || !wrote) {
            dir = d;
            wrote = !bash;
        }
    }
    (dir, wrote)
}

/// The folder a command starts by cd'ing into, if it does.
fn cd_target(cmd: &str) -> &str {
    let Some(rest) = cmd.trim().strip_prefix("cd ") else {
        return "";
    };
    let rest = rest.trim();
    if let Some(q) = rest.chars().next().filter(|&q| q == '"' || q == '\'') {
        return match rest[1..].find(q) {
            Some(end) => &rest[1..end + 1],
            None => "",
        };
    }
    match rest.find([' ', '\t', '\n', ';', '&', '|', ')']) {
        Some(i) => &rest[..i],
        None => rest,
    }
}

/// filepath.Dir for the absolute paths it's given here: "" is ".".
fn dir_of(p: &str) -> &str {
    match p.rfind('/') {
        None => ".",
        Some(i) => {
            let d = p[..i].trim_end_matches('/');
            if d.is_empty() { "/" } else { d }
        }
    }
}

/// A folder that says where a session works: absolute, and not scratch
/// space or Claude Code's own (memory, plans), which any session writes to
/// wherever it works.
fn work_folder(d: &str) -> bool {
    if !d.starts_with('/') {
        return false;
    }
    let slashed = format!("{d}/");
    for p in [
        "/tmp/",
        "/private/tmp/",
        "/var/folders/",
        "/private/var/folders/",
    ] {
        if slashed.starts_with(p) {
            return false;
        }
    }
    !slashed.contains("/.claude/") || d.contains("/.claude/worktrees/")
}

/// The first line of a message's first text block.
fn first_text(content: Option<&RawValue>) -> String {
    let Some(blocks) = blocks_of(content) else {
        return String::new();
    };
    for b in &blocks {
        if &*b.typ == "text" && !b.text.is_empty() {
            let t = b.text.trim();
            return t.split('\n').next().unwrap_or("").to_string();
        }
    }
    String::new()
}

fn add_unique(s: &mut Vec<String>, v: &str) {
    if !s.iter().any(|x| x == v) {
        s.push(v.to_string());
    }
}

/// The folder a session's subagents keep their transcripts in.
pub fn subagents_dir(main: &Path) -> PathBuf {
    let s = main.to_string_lossy();
    let base = s.strip_suffix(".jsonl").unwrap_or(&s);
    Path::new(base).join("subagents")
}

/// The transcripts of subagents a session spawned, sorted.
pub fn subagent_transcripts(main: &Path) -> Vec<PathBuf> {
    let Ok(ents) = std::fs::read_dir(subagents_dir(main)) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = ents
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    out.sort();
    out
}

/// A session's subagents from their metadata files: how many it ever
/// spawned, and how many are running (written in the last 90s) directly
/// and at any depth.
pub fn read_subagent_stats(main: &Path, now: Ms) -> SubagentStats {
    let mut st = SubagentStats::default();
    let Ok(ents) = std::fs::read_dir(subagents_dir(main)) else {
        return st;
    };
    for e in ents.flatten() {
        let name = e.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".meta.json")) else {
            continue;
        };
        st.spawned += 1;
        let jsonl = e.path().with_file_name(format!("{stem}.jsonl"));
        match crate::jobs::mtime(&jsonl) {
            Some(m) if now - m <= 90_000 => {}
            _ => continue,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct M {
            #[serde(rename = "spawnDepth", deserialize_with = "nd")]
            depth: i64,
        }
        let m: M = std::fs::read(e.path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if m.depth <= 1 {
            st.direct += 1;
        } else {
            st.nested += 1;
        }
    }
    st
}

/// Reads only the last `window` bytes of a transcript: what it's doing and
/// lately said.
pub fn read_preview(path: &Path, window: u64) -> Preview {
    let mut p = Preview::default();
    let Ok(f) = File::open(path) else {
        return p;
    };
    let Ok(md) = f.metadata() else {
        return p;
    };
    let size = md.len();
    let off = size.saturating_sub(window);
    let mut b = vec![0u8; (size - off) as usize];
    let n = read_at_full(&f, &mut b, off);
    if n.is_err() {
        return p;
    }
    let ls: Vec<&[u8]> = lines(&b).collect();
    p.recent = recent_events(ls.iter().copied(), 40); // enough for the Wall's run of steps
    p.first = first_user(&f, &ls, off);
    let mut i = ls.len();
    while i > 0
        && (p.text.is_empty() || p.tool.is_empty() || p.last_user.is_empty() || p.context == 0)
    {
        i -= 1;
        let Ok(l) = serde_json::from_slice::<Line>(ls[i]) else {
            continue;
        };
        let ts = ms(&l.timestamp);
        if p.at == 0 && ts != 0 {
            p.at = ts;
        }
        if &*l.typ == "user" && p.last_user.is_empty() {
            if let Some(s) = str_of(l.message.content)
                && !s.is_empty()
                && !s.starts_with('<')
            {
                p.last_user = s.into_owned();
            }
            continue;
        }
        if &*l.typ != "assistant" {
            continue;
        }
        let Some(blocks) = blocks_of(l.message.content) else {
            continue;
        };
        if p.model.is_empty() {
            p.model = l.message.model.to_string();
            p.effort = l.effort.to_string();
        }
        if let Some(u) = &l.message.usage
            && p.context == 0
            && &*l.message.model != "<synthetic>"
        {
            p.context = u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
        }
        for bl in blocks.iter().rev() {
            if &*bl.typ == "text" && p.text.is_empty() && !bl.text.trim().is_empty() {
                p.text = bl.text.trim().to_string();
            } else if &*bl.typ == "tool_use" && p.tool.is_empty() {
                let input = bl.input.map_or("", |r| r.get());
                p.tool = bl.name.to_string();
                p.tool_arg = tool_arg(input);
                p.doing = doing(&bl.name, input);
            }
        }
    }
    p
}

/// Reads into b from off until b is full or the file ends; how much.
fn read_at_full(f: &File, b: &mut [u8], off: u64) -> io::Result<usize> {
    let mut n = 0;
    while n < b.len() {
        match f.read_at(&mut b[n..], off + n as u64) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// A tool call's main argument: its description, command, file, pattern…
fn tool_arg(input: &str) -> String {
    #[derive(Deserialize)]
    struct A<'a> {
        #[serde(borrow)]
        description: Option<&'a RawValue>,
        #[serde(borrow)]
        command: Option<&'a RawValue>,
        #[serde(borrow)]
        file_path: Option<&'a RawValue>,
        #[serde(borrow)]
        pattern: Option<&'a RawValue>,
        #[serde(borrow)]
        prompt: Option<&'a RawValue>,
        #[serde(borrow)]
        url: Option<&'a RawValue>,
        #[serde(borrow)]
        query: Option<&'a RawValue>,
    }
    let Ok(a) = serde_json::from_str::<A>(input) else {
        return String::new();
    };
    for v in [
        a.description,
        a.command,
        a.file_path,
        a.pattern,
        a.prompt,
        a.url,
        a.query,
    ] {
        if let Some(s) = str_of(v)
            && !s.is_empty()
        {
            return s.into_owned();
        }
    }
    String::new()
}

/// The first thing the user typed, reading the head of the file when the
/// tail window does not reach back to it.
fn first_user(f: &File, tail: &[&[u8]], off: u64) -> PreviewLine {
    let head;
    let head_lines: Vec<&[u8]>;
    let ls: &[&[u8]] = if off > 0 {
        let mut b = vec![0u8; 256 << 10];
        let n = read_at_full(f, &mut b, 0).unwrap_or(0);
        b.truncate(n);
        head = b;
        head_lines = lines(&head).collect();
        &head_lines
    } else {
        tail
    };
    for l in ls {
        if !contains(l, USER) {
            continue;
        }
        if let Some(e) = recent_events(std::iter::once(*l), 1)
            .into_iter()
            .find(|e| e.role == "user")
        {
            return e;
        }
    }
    PreviewLine::default()
}

/// The last n messages and tool calls of these lines.
pub(crate) fn recent_events<'a>(ls: impl Iterator<Item = &'a [u8]>, n: usize) -> Vec<PreviewLine> {
    let mut out: VecDeque<PreviewLine> = VecDeque::with_capacity(n.min(64) + 1);
    let mut push = |e: PreviewLine| {
        out.push_back(e);
        if out.len() > n {
            out.pop_front();
        }
    };
    for raw in ls {
        if !contains(raw, USER) && !contains(raw, ASSISTANT) {
            continue; // neither said nor done: not worth decoding
        }
        let Ok(l) = serde_json::from_slice::<Line>(raw) else {
            continue;
        };
        let user = &*l.typ == "user";
        if !user && &*l.typ != "assistant" {
            continue;
        }
        let at = ms(&l.timestamp);
        if user && let Some(s) = str_of(l.message.content) {
            if !s.is_empty() && !s.starts_with('<') {
                push(PreviewLine {
                    role: "user".into(),
                    text: s.into_owned(),
                    at,
                    doing: String::new(),
                });
            }
            continue;
        }
        let Some(blocks) = blocks_of(l.message.content) else {
            continue;
        };
        for bl in &blocks {
            if &*bl.typ == "text" && !bl.text.trim().is_empty() {
                let t = bl.text.trim();
                if user {
                    if t.starts_with('<') || t.starts_with('[') || t.contains("</") {
                        continue; // a harness note, not something the user typed
                    }
                    push(PreviewLine {
                        role: "user".into(),
                        text: t.into(),
                        at,
                        doing: String::new(),
                    });
                } else {
                    push(PreviewLine {
                        role: "assistant".into(),
                        text: t.into(),
                        at,
                        doing: String::new(),
                    });
                }
            } else if &*bl.typ == "tool_use" {
                let input = bl.input.map_or("", |r| r.get());
                push(PreviewLine {
                    role: "tool".into(),
                    text: format!("{}\0{}", &*bl.name, tool_arg(input)),
                    at,
                    doing: doing(&bl.name, input),
                });
            }
        }
    }
    out.into()
}

/// Whether a subagent's transcript has a tool call still without its
/// result: it's running a command. Reads its last 256 KB.
pub fn ends_in_tool(path: &Path) -> bool {
    use std::os::unix::fs::FileExt;
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(m) = f.metadata() else { return false };
    let off = m.len().saturating_sub(256 << 10);
    let mut b = vec![0u8; (m.len() - off) as usize];
    if f.read_exact_at(&mut b, off).is_err() {
        return false;
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct L<'a> {
        #[serde(borrow)]
        message: M<'a>,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct M<'a> {
        #[serde(borrow)]
        content: Option<&'a serde_json::value::RawValue>,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct B<'a> {
        #[serde(rename = "type", borrow)]
        typ: Str<'a>,
        #[serde(borrow)]
        id: Str<'a>,
        #[serde(borrow)]
        tool_use_id: Str<'a>,
    }
    // Each call is its own line, and each result: one still open, of
    // several made at once, is a command still running.
    let mut open: HashSet<String> = HashSet::new();
    for ln in lines(&b) {
        if !contains(ln, br#""tool_use""#) && !contains(ln, br#""tool_result""#) {
            continue;
        }
        let Ok(l) = serde_json::from_slice::<L>(ln) else {
            continue;
        };
        let Some(c) = l.message.content else { continue };
        let Ok(blocks) = serde_json::from_str::<Vec<B>>(c.get()) else {
            continue;
        };
        for bl in blocks {
            match &*bl.typ {
                "tool_use" => {
                    open.insert(bl.id.to_string());
                }
                "tool_result" => {
                    open.remove(&*bl.tool_use_id);
                }
                _ => {}
            }
        }
    }
    !open.is_empty()
}

#[cfg(test)]
pub(crate) mod tests;
