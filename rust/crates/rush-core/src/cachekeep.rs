//! Keeping an idle session's prompt cache warm (Rust-only; Go has none).
//! No terminal or platform code: the UI hands in what its hosts say of
//! each idle session and does what [`decide`] says.
//!
//! A provider keeps a session's prompt cached for a while after its last
//! request: Anthropic's for 5 minutes or an hour, as the agent asked. A
//! message after that pays to write the whole context again. A tiny
//! request that reads the whole cached context (a "bump", which an agent
//! does through `adapter::CacheKeeper`) starts the cache's life over for a
//! tenth of the context's price. So while you're likely to come back, a
//! few bumps cost less than the rewrite they save; past that, the cache is
//! let go. The rule and its numbers are in rust/DESIGN.md, "Keeping the
//! prompt cache warm".
//!
//! Compacting instead is never done here: it costs a turn whether you come
//! back or not, and it loses detail the agent then reads again.

use crate::time::Ms;

const MIN: Ms = 60_000;

/// How long before the cache would lapse a bump goes: time for the
/// request, and for a look or two to miss it.
pub const LEAD: Ms = MIN;

/// Two looks no closer than this for one session: a bump takes seconds,
/// and the host says the cache's new life only once it's back.
pub const RETRY: Ms = 2 * MIN;

/// What a cache read costs, as a share of the input price (Anthropic,
/// OpenAI's GPT-5 family, Gemini 3: a tenth).
pub const READ: f64 = 0.1;

/// The tokens' worth each bump costs beside its read of the context: its
/// question and the few words of its answer, at the output price (5× the
/// input), in input tokens.
pub const FIXED: f64 = 300.0;

/// What writing the cache costs, as a share of the input price, for a
/// cache that lives ttl: Anthropic's 1.25× for 5 minutes, 2× for an hour.
pub fn write(ttl: Ms) -> f64 {
    if ttl > 10 * MIN { 2.0 } else { 1.25 }
}

/// What a lapse adds to the next message, as a share of the context at
/// the input price: a write where a read would have done.
pub fn penalty(ttl: Ms) -> f64 {
    write(ttl) - READ
}

/// The most bumps one idle stretch may take: together they never cost
/// more than half the rewrite they're there to save (a 5-minute cache: 5;
/// an hour's: 9).
pub fn max_bumps(ttl: Ms) -> u32 {
    (penalty(ttl) / READ / 2.0).floor() as u32
}

/// How it's set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// How long after a session goes idle its cache is kept: past that it
    /// lapses. 15 minutes keeps a 5-minute cache for 3 bumps, and leaves
    /// an hour's cache alone.
    pub keep_for: Ms,
    /// No bump for a context smaller than this: its rewrite costs cents.
    pub min_context: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            keep_for: 15 * MIN,
            min_context: 20_000,
        }
    }
}

/// One idle session as its host says it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Seat {
    /// When its last turn ended.
    pub idle_since: Ms,
    /// When its cache lapses.
    pub warm_until: Ms,
    /// How long its cache lives after a request; 0 when not known.
    pub ttl: Ms,
    /// The tokens its last request sent.
    pub context: i64,
    /// Bumps it has had since it went idle.
    pub bumps: u32,
}

/// What to do with a session's cache now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Bump it now.
    Keep,
    /// Not yet: it isn't about to lapse.
    Wait,
    /// It lapses, and why.
    Let(&'static str),
}

/// Whether to bump session x's cache at now.
pub fn decide(s: &Settings, x: &Seat, now: Ms) -> Verdict {
    if x.ttl <= 2 * LEAD || x.warm_until == 0 || x.idle_since == 0 {
        return Verdict::Let("how long its cache lives isn't known");
    }
    if now >= x.warm_until {
        return Verdict::Let("its cache has lapsed already");
    }
    if x.warm_until - now > LEAD {
        return Verdict::Wait;
    }
    if x.context < s.min_context {
        return Verdict::Let("its context is too small to be worth it");
    }
    if now - x.idle_since >= s.keep_for {
        return Verdict::Let("idle longer than the cache is kept for");
    }
    if x.bumps >= max_bumps(x.ttl) {
        return Verdict::Let("its bumps would cost more than half the rewrite");
    }
    let c = x.context as f64;
    let bumps = f64::from(x.bumps + 1) * (READ * c + FIXED);
    if bumps >= penalty(x.ttl) * c {
        return Verdict::Let("its bumps would cost more than the rewrite");
    }
    Verdict::Keep
}

/// What one bump costs and what a lapse would add, in the context's input
/// tokens' worth: (bump, lapse).
pub fn worth(ttl: Ms, context: i64) -> (f64, f64) {
    let c = context as f64;
    (READ * c + FIXED, penalty(ttl) * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Ms = 1_000_000_000;

    fn seat(idle_for: Ms, left: Ms, ttl: Ms, context: i64, bumps: u32) -> Seat {
        Seat {
            idle_since: T0 - idle_for,
            warm_until: T0 + left,
            ttl,
            context,
            bumps,
        }
    }

    #[test]
    fn the_rule() {
        let s = Settings::default();
        let five = 5 * MIN;
        let hour = 60 * MIN;
        let cases: &[(&str, Seat, Verdict)] = &[
            (
                "about to lapse, just idle",
                seat(4 * MIN, 30_000, five, 100_000, 0),
                Verdict::Keep,
            ),
            (
                "not about to lapse",
                seat(MIN, 4 * MIN, five, 100_000, 0),
                Verdict::Wait,
            ),
            (
                "lapsed",
                seat(6 * MIN, -1, five, 100_000, 0),
                Verdict::Let("its cache has lapsed already"),
            ),
            (
                "idle past 15m",
                seat(15 * MIN, 30_000, five, 100_000, 3),
                Verdict::Let("idle longer than the cache is kept for"),
            ),
            (
                "third bump at 12m",
                seat(12 * MIN, 30_000, five, 100_000, 2),
                Verdict::Keep,
            ),
            (
                "small",
                seat(4 * MIN, 30_000, five, 19_999, 0),
                Verdict::Let("its context is too small to be worth it"),
            ),
            (
                "ttl unknown",
                seat(4 * MIN, 30_000, 0, 100_000, 0),
                Verdict::Let("how long its cache lives isn't known"),
            ),
            (
                "an hour's cache, idle 59m",
                seat(59 * MIN, 30_000, hour, 100_000, 0),
                Verdict::Let("idle longer than the cache is kept for"),
            ),
            (
                "too many bumps",
                seat(MIN, 30_000, five, 100_000, 5),
                Verdict::Let("its bumps would cost more than half the rewrite"),
            ),
        ];
        for (name, x, want) in cases {
            assert_eq!(&decide(&s, x, T0), want, "{name}");
        }
        // Kept 2 hours, an hour's cache gets its bump at 59 minutes.
        let long = Settings {
            keep_for: 120 * MIN,
            ..s
        };
        assert_eq!(
            decide(&long, &seat(59 * MIN, 30_000, hour, 100_000, 0), T0),
            Verdict::Keep
        );
    }

    #[test]
    fn the_numbers() {
        assert_eq!(max_bumps(5 * MIN), 5);
        assert_eq!(max_bumps(60 * MIN), 9);
        let (bump, lapse) = worth(5 * MIN, 100_000);
        assert_eq!((bump, lapse.round()), (10_300.0, 115_000.0));
        // A context so small the bump's own words outweigh it.
        let s = Settings {
            min_context: 0,
            ..Settings::default()
        };
        assert_eq!(
            decide(&s, &seat(MIN, 30_000, 5 * MIN, 200, 0), T0),
            Verdict::Let("its bumps would cost more than the rewrite")
        );
    }
}
