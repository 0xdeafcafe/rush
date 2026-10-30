//! The gate shares the machine as an OS would, not as a queue: a run's CPU
//! is never held back, only lowered as it ages; memory is the one thing a
//! run waits for, and waiters go fairly by agent. The decisions are here,
//! as pure functions; gate.rs and watch.rs do what they say.
//! Ported from internal/gate/share.go:1-162.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::godur;
use crate::rules::{Rule, size};

/// One gated run, waiting or running, as its files have it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ticket {
    /// Its file's name, which sorts by when it was taken.
    pub file: String,
    /// Its holder's.
    pub pid: i32,
    pub name: String,
    /// Its agent: RUSH_SESSION, else the agent's pid.
    pub session: String,
    pub since: Option<SystemTime>,
    /// What it's expected to need.
    pub memory: u64,
    pub running: bool,
    pub state: State,
}

/// What a running ticket's holder last said of its run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub nice: i32,
    /// Bytes its processes hold.
    pub footprint: u64,
    pub paused_at: Option<SystemTime>,
    /// Why it's paused; "" when it isn't.
    pub why: String,
}

impl State {
    /// Whether its run is stopped for memory.
    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }
}

impl Ticket {
    pub fn paused(&self) -> bool {
        self.state.paused()
    }
}

/// The waiters in ts in the order they go: the one whose agent has the
/// fewest runs going first, the oldest ticket of those first.
pub fn order(ts: &[Ticket]) -> Vec<Ticket> {
    let mut running: HashMap<&str, usize> = HashMap::new();
    let mut waiting = Vec::new();
    for t in ts {
        if t.running {
            *running.entry(&t.session).or_default() += 1;
        } else {
            waiting.push(t.clone());
        }
    }
    let n = |s: &str| running.get(s).copied().unwrap_or(0);
    waiting.sort_by(|a, b| {
        n(&a.session)
            .cmp(&n(&b.session))
            .then_with(|| a.file.cmp(&b.file))
    });
    waiting
}

/// The memory there is for one more run: what's available, less the
/// reserve, less what the runs going are expected to take yet (their
/// estimate past what they hold now), so starts in a row don't each count
/// the same free memory.
pub fn room(ts: &[Ticket], avail: u64, reserve: u64) -> u64 {
    let mut take = reserve;
    for t in ts {
        if t.running && t.memory > t.state.footprint {
            take = take.saturating_add(t.memory - t.state.footprint);
        }
    }
    avail.saturating_sub(take)
}

/// Whether next may start beside ts: always when nothing gated runs, else
/// when the room covers what it's expected to need.
pub fn admit(next: &Ticket, ts: &[Ticket], avail: u64, reserve: u64) -> bool {
    if ts.iter().any(|t| t.running) {
        return room(ts, avail, reserve) >= next.memory;
    }
    true
}

/// The nice a run gets once it has run for d: short checks keep the CPU,
/// and a long run makes way for them, though it keeps the CPU no one else
/// wants.
pub fn nice_at(d: Duration) -> i32 {
    let s = Duration::from_secs;
    if d >= s(600) {
        15
    } else if d >= s(120) {
        10
    } else if d >= s(30) {
        5
    } else {
        0
    }
}

/// Pressure levels, as kern.memorystatus_vm_pressure_level has them; 0 is
/// unknown.
pub const PRESSURE_NORMAL: i32 = 1;
pub const PRESSURE_WARN: i32 = 2;
pub const PRESSURE_CRITICAL: i32 = 4;

/// What a holder's watchdog sees of its run each tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    /// Running, its time paused left out.
    pub ran: Duration,
    pub footprint: u64,
    /// Whether an agent's program started it; agent_alive whether that's
    /// still running. orphaned is whether it's been handed to launchd, and
    /// session_alive whether its rush session still runs.
    pub has_agent: bool,
    pub agent_alive: bool,
    pub orphaned: bool,
    pub session_alive: bool,
    pub pressure: i32,
    pub paused: bool,
    /// Whether it's the newest-started run going unpaused, and others how
    /// many other runs go unpaused.
    pub newest: bool,
    pub others: usize,
    /// Since any run on the machine was last paused.
    pub since_pause: Duration,
}

/// What the watchdog does to a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Carry,
    Stop,
    Pause,
    Resume,
}

/// How often a holder looks at its run.
pub const TICK: Duration = Duration::from_secs(2);

/// What the watchdog does to a run of r, as l sees it, and why. A run its
/// agent left, or past its time or its memory, is stopped. Under critical
/// memory pressure the newest run going is paused, one a tick and never
/// the last, so the oldest work finishes; back to normal, it goes on.
pub fn judge(r: &Rule, l: &Look) -> (Act, String) {
    if l.has_agent && !l.agent_alive {
        return (Act::Stop, "the agent that started it is gone".into());
    }
    if !l.has_agent && l.orphaned && !l.session_alive {
        return (
            Act::Stop,
            "it was left running with no session to wait on it".into(),
        );
    }
    if !r.limit.is_zero() && l.ran > r.limit {
        return (
            Act::Stop,
            format!("it ran past its {} limit", godur::string(r.limit)),
        );
    }
    if r.memory > 0 && l.footprint > r.cap() {
        return (
            Act::Stop,
            format!(
                "it held {}, past its {} cap (three times the {} it's expected to need)",
                size(l.footprint),
                size(r.cap()),
                size(r.memory)
            ),
        );
    }
    if l.paused && l.pressure <= PRESSURE_NORMAL {
        return (Act::Resume, "memory is back to normal".into());
    }
    if !l.paused
        && l.pressure == PRESSURE_CRITICAL
        && l.newest
        && l.others > 0
        && l.since_pause >= TICK
    {
        return (
            Act::Pause,
            "the machine is critically short of memory".into(),
        );
    }
    (Act::Carry, String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::GIB;

    fn ticket(file: &str, session: &str, running: bool) -> Ticket {
        Ticket {
            file: file.into(),
            session: session.into(),
            running,
            ..Ticket::default()
        }
    }

    // share_test.go TestOrderIsFairByAgent
    #[test]
    fn order_is_fair_by_agent() {
        let ts = [
            ticket("ticket-1", "busy", true),
            ticket("ticket-2", "busy", true),
            ticket("ticket-3", "busy", false),
            ticket("ticket-4", "calm", true),
            ticket("ticket-5", "calm", false),
            ticket("ticket-6", "new", false),
            ticket("ticket-7", "new", false),
        ];
        let got: Vec<String> = order(&ts).into_iter().map(|t| t.file).collect();
        assert_eq!(got, ["ticket-6", "ticket-7", "ticket-5", "ticket-3"]);
    }

    // share_test.go TestAdmit
    #[test]
    fn admit_counts_what_runs_will_take() {
        let next = Ticket {
            memory: 4 * GIB,
            ..Ticket::default()
        };
        assert!(admit(&next, &[], 0, 0), "held with nothing running");
        // One running expects 4 GiB and holds 1 of it: 3 more are spoken for.
        let ts = [Ticket {
            running: true,
            memory: 4 * GIB,
            state: State {
                footprint: GIB,
                ..State::default()
            },
            ..Ticket::default()
        }];
        assert!(!admit(&next, &ts, 7 * GIB, GIB));
        assert!(admit(&next, &ts, 8 * GIB, GIB));
        assert_eq!(room(&ts, GIB, 2 * GIB), 0);
    }

    // share_test.go TestNiceAt
    #[test]
    fn nice_with_age() {
        let s = Duration::from_secs;
        for (d, want) in [
            (s(0), 0),
            (s(29), 0),
            (s(30), 5),
            (s(119), 5),
            (s(120), 10),
            (s(540), 10),
            (s(600), 15),
            (s(3 * 3600), 15),
        ] {
            assert_eq!(nice_at(d), want, "{d:?}");
        }
    }

    // share_test.go TestJudge
    #[test]
    fn judge_runs() {
        let r = Rule {
            memory: 2 * GIB,
            limit: Duration::from_secs(30 * 60),
        };
        let ok = Look {
            ran: Duration::from_secs(60),
            footprint: GIB,
            has_agent: true,
            agent_alive: true,
            pressure: PRESSURE_NORMAL,
            newest: true,
            others: 1,
            since_pause: Duration::from_secs(3600),
            ..Look::default()
        };
        type Change = fn(&mut Look);
        let cases: &[(&str, Change, Act)] = &[
            ("healthy", |_| {}, Act::Carry),
            ("agent gone", |l| l.agent_alive = false, Act::Stop),
            (
                "orphaned, no session",
                |l| {
                    l.has_agent = false;
                    l.orphaned = true
                },
                Act::Stop,
            ),
            (
                "orphaned, session lives",
                |l| {
                    l.has_agent = false;
                    l.orphaned = true;
                    l.session_alive = true
                },
                Act::Carry,
            ),
            (
                "orphaned under a live agent",
                |l| l.orphaned = true,
                Act::Carry,
            ),
            (
                "past its time",
                |l| l.ran = Duration::from_secs(31 * 60),
                Act::Stop,
            ),
            ("past its cap", |l| l.footprint = 6 * GIB + 1, Act::Stop),
            ("at its cap", |l| l.footprint = 6 * GIB, Act::Carry),
            (
                "critical, newest",
                |l| l.pressure = PRESSURE_CRITICAL,
                Act::Pause,
            ),
            (
                "critical, not newest",
                |l| {
                    l.pressure = PRESSURE_CRITICAL;
                    l.newest = false
                },
                Act::Carry,
            ),
            (
                "critical, the last going",
                |l| {
                    l.pressure = PRESSURE_CRITICAL;
                    l.others = 0
                },
                Act::Carry,
            ),
            (
                "critical, one paused this tick",
                |l| {
                    l.pressure = PRESSURE_CRITICAL;
                    l.since_pause = Duration::from_secs(1)
                },
                Act::Carry,
            ),
            (
                "warning, paused",
                |l| {
                    l.pressure = PRESSURE_WARN;
                    l.paused = true
                },
                Act::Carry,
            ),
            ("normal, paused", |l| l.paused = true, Act::Resume),
            (
                "critical, paused",
                |l| {
                    l.pressure = PRESSURE_CRITICAL;
                    l.paused = true
                },
                Act::Carry,
            ),
        ];
        for (name, change, want) in cases {
            let mut l = ok.clone();
            change(&mut l);
            let (got, why) = judge(&r, &l);
            assert_eq!(got, *want, "{name}: {why}");
        }
        let (_, why) = judge(
            &r,
            &Look {
                ran: Duration::from_secs(31 * 60),
                ..ok.clone()
            },
        );
        assert_eq!(why, "it ran past its 30m0s limit");
        let (_, why) = judge(
            &r,
            &Look {
                footprint: 7 * GIB,
                ..ok
            },
        );
        assert_eq!(
            why,
            "it held 7.0 GiB, past its 6.0 GiB cap (three times the 2.0 GiB it's expected to need)"
        );
    }
}
