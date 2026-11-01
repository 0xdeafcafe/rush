//! The runaway guard (Rust-only; Go has none): three checks, each off
//! until you turn it on, that keep a busy few hours from spending a week's
//! limit. No terminal or platform code: the UI hands in what it sees and
//! does what [`plan`] says.
//!
//! - Pace, per account: its fair share is the weekly room left over the
//!   time to the weekly reset; its burn is how much of the week it used
//!   over the last few hours, from the plan's own readings (every machine
//!   and session on the account, priced by Anthropic). Past `warn` times
//!   its fair share it warns, past `trip` it trips, and it stays tripped
//!   until it's back under `release`.
//! - The agent cap: at most so many agents working at once. A session's
//!   turn counts 1, and each subagent and workflow run it has going 1
//!   more; background shells don't count.
//! - Away: while you're away, a lower cap, no new subagents or workflows,
//!   and each session's running turn finishes, then holds.
//!
//! A session on an account past its pace is refused new subagents and
//! workflows as well (`Plan::spawns_for`), until you let it through.
//!
//! What trips never cuts a running turn: a session's next turn waits
//! (held), or moves to another account. Moving costs: the session's
//! context is written again at full price on the new account unless its
//! prompt cache was cold anyway. So a new session starts on the account
//! with the most room freely; one with a warm cache waits; a cold one
//! moves, at most once in `move_gap`; work only moves to an account
//! clearly under its pace (below `release`), so it never flips between
//! two near the line.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::time::Ms;

const MIN: Ms = 60_000;
const HOUR: Ms = 60 * MIN;

/// Where every rush keeps the weekly readings it has seen, in its folder.
pub const FILE: &str = "weekuse.json";

/// How long readings are kept for: the longest a burn is measured over.
const KEEP: Ms = 3 * HOUR;

/// A week that resets more than this later than the one before is a new
/// week: a reading's reset time moves by seconds between readings.
const NEW_WEEK: Ms = HOUR;

/// One reading of a login's week: when it was read, how much of the week
/// was used then, and when that week resets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sample(pub Ms, pub f64, pub Ms);

/// Every login's readings of its week over the last few hours, by login
/// id, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct History(pub BTreeMap<String, Vec<Sample>>);

impl History {
    /// What's kept at path; none when it can't be read.
    pub fn load(path: &Path) -> History {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes it to path whole, so a reader never sees half of it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(
            &tmp,
            serde_json::to_vec(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, path)
    }

    /// Keeps a reading of login id's week; what's older than needed at now
    /// goes. Whether anything changed.
    pub fn note(&mut self, id: &str, s: Sample, now: Ms) -> bool {
        if id.is_empty() || s.0 == 0 {
            return false;
        }
        let v = self.0.entry(id.to_string()).or_default();
        let mut changed = false;
        if !v.iter().any(|o| o.0 == s.0) {
            let at = v.partition_point(|o| o.0 < s.0);
            v.insert(at, s);
            changed = true;
        }
        let base = v.iter().rposition(|s| s.0 <= now - KEEP).unwrap_or(0);
        if base > 0 {
            v.drain(..base);
            changed = true;
        }
        changed
    }

    /// Forgets the accounts with no reading in the last few hours: one
    /// gone, or not read lately. (Not by the accounts listed: a rush that
    /// has yet to read its logins would forget them all.)
    pub fn forget_stale(&mut self, now: Ms) -> bool {
        let n = self.0.len();
        self.0
            .retain(|_, v| v.last().is_some_and(|s| s.0 > now - KEEP));
        n != self.0.len()
    }

    /// Login id's readings.
    pub fn of(&self, id: &str) -> &[Sample] {
        self.0.get(id).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// What each reading after base added to the week, with when it was read,
/// up to now: a reading of a new week counts from 0, and one lower than
/// the most seen this week (an older cache) adds nothing.
fn uses(v: &[Sample], base: usize, now: Ms) -> Vec<(Ms, f64)> {
    let mut out = Vec::new();
    let Some(first) = v.get(base) else {
        return out;
    };
    let (mut level, mut week) = (first.1, first.2);
    for s in &v[base + 1..] {
        if s.0 > now {
            break;
        }
        if s.2 != 0 && week != 0 && s.2 - week > NEW_WEEK {
            level = 0.0;
        }
        if s.2 != 0 {
            week = s.2;
        }
        let d = (s.1 - level).max(0.0);
        level = level.max(s.1);
        if d > 0.0 {
            out.push((s.0, d));
        }
    }
    out
}

/// How the pace is judged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaceCfg {
    /// Warns past this many times the fair share.
    pub warn: f64,
    /// Holds or moves work past this many.
    pub trip: f64,
    /// Tripped stays tripped until under this, and work only moves to an
    /// account under it.
    pub release: f64,
    /// The burn is measured over this long.
    pub window: Ms,
    /// Less than this much of it read says nothing yet.
    pub least: Ms,
}

impl Default for PaceCfg {
    fn default() -> PaceCfg {
        PaceCfg {
            warn: 1.5,
            trip: 2.0,
            release: 1.2,
            window: 2 * HOUR,
            least: 30 * MIN,
        }
    }
}

/// Where an account stands against its pace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    /// Not enough read yet to say.
    #[default]
    Unknown,
    Under,
    Warn,
    Over,
}

/// An account's pace at a moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pace {
    /// How fast it's using its week, and how fast it may: percent an hour.
    pub burn: f64,
    pub fair: f64,
    /// burn over fair.
    pub ratio: f64,
    pub level: Level,
    /// When it's back under release with no more use: 0 when it isn't
    /// over.
    pub frees_at: Ms,
    /// Its week: how much is used and when it resets.
    pub week: f64,
    pub resets_at: Ms,
}

/// How fast a week used to week% may be used from at to its reset at
/// resets_at, in percent an hour.
fn fair(week: f64, resets_at: Ms, at: Ms) -> f64 {
    let hours = ((resets_at - at) as f64 / HOUR as f64).max(0.25);
    (100.0 - week).max(0.0) / hours
}

/// An account's pace from its week's readings v, at now; was_over says it
/// had tripped and not yet come back under release.
pub fn pace(v: &[Sample], c: &PaceCfg, was_over: bool, now: Ms) -> Pace {
    let Some(last) = v.iter().rev().find(|s| s.0 <= now) else {
        return Pace::default();
    };
    let mut p = Pace {
        week: last.1,
        resets_at: last.2,
        ..Pace::default()
    };
    if last.2 == 0 || last.2 <= now {
        return p; // a week rush doesn't know the end of, or one just over
    }
    p.fair = fair(last.1, last.2, now);
    let base = v.iter().rposition(|s| s.0 <= now - c.window).unwrap_or(0);
    let span = now - v[base].0;
    if span < c.least {
        return p;
    }
    let us = uses(v, base, now);
    let used: f64 = us.iter().map(|u| u.1).sum();
    p.burn = used / (span as f64 / HOUR as f64);
    p.ratio = if p.fair > 0.0 {
        p.burn / p.fair
    } else if p.burn > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };
    p.level = if p.ratio >= c.trip || was_over && p.ratio >= c.release {
        Level::Over
    } else if p.ratio >= c.warn {
        Level::Warn
    } else {
        Level::Under
    };
    if p.level == Level::Over {
        p.frees_at = frees_at(&us, used, &p, c);
    }
    p
}

/// When an account over its pace is back under release, if it uses no
/// more: as each use ages out of the window its burn drops, and as its
/// reset nears its fair share grows. Its reset at the latest.
fn frees_at(us: &[(Ms, f64)], used: f64, p: &Pace, c: &PaceCfg) -> Ms {
    let mut left = used;
    let hours = c.window as f64 / HOUR as f64;
    for (at, d) in us {
        left -= d;
        let t = at + c.window;
        if t >= p.resets_at {
            return p.resets_at;
        }
        if left / hours < c.release * fair(p.week, p.resets_at, t) {
            return t;
        }
    }
    us.last().map_or(p.resets_at, |u| u.0 + c.window)
}

/// How much an account's room is worth spending now: the room it would
/// leave unused by its reset at the pace it goes, an hour, counting less
/// the further off its reset is. Room on one that resets in 4 hours is
/// lost if it isn't used; on one that resets in 6 days it has to last.
fn spare(p: &Pace, now: Ms) -> f64 {
    let hours = ((p.resets_at - now) as f64 / HOUR as f64).max(0.25);
    (p.fair - p.burn).max(0.0) / (1.0 + hours / 24.0)
}

/// "alex resets in 4h with 38% left".
pub fn room_said(a: &Acct, now: Ms) -> String {
    format!(
        "{} resets in {} with {:.0}% left",
        a.name,
        roughly(a.pace.resets_at - now),
        (100.0 - a.pace.week).max(0.0)
    )
}

/// A duration to one unit: 40m, 3h, 5d.
pub fn roughly(d: Ms) -> String {
    let d = d.max(0);
    if d < HOUR {
        format!("{}m", (d / MIN).max(1))
    } else if d < 48 * HOUR {
        format!("{}h", (d + HOUR / 2) / HOUR)
    } else {
        format!("{}d", (d + 12 * HOUR) / (24 * HOUR))
    }
}

/// "2.1×".
pub fn times(r: f64) -> String {
    if r.is_infinite() {
        "∞×".into()
    } else {
        format!("{r:.1}×")
    }
}

/// The guard as set: each part None or 0 while off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub pace: Option<PaceCfg>,
    /// At most this many agents working at once; 0 is no cap.
    pub cap: usize,
    pub away: Option<AwayCfg>,
    /// A session moves to another account at most once in this long.
    pub move_gap: Ms,
}

impl Settings {
    /// Whether any part is on.
    pub fn on(&self) -> bool {
        self.pace.is_some() || self.cap > 0 || self.away.is_some()
    }
}

/// What's stricter while you're away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AwayCfg {
    /// Away after this long with no keys.
    pub after: Ms,
    /// At most this many agents working at once while you are.
    pub cap: usize,
}

/// One account, as the guard weighs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Acct {
    pub id: String,
    pub name: String,
    pub pace: Pace,
    /// Nearly out of a real limit: never moved to.
    pub out: bool,
    /// Someone else's: never moved to on rush's own.
    pub borrowed: bool,
    /// The one new sessions start on.
    pub current: bool,
}

/// One rush session, as the guard weighs it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seat {
    pub id: String,
    /// The account it runs on, by login id.
    pub login: String,
    pub working: bool,
    /// The agents it has working: its turn, and each subagent and
    /// workflow run (0 when idle with none).
    pub agents: usize,
    /// Its turns so far: 0 is a new session, with no cache to lose.
    pub turns: i64,
    /// When its prompt cache goes cold; 0 or past: it is.
    pub warm_until: Ms,
    /// How many tokens its context holds: what a move writes again.
    pub context: i64,
    /// When the guard last moved it to another account.
    pub moved_at: Ms,
    /// When the guard began holding it; 0 when it doesn't.
    pub held_since: Ms,
    /// Messages wait in its queue: let through, a turn starts.
    pub queued: bool,
    /// You've set it to keep working while you're away (#away).
    pub keeps_on: bool,
    /// You let its held turn through: it isn't held again until that
    /// turn has run.
    pub let_through: bool,
}

/// Why the guard holds a session's next turn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Hold {
    /// Its account is past its pace and moving costs more than waiting.
    Pace {
        acct: String,
        ratio: f64,
        frees_at: Ms,
        why: String,
    },
    /// Every account is past its pace.
    AllPaced { frees_at: Ms },
    /// The cap: so many agents working of so many.
    Cap { working: usize, cap: usize },
    /// You're away.
    Away,
}

impl Hold {
    /// The hold in a few words, for a row and the header.
    pub fn say(&self, clock: &dyn Fn(Ms) -> String) -> String {
        match self {
            Hold::Pace {
                acct,
                ratio,
                frees_at,
                ..
            } => {
                let mut s = format!("held: past your pace on {acct} · {}", times(*ratio));
                if *frees_at != 0 {
                    s += &format!(" · frees at {}", clock(*frees_at));
                }
                s
            }
            Hold::AllPaced { frees_at } => {
                let mut s = "held: every account is past its pace".to_string();
                if *frees_at != 0 {
                    s += &format!(" · frees at {}", clock(*frees_at));
                }
                s
            }
            Hold::Cap { working, cap } => {
                format!("held: {working} of {cap} agents working")
            }
            Hold::Away => "held: you're away · it carries on when you're back".into(),
        }
    }

    /// The hold as a row or queue card says it while the session's turn is
    /// still running: a pace hold only pauses what comes after the turn.
    /// Other holds read as they do.
    pub fn say_running(&self, clock: &dyn Fn(Ms) -> String) -> String {
        let s = self.say(clock);
        match self {
            Hold::Pace { .. } | Hold::AllPaced { .. } => match s.strip_prefix("held: ") {
                Some(rest) => format!("pauses after this turn: {rest}"),
                None => s,
            },
            _ => s,
        }
    }

    /// The kind, for the events log.
    pub fn kind(&self) -> &'static str {
        match self {
            Hold::Pace { .. } => "pace",
            Hold::AllPaced { .. } => "every account past its pace",
            Hold::Cap { .. } => "cap",
            Hold::Away => "away",
        }
    }
}

/// What the guard does with a session.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// Nothing: it goes as it would.
    Go,
    /// Its next turn waits; a running turn finishes first.
    Hold(Hold),
    /// Its next turn starts on account to: cold says its cache was cold
    /// anyway; why says how that was weighed.
    Move {
        to: String,
        from: String,
        cold: bool,
        context: i64,
        why: String,
    },
    /// The guard held it, and needn't any more.
    Release,
}

/// What the guard does now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// The account new sessions should start on, when it should change.
    pub new_on: Option<String>,
    /// Each session's, by id, in the order given.
    pub seats: Vec<(String, Do)>,
    /// Agents working, and the cap that applies (0: none).
    pub working: usize,
    pub cap: usize,
    /// Why new subagents and workflows are refused; None when they aren't.
    pub spawns: Option<String>,
    /// Sessions refused new subagents and workflows on their own, by id,
    /// and why: those on an account past its pace (unless let through).
    pub spawns_for: Vec<(String, String)>,
    /// Accounts past their pace, by id.
    pub over: Vec<String>,
    /// Every account is past its pace: when the first frees up.
    pub all_paced: Option<Ms>,
    /// Why new_on: its reset and room.
    pub to_why: String,
}

/// What the guard does at now with accts and the rush sessions seats;
/// others is how many agents work outside rush's sessions, away whether
/// you're away.
pub fn plan(
    s: &Settings,
    accts: &[Acct],
    seats: &[Seat],
    others: usize,
    away: bool,
    now: Ms,
) -> Plan {
    let mut p = Plan::default();
    let away = away && s.away.is_some();
    // Pace: the accounts over it, and where work goes instead.
    let over: HashSet<&str> = if s.pace.is_some() {
        accts
            .iter()
            .filter(|a| a.pace.level == Level::Over)
            .map(|a| a.id.as_str())
            .collect()
    } else {
        HashSet::new()
    };
    p.over = accts
        .iter()
        .filter(|a| over.contains(a.id.as_str()))
        .map(|a| a.id.clone())
        .collect();
    let release = s.pace.map_or(0.0, |c| c.release);
    let cur = accts.iter().find(|a| a.current);
    let mut dest: Option<&Acct> = cur.filter(|a| !over.contains(a.id.as_str()));
    if !over.is_empty() && dest.is_none() {
        // Clearly under its pace, and not tripped by the work it takes
        // on (the burn of the one it leaves): of those, the one whose
        // room would go unused soonest.
        let load = cur.map_or(0.0, |c| c.pace.burn);
        let trip = s.pace.map_or(f64::INFINITY, |c| c.trip);
        dest = accts
            .iter()
            .filter(|a| {
                !a.current
                    && !a.borrowed
                    && !a.out
                    && a.pace.level != Level::Unknown
                    && a.pace.ratio < release
                    && a.pace.fair > 0.0
                    && (a.pace.burn + load) / a.pace.fair < trip
            })
            .max_by(|a, b| {
                spare(&a.pace, now)
                    .total_cmp(&spare(&b.pace, now))
                    .then(b.pace.week.total_cmp(&a.pace.week))
            });
        match dest {
            Some(d) => p.new_on = Some(d.id.clone()),
            None => {
                p.all_paced = accts
                    .iter()
                    .filter(|a| over.contains(a.id.as_str()) && a.pace.frees_at != 0)
                    .map(|a| a.pace.frees_at)
                    .min()
                    .or(Some(0));
            }
        }
    }
    if let Some(d) = dest.filter(|_| !over.is_empty()) {
        p.to_why = room_said(d, now);
    }
    // The cap.
    p.cap = match (away, s.away) {
        (true, Some(a)) if s.cap == 0 || a.cap < s.cap => a.cap,
        _ => s.cap,
    };
    p.working = others + seats.iter().map(|x| x.agents).sum::<usize>();
    if away {
        p.spawns = Some("you're away: no new subagents or workflows until you're back".into());
    } else if p.cap > 0 && p.working >= p.cap {
        p.spawns = Some(format!(
            "{} of {} agents are working: no new ones until some finish",
            p.working, p.cap
        ));
    }
    // Held for the cap, in the order held: let through while there's room.
    let mut room = p.cap.saturating_sub(p.working);
    let mut order: Vec<usize> = (0..seats.len()).collect();
    order.sort_by_key(|&i| (seats[i].held_since == 0, seats[i].held_since));
    let mut out: Vec<Option<Do>> = vec![None; seats.len()];
    let name = |id: &str| {
        accts
            .iter()
            .find(|a| a.id == id)
            .map_or(id.to_string(), |a| a.name.clone())
    };
    for i in order {
        let x = &seats[i];
        if over.contains(x.login.as_str()) && !x.let_through {
            let n = accts
                .iter()
                .find(|a| a.id == x.login)
                .map_or(x.login.as_str(), |a| a.name.as_str());
            p.spawns_for.push((
                x.id.clone(),
                format!("the account this session runs on ({n}) is past its pace"),
            ));
        }
        let d = if x.let_through {
            Do::Release
        } else if away && !x.keeps_on {
            Do::Hold(Hold::Away)
        } else if over.contains(x.login.as_str()) {
            let a = accts
                .iter()
                .find(|a| a.id == x.login)
                .expect("over is of accts");
            let held = |why: String| {
                Do::Hold(Hold::Pace {
                    acct: a.name.clone(),
                    ratio: a.pace.ratio,
                    frees_at: a.pace.frees_at,
                    why,
                })
            };
            match dest {
                None => Do::Hold(Hold::AllPaced {
                    frees_at: p.all_paced.unwrap_or(0),
                }),
                Some(to) if x.turns == 0 && !x.working => Do::Move {
                    to: to.id.clone(),
                    from: x.login.clone(),
                    cold: true,
                    context: 0,
                    why: format!("a new session: no cache to lose · {}", p.to_why),
                },
                Some(_) if x.moved_at != 0 && now - x.moved_at < s.move_gap => held(format!(
                    "it moved account once already in the last {}h: it waits for {}'s pace to ease",
                    s.move_gap / HOUR,
                    a.name
                )),
                Some(_) if x.working => held("its turn runs to the end; the next waits".into()),
                Some(to) if x.warm_until > now => held(format!(
                    "its cache is warm: moving to {} would write its {} tokens of context again; it waits for {}'s pace to ease, or its cache to cool",
                    to.name, x.context, a.name
                )),
                Some(to) => Do::Move {
                    to: to.id.clone(),
                    from: x.login.clone(),
                    cold: true,
                    context: x.context,
                    why: format!(
                        "its cache is cold: its {} tokens of context are written again here or on {} alike · {}",
                        x.context,
                        name(&to.id),
                        p.to_why
                    ),
                },
            }
        } else if p.cap > 0 && !x.working && p.working >= p.cap && room == 0 {
            Do::Hold(Hold::Cap {
                working: p.working,
                cap: p.cap,
            })
        } else if p.cap > 0 && !x.working && x.held_since != 0 && x.queued {
            if room > 0 {
                room -= 1;
                Do::Release
            } else {
                Do::Hold(Hold::Cap {
                    working: p.working,
                    cap: p.cap,
                })
            }
        } else if x.held_since != 0 {
            Do::Release
        } else {
            Do::Go
        };
        out[i] = Some(d);
    }
    p.seats = seats
        .iter()
        .zip(out)
        .map(|(x, d)| (x.id.clone(), d.unwrap_or(Do::Go)))
        .collect();
    p
}

#[cfg(test)]
mod tests;
