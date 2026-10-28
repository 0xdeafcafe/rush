//! Claude's message JSON (a transcript line's `message`, or a headless
//! stream event's) into rush_core's Message. Port of
//! internal/adapters/claude/headless/event.go:490-573 (DecodeMessage,
//! flatten) and neutral.go:255-264 (tokens).

use rush_core::message::{Block, BlockKind, Image, Message, Role, Usage};
use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Deserialize)]
struct RawMessage<'a> {
    #[serde(default)]
    id: String,
    #[serde(default, borrow)]
    role: Option<&'a str>,
    #[serde(default)]
    model: String,
    #[serde(default, borrow)]
    content: Option<&'a RawValue>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawBlock<'a> {
    #[serde(rename = "type", default, borrow)]
    typ: &'a str,
    #[serde(default)]
    text: String,
    #[serde(default)]
    thinking: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, borrow)]
    input: Option<&'a RawValue>,
    #[serde(default)]
    tool_use_id: String,
    #[serde(default, borrow)]
    content: Option<&'a RawValue>,
    #[serde(default)]
    is_error: bool,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: i64,
    #[serde(default)]
    cache_creation: Option<CacheBreakup>,
}

#[derive(Deserialize, Default)]
struct CacheBreakup {
    #[serde(default)]
    ephemeral_5m_input_tokens: i64,
    #[serde(default)]
    ephemeral_1h_input_tokens: i64,
}

/// A request's usage. Cache writes not split by lifetime count as the hour
/// Claude Code asks for its main agent.
fn tokens(u: &RawUsage) -> Usage {
    let mut t = Usage {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_read: u.cache_read_input_tokens,
        cache_write_5m: 0,
        cache_write_1h: u.cache_creation_input_tokens,
    };
    if let Some(b) = &u.cache_creation
        && b.ephemeral_5m_input_tokens + b.ephemeral_1h_input_tokens > 0
    {
        t.cache_write_5m = b.ephemeral_5m_input_tokens;
        t.cache_write_1h = b.ephemeral_1h_input_tokens;
    }
    t
}

/// Decodes one `message` object; `None` when it isn't one. Its role is the
/// message's own, else the assistant's; a transcript reader that knows the
/// line's type sets it. Blocks of a kind rush doesn't draw (an image in a
/// prompt, a server tool's) are left out.
pub fn decode_message(raw: &str) -> Option<Message> {
    let m: RawMessage = serde_json::from_str(raw).ok()?;
    let role = match m.role {
        Some("user") => Role::User,
        _ => Role::Assistant,
    };
    let mut out = Message {
        role,
        id: m.id,
        model: m.model,
        uuid: String::new(),
        parent_tool_use_id: String::new(),
        blocks: Vec::new(),
        usage: m.usage.as_ref().map(tokens),
        tool_result: None,
    };
    let content = m.content?;
    let c = content.get();
    if c == "null" {
        let mut b = Block::new(BlockKind::Text);
        b.text = String::new();
        out.blocks.push(b);
        return Some(out);
    }
    if c.starts_with('"') {
        let mut b = Block::new(BlockKind::Text);
        b.text = serde_json::from_str(c).ok()?;
        out.blocks.push(b);
        return Some(out);
    }
    let blocks: Vec<RawBlock> = serde_json::from_str(c).ok()?;
    out.blocks.reserve_exact(blocks.len());
    for b in blocks {
        let kind = match b.typ {
            "text" => BlockKind::Text,
            "thinking" | "redacted_thinking" => BlockKind::Thinking,
            "tool_use" => BlockKind::ToolUse,
            "tool_result" => BlockKind::ToolResult,
            _ => continue,
        };
        let mut blk = Block::new(kind);
        match kind {
            BlockKind::Text => blk.text = b.text,
            BlockKind::Thinking => blk.text = b.thinking,
            BlockKind::ToolUse => {
                blk.id = b.id;
                blk.name = b.name;
                blk.input = b.input.map(|r| r.to_owned());
            }
            BlockKind::ToolResult => {
                blk.tool_use_id = b.tool_use_id;
                blk.is_error = b.is_error;
                if let Some(c) = b.content {
                    let (text, images) = flatten(c.get());
                    blk.text = text;
                    blk.images = images;
                }
            }
        }
        out.blocks.push(blk);
    }
    Some(out)
}

#[derive(Deserialize)]
struct RawPart<'a> {
    #[serde(rename = "type", default, borrow)]
    typ: &'a str,
    #[serde(default)]
    text: String,
    #[serde(default)]
    source: Option<RawSource>,
}

#[derive(Deserialize)]
struct RawSource {
    #[serde(default)]
    media_type: String,
    #[serde(default)]
    data: String,
}

/// A tool result's content, a string or a list of blocks, as text and the
/// images it holds.
fn flatten(raw: &str) -> (String, Vec<Image>) {
    if raw.starts_with('"') {
        return (serde_json::from_str(raw).unwrap_or_default(), Vec::new());
    }
    let Ok(parts) = serde_json::from_str::<Vec<RawPart>>(raw) else {
        return (String::new(), Vec::new());
    };
    let mut text = String::new();
    let mut first = true;
    let mut images = Vec::new();
    for p in parts {
        match p.typ {
            "text" => {
                if !first {
                    text.push('\n');
                }
                first = false;
                text.push_str(&p.text);
            }
            "image" => {
                let s = p.source.unwrap_or(RawSource {
                    media_type: String::new(),
                    data: String::new(),
                });
                images.push(Image {
                    media_type: s.media_type,
                    data: s.data,
                });
            }
            _ => {}
        }
    }
    (text, images)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lines as Claude Code 2.1.280 writes them (headless_test.go).
    const TOOL_USE: &str = r#"{"model":"claude-haiku-4-5-20251001","id":"msg_1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo hi","description":"Say hi"}}],"usage":{"input_tokens":10,"output_tokens":4,"cache_read_input_tokens":13689,"cache_creation_input_tokens":7787}}"#;
    const TOOL_RESULT: &str = r#"{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":[{"type":"text","text":"hi"}],"is_error":false}]}"#;

    #[test]
    fn decodes_a_tool_use() {
        let m = decode_message(TOOL_USE).unwrap();
        assert_eq!(m.role, Role::Assistant);
        assert_eq!(m.id, "msg_1");
        assert_eq!(m.model, "claude-haiku-4-5-20251001");
        assert_eq!(m.blocks.len(), 1);
        let b = &m.blocks[0];
        assert_eq!(b.kind, BlockKind::ToolUse);
        assert_eq!(b.name, "Bash");
        assert_eq!(b.id, "toolu_1");
        assert_eq!(
            b.input.as_ref().unwrap().get(),
            r#"{"command":"echo hi","description":"Say hi"}"#
        );
        let u = m.usage.unwrap();
        assert_eq!(u.cache_read, 13689);
        // Not split by lifetime: the hour's.
        assert_eq!((u.cache_write_5m, u.cache_write_1h), (0, 7787));
    }

    #[test]
    fn decodes_a_tool_result() {
        let m = decode_message(TOOL_RESULT).unwrap();
        assert_eq!(m.role, Role::User);
        let b = &m.blocks[0];
        assert_eq!(b.kind, BlockKind::ToolResult);
        assert_eq!(b.text, "hi");
        assert_eq!(b.tool_use_id, "toolu_1");
        assert!(!b.is_error);
    }

    #[test]
    fn flattens_results() {
        let cases: &[(&str, &str, usize)] = &[
            (r#""plain""#, "plain", 0),
            (
                r#"[{"type":"text","text":"a"},{"type":"text","text":"b"}]"#,
                "a\nb",
                0,
            ),
            (
                r#"[{"type":"text","text":"shot"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBO"}}]"#,
                "shot",
                1,
            ),
            (r#"{"odd":1}"#, "", 0),
        ];
        for (raw, text, n) in cases {
            let (t, imgs) = flatten(raw);
            assert_eq!(&t, text, "{raw}");
            assert_eq!(imgs.len(), *n, "{raw}");
        }
        let (_, imgs) = flatten(cases[2].0);
        assert_eq!(imgs[0].media_type, "image/png");
        assert_eq!(imgs[0].data, "iVBO");
    }

    #[test]
    fn decodes_text_thinking_and_usage_split() {
        let raw = r#"{"role":"assistant","content":[{"type":"thinking","thinking":"hmm","signature":"x"},{"type":"redacted_thinking","data":"zz"},{"type":"text","text":"Done."},{"type":"server_tool_use","id":"s"}],"usage":{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":30,"cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20}}}"#;
        let m = decode_message(raw).unwrap();
        let kinds: Vec<_> = m.blocks.iter().map(|b| (b.kind, b.text.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (BlockKind::Thinking, "hmm"),
                (BlockKind::Thinking, ""),
                (BlockKind::Text, "Done.")
            ]
        );
        let u = m.usage.unwrap();
        assert_eq!((u.cache_write_5m, u.cache_write_1h), (10, 20));
    }

    #[test]
    fn a_string_is_one_text_block() {
        let m = decode_message(r#"{"role":"user","content":"say hi"}"#).unwrap();
        assert_eq!(m.blocks.len(), 1);
        assert_eq!(m.blocks[0].kind, BlockKind::Text);
        assert_eq!(m.blocks[0].text, "say hi");
    }

    #[test]
    fn not_a_message() {
        for raw in ["", "nope", "[]", r#"{"role":"user"}"#, r#"{"content":5}"#] {
            assert!(decode_message(raw).is_none(), "{raw}");
        }
    }
}
