//! What a session's agent says of its models (prices and windows) and when
//! its prompt cache goes cold: what session_core's Overview draws and the
//! agent header reads. After Go's internal/convo/overview.go (CacheCold
//! :93, Session.cost :639) and agent.ContextWindow.

use rush_convo::Session;
use rush_core::agent::TokenUsage;
use rush_core::time::Ms;

use crate::model::Model;

/// How long the main agent's prompt cache lives.
pub const CACHE_HOUR: Ms = 3_600_000;

/// What the session's agent says of its models: prices and windows
/// (Go's Session.cost, overview.go:639, and agent.ContextWindow).
pub struct Agentry<'a> {
    pub reg: &'a rush_core::adapter::Registry,
    pub kind: &'a str,
}

impl Agentry<'_> {
    /// What model's tokens u cost at the agent's prices, else the native
    /// agent's; 0 when neither can say.
    pub fn cost(&self, model: &str, u: &TokenUsage) -> f64 {
        if let Some(c) = self.reg.price(self.kind, model, u) {
            return c;
        }
        self.reg
            .native_agent()
            .and_then(|n| self.reg.price(n.kind().as_str(), model, u))
            .unwrap_or(0.0)
    }

    /// model's context window.
    pub fn window(&self, model: &str) -> i64 {
        self.reg.context_window(self.kind, model)
    }
}

/// Session key's agent, as the registry knows it.
pub fn agentry<'a>(m: &'a Model, key: &str) -> Agentry<'a> {
    let kind = m
        .agents
        .iter()
        .find(|a| a.key == key)
        .map_or("", |a| a.kind.as_str());
    Agentry { reg: &m.reg, kind }
}

/// When the main agent's cache goes cold, and whether it has (CacheCold).
pub fn cache_cold(s: &Session, now: Ms) -> Option<(Ms, bool)> {
    let r = s.requests.iter().rev().find(|r| r.run.is_empty())?;
    let at = r.at + CACHE_HOUR;
    Some((at, now > at))
}
