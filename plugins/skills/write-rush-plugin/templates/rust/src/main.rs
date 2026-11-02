//! A minimal rush plugin in Rust, on rush's plugin crate.
//!
//! It offers Claude three tools: note and notes, which keep notes in the
//! plugin's data folder (the only place it may write), and agents, which
//! calls rush back to list its agents. Rename it, change the tools, and keep
//! the plumbing: rush speaks JSON-RPC 2.0 on fd 3, each message a 4-byte
//! big-endian length and then that many bytes of JSON, and
//! `rush_plugin_sdk::serve` answers it.

use std::io::Write;
use std::path::PathBuf;

use rush_plugin_sdk::proto::{Session, ToolCall};
use rush_plugin_sdk::{Conn, Error, params, tool};
use serde::Deserialize;
use serde_json::{Value, json};

// --- tools: change these ---

fn tools() -> Value {
    json!([
        tool::tool(
            "note",
            "Keep a note for later sessions.",
            tool::schema(&[("text", "The note.")], &["text"]),
        ),
        tool::tool(
            "notes",
            "Read every note kept so far.",
            tool::schema(&[], &[]),
        ),
        tool::tool(
            "agents",
            "List the agents running in rush.",
            tool::schema(&[], &[]),
        ),
    ])
}

/// Runs one tool. session is the rush session that called it. An error
/// goes back to Claude as a failed tool call, in words it can act on.
fn call_tool(conn: &Conn, session: &str, name: &str, args: Value) -> Result<String, String> {
    let file = data_dir().join("notes.txt");
    match name {
        "note" => {
            #[derive(Deserialize)]
            struct In {
                text: String,
            }
            let input: In = serde_json::from_value(args).map_err(|_| "text is required")?;
            if input.text.trim().is_empty() {
                return Err("text is required".into());
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)
                .map_err(|e| e.to_string())?;
            writeln!(f, "[{session}] {}", input.text).map_err(|e| e.to_string())?;
            Ok("Noted.".into())
        }
        "notes" => match std::fs::read_to_string(&file) {
            Ok(s) if !s.is_empty() => Ok(s),
            Ok(_) => Ok("No notes yet.".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("No notes yet.".into()),
            Err(e) => Err(e.to_string()),
        },
        "agents" => {
            // Calling rush back: needs "list" in the manifest's "sessions".
            let all: Vec<Session> = conn.call("sessions.list", &()).map_err(|e| e.to_string())?;
            if all.is_empty() {
                return Ok("No agents.".into());
            }
            Ok(all
                .iter()
                .map(|s| format!("{} {:<8} {}: {}\n", s.id, s.state, s.name, s.detail))
                .collect())
        }
        _ => Err(format!("no tool named {name}")),
    }
}

/// Answers rush's requests and takes its notifications.
fn handle(conn: &Conn, method: &str, p: Value) -> Result<Value, Error> {
    match method {
        // params: protocol, name, dataDir, sessions, workspaces, network,
        // settings (see rush_plugin_sdk::proto::Initialize).
        "initialize" => Ok(json!({})),
        "tools.list" => Ok(json!({"tools": tools()})),
        "tools.call" => {
            let c: ToolCall = params(p)?;
            Ok(tool::result(call_tool(
                conn,
                &c.session,
                &c.name,
                c.arguments,
            )))
        }
        // Sessions this plugin follows (sessions.subscribe) report here.
        "session.event" => Ok(Value::Null),
        _ => Err(Error::no_method(method)),
    }
}

/// The plugin's one writable folder.
fn data_dir() -> PathBuf {
    std::env::var_os("RUSH_PLUGIN_DATA")
        .map(PathBuf::from)
        .unwrap_or_default()
}

// --- plumbing: keep as is ---

fn main() {
    rush_plugin_sdk::serve(handle)
}
