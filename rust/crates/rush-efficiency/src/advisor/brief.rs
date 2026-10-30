//! A transcript cut down to what the advisor needs: one short line for
//! each prompt, tool call and compaction, with what each tool sent back
//! and how big the context was. Port of internal/advisor/brief.go. A
//! transcript line can be hundreds of kilobytes; a brief line is at most
//! BRIEF_LINE bytes, so Haiku can read a whole session for cents.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rush_state::Time;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::when::local;

const BRIEF_LINE: usize = 160;
/// Lines kept from a session's start.
pub(crate) const BRIEF_HEAD: usize = 100;
/// And from its end: about 16k tokens in all.
pub(crate) const BRIEF_TAIL: usize = 400;

/// Writes a brief of each transcript into dir, and returns their paths in
/// the same order ("" where one couldn't be read).
pub fn briefs(dir: &Path, transcripts: &[String]) -> Vec<String> {
    let _ = std::fs::remove_dir_all(dir);
    let mut out = vec![String::new(); transcripts.len()];
    if std::fs::create_dir_all(dir).is_err() {
        return out;
    }
    for (i, t) in transcripts.iter().enumerate() {
        let Ok(lines) = brief(t) else { continue };
        let stem = Path::new(t)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = stem.strip_suffix(".jsonl").unwrap_or(&stem);
        let p = dir.join(format!("{}-{stem}.txt", i + 1));
        let mut body = lines.join("\n");
        body.push('\n');
        if std::fs::write(&p, body).is_ok() {
            out[i] = p.to_string_lossy().into_owned();
        }
    }
    out
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Entry<'a> {
    #[serde(rename = "type")]
    kind: String,
    subtype: String,
    timestamp: Time,
    #[serde(borrow)]
    message: Option<Msg<'a>>,
    #[serde(rename = "compactMetadata")]
    compact: Option<CompactMeta>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Msg<'a> {
    #[serde(borrow)]
    content: Option<&'a RawValue>,
    usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Usage {
    input_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CompactMeta {
    trigger: String,
    #[serde(rename = "preTokens")]
    pre: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Block<'a> {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    name: String,
    text: String,
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    tool_use_id: String,
    #[serde(borrow)]
    content: Option<&'a RawValue>,
}

fn brief(path: &str) -> std::io::Result<Vec<String>> {
    let f = std::fs::File::open(path)?;
    let mut lines = Vec::new();
    // tool_use id → its line, for the size of what came back.
    let mut calls: HashMap<String, usize> = HashMap::new();
    let mut ctx = 0i64;
    let mut r = BufReader::with_capacity(1 << 16, f);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if r.read_until(b'\n', &mut raw)? == 0 {
            break;
        }
        brief_of(&raw, &mut lines, &mut calls, &mut ctx);
    }
    let n = lines.len();
    if n > BRIEF_HEAD + BRIEF_TAIL {
        let skipped = format!("… {} lines skipped …", n - BRIEF_HEAD - BRIEF_TAIL);
        let tail = lines.split_off(n - BRIEF_TAIL);
        lines.truncate(BRIEF_HEAD);
        lines.push(skipped);
        lines.extend(tail);
    }
    Ok(lines)
}

fn has(b: &[u8], s: &str) -> bool {
    b.windows(s.len()).any(|w| w == s.as_bytes())
}

fn brief_of(
    raw: &[u8],
    lines: &mut Vec<String>,
    calls: &mut HashMap<String, usize>,
    ctx: &mut i64,
) {
    // Only these lines say anything a brief keeps; the rest aren't decoded.
    if !has(raw, r#""type":"assistant""#)
        && !has(raw, r#""type":"user""#)
        && !has(raw, r#""compact_boundary""#)
    {
        return;
    }
    let Ok(e) = serde_json::from_slice::<Entry>(raw) else {
        return;
    };
    let at = local(e.timestamp, "15:04");
    let add = |lines: &mut Vec<String>, s: String| {
        let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if s.len() > BRIEF_LINE {
            let mut end = BRIEF_LINE;
            while !s.is_char_boundary(end) {
                end -= 1; // not half a character
            }
            s.truncate(end);
            s.push('…');
        }
        lines.push(s);
    };
    if e.subtype == "compact_boundary"
        && let Some(c) = &e.compact
    {
        add(
            lines,
            format!("{at} — compacted ({}) at {}", c.trigger, tokens(c.pre)),
        );
        return;
    }
    let Some(m) = &e.message else { return };
    let Some(content) = m.content else { return };
    let Ok(blocks) = serde_json::from_str::<Vec<Block>>(content.get()) else {
        // A prompt written as a plain string.
        if e.kind == "user"
            && let Ok(s) = serde_json::from_str::<String>(content.get())
            && !s.is_empty()
        {
            add(lines, format!("{at} you (ctx {}): {s}", tokens(*ctx)));
        }
        return;
    };
    if let Some(u) = &m.usage {
        *ctx = u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
    }
    for b in &blocks {
        if e.kind == "user" && b.kind == "text" && !b.text.is_empty() {
            add(
                lines,
                format!("{at} you (ctx {}): {}", tokens(*ctx), b.text),
            );
        } else if b.kind == "tool_use" {
            let i = lines.len();
            add(lines, format!("{at} {}: {}", b.name, tool_input(b.input)));
            calls.insert(b.id.clone(), i);
        } else if b.kind == "tool_result"
            && let Some(&i) = calls.get(&b.tool_use_id)
            && i < lines.len()
        {
            let n = b.content.map_or(0, |c| c.get().len());
            lines[i] += &format!(" → {}", size(n));
        }
    }
}

/// The part of a tool call's input that says what it did.
fn tool_input(raw: Option<&RawValue>) -> String {
    let Some(raw) = raw else {
        return String::new();
    };
    let Ok(input) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(raw.get())
    else {
        return String::new();
    };
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).map(str::to_string);
    for k in [
        "command",
        "file_path",
        "pattern",
        "description",
        "prompt",
        "url",
        "query",
    ] {
        let Some(mut v) = s(k).filter(|v| !v.is_empty()) else {
            continue;
        };
        if k == "description" || k == "prompt" {
            if let Some(t) = s("subagent_type") {
                v = format!("{t}, {v}");
            }
            if let Some(m) = s("model") {
                v = format!("{m}, {v}");
            }
        }
        return v;
    }
    String::new()
}

fn tokens(n: i64) -> String {
    if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

fn size(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{} KB", n >> 10)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn briefs_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(300_000);
        let lines = [
            r#"{"type":"user","timestamp":"2026-09-28T10:00:00Z","message":{"content":"fix the build"}}"#.to_string(),
            r#"{"type":"assistant","timestamp":"2026-09-28T10:01:00Z","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":40000},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go test ./..."}}]}}"#.to_string(),
            format!(r#"{{"type":"user","timestamp":"2026-09-28T10:02:00Z","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"{big}"}}]}}}}"#),
            r#"{"type":"assistant","timestamp":"2026-09-28T10:03:00Z","message":{"content":[{"type":"tool_use","id":"t2","name":"Agent","input":{"subagent_type":"Explore","model":"opus","description":"find it"}}]}}"#.to_string(),
            r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-09-28T10:04:00Z","compactMetadata":{"trigger":"auto","preTokens":400000}}"#.to_string(),
            format!(r#"{{"type":"progress","data":"{big}"}}"#),
        ];
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        let out = briefs(
            &dir.path().join("briefs"),
            &[
                p.to_string_lossy().into_owned(),
                dir.path()
                    .join("missing.jsonl")
                    .to_string_lossy()
                    .into_owned(),
            ],
        );
        assert_eq!(out[1], "", "a brief of a missing transcript");
        let got = std::fs::read_to_string(&out[0]).unwrap();
        for want in [
            "you (ctx 0): fix the build",
            "Bash: go test ./... → 292 KB",
            "Agent: opus, Explore, find it",
            "compacted (auto) at 400k",
        ] {
            assert!(got.contains(want), "brief lacks {want:?}:\n{got}");
        }
        assert!(got.len() <= 2000, "brief is {} bytes", got.len());
    }

    #[test]
    fn capped() {
        let dir = tempfile::tempdir().unwrap();
        let lines: Vec<String> = (0..2000)
            .map(|i| format!(r#"{{"type":"assistant","timestamp":"2026-09-28T10:00:00Z","message":{{"content":[{{"type":"tool_use","id":"t{i}","name":"Read","input":{{"file_path":"/f{i}.go"}}}}]}}}}"#))
            .collect();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, lines.join("\n")).unwrap(); // no final newline
        let b = std::fs::read_to_string(
            &briefs(&dir.path().join("b"), &[p.to_string_lossy().into_owned()])[0],
        )
        .unwrap();
        let got: Vec<&str> = b.trim().split('\n').collect();
        assert_eq!(got.len(), BRIEF_HEAD + BRIEF_TAIL + 1);
        assert!(b.contains("1500 lines skipped") && got.last().unwrap().contains("/f1999.go"));
    }
}
