//! The setups the fleet's agents ran lately, for the shelf to offer.
//! After Go's internal/ui/recentsetups.go:13 (recentSetups) and
//! model.go:392 (newest, sorted once a reading).

use super::setup::{StartOver, start_defaults, start_words};
use super::state::{accounts, peek};
use crate::model::Model;

/// After each reading of the list: its agents newest first, by key, so
/// a frame doesn't sort them again.
pub(crate) fn on_fleet(m: &mut Model) {
    let mut idx: Vec<usize> = (0..m.agents.len()).collect();
    idx.sort_by_key(|i| std::cmp::Reverse(m.agents[*i].job.created_at));
    let keys = idx.into_iter().map(|i| m.agents[i].key.clone()).collect();
    accounts(m).newest = keys;
}

/// Up to 3 setups the fleet's agents ran, newest first, each once, cur
/// left out (recentSetups). The open session's comes from what it runs
/// as now, when its screens say (`Accounts::session_start`).
pub fn recent_setups(m: &Model, cur: &StartOver) -> Vec<StartOver> {
    let mut seen = vec![start_words(m, cur).join(" · ")];
    let mut out = Vec::new();
    // Most of a fleet runs a few setups: each is weighed once.
    let mut tried: Vec<(String, String)> = Vec::new();
    let st = peek(m);
    for key in &st.newest {
        if out.len() == 3 {
            break;
        }
        let Some(a) = m.agents.iter().find(|a| a.key == *key) else {
            continue;
        };
        if a.advisor {
            continue; // rush's own advisor, not a setup you ran
        }
        let k = if a.kind.as_str().is_empty() {
            a.acct.kind.as_str()
        } else {
            a.kind.as_str()
        };
        if k.is_empty() {
            continue;
        }
        let o = match st.session_start.and_then(|f| f(m, key)) {
            Some(o) => o,
            None => {
                let km = (k.to_string(), a.spend.model.clone());
                if tried.contains(&km) {
                    continue;
                }
                tried.push(km);
                let mut o = start_defaults(m, k);
                if !a.spend.model.is_empty() {
                    o.model = a.spend.model.clone();
                }
                o
            }
        };
        let w = start_words(m, &o).join(" · ");
        if !seen.contains(&w) {
            seen.push(w);
            out.push(o);
        }
    }
    out
}
