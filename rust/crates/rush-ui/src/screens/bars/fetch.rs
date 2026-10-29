//! The plans' limits read again every few minutes, off the UI thread, so
//! the meters move: the logins agent's plan through its adapter's
//! PlanReader (into usage.json), every other agent's account in use
//! through its QuotaSource (into quotas.json), both shared with every
//! rush and session. After Go's internal/ui/model.go:613 (fetchUsage)
//! and agentquota.go:26 (fetchQuotas). Not yet: the other logins' own
//! readings (fetchLoginUsage) and each kept sign-in's of an agent that
//! reads any account's (AnyAccountQuota).

use std::path::PathBuf;

use rush_core::adapter::{Account, AdapterRef, Profile};
use rush_core::time::Ms;
use rush_core::usage::{self, EVERY};

use crate::frame::Out;
use crate::model::Model;
use crate::screens::accounts::look::LOGINS_KIND;
use crate::screens::accounts::rows::{agent_order, profile_of};

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
    if m.sender().is_none() {
        return;
    }
    let now = rush_core::time::now();
    let st = m.kept::<Fetch>();
    if st.busy || now - st.at < EVERY.as_millis() as Ms {
        return;
    }
    st.busy = true;
    st.at = now;
    let jobs: Vec<(AdapterRef, Profile)> = agent_order(m)
        .into_iter()
        .filter_map(|a| profile_of(m, &a).map(|p| (a, p)))
        .collect();
    m.later(
        move || read_all(&jobs),
        |m, ()| {
            m.kept::<Fetch>().busy = false;
            crate::screens::accounts::state::read(m);
            Out::Idle
        },
    );
}

fn read_all(jobs: &[(AdapterRef, Profile)]) {
    let dir = rush_state::dir();
    let usage_path = dir.join("usage.json");
    let quotas: PathBuf = dir.join("quotas.json");
    for (ad, p) in jobs {
        if ad.kind().as_str() == LOGINS_KIND {
            if let Some(pr) = ad.plan_reader() {
                pr.refresh_plan(&usage_path.to_string_lossy(), p, false);
            }
            continue;
        }
        let Some(src) = ad.quota_source() else {
            continue;
        };
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
