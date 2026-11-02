//! What the prompt cache lapsing cost the subagents of a session
//! (Rust-only; Go has none). No terminal or platform code: an agent
//! hands in each request's usage, in order, one [`Tracker`] per
//! subagent, with a way to price tokens; this says how often the cache
//! had lapsed between two requests and what that cost, and what a
//! 1-hour cache would have cost instead.
//!
//! A subagent's cache lives 5 minutes after each of its requests unless
//! the agent is told otherwise (Claude Code: Settings › Claude Code ›
//! Subagents' cache). A subagent that waits on a long command past that
//! pays to write its whole context again where a read would have done.
//! rush/DESIGN.md, "Keeping the prompt cache warm", has the numbers.

use serde::{Deserialize, Serialize};

use crate::agent::TokenUsage;
use crate::time::Ms;

/// A short cache's life after each request.
pub const SHORT: Ms = 5 * 60_000;
/// The long cache's.
pub const LONG: Ms = 60 * 60_000;

/// Below this many tokens a rewrite is cents: not counted as a lapse.
const MIN_LAPSED: i64 = 1_000;

/// Prices model's tokens in dollars; 0 when it can't say.
pub type Price<'a> = &'a dyn Fn(&str, &TokenUsage) -> f64;

/// What lapses cost, added up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubCache {
    /// Requests that wrote the context again after the cache had lapsed.
    pub lapses: u32,
    /// What those rewrites cost beyond what reading would have.
    pub cost: f64,
    /// The part of `cost` a 1-hour cache would have avoided: lapses after
    /// less than an hour.
    pub avoidable: f64,
    /// What a 1-hour cache would have cost: the dearer write on every
    /// token written, but for the rewrites it would have avoided.
    pub hour: f64,
}

impl SubCache {
    pub fn add(&mut self, o: &SubCache) {
        self.lapses += o.lapses;
        self.cost += o.cost;
        self.avoidable += o.avoidable;
        self.hour += o.hour;
    }

    /// Nothing to say: no lapse, and no cache written.
    pub fn is_empty(&self) -> bool {
        self.lapses == 0 && self.hour == 0.0
    }

    /// What a 1-hour cache would have saved: the lapses it avoids less
    /// what it costs. 0 when it wouldn't have paid.
    pub fn hour_saves(&self) -> f64 {
        (self.avoidable - self.hour).max(0.0)
    }
}

/// One subagent's requests, followed in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tracker {
    /// When its last request was, and how many tokens it sent.
    at: Ms,
    size: i64,
    /// How long its cache lives: 0 until a request writes one.
    ttl: Ms,
    pub sum: SubCache,
}

impl Tracker {
    /// Takes the subagent's next request, made at `at`.
    pub fn request(&mut self, at: Ms, model: &str, u: &TokenUsage, price: Price) {
        let (w5, w1) = (u.cache_write_5m, u.cache_write_1h);
        let wrote = w5 + w1;
        let mut lapsed = 0;
        let mut gap = 0;
        if self.at != 0 && at != 0 {
            gap = at - self.at;
            let ttl = if self.ttl == 0 { SHORT } else { self.ttl };
            if gap > ttl && wrote > 0 {
                lapsed = (self.size - u.cache_read).clamp(0, wrote);
            }
        }
        let mut avoided5 = 0;
        if lapsed >= MIN_LAPSED {
            let l1 = (lapsed as f64 * w1 as f64 / wrote as f64).round() as i64;
            let l5 = lapsed - l1;
            let wrote_again = TokenUsage {
                cache_write_5m: l5,
                cache_write_1h: l1,
                ..TokenUsage::default()
            };
            let read = TokenUsage {
                cache_read: lapsed,
                ..TokenUsage::default()
            };
            let extra = (price(model, &wrote_again) - price(model, &read)).max(0.0);
            self.sum.lapses += 1;
            self.sum.cost += extra;
            if gap <= LONG {
                self.sum.avoidable += extra;
                avoided5 = l5;
            }
        }
        let p5 = (w5 - avoided5).max(0);
        if p5 > 0 {
            let at_5m = TokenUsage {
                cache_write_5m: p5,
                ..TokenUsage::default()
            };
            let at_1h = TokenUsage {
                cache_write_1h: p5,
                ..TokenUsage::default()
            };
            self.sum.hour += (price(model, &at_1h) - price(model, &at_5m)).max(0.0);
        }
        if w5 > 0 {
            self.ttl = SHORT; // the newest part of the context lapses first
        } else if w1 > 0 {
            self.ttl = LONG;
        }
        self.at = at;
        self.size = u.input + u.cache_read + wrote;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// $10 a million for input, a write 1.25× or 2×, a read 0.1×.
    fn price(_: &str, u: &TokenUsage) -> f64 {
        (u.input as f64 * 10.0
            + u.cache_read as f64 * 1.0
            + u.cache_write_5m as f64 * 12.5
            + u.cache_write_1h as f64 * 20.0)
            / 1e6
    }

    fn req(read: i64, w5: i64) -> TokenUsage {
        TokenUsage {
            cache_read: read,
            cache_write_5m: w5,
            ..TokenUsage::default()
        }
    }

    #[test]
    fn a_seven_minute_gap_is_a_lapse_and_four_is_not() {
        let mut t = Tracker::default();
        t.request(60_000, "m", &req(0, 100_000), &price);
        t.request(120_000, "m", &req(100_000, 1_000), &price); // warm
        t.request(120_000 + 7 * 60_000, "m", &req(0, 101_000), &price); // lapsed
        t.request(120_000 + 11 * 60_000, "m", &req(101_000, 500), &price); // 4 minutes
        assert_eq!(t.sum.lapses, 1);
        // 101k tokens written at $12.50 where a read is $1.
        assert!((t.sum.cost - 101_000.0 * 11.5 / 1e6).abs() < 1e-9);
        assert!((t.sum.avoidable - t.sum.cost).abs() < 1e-9);
        // The dearer write ($7.50 more a million) on what a 1-hour cache
        // would have written but for the rewrite.
        let written = 100_000.0 + 1_000.0 + 500.0;
        assert!((t.sum.hour - written * 7.5 / 1e6).abs() < 1e-9);
        assert!(t.sum.hour_saves() > 0.0);
    }

    #[test]
    fn past_the_hour_a_long_cache_lapses_too() {
        let mut t = Tracker::default();
        t.request(60_000, "m", &req(0, 50_000), &price);
        t.request(60_000 + 2 * LONG, "m", &req(0, 50_000), &price);
        assert_eq!(t.sum.lapses, 1);
        assert_eq!(t.sum.avoidable, 0.0);
        assert_eq!(t.sum.hour_saves(), 0.0);
    }

    #[test]
    fn a_long_cache_lives_an_hour() {
        let mut t = Tracker::default();
        let long = TokenUsage {
            cache_write_1h: 50_000,
            ..TokenUsage::default()
        };
        t.request(60_000, "m", &long, &price);
        t.request(60_000 + 20 * 60_000, "m", &req(50_000, 300), &price);
        assert_eq!(t.sum.lapses, 0);
    }
}
