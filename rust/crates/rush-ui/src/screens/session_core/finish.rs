//! What each reading of the list says happened to an agent since the one
//! before: it finished, it asks something, it started failing, its turn
//! stalled, each noted as a line from your agents (the feed's, kept to the
//! last 30). A turn that ends with agents it started still running in the
//! background hasn't finished: it's noted finished once they're done.
//! Clanker reacts to each: an error, an ask, one you kept waiting moving
//! on, one finishing; a stronger reaction takes over a weaker one already
//! running, never the other way. After Go's internal/ui/model.go:1946-1990
//! notify (with 4d29f271: Busy counts as working), clanker.go:429-552
//! (fxKind, clkFX, fxLen, react) and feed.go:21 noteEvent. Not yet: the
//! reactions drawn on clanker and ticked on (clkFX.draw, tint,
//! onFXTick: clanker.rs's), the system notification when an agent needs
//! you or stalls, and a usage limit rush moves on from by itself noted
//! quietly (limitHandled).

use std::collections::HashMap;

use rush_core::agent::{Agent, State};
use rush_core::time::Ms;
use rush_fleet::Predicates;

use crate::model::Model;
use crate::screens::session_cards::mention::mention_name;
use crate::screens::social::feed::AgentEvent;
use crate::style::one_line;

/// The feed keeps this many lines.
const KEPT: usize = 30;

/// Something clanker is reacting to, weakest first: a stronger one takes
/// over a weaker one already running, never the other way (fxKind).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fx {
    #[default]
    None,
    /// A light passes over him, now and then.
    Shimmer,
    /// An agent finished: a glint and a drop.
    Done,
    /// One you kept waiting moved on: he greens, confetti.
    Answered,
    /// One started waiting on you: streaks fly off him.
    Ask,
    /// One hit an error, or rush did: he reddens, sparks.
    Error,
}

impl Fx {
    /// How many frames of FX_EVERY it runs for (fxLen).
    pub fn frames(self) -> usize {
        match self {
            Fx::None => 0,
            Fx::Shimmer => 14, // clkShine
            Fx::Done => 14,
            Fx::Answered => 22,
            Fx::Ask => 18,
            Fx::Error => 16,
        }
    }
}

/// How long a frame of a reaction is (fxEvery).
pub const FX_EVERY: Ms = 70;

/// The reaction under way and how far into it he is, in frames (clkFX).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reaction {
    pub kind: Fx,
    pub frame: usize,
}

/// Starts a reaction, unless a stronger one is running (react).
pub fn react(r: &mut Reaction, k: Fx) {
    if k < r.kind {
        return;
    }
    *r = Reaction { kind: k, frame: 0 };
}

/// Moves the reaction on a frame; it ends when it's over (onFXTick).
/// Whether it still runs.
pub fn fx_tick(r: &mut Reaction) -> bool {
    r.frame += 1;
    if r.frame >= r.kind.frames() {
        *r = Reaction::default();
        return false;
    }
    true
}

/// What the last reading said of each agent.
#[derive(Default)]
pub struct Finish {
    /// Each agent's state as last read, by key; busy reads as working
    /// (lastState).
    pub last_state: HashMap<String, &'static str>,
    last_err: HashMap<String, bool>,
    last_stall: HashMap<String, Ms>,
    /// The lines from your agents, oldest first (m.events).
    pub events: Vec<AgentEvent>,
    /// What clanker is reacting to (m.fx).
    pub fx: Reaction,
}

/// An agent's @handle (agentHandle).
fn handle(a: &Agent) -> String {
    let host = a.rush.then_some(a.job.id.as_str());
    format!("@{}", mention_name(a, host))
}

fn note(f: &mut Finish, a: &Agent, now: Ms, glyph: &str, what: String) {
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

/// With each reading of the list (notify).
pub fn on_fleet(m: &mut Model) {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let mut f = std::mem::take(m.kept::<Finish>());
    let first = f.last_state.is_empty();
    for a in m.agents.iter().filter(|a| !a.checking) {
        let prev = f.last_state.get(&a.key).copied().unwrap_or("");
        // Agents it left running in the background are work: it finishes
        // when they do, not when its turn ends.
        let st = if a.busy() {
            "working"
        } else {
            a.job.state.as_str()
        };
        f.last_state.insert(a.key.clone(), st);
        let detail = a.job.detail.as_str();
        let failing =
            detail.starts_with("API error") || detail.starts_with("usage limit") || a.halted();
        let was_failing = f.last_err.insert(a.key.clone(), failing).unwrap_or(false);
        if !first && !prev.is_empty() {
            if failing && !was_failing {
                react(&mut f.fx, Fx::Error);
                note(&mut f, a, now, "✗", one_line(&a.halt_reason(now)));
            } else if prev != "blocked" && a.needs_you() {
                react(&mut f.fx, Fx::Ask);
                note(&mut f, a, now, "?", one_line(&a.job.needs));
            } else if prev == "blocked" && st == State::Working.as_str() {
                react(&mut f.fx, Fx::Answered);
            } else if prev == "working" && st == State::Done.as_str() {
                react(&mut f.fx, Fx::Done);
                note(&mut f, a, now, "✓", "finished".into());
            }
        }
        // A stalled turn waits for good: said once a stall.
        let was = f.last_stall.insert(a.key.clone(), a.stalled).unwrap_or(0);
        if a.stalled != 0 && a.stalled != was && !first && !prev.is_empty() {
            note(
                &mut f,
                a,
                now,
                "⚠",
                "stalled · nothing from the model, no step running".into(),
            );
        }
    }
    *m.kept::<Finish>() = f;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(m: &mut Model) -> usize {
        m.kept::<Finish>()
            .events
            .iter()
            .filter(|e| e.what == "finished")
            .count()
    }

    fn read(m: &mut Model, a: &Agent) {
        m.agents = vec![a.clone()];
        on_fleet(m);
    }

    // finishnote_test.go: a turn that ends with agents still running in
    // the background hasn't finished; it's noted finished once they're
    // done, not when the turn ends.
    #[test]
    fn finished_waits_for_background_work() {
        let mut m = Model::new("test");
        m.clock = Some(1_790_000_000_000);
        let mut a = Agent {
            key: "k".into(),
            display_name: "porter".into(),
            pid: std::process::id() as i32,
            ..Agent::default()
        };
        a.job.state = State::Working;
        read(&mut m, &a); // the first read only learns what's there
        read(&mut m, &a);
        a.job.state = State::Done;
        a.job.in_flight = 1;
        a.job.background = vec!["task\0port the next round".into()];
        read(&mut m, &a);
        assert_eq!(
            finished(&mut m),
            0,
            "noted finished while its workflow runs"
        );
        a.job.in_flight = 0;
        a.job.background.clear();
        read(&mut m, &a);
        assert_eq!(finished(&mut m), 1, "noted once its workflow ended");
        assert_eq!(m.kept::<Finish>().fx.kind, Fx::Done, "and clanker reacted");
    }

    fn fx(m: &mut Model) -> Fx {
        m.kept::<Finish>().fx.kind
    }

    /// Clanker reacts to an ask, to one you kept waiting moving on (busy
    /// with what it left running counts as working), and to one finishing
    /// (model.go:1975-1986).
    #[test]
    fn clanker_reacts() {
        let mut m = Model::new("test");
        m.clock = Some(1_790_000_000_000);
        let mut a = Agent {
            key: "k".into(),
            display_name: "porter".into(),
            pid: std::process::id() as i32,
            ..Agent::default()
        };
        a.job.state = State::Working;
        read(&mut m, &a);
        read(&mut m, &a);
        assert_eq!(fx(&mut m), Fx::None, "nothing happened");
        a.job.state = State::Blocked;
        a.job.needs = "allow Bash?".into();
        read(&mut m, &a);
        assert_eq!(fx(&mut m), Fx::Ask);
        // Answered: a weaker reaction doesn't take over a stronger one
        // running, so let the ask run out first.
        while fx_tick(&mut m.kept::<Finish>().fx) {}
        a.job.state = State::Done;
        a.job.needs.clear();
        a.job.in_flight = 1; // busy: it reads as working
        a.job.background = vec!["task\0port the next round".into()];
        read(&mut m, &a);
        assert_eq!(fx(&mut m), Fx::Answered);
        a.job.in_flight = 0;
        a.job.background.clear();
        read(&mut m, &a);
        assert_eq!(fx(&mut m), Fx::Answered, "done is weaker than answered");
    }

    /// A stronger reaction takes over a weaker one, never the other way;
    /// each runs its frames and ends (react, onFXTick).
    #[test]
    fn reactions_by_strength() {
        let mut r = Reaction::default();
        react(&mut r, Fx::Done);
        react(&mut r, Fx::Error);
        assert_eq!(r.kind, Fx::Error);
        react(&mut r, Fx::Ask);
        assert_eq!(r.kind, Fx::Error, "an ask doesn't take over an error");
        let mut n = 1;
        while fx_tick(&mut r) {
            n += 1;
        }
        assert_eq!(n, Fx::Error.frames());
        assert_eq!(r, Reaction::default());
        react(&mut r, Fx::Ask);
        assert_eq!(r.kind, Fx::Ask, "anything once it's over");
    }
}
