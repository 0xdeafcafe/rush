//! The agent tools let a session hand work to any agent rush runs, of any
//! harness: spawn_agent starts one as a rush session of its own, the
//! caller's child, and agent_result and agent_send follow it up. See
//! docs/subagents.md.
//! Ported from internal/agtools/agents.go:1-181.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use super::{Tool, result};

// The agent tools' own names.
pub const SPAWN_AGENT: &str = "spawn_agent";
pub const AGENT_RESULT: &str = "agent_result";
pub const AGENT_SEND: &str = "agent_send";

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// What a session sends spawn_agent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpawnInput {
    pub agent: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
    /// The tool call that asked, when the harness says which (Claude's
    /// _meta claudecode/toolUseId): the UI draws the child under it.
    #[serde(skip)]
    pub step: String,
}

/// What a session sends agent_result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResultInput {
    pub id: String,
    #[serde(rename = "wait_seconds", skip_serializing_if = "is_zero")]
    pub wait: i64,
}

/// What a session sends agent_send.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SendInput {
    pub id: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
}

/// Keeps a session's views, the panel rush draws beside its conversation.
pub trait Viewer: Send + Sync {
    fn set_view(&self, name: &str, view: &RawValue) -> Result<String, String>;
    fn clear_view(&self, name: &str) -> Result<(), String>;
}

/// Runs the agent tools for one session.
pub trait Agents: Send + Sync {
    /// The agents spawn_agent can start, a line each.
    fn catalogue(&self) -> String;
    fn spawn(&self, input: SpawnInput) -> Result<String, String>;
    fn result(&self, input: ResultInput) -> Result<String, String>;
    fn send(&self, input: SendInput) -> Result<String, String>;
    /// Its views, when it keeps them (Go's Viewer, found by assertion).
    fn viewer(&self) -> Option<&dyn Viewer> {
        None
    }
    /// A run no one watches in rush: it isn't given the tools that draw
    /// (show, view). Go's Headless.
    fn headless(&self) -> bool {
        false
    }
}

/// Whether a tool call named name is spawn_agent, by any harness's name
/// for it: mcp__rush__spawn_agent, rush.spawn_agent.
pub fn is_spawn(name: &str) -> bool {
    name == SPAWN_AGENT
        || name.ends_with(&format!("_{SPAWN_AGENT}"))
        || name.ends_with(&format!(".{SPAWN_AGENT}"))
        || name.ends_with(&format!("/{SPAWN_AGENT}"))
}

/// Reads a spawn_agent call's input, as the agent sent it.
pub fn read_spawn(raw: &[u8]) -> Option<SpawnInput> {
    if raw.is_empty() {
        return None;
    }
    let input: SpawnInput = rush_state::json::from_slice(raw).ok()?;
    (!input.agent.is_empty()).then_some(input)
}

fn schema(props: Value, required: &[&str]) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

fn prop(kind: &str, description: &str) -> Value {
    serde_json::json!({ "type": kind, "description": description })
}

pub(crate) fn agent_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: SPAWN_AGENT.into(),
            description: "Hand a task to another agent, of any provider and harness rush runs, as your subagent. It runs as a \
session of its own, which the user sees under yours with its every step. By default the call waits and returns \
the agent's final answer. With background, it returns the agent's id at once; you are sent its answer as a \
message when it finishes, or can ask with agent_result. Give it the whole task with all the context it needs: \
it can't see your conversation. Pick an agent by what the work needs: a cheaper or faster model for routine \
work, another provider for a second opinion. This is how you reach other providers' models (OpenAI's in Codex, \
Kimi, GLM, DeepSeek, Gemini, Ollama and the rest): when you're asked to \"use astra\" or \"have sol review it\", \
call this tool rather than looking for another way. Name an agent as listed, or just its model, harness or both \
(\"gpt-6.1-sol\", \"sol 6.1\", \"codex\", \"codex astra\"): rush finds it, and if nothing fits it says what does. \
If one can't start, use another and tell the user; never debug its sign-in."
                .into(),
            input_schema: schema(
                serde_json::json!({
                    "agent": prop("string", "Which agent: a name listed, or a model, harness or provider by its name (\"sol 6.1\", \"codex\")."),
                    "prompt": prop("string", "The task, in full."),
                    "model": prop("string", "A model of that agent's other than the one its name picks. Rarely needed."),
                    "effort": prop("string", "Reasoning effort, for a model that takes one: low, medium, high."),
                    "background": prop("boolean", "Return at once; its answer comes as a message when it's done."),
                }),
                &["agent", "prompt"],
            ),
            meta: Some(serde_json::json!({ "anthropic/alwaysLoad": true })),
        },
        Tool {
            name: AGENT_RESULT.into(),
            description: "How an agent you started with spawn_agent stands, and its answer once it has finished. wait_seconds \
waits up to that long for it to finish (at most 600); 0 answers at once."
                .into(),
            input_schema: schema(
                serde_json::json!({
                    "id": prop("string", "The agent's id, as spawn_agent returned it."),
                    "wait_seconds": prop("integer", "How long to wait for it to finish."),
                }),
                &["id"],
            ),
            meta: None,
        },
        Tool {
            name: AGENT_SEND.into(),
            description: "Send a follow-up to an agent you started with spawn_agent: it carries on with its own conversation. \
Waits for its answer unless background."
                .into(),
            input_schema: schema(
                serde_json::json!({
                    "id": prop("string", "The agent's id, as spawn_agent returned it."),
                    "prompt": prop("string", "What to tell it."),
                    "background": prop("boolean", "Return at once; its answer comes as a message."),
                }),
                &["id", "prompt"],
            ),
            meta: None,
        },
    ]
}

/// The agent tools, spawn_agent's description naming the agents ag can
/// start.
pub(crate) fn agent_list(ag: &dyn Agents) -> Vec<Tool> {
    let mut out = agent_tools();
    let c = ag.catalogue();
    let c = c.trim();
    if !c.is_empty() {
        out[0].description.push_str("\n\nAgents:\n");
        out[0].description.push_str(c);
    }
    out
}

/// Runs an agent tool; None when name is none of them.
pub(crate) fn call_agent(
    ag: Option<&dyn Agents>,
    name: &str,
    args: Option<&RawValue>,
    step: &str,
) -> Option<Value> {
    let ag = ag?;
    let raw = args.map_or("", |a| a.get()).as_bytes();
    let got: Result<String, String> = match name {
        SPAWN_AGENT => rush_state::json::from_slice::<SpawnInput>(raw)
            .map_err(|e| e.to_string())
            .and_then(|mut input| {
                input.step = step.to_string();
                ag.spawn(input)
            }),
        AGENT_RESULT => rush_state::json::from_slice::<ResultInput>(raw)
            .map_err(|e| e.to_string())
            .and_then(|input| ag.result(input)),
        AGENT_SEND => rush_state::json::from_slice::<SendInput>(raw)
            .map_err(|e| e.to_string())
            .and_then(|input| ag.send(input)),
        _ => return None,
    };
    Some(match got {
        Ok(text) => result(&text, false),
        Err(e) => result(&e, true),
    })
}

/// What rush's tools run as a process need of rush's environment, as
/// ACP's MCP server env: an agent may start them with almost none.
pub fn env() -> Vec<BTreeMap<String, String>> {
    ["RUSH_HOME", "RUSH_CACHE"]
        .into_iter()
        .filter_map(|k| {
            let v = std::env::var(k).ok().filter(|v| !v.is_empty())?;
            Some(BTreeMap::from([
                ("name".to_string(), k.to_string()),
                ("value".to_string(), v),
            ]))
        })
        .collect()
}
