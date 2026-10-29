//! One command from a client, a line of JSON on the socket: what Go's
//! client.go writes and host.go:1840 (op) reads, field for field.

use rush_core::event::Exchange;
use rush_core::wire::RawJson;
use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::wire::{Away, Branch, zero, zero_time};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Op {
    #[serde(rename = "expectedSession", skip_serializing_if = "String::is_empty")]
    pub expected_session: String,
    #[serde(rename = "expectedUpdatedAt", skip_serializing_if = "zero_time")]
    pub expected_updated_at: Time,
    pub op: String,
    /// The ev of what asked, in the asker's events.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cause: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange: Option<Box<Exchange>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "zero")]
    pub always: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<RawJson>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(skip_serializing_if = "zero")]
    pub interrupt: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "zero")]
    pub now: bool,
    /// send: into the turn under way, not stopping it.
    #[serde(skip_serializing_if = "zero")]
    pub guide: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "rush_core::wire::nullvec::deserialize")]
    pub images: Vec<String>,
    #[serde(skip_serializing_if = "zero")]
    pub index: i64,
    /// The queued text the client saw at index.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub was: String,
    #[serde(skip_serializing_if = "zero")]
    pub to: i64,
    /// What rewind leaves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
    /// ask: the control request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<RawJson>,
    /// hello: the client's protocol.
    #[serde(skip_serializing_if = "zero")]
    pub proto: i64,
    /// without: what the session goes without.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "rush_core::wire::nullvec::deserialize")]
    pub without: Vec<String>,
    /// away: None comes back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub away: Option<Away>,
}

impl Op {
    /// An op of this name and nothing else.
    pub fn named(op: &str) -> Op {
        Op {
            op: op.to_string(),
            ..Op::default()
        }
    }

    /// Reads one op; a null input or request reads as none.
    pub fn parse(b: &[u8]) -> serde_json::Result<Op> {
        let mut o: Op = serde_json::from_slice(b)?;
        if o.input.as_ref().is_some_and(|r| r.get() == "null") {
            o.input = None;
        }
        if o.request.as_ref().is_some_and(|r| r.get() == "null") {
            o.request = None;
        }
        Ok(o)
    }
}
