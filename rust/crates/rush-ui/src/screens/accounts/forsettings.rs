//! What Settings › Harnesses and Providers ask of accounts (Go reads
//! them off one Model: settings_providers.go's accountRows, problem,
//! addAccount, loginKey and signInKey are accounts.go's and logins.go's),
//! handed over through settings::acct::provide. Until accounts has read
//! rush's config (the TUI reads it after the first reading of the list),
//! Settings keeps its own stand-ins, which read Settings' copy. Who each
//! agent is signed in as and why not (accounts.go:24 accountsState's why,
//! summary, now), the order the logins are spent in (fleet.Chain, as
//! settings_providers.go:636 draws it), r reading every limit again
//! (settings_providers.go:271: fetchUsage, model.go:613, and fetchQuotas,
//! agentquota.go:26) and u spending an earned reset (useReset,
//! settings_providers.go:946) are accounts' too.

use crate::frame::Out;
use crate::model::Model;
use crate::screens::settings::acct::{self as sacct, AcctRow as SRow};

use super::keys;
use super::rows::{self, AcctRow};
use super::state::peek;

/// Settings' part, as accounts answers it.
pub fn provide() {
    sacct::provide(sacct::Accounts {
        rows: settings_rows,
        problem: |m, r| match find(m, r) {
            Some(a) if loaded(m) => rows::problem(m, &a),
            _ => (sacct::STAND_IN.problem)(m, r),
        },
        add: |m, k| match loaded(m) {
            true => keys::add_account(m, k),
            false => (sacct::STAND_IN.add)(m, k),
        },
        login_key: |m, r, k| match find(m, r) {
            Some(a) if loaded(m) => keys::account_key(m, &a, k),
            _ => (sacct::STAND_IN.login_key)(m, r, k),
        },
        sign_in_key: |m, r, k| match find(m, r) {
            Some(a) if loaded(m) => keys::account_key(m, &a, k),
            _ => (sacct::STAND_IN.sign_in_key)(m, r, k),
        },
        find_sign_ins: |m| {
            if !loaded(m) {
                return (sacct::STAND_IN.find_sign_ins)(m);
            }
            keys::find_sign_ins(m);
            Out::Draw
        },
        spill: |m| peek(m).spill.clone(),
        // Settings has just changed its config: its default profile is
        // the one to go by (settings_config.go:303, settings_providers.go:260).
        spill_to: |m| {
            if !loaded(m) {
                return;
            }
            let p = crate::screens::settings::config(m).default_profile();
            super::profiles::spill_to_for(m, &p, true);
        },
        why: |m, k| peek(m).why.get(k).cloned().unwrap_or_default(),
        summary: |m, k| peek(m).summary.get(k).cloned(),
        signed_in: |m, k| peek(m).now.contains_key(k),
        chain,
        use_reset: |m, r| match find(m, r) {
            Some(a) if loaded(m) => use_reset(m, &a),
            _ => (sacct::STAND_IN.use_reset)(m, r),
        },
        fetch: |m| {
            if !loaded(m) {
                return (sacct::STAND_IN.fetch)(m);
            }
            fetch(m);
            Out::Draw
        },
    });
}

/// Every provider's limits read again, and who each agent is signed in
/// as: Claude Code's logins (fetchUsage), then every other agent's
/// accounts (fetchQuotas). A reading younger than usage.Every is kept, as
/// Go's usage.Refresh keeps it.
fn fetch(m: &mut Model) {
    super::logins::fetch_usage(m);
    keys::find_sign_ins(m);
    crate::screens::bars::fetch::fetch_now(m);
}

/// The order rush spends Claude Code's logins in, each with its week's
/// room (fleet.Chain).
fn chain(m: &Model) -> Vec<sacct::Link> {
    let now = m.clock.unwrap_or(m.now);
    let logins: Vec<rush_fleet::LoginView> = peek(m)
        .logins
        .iter()
        .map(|l| rush_fleet::LoginView {
            login: l.login.clone(),
            usage: l.usage.clone(),
            quota: l.quota.clone(),
            current: l.current,
        })
        .collect();
    rush_fleet::chain(&logins, now)
        .into_iter()
        .map(|k| {
            let w = rush_fleet::week(&k.login.quota).unwrap_or_default();
            sacct::Link {
                name: k.login.login.name.clone(),
                current: k.login.current,
                week_percent: w.percent,
                left: rush_fleet::window_left(&w, now),
                wait: k.wait,
            }
        })
        .collect()
}

/// What spending a reset came to: its words, or why it failed, and the
/// account's limits read again at once.
struct ResetDone {
    words: Result<String, String>,
    q: Vec<(String, rush_core::usage::Quota)>,
}

/// Spends one of an account's earned resets, once you say so. A reset
/// goes to the account its agent is signed in to, so another one is
/// switched to first (useReset).
fn use_reset(m: &mut Model, r: &AcctRow) -> Out {
    let name = r.name(m);
    let Some(ad) = m.reg.get(&r.kind) else {
        return Out::Idle;
    };
    let available = r.q.resets.as_ref().map_or(0, |rs| rs.available);
    if available == 0 {
        m.flash(&format!("{name} has no limit resets"));
        return Out::Draw;
    }
    if ad.reset_spender().is_none() {
        m.flash_err(&format!(
            "{} can't spend resets through rush",
            super::look::agent_name(&m.reg, &r.kind)
        ));
        return Out::Draw;
    }
    if !r.current {
        m.flash(&format!(
            "a reset goes to the account in use: enter switches to {name}, then u"
        ));
        return Out::Draw;
    }
    let Some(p) = rows::profile_of(m, &ad) else {
        return Out::Idle;
    };
    let q = format!(
        "Use one of {name}'s {available} limit resets now? Its limits go back to empty; {} left after.",
        available - 1
    );
    m.ask(crate::frame::confirm::Confirm::new(q, move |m| {
        m.flash(&format!("using a reset on {name}…"));
        m.later(
            move || {
                let mut d = ResetDone {
                    words: Err(String::new()),
                    q: Vec::new(),
                };
                let Some(sp) = ad.reset_spender() else {
                    return d;
                };
                d.words = sp.use_reset(&p, "").map_err(|e| e.to_string());
                let Some(src) = ad.quota_source() else {
                    return d;
                };
                // Read again at once, so the limits shown are the reset ones.
                let a = rush_core::adapter::Account {
                    kind: p.kind.clone(),
                    ..Default::default()
                };
                if let Ok(q) = src.quota(&p, &a) {
                    let path = rush_state::dir().join("quotas.json");
                    let _ = rush_core::usage::record(&path, &format!("{}:{}", p.kind, p.dir), &q);
                    d.q.push((p.dir.clone(), q.clone()));
                    if !q.account.is_empty() {
                        let _ = rush_core::usage::record(&path, &q.account.clone(), &q);
                        d.q.push((q.account.clone(), q));
                    }
                }
                d
            },
            move |m, d: ResetDone| {
                match d.words {
                    Err(e) => m.fail(&format!("couldn't use a reset on {name}: {e}")),
                    Ok(w) => m.flash(&format!("{name}: {w}")),
                }
                // The newest of each reading (quotaMsg.applyTo).
                let qs = &mut super::state::accounts(m).quotas;
                for (k, q) in d.q {
                    if qs.get(&k).is_some_and(|old| old.fetched_at > q.fetched_at) {
                        continue;
                    }
                    qs.insert(k, q);
                }
                super::autoswitch::check_limits(m);
                Out::Draw
            },
        );
        Out::Draw
    }));
    Out::Draw
}

/// Whether accounts has rush's config to answer from.
fn loaded(m: &Model) -> bool {
    peek(m).store.is_some()
}

/// Every agent and its accounts, as Settings lists them (accountRows).
fn settings_rows(m: &Model) -> Vec<SRow> {
    if !loaded(m) {
        return (sacct::STAND_IN.rows)(m);
    }
    rows::account_rows(m)
        .iter()
        .map(|r| to_settings(m, r))
        .collect()
}

fn to_settings(m: &Model, r: &AcctRow) -> SRow {
    let (login, borrowed, who) = match &r.login {
        Some(l) => (l.id.clone(), l.borrowed, vec![l.org.clone()]),
        None => (
            String::new(),
            false,
            vec![r.acct.org.clone(), r.acct.plan.clone()],
        ),
    };
    SRow {
        kind: r.kind.clone(),
        head: r.head,
        login,
        borrowed,
        name: if r.head { String::new() } else { r.name(m) },
        email: if r.head { String::new() } else { r.email() },
        who,
        current: r.current,
        q: r.q.clone(),
    }
}

/// Accounts' row for Settings' r: the same agent and login, or account.
fn find(m: &Model, r: &SRow) -> Option<AcctRow> {
    rows::account_rows(m).into_iter().find(|a| {
        a.kind == r.kind
            && a.head == r.head
            && match &a.login {
                Some(l) => l.id == r.login,
                None => r.login.is_empty() && (r.head || a.name(m) == r.name),
            }
    })
}
