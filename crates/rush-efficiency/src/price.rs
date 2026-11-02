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

/// What a subagent's bucket of tokens, which ran on model, saved over
/// running on the agent's default model (the strong one): the difference of
/// their prices for the same tokens, an estimate since the strong model
/// may have taken fewer or more turns. 0 when model isn't a cheaper one
/// than the default's, or either has no price. Rust-only.
pub fn saved_against_default(model: &str, b: &crate::scan::Bucket) -> f64 {
    let u = TokenUsage {
        input: b.input,
        output: b.out,
        cache_read: b.cr,
        cache_write_5m: b.cw,
        reasoning: b.think,
        ..TokenUsage::default()
    };
    let Some(own) = global()
        .get(AGENT)
        .and_then(|a| a.pricer()?.cost(model, &u))
    else {
        return 0.0;
    };
    let Some(strong) = global().price(AGENT, "", &u) else {
        return 0.0;
    };
    (strong - own).max(0.0)
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

    // Work that ran on Sonnet or Haiku saved the difference to the default
    // model's price for the same tokens; work on the default saved none.
    #[test]
    fn cheaper_models_save_against_the_default() {
        crate::testutil::claude();
        let b = crate::scan::Bucket {
            input: 1_000_000,
            out: 1_000_000,
            ..Default::default()
        };
        let strong = global().price(
            AGENT,
            "",
            &TokenUsage {
                input: 1_000_000,
                output: 1_000_000,
                ..TokenUsage::default()
            },
        );
        let Some(strong) = strong else { return };
        let sonnet = saved_against_default("claude-sonnet-5", &b);
        let haiku = saved_against_default("claude-haiku-4-5", &b);
        assert!(sonnet > 0.0 && sonnet < strong, "Sonnet saved {sonnet}");
        assert!(haiku > sonnet, "Haiku saved {haiku}, Sonnet {sonnet}");
        assert_eq!(saved_against_default("claude-opus-5-5", &b), 0.0);
        assert_eq!(saved_against_default("no-such-model", &b), 0.0);
    }

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
