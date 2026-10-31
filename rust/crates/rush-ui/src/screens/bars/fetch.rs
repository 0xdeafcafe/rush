//! The plans' limits read again every few minutes, off the UI thread, so
//! the meters move: the logins agent's plan through its adapter's
//! PlanReader (into usage.json), every other agent's account in use
//! through its QuotaSource (into quotas.json), both shared with every
//! rush and session. After Go's internal/ui/model.go:613 (fetchUsage),
//! agentquota.go:26 (fetchQuotas, with each kept sign-in of an agent
//! whose QuotaSource reads any account's: AnyAccountQuota, :39) and
//! logins.go:80 (fetchLoginUsage: every saved login but the one in use,
//! through the logins agent's RefreshLogin, adapters/claude/logins.go:159,
//! which the binary hands in with [`set_login_refresher`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use rush_core::adapter::{Account, AdapterRef, Profile};
use rush_core::time::Ms;
use rush_core::usage::{self, EVERY, Reading};
use rush_state::{Config, Login};

use crate::frame::Out;
use crate::model::Model;
use crate::screens::accounts::look::LOGINS_KIND;
use crate::screens::accounts::rows::{agent_order, profile_of, sign_in_account};
use crate::screens::accounts::state::{cfg, peek};

/// Reads a saved login's plan usage into the readings shared through
/// path (state.LoginKeeper's RefreshLogin): the logins agent's.
pub type LoginRefresher = fn(path: &str, cfg: &Config, lg: &Login, offline: bool) -> Reading;

static REFRESH_LOGIN: OnceLock<LoginRefresher> = OnceLock::new();

/// Hands in how saved logins' usage is read: the binary's, once.
pub fn set_login_refresher(f: LoginRefresher) {
    let _ = REFRESH_LOGIN.set(f);
}

/// One agent's readings to take: its profile, and the accounts kept for
/// it when its QuotaSource reads any account's.
type Job = (AdapterRef, Profile, Vec<Account>);

/// When the limits were last read, and whether a read is out.
#[derive(Default)]
pub struct Fetch {
    at: Ms,
    /// Reads out.
    busy: u32,
}

impl Fetch {
    /// Whether to read again at now: once the last is EVERY old and
    /// none is out; asked for (r), always, a read out or not, as Go's
    /// fetchUsage and fetchQuotas start one each time.
    fn due(&self, now: Ms, asked: bool) -> bool {
        asked || self.busy == 0 && now - self.at >= EVERY.as_millis() as Ms
    }
}

/// The network as the readings ask it: not while it's down, and each
/// failure said (netwatch).
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

/// With each reading of the list, once the last read is EVERY old: every
/// agent's plan read again, then the accounts' readings taken in. Only
/// with a loop: a test or `--render` never asks the network.
pub fn on_fleet(m: &mut Model) {
    read_again(m, false);
}

/// Every agent's plan read again, when due or asked (r).
fn read_again(m: &mut Model, asked: bool) {
    // Hosted never fetches usage or quotas (model.go:556, :1125).
    if m.sender().is_none() || crate::hosted::quiet(m) {
        return;
    }
    let now = rush_core::time::now();
    let st = m.kept::<Fetch>();
    if !st.due(now, asked) {
        return;
    }
    st.busy += 1;
    st.at = now;
    let c = cfg(m).clone();
    let jobs: Vec<Job> = agent_order(m)
        .into_iter()
        .filter_map(|a| {
            let kept = if a.reads_any_account() {
                c.sign_ins_of(a.kind().as_str())
                    .iter()
                    .map(sign_in_account)
                    .collect()
            } else {
                Vec::new()
            };
            profile_of(m, &a).map(|p| (a, p, kept))
        })
        .collect();
    // Every saved login but the one in use, whose reading is ~/.claude's.
    let logins: Vec<Login> = c
        .logins
        .iter()
        .filter(|lg| !peek(m).logins.iter().any(|l| l.id == lg.id && l.current))
        .cloned()
        .collect();
    m.later(
        move || {
            let mut got = read_all(&jobs);
            got.readings.extend(read_logins(&c, &logins));
            got
        },
        |m, got| {
            let st = m.kept::<Fetch>();
            st.busy = st.busy.saturating_sub(1);
            // What each read said, why it failed included, kept as Go's
            // usageMsg and quotaMsg keep it: the shared files only hold
            // the good ones.
            for (k, u) in got.readings {
                crate::screens::accounts::state::set_fetched(m, k, u);
            }
            crate::screens::accounts::state::set_quotas(m, got.quotas);
            crate::screens::accounts::state::read(m);
            Out::Idle
        },
    );
}

/// Every agent's plan read again now, whenever the last read was
/// (Settings › Providers' r: fetchUsage and fetchQuotas). Each reading
/// younger than usage::EVERY is still kept, as usage::refresh keeps it.
pub fn fetch_now(m: &mut Model) {
    read_again(m, true);
}

/// Each saved login's plan usage read again, into usage.json
/// (fetchLoginUsage): what each read said, by its reading key.
fn read_logins(cfg: &Config, logins: &[Login]) -> Vec<(String, Reading)> {
    let Some(refresh) = REFRESH_LOGIN.get() else {
        return Vec::new();
    };
    let path = rush_state::dir().join("usage.json");
    logins
        .iter()
        .map(|lg| {
            (
                lg.usage_key(),
                refresh(&path.to_string_lossy(), cfg, lg, false),
            )
        })
        .collect()
}

/// What a round of reads said: the logins' plans by reading key, every
/// other account's limits by key (usageMsg, quotaMsg).
#[derive(Default)]
struct Got {
    readings: Vec<(String, Reading)>,
    quotas: HashMap<String, usage::Quota>,
}

fn read_all(jobs: &[Job]) -> Got {
    let mut got = Got::default();
    let dir = rush_state::dir();
    let usage_path = dir.join("usage.json");
    let quotas: PathBuf = dir.join("quotas.json");
    for (ad, p, kept) in jobs {
        if ad.kind().as_str() == LOGINS_KIND {
            if let Some(pr) = ad.plan_reader() {
                let u = pr.refresh_plan(&usage_path.to_string_lossy(), p, false);
                got.readings.push((reading_key(p, &u), u));
            }
            continue;
        }
        let Some(src) = ad.quota_source() else {
            continue;
        };
        if !kept.is_empty() {
            // It reads any account's: each kept one by its own key.
            for a in kept {
                let q = usage::refresh(&quotas, &a.key, false, &Net, || {
                    src.quota(p, a).map_err(|e| e.to_string())
                });
                got.quotas.insert(a.key.clone(), q);
            }
            continue;
        }
        let key = format!("{}:{}", p.kind, p.dir);
        let a = Account {
            kind: p.kind.clone(),
            ..Account::default()
        };
        let q = usage::refresh(&quotas, &key, false, &Net, || {
            src.quota(p, &a).map_err(|e| e.to_string())
        });
        if !q.account.is_empty() && q.problem.is_empty() && q.fetched_at != 0 {
            // Kept as the account's too, for when it's no longer the one
            // in use.
            let older = usage::load(&quotas)
                .get(&q.account)
                .is_none_or(|old| old.fetched_at < q.fetched_at);
            if older {
                let _ = usage::record(&quotas, &q.account.clone(), &q);
            }
            got.quotas.insert(q.account.clone(), q.clone());
        }
        got.quotas.insert(p.dir.clone(), q);
    }
    got
}

/// Where a reading r of profile p's account is kept: by the login it's
/// signed in as, else by its folder (fleet.ReadingKey).
pub fn reading_key(p: &Profile, r: &Reading) -> String {
    if r.account_id.is_empty() {
        return p.dir.clone();
    }
    Login {
        id: r.account_id.clone(),
        ..Login::default()
    }
    .usage_key()
}

#[cfg(test)]
mod tests {
    use super::*;

    // settings_providers.go:271: r reads every limit again even while a
    // reading is out; the reading with the list waits for it and its time.
    #[test]
    fn r_reads_again_while_a_reading_is_out() {
        let now = 10 * EVERY.as_millis() as Ms;
        let out = Fetch {
            at: now - 1,
            busy: 1,
        };
        assert!(out.due(now, true), "asked, it reads again");
        assert!(!out.due(now, false), "by itself, it waits");
        let old = Fetch {
            at: now - EVERY.as_millis() as Ms,
            busy: 0,
        };
        assert!(old.due(now, false));
        let fresh = Fetch {
            at: now - 1,
            busy: 0,
        };
        assert!(!fresh.due(now, false));
    }
}
