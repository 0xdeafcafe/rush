//! Tools for Claude: their definitions, as `tools.list` answers them, and
//! results, as `tools.call` does.

use serde_json::{Map, Value, json};

/// One tool, as MCP defines them. Claude sees it as
/// `mcp__rush-<plugin>__<name>`.
pub fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": input_schema})
}

/// An input schema of string arguments, each with its description, and
/// those of them required. No other arguments are allowed.
pub fn schema(props: &[(&str, &str)], required: &[&str]) -> Value {
    let mut p = Map::new();
    for (k, d) in props {
        p.insert(
            (*k).to_string(),
            json!({"type": "string", "description": d}),
        );
    }
    json!({"type": "object", "properties": p, "required": required, "additionalProperties": false})
}

/// A tool's answer: text for Claude, and whether it's a failure. A tool
/// that fails answers this with `is_error`, saying what Claude can do
/// about it; a JSON-RPC error is for a broken request.
pub fn text(text: impl Into<String>, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text.into()}], "isError": is_error})
}

/// A tool's answer from what it did: the text, or the error's.
pub fn result<E: std::fmt::Display>(r: Result<String, E>) -> Value {
    match r {
        Ok(t) => text(t, false),
        Err(e) => text(e.to_string(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(
            schema(&[("id", "The agent's id.")], &["id"]),
            json!({"type": "object", "properties": {"id": {"type": "string", "description": "The agent's id."}},
                "required": ["id"], "additionalProperties": false})
        );
        assert_eq!(
            result::<String>(Err("no".into())),
            json!({"content": [{"type": "text", "text": "no"}], "isError": true})
        );
    }
}
