//! The Live view's story (DESIGN.md "Live"): what a watcher model writes
//! about a session as it works, and what it's told. The watcher gets only
//! what's new since it was last told (`delta`, a line a step), and answers
//! with a patch (`Story::apply`): a headline, how far along, and blocks by
//! id, each one of a few kinds the Live view draws (render/draw/presets.rs
//! `live`). What's sure (what needs you, checks, changed files, the step
//! running) the view draws from the session itself, never from the story.
//! New in the Rust port: Go rush had no such view.

use std::collections::HashSet;

use rush_core::time::Ms;
use rush_core::tool::Kind;
use serde_json::Value;

use crate::model::{Session, Status, StepId};

/// The most blocks a story holds, and items a block.
pub const MOST_BLOCKS: usize = 6;
pub const MOST_ITEMS: usize = 12;
/// The longest a headline, a title or an item's words may be, in chars.
const MOST_CHARS: usize = 120;
/// The most a delta says, in chars (about 1.5k tokens): past it, the steps
/// in the middle are counted, not listed.
pub const DELTA_CHARS: usize = 6_000;

/// What the watcher is told it is, and how to answer: the same words every
/// time, so every update reads them from the cache.
pub const SYSTEM: &str = r#"You watch a coding agent work and keep a small live dashboard of it for its user, who glances at it instead of reading the conversation. Each message gives you the dashboard as it stands, then only what happened since it was last updated: turns the user started, the agent's steps (one line each), its answers, its task list.

Reply with one JSON object and nothing else, a patch to the dashboard:
{"headline": "...", "progress": [done, total], "blocks": [block, ...], "remove": ["id", ...], "order": ["id", ...]}
Every key is optional; leave out what hasn't changed. A block replaces the one with the same id, or is added.

block: {"id": "short-stable-id", "kind": "steps|timeline|list|kv|bars|callout|text", "title": "...", "tone": "good|bad|warn|busy|quiet", "text": "...", "items": [item, ...]}
item: {"text": "...", "meta": "...", "tone": "...", "done": true, "value": 3, "time": "14:05"}
- steps: the phases of the work, in order; done ones "done": true, the current one "tone": "busy". The main block: what the user is after.
- timeline: what happened when, a few key moments, newest last.
- list: things by state (files to migrate, failing tests, open questions).
- kv: properties of one thing (branch, target, config).
- bars: sizes side by side; "value" is the number, "meta" its words.
- callout: one thing to notice now (a blocker, a risk, a decision made); its words in "text".
- text: a sentence or two of context.

Rules:
- headline: what the agent is doing, for whom, in under 60 characters; no "The agent".
- progress: your best estimate of phases done of phases planned; leave it out when there's no plan to measure.
- At most 6 blocks and 12 items a block; prefer updating blocks to adding them; keep ids stable.
- Don't repeat what the dashboard already shows on its own: what needs the user, which checks passed or failed, which files changed, the step running now.
- Plain words, no markdown. Never invent progress the messages don't show."#;

/// The shape of the watcher's answer, for a harness that can hold it to
/// one: SYSTEM's patch, every key optional.
pub const SCHEMA: &str = r#"{"type":"object","properties":{"headline":{"type":"string"},"progress":{"type":"array","items":{"type":"integer"}},"remove":{"type":"array","items":{"type":"string"}},"order":{"type":"array","items":{"type":"string"}},"blocks":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"kind":{"type":"string","enum":["steps","timeline","list","kv","bars","callout","text"]},"title":{"type":"string"},"tone":{"type":"string"},"text":{"type":"string"},"items":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string"},"meta":{"type":"string"},"tone":{"type":"string"},"done":{"type":"boolean"},"value":{"type":"number"},"time":{"type":"string"}},"required":["text"]}}},"required":["id","kind"]}}}}"#;

/// What the small-job model is told for the Now line: one step in, a few
/// words out.
pub const NOW_SYSTEM: &str = "You say in a few plain words what a coding agent's step is doing, for a status line: under 50 characters, present tense, no quotes, no full stop, no \"The agent\". Reply with the words only.";

/// Between updates while a turn runs, at least this long: the watcher is
/// asked again at a turn's end, and meanwhile only this often.
pub const EVERY: Ms = 20_000;

/// How a block or an item stands, as its colour says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    #[default]
    Plain,
    Good,
    Bad,
    Warn,
    Busy,
    Quiet,
}

impl Tone {
    fn of(s: &str) -> Tone {
        match s {
            "good" => Tone::Good,
            "bad" => Tone::Bad,
            "warn" => Tone::Warn,
            "busy" => Tone::Busy,
            "quiet" => Tone::Quiet,
            _ => Tone::Plain,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Tone::Plain => "",
            Tone::Good => "good",
            Tone::Bad => "bad",
            Tone::Warn => "warn",
            Tone::Busy => "busy",
            Tone::Quiet => "quiet",
        }
    }
}

/// What a block draws as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlockKind {
    #[default]
    List,
    Steps,
    Timeline,
    Kv,
    Bars,
    Callout,
    Text,
}

impl BlockKind {
    fn of(s: &str) -> Option<BlockKind> {
        Some(match s {
            "list" => BlockKind::List,
            "steps" => BlockKind::Steps,
            "timeline" => BlockKind::Timeline,
            "kv" => BlockKind::Kv,
            "bars" => BlockKind::Bars,
            "callout" => BlockKind::Callout,
            "text" => BlockKind::Text,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            BlockKind::List => "list",
            BlockKind::Steps => "steps",
            BlockKind::Timeline => "timeline",
            BlockKind::Kv => "kv",
            BlockKind::Bars => "bars",
            BlockKind::Callout => "callout",
            BlockKind::Text => "text",
        }
    }
}

/// One of a block's rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoryItem {
    pub text: String,
    pub meta: String,
    pub tone: Tone,
    pub done: bool,
    pub value: f64,
    pub time: String,
}

/// A block of the story, by its id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoryBlock {
    pub id: String,
    pub kind: BlockKind,
    pub title: String,
    pub tone: Tone,
    pub text: String,
    pub items: Vec<StoryItem>,
}

/// What the watcher has said of the session so far, and the one line
/// saying what the step running now is doing (the small-job model's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Story {
    pub headline: String,
    /// Phases done of phases planned; (0, 0) when there's no plan.
    pub progress: (u32, u32),
    pub blocks: Vec<StoryBlock>,
    /// The step running now, said plainly, and the step it says.
    pub now: String,
    pub now_of: String,
    /// Why there's no story yet, or what went wrong making it: "reading
    /// the conversation…", "the watcher stopped: …".
    pub note: String,
    /// Bumped on every change: the view's cache key.
    pub serial: u64,
}

impl Story {
    /// Takes in the watcher's answer: a JSON patch, perhaps in a ```json
    /// fence or after some words. What it can't read of a block is left
    /// out; an answer with no JSON at all is an error and changes nothing.
    pub fn apply(&mut self, answer: &str) -> Result<(), String> {
        let v = json_in(answer).ok_or_else(|| "no JSON in the watcher's answer".to_string())?;
        let Value::Object(p) = v else {
            return Err("the watcher's answer isn't a JSON object".into());
        };
        if let Some(h) = p.get("headline").and_then(Value::as_str) {
            self.headline = clip(h);
        }
        if let Some(a) = p.get("progress").and_then(Value::as_array) {
            let n = |i: usize| a.get(i).and_then(Value::as_u64).unwrap_or(0) as u32;
            let (done, total) = (n(0), n(1));
            self.progress = if total == 0 {
                (0, 0)
            } else {
                (done.min(total), total)
            };
        }
        if let Some(rm) = p.get("remove").and_then(Value::as_array) {
            let rm: HashSet<&str> = rm.iter().filter_map(Value::as_str).collect();
            self.blocks.retain(|b| !rm.contains(b.id.as_str()));
        }
        for b in p
            .get("blocks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(b) = block_of(b) else { continue };
            match self.blocks.iter().position(|x| x.id == b.id) {
                Some(i) => self.blocks[i] = b,
                None if self.blocks.len() < MOST_BLOCKS => self.blocks.push(b),
                None => {}
            }
        }
        if let Some(order) = p.get("order").and_then(Value::as_array) {
            let at = |id: &str| {
                order
                    .iter()
                    .position(|o| o.as_str() == Some(id))
                    .unwrap_or(usize::MAX)
            };
            self.blocks.sort_by_key(|b| at(&b.id));
        }
        self.note.clear();
        self.serial += 1;
        Ok(())
    }

    /// Sets the line for the step running now; whether it changed.
    pub fn set_now(&mut self, of: &str, line: &str) -> bool {
        let line = clip(line.trim().trim_matches('"'));
        if self.now == line && self.now_of == of {
            return false;
        }
        self.now = line;
        self.now_of = of.to_string();
        self.serial += 1;
        true
    }

    /// Sets the note; whether it changed.
    pub fn set_note(&mut self, note: &str) -> bool {
        if self.note == note {
            return false;
        }
        self.note = note.to_string();
        self.serial += 1;
        true
    }

    /// The dashboard as the watcher writes it, compact: what it's shown
    /// as standing, so a watcher that remembers nothing can patch it.
    /// Empty fields are left out.
    pub fn to_json(&self) -> String {
        use serde_json::{Map, json};
        let mut p = Map::new();
        if !self.headline.is_empty() {
            p.insert("headline".into(), json!(self.headline));
        }
        if self.progress.1 > 0 {
            p.insert("progress".into(), json!([self.progress.0, self.progress.1]));
        }
        let blocks: Vec<Value> = self
            .blocks
            .iter()
            .map(|b| {
                let mut o = Map::new();
                o.insert("id".into(), json!(b.id));
                o.insert("kind".into(), json!(b.kind.name()));
                for (k, v) in [("title", &b.title), ("text", &b.text)] {
                    if !v.is_empty() {
                        o.insert(k.into(), json!(v));
                    }
                }
                if b.tone != Tone::Plain {
                    o.insert("tone".into(), json!(b.tone.name()));
                }
                if !b.items.is_empty() {
                    let items: Vec<Value> = b.items.iter().map(item_json).collect();
                    o.insert("items".into(), Value::Array(items));
                }
                Value::Object(o)
            })
            .collect();
        if !blocks.is_empty() {
            p.insert("blocks".into(), Value::Array(blocks));
        }
        Value::Object(p).to_string()
    }
}

fn item_json(it: &StoryItem) -> Value {
    use serde_json::{Map, json};
    let mut o = Map::new();
    o.insert("text".into(), json!(it.text));
    for (k, v) in [("meta", &it.meta), ("time", &it.time)] {
        if !v.is_empty() {
            o.insert(k.into(), json!(v));
        }
    }
    if it.tone != Tone::Plain {
        o.insert("tone".into(), json!(it.tone.name()));
    }
    if it.done {
        o.insert("done".into(), json!(true));
    }
    if it.value != 0.0 {
        o.insert("value".into(), json!(it.value));
    }
    Value::Object(o)
}

/// What the watcher is sent: the dashboard as it stands, then what's new
/// (delta's). The same words every time come first, for the cache.
pub fn prompt(story: &Story, delta: &str) -> String {
    let now = story.to_json();
    let now = if now == "{}" {
        "(empty: nothing on it yet)".to_string()
    } else {
        now
    };
    format!("The dashboard now:\n{now}\n\nWhat happened since:\n{delta}")
}

/// The main agent's step running now in s's last turn, if a turn is live.
pub fn running(s: &Session) -> Option<StepId> {
    let t = s.turns.last().filter(|t| t.live)?;
    t.steps.iter().copied().find(|&id| {
        let st = s.step(id);
        st.parent.is_none() && st.kind != Kind::Todo && st.status == Status::Running
    })
}

/// Whether the watcher is due to be asked about s, told up to c and last
/// asked at `asked` (0: never): at once the first time, as soon as a turn
/// has ended it hasn't heard the end of, and while a turn runs every
/// EVERY at most.
pub fn due(s: &Session, c: &Cursor, asked: Ms, now: Ms) -> bool {
    if s.turns.is_empty() {
        return false;
    }
    if !c.started() {
        return true;
    }
    let ended = s
        .turns
        .iter()
        .rev()
        .take_while(|t| !c.answered.contains(&t.n))
        .any(|t| !t.live);
    if ended || s.turns.len() > c.turns {
        return true;
    }
    s.turns.last().is_some_and(|t| t.live) && now - asked >= EVERY
}

/// A block from the watcher's JSON; None without an id or a kind the view
/// draws.
fn block_of(v: &Value) -> Option<StoryBlock> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
    let id = s("id").trim();
    let kind = BlockKind::of(s("kind"))?;
    if id.is_empty() {
        return None;
    }
    let items = v
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MOST_ITEMS)
        .filter_map(|it| {
            let s = |k: &str| it.get(k).and_then(Value::as_str).unwrap_or("");
            let text = clip(s("text"));
            if text.is_empty() {
                return None;
            }
            Some(StoryItem {
                text,
                meta: clip(s("meta")),
                tone: Tone::of(s("tone")),
                done: it.get("done").and_then(Value::as_bool).unwrap_or(false),
                value: it.get("value").and_then(Value::as_f64).unwrap_or(0.0),
                time: clip(s("time")),
            })
        })
        .collect();
    Some(StoryBlock {
        id: clip(id),
        kind,
        title: clip(s("title")),
        tone: Tone::of(s("tone")),
        text: s("text").trim().chars().take(MOST_CHARS * 3).collect(),
        items,
    })
}

/// The JSON object in an answer: the whole of it, a ```json fence's, or
/// from its first { to its last }.
fn json_in(answer: &str) -> Option<Value> {
    let a = answer.trim();
    if let Ok(v) = serde_json::from_str(a) {
        return Some(v);
    }
    let (from, to) = (a.find('{')?, a.rfind('}')?);
    (from < to)
        .then(|| serde_json::from_str(&a[from..=to]).ok())
        .flatten()
}

/// s on one line, MOST_CHARS at most.
fn clip(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= MOST_CHARS {
        one
    } else {
        let mut t: String = one.chars().take(MOST_CHARS - 1).collect();
        t.push('…');
        t
    }
}

/// How far the watcher has been told: the turns it has heard of, the
/// steps it has been told the end of, the answers it has read, and the
/// task list as it last saw it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    turns: usize,
    steps: HashSet<StepId>,
    answered: HashSet<u32>,
    tasks: u64,
}

impl Cursor {
    /// Whether the watcher has been told anything yet.
    pub fn started(&self) -> bool {
        self.turns > 0
    }
}

/// What happened in session s since the watcher was told up to c, as the
/// message it's sent; None when there's nothing new. Covers the last few
/// turns only the first time (`FIRST_TURNS`), and moves c past what it
/// says. Steps are told once they've ended (failed, denied or waiting on
/// you as well); the main agent's only, a subagent's counted in its call.
pub fn delta(s: &Session, c: &mut Cursor, now: Ms) -> Option<String> {
    /// The turns told the first time, the newest.
    const FIRST_TURNS: usize = 3;
    let n = s.turns.len();
    if n == 0 {
        return None;
    }
    let from = if c.turns == 0 {
        n.saturating_sub(FIRST_TURNS)
    } else {
        c.turns.saturating_sub(1)
    };
    let mut lines: Vec<String> = Vec::new();
    let mut steps: Vec<String> = Vec::new();
    for (i, t) in s.turns.iter().enumerate().skip(from) {
        if i >= c.turns {
            let who = if t.from.is_empty() {
                "the user asked".to_string()
            } else {
                format!("{} said", t.from)
            };
            let asked = if t.prompt.trim().is_empty() {
                t.cause.clone()
            } else {
                t.prompt.clone()
            };
            lines.push(format!("Turn {}: {who}: {}", t.n, words(&asked, 400)));
        }
        for &id in &t.steps {
            let st = s.step(id);
            if st.parent.is_some() || st.kind == Kind::Todo || c.steps.contains(&id) {
                continue;
            }
            if st.status == Status::Running {
                continue;
            }
            c.steps.insert(id);
            steps.push(step_line(s, id, now));
        }
        if !t.live && c.answered.insert(t.n) {
            flush(&mut lines, &mut steps);
            let said = t.answer().unwrap_or("");
            let end = if t.stopped {
                "stopped by the user".to_string()
            } else if !t.err.is_empty() {
                format!("ended badly: {}", words(&t.err, 200))
            } else {
                format!("answered: {}", words(said, 600))
            };
            lines.push(format!("Turn {} {end}", t.n));
        }
    }
    flush(&mut lines, &mut steps);
    // Running now, without moving the cursor: told again until it ends.
    if let Some(t) = s.turns.last().filter(|t| t.live) {
        let running: Vec<String> = t
            .steps
            .iter()
            .filter(|&&id| {
                let st = s.step(id);
                st.parent.is_none() && matches!(st.status, Status::Running | Status::Waiting)
            })
            .map(|&id| step_line(s, id, now))
            .collect();
        if !running.is_empty() && !lines.is_empty() {
            lines.push(format!("Running now: {}", running.join("; ")));
        }
    }
    let tasks = tasks_line(s);
    let th = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        tasks.hash(&mut h);
        h.finish()
    };
    if !tasks.is_empty() && th != c.tasks {
        c.tasks = th;
        lines.push(format!("Task list: {tasks}"));
    }
    c.turns = n;
    if lines.is_empty() {
        return None;
    }
    Some(fit(lines))
}

/// The steps gathered, after the lines, as one block.
fn flush(lines: &mut Vec<String>, steps: &mut Vec<String>) {
    lines.extend(steps.drain(..).map(|l| format!("- {l}")));
}

/// lines joined, the middle steps counted when they run past DELTA_CHARS.
fn fit(mut lines: Vec<String>) -> String {
    let len = |ls: &[String]| ls.iter().map(|l| l.len() + 1).sum::<usize>();
    if len(&lines) <= DELTA_CHARS {
        return lines.join("\n");
    }
    // Drop step lines from the middle until it fits.
    let mut dropped = 0;
    while len(&lines) > DELTA_CHARS {
        let steps: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i].starts_with("- "))
            .collect();
        if steps.is_empty() {
            break;
        }
        lines.remove(steps[steps.len() / 2]);
        dropped += 1;
    }
    if dropped > 0 {
        let at = lines
            .iter()
            .position(|l| l.starts_with("- "))
            .map_or(lines.len(), |i| i + 1);
        lines.insert(at, format!("- (and {dropped} more steps)"));
    }
    let mut out = lines.join("\n");
    if out.len() > DELTA_CHARS {
        let mut cut = DELTA_CHARS;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

/// A step as one line: what it did, to what, and how it came out.
pub fn step_line(s: &Session, id: StepId, now: Ms) -> String {
    let st = s.step(id);
    let i = &st.call().input;
    let what = match st.kind {
        Kind::Shell => format!("ran `{}`", words(first_line(&i.command), 160)),
        Kind::Read => format!("read {}", i.path),
        Kind::Edit | Kind::Write => format!("edited {}", i.path),
        Kind::Delete => format!("deleted {}", i.path),
        Kind::Move => format!("moved {} to {}", i.path, i.to),
        Kind::Search => format!("searched for \"{}\"", words(&i.pattern, 80)),
        Kind::Glob => format!("listed files {}", words(&i.pattern, 80)),
        Kind::Fetch => format!("fetched {}", words(&i.url, 120)),
        Kind::WebSearch => format!("searched the web for \"{}\"", words(&i.query, 80)),
        Kind::Subagent => format!(
            "had a subagent {} ({} steps)",
            words(
                if i.description.is_empty() {
                    &i.prompt
                } else {
                    &i.description
                },
                120
            ),
            st.children.len()
        ),
        Kind::Question => "asked the user a question".to_string(),
        _ => format!("used {}", st.tool),
    };
    let how = match st.status {
        Status::Running => {
            let at = if st.start > 0 && now > st.start {
                format!(", {}s so far", (now - st.start) / 1000)
            } else {
                String::new()
            };
            format!(" (running{at})")
        }
        Status::Waiting => " (waiting on the user)".to_string(),
        Status::Failed if st.kind == Kind::Shell && st.exit > 0 => {
            format!(" (failed, exit {})", st.exit)
        }
        Status::Failed => " (failed)".to_string(),
        Status::Denied => " (refused)".to_string(),
        Status::Lost => " (lost)".to_string(),
        Status::Ok => {
            let took = st.end - st.start;
            if st.start > 0 && took >= 10_000 {
                format!(" ({}s)", took / 1000)
            } else {
                String::new()
            }
        }
    };
    format!("{what}{how}")
}

/// The task list on one line: ✓ done, ▶ in progress, ○ to do.
fn tasks_line(s: &Session) -> String {
    s.tasks
        .iter()
        .map(|t| {
            let mark = match t.status.as_str() {
                "completed" => "✓",
                "in_progress" => "▶",
                _ => "○",
            };
            format!("{mark} {}", words(&t.subject, 80))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn first_line(s: &str) -> &str {
    s.trim().lines().next().unwrap_or("")
}

/// s on one line, most chars at most.
fn words(s: &str, most: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= most {
        one
    } else {
        let mut t: String = one.chars().take(most.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches() {
        let mut s = Story::default();
        for (answer, want_head, want_ids, want_prog) in [
            (
                r#"{"headline":"Porting retries","progress":[1,4],"blocks":[{"id":"p","kind":"steps","items":[{"text":"map","done":true},{"text":"rewrite","tone":"busy"}]},{"id":"n","kind":"callout","text":"no tests yet"}]}"#,
                "Porting retries",
                vec!["p", "n"],
                (1, 4),
            ),
            // In a fence after words; replaces p, drops n, adds t first.
            (
                "Here:\n```json\n{\"blocks\":[{\"id\":\"p\",\"kind\":\"steps\",\"items\":[{\"text\":\"map\",\"done\":true}]},{\"id\":\"t\",\"kind\":\"timeline\"}],\"remove\":[\"n\"],\"order\":[\"t\",\"p\"]}\n```",
                "Porting retries",
                vec!["t", "p"],
                (1, 4),
            ),
            // Unknown kinds and id-less blocks are left out; progress past
            // its total is held to it.
            (
                r#"{"progress":[9,5],"blocks":[{"id":"x","kind":"chart"},{"kind":"list"}]}"#,
                "Porting retries",
                vec!["t", "p"],
                (5, 5),
            ),
            // No plan: no progress.
            (
                r#"{"progress":[0,0]}"#,
                "Porting retries",
                vec!["t", "p"],
                (0, 0),
            ),
        ] {
            let before = s.serial;
            s.apply(answer).unwrap();
            assert!(s.serial > before, "{answer}");
            assert_eq!(s.headline, want_head, "{answer}");
            let ids: Vec<&str> = s.blocks.iter().map(|b| b.id.as_str()).collect();
            assert_eq!(ids, want_ids, "{answer}");
            assert_eq!(s.progress, want_prog, "{answer}");
        }
        let p = &s.blocks[1];
        assert_eq!(p.items.len(), 1);
        assert!(p.items[0].done);
        let before = s.clone();
        assert!(s.apply("I can't tell yet.").is_err());
        assert!(s.apply("[1,2]").is_err());
        assert_eq!(s, before, "a bad answer changes nothing");
    }

    #[test]
    fn caps() {
        let many: Vec<String> = (0..10)
            .map(|i| {
                format!(r#"{{"id":"b{i}","kind":"list","items":[{}]}}"#, {
                    (0..20)
                        .map(|j| format!(r#"{{"text":"item {j}"}}"#))
                        .collect::<Vec<_>>()
                        .join(",")
                })
            })
            .collect();
        let mut s = Story::default();
        s.apply(&format!(
            r#"{{"headline":"{}","blocks":[{}]}}"#,
            "x".repeat(500),
            many.join(",")
        ))
        .unwrap();
        assert_eq!(s.blocks.len(), MOST_BLOCKS);
        assert!(s.blocks.iter().all(|b| b.items.len() == MOST_ITEMS));
        assert_eq!(s.headline.chars().count(), MOST_CHARS);
        assert!(s.headline.ends_with('…'));
    }

    #[test]
    fn now_and_note() {
        let mut s = Story::default();
        assert!(s.set_now("t1:s:a", "\"Running the retry tests\""));
        assert_eq!(s.now, "Running the retry tests");
        assert!(!s.set_now("t1:s:a", "Running the retry tests"));
        assert!(s.set_note("reading the conversation…"));
        assert!(!s.set_note("reading the conversation…"));
        s.apply("{}").unwrap();
        assert_eq!(s.note, "", "an answer clears the note");
    }

    #[test]
    fn fits() {
        let mut lines = vec!["Turn 1: the user asked: go".to_string()];
        lines.extend((0..400).map(|i| format!("- read src/file_{i}.rs")));
        lines.push("Turn 1 answered: done".into());
        let out = fit(lines);
        assert!(out.len() <= DELTA_CHARS, "{}", out.len());
        assert!(out.starts_with("Turn 1: the user asked: go\n- read src/file_0.rs"));
        assert!(out.ends_with("Turn 1 answered: done"));
        assert!(out.contains("more steps)"));
    }
}
