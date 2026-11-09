//! What a session is doing and said last, in a line each, as the places
//! that list a run (the dock, the panel, the subagents view) say it.
//! Port of internal/convo/overview.go (LastAnswer :683, LastWords :698,
//! Doing :712, Did :736, lastStep :806) and spawn.go's toolRun (an agent
//! a spawn_agent call asked is said by its name). Go keeps a light
//! session for a run's numbers; here a run's whole conversation is read.

use rush_core::time::Ms;

use super::{Item, Session, Status, Step};
use crate::text::first_plain;

/// Whether a tool is rush's spawn_agent, as whichever harness names it
/// (agtools.IsSpawn).
pub fn is_spawn(tool: &str) -> bool {
    named(tool, SPAWN_AGENT)
}

/// rush's agent tools, as its MCP server names them (agtools/agents.rs).
pub const SPAWN_AGENT: &str = "spawn_agent";
pub const START_AGENT: &str = "start_agent";
pub const AGENT_RESULT: &str = "agent_result";
pub const AGENT_SEND: &str = "agent_send";
/// Emeritus's: the session hands its task over on a baton and retires.
pub use crate::emeritus::RETIRE;

/// Whether tool is n as whichever harness names it: n, mcp__rush__n,
/// rush.n, rush/n.
fn named(tool: &str, n: &str) -> bool {
    tool == n
        || tool.len() > n.len()
            && tool.ends_with(n)
            && b"_./".contains(&tool.as_bytes()[tool.len() - n.len() - 1])
}

/// Which of rush's own tools a call is, by its plain name, as whichever
/// harness names it: the agent tools by any name ending in theirs, the
/// rest (show, view, remind, reminders, retire) only under rush's server,
/// as another server may have a tool of that name. None for any other
/// tool.
pub fn rush_tool(tool: &str) -> Option<&'static str> {
    if let Some(n) = [SPAWN_AGENT, START_AGENT, AGENT_RESULT, AGENT_SEND]
        .into_iter()
        .find(|n| named(tool, n))
    {
        return Some(n);
    }
    let rest = ["mcp__rush__", "rush.", "rush/", "rush__"]
        .iter()
        .find_map(|p| tool.strip_prefix(p))?;
    ["show", "view", "remind", "reminders", RETIRE]
        .into_iter()
        .find(|n| *n == rest)
}

/// What a call sent under key, when it's a string; "" when it isn't.
pub fn input_str(st: &Step, key: &str) -> String {
    serde_json::from_str::<serde_json::Value>(&st.input_json())
        .ok()
        .and_then(|v| v.get(key).and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// The first line under a baton's Destination heading: what the task
/// is for, as the session handing it over put it. "" when it has none.
pub fn destination(baton: &str) -> String {
    let mut lines = baton.lines();
    for l in lines.by_ref() {
        let h = l.trim_start_matches('#').trim();
        if l.trim_start().starts_with('#') && h.eq_ignore_ascii_case("destination") {
            break;
        }
    }
    lines
        .map(str::trim)
        .find(|l| !l.is_empty())
        .filter(|l| !l.starts_with('#'))
        .unwrap_or("")
        .to_string()
}

/// The agent an agent_result or agent_send call is about, by the id
/// spawn_agent or start_agent gave it; None for any other step.
pub fn agent_id(st: &Step) -> Option<String> {
    let t = rush_tool(&st.tool)?;
    if t != AGENT_RESULT && t != AGENT_SEND {
        return None;
    }
    Some(input_str(st, "id")).filter(|id| !id.is_empty())
}

/// Whether id is in text as a word of its own, not part of a longer id.
fn has_word(text: &str, id: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    text.match_indices(id).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(word)
            && !text[i + id.len()..].chars().next().is_some_and(word)
    })
}

impl Session {
    /// The agent rush started as id, by the name the session's spawn_agent
    /// or start_agent call gave it ("astra"), once that call has said the
    /// id; else the name an agent_result or agent_send about id was
    /// answered with ("Codex (id) finished"); "" when nothing says.
    pub fn agent_called(&self, id: &str) -> String {
        if id.is_empty() {
            return String::new();
        }
        for st in self.steps() {
            let t = rush_tool(&st.tool);
            if (t == Some(SPAWN_AGENT) || t == Some(START_AGENT)) && has_word(&st.output, id) {
                let n = input_str(st, "agent");
                if !n.is_empty() {
                    return n;
                }
            }
        }
        let by = format!(" ({id}) ");
        for st in self.steps() {
            if agent_id(st).as_deref() == Some(id)
                && let Some(i) = st.output.find(&by)
            {
                let n = st.output[..i].trim();
                if !n.is_empty() && !n.contains('\n') {
                    return n.to_string();
                }
            }
        }
        String::new()
    }
}

/// What a call of one of rush's tools does, in a few plain words: while
/// it runs (now), or once it's back. The agent it's about is named as the
/// call or the session names it. None for any other tool.
pub fn rush_words(s: &Session, st: &Step, now: bool) -> Option<String> {
    let t = rush_tool(&st.tool)?;
    let about = |n: String| {
        if n.is_empty() {
            "an agent".to_string()
        } else {
            n
        }
    };
    let asked = || about(input_str(st, "agent"));
    let called = || {
        about(
            agent_id(st)
                .map(|id| s.agent_called(&id))
                .unwrap_or_default(),
        )
    };
    Some(match (t, now) {
        (SPAWN_AGENT, true) => format!("asking {}", asked()),
        (SPAWN_AGENT, false) => format!("asked {}", asked()),
        (START_AGENT, true) => format!("starting {}", asked()),
        (START_AGENT, false) => format!("started {}", asked()),
        (AGENT_RESULT, true) => format!("waiting on {}", called()),
        (AGENT_RESULT, false) => format!("heard from {}", called()),
        (AGENT_SEND, true) => format!("telling {}", called()),
        (AGENT_SEND, false) => format!("told {}", called()),
        ("show", _) => "drawing".into(),
        ("view", _) => "updating its view".into(),
        (RETIRE, true) => "handing over".into(),
        (RETIRE, false) => "asked to hand over".into(),
        ("remind", true) => "setting a reminder".into(),
        ("remind", false) => "set a reminder".into(),
        (_, true) => "reading its reminders".into(),
        (_, false) => "read its reminders".into(),
    })
}

/// The agent a spawn_agent call asked for, by the name it gave; None for
/// any other step (toolRun).
pub fn spawn_agent_name(st: &Step) -> Option<String> {
    if !is_spawn(&st.tool) {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct In {
        #[serde(default)]
        agent: String,
    }
    let i: In = serde_json::from_str(&st.input_json()).ok()?;
    (!i.agent.is_empty()).then_some(i.agent)
}

/// A step as a run's line says what it does: "asking Codex" for one
/// spawn_agent made, else its doing words, else its tool.
fn step_words(s: &Session, st: &Step, asking: bool) -> String {
    if let Some(w) = rush_words(s, st, asking) {
        return w;
    }
    let d = rush_core::tool::doing(st.call());
    if d.is_empty() { st.tool.to_string() } else { d }
}

impl Session {
    /// The latest step for which f holds, newest turn and item first.
    fn last_step(&self, mut f: impl FnMut(&Step) -> bool) -> Option<&Step> {
        for t in self.turns.iter().rev() {
            for it in t.items.iter().rev() {
                if let Item::Step(id) = it {
                    let st = self.step(*id);
                    if f(st) {
                        return Some(st);
                    }
                }
            }
        }
        None
    }

    /// The agent's n-th latest answer in full (1 is the last): the last
    /// words of that turn.
    pub fn last_answer(&self, mut n: usize) -> String {
        for t in self.turns.iter().rev() {
            if let Some(text) = t.items.iter().rev().find_map(|it| match it {
                Item::Text { text, .. } if !text.trim().is_empty() => Some(text),
                _ => None,
            }) {
                n = n.saturating_sub(1);
                if n == 0 {
                    return text.trim().to_string();
                }
            }
        }
        String::new()
    }

    /// The first line of the latest thing the session said.
    pub fn last_words(&self) -> String {
        for t in self.turns.iter().rev() {
            for it in t.items.iter().rev() {
                if let Item::Text { text, .. } = it
                    && !text.trim().is_empty()
                {
                    return first_plain(text);
                }
            }
        }
        String::new()
    }

    /// What the session is doing now: its latest tool call still out, in
    /// words, and since when; "" when none is out.
    pub fn doing(&self) -> (String, Ms) {
        match self.last_step(|st| st.status == Status::Running) {
            Some(st) => (step_words(self, st, true), st.start),
            None => (String::new(), 0),
        }
    }

    /// What the session did last: its latest calls to have come back, in
    /// words, newest first, three at most.
    pub fn did(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.last_step(|st| {
            if st.status != Status::Running && st.status != Status::Waiting {
                out.push(step_words(self, st, false));
            }
            out.len() == 3
        });
        out
    }

    /// How many tool calls the session made, its subagents' included.
    pub fn tool_calls(&self) -> usize {
        self.steps.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_agent_by_any_harness_name() {
        for (t, want) in [
            ("spawn_agent", true),
            ("mcp__rush__spawn_agent", true),
            ("rush.spawn_agent", true),
            ("rush/spawn_agent", true),
            ("xspawn_agent", false),
            ("Bash", false),
        ] {
            assert_eq!(is_spawn(t), want, "{t}");
        }
    }
}
