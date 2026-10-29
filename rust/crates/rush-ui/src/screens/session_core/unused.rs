//! What a session carries in its context and has never used: MCP servers,
//! subagents and skills, the costliest first, for #slim to offer to drop.
//! After Go's internal/convo/slim.go (Unused :25, used :62, mcpID :88),
//! worked out from the host's count of the context (convo/model.go:496
//! sess.Usage) rather than kept on the conversation.

use std::collections::{HashMap, HashSet};

use rush_convo::Session;
use rush_core::tool::Kind;
use rush_core::usage::Context;

use crate::screens::session_cards::slim::Unused;

/// What u holds that s hasn't called once, the costliest first. rush's
/// own server stays: it's how rush draws.
pub fn unused(s: &Session, u: &Context) -> Vec<Unused> {
    let (servers, agents, skills) = used(s);
    let mut out = Vec::new();
    let mut cost: HashMap<&str, i64> = HashMap::new();
    let mut names: Vec<&str> = Vec::new();
    for t in &u.mcp_tools {
        if !cost.contains_key(t.server.as_str()) {
            names.push(&t.server);
            cost.insert(&t.server, 0);
        }
        if t.is_loaded {
            // One loaded on demand costs nothing until it is.
            *cost.entry(&t.server).or_default() += t.tokens;
        }
    }
    for n in names {
        let id = mcp_id(n);
        if id != "rush" && !servers.contains(&id) {
            out.push(Unused {
                what: "MCP server".into(),
                name: n.into(),
                tokens: cost[n],
                rules: vec![format!("mcp__{id}")],
            });
        }
    }
    for a in &u.agents {
        if !agents.contains(&a.kind) {
            out.push(Unused {
                what: "subagent".into(),
                name: a.kind.clone(),
                tokens: a.tokens,
                rules: vec![format!("Task({})", a.kind), format!("Agent({})", a.kind)],
            });
        }
    }
    for sk in &u.skills.each {
        if !skills.contains(&sk.name) {
            out.push(Unused {
                what: "skill".into(),
                name: sk.name.clone(),
                tokens: sk.tokens,
                rules: vec![format!("Skill({})", sk.name)],
            });
        }
    }
    out.sort_by_key(|u| std::cmp::Reverse(u.tokens));
    out
}

/// The MCP servers (by id), subagents and skills s has called.
fn used(s: &Session) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let (mut servers, mut agents, mut skills) = (HashSet::new(), HashSet::new(), HashSet::new());
    for name in s.tools.keys() {
        if let Some(rest) = name.strip_prefix("mcp__") {
            let id = rest.split_once("__").map_or(rest, |(id, _)| id);
            servers.insert(id.to_string());
        }
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct In {
        skill: String,
        command: String,
    }
    for st in s.steps() {
        if st.kind == Kind::Subagent {
            agents.insert(rush_convo::model::agent_name(st));
        } else if &*st.tool == "Skill"
            && let Ok(i) = serde_json::from_str::<In>(st.input_json())
        {
            let name = if i.skill.is_empty() {
                i.command.trim_start_matches('/').to_string()
            } else {
                i.skill
            };
            skills.insert(name);
        }
    }
    (servers, agents, skills)
}

/// A server's name as its tools are named, mcp__<id>__<tool>: anything
/// but letters, digits, _ and - made _.
fn mcp_id(name: &str) -> String {
    name.chars()
        .map(|r| {
            if r.is_ascii_alphanumeric() || r == '_' || r == '-' {
                r
            } else {
                '_'
            }
        })
        .collect()
}
