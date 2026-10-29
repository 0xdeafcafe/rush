//! Accounts as Settings shows them: an agent's accounts on Providers and
//! Harnesses, the one in use marked, their limits, what they'd do
//! together and the order rush spends them in; an API key asked for.
//! After Go's internal/ui/settings_providers.go (useOf :191, healthLines
//! :399, accountNamed :426, accountSection :602, borrowedRow :657,
//! accountRow :669, resetsText :731, together :750, togetherRow :794,
//! chainRow :820, acctWho :858, askAPIKey :885, resetsChip :918,
//! resetCredits :927) and accounts.go (acctRow :80, limits :670, problem
//! :726, windowLines :742), view.go:468 (resetIn, roughly).
//!
//! What accounts are kept, read and switched is the accounts module's
//! (screens/accounts): it hands Settings its rows and actions through
//! [`provide`]. Until it does, the rows are rush's kept logins and
//! sign-ins with no readings, and their actions say they aren't here.

use std::rc::Rc;
use std::sync::OnceLock;

use rush_core::adapter::AccountSummary;
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_core::usage::{self, Quota, Window};

use super::common::{LOGINS_KIND, agent_name, agent_order, reg, switches};
use super::form::{Section, Setting};
use super::paint::{first_non_empty, trunc};
use super::{ask_secret, config, config_mut};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// An agent, or one of its accounts (acctRow).
#[derive(Clone, Debug, Default)]
pub struct AcctRow {
    pub kind: String,
    /// The agent's own line.
    pub head: bool,
    /// One of Claude Code's logins, by id; empty for another agent's.
    pub login: String,
    pub borrowed: bool,
    pub name: String,
    pub email: String,
    /// Who it is beyond its email: organisation, role, plan, billing.
    pub who: Vec<String>,
    pub current: bool,
    pub q: Quota,
}

impl AcctRow {
    /// Its name: the agent's for its own line (name).
    pub fn name(&self) -> String {
        if self.head {
            return agent_name(&self.kind);
        }
        self.name.clone()
    }

    /// Its email, or the reading's (email).
    pub fn email(&self) -> String {
        first_non_empty(&[&self.email, &self.q.email]).to_string()
    }

    fn is_login(&self) -> bool {
        !self.login.is_empty()
    }
}

/// One link of the order rush spends Claude Code's logins in (fleet.Link).
#[derive(Clone, Debug, Default)]
pub struct Link {
    pub name: String,
    pub current: bool,
    /// The week's use, 0–100, and how long until its room is lost.
    pub week_percent: f64,
    pub left: Ms,
    /// How long until a full one has room again; 0 when it has room.
    pub wait: Ms,
}

/// What the accounts module hands Settings. Each has a stand-in until it
/// does.
#[derive(Clone, Copy)]
pub struct Accounts {
    /// Every agent and its accounts, in the default profile's order
    /// (accountRows).
    pub rows: fn(&Model) -> Vec<AcctRow>,
    /// Why who agent k is signed in as isn't known; "" when it is.
    pub why: fn(&Model, &str) -> String,
    /// What agent k's own sign-in said of itself, when read.
    pub summary: fn(&Model, &str) -> Option<AccountSummary>,
    /// Whether an account of agent k is known to be signed in.
    pub signed_in: fn(&Model, &str) -> bool,
    /// The agent new sessions run for now, when the first is nearly out.
    pub spill: fn(&Model) -> String,
    /// The order the logins are spent in.
    pub chain: fn(&Model) -> Vec<Link>,
    /// Why an account has no fresh reading, as its row says it.
    pub problem: fn(&Model, &AcctRow) -> String,
    /// Signs agent k in to another account.
    pub add: fn(&mut Model, &str) -> Out,
    /// A key on one of Claude Code's logins: enter n r l d x b.
    pub login_key: fn(&mut Model, &AcctRow, &str) -> Out,
    /// A key on another agent's account: enter r l d x.
    pub sign_in_key: fn(&mut Model, &AcctRow, &str) -> Out,
    /// Spends one of an account's earned resets.
    pub use_reset: fn(&mut Model, &AcctRow) -> Out,
    /// Reads every provider's limits again.
    pub fetch: fn(&mut Model) -> Out,
    /// Reads who each agent is signed in as again.
    pub find_sign_ins: fn(&mut Model) -> Out,
    /// Moves new sessions on, or back, as the accounts' room says.
    pub spill_to: fn(&mut Model),
}

static PROVIDED: OnceLock<Accounts> = OnceLock::new();

/// The accounts module's part, once: later calls are ignored.
pub fn provide(a: Accounts) {
    let _ = PROVIDED.set(a);
}

/// What Settings asks about accounts: the accounts module's, else the
/// stand-ins.
pub fn accounts() -> Accounts {
    PROVIDED.get().copied().unwrap_or(STAND_IN)
}

const STAND_IN: Accounts = Accounts {
    rows: kept_rows,
    why: |_, _| String::new(),
    summary: |_, _| None,
    signed_in: |_, _| false,
    spill: |_| String::new(),
    chain: |_| Vec::new(),
    problem: |_, r| r.q.problem.clone(),
    add: not_here,
    login_key: |m, _, _| not_here(m, ""),
    sign_in_key: |m, _, _| not_here(m, ""),
    use_reset: |m, _| not_here(m, ""),
    fetch: |m| not_here(m, ""),
    find_sign_ins: |_| Out::Idle,
    spill_to: |_| {},
};

fn not_here(m: &mut Model, _: &str) -> Out {
    m.flash_err("accounts aren't switched from this rush yet");
    Out::Draw
}

/// The stand-in rows: each agent, then the accounts rush keeps for it,
/// with no readings.
fn kept_rows(m: &Model) -> Vec<AcctRow> {
    let cfg = config(m);
    let mut out = Vec::new();
    for a in agent_order(m) {
        let k = a.kind().0;
        out.push(AcctRow {
            kind: k.clone(),
            head: true,
            ..AcctRow::default()
        });
        if k == LOGINS_KIND {
            for borrowed in [false, true] {
                for l in cfg.logins.iter().filter(|l| l.borrowed == borrowed) {
                    out.push(AcctRow {
                        kind: k.clone(),
                        login: l.id.clone(),
                        borrowed,
                        name: l.name.clone(),
                        email: l.email.clone(),
                        who: vec![l.org.clone()],
                        current: !m.account.is_empty() && m.account == l.name,
                        ..AcctRow::default()
                    });
                }
            }
        } else if a.accounts().is_some() {
            for s in cfg.sign_ins_of(&k) {
                out.push(AcctRow {
                    kind: k.clone(),
                    name: s.name.clone(),
                    email: s.email.clone(),
                    who: vec![s.plan.clone()],
                    current: cfg.using.get(&k) == Some(&s.id),
                    ..AcctRow::default()
                });
            }
        }
    }
    out
}

/// The account rows of agent k (accountsOf).
pub fn accounts_of(rows: &[AcctRow], k: &str) -> Vec<AcctRow> {
    rows.iter()
        .filter(|r| r.kind == k && !r.head)
        .cloned()
        .collect()
}

/// How an agent is being used: the account it's on, and what its
/// sessions run and cost (provUse).
#[derive(Clone, Debug, Default)]
pub struct Use {
    pub in_use: AcctRow,
    pub accts: usize,
    pub out: usize,
    pub running: usize,
    pub today: usize,
    pub spent_today: f64,
    pub spent_week: f64,
}

/// Whether an account is nearly out: a window past rush's switch point.
pub fn nearly_out(q: &Quota) -> bool {
    q.windows
        .iter()
        .any(|w| w.percent >= rush_state::config::SWITCH_AT)
}

const DAY: Ms = 24 * 3_600_000;

/// Agent k's use (useOf).
pub fn use_of(m: &Model, k: &str) -> Use {
    let mut u = Use::default();
    let mut found = false;
    for r in (accounts().rows)(m) {
        if r.kind != k {
            continue;
        }
        if r.head {
            if !found {
                u.in_use = r;
                found = true;
            }
            continue;
        }
        u.accts += 1;
        if nearly_out(&r.q) {
            u.out += 1;
        }
        if r.current {
            u.in_use = r;
            found = true;
        }
    }
    let now = m.now;
    for a in &m.agents {
        if a.kind.as_str() != k {
            continue;
        }
        if a.job.live() {
            u.running += 1;
        }
        if a.spend.today > 0.0 || same_day(a.spend.last, now) {
            u.today += 1;
        }
        u.spent_today += a.spend.today;
        if now - a.spend.last < 7 * DAY {
            u.spent_week += a.spend.cost;
        }
    }
    u
}

fn same_day(a: Ms, b: Ms) -> bool {
    let day = |t: Ms| (t + crate::model::local_zone(t) * 1000).div_euclid(DAY);
    a > 0 && day(a) == day(b)
}

/// A duration to one unit: 40m, 3h, 5d (roughly).
pub fn roughly(d: Ms) -> String {
    const MIN: f64 = 60_000.0;
    let d = d as f64;
    if d < 60.0 * MIN {
        format!("{}m", ((d / MIN).round() as i64).max(1))
    } else if d < 48.0 * 60.0 * MIN {
        format!("{}h", (d / (60.0 * MIN)).round() as i64)
    } else {
        format!("{}d", (d / (24.0 * 60.0 * MIN)).round() as i64)
    }
}

/// When a window resets, from now (resetIn).
pub fn reset_in(at: Ms, now: Ms) -> Line {
    if at == 0 {
        return Line::new();
    }
    if at <= now {
        return line([sp(" reset", FAINT)]);
    }
    line([sp(format!(" ↻{}", roughly(at - now)), FAINT)])
}

/// A time of day, or a weekday and time when it's past tomorrow: "15:04",
/// "Mon 15:04".
pub fn clock(at: Ms, now: Ms, week: bool) -> String {
    let local = at / 1000 + crate::model::local_zone(at);
    let hm = format!(
        "{:02}:{:02}",
        local.rem_euclid(86_400) / 3600,
        local.rem_euclid(3600) / 60
    );
    if week || at - now > 20 * 3_600_000 {
        const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
        let wd = DAYS[local.div_euclid(86_400).rem_euclid(7) as usize];
        return format!("{wd} {hm}");
    }
    hm
}

/// An account's first two windows, each a labelled meter, or its
/// balance, or why there are none (limits).
pub fn limits(m: &Model, r: &AcctRow, w1: usize, w2: usize) -> Line {
    let mut q = r.q.clone();
    q.problem = (accounts().problem)(m, r);
    if q.windows.is_empty() {
        let msg = if !q.balance.is_empty() {
            if q.problem.is_empty() {
                line([sp(q.balance.clone(), TEXT)])
            } else {
                line([sp(format!("{} · {}", q.balance, q.problem), YELLOW)])
            }
        } else if !q.problem.is_empty() {
            line([sp(q.problem.clone(), YELLOW)])
        } else if r.kind != LOGINS_KIND {
            if reg()
                .get(&r.kind)
                .is_none_or(|a| a.quota_source().is_none())
            {
                line([sp("doesn't report its limits", FAINT)])
            } else if q.fetched_at == 0 && r.current {
                line([sp("fetching…", FAINT)])
            } else if q.fetched_at == 0 {
                line([sp("read while it's in use", FAINT)])
            } else {
                line([sp("no reading", FAINT)])
            }
        } else if q.fetched_at == 0 {
            line([sp("fetching…", FAINT)])
        } else {
            line([sp("no reading", FAINT)])
        };
        return fit(msg, w1 + w2);
    }
    let now = m.now;
    if now - q.fetched_at > 3 * usage::EVERY.as_millis() as Ms {
        let at = clock(q.fetched_at, q.fetched_at, false);
        let why = if q.problem.is_empty() {
            format!("not read since {at}")
        } else {
            format!("{} · last read {at}", q.problem)
        };
        return fit(line([sp("! ", YELLOW), sp(why, DIM)]), w1 + w2);
    }
    let mut out = Line::new();
    for (i, cw) in [w1, w2].into_iter().enumerate() {
        let Some(win) = q.windows.get(i) else {
            join(&mut out, fit(Line::new(), cw));
            continue;
        };
        let mut cell = line([sp(fit_str(&win.label, 3), FAINT)]);
        join(&mut cell, bar(win.percent));
        cell.spans.push(plain(" "));
        cell.spans.push(sp(format!("{:3.0}%", win.percent), TEXT));
        join(&mut cell, reset_in(win.resets_at, now));
        join(&mut out, fit(cell, cw));
    }
    out
}

/// A reading's windows, each with when it resets (windowLines).
pub fn window_lines(q: &Quota, now: Ms, label: &dyn Fn(&str) -> Line) -> Vec<Line> {
    let mut out = Vec::new();
    for win in &q.windows {
        let mut s = label(&win.label);
        join(&mut s, bar(win.percent));
        s.spans.push(plain(" "));
        s.spans.push(sp(format!("{:.0}%", win.percent), TEXT));
        if win.resets_at != 0 {
            let when = clock(win.resets_at, now, false);
            if win.resets_at > now {
                s.spans.push(sp(
                    format!("  resets {when} · in {}", dur(win.resets_at - now)),
                    DIM,
                ));
            } else {
                s.spans
                    .push(sp(format!("  reset at {when}, since this reading"), FAINT));
            }
        }
        out.push(s);
    }
    if !q.balance.is_empty() {
        let mut l = label("balance");
        l.spans.push(sp(q.balance.clone(), TEXT));
        out.push(l);
    }
    if q.windows.is_empty() && !q.problem.is_empty() {
        let mut l = label("limits");
        l.spans.push(sp(q.problem.clone(), YELLOW));
        out.push(l);
    }
    if q.fetched_at != 0 {
        let source = match q.source {
            usage::Source::Fetched => "fetched",
            usage::Source::Live => "reported by a session",
            _ => "read",
        };
        let mut l = label("usage");
        l.spans.push(sp(
            format!("{source} at {}", clock(q.fetched_at, q.fetched_at, false)),
            FAINT,
        ));
        out.push(l);
    }
    out
}

/// Whether a reading failed on an expired sign-in (expired).
pub fn expired(q: &Quota) -> bool {
    q.problem.contains(usage::EXPIRED)
}

/// Harness k's logins as every rush session saw their turns end: a line
/// for each one failing, else one saying when it last answered
/// (healthLines).
pub fn health_lines(m: &Model, k: &str, label: &dyn Fn(&str) -> Line) -> Vec<Line> {
    use rush_state::health;
    let harness = reg().current_kind(k).0;
    let all = health::load();
    let mut keys: Vec<&String> = all.keys().collect();
    keys.sort();
    let mut out = Vec::new();
    let mut answered: Ms = 0;
    for key in keys {
        let (h, acct) = health::split(key);
        if h != harness {
            continue;
        }
        let e = &all[key];
        match e.state() {
            health::State::Failing => {
                let mut l = label("health");
                l.spans.push(sp(format!("✗ {}", e.error), RED));
                l.spans.push(sp(
                    format!(
                        " · {} · failing {}",
                        account_named(m, acct),
                        age(m.now - e.failed.to_ms())
                    ),
                    DIM,
                ));
                out.push(l);
            }
            health::State::Ok => answered = answered.max(e.ok.to_ms()),
            _ => {}
        }
    }
    if out.is_empty() && answered > 0 {
        let mut l = label("health");
        l.spans.push(sp("✓ answering", GREEN));
        l.spans.push(sp(
            format!(" · last answer {} ago", age(m.now - answered)),
            FAINT,
        ));
        out.push(l);
    }
    out
}

/// A health record's account as you know it: a login's name, by its
/// uuid, else the profile it names (accountNamed).
fn account_named(m: &Model, acct: &str) -> String {
    config(m)
        .logins
        .iter()
        .find(|l| l.id == acct)
        .map(|l| l.name.clone())
        .unwrap_or_else(|| acct.to_string())
}

/// When each of a reading's windows resets, by the clock and how long
/// from now (resetsText).
pub fn resets_text(q: &Quota, now: Ms) -> String {
    let mut out = Vec::new();
    for win in &q.windows {
        if win.resets_at <= now {
            continue;
        }
        out.push(format!(
            "{} resets {}, in {}",
            win.label,
            clock(win.resets_at, now, false),
            roughly(win.resets_at - now)
        ));
    }
    out.join(" · ")
}

/// An agent's accounts as one: each window's use averaged over the
/// accounts with a reading of it, resetting when the first of them does;
/// and how many accounts had a reading (together).
pub fn together(rows: &[AcctRow]) -> (Quota, usize) {
    let mut all = Quota::default();
    let mut count: std::collections::HashMap<String, f64> = Default::default();
    let mut n = 0;
    for r in rows {
        if r.q.windows.is_empty() {
            continue;
        }
        n += 1;
        all.fetched_at = all.fetched_at.max(r.q.fetched_at);
        for win in &r.q.windows {
            let i = match all.windows.iter().position(|w| w.label == win.label) {
                Some(i) => i,
                None => {
                    all.windows.push(Window {
                        label: win.label.clone(),
                        name: win.name.clone(),
                        resets_at: win.resets_at,
                        ..Window::default()
                    });
                    all.windows.len() - 1
                }
            };
            let a = &mut all.windows[i];
            a.percent += win.percent;
            if a.resets_at == 0 || win.resets_at != 0 && win.resets_at < a.resets_at {
                a.resets_at = win.resets_at;
            }
            *count.entry(win.label.clone()).or_default() += 1.0;
        }
    }
    for w in &mut all.windows {
        w.percent /= count[&w.label];
    }
    for r in rows {
        if let Some(rs) = &r.q.resets {
            all.resets.get_or_insert_with(Default::default).available += rs.available;
        }
    }
    (all, n)
}

/// How many limit resets an account has earned, when any (resetsChip).
pub fn resets_chip(q: &Quota) -> Line {
    match &q.resets {
        Some(rs) if rs.available > 0 => {
            line([plain("  "), sp(format!("↺ {}", rs.available), GREEN)])
        }
        _ => Line::new(),
    }
}

fn plural(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// An account's earned resets, each with what it does and when it runs
/// out, for its about: "" when it has none (resetCredits).
pub fn reset_credits(q: &Quota, now: Ms) -> String {
    let Some(rs) = q.resets.as_ref().filter(|r| r.available > 0) else {
        return String::new();
    };
    let mut b = format!(
        "\n↺ {} limit reset{} earned: each resets its limits at once · u uses one",
        rs.available,
        plural(rs.available)
    );
    for c in &rs.credits {
        b.push_str(&format!(
            "\n  • {}",
            first_non_empty(&[&c.title, "a limit reset"])
        ));
        if !c.about.is_empty() {
            b.push_str(&format!(": {}", c.about));
        }
        if c.expires != 0 {
            b.push_str(&format!(
                " · runs out {}, in {}",
                day_month(c.expires),
                roughly(c.expires - now)
            ));
        }
    }
    b
}

/// "Mon 2 Jan".
fn day_month(at: Ms) -> String {
    let local = at / 1000 + crate::model::local_zone(at);
    let days = local.div_euclid(86_400);
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Civil from days (Howard Hinnant's).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    format!(
        "{} {d} {}",
        DAYS[days.rem_euclid(7) as usize],
        MONTHS[(mo - 1) as usize]
    )
}

/// Who an account is: its email, organisation, role and plan, as far as
/// they're known (acctWho).
pub fn acct_who(r: &AcctRow) -> String {
    let mut who: Vec<String> = Vec::new();
    let mut add = |v: &str| {
        let v = v.replace('_', " ");
        if !v.is_empty() && !who.contains(&v) {
            who.push(v);
        }
    };
    add(&r.email());
    if r.is_login() {
        for v in &r.who {
            add(v);
        }
    } else {
        add(first_non_empty(&[
            &r.q.plan,
            r.who.first().map(String::as_str).unwrap_or(""),
        ]));
    }
    if who.is_empty() {
        return "Who it is isn't known yet.".into();
    }
    who.join(" · ")
}

/// Agent k's accounts, the one in use marked; none when rush can't
/// switch it (accountSection).
pub fn account_section(m: &Model, k: &str) -> Option<Section> {
    if !switches(k) {
        return None;
    }
    let a = accounts();
    let mut sec = Section {
        title: "Account".into(),
        note: "one at a time, shared by all its sessions".into(),
        ..Section::default()
    };
    let rows = accounts_of(&(a.rows)(m), k);
    let why = (a.why)(m, k);
    if !why.is_empty() {
        // Signed in as no one: its sessions fail until it is again.
        let (k1, k2, k3) = (k.to_string(), k.to_string(), k.to_string());
        sec.rows.push(Setting {
            label: "not signed in".into(),
            line: Some(Rc::new(move |_| {
                line([
                    sp(format!("✗ {} is signed out", agent_name(&k1)), RED),
                    sp(" · l signs it in", DIM),
                ])
            })),
            key: Some(Rc::new(move |m, s| {
                if s == "l" || s == "enter" {
                    (accounts().add)(m, &k2)
                } else {
                    Out::Pass
                }
            })),
            keys: vec!["l".into(), "sign in again".into()],
            about: Some(Rc::new(move || {
                (
                    "Not signed in".into(),
                    format!(
                        "{} isn't signed in, so its sessions fail to start or stop at their first request.",
                        agent_name(&k3)
                    ),
                    why.clone(),
                )
            })),
            ..Setting::default()
        });
    }
    for (i, r) in rows.iter().enumerate() {
        if r.is_login() && r.borrowed && (i == 0 || !rows[i - 1].borrowed) {
            sec.rows.push(borrowed_row());
        }
        sec.rows.push(account_row(m, r.clone()));
    }
    let mine: Vec<AcctRow> = rows
        .iter()
        .filter(|r| !(r.is_login() && r.borrowed))
        .cloned()
        .collect();
    let (all, n) = together(&mine);
    if n > 1 {
        sec.rows.push(together_row(m, all, n));
    }
    let chain = (a.chain)(m);
    if k == LOGINS_KIND && chain.len() > 1 {
        sec.rows.push(chain_row(chain));
    }
    let (k1, k2) = (k.to_string(), k.to_string());
    sec.rows.push(Setting {
        label: "+ add an account".into(),
        line: Some(Rc::new(|_| line([sp("+ add an account", FAINT)]))),
        key: Some(Rc::new(move |m, s| {
            if matches!(s, "enter" | "right" | "a") {
                (accounts().add)(m, &k1)
            } else {
                Out::Pass
            }
        })),
        keys: vec!["enter".into(), "sign in".into()],
        about: Some(Rc::new(move || {
            (
                "Add an account".into(),
                format!(
                    "Signs {} in to another account, for rush to keep and switch to when one runs low.",
                    agent_name(&k2)
                ),
                String::new(),
            )
        })),
        ..Setting::default()
    });
    Some(sec)
}

/// Heads the accounts rush never switches to on its own (borrowedRow).
fn borrowed_row() -> Setting {
    Setting {
        label: "borrowed".into(),
        line: Some(Rc::new(|_| {
            line([sp("── borrowed · used only when you pick one", FAINT)])
        })),
        key: Some(Rc::new(|_, _| Out::Pass)),
        about: Some(Rc::new(|| {
            (
                "Borrowed".into(),
                "Team or someone else's accounts. rush never switches to one on its own, and while you're on one it only moves you off when it runs low. b on an account moves it here or back.".into(),
                String::new(),
            )
        })),
        ..Setting::default()
    }
}

/// One of an agent's accounts: enter switches to it (accountRow).
fn account_row(m: &Model, r: AcctRow) -> Setting {
    let mut keys: Vec<String> = [
        "enter",
        "switch now",
        "a",
        "add",
        "r",
        "rename",
        "l",
        "sign in again",
        "d",
        "forget",
    ]
    .map(String::from)
    .to_vec();
    if r.is_login() && expired(&r.q) {
        keys[5] = "refresh the sign-in".into();
    }
    if r.q.resets.as_ref().is_some_and(|rs| rs.available > 0) {
        keys.extend(["u".into(), "use a reset".into()]);
    }
    if r.is_login() {
        keys.extend([
            "n".into(),
            "switch new sessions only".into(),
            "b".into(),
            if r.borrowed {
                "mark yours"
            } else {
                "mark borrowed"
            }
            .into(),
        ]);
    }
    let a = accounts();
    let signed_out = !(a.why)(m, &r.kind).is_empty() && !r.current;
    let use_line = {
        let mut l = limits(m, &r, 26, 26);
        join(&mut l, resets_chip(&r.q));
        if signed_out {
            // Its usage reads stale only because the agent is signed out.
            l = line([
                sp("! ", YELLOW),
                sp("signed out · l signs back in to it", DIM),
            ]);
        }
        l
    };
    let now = m.now;
    let (rl, rk, ra) = (r.clone(), r.clone(), r.clone());
    Setting {
        label: r.name(),
        line: Some(Rc::new(move |w| {
            let r = &rl;
            let mark = if r.current {
                sp("● ", ORANGE)
            } else if r.is_login() && r.borrowed {
                sp("◌ ", FAINT)
            } else {
                sp("○ ", FAINT)
            };
            let mut l = line([
                mark,
                sp(fit_str(&r.name(), 16), TEXT),
                sp(fit_str(&r.email(), 28.min(w.saturating_sub(72))), DIM),
            ]);
            join(&mut l, use_line.clone());
            l
        })),
        key: Some(Rc::new(move |m, s| {
            let r = &rk;
            let a = accounts();
            match s {
                "u" => (a.use_reset)(m, r),
                "a" => (a.add)(m, &r.kind),
                "enter" | "n" | "r" | "l" | "d" | "x" | "b" if r.is_login() => {
                    (a.login_key)(m, r, s)
                }
                "enter" | "r" | "l" | "d" | "x" if !r.is_login() => (a.sign_in_key)(m, r, s),
                _ => Out::Pass,
            }
        })),
        keys,
        about: Some(Rc::new(move || {
            let r = &ra;
            let mut now_s = if r.current {
                format!("In use: new {} sessions run on it.", agent_name(&r.kind))
            } else if r.is_login() && r.borrowed {
                "Borrowed: rush never switches to it on its own, only when you press enter or n. b makes it yours again.".into()
            } else {
                "Kept by rush: enter switches to it now, n for new sessions only (running ones follow once their cache cools).".to_string()
            };
            let resets = resets_text(&r.q, now);
            if !resets.is_empty() {
                now_s.push('\n');
                now_s.push_str(&resets);
            }
            now_s.push_str(&reset_credits(&r.q, now));
            (r.name(), acct_who(r), now_s)
        })),
        ..Setting::default()
    }
}

/// An agent's accounts added up: how much room they have between them
/// before rush has nowhere left to switch to (togetherRow).
fn together_row(m: &Model, all: Quota, n: usize) -> Setting {
    let row = AcctRow {
        q: all.clone(),
        ..AcctRow::default()
    };
    let mut meters = limits(m, &row, 26, 26);
    join(&mut meters, resets_chip(&all));
    let now = m.now;
    Setting {
        label: "together".into(),
        line: Some(Rc::new(move |w| {
            let mut l = line([
                sp("Σ ", FAINT),
                sp(fit_str("together", 16), SUB),
                sp(
                    fit_str(&format!("{n} accounts"), 28.min(w.saturating_sub(72))),
                    DIM,
                ),
            ]);
            join(&mut l, meters.clone());
            l
        })),
        key: Some(Rc::new(|_, _| Out::Pass)),
        about: Some(Rc::new(move || {
            let left: Vec<String> = all
                .windows
                .iter()
                .map(|w| {
                    format!(
                        "{}: {:.1} accounts' worth left",
                        w.label,
                        n as f64 * (100.0 - w.percent) / 100.0
                    )
                })
                .collect();
            let mut now_s = left.join(" · ");
            let resets = resets_text(&all, now);
            if !resets.is_empty() {
                now_s.push_str(&format!("\nSoonest: {resets}"));
            }
            if let Some(rs) = all.resets.as_ref().filter(|r| r.available > 0) {
                now_s.push_str(&format!(
                    "\n↺ {} limit reset{} earned between them: open an account to use one",
                    rs.available,
                    plural(rs.available)
                ));
            }
            (
                "All accounts together".into(),
                format!(
                    "Each limit's use averaged over the {n} accounts with a reading, so 50% means half their combined room is left. It resets when the first of them does."
                ),
                now_s,
            )
        })),
        ..Setting::default()
    }
}

/// The order rush spends the logins in: each with its week's room and
/// when that's lost, or how long until it has room again (chainRow).
fn chain_row(chain: Vec<Link>) -> Setting {
    fn node(k: &Link, plain_name: bool) -> Line {
        let left = format!("{:.0}% · {}", 100.0 - k.week_percent, roughly(k.left));
        let name = if plain_name {
            plain(k.name.clone())
        } else if k.current {
            sp(k.name.clone(), ORANGE)
        } else {
            sp(k.name.clone(), TEXT)
        };
        if k.wait > 0 {
            return line([
                sp(format!("⏸{} ", roughly(k.wait)), DIM),
                name,
                plain(" "),
                sp(left, FAINT),
            ]);
        }
        line([name, plain(" "), sp(left, DIM)])
    }
    let c2 = chain.clone();
    Setting {
        label: "switch order".into(),
        line: Some(Rc::new(move |w| {
            let mut parts = Line::new();
            for (i, k) in chain.iter().enumerate() {
                if i > 0 {
                    parts.spans.push(sp(" ▸ ", FAINT));
                }
                join(&mut parts, node(k, false));
            }
            let mut l = line([sp("⇢ ", FAINT), sp(fit_str("switch order", 16), SUB)]);
            join(&mut l, fit(parts, w.saturating_sub(18)));
            l
        })),
        key: Some(Rc::new(|_, _| Out::Pass)),
        about: Some(Rc::new(move || {
            let lines: Vec<String> = c2
                .iter()
                .enumerate()
                .map(|(i, k)| format!("{}. {}", i + 1, node(k, true).plain()))
                .collect();
            (
                "Switch order".into(),
                "Room an account doesn't use before its week resets is lost, so rush spends first whichever would lose the most soonest: its week's room left (%) over the time until it resets. It moves on when the one in use is nearly out, or early when another would lose twice as fast. ⏸ is how long until a full one has room again.".into(),
                lines.join("\n"),
            )
        })),
        ..Setting::default()
    }
}

/// Asks for provider p's API key, which pays for its models per token in
/// any harness that speaks its API; "none" forgets it (askAPIKey).
pub fn ask_api_key(m: &mut Model, p: &str) -> Out {
    let name = reg().provider_label(p);
    if reg().key_env(p).is_empty() {
        m.flash_err(&format!("{name} takes no API key"));
        return Out::Draw;
    }
    let mut what = format!("{name} API key");
    if config(m).has_api_key(p) {
        what.push_str(" (none forgets it)");
    }
    let p = p.to_string();
    ask_secret(
        m,
        &what,
        Rc::new(move |m: &mut Model, v: &str| {
            let v = if v == "none" { "" } else { v }.to_string();
            let (p, name) = (p.clone(), name.clone());
            let put = (p.clone(), v.clone());
            m.later(
                move || rush_state::vault::put_api_key(&put.0, &put.1).map_err(|e| e.to_string()),
                move |m, r| {
                    if let Err(e) = r {
                        m.flash_err(&e);
                        return Out::Draw;
                    }
                    config_mut(m).mark_api_key(&p, !v.is_empty());
                    super::changed(m);
                    if v.is_empty() {
                        m.flash(&format!("{name}'s API key is forgotten"));
                    } else {
                        m.flash(&format!(
                            "{name}'s API key is kept in the keychain · #use picks it per session"
                        ));
                    }
                    Out::Draw
                },
            );
            Out::Draw
        }),
    );
    Out::Draw
}

/// What every provider's spend and sessions sum to, and which provider
/// new sessions run for now, when it isn't the first (provTotals).
pub fn totals(m: &Model) -> Line {
    let mut u = Use::default();
    for a in agent_order(m) {
        let v = use_of(m, a.kind().as_str());
        u.running += v.running;
        u.today += v.today;
        u.spent_today += v.spent_today;
        u.spent_week += v.spent_week;
    }
    let mut l = line([
        sp(money(u.spent_today), TEXT),
        sp(" today", DIM),
        sp(" · ", FAINT),
        sp(money(u.spent_week), TEXT),
        sp(" this week", DIM),
        sp(" · ", FAINT),
        sp(u.running.to_string(), TEXT),
        sp(" running", DIM),
        sp(" · ", FAINT),
        sp(u.today.to_string(), TEXT),
        sp(" sessions today", DIM),
    ]);
    let spill = (accounts().spill)(m);
    if !spill.is_empty() {
        l.spans.extend([
            sp("   ·   ", FAINT),
            sp("→ ", YELLOW),
            sp("new sessions run ", DIM),
            sp(agent_name(&spill), TEXT),
            sp(" for now: the first's accounts are nearly out", DIM),
        ]);
    }
    trunc(l, usize::MAX)
}
