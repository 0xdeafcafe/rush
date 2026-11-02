//! Settings › Config: which harnesses are on, which models each provider
//! may call in each, and what happens at a usage limit. Settings ›
//! Harnesses: the effort and permissions every harness starts with, and
//! what one changes. See docs/providers-harnesses.md.
//!
//! Ported from internal/state/setup.go.

use crate::agents::{self, Choice};
use crate::config::{Config, Dispatch, Start};
use crate::profiles::LOGINS_KIND;
use std::collections::BTreeMap;

/// What AtLimit can be.
pub const AT_LIMIT_ACCOUNT: &str = "";
pub const AT_LIMIT_WAIT: &str = "wait";
pub const AT_LIMIT_NEXT: &str = "next";

/// The permissions Every and ByHarness can name for every harness alike:
/// whichever mode of its own approves every tool.
pub const MODE_APPROVE_ALL: &str = "approve-all";

/// The cell of provider id in harness h: "ollama@pi".
pub fn cell_key(id: &str, h: &str) -> String {
    format!("{id}@{h}")
}

impl Config {
    /// Whether harness h is switched on.
    pub fn harness_on(&self, h: &str) -> bool {
        !self.harnesses_off.iter().any(|x| x == h)
    }

    /// Switches harness h on or off.
    pub fn set_harness_on(&mut self, h: &str, on: bool) {
        self.harnesses_off.retain(|v| v != h);
        if !on {
            self.harnesses_off.push(h.into());
        }
        self.sync_legacy();
    }

    /// What provider id may call in harness h; empty is all.
    pub fn cell_models_of(&self, id: &str, h: &str) -> Vec<String> {
        self.cell_models
            .get(&cell_key(id, h))
            .cloned()
            .unwrap_or_default()
    }

    /// Sets what provider id may call in harness h; none is all.
    pub fn set_cell_models(&mut self, id: &str, h: &str, models: Vec<String>) {
        if models.is_empty() {
            self.cell_models.remove(&cell_key(id, h));
            return;
        }
        self.cell_models.insert(cell_key(id, h), models);
    }

    /// Whether provider id may call model in harness h; "" is its default
    /// model, which any cell may.
    pub fn calls(&self, id: &str, h: &str, model: &str) -> bool {
        let ms = self.cell_models_of(id, h);
        model.is_empty() || ms.is_empty() || ms.iter().any(|m| m == model)
    }

    /// The harness auto runs provider id in when it's on: the one you
    /// picked, else its own.
    pub fn star_harness(&self, id: &str) -> String {
        if let Some(h) = self.runs_in.get(id).filter(|h| !h.is_empty()) {
            return h.clone();
        }
        match agents::runs_for(id).first() {
            Some(def) => agents::harness_of(def),
            None => String::new(),
        }
    }

    /// Whether provider id may run in harness h: its ★ or one you use it
    /// in, the harness on, and the two able to go together.
    pub fn cell_on(&self, id: &str, h: &str) -> bool {
        if !self.harness_on(h) {
            return false;
        }
        let (k, _, why) = agents::compat(id, h);
        if k.is_empty() || !why.is_empty() {
            return false;
        }
        self.star_harness(id) == h || self.uses(id, h)
    }

    /// The harness auto runs provider id in: its ★, else, when that's
    /// switched off, its first cell that's on; its ★ when none is.
    pub fn harness_for(&self, id: &str) -> String {
        let star = self.star_harness(id);
        if self.harness_on(&star) {
            return star;
        }
        if let Some(hs) = self.harnesses.get(id) {
            for h in hs {
                if self.cell_on(id, h) {
                    return h.clone();
                }
            }
        }
        star
    }

    /// RunsIn as auto reads it: a provider whose ★ harness is off goes to
    /// harness_for's.
    pub(crate) fn harnesses_now(&self) -> BTreeMap<String, String> {
        let mut out = self.runs_in.clone();
        if self.harnesses_off.is_empty() {
            return out;
        }
        for id in agents::provider_ids() {
            let h = self.harness_for(&id);
            if h != self.star_harness(&id) {
                out.insert(id, h);
            }
        }
        out
    }

    /// Whether provider id can start here: installed, its key kept when
    /// it's paid by one, and its harness on.
    pub(crate) fn ready(&self, id: &str) -> bool {
        let (pr, key) = agents::billed(id);
        if !agents::provider_installed(&pr) {
            return false;
        }
        if (key || !agents::split(&pr) && !agents::key_env(&pr).is_empty())
            && !self.has_api_key(&pr)
        {
            return false;
        }
        self.harness_on(&self.harness_for(id))
    }
}

impl Dispatch {
    /// What harness h changes from Every.
    pub fn harness_start(&self, h: &str) -> Start {
        self.by_harness.get(h).cloned().unwrap_or_default()
    }

    /// Sets what harness h changes from Every; empty is none.
    pub fn set_harness_start(&mut self, h: &str, mut s: Start) {
        s.model.clear();
        if s == Start::default() {
            self.by_harness.remove(h);
            return;
        }
        self.by_harness.insert(h.into(), s);
    }

    /// What a new session on provider id, run by agent k, starts with: the
    /// route's model; the effort and permissions its harness changes, else
    /// Every's, each only where k offers it.
    pub fn resolved(&self, id: &str, k: &str) -> Start {
        let route = self.start_on(id, k);
        let h = self.harness_start(&agents::harness_of(k));
        let mut out = Start {
            model: route.model.clone(),
            effort: h.effort,
            mode: h.mode,
        };
        if out.effort.is_empty() {
            out.effort = self.every.effort.clone();
        }
        if out.mode.is_empty() {
            out.mode = self.every.mode.clone();
        }
        if !self.harness_starts {
            if out.effort.is_empty() {
                out.effort = route.effort;
            }
            if out.mode.is_empty() {
                out.mode = route.mode;
            }
        }
        let ch = agents::choices_of(k).unwrap_or_default();
        out.effort = offered(&ch.efforts, &out.effort);
        out.mode = if out.mode == MODE_APPROVE_ALL {
            approve_all_mode(&ch.modes)
        } else {
            offered(&ch.modes, &out.mode)
        };
        out
    }

    /// Resolved for agent kind on its own provider.
    pub fn resolved_for(&self, kind: &str) -> Start {
        self.resolved(&agents::provider_of(kind), kind)
    }

    /// Moves each route's own effort and permissions onto its harness, the
    /// first of a harness's routes to name one winning, once. The routes
    /// keep theirs for older rushes.
    pub(crate) fn move_harness_starts(&mut self) {
        // migration: effort and permissions by harness
        if self.harness_starts {
            return;
        }
        self.harness_starts = true;
        let mut moves = vec![(
            LOGINS_KIND.to_string(),
            Start {
                effort: self.effort.clone(),
                mode: self.permission.clone(),
                ..Start::default()
            },
        )];
        // The starts by kind, in order (a BTreeMap's).
        for k in self.starts.keys() {
            let (pr, key) = agents::billed(k);
            let kind = if key { pr } else { k.clone() };
            // As Go reads it: a key's start is looked up by its provider,
            // so the plan's moves, not the key's own.
            moves.push((
                kind.clone(),
                self.starts.get(&kind).cloned().unwrap_or_default(),
            ));
        }
        for (k, s) in moves {
            let h = agents::harness_of(&k);
            let mut hs = self.harness_start(&h);
            if hs.effort.is_empty() {
                hs.effort = s.effort;
            }
            if hs.mode.is_empty() {
                hs.mode = s.mode;
            }
            self.set_harness_start(&h, hs);
        }
    }
}

/// v when list has it, or lists nothing to check against.
fn offered(list: &[Choice], v: &str) -> String {
    if v.is_empty() || list.is_empty() || list.iter().any(|c| c.id == v) {
        return v.into();
    }
    String::new()
}

/// The mode of modes that approves every tool, or "".
pub fn approve_all_mode(modes: &[Choice]) -> String {
    for v in modes {
        if matches!(
            v.id.as_str(),
            "bypassPermissions" | "full-access" | "yolo" | "auto-approve" | "always-proceed"
        ) {
            return v.id.clone();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::fake::register_fakes;

    // A harness switched off runs nothing: a provider whose ★ it was goes
    // to its first other cell that's on, and comes back once it's on again.
    #[test]
    fn harness_off() {
        register_fakes();
        let mut c = Config::default();
        assert!(c.cell_on("pa", "pa") && !c.cell_on("pa", "pb") && c.harness_for("pa") == "pa");
        c.set_uses("pa", "pb", true);
        c.set_harness_on("pa", false);
        assert!(!c.cell_on("pa", "pa") && c.harness_for("pa") == "pb");
        assert_eq!(c.profile_named("pa").unwrap().kinds(), ["pa-pb"]);
        c.set_harness_on("pa", true);
        assert!(c.harness_for("pa") == "pa" && c.harnesses_off.is_empty());
    }

    // A cell calls every model until some are picked; its default always.
    #[test]
    fn cell_models() {
        let mut c = Config::default();
        assert!(c.calls("pa", "pb", "m1"), "no list is every model");
        c.set_cell_models("pa", "pb", vec!["m2".into()]);
        assert!(!c.calls("pa", "pb", "m1") && c.calls("pa", "pb", "m2"));
        assert!(c.calls("pa", "pb", "") && c.calls("pa", "pa", "m1"));
        c.set_cell_models("pa", "pb", vec![]);
        assert!(c.cell_models.is_empty(), "none picked is all again");
    }

    // A harness's own effort wins over every harness's; a route's own only
    // counts until they were moved onto harnesses.
    #[test]
    fn resolved() {
        register_fakes();
        let s = |model: &str, effort: &str| Start {
            model: model.into(),
            effort: effort.into(),
            ..Start::default()
        };
        let mut d = Dispatch {
            every: s("", "high"),
            ..Dispatch::default()
        };
        d.set_harness_start("pb", s("ignored", "low"));
        d.set_start_on("pa", "pa", s("m1", "max"));
        let st = d.resolved("pa", "pa-pb");
        assert!(
            st.effort == "low" && st.model.is_empty(),
            "pa in pb: {st:?}"
        );
        let st = d.resolved("pa", "pa");
        assert!(st.effort == "high" && st.model == "m1", "pa: {st:?}");
        d.every = Start::default();
        assert_eq!(
            d.resolved("pa", "pa").effort,
            "max",
            "before the move, the route's"
        );
        d.move_harness_starts();
        assert_eq!(d.resolved("pa", "pa").effort, "max");
        assert_eq!(
            d.harness_start("pa").effort,
            "max",
            "moved onto pa: {:?}",
            d.by_harness
        );
        assert_eq!(
            approve_all_mode(&[Choice::new("default"), Choice::new("full-access")]),
            "full-access"
        );
    }
}
