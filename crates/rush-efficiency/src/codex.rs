//! Codex as efficiency's Source: its rollouts,
//! $CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl, read into the same
//! figures as Claude Code's transcripts. Rust-only: Go's efficiency read
//! Claude Code alone.
//!
//! A rollout's lines are {timestamp, type, payload}. Three are read:
//! session_meta (the thread's id, folder and whether another thread
//! started it), turn_context (the model) and the event_msg token_count,
//! whose info (null at times) has the request's tokens, the running
//! total and the model's context window. OpenAI's input_tokens is the
//! whole prompt, the cached part included, so what wasn't cached is
//! input less the cached and cache-written tokens; output includes the
//! reasoning. Codex tells a count again when only its rate limits move:
//! a token_count whose running total hasn't changed is the same request,
//! not a new one. Codex has no Pricer, so its tokens are counted and not
//! priced.

use std::path::{Path, PathBuf};

use rush_core::adapter::Profile;
use rush_core::agent::TokenUsage;
use rush_state::Time;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::scan::File;
use crate::source::{Setup, Source};

/// Codex's rollouts.
pub struct Codex;

/// The agent kind Codex's profiles have.
pub const KIND: &str = "codex";

/// Codex's frontier models, each with the cheaper one rush suggests for
/// routine work: the one place efficiency names Codex's models.
pub const FRONTIER: &[(&str, &str)] = &[("gpt-6-astra", "gpt-6.1-sol")];

/// The cheaper model for a frontier one, if model is one.
pub fn cheaper(model: &str) -> Option<&'static str> {
    FRONTIER
        .iter()
        .find(|(f, _)| model.starts_with(f))
        .map(|(_, c)| *c)
}

/// The Codex home of p: its folder, else $CODEX_HOME, else ~/.codex.
fn home_of(p: &Profile) -> PathBuf {
    if !p.dir.is_empty() {
        return PathBuf::from(&p.dir);
    }
    if let Some(h) = std::env::var_os("CODEX_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".codex")
}

fn sorted(dir: &Path) -> Vec<(String, PathBuf, bool)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = rd
        .flatten()
        .map(|e| {
            let dir = e.file_type().is_ok_and(|t| t.is_dir());
            (e.file_name().to_string_lossy().into_owned(), e.path(), dir)
        })
        .collect();
    out.sort();
    out
}

impl Source for Codex {
    fn kind(&self) -> &'static str {
        KIND
    }

    /// sessions/YYYY/MM/DD/rollout-*.jsonl, oldest first.
    fn transcripts(&self, p: &Profile) -> Vec<String> {
        let mut out = Vec::new();
        let root = home_of(p).join("sessions");
        for (_, y, d) in sorted(&root) {
            if !d {
                continue;
            }
            for (_, m, d) in sorted(&y) {
                if !d {
                    continue;
                }
                for (_, day, d) in sorted(&m) {
                    if !d {
                        continue;
                    }
                    for (name, path, d) in sorted(&day) {
                        if !d && name.starts_with("rollout-") && name.ends_with(".jsonl") {
                            out.push(path.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
        out
    }

    fn transcripts_dir(&self, p: &Profile) -> String {
        home_of(p).join("sessions").to_string_lossy().into_owned()
    }

    fn read_line(&self, f: &mut File, line: &[u8]) {
        read_line(f, line);
    }

    /// No savers are found in Codex's setup yet.
    fn setup(&self, _: &Profile) -> Setup {
        Setup::default()
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Line<'a> {
    timestamp: Time,
    #[serde(rename = "type")]
    kind: String,
    #[serde(borrow)]
    payload: Option<&'a RawValue>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Meta {
    id: String,
    session_id: String,
    cwd: String,
    source: serde_json::Value,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TurnContext {
    model: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    info: Option<Info>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Info {
    total_token_usage: Usage,
    last_token_usage: Usage,
    model_context_window: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Usage {
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
}

/// The lines worth decoding say one of these near their start; the rest
/// (response items, most of the bytes) are skipped unread.
const MARKERS: &[&[u8]] = &[
    br#""type":"session_meta""#,
    br#""type":"turn_context""#,
    br#""type":"token_count""#,
];

fn interesting(b: &[u8]) -> bool {
    let head = &b[..b.len().min(256)];
    MARKERS
        .iter()
        .any(|m| head.windows(m.len()).any(|w| w == *m))
}

/// Reads one rollout line into f.
pub fn read_line(f: &mut File, b: &[u8]) {
    if !interesting(b) {
        return;
    }
    let Ok(l) = serde_json::from_slice::<Line>(b) else {
        return;
    };
    let at = l.timestamp;
    let Some(payload) = l.payload else { return };
    if at.is_zero() {
        return;
    }
    match l.kind.as_str() {
        "session_meta" => {
            let Ok(m) = serde_json::from_str::<Meta>(payload.get()) else {
                return;
            };
            let id = if m.id.is_empty() { m.session_id } else { m.id };
            if f.session.is_empty() {
                f.sub = m.source.get("subagent").is_some();
            }
            f.saw(at, &id, &m.cwd);
        }
        "turn_context" => {
            if let Ok(c) = serde_json::from_str::<TurnContext>(payload.get())
                && !c.model.is_empty()
            {
                f.turn_model = c.model;
            }
            f.saw(at, "", "");
        }
        "event_msg" => {
            let Ok(e) = serde_json::from_str::<Event>(payload.get()) else {
                return;
            };
            if e.kind != "token_count" {
                return;
            }
            f.saw(at, "", "");
            let Some(info) = e.info else { return };
            let total = info.total_token_usage.total_tokens;
            if total == f.seen_total {
                return; // the same request told again
            }
            f.seen_total = total;
            let u = &info.last_token_usage;
            let tu = TokenUsage {
                input: (u.input_tokens - u.cached_input_tokens - u.cache_write_input_tokens).max(0),
                output: u.output_tokens,
                cache_read: u.cached_input_tokens,
                cache_write_5m: u.cache_write_input_tokens,
                ..TokenUsage::default()
            };
            let model = f.turn_model.clone();
            f.request(
                &format!("t{total}"),
                &model,
                tu,
                u.reasoning_output_tokens,
                false,
                at,
            );
            f.sent(u.input_tokens, info.model_context_window, at);
        }
        _ => {}
    }
}
