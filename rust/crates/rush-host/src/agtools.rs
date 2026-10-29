//! rush's own tools for a session, served by its host: SubAgents as
//! rush-bundled's agtools::Agents (with its views and reminders), the
//! in-process MCP server a session is started with, and `rush mcp-tools`,
//! the same server over stdio for an agent that runs its MCP servers as
//! processes.
//!
//! Ported from internal/host/subtool.go:41-50 (AgentTools),
//! internal/host/remind.go:12-53 (Remind, Reminders as agtools'),
//! internal/host/agent.go:73-75 (the session's ToolServer) and
//! cmd/rush/main.go:170-183 (mcp-tools).

use std::io::Write;
use std::sync::Arc;

use rush_bundled::agtools::{self, Agents, Reminders, Viewer};
use rush_core::adapter::{Handle, Registry, ToolServer};
use rush_core::wire::RawJson;
use serde_json::value::RawValue;

use crate::subtool::{self, SubAgents};

impl Agents for SubAgents {
    fn catalogue(&self) -> String {
        SubAgents::catalogue(self)
    }

    fn spawn(&self, input: agtools::SpawnInput) -> Result<String, String> {
        SubAgents::spawn(
            self,
            &subtool::SpawnInput {
                agent: input.agent,
                model: input.model,
                effort: input.effort,
                prompt: input.prompt,
                background: input.background,
                step: input.step,
            },
        )
    }

    fn start(&self, input: agtools::StartInput) -> Result<String, String> {
        SubAgents::start(
            self,
            &subtool::StartInput {
                agent: input.agent,
                model: input.model,
                effort: input.effort,
                prompt: input.prompt,
            },
        )
    }

    fn result(&self, input: agtools::ResultInput) -> Result<String, String> {
        SubAgents::result(
            self,
            &subtool::ResultInput {
                id: input.id,
                wait: input.wait,
            },
        )
    }

    fn send(&self, input: agtools::SendInput) -> Result<String, String> {
        SubAgents::send(
            self,
            &subtool::SendInput {
                id: input.id,
                prompt: input.prompt,
                background: input.background,
            },
        )
    }

    fn viewer(&self) -> Option<&dyn Viewer> {
        Some(self)
    }

    fn headless(&self) -> bool {
        SubAgents::headless(self)
    }

    fn reminders(&self) -> Option<&dyn Reminders> {
        Some(self)
    }
}

impl Viewer for SubAgents {
    fn set_view(&self, name: &str, view: &RawValue) -> Result<String, String> {
        SubAgents::set_view(self, name, view.get())
    }

    fn clear_view(&self, name: &str) -> Result<(), String> {
        SubAgents::clear_view(self, name)
    }
}

impl Reminders for SubAgents {
    fn remind(&self, input: agtools::RemindInput) -> Result<String, String> {
        SubAgents::remind(self, &input.at, &input.text)
    }

    fn reminders(&self, input: agtools::RemindersInput) -> Result<String, String> {
        SubAgents::reminders(self, &input.cancel)
    }
}

/// The agent tools for rush session id, or None when there's no session
/// to start them from (Go's AgentTools).
pub fn agent_tools(reg: Registry, id: &str) -> Option<Arc<dyn Agents>> {
    SubAgents::of(reg, id).map(|s| Arc::new(s) as Arc<dyn Agents>)
}

/// The MCP server rush serves session id in process: its tools never
/// ask, as they draw, or start what the Agent tool would. An agent that
/// runs its MCP servers as processes runs `rush mcp-tools --session id`.
pub fn tool_server(reg: Registry, id: &str) -> ToolServer {
    let h = agtools::handler(agent_tools(reg, id));
    let handle: Handle = Arc::new(move |msg: &RawValue| {
        let out = h(msg.get().as_bytes());
        RawJson::new(&String::from_utf8_lossy(&out))
            .unwrap_or_else(|_| RawJson::new("null").expect("null is JSON"))
    });
    ToolServer {
        name: agtools::SERVER.into(),
        trusted: agtools::names(),
        handle: Some(handle),
        args: vec!["mcp-tools".into(), "--session".into(), id.into()],
    }
}

/// `rush mcp-tools [--session <id>]`: rush's own tools over stdio, for a
/// session's agent that runs MCP servers as processes. Not meant to be
/// run by hand.
pub fn mcp_tools_cmd(reg: Registry, args: &[String], stderr: &mut dyn Write) -> i32 {
    let id = match args {
        [flag, id, ..] if flag == "--session" => id.as_str(),
        _ => "",
    };
    // The agents it starts find the real programs, not the session's
    // stand-ins.
    let path = crate::shim::without_shims(&std::env::var("PATH").unwrap_or_default());
    // SAFETY: before any thread of this command starts.
    unsafe { std::env::set_var("PATH", path) };
    let h = agtools::handler(agent_tools(reg, id));
    match agtools::serve(std::io::stdin(), std::io::stdout(), h) {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(stderr, "rush: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    fn rpc(ts: &ToolServer, msg: &str) -> serde_json::Value {
        let h = ts.handle.clone().expect("in process");
        let raw = RawValue::from_string(msg.into()).unwrap();
        serde_json::from_str(h(&raw).get()).unwrap()
    }

    fn session(id: &str, headless: bool) {
        std::fs::create_dir_all(crate::paths::dir(id)).unwrap();
        std::fs::write(
            crate::paths::dir(id).join("config.json"),
            format!(r#"{{"id":"{id}","kind":"claude","cwd":"/tmp","headless":{headless}}}"#),
        )
        .unwrap();
    }

    fn names(v: &serde_json::Value) -> Vec<String> {
        v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_session_is_served_every_tool() {
        let _h = Home::new();
        session("at-0000-aaaa", false);
        let ts = tool_server(rush_core::adapter::Registry::new(), "at-0000-aaaa");
        assert_eq!(ts.name, "rush");
        assert_eq!(ts.args, ["mcp-tools", "--session", "at-0000-aaaa"]);
        assert!(ts.trusted.contains(&"start_agent".to_string()));
        let got = names(&rpc(
            &ts,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        ));
        for want in [
            "show",
            "spawn_agent",
            "start_agent",
            "agent_result",
            "agent_send",
            "remind",
            "reminders",
            "view",
        ] {
            assert!(got.contains(&want.to_string()), "{want} in {got:?}");
        }
    }

    #[test]
    fn a_headless_session_draws_nothing() {
        let _h = Home::new();
        session("at-0000-bbbb", true);
        let ts = tool_server(rush_core::adapter::Registry::new(), "at-0000-bbbb");
        let got = names(&rpc(
            &ts,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        ));
        assert!(!got.contains(&"show".to_string()), "{got:?}");
        assert!(!got.contains(&"view".to_string()), "{got:?}");
        assert!(got.contains(&"start_agent".to_string()), "{got:?}");
    }

    #[test]
    fn no_session_is_show_alone() {
        let _h = Home::new();
        let ts = tool_server(rush_core::adapter::Registry::new(), "");
        let got = names(&rpc(
            &ts,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        ));
        assert_eq!(got, ["show"]);
    }

    #[test]
    fn reminders_and_views_reach_the_session() {
        let _h = Home::new();
        rush_remind::no_waiters(true);
        session("at-0000-cccc", false);
        let ts = tool_server(rush_core::adapter::Registry::new(), "at-0000-cccc");
        let v = rpc(
            &ts,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"remind","arguments":{"at":"20m","text":"check the deploy"}}}"#,
        );
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Set: reminder "), "{text}");
        let v = rpc(
            &ts,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"reminders","arguments":{}}}"#,
        );
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("check the deploy"), "{text}");
        let v = rpc(
            &ts,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"view","arguments":{"name":"tests","view":{"title":"Tests"}}}}"#,
        );
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            text,
            "Shown beside the conversation as tests. Set it again as things change; clear it once it's no use."
        );
    }
}
