//! Every installed agent (a provider: Claude Code, Codex, Copilot…) can
//! be signed in as one account at a time. Every agent runs from its own
//! home (~/.claude, ~/.codex…); an account is a sign-in rush puts in that
//! home when you switch to it. Settings › Providers shows them, a row an
//! agent or one of its accounts, each with its limits. After Go's
//! internal/ui/accounts.go (acctRow :76, agentOrder :111, profileOf :136,
//! acctKey :150, signInAccount :158, accountRows :183, switches :233,
//! accountsOf :243, startKind :256, startAccount :286, startQuota :302,
//! limits :671, expired :720, problem :726, windowLines :742),
//! profiles.go (room :21, id :47, startProfile :55, startPick :61,
//! profileWords :114, limitWords :131), logins.go (inUse :184, inUseOf
//! :195, loginNamed :174, loginName :731, currentLoginName :740),
//! providers.go:173 (accountOf), usagebar.go:107 (label),
//! agentquota.go:159 (nearlyOut), usage_limit.go:129 (quotaWindowName) and
//! view.go (resetIn :468, roughly :479).

use rush_core::adapter::{Account, AdapterRef, Profile as AgentProfile};
use rush_core::text::{Line, Span};
use rush_core::time::Ms;
use rush_core::usage::{self, Quota, Source, Window};
use rush_state::{Pick, Profile, Room, Seat, SignIn};

use super::look::{LOGINS_KIND, agent_name};
use super::setup::setup_label;
use super::state::{LoginView, cfg, peek};
use crate::model::Model;
use crate::style::*;

/// An agent, or one of its accounts (acctRow).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AcctRow {
    pub kind: String,
    /// The agent's own line.
    pub head: bool,
    /// One of Claude Code's logins.
    pub login: Option<LoginView>,
    /// Another agent's account.
    pub acct: Account,
    pub current: bool,
    pub q: Quota,
}

impl AcctRow {
    pub fn name(&self, m: &Model) -> String {
        if self.head {
            return agent_name(&m.reg, &self.kind);
        }
        match &self.login {
            Some(l) => l.name.clone(),
            None => self.acct.name.clone(),
        }
    }

    pub fn email(&self) -> String {
        match &self.login {
            Some(l) => l.email.clone(),
            None if !self.acct.email.is_empty() => self.acct.email.clone(),
            None => self.q.email.clone(),
        }
    }

    /// The account's own id: a login's uuid, or another agent's.
    pub fn id(&self) -> &str {
        match &self.login {
            Some(l) => &l.id,
            None => &self.acct.id,
        }
    }

    /// The account as rush names it: harness:account, or the harness alone
    /// for one with a single sign-in of its own (label).
    pub fn label(&self, m: &Model) -> String {
        if self.head {
            return setup_label(&m.reg, &self.kind, "", "");
        }
        setup_label(&m.reg, &self.kind, "", &self.name(m))
    }
}

/// Where an account's limits are kept (acctKey).
pub fn acct_key(kind: &str, id: &str) -> String {
    if kind == "copilot" {
        return format!("copilot:gh:{id}");
    }
    format!("{kind}:{id}")
}

/// A kept account as the agent's (signInAccount).
pub fn sign_in_account(s: &SignIn) -> Account {
    Account {
        kind: s.kind.as_str().into(),
        id: s.id.clone(),
        key: acct_key(&s.kind, &s.id),
        name: s.name.clone(),
        email: s.email.clone(),
        plan: s.plan.clone(),
        ..Account::default()
    }
}

/// Every installed agent, in the default profile's order, which is the
/// order new sessions move on through when an agent's accounts are all
/// nearly out. Those it doesn't list follow: Claude Code, then by name
/// (agentOrder).
pub fn agent_order(m: &Model) -> Vec<AdapterRef> {
    let mut by: Vec<AdapterRef> = m
        .reg
        .installed_all()
        .into_iter()
        .filter(|a| m.reg.current_kind(a.kind().as_str()) == a.kind())
        .collect();
    let mut out = Vec::new();
    for k in cfg(m).default_profile().kinds() {
        if let Some(i) = by.iter().position(|a| a.kind().as_str() == k) {
            out.push(by.remove(i));
        }
    }
    by.sort_by_key(|a| a.kind().as_str() != LOGINS_KIND);
    out.extend(by);
    out
}

/// The home an agent runs from (profileOf).
pub fn profile_of(m: &Model, a: &AdapterRef) -> Option<AgentProfile> {
    if a.kind().as_str() == LOGINS_KIND {
        let acct = cfg(m).active_account();
        return Some(AgentProfile {
            kind: LOGINS_KIND.into(),
            name: acct.name,
            dir: acct.config_dir,
        });
    }
    m.reg.profiles_of(a).into_iter().next()
}

/// Whether rush can switch agent k between accounts (switches).
pub fn switches(m: &Model, k: &str) -> bool {
    k == LOGINS_KIND || m.reg.get(k).is_some_and(|a| a.accounts().is_some())
}

/// Each installed agent, then its accounts (accountRows).
pub fn account_rows(m: &Model) -> Vec<AcctRow> {
    let st = peek(m);
    let mut out = Vec::new();
    for ad in agent_order(m) {
        let k = ad.kind().0;
        let mut head = AcctRow {
            kind: k.clone(),
            head: true,
            ..AcctRow::default()
        };
        if k == LOGINS_KIND {
            if let Some(l) = st.logins.iter().find(|l| l.current) {
                head.q = l.quota.clone();
            }
            out.push(head);
            for borrowed in [false, true] {
                // borrowed ones last
                for lv in st.logins.iter().filter(|l| l.borrowed == borrowed) {
                    out.push(AcctRow {
                        kind: k.clone(),
                        login: Some(lv.clone()),
                        current: lv.current,
                        q: lv.quota.clone(),
                        ..AcctRow::default()
                    });
                }
            }
        } else if ad.accounts().is_some() {
            let p = profile_of(m, &ad).unwrap_or_default();
            out.push(head);
            for s in cfg(m).sign_ins_of(&k) {
                let a = sign_in_account(&s);
                let mut r = AcctRow {
                    kind: k.clone(),
                    current: st.now.get(&k).is_some_and(|id| *id == s.id),
                    q: st.quotas.get(&a.key).cloned().unwrap_or_default(),
                    acct: a,
                    ..AcctRow::default()
                };
                if let Some(pq) = st.quotas.get(&p.dir)
                    && r.current
                    && (pq.account.is_empty() || pq.account == r.acct.key)
                    && pq.fetched_at >= r.q.fetched_at
                {
                    r.q = pq.clone();
                }
                out.push(r);
            }
        } else {
            // An agent rush can't switch: its one sign-in is its own.
            if let Some(p) = profile_of(m, &ad) {
                head.q = st.quotas.get(&p.dir).cloned().unwrap_or_default();
            }
            out.push(head);
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

/// How old a reading of the account in use may be and still say it's
/// nearly out (fresh).
pub const FRESH: Ms = 15 * 60_000;

fn now(m: &Model) -> Ms {
    m.clock.unwrap_or_else(rush_core::time::now)
}

/// Whether a fresh reading says it's nearly out (nearlyOut).
pub fn nearly_out(q: &Quota, now: Ms) -> bool {
    !q.windows.is_empty() && now - q.fetched_at < FRESH && q.nearly_out("", usage::LEAD, now)
}

/// What rush knows of each installed provider's accounts: which is in
/// use, and which are nearly out (room).
pub fn room(m: &Model) -> Room {
    let now = now(m);
    let rows = account_rows(m);
    let mut room = Room::new();
    for r in rows.iter().filter(|r| !r.head) {
        room.entry(r.kind.clone()).or_default().push(Seat {
            id: r.id().to_string(),
            name: r.name(m),
            current: r.current,
            out: nearly_out(&r.q, now),
            used: r.q.used(""),
        });
    }
    // A provider with no accounts of its own kept: its one sign-in.
    for r in rows.iter().filter(|r| r.head) {
        if room.get(&r.kind).is_none_or(|s| s.is_empty()) && !r.q.windows.is_empty() {
            room.insert(
                r.kind.clone(),
                vec![Seat {
                    name: r.name(m),
                    current: true,
                    out: nearly_out(&r.q, now),
                    used: r.q.used(""),
                    ..Seat::default()
                }],
            );
        }
    }
    room
}

/// The folder new sessions start in (startDir): where rush was opened.
pub fn start_dir(m: &Model) -> String {
    m.launch_dir.clone()
}

/// The profile a session started in dir gets (startProfile).
pub fn start_profile(m: &Model, dir: &str) -> Profile {
    cfg(m).profile_for(dir, &peek(m).profile)
}

/// The provider and account a session started in dir runs on, as its
/// profile picks (startPick).
pub fn start_pick(m: &Model, dir: &str) -> Option<Pick> {
    start_profile(m, dir).pick(&room(m))
}

/// The agent a new session in dir runs: its profile's first, unless every
/// account of it is nearly out and the profile moves on (startKindIn).
pub fn start_kind_in(m: &Model, dir: &str) -> String {
    let mut k = cfg(m).default_agent();
    if let Some(p) = start_pick(m, dir) {
        k = p.kind;
    }
    m.reg.current_kind(&k).0
}

/// startKind: the agent new sessions from the box run.
pub fn start_kind(m: &Model) -> String {
    start_kind_in(m, &start_dir(m))
}

/// The name of the login ~/.claude is signed in as, or the folder's name
/// before rush knows which it is (inUse).
pub fn in_use(m: &Model) -> String {
    if let Some(l) = peek(m)
        .logins
        .iter()
        .find(|l| l.current && !l.name.is_empty())
    {
        return l.name.clone();
    }
    cfg(m).active_account().name
}

/// The name of the account agent k is signed in as (inUseOf).
pub fn in_use_of(m: &Model, k: &str) -> String {
    if k == LOGINS_KIND {
        return in_use(m);
    }
    let now = peek(m).now.get(k);
    cfg(m)
        .sign_ins_of(k)
        .into_iter()
        .find(|s| now == Some(&s.id))
        .map(|s| s.name)
        .unwrap_or_default()
}

/// The account provider k is signed in as, when rush keeps more than the
/// one (accountOf).
pub fn account_of(m: &Model, k: &str) -> String {
    if k != LOGINS_KIND {
        return in_use_of(m, k);
    }
    peek(m)
        .logins
        .iter()
        .find(|l| l.current)
        .map(|l| l.name.clone())
        .unwrap_or_default()
}

/// The agent and account new sessions start on, for the top bar
/// (startAccount).
pub fn start_account(m: &Model) -> String {
    let k = start_kind(m);
    if k == LOGINS_KIND {
        return in_use(m);
    }
    let name = agent_name(&m.reg, &k);
    let now = peek(m).now.get(&k);
    for s in cfg(m).sign_ins_of(&k) {
        if now == Some(&s.id) {
            return format!("{name} · {}", s.name);
        }
    }
    name
}

/// The limits of the account new sessions start on, when they run an
/// agent other than Claude Code (startQuota).
pub fn start_quota(m: &Model) -> Option<Quota> {
    let k = start_kind(m);
    if k == LOGINS_KIND {
        return None;
    }
    for r in account_rows(m) {
        if r.kind == k && (r.current || r.head && !switches(m, &r.kind)) {
            return Some(r.q);
        }
    }
    Some(Quota::default())
}

/// A login's name, by its uuid (loginNamed).
pub fn login_named(m: &Model, id: &str) -> String {
    cfg(m)
        .login(id)
        .map(|l| l.name)
        .unwrap_or_else(|| "another account".into())
}

/// What a login is called: its name, else its email, else the start of
/// its id (loginName).
pub fn login_name(l: &LoginView) -> String {
    if !l.name.is_empty() {
        return l.name.clone();
    }
    if !l.email.is_empty() {
        return l.email.clone();
    }
    l.id.chars().take(8).collect()
}

/// The name of the login new sessions run on (currentLoginName).
pub fn current_login_name(logins: &[LoginView]) -> String {
    logins
        .iter()
        .find(|l| l.current)
        .map(login_name)
        .unwrap_or_else(|| "another".into())
}

/// A profile's OnLimit in words (limitWords).
pub fn limit_words(v: &str) -> &'static str {
    match v {
        rush_state::profiles::LIMIT_WAIT => "wait for the reset",
        rush_state::profiles::LIMIT_HANDOFF => "another account, then hand on",
        _ => "another account",
    }
}

/// What a profile does, in a line (profileWords).
pub fn profile_words(m: &Model, p: &Profile) -> String {
    let names: Vec<String> = p
        .installed()
        .iter()
        .map(|k| agent_name(&m.reg, k))
        .collect();
    if names.is_empty() {
        return "none of its providers is installed".into();
    }
    let s = if p.mixes() {
        names.join(" → ")
    } else {
        format!("{} only", names[0])
    };
    format!("{s} · at a limit: {}", limit_words(p.limit()))
}

// ---- Limits as a row shows them ----

/// Whether a reading failed on an expired sign-in (expired).
pub fn expired(q: &Quota) -> bool {
    q.problem.contains(usage::EXPIRED)
}

/// Why an account has no fresh reading, as its row says it: a Claude Code
/// login's expired sign-in says r refreshes it, one just refreshed how
/// that went (problem).
pub fn problem(m: &Model, r: &AcctRow) -> String {
    let Some(l) = &r.login else {
        return r.q.problem.clone();
    };
    if let Some(n) = peek(m).renew.get(&l.id) {
        if n.why.is_empty() {
            return "refreshing the sign-in…".into();
        }
        if now(m) - n.at < 2 * 60_000 {
            return format!("couldn't refresh: {}", n.why);
        }
    }
    if expired(&r.q) {
        return format!("{} · r refreshes it", usage::EXPIRED);
    }
    r.q.problem.clone()
}

/// A duration to one unit: 40m, 3h, 5d (roughly).
pub fn roughly(d: Ms) -> String {
    const MIN: Ms = 60_000;
    const HOUR: Ms = 60 * MIN;
    if d < HOUR {
        format!("{}m", ((d + MIN / 2) / MIN).max(1))
    } else if d < 48 * HOUR {
        format!("{}h", (d + HOUR / 2) / HOUR)
    } else {
        format!("{}d", (d + 12 * HOUR) / (24 * HOUR))
    }
}

/// When a window resets, roughly: " ↻3h" (resetIn).
pub fn reset_in(at: Ms, now: Ms) -> Span {
    if at == 0 {
        return plain("");
    }
    if at <= now {
        return sp(" reset", FAINT); // read before a reset that has since passed
    }
    sp(format!(" ↻{}", roughly(at - now)), FAINT)
}

/// A window by name (quotaWindowName).
pub fn quota_window_name(w: &Window) -> String {
    [&w.name, &w.label, &w.id]
        .into_iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "Provider quota".into())
}

/// The local wall clock at ms, as 15:04, or Mon 15:04 with day.
pub fn clock(ms: Ms, day: bool) -> String {
    let t = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only the tm it's given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    let hm = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    if !day {
        return hm;
    }
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!("{} {hm}", DAYS[tm.tm_wday.clamp(0, 6) as usize])
}

/// An account's first two windows, each a labelled meter, or its balance,
/// or why there are none, w1 + w2 wide (limits).
pub fn limits(m: &Model, r: &AcctRow, w1: usize, w2: usize) -> Line {
    let mut q = r.q.clone();
    q.problem = problem(m, r);
    let now = now(m);
    if q.windows.is_empty() {
        let mut msg = line([sp("no reading", FAINT)]);
        if !q.balance.is_empty() {
            msg = if q.problem.is_empty() {
                line([sp(q.balance.clone(), TEXT)])
            } else {
                line([sp(format!("{} · {}", q.balance, q.problem), YELLOW)])
            };
        } else if !q.problem.is_empty() {
            msg = line([sp(q.problem.clone(), YELLOW)]);
        } else if r.kind != LOGINS_KIND {
            let reads = m
                .reg
                .get(&r.kind)
                .is_some_and(|a| a.quota_source().is_some());
            if !reads {
                msg = line([sp("doesn't report its limits", FAINT)]);
            } else if q.fetched_at == 0 && r.current {
                msg = line([sp("fetching…", FAINT)]);
            } else if q.fetched_at == 0 {
                msg = line([sp("read while it's in use", FAINT)]);
            }
        } else if q.fetched_at == 0 {
            msg = line([sp("fetching…", FAINT)]);
        }
        return fit(msg, w1 + w2);
    }
    let every = usage::EVERY.as_millis() as Ms;
    if now - q.fetched_at > 3 * every {
        // Say why it's old, not only that it is: a sign-in gone, a limit on asking.
        let mut why = format!("not read since {}", clock(q.fetched_at, false));
        if !q.problem.is_empty() {
            why = format!("{} · last read {}", q.problem, clock(q.fetched_at, false));
        }
        return fit(line([sp("! ", YELLOW), sp(why, DIM)]), w1 + w2);
    }
    let mut out = Line::new();
    for (i, cw) in [w1, w2].into_iter().enumerate() {
        let Some(win) = q.windows.get(i) else {
            join(&mut out, line([plain(blanks(cw))]));
            continue;
        };
        let mut cell = line([sp(fit_str(&win.label, 3), FAINT)]);
        join(&mut cell, bar(win.percent));
        cell.spans.push(plain(" "));
        cell.spans.push(sp(format!("{:>3.0}%", win.percent), TEXT));
        cell.spans.push(reset_in(win.resets_at, now));
        join(&mut out, fit(cell, cw));
    }
    out
}

/// A reading's windows, each with when it resets, then its balance and
/// when it was read (windowLines).
pub fn window_lines(q: &Quota, now: Ms, label: &dyn Fn(&str) -> Span) -> Vec<Line> {
    let mut out = Vec::new();
    for win in &q.windows {
        let mut s = line([label(&win.label)]);
        join(&mut s, bar(win.percent));
        s.spans.push(plain(" "));
        s.spans.push(sp(format!("{:.0}%", win.percent), TEXT));
        if win.resets_at != 0 {
            let when = clock(win.resets_at, win.resets_at - now > 20 * 3_600_000);
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
        out.push(line([label("balance"), sp(q.balance.clone(), TEXT)]));
    }
    if q.windows.is_empty() && !q.problem.is_empty() {
        out.push(line([label("limits"), sp(q.problem.clone(), YELLOW)]));
    }
    if q.fetched_at != 0 {
        let source = match q.source {
            Source::Fetched => "fetched",
            Source::Live => "reported by a session",
            _ => "read",
        };
        out.push(line([
            label("usage"),
            sp(format!("{source} at {}", clock(q.fetched_at, false)), FAINT),
        ]));
    }
    out
}
