//! Keeping a working subagent's prompt cache from lapsing while it waits
//! on a long call (Rust-only; Go has none). No terminal or platform code:
//! the host hands in what it has followed of each subagent and does what
//! [`decide`] says, through the agent's `adapter::SubagentKeeper`.
//!
//! A subagent's cache lives 5 minutes (or an hour) after each of its
//! requests, like the main conversation's, but nothing outside the
//! subagent can make a request against its context: the side request the
//! main keep-warm sends (`cachekeep`) reads only the main conversation.
//! What can be done is to move the call it waits on to the background:
//! the call goes on, the subagent gets its turn back at once and makes
//! its next request while the cache is still warm, which starts the
//! cache's life over at a tenth of the context's price. It's told the
//! command still runs and can check on it. Unlike an idle session, a
//! subagent waiting on a call is sure to make another request when the
//! call returns, so there's no "will you come back" bet: the only cost
//! is that request, against the rewrite it saves, and the rule spends no
//! more than half that rewrite on one subagent.
//!
//! [`ahead`] is the same rule a step earlier: a command that took longer
//! than the cache lives the last time it ran in this project runs in the
//! background from the start. The numbers are in DESIGN.md, "Subagents'
//! cache".

use crate::cachekeep::{LEAD, READ, Settings, most_bumps, penalty};
use crate::time::Ms;

/// What one step-in costs beside the subagent's read of its context: its
/// call's new result written to the cache, and the few words it says
/// before it checks back, at the output price, in input tokens.
pub const FIXED: f64 = 1_000.0;

/// What a subagent is doing now, as its host has followed it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Doing {
    /// Waiting on a call of its own. movable: the agent can move it to
    /// the background (Claude Code: a shell command running for more
    /// than 2 seconds).
    Calling { call: String, movable: bool },
    /// Between requests, or writing one: its next request is its own.
    #[default]
    Working,
    /// Waiting on nothing of its own (a teammate waiting for a message).
    Idle,
    /// Done: its cache is no use to anyone now.
    Finished,
}

/// One subagent, as its host has followed it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sub {
    /// When its last request was; 0 before its first.
    pub last_request: Ms,
    /// How long its cache lives after a request; 0 when not known.
    pub ttl: Ms,
    /// The tokens its last request sent.
    pub context: i64,
    pub doing: Doing,
    /// Times rush has stepped in for it already.
    pub steps: u32,
}

/// What to do for a subagent's cache now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Move this call to the background now.
    Move(String),
    /// Not yet: it isn't about to lapse, or it's making its own request.
    Wait,
    /// It lapses, and why.
    Let(&'static str),
}

/// Whether to step in for subagent x's cache at now, as s says: its
/// min_context, the smallest context worth it (20k tokens), and its
/// max_bumps, the most step-ins (0: half a rewrite). Its other settings
/// are the idle sessions'.
pub fn decide(s: &Settings, x: &Sub, now: Ms) -> Step {
    if x.doing == Doing::Finished {
        return Step::Let("it has finished");
    }
    if x.ttl <= 2 * LEAD || x.last_request == 0 {
        return Step::Let("how long its cache lives isn't known");
    }
    let warm_until = x.last_request + x.ttl;
    if now >= warm_until {
        return Step::Let("its cache has lapsed already");
    }
    if warm_until - now > LEAD {
        return Step::Wait;
    }
    let call = match &x.doing {
        Doing::Working => return Step::Wait,
        Doing::Idle => return Step::Let("nothing can refresh an idle subagent's cache"),
        Doing::Calling { movable: false, .. } => {
            return Step::Let("its call can't be moved to the background");
        }
        Doing::Calling { call, .. } => call,
        Doing::Finished => unreachable!(),
    };
    if x.context < s.min_context {
        return Step::Let("its context is too small to be worth it");
    }
    if x.steps >= most_bumps(s.max_bumps, x.ttl) {
        return Step::Let("its step-ins would cost more than half the rewrite");
    }
    let (step, lapse) = worth(x.ttl, x.context);
    if f64::from(x.steps + 1) * step >= lapse {
        return Step::Let("its step-ins would cost more than the rewrite");
    }
    Step::Move(call.clone())
}

/// What one step-in costs and what a lapse would add, in the context's
/// input tokens' worth: (step, lapse).
pub fn worth(ttl: Ms, context: i64) -> (f64, f64) {
    let c = context as f64;
    (READ * c + FIXED, penalty(ttl) * c)
}

/// Whether a command that took `took` the last time it ran here should run
/// in the background from the start, for a subagent whose cache lives ttl:
/// it ran past the point [`decide`] would move it anyway.
pub fn ahead(took: Ms, ttl: Ms) -> bool {
    ttl > 2 * LEAD && took >= ttl - LEAD
}

/// How long a subagent whose cache lives ttl may leave a command in the
/// background before it checks back, for its next request to read the
/// cache: a minute short of its life.
pub fn check_back(ttl: Ms) -> Ms {
    (ttl - LEAD).max(LEAD)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Ms = 60_000;
    const T0: Ms = 1_000_000_000;
    const FIVE: Ms = 5 * MIN;
    const HOUR: Ms = 60 * MIN;

    fn calling(movable: bool) -> Doing {
        Doing::Calling {
            call: "tu1".into(),
            movable,
        }
    }

    fn sub(ago: Ms, ttl: Ms, context: i64, doing: Doing, steps: u32) -> Sub {
        Sub {
            last_request: T0 - ago,
            ttl,
            context,
            doing,
            steps,
        }
    }

    #[test]
    fn the_rule() {
        let mv = Step::Move("tu1".into());
        let cases: &[(&str, Sub, Step)] = &[
            (
                "a long call, its cache about to lapse",
                sub(4 * MIN + 30_000, FIVE, 100_000, calling(true), 0),
                mv.clone(),
            ),
            (
                "not about to lapse",
                sub(2 * MIN, FIVE, 100_000, calling(true), 0),
                Step::Wait,
            ),
            (
                "lapsed",
                sub(5 * MIN, FIVE, 100_000, calling(true), 0),
                Step::Let("its cache has lapsed already"),
            ),
            (
                "making its own request",
                sub(4 * MIN + 30_000, FIVE, 100_000, Doing::Working, 0),
                Step::Wait,
            ),
            (
                "idle",
                sub(4 * MIN + 30_000, FIVE, 100_000, Doing::Idle, 0),
                Step::Let("nothing can refresh an idle subagent's cache"),
            ),
            (
                "finished",
                sub(MIN, FIVE, 100_000, Doing::Finished, 0),
                Step::Let("it has finished"),
            ),
            (
                "a call that can't move",
                sub(4 * MIN + 30_000, FIVE, 100_000, calling(false), 0),
                Step::Let("its call can't be moved to the background"),
            ),
            (
                "small",
                sub(4 * MIN + 30_000, FIVE, 19_999, calling(true), 0),
                Step::Let("its context is too small to be worth it"),
            ),
            (
                "ttl unknown",
                sub(4 * MIN + 30_000, 0, 100_000, calling(true), 0),
                Step::Let("how long its cache lives isn't known"),
            ),
            (
                "the fifth step-in",
                sub(4 * MIN + 30_000, FIVE, 100_000, calling(true), 4),
                mv.clone(),
            ),
            (
                "a sixth",
                sub(4 * MIN + 30_000, FIVE, 100_000, calling(true), 5),
                Step::Let("its step-ins would cost more than half the rewrite"),
            ),
            (
                "an hour's cache, 59 minutes on",
                sub(59 * MIN, HOUR, 100_000, calling(true), 0),
                mv.clone(),
            ),
            (
                "an hour's cache, 30 minutes on",
                sub(30 * MIN, HOUR, 100_000, calling(true), 0),
                Step::Wait,
            ),
        ];
        for (name, x, want) in cases {
            assert_eq!(&decide(&Settings::default(), x, T0), want, "{name}");
        }
    }

    #[test]
    fn as_set() {
        let mv = Step::Move("tu1".into());
        // min_context, max_bumps, the subagent: the step.
        let cases: &[(i64, u32, Sub, Step)] = &[
            (
                10_000,
                0,
                sub(4 * MIN + 30_000, FIVE, 15_000, calling(true), 0),
                mv.clone(),
            ),
            (
                50_000,
                0,
                sub(4 * MIN + 30_000, FIVE, 40_000, calling(true), 0),
                Step::Let("its context is too small to be worth it"),
            ),
            (
                20_000,
                2,
                sub(4 * MIN + 30_000, FIVE, 100_000, calling(true), 2),
                Step::Let("its step-ins would cost more than half the rewrite"),
            ),
            (
                20_000,
                9,
                sub(4 * MIN + 30_000, FIVE, 100_000, calling(true), 5),
                mv.clone(),
            ),
        ];
        for (min, max, x, want) in cases {
            let s = Settings {
                min_context: *min,
                max_bumps: *max,
                ..Settings::default()
            };
            assert_eq!(&decide(&s, x, T0), want, "min {min}, max {max}");
        }
    }

    #[test]
    fn the_numbers() {
        // 100k tokens, 5-minute cache: a step-in costs 11k tokens' worth,
        // a lapse 115k.
        let (step, lapse) = worth(FIVE, 100_000);
        assert_eq!((step, lapse.round()), (11_000.0, 115_000.0));
        // A context so small the step-in's own words outweigh it.
        assert_eq!(
            decide(
                &Settings {
                    min_context: 0,
                    ..Settings::default()
                },
                &sub(4 * MIN + 30_000, FIVE, 800, calling(true), 0),
                T0
            ),
            Step::Let("its step-ins would cost more than the rewrite")
        );
    }

    #[test]
    fn ahead_of_time() {
        let cases: &[(Ms, Ms, bool)] = &[
            (4 * MIN, FIVE, true),
            (4 * MIN - 1, FIVE, false),
            (10 * MIN, FIVE, true),
            (10 * MIN, HOUR, false),
            (59 * MIN, HOUR, true),
            (10 * MIN, 0, false),
        ];
        for &(took, ttl, want) in cases {
            assert_eq!(ahead(took, ttl), want, "{took} {ttl}");
        }
        assert_eq!(check_back(FIVE), 4 * MIN);
        assert_eq!(check_back(HOUR), 59 * MIN);
    }
}
