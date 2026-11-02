//! The reminder tools let a session have rush send it a message at a time
//! it picks (remind) and see or cancel what it set (reminders): rush keeps
//! the clock, so the agent needn't stay awake or loop to come back later.
//! Ported from internal/agtools/remind.go:1-106.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use super::{Agents, Tool, result};

// The reminder tools' own names.
pub const REMIND_TOOL: &str = "remind";
pub const REMINDERS_TOOL: &str = "reminders";

/// What a session sends remind.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemindInput {
    pub at: String,
    pub text: String,
}

/// What a session sends reminders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemindersInput {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cancel: String,
}

/// Keeps one session's reminders; the agent tools' Agents offer it when
/// they can.
pub trait Reminders: Send + Sync {
    fn remind(&self, input: RemindInput) -> Result<String, String>;
    fn reminders(&self, input: RemindersInput) -> Result<String, String>;
}

pub(crate) fn remind_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: REMIND_TOOL.into(),
            description: "Have rush send you a message at a time you pick, as a task due then: check a deploy in 20 minutes, \
follow up on a review this afternoon, look at a long job's results tomorrow at 9. rush keeps the clock and \
wakes you with the text when it falls due (queued behind your turn if you are at work), so end your turn \
instead of waiting, sleeping or looping. Write the text as a note to your future self with all it needs: \
what to do and where, since by then you may remember little else. Returns the reminder's id."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "at": { "type": "string", "description": "When it's due: a wait (\"20m\", \"2h\", \"1h30m\"), a time of day \
in the user's zone (\"15:30\", \"3pm\", \"tomorrow 9:00\"; the next one to come), or a date and time (\"2026-11-02 09:00\", RFC 3339)." },
                    "text": { "type": "string", "description": "What to do when it's due, written for you to act on then." },
                },
                "required": ["at", "text"],
                "additionalProperties": false,
            }),
            meta: None,
        },
        Tool {
            name: REMINDERS_TOOL.into(),
            description: "List the reminders you set with remind, soonest first, or cancel one by its id (\"all\" cancels every one)."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "cancel": { "type": "string", "description": "A reminder's id to cancel, or \"all\"." },
                },
                "additionalProperties": false,
            }),
            meta: None,
        },
    ]
}

/// The reminder tools, where ag keeps reminders.
pub(crate) fn remind_list(ag: &dyn Agents) -> Vec<Tool> {
    if ag.reminders().is_some() {
        remind_tools()
    } else {
        Vec::new()
    }
}

/// Runs a reminder tool; None when name isn't one, or ag keeps no
/// reminders.
pub(crate) fn call_remind(
    ag: Option<&dyn Agents>,
    name: &str,
    args: Option<&RawValue>,
) -> Option<Value> {
    let r = ag?.reminders()?;
    let raw = args.map_or("", |a| a.get()).as_bytes();
    let got = match name {
        REMIND_TOOL => match rush_state::json::from_slice::<RemindInput>(raw) {
            Ok(input) => r.remind(input),
            Err(e) => return Some(result(&format!("Its input: {e}"), true)),
        },
        REMINDERS_TOOL => {
            let input = if raw.is_empty() {
                RemindersInput::default()
            } else {
                match rush_state::json::from_slice::<RemindersInput>(raw) {
                    Ok(i) => i,
                    Err(e) => return Some(result(&format!("Its input: {e}"), true)),
                }
            };
            r.reminders(input)
        }
        _ => return None,
    };
    Some(match got {
        Ok(text) => result(&text, false),
        Err(e) => result(&e, true),
    })
}
