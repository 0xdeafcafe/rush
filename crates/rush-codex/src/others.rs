//! Every other model rush runs, as Codex's own: a tool of the thread's
//! (thread/start's dynamicTools) that rush answers itself. Codex's
//! spawn_agent can't be handed to rush, and a dynamic tool can't take its
//! name, so it's run_agent, shaped like spawn_agent (agent_type,
//! message). When the model calls it, Codex asks rush (item/tool/call);
//! rush runs the task on the model asked for (the Delegate: a rush-hosted
//! child) and answers with its answer, which Codex hands the model as the
//! tool's ordinary output. No model stands between. With background it
//! answers at once; the answer comes as rush's message. See
//! docs/native-agents.md.
//!
//! Rust-only.

use std::collections::HashSet;

use rush_core::adapter::{DelegateCall, DelegateRef, OtherAgent};
use serde_json::{Value, json};

/// The tool's name.
pub const RUN_AGENT: &str = "run_agent";

/// The request Codex makes when the model calls it.
pub const REQ_TOOL_CALL: &str = "item/tool/call";

/// thread/start's dynamicTools entry for others.
pub fn tool_spec(others: &[OtherAgent]) -> Value {
    let mut d = String::from(
        "Start another model as your subagent on a task, as spawn_agent starts one of yours, and get its final answer. Give it the whole task with all the context it needs: it can't see your conversation. Its agent_type is one of:",
    );
    for o in others {
        d += &format!("\n- {}: {}", o.name, o.description);
    }
    let names: Vec<&str> = others.iter().map(|o| o.name.as_str()).collect();
    json!({
        "name": RUN_AGENT,
        "description": d,
        "inputSchema": {
            "type": "object",
            "properties": {
                "agent_type": {"type": "string", "enum": names, "description": "Which model, or tier."},
                "message": {"type": "string", "description": "The task, in full."},
                "background": {"type": "boolean", "description": "Return at once; its answer comes as a message when it's done."},
            },
            "required": ["agent_type", "message"],
            "additionalProperties": false,
        },
    })
}

/// A thread's other models, and who runs them.
pub struct Others {
    pub delegate: DelegateRef,
    names: HashSet<String>,
}

impl Others {
    /// The thread's, when it has a delegate and models to offer.
    pub fn of(others: &[OtherAgent], delegate: Option<DelegateRef>) -> Option<Others> {
        let delegate = delegate?;
        if others.is_empty() {
            return None;
        }
        Some(Others {
            delegate,
            names: others.iter().map(|o| o.name.clone()).collect(),
        })
    }

    /// The call an item/tool/call request's params make, when it's
    /// run_agent's for one of the thread's models; Err says what's wrong
    /// with a run_agent call; None for another tool's.
    pub fn call(&self, p: &Value) -> Option<Result<DelegateCall, String>> {
        if p["tool"].as_str() != Some(RUN_AGENT) {
            return None;
        }
        let args = match &p["arguments"] {
            Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
            v => v.clone(),
        };
        let agent = args["agent_type"].as_str().unwrap_or_default();
        let message = args["message"].as_str().unwrap_or_default();
        if !self.names.contains(agent) {
            let mut names: Vec<&str> = self.names.iter().map(String::as_str).collect();
            names.sort_unstable();
            return Some(Err(format!(
                "no agent type {agent:?}: name one of {}",
                names.join(", ")
            )));
        }
        if message.trim().is_empty() {
            return Some(Err("run_agent needs a message: the whole task".into()));
        }
        Some(Ok(DelegateCall {
            agent: agent.into(),
            prompt: message.into(),
            background: args["background"].as_bool().unwrap_or(false),
            step: p["callId"].as_str().unwrap_or_default().into(),
        }))
    }
}

/// item/tool/call's result: the text, and whether the call worked.
pub fn result(text: &str, ok: bool) -> Value {
    json!({"contentItems": [{"type": "inputText", "text": text}], "success": ok})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Never;

    impl rush_core::adapter::Delegate for Never {
        fn run(&self, _: DelegateCall) -> Result<String, String> {
            Err("not here".into())
        }
    }

    fn others() -> Vec<OtherAgent> {
        vec![
            OtherAgent {
                name: "opus".into(),
                description: "Anthropic's; top tier.".into(),
            },
            OtherAgent {
                name: "quick".into(),
                description: "The quick tier.".into(),
            },
        ]
    }

    #[test]
    fn the_tool_lists_every_type() {
        let t = tool_spec(&others());
        assert_eq!(t["name"], RUN_AGENT);
        assert_eq!(
            t["inputSchema"]["properties"]["agent_type"]["enum"],
            json!(["opus", "quick"])
        );
        let d = t["description"].as_str().unwrap();
        assert!(
            d.ends_with("\n- opus: Anthropic's; top tier.\n- quick: The quick tier."),
            "{d}"
        );
    }

    #[test]
    fn calls_read_as_the_model_made_them() {
        let o = Others::of(&others(), Some(Arc::new(Never) as DelegateRef)).unwrap();
        for (params, want) in [
            (
                json!({"tool": "run_agent", "callId": "c1", "arguments": {"agent_type": "opus", "message": "design it"}}),
                Some(Ok(("opus", "design it", false, "c1"))),
            ),
            (
                json!({"tool": "run_agent", "callId": "c2", "arguments": "{\"agent_type\":\"quick\",\"message\":\"grep\",\"background\":true}"}),
                Some(Ok(("quick", "grep", true, "c2"))),
            ),
            (
                json!({"tool": "run_agent", "arguments": {"agent_type": "kimi", "message": "x"}}),
                Some(Err("no agent type \"kimi\": name one of opus, quick")),
            ),
            (
                json!({"tool": "run_agent", "arguments": {"agent_type": "opus", "message": " "}}),
                Some(Err("run_agent needs a message: the whole task")),
            ),
            (json!({"tool": "other"}), None),
            (json!({}), None),
        ] {
            let got = o
                .call(&params)
                .map(|r| r.map(|c| (c.agent, c.prompt, c.background, c.step)));
            let want = want.map(|r| {
                r.map(|(a, p, b, s)| (a.to_string(), p.to_string(), b, s.to_string()))
                    .map_err(str::to_string)
            });
            assert_eq!(got, want, "{params}");
        }
        assert!(Others::of(&[], Some(Arc::new(Never) as DelegateRef)).is_none());
        assert!(Others::of(&others(), None).is_none());
    }

    #[test]
    fn results_are_text() {
        assert_eq!(
            result("done", true),
            json!({"contentItems": [{"type": "inputText", "text": "done"}], "success": true})
        );
    }
}
