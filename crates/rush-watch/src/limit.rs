//! Waiting out GitHub's rate limit instead of failing on it. New in the
//! port (Go's watch ended with exit 3 on anything but a spent limit it
//! had already seen the headers of).
//!
//! What GitHub says is classified here ([`classify`]: REST's 403/429 with
//! X-RateLimit-Remaining: 0, Retry-After, a secondary limit, GraphQL's
//! "API rate limit already exceeded" that gh prints with no HTTP status),
//! when it resets is worked out from the answer or `gh api rate_limit`
//! ([`reset_from_rate_limit`]), and when nothing says, a watch backs off
//! ([`backoff`]: 1m, 2m, 4m, then 10m). [`Patience`] is what the commands
//! that run before a watch (resolving a branch or a PR) wait with. The
//! clock and GitHub's answers are all passed in: nothing here sleeps by
//! itself.

use std::collections::HashMap;
use std::time::Duration;

use rush_state::Time;

use crate::gh::Error;
use crate::go::round;

/// The backoff after the first answer that says nothing of when it ends.
pub const BACKOFF_FIRST: Duration = Duration::from_secs(60);
/// The longest the backoff gets.
pub const BACKOFF_MAX: Duration = Duration::from_secs(600);
/// The longest a reset GitHub names is waited for.
pub const WAIT_MAX: Duration = Duration::from_secs(3600);
/// How soon a watch looks again once a reset it waited for has passed.
pub const AFTER_RESET: Duration = Duration::from_secs(5);

/// How long to back off after the nth answer (0 the first) that said it's
/// limited and not when it ends: 1m, 2m, 4m, 8m, then 10m.
pub fn backoff(strikes: u32) -> Duration {
    BACKOFF_FIRST
        .saturating_mul(1u32 << strikes.min(8))
        .min(BACKOFF_MAX)
}

/// An answer that says GitHub's rate limit is spent, and when it resets
/// if the answer said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub reset: Option<Time>,
    /// It's GraphQL's limit (its own, apart from REST's).
    pub graphql: bool,
}

/// Whether an answer to a gh call is a rate limit: its HTTP status (0 when
/// gh printed none), headers (lowercased), body and what gh said on stderr.
/// 200 and 304 never are.
pub fn classify(
    status: u16,
    h: &HashMap<String, String>,
    body: &[u8],
    stderr: &[u8],
    now: Time,
) -> Option<Limit> {
    if !matches!(status, 0 | 403 | 429) {
        return None;
    }
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(body),
        String::from_utf8_lossy(stderr)
    )
    .to_lowercase();
    let says = text.contains("rate limit")
        || text.contains("rate_limited")
        || text.contains("abuse detection");
    let spent = h.get("x-ratelimit-remaining").map(|v| v.trim()) == Some("0");
    let limited = match status {
        429 => true,
        403 => spent || h.contains_key("retry-after") || says,
        _ => says || spent || h.contains_key("retry-after"),
    };
    limited.then(|| Limit {
        reset: reset_from_headers(h, now),
        graphql: text.contains("graphql"),
    })
}

/// When the headers say it resets: Retry-After's seconds, else
/// X-RateLimit-Reset, when that's still ahead. Never more than an hour.
pub fn reset_from_headers(h: &HashMap<String, String>, now: Time) -> Option<Time> {
    let most = now + WAIT_MAX;
    if let Some(s) = h
        .get("retry-after")
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        return Some((now + Duration::from_secs(s.max(1))).min(most));
    }
    let t = Time::from_unix(h.get("x-ratelimit-reset")?.trim().parse().ok()?, 0);
    (t > now).then(|| t.min(most))
}

/// When what `gh api rate_limit` said is spent resets: the exhausted
/// resource GitHub limited (graphql's when it was that), else the last of
/// those exhausted. None when none is spent, or it can't be read.
pub fn reset_from_rate_limit(body: &[u8], graphql: bool, now: Time) -> Option<Time> {
    let v: serde_json::Value = rush_state::json::from_slice(body).ok()?;
    let spent = |r: &serde_json::Value| {
        let left = r.get("remaining")?.as_i64()?;
        let at = Time::from_unix(r.get("reset")?.as_i64()?, 0);
        (left == 0 && at > now).then_some(at)
    };
    let res = v.get("resources")?;
    if graphql && let Some(t) = res.get("graphql").and_then(spent) {
        return Some(t.min(now + WAIT_MAX));
    }
    res.as_object()?
        .values()
        .filter_map(spent)
        .max()
        .map(|t| t.min(now + WAIT_MAX))
}

/// How long to wait: until the reset when one is known (a few seconds at
/// least, as the reset may have just passed), else the backoff for the
/// nth time in a row.
pub fn wait_for(known: Option<Time>, now: Time, strikes: u32) -> Duration {
    match known {
        Some(u) => since(u, now).max(AFTER_RESET),
        None => backoff(strikes),
    }
}

fn since(t: Time, o: Time) -> Duration {
    Duration::from_nanos(t.sub_nanos(o).max(0).min(u64::MAX as i128) as u64)
}

/// "14:32" in the zone t is written in.
fn clock(t: Time) -> String {
    t.format()[11..16].to_string()
}

/// The one line a watch says when it starts waiting. until is in the zone
/// to show it in (Time::local).
pub fn waiting_line(until: Time, now: Time) -> String {
    format!(
        "GitHub's rate limit is spent; waiting until {} (resets in {})",
        clock(until),
        round(since(until, now))
    )
}

/// What a watch says when the reset is after its timeout: it won't wait
/// for it.
pub fn timed_out_line(until: Time, now: Time, timeout: Duration) -> String {
    format!(
        "GitHub's rate limit is spent until {} (resets in {}), past the {} timeout; run it again after",
        clock(until),
        round(since(until, now)),
        round(timeout)
    )
}

/// Why waiting on the limit ended before it passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waited {
    /// Not a rate limit, or one that wasn't waited for.
    Err(Error),
    /// The reset is after the timeout; its line.
    TimedOut(String),
    /// A signal stopped the wait.
    Stopped,
}

/// How what runs before a watch (resolving a branch or a pull request)
/// waits out the rate limit.
pub struct Patience<'a> {
    /// Wait at all; false gives the limit back as an error.
    pub wait: bool,
    pub start: Time,
    /// Zero: never.
    pub timeout: Duration,
    pub say: Box<dyn Fn(&str) + 'a>,
    pub sleep: Box<dyn Fn(Duration) -> bool + 'a>,
    pub now: Box<dyn Fn() -> Time + 'a>,
    /// When GitHub's limit resets, if it is spent.
    pub limited: Box<dyn Fn() -> Option<Time> + 'a>,
}

impl Patience<'_> {
    /// No waiting: a limit is an error.
    pub fn none() -> Patience<'static> {
        Patience {
            wait: false,
            start: Time::default(),
            timeout: Duration::ZERO,
            say: Box::new(|_| {}),
            sleep: Box::new(|_| false),
            now: Box::new(Time::now),
            limited: Box::new(crate::gh::limited_until),
        }
    }

    /// Runs f until it isn't a rate limit, waiting out each as a watch
    /// does.
    pub fn call<T>(&self, mut f: impl FnMut() -> Result<T, Error>) -> Result<T, Waited> {
        let mut strikes = 0;
        let mut said: Option<Time> = None;
        loop {
            match f() {
                Err(Error::Limited) if self.wait => {}
                Err(e) => return Err(Waited::Err(e)),
                Ok(v) => return Ok(v),
            }
            let now = (self.now)();
            let wait = wait_for((self.limited)(), now, strikes);
            strikes += 1;
            let resume = now + wait;
            if !self.timeout.is_zero() && resume > self.start + self.timeout {
                return Err(Waited::TimedOut(timed_out_line(
                    resume.local(),
                    now,
                    self.timeout,
                )));
            }
            if said.is_none_or(|s| since(resume, s) > Duration::from_secs(30)) {
                said = Some(resume);
                (self.say)(&waiting_line(resume.local(), now));
            }
            if !(self.sleep)(wait) {
                return Err(Waited::Stopped);
            }
        }
    }
}
