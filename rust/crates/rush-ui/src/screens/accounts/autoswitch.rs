//! Moving new sessions to another Claude Code login before the one in use
//! runs out: which login is next, and when it's worth the move. After
//! Go's internal/fleet/logins.go (NextLogin :69, Urgency :114, Left :124,
//! Week :133, switchFor/minRoom :145, otherFor :152) and
//! internal/ui/logins.go (switchGap/earlyGap :57, autoSwitch :240). And
//! every other agent's: switching to another of its accounts when the one
//! in use is nearly out (agentquota.go:90-114 checkLimits, nextAccount
//! :121, profiles.go:73 limitStopped).
//!
//! The switch itself is the vault's ([`super::keys::LoginOps`], set by
//! [`super::logins`] with a loop running); carrying on the sessions a
//! limit stopped is [`super::state::Accounts::limit_stopped`]'s and
//! `relogin`'s (logins.rs's unless the screens that host sessions set
//! their own). Only the rush that leads switches on its own (`leads`).

use rush_core::time::Ms;
use rush_core::usage::{self, Quota, Window};
use rush_state::config::SWITCH_AT;
use rush_state::profiles::LIMIT_WAIT;

use super::keys::{has_room, login_ops, on_borrowed, switch_account};
use super::look::LOGINS_KIND;
use super::rows::{AcctRow, account_rows, accounts_of, agent_order, roughly, switches};
use super::state::{LoginView, accounts, cfg, peek};
use crate::frame::Out;
use crate::model::Model;

const MIN: Ms = 60_000;
const HOUR: Ms = 60 * MIN;

/// How long after a switch rush waits before switching on its own again:
/// the readings of both logins need time to catch up (switchGap).
pub const SWITCH_GAP: Ms = 10 * MIN;
/// How long it waits before moving early, while the one in use still has
/// room (earlyGap).
pub const EARLY_GAP: Ms = HOUR;

/// How many times more urgent another login must be than the one in use,
/// with room still, to move to it; minRoom is how much room it must have
/// left, so a few percent about to reset isn't worth the move.
const SWITCH_FOR: f64 = 2.0;
const MIN_ROOM: f64 = 5.0;

/// How old a reading of a login not in use may be and still be switched
/// to (otherFor).
const OTHER_FOR: Ms = HOUR;

/// q's longest window that limits every model: its scarce one, whose
/// unspent room is lost when it resets (Week).
pub fn week(q: &Quota) -> Option<&Window> {
    let mut out: Option<&Window> = None;
    for c in &q.windows {
        if c.scope.models.is_empty() && out.is_none_or(|w| c.span > w.span) {
            out = Some(c);
        }
    }
    out
}

/// How long until w resets, in ms: all of it when it hasn't begun (Left).
pub fn left(w: &Window, now: Ms) -> Ms {
    if w.resets_at == 0 {
        return w.span.as_millis() as Ms;
    }
    w.resets_at - now
}

/// How fast q must be spent, in percent an hour, not to lose any of the
/// room left in its longest window by the time it resets (Urgency).
pub fn urgency(q: &Quota, now: Ms) -> f64 {
    let Some(w) = week(q) else {
        return 0.0;
    };
    (100.0 - w.percent) / (left(w, now) as f64 / HOUR as f64).max(0.1)
}

/// The login to switch to. Room an account doesn't use before its week
/// resets is lost, so the one to spend is whichever loses the most
/// soonest: the most urgent login that isn't nearly out. rush moves to it
/// when the one in use is nearly out, stopped says a session already hit
/// a limit on it, or another would lose far more room by waiting. Once
/// the one in use is out altogether, any login with room left will do. A
/// borrowed login is never moved to (NextLogin).
pub fn next_login(logins: &[LoginView], stopped: bool, now: Ms) -> Option<LoginView> {
    let mut cur: Option<&LoginView> = None;
    let mut others: Vec<&LoginView> = Vec::new();
    for l in logins {
        if l.current {
            cur = Some(l);
        } else if l.borrowed {
        } else if now - l.quota.fetched_at < OTHER_FOR && !l.quota.windows.is_empty() {
            // Only a login with a reading: one rush can't read would look
            // empty. A login not in use only empties, so an older reading
            // of it still holds.
            others.push(l);
        }
    }
    let cur = cur?;
    if others.is_empty() {
        return None;
    }
    let fresh = now - cur.quota.fetched_at < 3 * usage::EVERY.as_millis() as Ms;
    if !stopped && !fresh {
        return None;
    }
    others.sort_by(|a, b| urgency(&b.quota, now).total_cmp(&urgency(&a.quota, now)));
    let nearly = cur.quota.nearly_out("", usage::LEAD, now);
    let out = stopped || cur.quota.used("") >= 100.0;
    for o in &others {
        if o.quota.used("") >= SWITCH_AT || o.quota.nearly_out("", usage::LEAD, now) {
            continue;
        }
        if stopped || nearly {
            return Some((*o).clone());
        }
        if 100.0 - o.quota.used("") >= MIN_ROOM
            && urgency(&o.quota, now) >= SWITCH_FOR * urgency(&cur.quota, now)
        {
            return Some((*o).clone());
        }
        return None; // the most urgent with room isn't worth moving for
    }
    if !out {
        return None;
    }
    // Out, and every other nearly: whichever has the most room now.
    let best = others
        .iter()
        .min_by(|a, b| a.quota.used("").total_cmp(&b.quota.used("")))?;
    (best.quota.used("") < 100.0).then(|| (*best).clone())
}

/// Whether a rush session on the login folder named root was stopped by a
/// usage limit, under a profile that moves on from it; set by the screens
/// that host sessions.
pub type LimitStopped = fn(&Model, &str) -> bool;
/// Tells every rush session in the folder named root that it runs as
/// another login now, carrying on those a limit stopped (reloginHosts);
/// set by the screens that host sessions.
pub type Relogin = fn(&mut Model, &str) -> Out;

/// Moves new sessions to another login when the one in use is nearly out
/// of its 5-hour or weekly usage, or a rush session was stopped by a
/// limit on it, unless the default profile waits and so do the sessions a
/// limit stopped (autoSwitch). Called after each reading of the logins.
pub fn auto_switch(m: &mut Model) -> Out {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let a = peek(m);
    // Only the rush that leads switches logins (model.go:1100).
    if !m.leads()
        || a.hosted
        || a.switching
        || a.store.is_none()
        || now - a.login_switched_at < SWITCH_GAP
    {
        return Out::Idle;
    }
    let root = cfg(m).active_account();
    let stopped = a.limit_stopped.is_some_and(|f| f(m, &root.name));
    if cfg(m).default_profile().limit() == LIMIT_WAIT && !stopped {
        return Out::Idle;
    }
    let logins = peek(m).logins.clone();
    if stopped && has_room(m) && logins.len() > 1 {
        // Stopped under a login rush has switched away from since: the
        // one in use has room, so they carry on in it.
        let a = accounts(m);
        if now - a.resumed_at < MIN {
            return Out::Idle;
        }
        a.resumed_at = now;
        return match a.relogin {
            Some(f) => f(m, &root.name),
            None => Out::Idle,
        };
    }
    let to = next_login(&logins, stopped, now);
    let (Some(to), true) = (to, logins.len() >= 2) else {
        // With no login left with room, the sessions whose profile hands
        // on go to the next provider: the host's (handOffStopped).
        return Out::Idle;
    };
    // Early: the one in use still has room, so only sessions that start
    // anew move; one with a warm cache stays until it cools.
    let early = !stopped && has_room(m);
    if early && (now - peek(m).login_switched_at < EARLY_GAP || on_borrowed(m)) {
        return Out::Idle; // a borrowed one you picked is kept until it runs low
    }
    let mut why = "a session hit a usage limit".to_string();
    for l in &logins {
        if !l.current || stopped {
            continue;
        }
        if has_room(m) {
            // Neither nearly out: to's room would be lost sooner.
            let (tw, lw) = (week(&to.quota), week(&l.quota));
            why = format!(
                "{}'s {:.0}% left resets in {}, {}'s in {}",
                to.name,
                100.0 - tw.map_or(0.0, |w| w.percent),
                roughly(tw.map_or(0, |w| left(w, now))),
                l.name,
                roughly(lw.map_or(0, |w| left(w, now)))
            );
        } else {
            why = format!("{} was at {:.0}%", l.name, l.quota.used(""));
        }
    }
    match login_ops(m) {
        Some(ops) => (ops.switch)(m, to.login, &why, !early),
        None => Out::Idle,
    }
}

/// Whether a rush session of agent k is stopped by a usage limit, under a
/// profile that moves on from it (limitStopped, profiles.go:73).
pub fn kind_limit_stopped(m: &Model, k: &str) -> bool {
    let c = cfg(m);
    m.agents.iter().any(|a| {
        a.rush
            && a.kind.as_str() == k
            && a.job.detail.starts_with("usage limit")
            && c.profile_for(&a.job.cwd, &a.profile).limit() != LIMIT_WAIT
    })
}

/// The account of agent k's to switch to when the one in use is nearly
/// out, and why: the one with the most room, unless that's nearly out
/// too (nextAccount over k's rows, agentquota.go:121).
pub fn next_account(m: &Model, accts: &[AcctRow], now: Ms) -> Option<(AcctRow, String)> {
    use crate::screens::list_extras::meters::MeterRow;
    let meters: Vec<MeterRow> = accts
        .iter()
        .enumerate()
        .map(|(i, r)| MeterRow {
            label: i.to_string(),
            q: r.q.clone(),
            kind: r.kind.clone(),
            name: r.name(m),
            current: r.current,
            ..MeterRow::default()
        })
        .collect();
    let (to, why) = crate::screens::list_extras::plan::next_account(&meters, now)?;
    let i: usize = to.label.parse().ok()?;
    Some((accts.get(i)?.clone(), why))
}

/// Switches each other agent to another of its accounts when the one in
/// use is nearly out, unless the default profile waits and no session a
/// limit stopped moves on; only the rush that leads does (checkLimits,
/// agentquota.go:90-114). Claude Code's logins are auto_switch's.
pub fn check_limits(m: &mut Model) -> Out {
    if !m.leads() {
        return Out::Idle;
    }
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let waits = cfg(m).default_profile().limit() == LIMIT_WAIT;
    let rows = account_rows(m);
    let mut out = Out::Idle;
    for ad in agent_order(m) {
        let k = ad.kind().0;
        if waits && !kind_limit_stopped(m, &k) {
            continue;
        }
        let at = peek(m).switched_at.get(&k).copied().unwrap_or(0);
        if k == LOGINS_KIND || !switches(m, &k) || now - at < SWITCH_GAP {
            continue;
        }
        if let Some((to, why)) = next_account(m, &accounts_of(&rows, &k), now) {
            accounts(m).switched_at.insert(k.clone(), now);
            out = switch_account(m, to.acct, &why);
        }
    }
    out
}
