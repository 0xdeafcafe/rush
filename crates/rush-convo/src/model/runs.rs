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
    const S: &str = "spawn_agent";
    tool == S
        || tool.len() > S.len()
            && tool.ends_with(S)
            && b"_./".contains(&tool.as_bytes()[tool.len() - S.len() - 1])
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
    let i: In = serde_json::from_str(st.input_json()).ok()?;
    (!i.agent.is_empty()).then_some(i.agent)
}

/// A step as a run's line says what it does: "asking Codex" for one
/// spawn_agent made, else its doing words, else its tool.
fn step_words(st: &Step, asking: bool) -> String {
    if let Some(name) = spawn_agent_name(st) {
        return if asking {
            format!("asking {name}")
        } else {
            format!("asked {name}")
        };
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
            Some(st) => (step_words(st, true), st.start),
            None => (String::new(), 0),
        }
    }

    /// What the session did last: its latest calls to have come back, in
    /// words, newest first, three at most.
    pub fn did(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.last_step(|st| {
            if st.status != Status::Running && st.status != Status::Waiting {
                out.push(step_words(st, false));
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
