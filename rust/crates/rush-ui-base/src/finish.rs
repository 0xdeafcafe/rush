//! What each reading of the list said of each agent, and the lines from
//! your agents it noted (the feed's, kept to the last 30): what the
//! Session works out after each reading (session_core/finish.rs), the
//! header's clanker reacts with (crate::clanker) and the feed shows
//! (social/feed.rs). After Go's internal/ui/model.go:1946-2033 (notify's
//! state), feed.go:21 (noteEvent, agentEvent) and feed.go newsRe.

use std::collections::HashMap;

use regex::Regex;
use rush_core::agent::Agent;
use rush_core::time::Ms;

use crate::clanker::Reaction;
use crate::mention::mention_name;
use crate::model::Model;
use crate::style::one_line;

/// The feed keeps this many lines.
const KEPT: usize = 30;

/// What the last reading said of each agent.
#[derive(Default)]
pub struct Finish {
    /// Each agent's state as last read, by key; busy reads as working
    /// (lastState).
    pub last_state: HashMap<String, &'static str>,
    pub last_err: HashMap<String, bool>,
    pub last_stall: HashMap<String, Ms>,
    /// The lines from your agents, oldest first (m.events).
    pub events: Vec<AgentEvent>,
    /// What clanker is reacting to (m.fx).
    pub fx: Reaction,
    /// The decisions each agent was last notified of, by key: the same
    /// asked again isn't said again (toldCard).
    pub told_card: HashMap<String, String>,
    /// The notifications posted, as (title, body), in a test's sandbox,
    /// where none is really sent.
    pub told: Vec<(String, String)>,
}

/// An agent's @handle (agentHandle).
fn handle(a: &Agent) -> String {
    let host = a.rush.then_some(a.job.id.as_str());
    format!("@{}", mention_name(a, host))
}

/// Adds a line from your agents for agent a, keeping the last 30
/// (noteEvent).
pub fn note_event(m: &mut Model, a: &Agent, glyph: &str, what: &str) {
    let now = m.now;
    note(m.kept::<Finish>(), a, now, glyph, what.to_string());
}

#[doc(hidden)]
pub fn note(f: &mut Finish, a: &Agent, now: Ms, glyph: &str, what: String) {
    let said = one_line(&a.job.detail);
    f.events.push(AgentEvent {
        at: now,
        key: a.key.clone(),
        name: handle(a),
        glyph: glyph.into(),
        what: if what.is_empty() { said.clone() } else { what },
        said,
    });
    if f.events.len() > KEPT {
        let n = f.events.len() - KEPT;
        f.events.drain(..n);
    }
}

/// A line in the from-your-agents box: an agent finished, asked, or
/// failed (feed.go agentEvent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentEvent {
    pub at: Ms,
    pub key: String,
    pub name: String,
    pub glyph: String,
    pub what: String,
    /// Its last words then, for telling news from routine.
    pub said: String,
}

/// An agent's last words saying something went badly wrong or it found
/// something big: what lifts a finish out of the routine (newsRe).
pub fn news_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(broke|broken|lost|wiped|corrupt\w*|data loss|catastroph\w*|disaster|irrecoverabl\w*|can'?t recover|force[- ]push\w*|wtf|what the|root cause|found it|breakthrough|turns out|figured (it )?out|discover\w*|huge)\b")
            .expect("newsRe compiles")
    })
}

impl AgentEvent {
    /// Only an agent starting and stopping.
    pub fn routine(&self) -> bool {
        self.glyph == "✓" && !news_re().is_match(&self.said)
    }
}
