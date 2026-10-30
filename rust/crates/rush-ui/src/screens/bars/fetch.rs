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
    busy: bool,
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
    // Hosted never fetches usage or quotas (model.go:556, :1125).
    if m.sender().is_none() || crate::hosted::quiet(m) {
        return;
    }
    let now = rush_core::time::now();
    let st = m.kept::<Fetch>();
    if st.busy || now - st.at < EVERY.as_millis() as Ms {
        return;
    }
    st.busy = true;
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
            read_all(&jobs);
            read_logins(&c, &logins);
        },
        |m, ()| {
            m.kept::<Fetch>().busy = false;
            crate::screens::accounts::state::read(m);
            Out::Idle
        },
    );
}

/// Every agent's plan read again now, whenever the last read was
/// (Settings › Providers' r: fetchUsage and fetchQuotas). Each reading
/// younger than usage::EVERY is still kept, as usage::refresh keeps it.
pub fn fetch_now(m: &mut Model) {
    m.kept::<Fetch>().at = 0;
    on_fleet(m);
}

/// Each saved login's plan usage read again, into usage.json
/// (fetchLoginUsage).
fn read_logins(cfg: &Config, logins: &[Login]) {
    let Some(refresh) = REFRESH_LOGIN.get() else {
        return;
    };
    let path = rush_state::dir().join("usage.json");
    for lg in logins {
        refresh(&path.to_string_lossy(), cfg, lg, false);
    }
}

fn read_all(jobs: &[Job]) {
    let dir = rush_state::dir();
    let usage_path = dir.join("usage.json");
    let quotas: PathBuf = dir.join("quotas.json");
    for (ad, p, kept) in jobs {
        if ad.kind().as_str() == LOGINS_KIND {
            if let Some(pr) = ad.plan_reader() {
                pr.refresh_plan(&usage_path.to_string_lossy(), p, false);
            }
            continue;
        }
        let Some(src) = ad.quota_source() else {
            continue;
        };
        if !kept.is_empty() {
            // It reads any account's: each kept one by its own key.
            for a in kept {
                usage::refresh(&quotas, &a.key, false, &Net, || {
                    src.quota(p, a).map_err(|e| e.to_string())
                });
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
        }
    }
}
