//! What Settings › Harnesses and Providers ask of accounts (Go reads
//! them off one Model: settings_providers.go's accountRows, problem,
//! addAccount, loginKey and signInKey are accounts.go's and logins.go's),
//! handed over through settings::acct::provide. Until accounts has read
//! rush's config (the TUI reads it after the first reading of the list),
//! Settings keeps its own stand-ins, which read Settings' copy.

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
        ..sacct::STAND_IN
    });
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
