//! What rush sends a plugin and answers it, as types. Every field is
//! optional on the wire, so each reads as empty when it's missing, and a
//! field rush adds later is ignored.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// A list or map sent as null (as an empty one may be) reads as empty.
pub fn nullable<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Option::unwrap_or_default)
}

/// `initialize`'s params: what the plugin was approved for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Initialize {
    pub protocol: i64,
    pub name: String,
    /// The plugin's one writable folder, also `$RUSH_PLUGIN_DATA`.
    pub data_dir: String,
    #[serde(deserialize_with = "nullable")]
    pub sessions: Vec<String>,
    /// Resolved to real paths.
    #[serde(deserialize_with = "nullable")]
    pub workspaces: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    pub network: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    pub exec: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    pub ui: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    pub commands: Vec<String>,
    /// Each setting's value: what the user set, or its default.
    #[serde(deserialize_with = "nullable")]
    pub settings: BTreeMap<String, String>,
}

/// `tools.call`'s params.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolCall {
    /// rush's id for the session whose Claude called the tool.
    pub session: String,
    /// Claude Code's id for its conversation, when rush knows it.
    pub session_id: String,
    pub cwd: String,
    /// What the session was started with, by `sessions.start`.
    #[serde(deserialize_with = "nullable")]
    pub meta: BTreeMap<String, String>,
    pub name: String,
    pub arguments: Value,
}

/// A session as `sessions.list`, `sessions.watch` and `session.changed`
/// give it. Never what was said.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    /// Claude Code's id for the conversation, empty until it has started.
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    /// The top of the git checkout cwd is in.
    pub repo: String,
    pub branch: String,
    /// Whether the checkout is a linked worktree.
    pub worktree: bool,
    /// `starting`, `working`, `blocked`, `idle` or `stopped`.
    pub state: String,
    pub detail: String,
    pub needs: String,
    pub model: String,
    pub permission_mode: String,
    pub cost_usd: f64,
    pub context_tokens: i64,
    pub queued: i64,
    /// The plugin that started it, or empty.
    pub started_by: String,
    #[serde(deserialize_with = "nullable")]
    pub meta: BTreeMap<String, String>,
    pub started_at: String,
    pub updated_at: String,
}

impl Session {
    /// Whether it's still there to work: not stopped.
    pub fn live(&self) -> bool {
        self.state != "stopped"
    }
}

/// `session.event`'s params, for a session the plugin follows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionEvent {
    pub session: String,
    /// `info`, `sent`, `text`, `tool`, `result` or `closed`.
    #[serde(rename = "type")]
    pub kind: String,
    pub state: String,
    pub detail: String,
    pub needs: String,
    pub text: String,
    pub name: String,
    pub doing: String,
    pub is_error: bool,
    pub cost_usd: f64,
    pub turns: i64,
}

/// A session as a `ui.event` or `ui.command` names it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiSession {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub agent: String,
    pub cwd: String,
    pub repo: String,
    pub branch: String,
    pub state: String,
    pub hosted: bool,
}

/// Why a session stopped, in a `session.stopped` event.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StopError {
    /// `limit`, `auth`, `offline`, `retryable`, `too-long` or `other`.
    pub kind: String,
    pub message: String,
    /// rush is continuing the session itself.
    pub retrying: bool,
}

/// `ui.event`'s params: something that happened in one of rush's windows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiEvent {
    pub kind: String,
    /// The window it happened in.
    pub ui: String,
    pub at: String,
    /// None for the Prompt's box, and for `network.*`.
    pub session: Option<UiSession>,
    /// The message box's text, for `input.*` events only.
    pub text: String,
    pub error: Option<StopError>,
}

/// `ui.command`'s params: the user ran one of the plugin's commands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiCommand {
    pub plugin: String,
    pub command: String,
    pub ui: String,
    pub session: Option<UiSession>,
    /// Whose message box had the keys: a session's id, or "" the Prompt's.
    #[serde(rename = "box")]
    pub input_box: String,
    pub input: Option<Value>,
}

/// `ui.settings`' params: every setting's value now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    #[serde(deserialize_with = "nullable")]
    pub values: BTreeMap<String, String>,
}

/// What `exec` answers: a non-zero exit is a result, not an error.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecResult {
    pub code: i64,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

/// What `sessions.start` answers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Started {
    pub id: String,
    /// Where the session runs: the worktree's folder, if it made one.
    pub cwd: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_what_rush_sends() {
        let s: Session = serde_json::from_value(json!({
            "id": "a1b2c3d4", "sessionId": "5f0c", "repo": "/r", "worktree": true,
            "state": "working", "costUsd": 0.42, "startedBy": "kanban",
            "meta": {"card": "c1"}, "somethingNew": 1
        }))
        .unwrap();
        let none: Session = serde_json::from_value(json!({"id": "x", "meta": null})).unwrap();
        assert!(none.meta.is_empty());
        assert_eq!(s.session_id, "5f0c");
        assert!(s.worktree && s.live());
        assert_eq!(s.meta["card"], "c1");

        let ev: UiEvent = serde_json::from_value(json!({
            "kind": "session.stopped", "ui": "main",
            "session": {"id": "a1", "name": "fix it"},
            "error": {"kind": "offline", "message": "…", "retrying": true}
        }))
        .unwrap();
        assert_eq!(ev.session.unwrap().name, "fix it");
        assert!(ev.error.unwrap().retrying);

        let ev: SessionEvent =
            serde_json::from_value(json!({"session": "a1", "type": "result", "isError": true}))
                .unwrap();
        assert_eq!(ev.kind, "result");
        assert!(ev.is_error);
    }
}
