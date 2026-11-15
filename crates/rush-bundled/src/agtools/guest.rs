//! The ask_guest tool: an agent in a conversation addresses another
//! participant by name (the lead, a guest, or an agent that joins as one
//! on being named), whose answer shows in the conversation (rush-host's
//! guests, docs/guests.md). Offered where the session's Agents asks
//! guests. Rust-only.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use super::{Agents, Tool, result};

/// The tool's own name.
pub const ASK_GUEST: &str = "ask_guest";

fn yes() -> bool {
    true
}

/// What a session sends ask_guest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuestInput {
    /// Whom: a name in the chat, or an agent's ("kimi", "sol", "claude
    /// haiku").
    pub name: String,
    pub prompt: String,
    /// Wait for the answer (the default); false returns at once and the
    /// answer comes as a message.
    #[serde(default = "yes")]
    pub wait: bool,
}

impl Default for GuestInput {
    fn default() -> Self {
        GuestInput {
            name: String::new(),
            prompt: String::new(),
            wait: true,
        }
    }
}

/// Asks the participants of one session's conversation.
pub trait Guests: Send + Sync {
    fn ask_guest(&self, input: GuestInput) -> Result<String, String>;
}

pub(crate) fn guest_tool() -> Tool {
    Tool {
        name: ASK_GUEST.into(),
        description: "Ask someone in this conversation by name: its own agent (the lead), another model that's a guest here, or \
any agent rush runs (\"kimi\", \"sol\", \"glm\", \"claude haiku\"), which joins as a guest the first time it's named. They see \
the conversation so far and answer in it, where the user sees them; only one participant changes files at a time. \
Use it when the user asks you to involve another model, or for a second opinion they'd want to see. By default it \
waits and returns the answer; wait false returns at once and the answer comes to you as a message. rush stops asks \
between agents after a few without the user."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Whom to ask: a name in the conversation, or an agent's (\"kimi\", \"sol\", \"claude haiku\")." },
                "prompt": { "type": "string", "description": "What to ask them." },
                "wait": { "type": "boolean", "description": "Wait for the answer (default true)." },
            },
            "required": ["name", "prompt"],
            "additionalProperties": false,
        }),
        meta: None,
    }
}

/// The ask_guest tool, where ag offers it.
pub(crate) fn guest_list(ag: &dyn Agents) -> Vec<Tool> {
    if ag.guests().is_some() {
        vec![guest_tool()]
    } else {
        Vec::new()
    }
}

/// Runs ask_guest; None when name isn't it, or ag doesn't offer it.
pub(crate) fn call_guest(
    ag: Option<&dyn Agents>,
    name: &str,
    args: Option<&RawValue>,
) -> Option<Value> {
    if name != ASK_GUEST {
        return None;
    }
    let g = ag?.guests()?;
    let raw = args.map_or("", |a| a.get()).as_bytes();
    let input = match rush_state::json::from_slice::<GuestInput>(raw) {
        Ok(i) => i,
        Err(e) => return Some(result(&format!("Its input: {e}"), true)),
    };
    Some(match g.ask_guest(input) {
        Ok(text) => result(&text, false),
        Err(e) => result(&e, true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_unless_told() {
        for (raw, want) in [
            (r#"{"name":"kimi","prompt":"look"}"#, true),
            (r#"{"name":"kimi","prompt":"look","wait":false}"#, false),
        ] {
            let i: GuestInput = rush_state::json::from_slice(raw.as_bytes()).unwrap();
            assert_eq!((i.name.as_str(), i.wait), ("kimi", want), "{raw}");
        }
    }
}
