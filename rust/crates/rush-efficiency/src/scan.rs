//! One transcript read into hourly figures, line by line, carrying on
//! from where the last read stopped. Port of internal/efficiency/scan.go.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};

use rush_core::agent::TokenUsage;
use rush_core::tool::{Input, Kind};
use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::source::Source;
use crate::when::hour_of;

// Tool classes: what tool results are counted by. Bash and Read are most
// of what's carried in context; the rest are grouped.
pub const TOOL_BASH: usize = 0;
pub const TOOL_READ: usize = 1;
/// Grep, Glob.
pub const TOOL_SEARCH: usize = 2;
/// Edit, Write, NotebookEdit.
pub const TOOL_EDIT: usize = 3;
pub const TOOL_MCP: usize = 4;
pub const TOOL_OTHER: usize = 5;
pub const N_TOOLS: usize = 6;

pub const TOOL_NAMES: [&str; N_TOOLS] = ["Bash", "Read", "Grep/Glob", "Edit/Write", "MCP", "other"];

/// The class a call of this kind is counted in; name is the agent's own
/// name for the tool, for an MCP tool's.
fn tool_class(k: Kind, name: &str) -> u8 {
    (match k {
        Kind::Shell => TOOL_BASH,
        Kind::Read => TOOL_READ,
        Kind::Search | Kind::Glob => TOOL_SEARCH,
        Kind::Edit | Kind::Write | Kind::Notebook => TOOL_EDIT,
        Kind::Mcp => TOOL_MCP,
        _ if name.starts_with("mcp__") => TOOL_MCP,
        _ => TOOL_OTHER,
    }) as u8
}

fn zero_i(n: &i64) -> bool {
    *n == 0
}
fn zero_i32(n: &i32) -> bool {
    *n == 0
}
fn zero_f(n: &f64) -> bool {
    *n == 0.0
}
fn no(b: &bool) -> bool {
    !*b
}

/// One hour of use: requests, tokens by kind, dollars by kind and the tool
/// output that came back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bucket {
    #[serde(rename = "r", skip_serializing_if = "zero_i")]
    pub req: i64,
    #[serde(rename = "i", skip_serializing_if = "zero_i")]
    pub input: i64,
    #[serde(rename = "o", skip_serializing_if = "zero_i")]
    pub out: i64,
    #[serde(rename = "cr", skip_serializing_if = "zero_i")]
    pub cr: i64,
    #[serde(rename = "cw", skip_serializing_if = "zero_i")]
    pub cw: i64,
    #[serde(rename = "t", skip_serializing_if = "zero_i")]
    pub think: i64,
    /// Cost by what it paid for: input, output, cache read, cache write.
    #[serde(rename = "ci", skip_serializing_if = "zero_f")]
    pub c_in: f64,
    #[serde(rename = "co", skip_serializing_if = "zero_f")]
    pub c_out: f64,
    #[serde(rename = "ccr", skip_serializing_if = "zero_f")]
    pub c_cr: f64,
    #[serde(rename = "ccw", skip_serializing_if = "zero_f")]
    pub c_cw: f64,
    #[serde(rename = "k")]
    pub calls: [i32; N_TOOLS],
    #[serde(rename = "b")]
    pub bytes: [i64; N_TOOLS],
    /// Look is the part of Bash's output that came from searching and
    /// reading code (grep, rg, find, cat, sed -n…): what code-graph and
    /// retrieval savers stand in for.
    #[serde(rename = "lk", skip_serializing_if = "zero_i32")]
    pub look_calls: i32,
    #[serde(rename = "lb", skip_serializing_if = "zero_i")]
    pub look: i64,
}

impl Bucket {
    pub fn cost(&self) -> f64 {
        self.c_in + self.c_out + self.c_cr + self.c_cw
    }

    /// The input side of the requests: what was sent each time.
    pub fn context(&self) -> i64 {
        self.input + self.cr + self.cw
    }

    pub fn add(&mut self, o: &Bucket) {
        self.req += o.req;
        self.input += o.input;
        self.out += o.out;
        self.cr += o.cr;
        self.cw += o.cw;
        self.think += o.think;
        self.c_in += o.c_in;
        self.c_out += o.c_out;
        self.c_cr += o.c_cr;
        self.c_cw += o.c_cw;
        for i in 0..N_TOOLS {
            self.calls[i] += o.calls[i];
            self.bytes[i] += o.bytes[i];
        }
        self.look_calls += o.look_calls;
        self.look += o.look;
    }
}

/// How often something was seen in a transcript, and when first and last:
/// a hook's command, a skill, a slash command, an MCP server, the first
/// word of a shell command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Use {
    pub n: i64,
    #[serde(rename = "f")]
    pub first: Time,
    #[serde(rename = "l")]
    pub last: Time,
}

impl Use {
    fn see(&mut self, at: Time) {
        self.n += 1;
        if self.first.is_zero() || at < self.first {
            self.first = at;
        }
        if at > self.last {
            self.last = at;
        }
    }
}

/// One compaction of the conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Compact {
    #[serde(rename = "a")]
    pub at: Time,
    #[serde(rename = "u", skip_serializing_if = "no")]
    pub auto: bool,
    #[serde(rename = "p")]
    pub pre: i64,
    #[serde(rename = "q")]
    pub post: i64,
}

/// One transcript as far as it has been read. It is serialisable so the
/// next run carries on from offset.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct File {
    #[serde(rename = "off")]
    pub offset: i64,
    pub size: i64,
    #[serde(rename = "acct")]
    pub account: String,
    #[serde(rename = "sid", skip_serializing_if = "String::is_empty")]
    pub session: String,
    /// The folder it first worked in.
    #[serde(rename = "cwd", skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// A subagent's transcript.
    #[serde(skip_serializing_if = "no")]
    pub sub: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,

    pub first: Time,
    pub last: Time,

    /// Unix hour → use.
    #[serde(rename = "h", skip_serializing_if = "HashMap::is_empty")]
    pub hours: HashMap<i64, Bucket>,

    /// The first request's context: what a session starts with.
    #[serde(rename = "sc", skip_serializing_if = "zero_i")]
    pub start_ctx: i64,
    #[serde(rename = "pc", skip_serializing_if = "zero_i")]
    pub peak_ctx: i64,
    #[serde(rename = "cp", skip_serializing_if = "Vec::is_empty")]
    pub compacts: Vec<Compact>,
    /// Tool results over BIG_RESULT bytes.
    #[serde(skip_serializing_if = "zero_i")]
    pub big: i64,

    #[serde(rename = "u", skip_serializing_if = "HashMap::is_empty")]
    pub uses: HashMap<String, Use>,
    /// File → times read.
    #[serde(rename = "rd", skip_serializing_if = "HashMap::is_empty")]
    pub reads: HashMap<String, i64>,

    // The newest assistant message; its usage can still change while more
    // of its blocks are appended, so it's added when the next one starts.
    #[serde(rename = "pi", skip_serializing_if = "String::is_empty")]
    pub pend_id: String,
    #[serde(rename = "pm", skip_serializing_if = "String::is_empty")]
    pub pend_model: String,
    #[serde(rename = "pu")]
    pub pend_use: TokenUsage,
    #[serde(rename = "pt", skip_serializing_if = "zero_i")]
    pub pend_think: i64,
    #[serde(rename = "pf", skip_serializing_if = "no")]
    pub pend_fast: bool,
    #[serde(rename = "pa")]
    pub pend_at: Time,
    /// Tool calls whose results haven't come back yet: id → class, with
    /// LOOK_FLAG for a shell command that searches or reads code.
    #[serde(rename = "op", skip_serializing_if = "HashMap::is_empty")]
    pub open: HashMap<String, u8>,
}

/// A tool result big enough to be worth a finding: about 10k tokens.
pub const BIG_RESULT: i64 = 40_000;

const MAX_USES: usize = 96;
const MAX_READS: usize = 300;
const MAX_OPEN: usize = 256;
const LOOK_FLAG: u8 = 0x80;

fn ctx_of(u: &TokenUsage) -> i64 {
    u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h
}

impl File {
    /// A transcript not yet read, of account's.
    pub fn new(account: &str, sub: bool) -> File {
        File {
            account: account.to_string(),
            sub,
            ..File::default()
        }
    }

    /// The file's use summed over its hours.
    pub fn totals(&self) -> Bucket {
        let mut t = Bucket::default();
        self.each_hour(|_, b| t.add(b));
        t
    }

    fn hour(&mut self, at: Time) -> &mut Bucket {
        self.hours.entry(hour_of(at)).or_default()
    }

    fn use_(&mut self, key: String, at: Time) {
        if !self.uses.contains_key(&key) && self.uses.len() >= MAX_USES {
            return;
        }
        self.uses.entry(key).or_default().see(at);
    }

    /// The pending message as one request's use.
    fn pend_bucket(&self) -> Bucket {
        let (u, model, fast) = (&self.pend_use, self.pend_model.as_str(), self.pend_fast);
        let c = |t: TokenUsage| crate::price::cost(model, &t, fast);
        Bucket {
            req: 1,
            input: u.input,
            out: u.output,
            cr: u.cache_read,
            cw: u.cache_write_5m + u.cache_write_1h,
            think: self.pend_think,
            c_in: c(TokenUsage {
                input: u.input,
                ..TokenUsage::default()
            }),
            c_out: c(TokenUsage {
                output: u.output,
                ..TokenUsage::default()
            }),
            c_cr: c(TokenUsage {
                cache_read: u.cache_read,
                ..TokenUsage::default()
            }),
            c_cw: c(TokenUsage {
                cache_write_5m: u.cache_write_5m,
                cache_write_1h: u.cache_write_1h,
                ..TokenUsage::default()
            }),
            ..Bucket::default()
        }
    }

    /// Adds the pending message: a request, its tokens and what it cost.
    fn commit(&mut self) {
        if self.pend_id.is_empty() {
            return;
        }
        let pb = self.pend_bucket();
        let at = self.pend_at;
        self.hour(at).add(&pb);
        let ctx = pb.context();
        if self.start_ctx == 0 {
            self.start_ctx = ctx;
        }
        self.peak_ctx = self.peak_ctx.max(ctx);
        self.model = std::mem::take(&mut self.pend_model);
        self.pend_id.clear();
        self.pend_use = TokenUsage::default();
        self.pend_think = 0;
        self.pend_fast = false;
    }

    /// The context the session started with, the message still being
    /// written included.
    pub fn start(&self) -> i64 {
        if self.start_ctx == 0 && !self.pend_id.is_empty() {
            return ctx_of(&self.pend_use);
        }
        self.start_ctx
    }

    /// The most context it carried, the message still being written
    /// included.
    pub fn peak(&self) -> i64 {
        let mut p = self.peak_ctx;
        if !self.pend_id.is_empty() {
            p = p.max(ctx_of(&self.pend_use));
        }
        p
    }

    /// Calls f with every hour's use, the message still being written
    /// included, so a running session's figures move.
    pub fn each_hour(&self, mut f: impl FnMut(i64, &Bucket)) {
        let mut pend = None;
        if !self.pend_id.is_empty() {
            pend = Some((hour_of(self.pend_at), self.pend_bucket()));
        }
        for (&h, b) in &self.hours {
            if let Some((ph, pb)) = pend
                && ph == h
            {
                let mut sum = *b;
                sum.add(&pb);
                f(h, &sum);
                pend = None;
                continue;
            }
            f(h, b);
        }
        if let Some((ph, pb)) = pend {
            f(ph, &pb);
        }
    }

    // What a Source records, line by line.

    /// A line written at, by session, in cwd: it keeps the file's span,
    /// and its session and folder from the first line that says them.
    pub fn saw(&mut self, at: Time, session: &str, cwd: &str) {
        if self.first.is_zero() || at < self.first {
            self.first = at;
        }
        if at > self.last {
            self.last = at;
        }
        if self.session.is_empty() {
            self.session = session.to_string();
        }
        if self.project.is_empty() {
            self.project = cwd.to_string();
        }
    }

    /// One model request's use, by its id: a message's usage can still
    /// change while more of it is written, so it's added once the next
    /// one starts.
    pub fn request(
        &mut self,
        id: &str,
        model: &str,
        u: TokenUsage,
        think: i64,
        fast: bool,
        at: Time,
    ) {
        if id != self.pend_id {
            self.commit();
        }
        self.pend_id = id.to_string();
        self.pend_model = model.to_string();
        self.pend_use = u;
        self.pend_at = at;
        self.pend_fast = fast;
        self.pend_think = think;
    }

    /// A tool call of kind k, name being the agent's own name for the
    /// tool; its result is counted when result comes with the same id.
    pub fn call(&mut self, id: &str, k: Kind, name: &str, input: &Input, at: Time) {
        let mut c = tool_class(k, name);
        match k {
            Kind::Shell => {
                if looks(&input.command) {
                    c |= LOOK_FLAG;
                }
                match first_word(&input.command) {
                    (w, true) if !w.is_empty() => self.use_(format!("bash:{w}/admin"), at),
                    (w, _) if !w.is_empty() => self.use_(format!("bash:{w}"), at),
                    _ => {}
                }
            }
            Kind::Read
                if !input.path.is_empty()
                    && (self.reads.contains_key(&input.path) || self.reads.len() < MAX_READS) =>
            {
                *self.reads.entry(input.path.clone()).or_default() += 1;
            }
            _ => {}
        }
        if self.open.len() < MAX_OPEN {
            self.open.insert(id.to_string(), c);
        }
    }

    /// What the call id gave back: n bytes of text.
    pub fn result(&mut self, id: &str, n: i64, at: Time) {
        let mut cl = self.open.remove(id).unwrap_or(TOOL_OTHER as u8);
        let big = n > BIG_RESULT;
        let b = self.hour(at);
        if cl & LOOK_FLAG != 0 {
            cl &= !LOOK_FLAG;
            b.look_calls += 1;
            b.look += n;
        }
        let cl = (cl as usize).min(N_TOOLS - 1);
        b.calls[cl] += 1;
        b.bytes[cl] += n;
        if big {
            self.big += 1;
        }
    }

    /// A use of a skill.
    pub fn skill(&mut self, name: &str, at: Time) {
        self.use_(format!("skill:{name}"), at);
    }
    /// A use of an MCP server.
    pub fn mcp(&mut self, server: &str, at: Time) {
        self.use_(format!("mcp:{server}"), at);
    }
    /// A slash command.
    pub fn command(&mut self, name: &str, at: Time) {
        self.use_(format!("cmd:{name}"), at);
    }
    /// A hook that ran.
    pub fn hook(&mut self, cmd: &str, at: Time) {
        self.use_(format!("hook:{}", hook_key(cmd)), at);
    }

    /// One compaction of the conversation.
    pub fn compacted(&mut self, c: Compact) {
        self.compacts.push(c);
    }
}

/// A hook's command, short enough to keep: agents' hooks are told apart by
/// their program, not their arguments' details.
fn hook_key(cmd: &str) -> &str {
    let cmd = cmd.trim();
    if cmd.len() <= 120 {
        return cmd;
    }
    let mut end = 120;
    while !cmd.is_char_boundary(end) {
        end -= 1;
    }
    &cmd[..end]
}

/// Reads what was appended to path since f.offset, each whole line
/// through src. A half-written line is read next time. buf is a buffer to
/// read lines into, handed back for the next call.
pub fn scan(src: &dyn Source, path: &str, f: &mut File, mut buf: Vec<u8>) -> io::Result<Vec<u8>> {
    let mut fh = fs::File::open(path)?;
    let size = fh.metadata()?.len() as i64;
    if size < f.offset {
        *f = File::new(&f.account, f.sub); // rewritten: read it again
    }
    f.size = size;
    if size == f.offset {
        return Ok(buf);
    }
    fh.seek(SeekFrom::Start(f.offset as u64))?;
    let mut r = BufReader::with_capacity(256 << 10, fh);
    loop {
        buf.clear();
        let n = r.read_until(b'\n', &mut buf)?;
        if n == 0 || buf.last() != Some(&b'\n') {
            return Ok(buf);
        }
        f.offset += buf.len() as i64;
        src.read_line(f, &buf);
    }
}

/// The programs that search or read code: what a code graph answers
/// instead.
const LOOK_PROGRAMS: &[&str] = &[
    "grep", "rg", "ag", "ack", "egrep", "fgrep", "find", "fd", "ls", "tree", "eza", "cat", "bat",
    "head", "tail", "less", "nl", "wc", "sg", "ast-grep",
];

/// Whether a shell command searches or reads code, going by the program of
/// its last step that isn't a cd, echo or true, past rtk and VAR=x, and
/// not redirected to a file. sed and awk count with -n, git with grep,
/// ls-files or show.
pub fn looks(cmd: &str) -> bool {
    let cmd = cmd.replace("&&", ";").replace("||", ";").replace('\n', ";");
    let steps: Vec<&str> = cmd.split(';').filter(|s| !s.is_empty()).collect();
    for step in steps.iter().rev() {
        let step = step.split('|').next().unwrap_or("");
        let mut fields: Vec<&str> = step.split_whitespace().collect();
        while let Some(f) = fields.first()
            && (f.contains('=') || *f == "rtk" || *f == "command" || *f == "env")
        {
            fields.remove(0);
        }
        if fields.is_empty() {
            continue;
        }
        let mut w = fields[0].trim_matches(|c| c == '\'' || c == '"' || c == '(');
        if let Some(j) = w.rfind('/') {
            w = &w[j + 1..];
        }
        let arg = fields.get(1).copied().unwrap_or("");
        if fields[1..]
            .iter()
            .any(|f| f.starts_with('>') || f.starts_with("1>"))
        {
            w = ""; // written to a file, not read back
        }
        match w {
            "cd" | "echo" | "true" | ":" | "printf" | "pushd" | "popd" | "set" | "export" => {
                continue;
            }
            "sed" | "awk" => return arg == "-n",
            "git" => return arg == "grep" || arg == "ls-files" || arg == "show",
            _ => return LOOK_PROGRAMS.contains(&w),
        }
    }
    false
}

/// Words after a program that mean it's being looked after rather than
/// used: installed, set up, asked its version or its figures.
const ADMIN_WORDS: &[&str] = &[
    "init",
    "gain",
    "install",
    "uninstall",
    "setup",
    "help",
    "--help",
    "-h",
    "--version",
    "-V",
    "version",
    "discover",
    "session",
    "verify",
    "telemetry",
    "doctor",
    "config",
];

/// The program a shell command runs, past any VAR=x, and whether it's
/// being looked after rather than used.
pub fn first_word(cmd: &str) -> (String, bool) {
    let fields: Vec<&str> = cmd.split_whitespace().collect();
    for (i, w) in fields.iter().enumerate() {
        if w.contains('=') && !w.starts_with('=') {
            continue;
        }
        let mut w = w.trim_matches(|c| c == '\'' || c == '"' || c == '(');
        if let Some(j) = w.rfind('/') {
            w = &w[j + 1..];
        }
        if w.is_empty() || w.len() > 32 {
            return (String::new(), false);
        }
        let admin = fields.get(i + 1).is_some_and(|n| ADMIN_WORDS.contains(n));
        return (w.to_string(), admin);
    }
    (String::new(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_words() {
        for (cmd, want) in [
            ("rtk git status", "rtk"),
            ("FOO=1 BAR=2 /usr/bin/rtk ls", "rtk"),
            ("'rtk' ls", "rtk"),
            ("", ""),
        ] {
            assert_eq!(first_word(cmd).0, want, "{cmd:?}");
        }
        assert!(
            first_word("rtk --version").1,
            "looking after rtk, not using it"
        );
    }

    #[test]
    fn looking() {
        for (cmd, want) in [
            ("rg -n foo internal", true),
            ("cd /x && grep -rn foo . | head -20", true),
            ("rtk grep foo", true),
            ("sed -n 10,40p a.go", true),
            ("sed -i s/a/b/ a.go", false),
            ("git grep foo", true),
            ("git status", false),
            ("find . -name '*.go' 2>/dev/null || true", true),
            ("go test ./... && echo done", false),
            ("LC_ALL=C cat a.go", true),
            ("cd /x\nls -la", true),
            ("python3 - <<'EOF'", false),
            ("cat > a.go <<'EOF'", false),
        ] {
            assert_eq!(looks(cmd), want, "{cmd:?}");
        }
    }

    #[test]
    fn writes_as_go_does() {
        let mut f = File::new("/acct", true);
        f.hours.insert(
            7,
            Bucket {
                req: 1,
                ..Bucket::default()
            },
        );
        let s = serde_json::to_string(&f).unwrap();
        assert!(
            s.contains(r#""acct":"/acct""#) && s.contains(r#""sub":true"#),
            "{s}"
        );
        assert!(
            s.contains(r#""h":{"7":{"r":1,"k":[0,0,0,0,0,0],"b":[0,0,0,0,0,0]}}"#),
            "{s}"
        );
        assert!(s.contains(r#""first":"0001-01-01T00:00:00Z""#), "{s}");
        let back: File = serde_json::from_str(&s).unwrap();
        assert_eq!(back, f);
    }
}
