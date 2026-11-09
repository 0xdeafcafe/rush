//! Emeritus in the list (docs/emeritus.md): a session that handed its
//! task over and retired is folded into the row of the session carrying
//! the task on, its cost counted there, so a chain of handovers is one
//! row, not one for each context. One that retired a moment ago stays a
//! row for a while, saying whom it handed over to; one opened as a
//! session of its own (Handle::unfold) stays a row too, as spawned runs
//! do. Rust-only: Go rush has no Emeritus.

use std::collections::HashMap;

use rush_core::agent::{Agent, Job, Lineage};
use rush_core::time::Ms;

use crate::hosted::HostInfo;
use crate::loader::Loader;

/// How long a session that just retired stays a row of its own, saying
/// whom it handed over to, before it folds into its successor's.
pub(crate) const RETIRED_SHOWN: Ms = 10 * 60_000;

/// Where info's session stands in a chain of handovers, when it's in
/// one: the session that took over from it once it's retired, and the one
/// it took over from (its meta's predecessor and lineage).
pub(crate) fn lineage_of(info: &HostInfo) -> Option<Box<Lineage>> {
    let successor = if info.emeritus == "retired" {
        info.successor.clone()
    } else {
        String::new()
    };
    let predecessor = info.meta.get("predecessor").cloned().unwrap_or_default();
    if successor.is_empty() && predecessor.is_empty() {
        return None;
    }
    Some(Box::new(Lineage {
        handovers: if predecessor.is_empty() {
            0
        } else {
            info.meta
                .get("lineage")
                .and_then(|l| l.parse().ok())
                .unwrap_or(1)
        },
        successor,
        predecessor,
        capped: info.emeritus == "capped",
        ..Lineage::default()
    }))
}

/// A retired session's row says so, in place of its host's last word.
pub(crate) fn retired_detail(info: &HostInfo, j: &mut Job) {
    if info.emeritus != "retired" {
        return;
    }
    j.detail = if info.successor.is_empty() {
        "retired · its start as a successor was rolled back".into()
    } else {
        "retired · handed over to its successor".into()
    };
}

impl Loader {
    /// Folds each retired session into the row at the end of its chain
    /// of successors, once it's been retired a while and isn't open as a
    /// session of its own; one whose successor isn't listed stays a row.
    /// Each successor's row names the session it took over from.
    pub(crate) fn fold_retired(&self, mut agents: Vec<Agent>, now: Ms) -> Vec<Agent> {
        if !agents.iter().any(|a| a.rush && !a.successor().is_empty()) {
            return agents;
        }
        let by_id: HashMap<String, usize> = agents
            .iter()
            .enumerate()
            .filter(|(_, a)| a.rush)
            .map(|(i, a)| (a.job.id.clone(), i))
            .collect();
        // Each successor's row names its predecessor while that's listed.
        let names: Vec<(usize, String)> = agents
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                let p = &a.lineage.as_ref()?.predecessor;
                if p.is_empty() {
                    return None;
                }
                Some((i, agents[*by_id.get(p)?].display_name.clone()))
            })
            .collect();
        for (i, n) in names {
            if let Some(l) = agents[i].lineage.as_mut() {
                l.name = n;
            }
        }
        let mut keep = vec![true; agents.len()];
        for i in 0..agents.len() {
            let a = &agents[i];
            if !a.rush
                || a.successor().is_empty()
                || self.unfolded.contains(&a.job.id)
                || now - a.job.updated_at < RETIRED_SHOWN
            {
                continue;
            }
            // The row at the end of its chain, as listed.
            let mut top = None;
            let mut cur = a.successor().to_string();
            for _ in 0..64 {
                let Some(&j) = by_id.get(&cur) else { break };
                if j == i {
                    break;
                }
                top = Some(j);
                if agents[j].successor().is_empty() {
                    break;
                }
                cur = agents[j].successor().to_string();
            }
            let Some(t) = top.filter(|&t| agents[t].successor().is_empty()) else {
                continue;
            };
            let (cost, today) = (a.spend.cost, a.spend.today);
            let b = &mut agents[t];
            b.spend.cost += cost;
            b.spend.today += today;
            b.lineage.get_or_insert_with(Box::default).folded += 1;
            keep[i] = false;
        }
        let mut k = keep.into_iter();
        agents.retain(|_| k.next().unwrap_or(true));
        agents
    }
}
