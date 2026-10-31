//! What each reading of the list says happened to an agent since the one
//! before: it finished, it asks something, it started failing, its turn
//! stalled, each noted as a line from your agents (the feed's, kept to the
//! last 30). A turn that ends with agents it started still running in the
//! background hasn't finished: it's noted finished once they're done. A
//! usage limit rush moves a session on from by itself is only noted,
//! quietly. Clanker reacts to each: an error, an ask, one you kept waiting
//! moving on, one finishing; a stronger reaction takes over a weaker one
//! already running, never the other way. One that starts waiting on you,
//! stalls or stops on an error is a system notification too, unless
//! you're looking at it, rush is quiet or it follows another; the menu
//! bar posts the first kind itself when it runs. After Go's
//! internal/ui/model.go:1946-2033 (notify, with 4d29f271: Busy counts as
//! working; notifyLater), profiles.go:82 (limitHandled), clanker.go:429-552
//! (fxKind, clkFX, fxLen, react) and feed.go:21 noteEvent. clanker.rs
//! draws the reactions and ticks them on (clkFX.draw, tint, onFXTick).

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
    /// The decisions each agent was last notified of, by key: the same
    /// asked again isn't said again (toldCard).
    told_card: HashMap<String, String>,
    /// The notifications posted, as (title, body), in tests.
    #[cfg(test)]
    pub told: Vec<(String, String)>,
}

/// An agent's @handle (agentHandle).
fn handle(a: &Agent) -> String {
    let host = a.rush.then_some(a.job.id.as_str());
    format!("@{}", mention_name(a, host))
}

/// Adds a line from your agents for agent a, keeping the last 30
/// (noteEvent).
pub fn note_event(m: &mut Model, a: &Agent, glyph: &str, what: &str) {
    let now = m.now;
    note(m.kept::<Finish>(), a, now, glyph, what.to_string());
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

/// A rush session a usage limit stopped that rush moves on by itself: its
/// profile doesn't wait, and a login, account or provider has room for
/// it. Nobody needs telling it stopped (limitHandled).
pub fn limit_handled(m: &Model, a: &Agent) -> bool {
    use crate::screens::accounts::{autoswitch, keys, rows, state};
    use rush_state::profiles::{LIMIT_HANDOFF, LIMIT_WAIT};
    let rate = a
        .spend
        .halt
        .as_ref()
        .is_some_and(|h| h.kind == "rate_limit");
    if !a.rush || !a.job.detail.starts_with("usage limit") && !rate {
        return false;
    }
    let p = state::cfg(m).profile_for(&a.job.cwd, &a.profile);
    if p.limit() == LIMIT_WAIT {
        return false;
    }
    let logins = &state::peek(m).logins;
    if a.kind.as_str() == rush_state::LOGINS_KIND && logins.len() > 1 {
        let now = m.clock.unwrap_or_else(rush_core::time::now);
        if autoswitch::next_login(logins, true, now).is_some() || keys::has_room(m) {
            return true;
        }
    }
    let room = rows::room(m);
    if p.pick_for(a.kind.as_str(), &room).1 {
        return true;
    }
    p.next(a.kind.as_str(), &room).is_some() && p.limit() == LIMIT_HANDOFF
}

/// Posts a notification off the UI thread: posting runs a program, and
/// whether the menu bar runs is a look at the disk. With unless_menu_bar,
/// the menu bar posts it instead when it runs (notifyLater).
fn notify_later(f: &mut Finish, title: String, body: String, unless_menu_bar: bool) {
    #[cfg(test)]
    {
        let _ = unless_menu_bar;
        f.told.push((title, body));
    }
    #[cfg(not(test))]
    {
        let _ = f;
        std::thread::spawn(move || {
            let by = if unless_menu_bar && rush_menubar::running() {
                "menu bar"
            } else {
                rush_sys::actions::notify(&title, &body);
                "rush"
            };
            rush_sys::events::log(
                "notify",
                serde_json::json!({
                    "title": title, "body": rush_host::events::clip(&body), "by": by,
                }),
            );
        });
    }
}

/// With each reading of the list (notify).
pub fn on_fleet(m: &mut Model) {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let mut f = std::mem::take(m.kept::<Finish>());
    let first = f.last_state.is_empty();
    // Not for the agent you're looking at while rush has focus.
    let looking = if m.term_focused {
        super::open_key(m).unwrap_or_default()
    } else {
        String::new()
    };
    let quiet = crate::screens::accounts::state::cfg(m).quiet || m.following();
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
        // A limit rush moves it on from is only noted, quietly, in the feed.
        let handled = failing && !was_failing && limit_handled(m, a);
        if !first && !prev.is_empty() {
            if handled {
                note(&mut f, a, now, "↻", "usage limit · rush moves it on".into());
            } else if failing && !was_failing {
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
        let watching = a.key == looking;
        // The menu bar icon, when it runs, notifies instead, with buttons.
        if !first && !quiet && !prev.is_empty() && prev != "blocked" && a.needs_you() && !watching {
            let body = if a.job.needs.is_empty() {
                one_line(&a.job.detail)
            } else {
                a.job.needs.clone()
            };
            // The same decisions asked again, by a turn that came between
            // (news of a pull request it follows, say), were said already.
            let k = &a.decisions_key;
            if k.is_empty() || f.told_card.get(&a.key) != Some(k) {
                if !k.is_empty() {
                    f.told_card.insert(a.key.clone(), k.clone());
                }
                notify_later(&mut f, format!("{} needs you", a.display_name), body, true);
            }
        }
        // A stalled turn waits for good: rush never ends it, so it says
        // so, once a stall.
        let was = f.last_stall.insert(a.key.clone(), a.stalled).unwrap_or(0);
        if a.stalled != 0 && a.stalled != was && !first && !prev.is_empty() {
            note(
                &mut f,
                a,
                now,
                "⚠",
                "stalled · nothing from the model, no step running".into(),
            );
            if !quiet && !watching {
                notify_later(
                    &mut f,
                    format!("{} stalled", a.display_name),
                    format!(
                        "nothing from the model for {}m and no step running · it keeps waiting until you stop it or send it something",
                        (now - a.stalled).max(0) / 60_000
                    ),
                    false,
                );
            }
        }
        // A turn that died on an error goes nowhere until someone says so;
        // the menu bar only knows about questions, so this is said here.
        if !first
            && !quiet
            && !prev.is_empty()
            && a.halted()
            && !was_failing
            && !watching
            && !handled
        {
            notify_later(
                &mut f,
                format!("{} stopped", a.display_name),
                a.halt_reason(now),
                false,
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

    fn told(m: &mut Model) -> Vec<(String, String)> {
        std::mem::take(&mut m.kept::<Finish>().told)
    }

    fn working_agent() -> Agent {
        let mut a = Agent {
            key: "k".into(),
            display_name: "porter".into(),
            pid: std::process::id() as i32,
            ..Agent::default()
        };
        a.job.state = State::Working;
        a
    }

    /// One that starts waiting on you is a notification, once for the same
    /// decisions; not while you're looking at it, nor in a quiet rush or
    /// one that follows another (model.go:1990-2001).
    #[test]
    fn needs_you_notifies() {
        let mut m = Model::new("test");
        m.clock = Some(1_790_000_000_000);
        let mut a = working_agent();
        read(&mut m, &a);
        read(&mut m, &a);
        a.job.state = State::Blocked;
        a.job.needs = "allow Bash?".into();
        a.decisions_key = "d1".into();
        read(&mut m, &a);
        assert_eq!(
            told(&mut m),
            [("porter needs you".to_string(), "allow Bash?".to_string())]
        );
        // Answered, then the same decisions asked again: said already.
        a.job.state = State::Working;
        read(&mut m, &a);
        a.job.state = State::Blocked;
        read(&mut m, &a);
        assert!(told(&mut m).is_empty(), "the same decisions twice");
        // Following another rush: it notifies.
        a.job.state = State::Working;
        a.decisions_key.clear();
        read(&mut m, &a);
        m.follows = true;
        a.job.state = State::Blocked;
        read(&mut m, &a);
        assert!(told(&mut m).is_empty(), "a follower is quiet");
    }

    /// A stall is said once, and is a notification (model.go:2004-2011).
    #[test]
    fn a_stall_notifies() {
        let mut m = Model::new("test");
        let now = 1_790_000_000_000;
        m.clock = Some(now);
        let mut a = working_agent();
        read(&mut m, &a);
        a.stalled = now - 5 * 60_000;
        read(&mut m, &a);
        assert_eq!(
            told(&mut m),
            [(
                "porter stalled".to_string(),
                "nothing from the model for 5m and no step running · it keeps waiting until you stop it or send it something".to_string()
            )]
        );
        read(&mut m, &a);
        assert!(told(&mut m).is_empty(), "once a stall");
    }

    /// A usage limit rush moves a session on from by itself is noted
    /// quietly, not as an error: here another login has room
    /// (limitHandled, model.go:1963-1969).
    #[test]
    fn a_handled_limit_is_noted_quietly() {
        let mut m = Model::new("test");
        let now = 1_790_000_000_000;
        m.clock = Some(now);
        let login = |current: bool| crate::screens::accounts::state::LoginView {
            current,
            quota: rush_core::usage::Quota {
                fetched_at: now,
                ..Default::default()
            },
            ..Default::default()
        };
        crate::screens::accounts::state::accounts(&mut m).logins = vec![login(true), login(false)];
        let mut a = working_agent();
        a.rush = true;
        a.kind = rush_state::LOGINS_KIND.into();
        read(&mut m, &a);
        read(&mut m, &a);
        a.job.state = State::Blocked;
        a.job.detail = "usage limit · resets 5pm".into();
        read(&mut m, &a);
        let f = m.kept::<Finish>();
        let last = f.events.last().expect("noted");
        assert_eq!(
            (last.glyph.as_str(), last.what.as_str()),
            ("↻", "usage limit · rush moves it on")
        );
        assert_ne!(f.fx.kind, Fx::Error, "nothing went wrong");
        // A rush that waits at a limit says it stopped.
        assert!(!limit_handled(
            &m,
            &Agent {
                rush: false,
                ..a.clone()
            }
        ));
    }
}
