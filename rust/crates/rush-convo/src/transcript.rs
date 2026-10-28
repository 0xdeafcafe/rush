//! Following a Claude Code transcript file, so a session rush doesn't run
//! itself draws exactly like one it does. Port of
//! internal/convo/transcript.go (Tail, NewTail, NewTailFrom, History,
//! HistoryFrom, HistoryStart, Read, Fetch, Take, parse, take, told, prompt,
//! joinTexts, cleanPrompt, shellOutput, shellStart/Result, Injected,
//! noteTask, compactSummary, promptPictures), hooks.go:16-40 (hookNotes)
//! and subagents.go:115 (SubagentTail). Light sessions (applyLight) aren't
//! ported yet.
//!
//! The Tail reads; the Session it feeds is the caller's. A worker thread
//! can own the Tail and Fetch, and the UI thread apply each Fresh to its
//! Session in one short step.
//!
//! The lines' framing (prompts, notices, compactions, hooks) is Claude
//! Code's transcript format, read here as Go's convo does; each message
//! line is read by the agent's MessageDecoder (rush_claude::messages) into
//! rush's events, which the Session applies as it would a live session's.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rush_core::adapter::MessageDecoder;
use rush_core::event::{self, Event};
use rush_core::time::{Ms, parse_rfc3339};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::model::{Hook, Item, Picture, Session, task_status};
use crate::text::{attr, between, ends_with_image, first_line, first_non_empty, strip_tags};

/// Starts reading one transcript's messages as rush's events: the native
/// agent's Native::messages, from the registry (Claude Code's is
/// `rush_claude::messages`; `decode(rush_claude::messages)` makes one of
/// it). A Tail starts one, and another when its file is rewritten.
pub type Decode = Arc<dyn Fn() -> Box<dyn MessageDecoder> + Send + Sync>;

/// A Decode of a plain function.
pub fn decode(f: fn() -> Box<dyn MessageDecoder>) -> Decode {
    Arc::new(f)
}

/// What rush's inbox hook puts ahead of a message you sent mid-turn
/// (host/inbox.go:92).
pub const TELL_NOTE: &str = "The user sent you this message directly while you work. Take it into account, then carry on:\n\n";

const READ_BUF: usize = 64 << 10;

/// Follows a transcript file; each read takes in only what's new.
pub struct Tail {
    pub path: PathBuf,
    decode: Decode,
    /// Reads its message lines, knowing the calls their results answer.
    messages: Box<dyn MessageDecoder>,
    off: u64,
    /// An unfinished last line, waiting for the rest of it.
    partial: Vec<u8>,
    /// A subagent's own transcript: its sidechain lines are the story.
    sidechain: bool,
    /// History: only lines from before this.
    before: Ms,
    /// History: read past before, so there's nothing more to take.
    past: bool,
    /// The first line read is the end of one, dropped.
    cut: bool,
    /// It began part way through the file: its Session is Partial.
    partway: bool,
    /// A cwd has been handed on; later lines' needn't be copied.
    cwd_seen: bool,
    /// Once set, a read gives up where it is.
    pub stop: Option<Arc<AtomicBool>>,
}

/// What a transcript gained since its Tail last fetched: whole lines,
/// parsed, not yet applied.
pub struct Fresh {
    /// The file was rewritten: start the conversation over.
    pub reset: bool,
    /// How far into the file the tail has now read.
    pub size: u64,
    partway: bool,
    before: Ms,
    lines: Vec<Parsed>,
}

impl Fresh {
    /// Whether it holds anything to apply.
    pub fn is_empty(&self) -> bool {
        !self.reset && self.lines.is_empty()
    }

    /// Applies what was fetched; says whether anything changed.
    pub fn apply(self, sess: &mut Session) -> bool {
        let mut changed = self.reset;
        if self.reset {
            *sess = Session::new();
        }
        if self.partway {
            sess.partial = true;
        }
        for p in self.lines {
            let (ch, _) = take(sess, p, self.before);
            changed |= ch;
        }
        changed
    }
}

impl Tail {
    pub fn new(path: impl Into<PathBuf>, decode: Decode) -> Tail {
        Tail {
            path: path.into(),
            messages: decode(),
            decode,
            off: 0,
            partial: Vec::new(),
            sidechain: false,
            before: 0,
            past: false,
            cut: false,
            partway: false,
            cwd_seen: false,
            stop: None,
        }
    }

    /// A Tail that starts at the first whole line of the file's last most
    /// bytes, so a long conversation's end is read at once; its Session is
    /// then Partial. Open a session with 2 MB, and read the whole file
    /// later with `history_from`.
    pub fn from_end(path: impl Into<PathBuf>, most: u64, decode: Decode) -> Tail {
        let mut t = Tail::new(path, decode);
        if let Ok(m) = std::fs::metadata(&t.path)
            && m.len() > most
        {
            // A byte early: the line cut short there runs to the first
            // newline, which is that byte when the cut fell between lines.
            t.start_at(m.len() - most);
        }
        t
    }

    /// A Tail of a subagent's own transcript, whose lines are all sidechain.
    pub fn subagent(path: impl Into<PathBuf>, decode: Decode) -> Tail {
        let mut t = Tail::new(path, decode);
        t.sidechain = true;
        t
    }

    fn start_at(&mut self, from: u64) {
        self.off = from - 1;
        self.cut = true;
        self.partway = true;
    }

    /// How far into the file it has read.
    pub fn size(&self) -> u64 {
        self.off
    }

    fn reset(&mut self) {
        self.messages = (self.decode)();
        self.off = 0;
        self.partial = Vec::new();
        self.cut = false;
        self.partway = false;
        self.cwd_seen = false;
    }

    fn stopped(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|s| s.load(Ordering::Relaxed))
    }

    /// Applies whatever has been appended since the last call and says
    /// whether anything changed. Nothing new costs one stat.
    pub fn read(&mut self, sess: &mut Session) -> io::Result<bool> {
        let size = std::fs::metadata(&self.path)?.len();
        if size < self.off {
            // Rewritten from scratch: start over, in place.
            self.reset();
            *sess = Session::new();
        }
        if self.partway {
            sess.partial = true;
        }
        if size == self.off {
            return Ok(false);
        }
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.off))?;
        let mut buf = vec![0u8; READ_BUF];
        let mut changed = false;
        loop {
            let n = match f.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => 0,
            };
            self.off += n as u64;
            let mut chunk = &buf[..n];
            while !chunk.is_empty() {
                let Some(i) = memchr(b'\n', chunk) else {
                    // An unfinished last line waits for the rest of it.
                    self.partial.extend_from_slice(chunk);
                    break;
                };
                let line = &chunk[..i + 1];
                chunk = &chunk[i + 1..];
                let ch = if self.partial.is_empty() {
                    self.line(sess, line)
                } else {
                    // Its storage serves the next long line of this read.
                    let mut whole = std::mem::take(&mut self.partial);
                    whole.extend_from_slice(line);
                    let ch = self.line(sess, &whole);
                    whole.clear();
                    self.partial = whole;
                    ch
                };
                changed |= ch;
                if self.past {
                    self.let_go();
                    return Ok(changed);
                }
            }
            if n == 0 || self.stopped() {
                break;
            }
        }
        self.let_go();
        Ok(changed)
    }

    fn line(&mut self, sess: &mut Session, line: &[u8]) -> bool {
        if self.cut {
            self.cut = false;
            return false;
        }
        let b = line.trim_ascii();
        if b.is_empty() {
            return false;
        }
        let Some(p) = self.parse(b) else { return false };
        let (changed, past) = take(sess, p, self.before);
        self.past = past;
        changed
    }

    /// Drops the long-line storage when no unfinished line is in it: a
    /// tail mostly sits idle, and hundreds each holding a buffer add up.
    fn let_go(&mut self) {
        if self.partial.is_empty() {
            self.partial = Vec::new();
        }
    }

    /// Reads what the file has gained and parses it, without applying it:
    /// it touches only where the tail is in the file, so it can run off
    /// the UI's thread. `Fresh::apply` then applies it.
    pub fn fetch(&mut self) -> io::Result<Fresh> {
        let mut fresh = Fresh {
            reset: false,
            size: self.off,
            partway: false,
            before: self.before,
            lines: Vec::new(),
        };
        let size = std::fs::metadata(&self.path)?.len();
        if size < self.off {
            self.reset();
            fresh.reset = true;
        }
        fresh.partway = self.partway;
        if size == self.off {
            fresh.size = self.off;
            return Ok(fresh);
        }
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.off))?;
        let have = self.partial.len();
        let mut buf = std::mem::take(&mut self.partial);
        buf.resize(have + (size - self.off) as usize, 0);
        let mut got = 0;
        while have + got < buf.len() {
            match f.read(&mut buf[have + got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    buf.truncate(have);
                    self.partial = buf;
                    return Err(e);
                }
            }
        }
        buf.truncate(have + got);
        self.off += got as u64;
        fresh.size = self.off;
        // An unfinished last line waits for the rest of it.
        let whole = buf.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);
        self.partial = buf[whole..].to_vec();
        let mut rest = &buf[..whole];
        while !rest.is_empty() {
            let i = memchr(b'\n', rest).unwrap_or(rest.len() - 1);
            let line = &rest[..i];
            rest = &rest[i + 1..];
            if self.cut {
                self.cut = false;
                continue;
            }
            let b = line.trim_ascii();
            if b.is_empty() {
                continue;
            }
            if let Some(p) = self.parse(b) {
                fresh.lines.push(p);
            }
        }
        Ok(fresh)
    }

    /// Decodes a line for take; None when it's not one to take.
    fn parse(&mut self, b: &[u8]) -> Option<Parsed> {
        let l: TLine = serde_json::from_slice(b).ok()?;
        if l.is_sidechain != self.sidechain || l.is_meta {
            return None;
        }
        let mut p = Parsed {
            at: parse_rfc3339(&l.timestamp),
            cwd: None,
            effort: None,
            line: Line::Other,
        };
        if !self.cwd_seen && !l.cwd.is_empty() {
            self.cwd_seen = true;
            p.cwd = Some(l.cwd.as_ref().into());
        }
        if !l.effort.is_empty() {
            p.effort = Some(l.effort.as_ref().into());
        }
        match &*l.typ {
            "system" => {
                p.line = match &*l.subtype {
                    "turn_duration" => Line::TurnDone,
                    "informational" | "local_command" => Line::Notice {
                        text: l
                            .content
                            .and_then(|c| serde_json::from_str::<String>(c.get()).ok())
                            .unwrap_or_default(),
                        level: l.level.as_ref().into(),
                    },
                    "compact_boundary" => {
                        let c = l.compact.unwrap_or_default();
                        Line::Compact(event::Compacted {
                            trigger: c.trigger,
                            before: c.pre_tokens,
                            after: c.post_tokens,
                        })
                    }
                    _ => Line::Other,
                };
            }
            typ @ ("user" | "assistant") => {
                let user = typ == "user";
                if user {
                    #[derive(Deserialize)]
                    struct M<'a> {
                        #[serde(default, borrow)]
                        content: Option<&'a RawValue>,
                    }
                    let content = l
                        .message
                        .and_then(|m| serde_json::from_str::<M>(m.get()).ok())
                        .and_then(|m| m.content);
                    if let Some(c) = content
                        && let Some((text, images)) = prompt(c.get())
                    {
                        let pictures = if images.is_empty() {
                            Vec::new()
                        } else {
                            prompt_pictures(c.get())
                        };
                        let note = c.get().contains("<task-id>").then(|| raw_text(c.get()));
                        p.line = Line::Prompt {
                            text,
                            images,
                            pictures,
                            note,
                        };
                        return Some(p);
                    }
                }
                let events = l.message.map_or_else(Vec::new, |m| {
                    self.messages.message(typ, m, l.tool_use_result)
                });
                p.line = Line::Message {
                    assistant: !user,
                    events,
                };
            }
            "attachment" => {
                let a = l.attachment.unwrap_or_default();
                let content = a.content.map_or("", RawValue::get);
                let told = told(&a.typ, content);
                let hooks = if a.typ == "hook_success" || a.typ == "hook_additional_context" {
                    hook_notes(&a.hook_event, content)
                } else {
                    Vec::new()
                };
                p.line = Line::Attachment { told, hooks };
            }
            _ => {}
        }
        Some(p)
    }
}

/// A transcript line as parse decoded it, read without touching the
/// Session.
struct Parsed {
    at: Ms,
    cwd: Option<Box<str>>,
    effort: Option<Box<str>>,
    line: Line,
}

enum Line {
    /// Nothing to take, but counted.
    Other,
    TurnDone,
    Notice {
        text: String,
        level: Box<str>,
    },
    Compact(event::Compacted),
    /// Something you typed.
    Prompt {
        text: String,
        images: Vec<Box<str>>,
        pictures: Vec<Picture>,
        /// The text as sent, when it tells of a task.
        note: Option<String>,
    },
    /// A message as a live session sends it; none when it didn't decode.
    Message {
        assistant: bool,
        events: Vec<Event>,
    },
    Attachment {
        told: Option<String>,
        hooks: Vec<Hook>,
    },
}

#[derive(Deserialize)]
struct TLine<'a> {
    #[serde(rename = "type", default, borrow)]
    typ: Cow<'a, str>,
    #[serde(default, borrow)]
    subtype: Cow<'a, str>,
    #[serde(rename = "isMeta", default)]
    is_meta: bool,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    #[serde(default, borrow)]
    timestamp: Cow<'a, str>,
    #[serde(default, borrow)]
    message: Option<&'a RawValue>,
    #[serde(rename = "toolUseResult", default, borrow)]
    tool_use_result: Option<&'a RawValue>,
    #[serde(default, borrow)]
    cwd: Cow<'a, str>,
    #[serde(default, borrow)]
    effort: Cow<'a, str>,
    #[serde(default, borrow)]
    content: Option<&'a RawValue>,
    #[serde(default, borrow)]
    level: Cow<'a, str>,
    #[serde(default, borrow)]
    attachment: Option<Attachment<'a>>,
    #[serde(rename = "compactMetadata", default)]
    compact: Option<CompactMeta>,
}

#[derive(Deserialize, Default)]
struct Attachment<'a> {
    #[serde(rename = "type", default, borrow)]
    typ: Cow<'a, str>,
    #[serde(rename = "hookEvent", default, borrow)]
    hook_event: Cow<'a, str>,
    #[serde(default, borrow)]
    content: Option<&'a RawValue>,
}

#[derive(Deserialize, Default)]
struct CompactMeta {
    #[serde(default)]
    trigger: String,
    #[serde(rename = "preTokens", default)]
    pre_tokens: i64,
    #[serde(rename = "postTokens", default)]
    post_tokens: i64,
}

fn done() -> Event {
    Event::TurnEnd(event::TurnEnd {
        reason: "done".into(),
        ..event::TurnEnd::default()
    })
}

/// Applies a parsed line to the Session: whether anything changed, and
/// whether it's well past before, so there's nothing more to read.
fn take(s: &mut Session, p: Parsed, before: Ms) -> (bool, bool) {
    s.count_applied(); // what follows may change it without apply
    let at = p.at;
    if before != 0 && at != 0 && at >= before {
        // A transcript is written in order: once well past before (a
        // minute, for lines written a little out of it), the rest is after
        // it too, and needn't be read.
        return (false, at > before + 60_000);
    }
    if let Some(cwd) = &p.cwd {
        s.set_cwd(cwd);
    }
    let changed = match p.line {
        Line::Other => false,
        Line::TurnDone => {
            s.apply(&done(), at);
            true
        }
        Line::Notice { text, level } => {
            // The agent's program telling you something (an unknown
            // command, a warning): shown where it happened.
            if text.trim().is_empty() {
                false
            } else {
                s.notice(strip_tags(&text), &level, at);
                true
            }
        }
        Line::Compact(c) => {
            s.apply(&Event::Compacted(c), at);
            true
        }
        Line::Prompt {
            text,
            images,
            pictures,
            note,
        } => {
            take_prompt(s, text, images, pictures, note, p.effort, at);
            true
        }
        Line::Message { assistant, events } => {
            if events.is_empty() {
                false
            } else {
                if assistant
                    && let Some(e) = &p.effort
                    && let Some(live) = s.live_turn_mut()
                    && live.effort.is_empty()
                {
                    live.effort = e.clone();
                }
                for ev in &events {
                    s.apply(ev, at);
                }
                true
            }
        }
        Line::Attachment { told, hooks } => {
            // A message rush's inbox handed over mid-turn: yours, as sent.
            let was = told.is_some();
            if let Some(text) = told {
                s.sent(text, Vec::new(), at);
            }
            if !hooks.is_empty() {
                s.add_hooks(hooks);
                true
            } else {
                was
            }
        }
    };
    (changed, false)
}

fn take_prompt(
    s: &mut Session,
    mut text: String,
    images: Vec<Box<str>>,
    pictures: Vec<Picture>,
    note: Option<String>,
    effort: Option<Box<str>>,
    at: Ms,
) {
    // A shell command you ran with ! is its own small turn, and its output
    // lands on it rather than starting another.
    if let Some((out, err)) = shell_output(&text) {
        if let Some(live) = s.live()
            && live.prompt.starts_with("! ")
        {
            let ti = s.turns.len() - 1;
            s.shell_result(ti, out, err, at);
        }
        return;
    }
    let trimmed = text.trim();
    // The summary a compaction leaves goes with its divider.
    if trimmed.starts_with("This session is being continued from a previous conversation")
        && s.compact_summary(&text)
    {
        return;
    }
    // You stopped it: that ends the turn, it isn't a new one.
    if trimmed.starts_with("[Request interrupted by user") {
        s.interrupted(at);
        return;
    }
    // A new prompt closes a turn the transcript never marked done, and the
    // one a Partial session began part way through.
    if let Some(live) = s.live()
        && (!live.prompt.is_empty() || s.partial && live.n == 1)
    {
        s.apply(&done(), at);
    }
    let mut from = None;
    if let Some((f, t)) = injected(&text) {
        from = Some(f);
        text = t;
        if let Some(n) = &note {
            s.note_task(n);
        }
    }
    let shell = text.strip_prefix("! ").map(str::to_owned);
    let (ti, item) = s.send(text, images, at, false);
    if !pictures.is_empty() {
        match item.and_then(|i| s.turns[ti].items.get_mut(i)) {
            Some(Item::Interject(it)) => it.pictures = pictures,
            _ => s.turns[ti].pictures = pictures,
        }
    }
    if let Some(f) = from
        && let Some(t) = s.turns.last_mut()
    {
        t.from = f;
    }
    if let Some(cmd) = shell {
        s.shell_start(&cmd, at);
    }
    if let Some(e) = effort
        && let Some(t) = s.turns.last_mut()
    {
        t.effort = e;
    }
}

/// The message rush's inbox hook handed the agent, from the attachment
/// Claude Code keeps of what the hook added.
fn told(kind: &str, content: &str) -> Option<String> {
    if kind != "hook_additional_context" {
        return None;
    }
    let texts: Vec<String> = serde_json::from_str(content).ok()?;
    texts
        .into_iter()
        .find_map(|t| t.strip_prefix(TELL_NOTE).map(str::to_owned))
}

/// The texts a start hook added, from a transcript's attachment: what it
/// printed, or the context it returned.
fn hook_notes(event: &str, content: &str) -> Vec<Hook> {
    if event != "SessionStart" && event != "SubagentStart" {
        return Vec::new();
    }
    let texts: Vec<String> = match serde_json::from_str::<String>(content) {
        Ok(one) => vec![one],
        Err(_) => match serde_json::from_str(content) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        },
    };
    texts
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty() && !t.starts_with('{'))
        .map(|t| Hook {
            event: event.into(),
            text: t.into(),
        })
        .collect()
}

#[derive(Deserialize)]
struct PromptBlock {
    #[serde(rename = "type", default)]
    typ: String,
    #[serde(default)]
    text: String,
}

/// A user line's content read as something you typed: plain text, or text
/// and image blocks, not a tool result. Command wrappers are unwrapped.
pub fn prompt(raw: &str) -> Option<(String, Vec<Box<str>>)> {
    if raw.starts_with('"') {
        let s: String = serde_json::from_str(raw).ok()?;
        return (!s.trim().is_empty()).then(|| (clean_prompt(&s), Vec::new()));
    }
    let blocks: Vec<PromptBlock> = serde_json::from_str(raw).ok()?;
    if blocks.is_empty() {
        return None;
    }
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for b in &blocks {
        match b.typ.as_str() {
            "text" => texts.push(b.text.as_str()),
            "image" => images.push("image".into()),
            _ => return None, // a tool result
        }
    }
    Some((clean_prompt(&join_texts(&texts)), images))
}

/// The text of a user line's content as sent: the string, or its blocks'
/// texts run together.
fn raw_text(raw: &str) -> String {
    if let Ok(s) = serde_json::from_str::<String>(raw) {
        return s;
    }
    let blocks: Vec<PromptBlock> = serde_json::from_str(raw).unwrap_or_default();
    blocks.into_iter().map(|b| b.text).collect()
}

/// Puts a message's text blocks back together. Text split around an image
/// at its [Image #N] marker was one text, so its pieces join as they were;
/// other blocks are separate paragraphs.
pub fn join_texts(texts: &[&str]) -> String {
    let mut b = String::new();
    for (i, t) in texts.iter().enumerate() {
        if i > 0 && !ends_with_image(texts[i - 1]) {
            b.push('\n');
        }
        b.push_str(t);
    }
    b
}

/// Drops the markup Claude Code wraps around slash commands and system
/// notes, keeping what you actually said.
pub fn clean_prompt(s: &str) -> String {
    let cmd = between(s, "<bash-input>", "</bash-input>");
    if !cmd.is_empty() {
        return format!("! {cmd}");
    }
    if s.contains("<bash-stdout>") || s.contains("<bash-stderr>") {
        return s.to_owned(); // kept whole for shell_output
    }
    let name = between(s, "<command-name>", "</command-name>");
    if !name.is_empty() {
        let args = between(s, "<command-args>", "</command-args>");
        return format!("{name} {args}").trim().to_owned();
    }
    let mut s = Cow::Borrowed(s);
    for tag in [
        "system-reminder",
        "local-command-stdout",
        "local-command-caveat",
    ] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let (Some(i), Some(j)) = (s.find(&open), s.find(&close)) {
            if j < i {
                break;
            }
            s = Cow::Owned(format!("{}{}", &s[..i], &s[j + close.len()..]));
        }
    }
    s.trim().to_owned()
}

/// What Claude Code writes after a ! command: its stdout and stderr.
fn shell_output(s: &str) -> Option<(&str, &str)> {
    if !s.contains("<bash-stdout>") && !s.contains("<bash-stderr>") {
        return None;
    }
    Some((
        between(s, "<bash-stdout>", "</bash-stdout>"),
        between(s, "<bash-stderr>", "</bash-stderr>"),
    ))
}

/// Recognises text Claude Code puts in a user message that you didn't
/// type, and says who it's from and what it says in a line.
pub fn injected(s: &str) -> Option<(String, String)> {
    let t = s.trim();
    if t.starts_with("<task-notification>") {
        let status = task_status(between(t, "<status>", "</status>"));
        let sum = between(t, "<summary>", "</summary>");
        let mut from = "background task".to_owned();
        if !status.is_empty() {
            from = format!("{from} · {status}");
        }
        return Some((
            from,
            first_line(first_non_empty(&[sum, "finished"])).to_owned(),
        ));
    }
    if t.starts_with("<cross-session-message") {
        let name = attr(t, "from-name");
        let body = &t[t.find('>').map_or(t.len(), |i| i + 1)..];
        let body = body.trim();
        let body = body
            .strip_suffix("</cross-session-message>")
            .unwrap_or(body);
        return Some((
            format!(
                "message from {}",
                first_non_empty(&[name, "another session"])
            ),
            first_line(body).to_owned(),
        ));
    }
    if t.starts_with("<rush-pr-news") {
        let (pr, news) = pr_news_parts(t).unwrap_or(("", t));
        let news = news
            .strip_prefix("New since rush last told you:\n")
            .unwrap_or(news);
        let news = news.strip_prefix("- ").unwrap_or(news);
        return Some((
            format!("rush · {}", first_non_empty(&[pr, "pull request"])),
            first_line(news).to_owned(),
        ));
    }
    if t.starts_with("<agent-message") {
        let body = &t[t.find('>').map_or(t.len(), |i| i + 1)..];
        let body = body.trim();
        let body = body.strip_suffix("</agent-message>").unwrap_or(body);
        return Some((
            "a subagent reported back".into(),
            first_line(&strip_tags(body)).to_owned(),
        ));
    }
    if t.starts_with('<') && t.contains("</") {
        // Some other wrapper: drop the tags, keep the words.
        return Some(("Claude Code".into(), first_line(&strip_tags(t)).to_owned()));
    }
    None
}

/// rush's news of a pull request split into the PR it's about and what it
/// says, without the tags and the note to the agent (exchange.go:178).
pub fn pr_news_parts(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim().strip_prefix("<rush-pr-news pr=\"")?;
    let (pr, rest) = rest.split_once("\">")?;
    let rest = rest.trim();
    let mut rest = rest.strip_suffix("</rush-pr-news>").unwrap_or(rest).trim();
    if rest.starts_with("rush is following ") {
        rest = rest.split_once("\n\n").map_or("", |(_, r)| r);
    }
    Some((pr, rest.trim()))
}

#[derive(Deserialize)]
struct PictureBlock {
    #[serde(rename = "type", default)]
    typ: String,
    #[serde(default)]
    source: PictureSource,
}

#[derive(Deserialize, Default)]
struct PictureSource {
    #[serde(default)]
    data: String,
    #[serde(default)]
    media_type: String,
}

/// The images a prompt carried, read once, off the UI: what each is and
/// how big, its base64 never decoded or kept.
fn prompt_pictures(raw: &str) -> Vec<Picture> {
    let Ok(blocks) = serde_json::from_str::<Vec<PictureBlock>>(raw) else {
        return Vec::new();
    };
    blocks
        .into_iter()
        .filter(|b| b.typ == "image")
        .map(|b| Picture {
            media_type: b.source.media_type.into(),
            bytes: base64_len(&b.source.data),
            path: Box::default(),
        })
        .collect()
}

/// How many bytes base64 text decodes to.
pub fn base64_len(s: &str) -> usize {
    let s = s.trim_end();
    let pad = s.bytes().rev().take_while(|&c| c == b'=').count().min(2);
    (s.len() / 4 * 3
        + match s.len() % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        })
    .saturating_sub(pad)
}

/// The conversation a transcript holds from before a moment: what a rush
/// session that resumed one had already said before its host started.
/// Its last turn is closed.
pub fn history(path: impl Into<PathBuf>, before: Ms, decode: Decode) -> Session {
    history_from(path, before, 0, None, decode)
}

/// History read from the first whole line at or after byte from, its
/// Session Partial when that's past the start; once stop is set it gives
/// up, and says nothing.
pub fn history_from(
    path: impl Into<PathBuf>,
    before: Ms,
    from: u64,
    stop: Option<Arc<AtomicBool>>,
    decode: Decode,
) -> Session {
    let mut t = Tail::new(path, decode);
    t.before = before;
    t.stop = stop;
    if from > 0 {
        t.start_at(from);
    }
    let mut s = Session::new();
    if t.read(&mut s).is_err() || t.stopped() {
        return Session::new();
    }
    if let Some(start) = s.live().map(|l| l.start) {
        s.apply(&done(), start);
    }
    s
}

/// Where to read a transcript from for the last most bytes written before
/// at: at's line is found by its timestamps, a handful of small reads, so
/// the rest of the file is never read.
pub fn history_start(path: &Path, at: Ms, most: u64) -> u64 {
    let Ok(f) = File::open(path) else { return 0 };
    let Ok(m) = f.metadata() else { return 0 };
    let (mut lo, mut hi) = (0u64, m.len());
    let mut buf = vec![0u8; 64 << 10];
    while hi - lo > (most / 8).max(64 << 10) {
        let mid = lo + (hi - lo) / 2;
        match stamp_after(&f, mid, &mut buf) {
            Some(t) if t < at => lo = mid,
            _ => hi = mid, // unknown reads as later: the tail starts earlier
        }
    }
    hi.saturating_sub(most)
}

/// The time of the first whole line after byte off that says one, looked
/// for over up to a megabyte: a tool's output can be long.
fn stamp_after(f: &File, mut off: u64, buf: &mut [u8]) -> Option<Ms> {
    for _ in 0..16 {
        let mut r = f;
        let n = match r.seek(SeekFrom::Start(off)) {
            Ok(_) => read_full(&mut r, buf),
            Err(_) => 0,
        };
        let lines: Vec<&[u8]> = buf[..n].split(|&c| c == b'\n').collect();
        // The first may have begun before off, and the last may run on.
        for l in lines.iter().take(lines.len().saturating_sub(1)).skip(1) {
            if let Some(t) = line_stamp(l) {
                return Some(t);
            }
        }
        if n < buf.len() {
            break;
        }
        off += n as u64;
    }
    None
}

/// A transcript line's own time: its last stamp, since the message before
/// it can hold tool inputs that have one.
fn line_stamp(b: &[u8]) -> Option<Ms> {
    const MARK: &[u8] = b"\"timestamp\":\"";
    let k = b.windows(MARK.len()).rposition(|w| w == MARK)?;
    let rest = &b[k + MARK.len()..];
    let e = memchr(b'"', rest)?;
    let t = parse_rfc3339(std::str::from_utf8(&rest[..e]).ok()?);
    (t != 0).then_some(t)
}

/// Reads until buf is full or the file ends.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> usize {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    got
}

fn memchr(c: u8, b: &[u8]) -> Option<usize> {
    b.iter().position(|&x| x == c)
}
