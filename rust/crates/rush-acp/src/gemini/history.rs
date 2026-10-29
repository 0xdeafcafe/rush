//! Gemini's saved sessions: legacy JSON snapshots and append-only JSONL,
//! with updated messages, rewinds and compacted replacement histories.
//! Port of internal/adapters/gemini/history.go:1-271.
//!
//! A JSONL log can rewind or replace what came before, so a session is
//! read whole before its events are handed on, a slice at a time.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rush_core::adapter::{self, Discoverer, HistoryReader, Profile, Session};
use rush_core::event::{self, Event, PartKind};
use rush_core::time::{Ms, parse_rfc3339};
use rush_core::tool;
use rush_core::usage::TokenUsage;
use rush_core::wire::RawJson;
use serde::Deserialize;
use serde_json::value::RawValue;

use super::Gemini;
use crate::history::{HistoryError, Turns, content_parts, text_part};
use crate::session::lock;
use crate::util::{glob, home_dir, mod_ms, path_str};

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Thought {
    subject: String,
    description: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Tokens {
    input: i64,
    output: i64,
    cached: i64,
    thoughts: i64,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct SavedCall {
    id: String,
    name: String,
    args: Option<Box<RawValue>>,
    result: Option<Box<RawValue>>,
    status: String,
}

/// One message as Gemini saves it, and the records a JSONL log keeps
/// beside them.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(super) struct SavedMessage {
    injected: bool,
    id: String,
    #[serde(rename = "type")]
    kind: String,
    timestamp: String,
    model: String,
    content: Option<Box<RawValue>>,
    thoughts: Vec<Thought>,
    tokens: Option<Tokens>,
    #[serde(rename = "toolCalls")]
    tools: Vec<SavedCall>,
    // A JSONL log's own records.
    #[serde(rename = "$rewindTo")]
    rewind: String,
    #[serde(rename = "$set")]
    set: Option<Box<RawValue>>,
    #[serde(rename = "sessionId")]
    session: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Conversation {
    #[serde(rename = "sessionId")]
    id: String,
    summary: String,
    #[serde(rename = "startTime")]
    start: String,
    #[serde(rename = "lastUpdated")]
    updated: String,
    directories: Vec<String>,
    messages: Vec<SavedMessage>,
}

/// A saved session, whichever way it was saved.
pub(super) fn read_conversation(path: &str) -> Result<Conversation, HistoryError> {
    let io = |e: std::io::Error| HistoryError(format!("open {path}: {e}"));
    if path.ends_with(".json") {
        let b = std::fs::read(path).map_err(io)?;
        return serde_json::from_slice(&b).map_err(|e| HistoryError(e.to_string()));
    }
    let f = std::fs::File::open(path).map_err(io)?;
    let mut r = BufReader::with_capacity(64 << 10, f);
    let mut c = Conversation::default();
    let mut indexes: HashMap<String, usize> = HashMap::new();
    let rebuild = |c: &Conversation, idx: &mut HashMap<String, usize>| {
        idx.clear();
        for (i, m) in c.messages.iter().enumerate() {
            idx.insert(m.id.clone(), i);
        }
    };
    let mut line = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return Err(HistoryError(e.to_string())),
        }
        let b = line.trim_ascii_end();
        let Ok(rec) = serde_json::from_slice::<SavedMessage>(b) else {
            continue;
        };
        if !rec.rewind.is_empty() {
            match indexes.get(&rec.rewind) {
                Some(&i) => c.messages.truncate(i),
                None => c.messages.clear(),
            }
            rebuild(&c, &mut indexes);
        } else if !rec.id.is_empty() {
            match indexes.get(&rec.id) {
                Some(&i) => c.messages[i] = rec,
                None => {
                    indexes.insert(rec.id.clone(), c.messages.len());
                    c.messages.push(rec);
                }
            }
        } else if let Some(set) = &rec.set {
            let Ok(patch) = serde_json::from_str::<HashMap<String, Box<RawValue>>>(set.get())
            else {
                continue;
            };
            if let Some(v) = patch.get("summary") {
                c.summary = serde_json::from_str(v.get()).unwrap_or(c.summary);
            }
            if let Some(v) = patch.get("lastUpdated") {
                c.updated = serde_json::from_str(v.get()).unwrap_or(c.updated);
            }
            if let Some(v) = patch.get("directories") {
                c.directories = serde_json::from_str(v.get()).unwrap_or(c.directories);
            }
            if let Some(v) = patch.get("messages") {
                c.messages = serde_json::from_str(v.get()).unwrap_or(c.messages);
                rebuild(&c, &mut indexes);
            }
        } else if !rec.session.is_empty() {
            if let Ok(head) = serde_json::from_slice::<Conversation>(b) {
                c.id = head.id;
                c.summary = head.summary;
                c.start = head.start;
                c.updated = head.updated;
                c.directories = head.directories;
                c.messages = head.messages;
            }
            rebuild(&c, &mut indexes);
        }
    }
    Ok(c)
}

/// A saved session's row, kept while its file stays the same size and
/// age.
#[derive(Clone)]
pub(super) struct Cached {
    size: u64,
    modified: Ms,
    row: Session,
}

/// How many rows the cache keeps before it starts over.
const CACHE_MOST: usize = 2048;

impl Discoverer for Gemini {
    fn live(&self, _: &Profile) -> Vec<Session> {
        Vec::new()
    }

    fn past(&self, p: &Profile) -> Vec<Session> {
        let mut p = p.clone();
        if p.dir.is_empty() {
            p.dir = path_str(&Path::new(&home_dir()).join(".gemini"));
        }
        let mut out: Vec<Session> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut keep = |row: Session, path: &str, out: &mut Vec<Session>| match seen.get(&row.id) {
            Some(&i) => {
                if path.ends_with(".jsonl") {
                    out[i] = row;
                }
            }
            None => {
                seen.insert(row.id.clone(), out.len());
                out.push(row);
            }
        };
        for path in glob(Path::new(&p.dir), &["tmp", "*", "chats", "session-*.json*"]) {
            let path = path_str(&path);
            if !path.ends_with(".json") && !path.ends_with(".jsonl") {
                continue;
            }
            let Ok(st) = std::fs::metadata(&path) else {
                continue;
            };
            let hit = lock(&self.cache).get(&path).cloned();
            if let Some(c) = hit
                && c.size == st.len()
                && c.modified == mod_ms(&st)
            {
                let mut row = c.row;
                row.profile = p.clone();
                keep(row, &path, &mut out);
                continue;
            }
            let Ok(c) = read_conversation(&path) else {
                continue;
            };
            if c.id.is_empty() {
                continue;
            }
            let cwd = match c.directories.first() {
                Some(d) => d.clone(),
                None => {
                    let root = Path::new(&path)
                        .parent()
                        .and_then(Path::parent)
                        .unwrap_or(Path::new(""))
                        .join(".project_root");
                    std::fs::read_to_string(root)
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                }
            };
            let mut name = c.summary.clone();
            let mut model = String::new();
            for m in &c.messages {
                if name.is_empty()
                    && m.kind == "user"
                    && let Some(t) = content_parts(m.content.as_deref())
                        .into_iter()
                        .find(|p| p.kind == PartKind::Text)
                {
                    name = t.text;
                }
                if !m.model.is_empty() {
                    model = m.model.clone();
                }
            }
            let row = Session {
                kind: self.agent.id.clone(),
                profile: p.clone(),
                id: c.id,
                name,
                cwd,
                model,
                transcript: path.clone(),
                state: "done".into(),
                created_at: parse_rfc3339(&c.start),
                updated_at: parse_rfc3339(&c.updated),
                ..Session::default()
            };
            {
                let mut cache = lock(&self.cache);
                if cache.len() >= CACHE_MOST {
                    cache.clear();
                }
                cache.insert(
                    path.clone(),
                    Cached {
                        size: st.len(),
                        modified: mod_ms(&st),
                        row: row.clone(),
                    },
                );
            }
            keep(row, &path, &mut out);
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }
}

/// About how many bytes a saved message comes to, to slice by.
fn weight(m: &SavedMessage) -> usize {
    let raw = |r: &Option<Box<RawValue>>| r.as_ref().map_or(0, |r| r.get().len());
    raw(&m.content)
        + m.thoughts
            .iter()
            .map(|t| t.subject.len() + t.description.len())
            .sum::<usize>()
        + m.tools
            .iter()
            .map(|t| raw(&t.args) + raw(&t.result))
            .sum::<usize>()
}

impl HistoryReader for Gemini {
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> adapter::Result<()> {
        let c = read_conversation(&s.transcript)?;
        let mut turns = Turns::default();
        let (mut out, mut taken) = (Vec::new(), 0usize);
        for m in c.messages {
            if before != 0 {
                let at = parse_rfc3339(&m.timestamp);
                if at != 0 && at >= before {
                    continue;
                }
            }
            let role = match m.kind.as_str() {
                "gemini" => "assistant",
                r @ ("user" | "assistant") => r,
                _ => continue,
            };
            taken += weight(&m);
            let mut msg = event::Message {
                role: role.into(),
                id: m.id.clone(),
                model: m.model.clone(),
                injected: m.injected,
                ..event::Message::default()
            };
            for t in &m.thoughts {
                msg.parts.push(text_part(
                    PartKind::Thinking,
                    format!("{}\n{}", t.subject, t.description)
                        .trim()
                        .to_string(),
                ));
            }
            msg.parts.extend(content_parts(m.content.as_deref()));
            if let Some(t) = &m.tokens {
                msg.tokens = Some(TokenUsage {
                    input: t.input,
                    output: t.output,
                    cache_read: t.cached,
                    reasoning: t.thoughts,
                    ..TokenUsage::default()
                });
            }
            let mut results = Vec::new();
            for t in m.tools {
                msg.parts.push(event::Part {
                    kind: PartKind::ToolCall,
                    call: Some(Box::new(tool::Call {
                        id: t.id.clone(),
                        name: t.name,
                        raw: t.args.map(RawJson),
                        ..tool::Call::default()
                    })),
                    ..event::Part::default()
                });
                if let Some(r) = t.result.filter(|r| r.get() != "null") {
                    results.push(event::Part {
                        kind: PartKind::ToolResult,
                        output: Some(Box::new(tool::Output {
                            call_id: t.id,
                            text: r.get().to_string(),
                            is_error: t.status == "error",
                            ..tool::Output::default()
                        })),
                        ..event::Part::default()
                    });
                }
            }
            if !msg.parts.is_empty() {
                turns.push(Event::Message(msg), &mut out);
            }
            if !results.is_empty() {
                turns.push(
                    Event::Message(event::Message {
                        role: "user".into(),
                        parts: results,
                        ..event::Message::default()
                    }),
                    &mut out,
                );
            }
            if taken >= most.max(1) {
                taken = 0;
                if !out.is_empty() && !each(std::mem::take(&mut out)) {
                    return Ok(());
                }
            }
        }
        turns.finish(&mut out);
        if !out.is_empty() {
            each(out);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::adapter::read_history;

    // history_test.go: TestHistoryReplacementsRewindImagesAndDiscovery
    #[test]
    fn replacements_rewinds_images_and_discovery() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("tmp/project/chats");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.parent().unwrap().join(".project_root"),
            "/work/project",
        )
        .unwrap();
        std::fs::write(
            root.join("session-test.jsonl"),
            r#"{"sessionId":"s1","projectHash":"hash","startTime":"2026-01-01T00:00:00Z"}
{"id":"u1","type":"user","timestamp":"2026-01-01T00:00:01Z","content":[{"text":"whole prompt"},{"inlineData":{"mimeType":"image/png","data":"aGk="}}]}
{"id":"a1","type":"gemini","content":"partial"}
{"id":"a1","type":"gemini","content":"complete","tokens":{"input":50,"output":20},"model":"pro"}
{"id":"u2","type":"user","content":"discard me"}
{"$rewindTo":"u2"}
{"$set":{"summary":"My session","lastUpdated":"2026-01-01T00:00:10Z"}}
"#,
        )
        .unwrap();
        let a = Gemini::new();
        let p = Profile {
            dir: path_str(d.path()),
            ..Profile::default()
        };
        let past = a.past(&p);
        assert_eq!(past.len(), 1);
        assert_eq!(
            (past[0].name.as_str(), past[0].cwd.as_str()),
            ("My session", "/work/project")
        );
        // Read again from the cache, the same.
        assert_eq!(a.past(&p)[0].name, "My session");
        let evs = read_history(&a, &past[0], 0).unwrap();
        assert_eq!(evs.len(), 3, "{evs:?}");
        let Event::Message(user) = &evs[0] else {
            panic!()
        };
        assert_eq!(user.parts.len(), 2);
        assert_eq!(user.parts[1].image.as_ref().unwrap().data, b"hi");
        let Event::Message(msg) = &evs[1] else {
            panic!()
        };
        assert_eq!(msg.parts[0].text, "complete");
        assert_eq!(msg.tokens.unwrap().input, 50);
    }

    // history_test.go: TestLegacyHistoryAndCompaction
    #[test]
    fn legacy_history_and_compaction() {
        for ext in ["json", "jsonl"] {
            let d = tempfile::tempdir().unwrap();
            let path = d.path().join(format!("session.{ext}"));
            let mut text =
                r#"{"sessionId":"s","messages":[{"id":"u","type":"user","content":"before"}]}"#
                    .to_string();
            if ext == "jsonl" {
                text += "\n";
                text += r#"{"$set":{"messages":[{"id":"u2","type":"user","content":"after"}]}}"#;
            }
            std::fs::write(&path, text).unwrap();
            let c = read_conversation(&path_str(&path)).unwrap();
            assert_eq!(c.messages.len(), 1);
            let want = if ext == "jsonl" { "u2" } else { "u" };
            assert_eq!(c.messages[0].id, want);
        }
    }

    // history_turns_test.go: TestSavedGeminiTurnsRenderSeparatelyAndFinish,
    // without convo: the boundaries it needs.
    #[test]
    fn saved_turns_end() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("session.json");
        std::fs::write(&path, r#"{"sessionId":"s","messages":[{"id":"u1","type":"user","content":"first"},{"id":"a1","type":"gemini","content":"answer one"},{"id":"u2","type":"user","content":"second"},{"id":"a2","type":"gemini","content":"answer two"}]}"#).unwrap();
        let s = Session {
            transcript: path_str(&path),
            ..Session::default()
        };
        let evs = read_history(&Gemini::new(), &s, 0).unwrap();
        let ends = evs
            .iter()
            .filter(|e| matches!(e, Event::TurnEnd(_)))
            .count();
        assert_eq!(ends, 2);
        assert!(matches!(evs.last(), Some(Event::TurnEnd(_))));
    }
}
