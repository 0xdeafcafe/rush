//! The retire tool (Emeritus, docs/emeritus.md): a session near the end
//! of its context hands its task over on a baton it writes. The call only
//! asks: the session retires once its turn ends, when rush has started
//! its successor, and carries on, told why, if that fails. Offered only
//! while Emeritus is on. Rust-only.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use super::{Agents, Tool, result};

/// The retire tool's own name.
pub const RETIRE_TOOL: &str = "retire";

/// What a session sends retire.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetireInput {
    pub baton: String,
}

/// Retires one session after its turn; the agent tools' Agents offer it
/// while Emeritus is on.
pub trait Retirer: Send + Sync {
    fn retire(&self, input: RetireInput) -> Result<String, String>;
}

pub(crate) fn retire_tool() -> Tool {
    Tool {
        name: RETIRE_TOOL.into(),
        description: "Hand your task over to a fresh agent and retire, when rush asks you to near the end of your context \
(or the user does). Write the baton first: your judgement of what your successor needs, in the sections rush gave \
you (Destination, Core ideas, Methods, Journey, Limits, Tests that pin text, Key files, Next action). rush adds the \
branch, HEAD, git status, diffstat, your transcript's path and cost. The call only asks: end your turn after it. \
Once it ends rush starts your successor in this folder on your model, mode and account, and you retire for good; \
if it can't start you carry on and are told why."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "baton": { "type": "string", "description": "The baton, in Markdown, one ## heading per section." },
            },
            "required": ["baton"],
            "additionalProperties": false,
        }),
        meta: None,
    }
}

/// The retire tool, where ag offers it.
pub(crate) fn retire_list(ag: &dyn Agents) -> Vec<Tool> {
    if ag.retirer().is_some() {
        vec![retire_tool()]
    } else {
        Vec::new()
    }
}

/// Runs the retire tool; None when name isn't it, or ag doesn't offer it.
pub(crate) fn call_retire(
    ag: Option<&dyn Agents>,
    name: &str,
    args: Option<&RawValue>,
) -> Option<Value> {
    if name != RETIRE_TOOL {
        return None;
    }
    let r = ag?.retirer()?;
    let raw = args.map_or("", |a| a.get()).as_bytes();
    let input = match rush_state::json::from_slice::<RetireInput>(raw) {
        Ok(i) => i,
        Err(e) => return Some(result(&format!("Its input: {e}"), true)),
    };
    Some(match r.retire(input) {
        Ok(text) => result(&text, false),
        Err(e) => result(&e, true),
    })
}
