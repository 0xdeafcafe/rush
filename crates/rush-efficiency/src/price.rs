//! What the agent's tokens cost, at its own prices, through the adapters
//! rush knows (scan.go cost, aggregate.go cacheReadPrice).

use rush_core::adapter::global;
use rush_core::agent::TokenUsage;

use crate::AGENT;

/// What the agent's tokens u cost on model, in its fast mode or not; 0 for
/// a model it doesn't price.
pub fn cost(model: &str, u: &TokenUsage, fast: bool) -> f64 {
    let Some(a) = global().get(AGENT) else {
        return 0.0;
    };
    if fast && let Some(fp) = a.fast_pricer() {
        return fp.cost_fast(model, u).unwrap_or(0.0);
    }
    a.pricer().and_then(|p| p.cost(model, u)).unwrap_or(0.0)
}

/// Dollars per million tokens read from the cache, at the agent's own
/// prices: its default model's when model has none.
pub fn cache_read_price(model: &str) -> f64 {
    let u = TokenUsage {
        cache_read: 1_000_000,
        ..TokenUsage::default()
    };
    global().price(AGENT, model, &u).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cache reads are priced at the agent's own prices, and at its default
    // model's when the model isn't one it prices.
    #[test]
    fn cache_reads() {
        crate::testutil::claude();
        assert_eq!(cache_read_price("claude-opus-5-5"), 0.20);
        // An unknown model reads at the default model's prices, once
        // Claude Code's adapter says which that is (its DefaultModeler).
        let defaults = global()
            .get(AGENT)
            .is_some_and(|a| a.default_modeler().is_some());
        if defaults {
            assert_eq!(cache_read_price(""), cache_read_price("claude-opus-5-5"));
        }
        let h = cache_read_price("claude-haiku-4-5");
        assert!(h > 0.0 && h < 0.20, "Haiku reads at {h}");
    }
}
