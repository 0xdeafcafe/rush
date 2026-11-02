//! delegate is an example rush plugin. It gives Claude tools to hand work
//! to agents of their own, in rush, and to follow them: start one, list
//! them, message one, and read what one said last.
//!
//! It speaks rush's plugin protocol: JSON-RPC 2.0 in length-prefixed frames
//! on fd 3. Build it into its folder and approve it:
//!
//! ```sh
//! cargo build --release -p delegate
//! mkdir -p ~/.config/rush/plugins/delegate
//! cp target/release/delegate plugins/examples/delegate/plugin.json ~/.config/rush/plugins/delegate/
//! rush plugin approve delegate
//! ```
//!
//! Ported from plugins/examples/delegate/main.go.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rush_plugin_sdk::proto::{Session, SessionEvent, Started, ToolCall};
use rush_plugin_sdk::{CallError, Conn, Error, params, tool};
use serde::Deserialize;
use serde_json::{Value, json};

/// What each agent this plugin started said last, and its state, as it
/// happens.
#[derive(Default)]
struct News {
    last: HashMap<String, String>,
    state: HashMap<String, String>,
}

fn main() {
    let news = Arc::new(Mutex::new(News::default()));
    rush_plugin_sdk::serve(move |conn, method, p| handle(conn, &news, method, p))
}

fn handle(conn: &Conn, news: &Mutex<News>, method: &str, p: Value) -> Result<Value, Error> {
    match method {
        "initialize" => Ok(json!({})),
        "tools.list" => Ok(json!({"tools": tools()})),
        "tools.call" => {
            let tc: ToolCall = params(p)?;
            Ok(tool::result(call(conn, news, &tc.name, tc.arguments)))
        }
        "session.event" => {
            let ev: SessionEvent = serde_json::from_value(p).unwrap_or_default();
            heard(&mut lock(news), ev);
            Ok(Value::Null)
        }
        _ => Err(Error::no_method(method)),
    }
}

/// Takes what one of its agents did.
fn heard(n: &mut News, ev: SessionEvent) {
    match ev.kind.as_str() {
        "text" | "result" if !ev.text.is_empty() => {
            n.last.insert(ev.session, ev.text);
        }
        "info" => {
            n.state.insert(ev.session, ev.state);
        }
        "closed" => {
            n.state.insert(ev.session, "stopped".into());
        }
        _ => {}
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Args {
    prompt: String,
    cwd: String,
    id: String,
    text: String,
}

fn call(conn: &Conn, news: &Mutex<News>, name: &str, args: Value) -> Result<String, CallError> {
    let a: Args = serde_json::from_value(args).unwrap_or_default();
    match name {
        "start_agent" => {
            let out: Started =
                conn.call("sessions.start", &json!({"cwd": a.cwd, "prompt": a.prompt}))?;
            conn.call::<_, Value>("sessions.subscribe", &json!({"id": out.id}))?;
            Ok(format!(
                "Started agent {}. Check on it with agent_result.",
                out.id
            ))
        }
        "list_agents" => {
            let all: Vec<Session> = conn.call("sessions.list", &())?;
            Ok(list(&all))
        }
        "message_agent" => {
            conn.call::<_, Value>("sessions.send", &json!({"id": a.id, "text": a.text}))?;
            // Following it again is harmless, and picks it back up after a
            // restart.
            let _ = conn.call::<_, Value>("sessions.subscribe", &json!({"id": a.id}));
            Ok("Sent.".into())
        }
        "agent_result" => result(&lock(news), &a.id).map_err(CallError::Other),
        _ => Err(CallError::Other(format!("no tool named {name}"))),
    }
}

/// Every agent, a line each.
fn list(all: &[Session]) -> String {
    let mut b = String::new();
    for s in all {
        b += &format!("{}  {:<8} {}  ({})", s.id, s.state, s.name, s.cwd);
        if !s.detail.is_empty() {
            b += &format!(": {}", s.detail);
        }
        if !s.started_by.is_empty() {
            b += &format!(" [started by {}]", s.started_by);
        }
        b.push('\n');
    }
    if b.is_empty() {
        return "No agents.".into();
    }
    b
}

/// The state of an agent it started, and what it said last.
fn result(n: &News, id: &str) -> Result<String, String> {
    let st = n.state.get(id).map_or("", String::as_str);
    let said = n.last.get(id).map_or("", String::as_str);
    if st.is_empty() && said.is_empty() {
        return Err(format!(
            "no news from {id} yet (only agents started with start_agent are followed)"
        ));
    }
    Ok(format!("State: {st}\nLast said:\n{said}"))
}

fn tools() -> Value {
    json!([
        tool::tool(
            "start_agent",
            "Start another Claude agent, in rush, on a task of its own. It runs alongside you; check on it with agent_result.",
            tool::schema(
                &[
                    ("prompt", "The task, as you'd give it to a colleague."),
                    ("cwd", "The absolute path of the folder it works in."),
                ],
                &["prompt", "cwd"],
            ),
        ),
        tool::tool(
            "list_agents",
            "List every rush agent: its id, state, name, folder and what it's doing.",
            tool::schema(&[], &[]),
        ),
        tool::tool(
            "message_agent",
            "Send a message to an agent you started.",
            tool::schema(
                &[("id", "The agent's id."), ("text", "The message.")],
                &["id", "text"],
            ),
        ),
        tool::tool(
            "agent_result",
            "The state of an agent you started, and the last thing it said.",
            tool::schema(&[("id", "The agent's id.")], &["id"]),
        ),
    ])
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn event(session: &str, kind: &str, text: &str, state: &str) -> SessionEvent {
        SessionEvent {
            session: session.into(),
            kind: kind.into(),
            text: text.into(),
            state: state.into(),
            ..Default::default()
        }
    }

    #[test]
    fn follows_what_its_agents_say() {
        let mut n = News::default();
        assert!(result(&n, "a1").unwrap_err().contains("no news from a1"));
        heard(&mut n, event("a1", "info", "", "working"));
        heard(&mut n, event("a1", "text", "Looking.", ""));
        heard(&mut n, event("a1", "result", "", "")); // says nothing new
        assert_eq!(
            result(&n, "a1").unwrap(),
            "State: working\nLast said:\nLooking."
        );
        heard(&mut n, event("a1", "closed", "", ""));
        assert!(result(&n, "a1").unwrap().starts_with("State: stopped\n"));
    }

    #[test]
    fn lists_agents() {
        let s = Session {
            id: "a1".into(),
            name: "fix it".into(),
            cwd: "/src".into(),
            state: "idle".into(),
            detail: "done".into(),
            started_by: "delegate".into(),
            ..Default::default()
        };
        assert_eq!(
            list(&[s]),
            "a1  idle     fix it  (/src): done [started by delegate]\n"
        );
        assert_eq!(list(&[]), "No agents.");
    }

    #[test]
    fn manifest_is_valid() {
        let m = rush_plugin_sdk::Manifest::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(m.agents.contains_key("reviewer"));
        assert_eq!(tools().as_array().unwrap().len(), 4);
    }
}
