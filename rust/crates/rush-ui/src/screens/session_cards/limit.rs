//! The usage-limit card: a session a provider's limit has stopped asks
//! whether to carry on at the reset. It shows the limit's window and how
//! much of it is used, a countdown to the reset, when usage was last read
//! and the account's other windows; y carries on at the reset, n waits
//! for you, and r reads usage again without answering the card or
//! retrying the turn. After Go's internal/ui/usage_limit.go
//! (limitUsageState :19, limitQuota :26, refreshLimitUsage :54, apply
//! :115, quotaWindowName :130, limitCountdown :133, limitUsageLines :148,
//! limitCardRows :205) and rushmode.go (cardRows :3103, cardKind :5957,
//! cardID :5974, cardKey's limit :6017).

use rush_core::adapter::Profile;
use rush_core::text::{Color, Line};
use rush_core::time::Ms;
use rush_core::usage::{self, Quota, Window};

use super::card::CARD_BG;
use super::{HostInfo, conn};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// The usage read again by hand, for the card alone: it never answers the
/// card, retries a turn or moves the session on (limitUsageState).
#[derive(Clone, Debug, Default)]
pub struct LimitUsage {
    /// The last good reading.
    pub reading: Quota,
    pub refreshing: bool,
    /// When a refresh was last tried, and what went wrong with it.
    pub attempted: Ms,
    pub problem: String,
}

impl LimitUsage {
    /// A refresh came back with q (apply).
    pub fn apply(&mut self, q: Quota, now: Ms, offline: bool) {
        self.refreshing = false;
        self.attempted = now;
        self.problem = q.problem.clone();
        if offline {
            self.problem = "Offline · showing the cached reading".into();
        }
        if self.problem.is_empty() && q.fetched_at == 0 {
            self.problem = "No usage reading available; the session remains paused".into();
        }
        if q.problem.is_empty() && q.fetched_at != 0 && q.fetched_at >= self.reading.fetched_at {
            self.reading = q;
        }
    }
}

/// Session key's account as its agent runs it, its kind the harness the
/// session runs in.
fn profile(m: &Model, key: &str, i: &HostInfo) -> Option<Profile> {
    let a = m.agents.iter().find(|a| a.key == key)?;
    let mut p = a.acct.clone();
    p.kind = rush_core::adapter::Kind::from(i.kind.as_str());
    Some(p)
}

/// The newest reading of session key's account: the card's own, the
/// accounts' limits and its login's (limitQuota).
pub fn limit_quota(m: &Model, key: &str, i: &HostInfo) -> Quota {
    let own = super::conn_ref(m, key)
        .map(|c| c.limit_usage.reading.clone())
        .unwrap_or_default();
    limit_quota_from(m, key, i, own)
}

/// limit_quota, the card's own reading given.
fn limit_quota_from(m: &Model, key: &str, i: &HostInfo, own: Quota) -> Quota {
    let mut q = own;
    let mut newer = |next: &Quota| {
        if q.fetched_at == 0 || next.fetched_at > q.fetched_at {
            q = next.clone();
        }
    };
    if let Some(a) = m.agents.iter().find(|a| a.key == key) {
        let acc = crate::screens::accounts::state::peek(m);
        let p = profile(m, key, i).unwrap_or_default();
        for k in [a.acct.dir.clone(), rush_host::kind::quota_key(&p)] {
            if let Some(r) = acc.quotas.get(&k) {
                newer(r);
            }
        }
        if i.kind == rush_state::LOGINS_KIND {
            for lv in acc.logins.iter().filter(|l| l.login.name == a.acct.name) {
                newer(&lv.quota);
            }
        }
    }
    q
}

/// The network as a reading asks it: not while it's down, each failure
/// said (netwatch).
struct Net;

impl usage::Network for Net {
    fn run(&self, job: &str) -> bool {
        rush_net::netwatch::run(job)
    }
    fn done(&self, job: &str, err: Option<&str>) {
        let e = err.map(std::io::Error::other);
        rush_net::netwatch::done(
            job,
            e.as_ref().map(|e| e as &(dyn std::error::Error + 'static)),
        );
    }
}

/// r on the card: session key's usage read again off the UI thread, for
/// the card to show (refreshLimitUsage).
pub fn refresh_limit_usage_on(m: &mut Model, key: &str, i: &HostInfo, c: &mut super::Conn) -> Out {
    if c.limit_usage.refreshing {
        return Out::Idle;
    }
    let now = super::now(m);
    let fail = |c: &mut super::Conn, why: &str| {
        let lu = &mut c.limit_usage;
        lu.problem = why.into();
        lu.attempted = now;
        Out::Draw
    };
    let Some(p) = profile(m, key, i) else {
        return fail(c, "The session's account is unavailable");
    };
    if p.dir.is_empty() {
        return fail(c, "The session's account location is unknown");
    }
    let Some(ad) = m.reg.get(&i.kind) else {
        return fail(c, "This harness has no usage reader");
    };
    let q = limit_quota_from(m, key, i, c.limit_usage.reading.clone());
    c.limit_usage.reading = q.clone();
    let offline = rush_net::netwatch::down();
    let read: Box<dyn FnOnce() -> Quota + Send> = if ad.plan_reader().is_some() {
        let path = rush_state::dir().join("usage.json");
        let p = p.clone();
        Box::new(move || {
            let Some(plan) = ad.plan_reader() else {
                return Quota::default();
            };
            let r = plan.refresh_plan(&path.to_string_lossy(), &p, offline);
            r.quota(&rush_fleet::reading_key(&p.dir, &r))
        })
    } else if ad.quota_source().is_some() {
        let rows = crate::screens::accounts::rows::account_rows(m);
        let account = full_account(&rows, p.kind.as_str(), &q.account);
        let path = rush_host::kind::quotas_path();
        let qkey = if ad.reads_any_account() && !account.key.is_empty() {
            account.key.clone()
        } else {
            rush_host::kind::quota_key(&p)
        };
        Box::new(move || {
            let Some(src) = ad.quota_source() else {
                return Quota::default();
            };
            usage::refresh(&path, &qkey, offline, &Net, || {
                src.quota(&p, &account).map_err(|e| e.to_string())
            })
        })
    } else {
        return fail(c, "This harness does not provide usage readings");
    };
    c.limit_usage.refreshing = true;
    let k = key.to_string();
    m.later(read, move |m, q| {
        let now = super::now(m);
        conn(m, &k).limit_usage.apply(q, now, offline);
        Out::Draw
    });
    Out::Draw
}

/// The account a reading is of, in full from the Accounts rows when it's
/// one of them (its name, home and sign-in for the provider to read
/// with), else only its kind and key (refreshLimitUsage,
/// usage_limit.go:84).
pub(crate) fn full_account(
    rows: &[crate::screens::accounts::rows::AcctRow],
    kind: &str,
    key: &str,
) -> rush_core::adapter::Account {
    rows.iter()
        .find(|r| r.kind == kind && !r.acct.key.is_empty() && r.acct.key == key)
        .map(|r| r.acct.clone())
        .unwrap_or_else(|| rush_core::adapter::Account {
            kind: kind.into(),
            key: key.into(),
            ..Default::default()
        })
}

/// A window's name as the card says it (quotaWindowName).
fn window_name(w: &Window) -> String {
    [&w.name, &w.label, &w.id]
        .into_iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "Provider quota".into())
}

/// hh:mm:ss, local.
fn clock_secs(ms: Ms) -> String {
    let s = (ms / 1000 + crate::model::local_zone(ms)).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// "Mon 15:04", local.
fn day_clock(ms: Ms) -> String {
    let local = ms / 1000 + crate::model::local_zone(ms);
    let day = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]
        [local.div_euclid(86_400).rem_euclid(7) as usize];
    let s = local.rem_euclid(86_400);
    format!("{day} {:02}:{:02}", s / 3600, s / 60 % 60)
}

/// The time left to the reset, as a clock, and when it is
/// (limitCountdown).
pub fn countdown(reset: Ms, now: Ms) -> String {
    if reset == 0 {
        return "Reset time unknown".into();
    }
    if reset <= now {
        return "Reset time passed · refresh to check usage".into();
    }
    let secs = ((reset - now + 500) / 1000).max(1);
    format!(
        "Reset in {:02}:{:02}:{:02} · {}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        day_clock(reset)
    )
}

/// What the card says of the limit, a colour each: never a fresh reading
/// inferred from a refresh, nor quota inferred free because its reset
/// time has passed (limitUsageLines).
pub fn usage_lines(m: &Model, key: &str, i: &HostInfo, now: Ms) -> Vec<(String, Color)> {
    let Some(l) = &i.limit else {
        return Vec::new();
    };
    let q = limit_quota(m, key, i);
    let mut sel = q.window(&l.window).cloned();
    if sel.is_none() && l.window.is_empty() {
        sel = q.tightest(&i.model);
    }
    let mut reset = l.resets_at;
    if let Some(s) = &sel
        && s.resets_at != 0
        && (l.window.is_empty() || s.id == l.window)
        && (reset == 0 || q.fetched_at > i.updated)
    {
        reset = s.resets_at;
    }
    let mut out = Vec::new();
    match &sel {
        Some(s) => {
            let mut n = format!("{:.0}% used", s.percent);
            if s.limit > 0.0 {
                n.push_str(&format!(" · {} / {}", g(s.used), g(s.limit)));
            }
            out.push((format!("{} · {n}", window_name(s)), TEXT));
        }
        None => out.push((
            "Provider quota reached · usage numbers unavailable".into(),
            TEXT,
        )),
    }
    out.push((countdown(reset, now), YELLOW));
    if q.fetched_at == 0 {
        out.push(("Last checked: no successful usage reading".into(), DIM));
    } else {
        let age = (now - q.fetched_at).max(0);
        out.push((
            format!(
                "Last checked {} · {} ago",
                clock_secs(q.fetched_at),
                rush_convo::render::dur(age)
            ),
            DIM,
        ));
    }
    for w in &q.windows {
        if sel.as_ref().is_some_and(|s| s.id == w.id) {
            continue;
        }
        out.push((format!("{} · {:.0}% used", window_name(w), w.percent), DIM));
    }
    let lu = super::conn_ref(m, key)
        .map(|c| c.limit_usage.clone())
        .unwrap_or_default();
    if lu.refreshing {
        out.push(("Refreshing usage…".into(), DIM));
    } else if !lu.problem.is_empty() {
        let at = if lu.attempted == 0 {
            String::new()
        } else {
            format!(" at {}", clock_secs(lu.attempted))
        };
        out.push((format!("Refresh{at}: {}", lu.problem), YELLOW));
    } else if lu.attempted != 0 {
        out.push((
            format!(
                "Refresh checked at {} · provider cache limits apply",
                clock_secs(lu.attempted)
            ),
            DIM,
        ));
    } else if !q.problem.is_empty() {
        out.push((format!("Usage status: {}", q.problem), YELLOW));
    }
    out
}

/// A number as Go's %g says it: 12, 0.5, 1e+06 (the shortest digits;
/// written with an exponent below 1e-4 or from 1e+06 up).
pub(crate) fn g(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "+Inf" } else { "-Inf" }.into();
    }
    if x == 0.0 {
        return "0".into();
    }
    let e = format!("{x:e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..6).contains(&exp) {
        return format!("{x}");
    }
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mant}e{sign}{:02}", exp.abs())
}

/// The card's rows, w wide, at most max_h (0: as many as it needs), its
/// decisions and refresh always in sight (limitCardRows).
pub fn card_rows(m: &Model, key: &str, i: &HostInfo, w: usize, max_h: usize) -> Vec<Line> {
    let now = super::now(m);
    let ww = 8.max(w.saturating_sub(4));
    let mut out: Vec<Line> = Vec::new();
    for (t, c) in usage_lines(m, key, i, now) {
        for r in wrap(&t, ww) {
            out.push(line([plain("  "), sp(r, c)]));
        }
    }
    let kw = w.saturating_sub(4);
    let pad = |l: Line| {
        let mut p = line([plain("  ")]);
        join(&mut p, l);
        p
    };
    let mut footer = vec![
        pad(fit_keys(kw, &[("y", "continue"), ("n", "wait for me")])),
        pad(fit_keys(kw, &[("r", "refresh usage"), ("esc", "later")])),
    ];
    if max_h == 0 || max_h >= 7 {
        let prompt: Vec<Line> = wrap("Continue at reset? New messages stay queued.", ww)
            .into_iter()
            .map(|r| line([plain("  "), sp(r, DIM)]))
            .collect();
        footer = prompt.into_iter().chain(footer).collect();
    }
    if max_h > 0 && out.len() + footer.len() > max_h {
        out.truncate(max_h.saturating_sub(footer.len()));
    }
    out.extend(footer);
    out.into_iter().map(|l| on_ground(l, w, CARD_BG)).collect()
}

fn fit_keys(w: usize, pairs: &[(&str, &str)]) -> Line {
    crate::screens::session_core::dock::fit_keys(w, pairs)
}
