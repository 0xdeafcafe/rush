//! A session's events.jsonl is what rush did with it and why, a line each:
//! the state it went into, the messages it was given and by whom, the
//! turns' ends, its agent starting and exiting. It's there to debug rush
//! by, not to show. Each line is in rush's own events.jsonl too
//! (rush-events).
//!
//! Ported from internal/host/events.go.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::op::Op;
use crate::paths::events_path;
use crate::wire::{Info, Limit, Retry};

/// Records an event of kind of session id's, with fields. It never waits:
/// any process may log, the UI's too.
pub fn log_event(id: &str, kind: &str, fields: Value) {
    if id.is_empty() {
        return;
    }
    let f = match fields {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    #[cfg(not(test))]
    rush_events::session(id, &events_path(id), kind, f);
    #[cfg(test)]
    test_logger().session(id, &events_path(id), kind, f);
}

/// Records an event in rush's own log.
pub fn log(kind: &str, fields: Value) {
    let f = match fields {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    #[cfg(not(test))]
    rush_events::log(kind, f);
    #[cfg(test)]
    test_logger().log(kind, f);
}

/// Waits, up to a second, for what's been logged to be written.
pub fn flush() {
    #[cfg(not(test))]
    rush_events::flush();
    #[cfg(test)]
    test_logger().flush();
}

/// Tests log sessions' events to their own folders only: never to rush's
/// log in the real rush folder.
#[cfg(test)]
fn test_logger() -> &'static rush_events::Logger {
    static L: std::sync::OnceLock<rush_events::Logger> = std::sync::OnceLock::new();
    L.get_or_init(|| rush_events::Logger::new(None))
}

/// What of Info the events follow: a change in any is logged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shown {
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub needs: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub sleeping: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<Limit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    #[serde(
        rename = "background",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "rush_core::wire::nullvec::deserialize"
    )]
    pub tasks: Vec<String>,
    pub queue: i64,
    #[serde(rename = "queueHeld")]
    pub held: bool,
    pub away: bool,
}

pub fn shown_of(i: &Info) -> Shown {
    Shown {
        state: i.state.clone(),
        needs: i.needs.clone(),
        error: i.error.clone(),
        sleeping: i.sleeping,
        limit: i.limit.clone(),
        retry: i.retry.clone(),
        tasks: i.background.iter().map(|t| t.id.clone()).collect(),
        queue: i.queue.len() as i64,
        held: i.queue_held,
        away: i.away.as_ref().is_some_and(|a| a.on()),
    }
}

/// The start of a message, enough to know it by.
pub fn clip(text: &str) -> String {
    let mut it = text.char_indices();
    match it.nth(300) {
        None => text.to_string(),
        Some((i, _)) => format!("{}…", &text[..i]),
    }
}

/// Ops asked of a session all the time, which change nothing.
const QUIET_OPS: [&str; 4] = ["send", "exchange", "hello", "context"];

/// The fields of the setting event an op a client asked of the session
/// is logged with, and what came of it; None for the quiet ones.
pub fn op_fields(o: &Op, err: Option<&str>) -> Option<Value> {
    if QUIET_OPS.contains(&o.op.as_str()) {
        return None;
    }
    let mut f = Map::new();
    f.insert("op".into(), o.op.clone().into());
    let mut put = |k: &str, v: Value, set: bool| {
        if set {
            f.insert(k.into(), v);
        }
    };
    put("id", o.id.clone().into(), !o.id.is_empty());
    put("mode", o.mode.clone().into(), !o.mode.is_empty());
    put("model", o.model.clone().into(), !o.model.is_empty());
    put("effort", o.effort.clone().into(), !o.effort.is_empty());
    put("text", clip(&o.text).into(), !o.text.is_empty());
    put("message", clip(&o.message).into(), !o.message.is_empty());
    put("index", o.index.into(), o.index != 0);
    put("to", o.to.into(), o.to != 0);
    put("without", o.without.clone().into(), !o.without.is_empty());
    put(
        "away",
        serde_json::to_value(&o.away).unwrap_or(Value::Null),
        o.op == "away",
    );
    put("interrupt", o.interrupt.into(), o.interrupt);
    put("always", o.always.into(), o.always);
    put("err", err.unwrap_or("").into(), err.is_some());
    put("cause", o.cause.clone().into(), !o.cause.is_empty());
    Some(Value::Object(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_is_written_as_go_writes_it() {
        let info = Info::parse(include_bytes!("../tests/fixtures/info_full.json")).unwrap();
        let want = include_str!("../tests/fixtures/shown.json");
        // Go's away is "on", which the fixture's ended away isn't.
        assert_eq!(
            String::from_utf8(crate::json::line(&shown_of(&info))).unwrap(),
            want
        );
    }

    #[test]
    fn clips_long_messages() {
        assert_eq!(clip("short"), "short");
        let long = "é".repeat(301);
        assert_eq!(clip(&long), "é".repeat(300) + "…");
    }

    #[test]
    fn quiet_ops_are_not_logged() {
        assert!(op_fields(&Op::named("send"), None).is_none());
        let mut o = Op::named("mode");
        o.mode = "plan".into();
        let f = op_fields(&o, Some("no")).unwrap();
        assert_eq!(f.as_object().unwrap().len(), 3);
        assert_eq!(f["op"], "mode");
        assert_eq!(f["mode"], "plan");
        assert_eq!(f["err"], "no");
    }
}
