//! Every saved login with its plan usage, and which one rush spends next.
//! Port of internal/fleet/logins.go, and fleet.go:199 (AccountView),
//! 1273-1308 (readUsage, freshest).

use std::collections::HashMap;

use rush_core::time::Ms;
use rush_core::usage::{self, Quota, Reading, Window};
use rush_state::{Config, Folder, Login, config::SWITCH_AT};

/// An account's folder with what its sessions spend (Go's AccountView).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountView {
    pub folder: Folder,
    /// Who it's signed in as, and its plan.
    pub usage: Reading,
    /// The plan's limits, as usage read them.
    pub quota: Quota,
    pub daemon: bool,
    pub live: usize,
    pub agents: usize,
    pub spend: f64,
    pub today: f64,
    pub current: bool,
}

/// One login with its plan usage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoginView {
    pub login: Login,
    /// Who it is, and its plan.
    pub usage: Reading,
    /// The plan's limits, as usage read them.
    pub quota: Quota,
    /// The one new sessions run as.
    pub current: bool,
}

/// Where a reading of profile dir's account is kept: by the login it's
/// signed in as, else by its folder.
pub fn reading_key(dir: &str, r: &Reading) -> String {
    if !r.account_id.is_empty() {
        return login_key(&r.account_id);
    }
    dir.to_string()
}

fn login_key(id: &str) -> String {
    Login {
        id: id.to_string(),
        ..Login::default()
    }
    .usage_key()
}

/// Prefers rush's own fetch, in fetched, when it is newer than the
/// agent's cache. Windows that have reset since either reading are
/// dropped.
pub fn freshest(
    fetched: &HashMap<String, Reading>,
    dir: &str,
    cached: Reading,
    now: Ms,
) -> Reading {
    let mut f = fetched.get(dir).cloned();
    if !cached.account_id.is_empty()
        && let Some(g) = fetched.get(&login_key(&cached.account_id))
        && f.as_ref()
            .is_none_or(|f| g.fetched_at > f.fetched_at || f.account_id != cached.account_id)
    {
        let mut g = g.clone();
        g.account_id.clone_from(&cached.account_id);
        f = Some(g);
    }
    let Some(mut f) = f else {
        return cached.since(now);
    };
    // A reading made before the folder was signed in as another account is
    // that account's, not this one's.
    let other = !f.account_id.is_empty()
        && !cached.account_id.is_empty()
        && f.account_id != cached.account_id;
    if f.fetched_at > cached.fetched_at && !other {
        f.account_id = cached.account_id;
        f.email = cached.email;
        f.org = cached.org;
        f.plan = cached.plan;
        f.role = cached.role;
        f.billing = cached.billing;
        f.org_type = cached.org_type;
        f.extra = cached.extra;
        return f.since(now);
    }
    let mut cached = cached;
    if !other {
        cached.problem = f.problem;
    }
    cached.since(now)
}

/// Every saved login with its usage; root is the agent's home, whose
/// saved reading belongs to the login it's signed in as. using is the
/// login new sessions run as, in its own home; "" for whoever the home is
/// signed in as. burns are each login's last reading a minute or more
/// old, to tell how fast it's filling.
pub fn logins(
    cfg: &Config,
    root: &AccountView,
    using: &str,
    fetched: &HashMap<String, Reading>,
    burns: &mut HashMap<String, Quota>,
    now: Ms,
) -> Vec<LoginView> {
    let mut out = Vec::new();
    for lg in &cfg.logins {
        let mut v = LoginView {
            login: lg.clone(),
            current: !lg.id.is_empty() && lg.id == root.usage.account_id,
            ..LoginView::default()
        };
        if !using.is_empty() {
            v.current = using == lg.id;
        }
        let f = fetched.get(&lg.usage_key());
        if root.usage.account_id == lg.id && f.is_none_or(|f| root.usage.fetched_at > f.fetched_at)
        {
            v.usage = root.usage.clone();
            v.usage.problem = f.map(|f| f.problem.clone()).unwrap_or_default();
        } else {
            v.usage = f.cloned().unwrap_or_default();
        }
        v.usage.email.clone_from(&lg.email);
        v.usage.org.clone_from(&lg.org);
        if root.usage.account_id == lg.id {
            v.usage.plan.clone_from(&root.usage.plan);
            v.usage.role.clone_from(&root.usage.role);
            v.usage.billing.clone_from(&root.usage.billing);
            v.usage.org_type.clone_from(&root.usage.org_type);
            v.usage.extra = root.usage.extra;
        }
        v.usage = v.usage.since(now);
        let prev = burns.get(&lg.id).cloned().unwrap_or_default();
        v.quota = usage::follow(&prev, &v.usage.quota(&lg.usage_key()));
        if v.quota.fetched_at - prev.fetched_at >= usage::EVERY.as_millis() as Ms {
            burns.insert(lg.id.clone(), v.quota.clone()); // what its next is measured from
        }
        out.push(v);
    }
    out
}

/// How many times more urgent another login must be than the one in use,
/// with room still, to move to it; and how much room it must have left.
const SWITCH_FOR: f64 = 2.0;
const MIN_ROOM: f64 = 5.0;

/// How old a reading of a login not in use may be and still be switched
/// to.
const OTHER_FOR: Ms = 3_600_000;

/// The login to switch to. Room an account doesn't use before its week
/// resets is lost, so the one to spend is whichever loses the most
/// soonest: the most urgent login that isn't nearly out. rush moves to it
/// when the one in use is nearly out, stopped says a session already hit a
/// limit on it, or another would lose far more room by waiting. Once the
/// one in use is out altogether, any login with room left will do. A
/// borrowed login is never moved to.
pub fn next_login(logins: &[LoginView], stopped: bool, now: Ms) -> Option<LoginView> {
    let mut cur: Option<&LoginView> = None;
    let mut others: Vec<&LoginView> = Vec::new();
    for l in logins {
        if l.current {
            cur = Some(l);
        } else if l.login.borrowed {
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
    others.sort_by(|a, b| {
        urgency(&b.quota, now)
            .partial_cmp(&urgency(&a.quota, now))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let nearly = cur.quota.nearly_out("", usage::LEAD, now);
    let out = stopped || cur.quota.used("") >= 100.0;
    if let Some(o) = others
        .iter()
        .find(|o| !(o.quota.used("") >= SWITCH_AT || o.quota.nearly_out("", usage::LEAD, now)))
    {
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
    let best = others.iter().min_by(|a, b| {
        a.quota
            .used("")
            .partial_cmp(&b.quota.used(""))
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    (best.quota.used("") < 100.0).then(|| (*best).clone())
}

/// How fast q must be spent, in percent an hour, not to lose any of the
/// room left in its longest window by the time it resets.
pub fn urgency(q: &Quota, now: Ms) -> f64 {
    let Some(w) = week(q) else { return 0.0 };
    (100.0 - w.percent) / (left(&w, now) as f64 / 3_600_000.0).max(0.1)
}

/// How long until w resets: all of it when it hasn't begun.
pub fn left(w: &Window, now: Ms) -> Ms {
    if w.resets_at == 0 {
        return w.span.as_millis() as Ms;
    }
    w.resets_at - now
}

/// q's longest window that limits every model: its scarce one.
pub fn week(q: &Quota) -> Option<Window> {
    let mut w: Option<&Window> = None;
    for c in &q.windows {
        if c.scope.models.is_empty() && w.is_none_or(|w| c.span > w.span) {
            w = Some(c);
        }
    }
    w.cloned()
}

/// One login in the order rush spends them, and how long until it has
/// room again when a window is full now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Link {
    pub login: LoginView,
    pub wait: Ms,
}

/// The order rush spends the logins in, as next_login picks them: the one
/// in use, then the rest by urgency, a full one last by when it has room
/// again. A login without a reading, or borrowed and not in use, is left
/// out.
pub fn chain(logins: &[LoginView], now: Ms) -> Vec<Link> {
    let (mut cur, mut rest) = (Vec::new(), Vec::new());
    for l in logins {
        if l.quota.windows.is_empty() || l.login.borrowed && !l.current {
            continue;
        }
        let mut k = Link {
            login: l.clone(),
            wait: 0,
        };
        for w in &l.quota.windows {
            if w.percent >= SWITCH_AT && w.resets_at > now {
                k.wait = k.wait.max(w.resets_at - now);
            }
        }
        if l.current {
            cur.push(k);
        } else {
            rest.push(k);
        }
    }
    rest.sort_by(|a, b| {
        urgency(&b.login.quota, now)
            .partial_cmp(&urgency(&a.login.quota, now))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.wait.cmp(&b.wait))
    });
    cur.extend(rest);
    cur
}
