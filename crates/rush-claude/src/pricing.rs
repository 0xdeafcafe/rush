//! List prices of Claude's models, and what tokens cost. Port of
//! internal/adapters/claude/claude/pricing.go.

use rush_core::agent::TokenUsage;

/// Dollars per million tokens. A cache_read of zero means 0.1x input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

const fn p(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_read,
    }
}

/// List prices, first-party API. Longest prefix wins.
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-1", p(10.0, 50.0, 0.25)),
    ("claude-mythos-5-1", p(10.0, 50.0, 0.25)),
    ("claude-fable-5", p(10.0, 50.0, 0.0)),
    ("claude-mythos-5", p(10.0, 50.0, 0.0)),
    ("claude-opus-5-5", p(4.0, 20.0, 0.20)),
    ("claude-opus-5", p(5.0, 25.0, 0.0)),
    ("claude-opus-4-1", p(15.0, 75.0, 0.0)),
    ("claude-opus-4-0", p(15.0, 75.0, 0.0)),
    ("claude-opus-4-2025", p(15.0, 75.0, 0.0)),
    ("claude-opus-4", p(5.0, 25.0, 0.0)),
    ("claude-sonnet-5", p(2.0, 10.0, 0.0)),
    ("claude-sonnet-4", p(3.0, 15.0, 0.0)),
    ("claude-3-7-sonnet", p(3.0, 15.0, 0.0)),
    ("claude-haiku-4", p(1.0, 5.0, 0.0)),
    ("claude-3-5-haiku", p(0.8, 4.0, 0.0)),
];

/// The models Claude Code's --model aliases run now; by hand, moved when
/// Claude Code's aliases move.
fn alias(m: &str) -> Option<&'static str> {
    Some(match m {
        "opus" => "claude-opus-5-5",
        "sonnet" => "claude-sonnet-5-5",
        "haiku" => "claude-haiku-4-5",
        "fable" => "claude-fable-5-1",
        _ => return None,
    })
}

/// The model's price, when it's one rush knows.
pub fn price_for(model: &str) -> Option<Price> {
    let model = alias(model.strip_suffix("[1m]").unwrap_or(model)).unwrap_or(model);
    PRICES
        .iter()
        .filter(|(pre, _)| model.starts_with(pre))
        .max_by_key(|(pre, _)| pre.len())
        .map(|&(_, p)| p)
}

/// Dollars at list prices; fast mode is billed at twice the standard rate.
pub fn cost(model: &str, u: TokenUsage, fast: bool) -> f64 {
    let Some(p) = price_for(model) else {
        return 0.0;
    };
    let read = if p.cache_read == 0.0 {
        p.input * 0.1
    } else {
        p.cache_read
    };
    let mut c = u.input as f64 * p.input
        + u.output as f64 * p.output
        + u.cache_read as f64 * read
        + u.cache_write_5m as f64 * p.input * 1.25
        + u.cache_write_1h as f64 * p.input * 2.0;
    if fast {
        c *= 2.0;
    }
    c / 1e6
}

/// The model's window; everything current but Haiku has 1M.
pub fn context_window(model: &str) -> i64 {
    if model.contains("haiku") {
        200_000
    } else {
        1_000_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing() {
        assert_eq!(price_for("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(price_for("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(price_for("claude-haiku-4-5-20251001").unwrap().input, 1.0);
        assert_eq!(price_for("opus[1m]").unwrap().input, 4.0);
        assert!(price_for("gpt-5").is_none(), "unknown model priced");
        let u = TokenUsage {
            cache_read: 1_000_000,
            ..Default::default()
        };
        assert!(
            (cost("claude-sonnet-5", u, false) - 0.2).abs() < 1e-12,
            "default cache read is 0.1x input"
        );
        let u = TokenUsage {
            output: 1_000_000,
            ..Default::default()
        };
        assert_eq!(cost("claude-opus-5-5", u, true), 40.0, "fast mode doubles");
    }
}
