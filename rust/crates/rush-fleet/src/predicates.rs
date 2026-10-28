//! What a row's state means: needs you, your turn, halted, stuck. Port
//! of internal/fleet/fleet.go:116-190 (NeedsYou, JustFinished, Busy, Age,
//! Elapsed), fleet/stall.go and fleet/stuck.go. Errors are sorted by
//! rush_core::failure, as Go's agent/failure.

use rush_core::agent::{Agent, State};
use rush_core::failure;
use rush_core::time::Ms;

const MINUTE: Ms = 60_000;
const HOUR: Ms = 60 * MINUTE;

/// How long until an agent at work that has written nothing shows as
/// stuck; the same for a subagent run, whose prompt cache lasts five
/// minutes, so a longer wait buys nothing.
pub const STUCK_AFTER: Ms = 30 * MINUTE;
pub const STUCK_SUB_AFTER: Ms = 5 * MINUTE;

/// What a row's state means. Seen, put-away (done) and rush-hosted
/// sessions aren't known here yet: rush's own state keeps them.
pub trait Predicates {
    /// A live agent asking something the user has not looked at yet.
    fn needs_you(&self) -> bool;
    /// A finished turn whose background work is still running in a live
    /// process; subagents still writing count too.
    fn busy(&self) -> bool;
    /// A turn that ended moments ago; it lingers in Working so a finish is
    /// noticed rather than vanishing into history.
    fn just_finished(&self, now: Ms) -> bool;
    /// Its last turn ended on an error: it sits there until told to go on.
    fn halted(&self) -> bool;
    /// A halt in the last day that trying again gets past.
    fn continues(&self, now: Ms) -> bool;
    /// A halt the network being down caused.
    fn offline(&self) -> bool;
    /// It finished its turn without asking anything: waiting for the next
    /// thing to do.
    fn your_turn(&self, now: Ms) -> bool;
    /// A halt said briefly, for a row.
    fn halt_reason(&self, now: Ms) -> String;
    /// What going on means for it: "continue" after an error, else "keep
    /// going".
    fn continue_text(&self) -> &'static str;
    /// At work and quiet for STUCK_AFTER.
    fn stuck(&self, now: Ms) -> bool;
    /// How long since it last wrote.
    fn quiet(&self, now: Ms) -> Ms;
    /// Time since its last change: what the list prints on the right.
    fn age(&self, now: Ms) -> Ms;
    /// How long it has existed, frozen once it stops.
    fn elapsed(&self, now: Ms) -> Ms;
}

impl Predicates for Agent {
    fn needs_you(&self) -> bool {
        self.job.state == State::Blocked && !self.checking && self.pid != 0
    }

    fn busy(&self) -> bool {
        self.pid != 0 && self.job.busy()
            || !self.job.live() && self.subs.direct + self.subs.nested > 0
    }

    fn just_finished(&self, now: Ms) -> bool {
        self.job.state == State::Done && !self.busy() && now - self.job.updated_at < 2 * MINUTE
    }

    fn halted(&self) -> bool {
        self.spend.halt.is_some()
            && !self.job.live()
            && !self.busy()
            && !self.checking
            && !self.past
            && !self.interactive
            && self.job.state != State::Stopped
    }

    fn continues(&self, now: Ms) -> bool {
        match &self.spend.halt {
            Some(h) if self.halted() && now - h.at < 24 * HOUR => {
                failure::kind_of(&h.text).retries()
            }
            _ => false,
        }
    }

    fn offline(&self) -> bool {
        self.spend
            .halt
            .as_ref()
            .is_some_and(|h| failure::kind_of(&h.text) == failure::Kind::Offline)
    }

    fn your_turn(&self, now: Ms) -> bool {
        self.job.state == State::Done
            && !self.checking
            && !self.busy()
            && !self.past
            && !self.interactive
            && self.spend.halt.is_none()
            && now - self.job.updated_at < 24 * HOUR
    }

    fn halt_reason(&self, now: Ms) -> String {
        let Some(h) = &self.spend.halt else {
            return String::new();
        };
        let t = h.text.strip_prefix("API Error: ").unwrap_or(&h.text);
        if t.is_empty() {
            return h.kind.replace('_', " ");
        }
        if h.kind == "rate_limit" {
            // "You've hit your session limit · resets 5am (Europe/London)"
            let t = t.strip_prefix("You've hit your ").unwrap_or(t);
            return match t.find(" (") {
                Some(i) if i > 0 => t[..i].to_string(),
                _ => t.to_string(),
            };
        }
        if self.continues(now) && self.offline() {
            return "offline · continues when the network is back".into();
        }
        if self.continues(now) {
            return format!("{t} · continues by itself");
        }
        t.to_string()
    }

    fn continue_text(&self) -> &'static str {
        if self.spend.halt.is_some() {
            "continue"
        } else {
            "keep going"
        }
    }

    fn stuck(&self, now: Ms) -> bool {
        if self.past || self.checking || !(self.job.state == State::Working || self.busy()) {
            return false;
        }
        self.quiet(now) >= STUCK_AFTER
    }

    fn quiet(&self, now: Ms) -> Ms {
        now - self.job.updated_at.max(self.job.mod_time)
    }

    fn age(&self, now: Ms) -> Ms {
        now - self.job.updated_at.max(self.job.mod_time)
    }

    fn elapsed(&self, now: Ms) -> Ms {
        let mut end = now;
        if !self.job.live() {
            end = self.job.updated_at;
            if self.spend.last != 0 && self.spend.last < end {
                end = self.spend.last;
            }
        }
        (end - self.job.created_at).max(0)
    }
}
