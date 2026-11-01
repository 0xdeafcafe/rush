//! What Accounts knows beyond the list: rush's config (the store), each
//! login with its plan's reading, every other account's limits, who each
//! agent is signed in as, and what the next session starts as. The state
//! itself is the base's (crate::accounts); here is its reading after each
//! reading of the list. After Go's internal/ui/accounts.go (accountsState
//! :24, ready :57) and agentquota.go:72 (quotaMsg.applyTo).

use rush_state::Store;

// The accounts' state is the base's: every screen reads it.
pub use crate::accounts::{
    Accounts, LoginView, Read, RenewNote, Starter, Switcher, accounts, cfg, cfg_mut, ensure_store,
    logins, peek, read_only, read_settled, readings, save, set_fetched, set_quotas, set_store,
    signed_in_as,
};
use crate::frame::Out;
use crate::model::Model;

/// rush's config as it is, never written back: `--render` draws what Go's
/// does from the same config.json (cmd/rush/main.go:510 state.Load) but
/// leaves it as it was.
pub fn load_read_only(m: &mut Model) {
    accounts(m).read_only = true;
    ensure_store(m);
    read(m); // with no loop, at once
}

/// After each reading of the list: the store loaded off the UI thread the
/// first time, and the accounts' readings read again. Only with a loop:
/// tests and `--render` load what they need when asked.
pub fn on_fleet(m: &mut Model) {
    // The header names the account new sessions start on from the first
    // frame: state's list knows Claude Code's home.
    crate::bridge::sync(m);
    if m.sender().is_none() {
        return;
    }
    let a = accounts(m);
    if a.store.is_none() && !a.loading {
        a.loading = true;
        m.later(Store::load, |m, s| {
            let a = accounts(m);
            a.loading = false;
            if a.store.is_none() {
                a.store = Some(s);
            }
            read(m);
            Out::Draw
        });
        return;
    }
    read(m);
    if !accounts(m).asked {
        accounts(m).asked = true;
        super::keys::find_sign_ins(m);
    }
}

/// Reads the logins' and accounts' readings off the UI thread.
pub fn read(m: &mut Model) {
    let a = accounts(m);
    let Some(store) = a.store.as_ref() else {
        return;
    };
    if a.reading {
        return;
    }
    a.reading = true;
    let cfg = store.config.clone();
    let kept = a.fetched.clone();
    let kept_quotas = a.quotas_read.clone();
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    m.later(
        move || {
            let mut r = Read::from_disk_with(
                &cfg,
                &rush_state::dir(),
                &crate::accounts::claude_json(),
                &kept,
                now,
            );
            for (k, q) in kept_quotas {
                if r.quotas
                    .get(&k)
                    .is_none_or(|f| f.fetched_at <= q.fetched_at)
                {
                    r.quotas.insert(k, q);
                }
            }
            r
        },
        |m, r| {
            let a = accounts(m);
            a.reading = false;
            a.read_once = true;
            let changed = a.logins != r.logins || a.quotas != r.quotas;
            a.logins = r.logins;
            a.quotas = r.quotas;
            // Other agents' accounts, with each reading of the list: a
            // session a limit stopped is in the list's (checkLimits).
            super::autoswitch::check_limits(m);
            if !changed {
                return Out::Idle;
            }
            // A session's usage reading arrives with the list's reading.
            super::autoswitch::auto_switch(m);
            // New sessions move on when the first provider's accounts are
            // all nearly out: rush does it on its own only where it leads
            // (agentquota.go:95, :113).
            super::profiles::auto_spill(m);
            Out::Draw
        },
    );
}
